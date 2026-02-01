//! Depth Pulse
//!
//! The Depth Pulse measures **route-level liquidity quality** for RFQ execution.
//!
//! ## What this pulse answers
//! > “Is the current swap route liquid relative to how liquid it has been recently?”
//!
//! This pulse is **not** a price predictor and **not** a global order-book depth.
//! It is a **route-scoped execution-capacity signal**, derived only from
//! *executable RFQ data*.
//!
//! ## Data source
//! Depth is extracted from Omniston RFQ quotes at:
//!
//! ```text
//! params.swap.routes[0].steps[0].chunks[].bid_amount
//! ```
//!
//! Each `chunk.bid_amount` represents how much input liquidity that protocol
//! is currently willing to absorb.
//!
//! ## Definition
//!
//! ```text
//! depth_now = sum(chunk.bid_amount)
//! ```
//!
//! This gives a conservative, executable approximation of how much size
//! the route can currently handle.
//!
//! ## Rolling window logic
//! The pulse tracks recent depth values in a bounded rolling window:
//!
//! - `depth_best`: maximum depth observed in the window
//! - `depth_deficit_bps`: how far current depth is below the recent peak
//!
//! ```text
//! depth_deficit_bps = (1 - depth_now / depth_best) * 10_000
//! ```
//!
//! ## Warm-up guard
//! The pulse is marked `Invalid` until:
//! - a minimum number of samples is collected
//! - the window spans a minimum duration
//!
//! This prevents execution on cold or unstable data.
//!
//! ## Design guarantees
//! - Deterministic
//! - Bounded memory
//! - O(1) max lookup
//! - Fail-safe (invalid data never allows execution)
//! - Per-pair isolated state

use std::collections::VecDeque;

use crate::pulse::input::QuoteInput;
use crate::pulse::{Pulse, PulseResult, PulseValidity};
use crate::types::Quote;

/// --- Configuration constants ---

/// Maximum age of the rolling window (milliseconds)
const WINDOW_MAX_AGE_MS: u64 = 30_000;

/// Minimum number of samples before the pulse becomes valid
const MIN_SAMPLES: usize = 10;

/// Minimum time span before the pulse becomes valid
const MIN_AGE_MS: u64 = 5_000;

/// --- Pulse output ---

/// Output of the Depth Pulse.
///
/// All fields are safe to log, compare, and threshold.
#[derive(Clone, Debug, Default)]
pub struct DepthPulseResult {
    /// Current observed depth
    pub depth_now: f64,

    /// Best depth observed in the rolling window
    pub depth_best: f64,

    /// Deficit from the recent peak, in basis points
    pub depth_deficit_bps: f64,

    /// Validity guard
    pub validity: PulseValidity,
}

impl PulseResult for DepthPulseResult {
    fn validity(&self) -> PulseValidity {
        self.validity
    }
}

/// --- Internal rolling window types ---

#[derive(Clone, Debug)]
struct TimedValue {
    ts_ms: u64,
    value: u128,
}

/// Rolling window with monotonic max queue (O(1) max lookup).
struct DepthWindow {
    values: VecDeque<TimedValue>,
    max_queue: VecDeque<TimedValue>,
    max_age_ms: u64,
}

impl DepthWindow {
    fn new(max_age_ms: u64) -> Self {
        Self {
            values: VecDeque::new(),
            max_queue: VecDeque::new(),
            max_age_ms,
        }
    }

    fn push(&mut self, ts_ms: u64, value: u128) {
        let tv = TimedValue { ts_ms, value };
        self.values.push_back(tv.clone());

        // Maintain decreasing monotonic queue
        while let Some(back) = self.max_queue.back() {
            if back.value < value {
                self.max_queue.pop_back();
            } else {
                break;
            }
        }
        self.max_queue.push_back(tv);

        self.evict_old(ts_ms);
    }

    fn evict_old(&mut self, now_ms: u64) {
        while let Some(front) = self.values.front() {
            if now_ms.saturating_sub(front.ts_ms) > self.max_age_ms {
                let removed = self.values.pop_front().unwrap();

                if let Some(max) = self.max_queue.front()
                    && max.ts_ms == removed.ts_ms
                    && max.value == removed.value
                {
                    self.max_queue.pop_front();
                }
            } else {
                break;
            }
        }
    }

    fn max(&self) -> Option<u128> {
        self.max_queue.front().map(|v| v.value)
    }

    fn is_warm(&self) -> bool {
        if self.values.len() < MIN_SAMPLES {
            return false;
        }

        let age = self.values.back().unwrap().ts_ms - self.values.front().unwrap().ts_ms;

        age >= MIN_AGE_MS
    }
}

/// --- Pulse state ---

/// Stateful Depth Pulse (per trading pair).
pub struct DepthPulse {
    window: DepthWindow,
}

impl Default for DepthPulse {
    fn default() -> Self {
        Self {
            window: DepthWindow::new(WINDOW_MAX_AGE_MS),
        }
    }
}

impl Pulse for DepthPulse {
    type Input = QuoteInput;
    type Output = DepthPulseResult;

    fn evaluate(&mut self, input: Self::Input) -> Self::Output {
        compute_depth(input.ts_ms, &input.quote, &mut self.window)
    }
}

/// --- Core logic ---

fn compute_depth(ts_ms: u64, quote: &Quote, window: &mut DepthWindow) -> DepthPulseResult {
    let depth_now = extract_depth(quote).unwrap_or(0);

    // Fail-safe: zero or missing depth blocks execution
    if depth_now == 0 {
        return DepthPulseResult {
            depth_now: 0.0,
            depth_best: 0.0,
            depth_deficit_bps: f64::MAX,
            validity: PulseValidity::Invalid,
        };
    }

    window.push(ts_ms, depth_now);

    // Warm-up guard
    if !window.is_warm() {
        return DepthPulseResult {
            depth_now: depth_now as f64,
            depth_best: depth_now as f64,
            depth_deficit_bps: f64::MAX,
            validity: PulseValidity::Invalid,
        };
    }

    let depth_best = window.max().unwrap_or(depth_now) as f64;
    let depth_now_f = depth_now as f64;

    let deficit = (1.0 - depth_now_f / depth_best) * 10_000.0;

    DepthPulseResult {
        depth_now: depth_now_f,
        depth_best,
        depth_deficit_bps: deficit.max(0.0),
        validity: PulseValidity::Valid,
    }
}

/// Extract executable liquidity depth from an RFQ quote.
///
/// We intentionally take:
/// - the **first route**
/// - the **first step**
///
/// because Omniston already orders routes by best execution.
/// Summing `bid_amount` across chunks gives total executable capacity.
fn extract_depth(quote: &Quote) -> Option<u128> {
    let swap = quote.params.swap.as_ref()?;
    let route = swap.routes.first()?;
    let step = route.steps.first()?;

    let mut total = 0u128;

    for chunk in &step.chunks {
        if let Ok(v) = chunk.bid_amount.parse::<u128>() {
            total = total.saturating_add(v);
        }
    }

    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn ts(n: u64) -> u64 {
        n * 1000
    }

    fn dummy_addr() -> AssetAddress {
        AssetAddress {
            blockchain: 607,
            address: "EQTEST".into(),
        }
    }

    fn mk_quote(depths: &[&str]) -> Quote {
        let chunks = depths
            .iter()
            .map(|d| RouteChunk {
                protocol: "Test".into(),
                bid_amount: (*d).into(),
                ask_amount: "0".into(),
                extra_version: 1,
                extra: vec![],
            })
            .collect();

        let route = Route {
            steps: vec![RouteStep {
                bid_asset_address: dummy_addr(),
                ask_asset_address: dummy_addr(),
                chunks,
            }],
        };

        Quote {
            quote_id: "q".into(),
            resolver_id: "r".into(),
            resolver_name: "Omniston".into(),
            bid_asset_address: dummy_addr(),
            ask_asset_address: dummy_addr(),
            bid_units: "100".into(),
            ask_units: "200".into(),
            referrer_address: None,
            referrer_fee_asset: dummy_addr(),
            referrer_fee_units: "0".into(),
            protocol_fee_asset: dummy_addr(),
            protocol_fee_units: "0".into(),
            quote_timestamp: 0,
            trade_start_deadline: 0,
            gas_budget: "0".into(),
            estimated_gas_consumption: "0".into(),
            params: QuoteParams {
                swap: Some(SwapParams {
                    routes: vec![route],
                    min_ask_amount: "0".into(),
                    recommended_min_ask_amount: "0".into(),
                    recommended_slippage_bps: 0,
                }),
            },
        }
    }

    #[test]
    fn sums_all_chunks() {
        let mut w = DepthWindow::new(WINDOW_MAX_AGE_MS);
        let q = mk_quote(&["100", "200", "300"]);

        let r = compute_depth(ts(1), &q, &mut w);
        assert_eq!(r.depth_now, 600.0);
    }

    #[test]
    fn deficit_increases_when_depth_drops() {
        let mut w = DepthWindow::new(WINDOW_MAX_AGE_MS);

        for i in 0..10 {
            compute_depth(ts(i), &mk_quote(&["1000"]), &mut w);
        }

        let r1 = compute_depth(ts(11), &mk_quote(&["800"]), &mut w);
        let r2 = compute_depth(ts(12), &mk_quote(&["400"]), &mut w);

        assert!(r1.depth_deficit_bps < r2.depth_deficit_bps);
    }

    #[test]
    fn old_peak_expires() {
        let mut w = DepthWindow::new(3_000);

        for i in 0..5 {
            compute_depth(ts(i), &mk_quote(&["2000"]), &mut w);
        }

        let r = compute_depth(ts(10), &mk_quote(&["800"]), &mut w);
        assert!(r.depth_best <= 800.0);
    }

    #[test]
    fn large_numbers_do_not_overflow() {
        let mut w = DepthWindow::new(WINDOW_MAX_AGE_MS);
        let q = mk_quote(&["1000000000000000000"]);

        let r = compute_depth(ts(1), &q, &mut w);
        assert!(r.depth_now > 0.0);
    }
}

//! Depth Pulse
//!
//! Goal: execute micro-swaps when **route liquidity depth improves**.
//!
//! In Omniston RFQ `quote_updated`, depth information lives inside:
//! `params.swap.routes[].steps[].chunks[]`
//!
//! Each `chunk` includes `bid_amount` and `ask_amount` as strings.
//! While the exact economic meaning depends on the resolver/protocol,
//! a practical MVP definition of "depth" for execution-quality is:
//!
//! ```text
//! depth_now = sum(bid_amount) across chunks in the best route step
//! ```
//!
//! Intuition:
//! - Larger chunk capacity implies the route can absorb more input with less impact.
//! - We only need a *relative* signal over a short window for a pulse trigger.
//!
//! Pulse definition used in v0.1:
//! - Maintain a rolling window of recent `depth_now` values per pair.
//! - Define `depth_best = max(depth_now over window)`.
//! - Compute deficit from peak:
//!       depth_deficit_bps = (1 - depth_now/depth_best) * 10_000
//! - The pulse is "good" when `depth_deficit_bps` is small (depth is close to peak).
//!
//! Warm-up safeguard:
//! - Until we have enough samples and enough time span, mark pulse `Invalid`,
//!   to prevent firing on first tick.
//!
//! Notes:
//! - This is *not* a global order-book depth. It's a route-level liquidity proxy.
//! - For v0.1 it is good enough and explainable.
//! - Later you can refine to: min step depth across hops, or use ask_amount, etc.

use std::collections::VecDeque;

use crate::types::Quote;

/// Validity marker used to prevent the scheduler firing on the first few ticks.
///
/// Reuse your existing one if you already defined it elsewhere.
/// If you moved it into another module, update this import accordingly.
use crate::pulse::PulseValidity;

/// Depth pulse output (debuggable and loggable).
#[derive(Clone, Debug)]
pub struct DepthPulseResult {
    /// Current observed depth (in bid units, aggregated as u128, then cast for reporting)
    pub depth_now: f64,

    /// Best depth seen in the rolling window
    pub depth_best: f64,

    /// How far we are below the best depth, in basis points:
    /// 0 bps => depth_now == depth_best
    /// 10000 bps => depth_now == 0 (worst)
    pub depth_deficit_bps: f64,

    /// Warm-up validity
    pub validity: PulseValidity,
}

/// A timestamped value for rolling calculations.
#[derive(Clone, Debug)]
struct TimedValue {
    ts_ms: u64,
    value: u128,
}

/// Rolling window + monotonic max queue (for `max()` in O(1)).
#[derive(Default)]
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

    fn push(&mut self, ts_ms: u64, depth: u128) {
        let tv = TimedValue {
            ts_ms,
            value: depth,
        };

        self.values.push_back(tv.clone());

        // Maintain decreasing max-queue (front = current max)
        while let Some(back) = self.max_queue.back() {
            if back.value < depth {
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
                let removed = self.values.pop_front().expect("front exists");

                if let Some(max_front) = self.max_queue.front()
                    && max_front.ts_ms == removed.ts_ms
                    && max_front.value == removed.value
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

    fn len(&self) -> usize {
        self.values.len()
    }

    fn age_ms(&self) -> Option<u64> {
        let oldest = self.values.front()?.ts_ms;
        let newest = self.values.back()?.ts_ms;
        Some(newest.saturating_sub(oldest))
    }

    fn is_warm(&self, min_samples: usize, min_age_ms: u64) -> bool {
        if self.len() < min_samples {
            return false;
        }
        self.age_ms().unwrap_or(0) >= min_age_ms
    }
}

/// Extract a liquidity "depth" proxy from an Omniston quote.
///
/// MVP definition:
/// - Take `routes[0].steps[0].chunks`
/// - depth_now = sum(chunk.bid_amount)
///
/// If no swap params exist, returns None.
fn extract_depth_bid_units(quote: &Quote) -> Option<u128> {
    let swap = quote.params.swap.as_ref()?;

    let route0 = swap.routes.get(0)?;
    let step0 = route0.steps.get(0)?;

    let mut total: u128 = 0;

    for ch in step0.chunks.iter() {
        // bid_amount is a string, can be large.
        // If parsing fails, ignore that chunk (don't crash the engine).
        if let Ok(v) = ch.bid_amount.parse::<u128>() {
            total = total.saturating_add(v);
        }
    }

    Some(total)
}

/// Compute the Depth Pulse for one tick.
///
/// Inputs:
/// - `ts_ms`: timestamp of the quote tick (monotonic ms preferred)
/// - `quote`: the Omniston Quote (already parsed/typed)
/// - `window`: rolling window state for this pair
///
/// Output:
/// - `DepthPulseResult` with validity gating
pub fn compute_depth_pulse(
    ts_ms: u64,
    quote: &Quote,
    window: &mut DepthWindow,
) -> DepthPulseResult {
    const WINDOW_MAX_AGE_MS: u64 = 30_000;
    const MIN_SAMPLES: usize = 10;
    const MIN_AGE_MS: u64 = 5_000;

    // Ensure window has correct max age (if caller created with default)
    window.max_age_ms = WINDOW_MAX_AGE_MS;

    // If we can't extract depth from the quote, treat pulse as invalid for safety.
    let depth_now_u128 = match extract_depth_bid_units(quote) {
        Some(v) if v > 0 => v,
        _ => {
            return DepthPulseResult {
                depth_now: 0.0,
                depth_best: 0.0,
                depth_deficit_bps: f64::MAX,
                validity: PulseValidity::Invalid,
            };
        }
    };

    // Update rolling window
    window.push(ts_ms, depth_now_u128);

    // Warm-up safeguard
    if !window.is_warm(MIN_SAMPLES, MIN_AGE_MS) {
        return DepthPulseResult {
            depth_now: depth_now_u128 as f64,
            depth_best: depth_now_u128 as f64,
            depth_deficit_bps: f64::MAX,
            validity: PulseValidity::Invalid,
        };
    }

    // Compute best depth in window
    let depth_best_u128 = window.max().unwrap_or(depth_now_u128);

    // Convert to f64 only for ratio math
    let depth_now = depth_now_u128 as f64;
    let depth_best = depth_best_u128 as f64;

    // deficit_bps = (1 - now/best)*10000
    let depth_deficit_bps = if depth_best > 0.0 {
        (1.0 - (depth_now / depth_best)) * 10_000.0
    } else {
        f64::MAX
    };

    DepthPulseResult {
        depth_now,
        depth_best,
        depth_deficit_bps,
        validity: PulseValidity::Valid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn dummy_addr(addr: &str) -> AssetAddress {
        AssetAddress {
            blockchain: 607,
            address: addr.into(),
        }
    }

    fn mk_quote_with_depth(depths: &[&str]) -> Quote {
        let chunks = depths
            .iter()
            .map(|d| RouteChunk {
                protocol: "StonFiV2".into(),
                bid_amount: (*d).into(),
                ask_amount: "0".into(),
                extra_version: 1,
                extra: vec![],
            })
            .collect::<Vec<_>>();

        let route = Route {
            steps: vec![RouteStep {
                bid_asset_address: dummy_addr("EQBID"),
                ask_asset_address: dummy_addr("EQASK"),
                chunks,
            }],
        };

        Quote {
            quote_id: "q".into(),
            resolver_id: "r".into(),
            resolver_name: "Omniston".into(),

            bid_asset_address: dummy_addr("EQBID"),
            ask_asset_address: dummy_addr("EQASK"),

            bid_units: "100".into(),
            ask_units: "200".into(),

            referrer_address: None,
            referrer_fee_asset: dummy_addr("EQFEE"),
            referrer_fee_units: "0".into(),
            protocol_fee_asset: dummy_addr("EQPROT"),
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

    fn ts(n: u64) -> u64 {
        n * 1000
    }

    #[test]
    fn extract_depth_sums_bid_amounts() {
        let q = mk_quote_with_depth(&["10", "20", "30"]);
        let d = extract_depth_bid_units(&q).unwrap();
        assert_eq!(d, 60);
    }

    #[test]
    fn invalid_when_no_swap_params() {
        let mut q = mk_quote_with_depth(&["10"]);
        q.params.swap = None;

        let mut w = DepthWindow::new(30_000);
        let res = compute_depth_pulse(ts(1), &q, &mut w);

        assert!(matches!(res.validity, PulseValidity::Invalid));
        assert_eq!(res.depth_deficit_bps, f64::MAX);
    }

    #[test]
    fn warmup_blocks_first_ticks() {
        let q = mk_quote_with_depth(&["100"]);

        let mut w = DepthWindow::new(30_000);

        // First tick => invalid
        let r1 = compute_depth_pulse(ts(1), &q, &mut w);
        assert!(matches!(r1.validity, PulseValidity::Invalid));

        // Still likely invalid without enough samples/age
        let r2 = compute_depth_pulse(ts(2), &q, &mut w);
        assert!(matches!(r2.validity, PulseValidity::Invalid));
    }

    #[test]
    fn valid_after_warmup_and_deficit_is_zero_at_peak() {
        let mut w = DepthWindow::new(30_000);

        // Create increasing depth values over >5s and >=10 samples
        for i in 0..10 {
            let q = mk_quote_with_depth(&[&(100 + i).to_string()]);
            let _ = compute_depth_pulse(ts(i as u64), &q, &mut w);
        }

        // By now, window should be warm (10 samples, 9s age)
        let q_peak = mk_quote_with_depth(&["200"]);
        let res = compute_depth_pulse(ts(12), &q_peak, &mut w);

        // If it's peak, deficit should be 0 (or extremely close)
        assert!(matches!(res.validity, PulseValidity::Valid));
        assert!(res.depth_deficit_bps.abs() < 1e-9);
    }

    #[test]
    fn deficit_positive_when_depth_below_peak() {
        let mut w = DepthWindow::new(30_000);

        // Warm the window with a strong peak
        for i in 0..10 {
            let q = mk_quote_with_depth(&["1000"]);
            let _ = compute_depth_pulse(ts(i as u64), &q, &mut w);
        }

        // Now a lower depth tick
        let q_low = mk_quote_with_depth(&["500"]);
        let res = compute_depth_pulse(ts(12), &q_low, &mut w);

        assert!(matches!(res.validity, PulseValidity::Valid));
        assert!(res.depth_deficit_bps > 0.0);
    }
}

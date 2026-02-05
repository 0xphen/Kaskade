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
//! Depth is extracted from Omniston RFQ quotes based on the provided `ExecutionScope`.
//! - In `MarketWide` scope, it uses the total `bid_units` of the quote.
//! - In `ProtocolOnly` scope, it iterates through all routes and picks the route
//!   with the maximum protocol-specific bid depth.
//!
//! ## Definition
//!
//! ```text
//! depth_now = sum(chunk.bid_amount) // across the best available protocol route
//! ```
//!
//! ## Rolling window logic
//! The pulse tracks recent depth values in a bounded rolling window:
//! - `depth_best`: maximum depth observed in the window
//! - `depth_deficit_bps`: how far current depth is below the recent peak
//!
//! ## Warm-up guard
//! The pulse is marked `Invalid` until a minimum number of samples and time span are met.

use std::collections::VecDeque;

use super::input::QuoteInput;
use super::{Pulse, PulseResult, PulseValidity};
use crate::market::types::{ExecutionScope, Quote};

/// Maximum age of the rolling window (milliseconds)
const WINDOW_MAX_AGE_MS: u64 = 30_000;

/// Minimum number of samples before the pulse becomes valid
const MIN_SAMPLES: usize = 10;

/// Minimum time span before the pulse becomes valid
const MIN_AGE_MS: u64 = 5_000;

/// Output of the Depth Pulse.
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

/// Stateful Depth Pulse (per trading pair).
pub struct DepthPulse {
    window: DepthWindow,
    scope: ExecutionScope,
}

impl DepthPulse {
    pub fn new(scope: ExecutionScope) -> Self {
        Self {
            window: DepthWindow::new(WINDOW_MAX_AGE_MS),
            scope,
        }
    }
}

impl Default for DepthPulse {
    fn default() -> Self {
        Self::new(ExecutionScope::MarketWide)
    }
}

impl Pulse for DepthPulse {
    type Input = QuoteInput;
    type Output = DepthPulseResult;

    fn evaluate(&mut self, input: Self::Input) -> Self::Output {
        compute_depth(input.ts_ms, &input.quote, &mut self.window, &self.scope)
    }
}

fn compute_depth(
    ts_ms: u64,
    quote: &Quote,
    window: &mut DepthWindow,
    scope: &ExecutionScope,
) -> DepthPulseResult {
    let depth_now = match scope {
        ExecutionScope::MarketWide => extract_market_depth(quote),
        ExecutionScope::ProtocolOnly { protocol } => extract_protocol_depth(quote, protocol),
    }
    .unwrap_or(0);

    // Fail-safe: zero depth blocks execution
    if depth_now == 0 {
        return DepthPulseResult {
            depth_now: 0.0,
            depth_best: 0.0,
            depth_deficit_bps: f64::MAX,
            validity: PulseValidity::Invalid,
        };
    }

    window.push(ts_ms, depth_now);

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

/// Extracts total market depth from the top-level quote units.
fn extract_market_depth(quote: &Quote) -> Option<u128> {
    quote.bid_units.parse::<u128>().ok()
}

/// Extracts protocol-specific depth by finding the route with the highest liquidity for that protocol.
fn extract_protocol_depth(quote: &Quote, protocol_name: &str) -> Option<u128> {
    let mut best_bid = 0u128;
    let swap = quote.params.swap.as_ref()?;

    for route in &swap.routes {
        let mut route_bid = 0u128;
        for step in &route.steps {
            for chunk in &step.chunks {
                if chunk.protocol.contains(protocol_name)
                    && let Ok(v) = chunk.bid_amount.parse::<u128>()
                {
                    route_bid = route_bid.saturating_add(v);
                }
            }
        }
        if route_bid > best_bid {
            best_bid = route_bid;
        }
    }

    if best_bid > 0 { Some(best_bid) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::types::*;

    fn ts(n: u64) -> u64 {
        n * 1000
    }
    fn dummy_addr() -> AssetAddress {
        AssetAddress {
            blockchain: 607,
            address: "EQTEST".into(),
        }
    }

    fn mk_quote_complex(routes_data: Vec<Vec<(&str, &str)>>) -> Quote {
        let routes = routes_data
            .into_iter()
            .map(|chunks_data| Route {
                steps: vec![RouteStep {
                    bid_asset_address: dummy_addr(),
                    ask_asset_address: dummy_addr(),
                    chunks: chunks_data
                        .into_iter()
                        .map(|(p, b)| RouteChunk {
                            protocol: p.into(),
                            bid_amount: b.into(),
                            ask_amount: "0".into(),
                            extra_version: 1,
                            extra: vec![],
                        })
                        .collect(),
                }],
            })
            .collect();

        Quote {
            quote_id: "q".into(),
            resolver_id: "r".into(),
            resolver_name: "Omniston".into(),
            bid_asset_address: dummy_addr(),
            ask_asset_address: dummy_addr(),
            bid_units: "1000".into(),
            ask_units: "2000".into(),
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
                    routes,
                    min_ask_amount: "0".into(),
                    recommended_min_ask_amount: "0".into(),
                    recommended_slippage_bps: 0,
                }),
            },
        }
    }

    #[test]
    fn test_depth_protocol_selection() {
        let mut w = DepthWindow::new(WINDOW_MAX_AGE_MS);
        let scope = ExecutionScope::ProtocolOnly {
            protocol: "StonFi".into(),
        };

        // Route 0: DeDust 500
        // Route 1: StonFi 300
        let q = mk_quote_complex(vec![vec![("DeDust", "500")], vec![("StonFi", "300")]]);

        let r = compute_depth(ts(1), &q, &mut w, &scope);
        // Should pick StonFi from Route 1
        assert_eq!(r.depth_now, 300.0);
    }

    #[test]
    fn test_depth_multi_hop_aggregation() {
        let mut w = DepthWindow::new(WINDOW_MAX_AGE_MS);
        let scope = ExecutionScope::ProtocolOnly {
            protocol: "StonFi".into(),
        };

        // Single route with two StonFi chunks
        let q = mk_quote_complex(vec![vec![("StonFiV1", "100"), ("StonFiV2", "200")]]);

        let r = compute_depth(ts(1), &q, &mut w, &scope);
        assert_eq!(r.depth_now, 300.0);
    }

    #[test]
    fn test_market_wide_uses_top_level_bid() {
        let mut w = DepthWindow::new(WINDOW_MAX_AGE_MS);
        let scope = ExecutionScope::MarketWide;

        let q = mk_quote_complex(vec![vec![("StonFi", "100")]]);
        // bid_units in mk_quote_complex is "1000"
        let r = compute_depth(ts(1), &q, &mut w, &scope);
        assert_eq!(r.depth_now, 1000.0);
    }
}

use super::input::QuoteInput;
use super::{Pulse, PulseResult, PulseValidity};
use crate::market::omniston::normalized_quote::NormalizedQuote;
use crate::market::rolling_window::RollingWindow;
use crate::market::types::ExecutionScope;

/// Spread Pulse
///
/// The Spread Pulse measures **price degradation relative to the best recent
/// executable price** within a specific execution scope (e.g., Market-wide or Ston.fi only).
///
/// It answers: "How much worse is the current quote compared to the best price we've seen recently
/// in the pool(s) we are allowed to use?"
#[derive(Clone, Debug)]
pub struct SpreadPulseResult {
    /// Current price (ask_units / bid_units) for the active scope.
    pub p_now: f64,

    /// Best observed price in the rolling window for the active scope.
    pub p_best: f64,

    /// Spread relative to best price, in basis points.
    pub spread_bps: f64,

    /// Validity guard.
    pub validity: PulseValidity,
}

impl Default for SpreadPulseResult {
    fn default() -> Self {
        Self {
            p_now: 0.0,
            p_best: 0.0,
            spread_bps: f64::MAX,
            validity: PulseValidity::Invalid,
        }
    }
}

impl PulseResult for SpreadPulseResult {
    fn validity(&self) -> PulseValidity {
        self.validity
    }
}

/// Warm-up configuration for the Spread Pulse.
#[derive(Clone, Copy, Debug)]
pub struct SpreadWarmup {
    pub min_samples: usize,
    pub min_age_ms: u64,
}

impl Default for SpreadWarmup {
    fn default() -> Self {
        Self {
            min_samples: super::MIN_SAMPLES,
            min_age_ms: super::MIN_AGE_MS,
        }
    }
}

/// Spread pulse state.
pub struct SpreadPulse {
    window: RollingWindow,
    warmup: SpreadWarmup,
    scope: ExecutionScope,
}

impl SpreadPulse {
    pub fn new(warmup: SpreadWarmup, scope: ExecutionScope) -> Self {
        Self {
            window: RollingWindow::new(),
            warmup,
            scope,
        }
    }
}

impl Pulse for SpreadPulse {
    type Input = QuoteInput;
    type Output = SpreadPulseResult;

    fn evaluate(&mut self, input: Self::Input) -> Self::Output {
        // Normalize and filter the quote based on the ExecutionScope.
        let normalized = NormalizedQuote::from_event(&input.quote, &self.scope);

        compute_spread(
            normalized.ts_ms,
            normalized.bid_units,
            normalized.ask_units,
            &mut self.window,
            self.warmup,
        )
    }
}

/// Core spread computation logic.
///
/// This is isolated from the input type to keep the rolling window logic pure.
fn compute_spread(
    ts_ms: u64,
    bid_units: f64,
    ask_units: f64,
    window: &mut RollingWindow,
    warmup: SpreadWarmup,
) -> SpreadPulseResult {
    // Fail-safe: If bid_units is 0 (parsing error or protocol not found in quote),
    // we return Invalid to block execution.
    if bid_units <= 0.0 {
        return SpreadPulseResult::default();
    }

    let p_now = ask_units / bid_units;
    window.push(ts_ms, p_now);

    // Warm-up check: Ensure we have enough data to determine what a "good" price looks like.
    if !window.is_warm(warmup.min_samples, warmup.min_age_ms) {
        return SpreadPulseResult {
            p_now,
            p_best: p_now,
            spread_bps: f64::MAX,
            validity: PulseValidity::Invalid,
        };
    }

    // p_best is the maximum execution rate (most output per 1 unit input) seen in the window.
    let p_best = window.max().unwrap_or(p_now);

    // Calculate basis points: ((Best - Now) / Best) * 10,000
    let spread_bps = if p_best > 0.0 {
        ((p_best - p_now) / p_best) * 10_000.0
    } else {
        f64::MAX
    };

    SpreadPulseResult {
        p_now,
        p_best,
        spread_bps,
        validity: PulseValidity::Valid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::types::*;
    use std::sync::Arc;

    fn dummy_addr() -> AssetAddress {
        AssetAddress {
            blockchain: 607,
            address: "EQDummy".into(),
        }
    }

    fn warmup() -> SpreadWarmup {
        SpreadWarmup {
            min_samples: 2,
            min_age_ms: 1000,
        }
    }

    /// Helper to create a QuoteInput with multiple alternative routes/chunks.
    /// Wraps the Quote in an Arc to match the production QuoteInput definition.
    fn mk_multi_protocol_quote(
        ts: u64,
        market_bid: &str,
        market_ask: &str,
        stonfi_bid: &str,
        stonfi_ask: &str,
    ) -> QuoteInput {
        let route = Route {
            steps: vec![RouteStep {
                bid_asset_address: dummy_addr(),
                ask_asset_address: dummy_addr(),
                chunks: vec![
                    RouteChunk {
                        protocol: "StonFi".into(),
                        bid_amount: stonfi_bid.into(),
                        ask_amount: stonfi_ask.into(),
                        extra_version: 1,
                        extra: vec![],
                    },
                    RouteChunk {
                        protocol: "DeDust".into(),
                        bid_amount: "1000".into(),
                        ask_amount: "1100".into(),
                        extra_version: 1,
                        extra: vec![],
                    },
                ],
            }],
        };

        QuoteInput {
            ts_ms: ts * 1000,
            quote: Arc::new(Quote {
                quote_id: "q".into(),
                resolver_id: "r".into(),
                resolver_name: "Omniston".into(),
                bid_asset_address: dummy_addr(),
                ask_asset_address: dummy_addr(),
                bid_units: market_bid.into(),
                ask_units: market_ask.into(),
                referrer_address: None,
                referrer_fee_asset: dummy_addr(),
                referrer_fee_units: "0".into(),
                protocol_fee_asset: dummy_addr(),
                protocol_fee_units: "0".into(),
                quote_timestamp: ts,
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
            }),
        }
    }

    #[test]
    fn test_spread_isolation_protocol_only() {
        let scope = ExecutionScope::ProtocolOnly {
            protocol: "StonFi".into(),
        };
        let mut pulse = SpreadPulse::new(warmup(), scope);

        // T=0: StonFi Price = 0.8. Market Total = 1.0.
        let input1 = mk_multi_protocol_quote(0, "100", "100", "100", "80");
        // T=1: StonFi Price = 0.8. Market Total = 0.9.
        let input2 = mk_multi_protocol_quote(1, "100", "90", "100", "80");

        pulse.evaluate(input1);
        let result = pulse.evaluate(input2);

        // Because we are scoped to StonFi, the global market price move (1.0 -> 0.9) is ignored.
        // StonFi's price stayed at 0.8, so spread should be 0 bps.
        assert_eq!(result.validity, PulseValidity::Valid);
        assert_eq!(result.spread_bps, 0.0);
        assert_eq!(result.p_best, 0.8);
    }

    #[test]
    fn test_spread_market_wide() {
        let mut pulse = SpreadPulse::new(warmup(), ExecutionScope::MarketWide);

        // T=0: Price 1.0
        let input1 = mk_multi_protocol_quote(0, "100", "100", "100", "80");
        // T=1: Price 0.9
        let input2 = mk_multi_protocol_quote(1, "100", "90", "100", "80");

        pulse.evaluate(input1);
        let result = pulse.evaluate(input2);

        // In MarketWide scope, we use 100/100 -> 90/100.
        // Best was 1.0, Now is 0.9. Spread = (1.0 - 0.9) / 1.0 * 10,000 = 1,000 bps.
        assert_eq!(result.validity, PulseValidity::Valid);
        assert!((result.spread_bps - 1000.0).abs() < 1e-9);
    }
}

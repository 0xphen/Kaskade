use super::input::QuoteInput;
use super::{Pulse, PulseResult, PulseValidity};
use crate::market::omniston::normalized_quote::NormalizedQuote;
use crate::market::rolling_window::RollingWindow;
use crate::market::types::ExecutionScope;

/// Trend Pulse
///
/// The Trend Pulse measures **price directionality over time** within a specific
/// execution scope (e.g., Market-wide or Ston.fi only).
///
/// It answers: “Is the execution price getting worse compared to where it started recently?”
#[derive(Clone, Debug)]
pub struct TrendPulseResult {
    /// Current price for the active scope.
    pub p_now: f64,

    /// Oldest price in the rolling window for the active scope.
    pub p_oldest: f64,

    /// Downward price movement in basis points.
    pub trend_drop_bps: f64,

    /// Validity guard.
    pub validity: PulseValidity,
}

impl Default for TrendPulseResult {
    fn default() -> Self {
        Self {
            p_now: 0.0,
            p_oldest: 0.0,
            trend_drop_bps: f64::MAX,
            validity: PulseValidity::Invalid,
        }
    }
}

impl PulseResult for TrendPulseResult {
    fn validity(&self) -> PulseValidity {
        self.validity
    }
}

/// Warm-up configuration for the Trend Pulse.
#[derive(Clone, Copy, Debug)]
pub struct TrendWarmup {
    pub min_samples: usize,
    pub min_age_ms: u64,
}

impl Default for TrendWarmup {
    fn default() -> Self {
        Self {
            min_samples: super::MIN_SAMPLES,
            min_age_ms: super::MIN_AGE_MS,
        }
    }
}

/// Trend pulse state.
pub struct TrendPulse {
    window: RollingWindow,
    warmup: TrendWarmup,
    scope: ExecutionScope,
}

impl TrendPulse {
    pub fn new(warmup: TrendWarmup, scope: ExecutionScope) -> Self {
        Self {
            window: RollingWindow::new(),
            warmup,
            scope,
        }
    }
}

impl Pulse for TrendPulse {
    type Input = QuoteInput;
    type Output = TrendPulseResult;

    fn evaluate(&mut self, input: Self::Input) -> Self::Output {
        // Normalize and filter the quote based on the ExecutionScope.
        let normalized = NormalizedQuote::from_event(&input.quote, &self.scope);

        compute_trend(
            normalized.ts_ms,
            normalized.bid_units,
            normalized.ask_units,
            &mut self.window,
            self.warmup,
        )
    }
}

/// Core trend computation.
fn compute_trend(
    ts_ms: u64,
    bid_units: f64,
    ask_units: f64,
    window: &mut RollingWindow,
    warmup: TrendWarmup,
) -> TrendPulseResult {
    // Fail-safe: Non-positive bid units (protocol not found or invalid data) blocks execution.
    if bid_units <= 0.0 {
        return TrendPulseResult::default();
    }

    let p_now = ask_units / bid_units;
    window.push(ts_ms, p_now);

    if !window.is_warm(warmup.min_samples, warmup.min_age_ms) {
        return TrendPulseResult {
            p_now,
            p_oldest: p_now,
            trend_drop_bps: f64::MAX,
            validity: PulseValidity::Invalid,
        };
    }

    // Trend compares the current price to the OLDEST price in the window.
    let p_oldest = window.oldest().unwrap_or(p_now);

    let trend_drop_bps = if p_oldest > 0.0 {
        ((p_oldest - p_now) / p_oldest) * 10_000.0
    } else {
        f64::MAX
    };

    TrendPulseResult {
        p_now,
        p_oldest,
        trend_drop_bps,
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

    fn warmup() -> TrendWarmup {
        TrendWarmup {
            min_samples: 2,
            min_age_ms: 1000,
        }
    }

    fn mk_quote(ts: u64, protocol: &str, bid: &str, ask: &str) -> QuoteInput {
        let route = Route {
            steps: vec![RouteStep {
                bid_asset_address: dummy_addr(),
                ask_asset_address: dummy_addr(),
                chunks: vec![RouteChunk {
                    protocol: protocol.into(),
                    bid_amount: bid.into(),
                    ask_amount: ask.into(),
                    extra_version: 1,
                    extra: vec![],
                }],
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
                bid_units: bid.into(),
                ask_units: ask.into(),
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
    fn test_trend_protocol_isolation() {
        let scope = ExecutionScope::ProtocolOnly {
            protocol: "StonFi".into(),
        };
        let mut pulse = TrendPulse::new(warmup(), scope);

        // T=0: StonFi Price = 0.50
        pulse.evaluate(mk_quote(0, "StonFi", "100", "50"));
        // T=1: Price drops to 0.40. MarketWide would see this, but we filter only StonFi.
        let r = pulse.evaluate(mk_quote(1, "StonFi", "100", "40"));

        assert_eq!(r.validity, PulseValidity::Valid);
        // (0.50 - 0.40) / 0.50 * 10,000 = 2,000 bps
        assert!((r.trend_drop_bps - 2000.0).abs() < 1e-9);
    }

    #[test]
    fn test_trend_invalid_on_protocol_mismatch() {
        let scope = ExecutionScope::ProtocolOnly {
            protocol: "StonFi".into(),
        };
        let mut pulse = TrendPulse::new(warmup(), scope);

        // Provide a DeDust quote to a StonFi-scoped pulse
        let r = pulse.evaluate(mk_quote(0, "DeDust", "100", "50"));

        assert_eq!(r.validity, PulseValidity::Invalid);
        assert_eq!(r.trend_drop_bps, f64::MAX);
    }

    #[test]
    fn test_trend_directionality() {
        let mut window = RollingWindow::new();
        let wup = warmup();

        // T=0: Price 1.0 (Oldest)
        compute_trend(0, 100.0, 100.0, &mut window, wup);
        // T=1.1: Price 1.2 (Improving - should result in negative bps)
        let r = compute_trend(1100, 100.0, 120.0, &mut window, wup);

        assert_eq!(r.validity, PulseValidity::Valid);
        assert!(r.trend_drop_bps < 0.0);
    }
}

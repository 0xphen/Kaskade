use super::input::QuoteInput;
use super::{Pulse, PulseResult, PulseValidity};
use crate::market::omniston::normalized_quote::NormalizedQuote;
use crate::market::types::{ExecutionScope, Quote};

/// Slippage Pulse
///
/// Measures execution quality degradation for a single RFQ.
/// In `ProtocolOnly` scope, it calculates a synthetic slippage based on the
/// resolver's recommended basis points.
#[derive(Clone, Debug)]
pub struct SlippagePulseResult {
    /// Slippage expressed in basis points.
    pub slippage_bps: f64,

    /// Validity guard.
    pub validity: PulseValidity,
}

impl Default for SlippagePulseResult {
    fn default() -> Self {
        Self {
            slippage_bps: f64::MAX,
            validity: PulseValidity::Invalid,
        }
    }
}

impl PulseResult for SlippagePulseResult {
    fn validity(&self) -> PulseValidity {
        self.validity
    }
}

/// Stateless Slippage Pulse.
pub struct SlippagePulse {
    scope: ExecutionScope,
}

impl SlippagePulse {
    pub fn new(scope: ExecutionScope) -> Self {
        Self { scope }
    }
}

impl Default for SlippagePulse {
    fn default() -> Self {
        Self::new(ExecutionScope::MarketWide)
    }
}

impl Pulse for SlippagePulse {
    type Input = QuoteInput;
    type Output = SlippagePulseResult;

    fn evaluate(&mut self, input: Self::Input) -> Self::Output {
        compute_slippage(&input.quote, &self.scope)
    }
}

/// Core slippage computation.
fn compute_slippage(quote: &Quote, scope: &ExecutionScope) -> SlippagePulseResult {
    let swap = match &quote.params.swap {
        Some(s) => s,
        None => return SlippagePulseResult::default(),
    };

    // Normalize units based on scope (Market-wide total vs Protocol-only chunks)
    let normalized = NormalizedQuote::from_event(quote, scope);
    let ask_units = normalized.ask_units;

    if ask_units <= 0.0 {
        return SlippagePulseResult::default();
    }

    let slippage_bps = match scope {
        ExecutionScope::MarketWide => {
            // Use the global resolver-enforced minimum
            let min_ask: f64 = swap.min_ask_amount.parse().unwrap_or(0.0);
            if min_ask > ask_units {
                return SlippagePulseResult::default();
            }
            ((ask_units - min_ask) / ask_units) * 10_000.0
        }
        ExecutionScope::ProtocolOnly { .. } => {
            // Synthetic slippage: We use the resolver's recommended limit
            // as our benchmark for the isolated protocol leg.
            swap.recommended_slippage_bps as f64
        }
    };

    SlippagePulseResult {
        slippage_bps,
        validity: PulseValidity::Valid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::types::*;

    fn dummy_addr() -> AssetAddress {
        AssetAddress {
            blockchain: 607,
            address: "EQDummy".into(),
        }
    }

    fn mk_quote(ask: &str, min_ask: &str, rec_slip: u32, protocol: &str) -> Quote {
        let route = Route {
            steps: vec![RouteStep {
                bid_asset_address: dummy_addr(),
                ask_asset_address: dummy_addr(),
                chunks: vec![RouteChunk {
                    protocol: protocol.into(),
                    bid_amount: "100".into(),
                    ask_amount: ask.into(),
                    extra_version: 1,
                    extra: vec![],
                }],
            }],
        };

        Quote {
            quote_id: "q".into(),
            resolver_id: "r".into(),
            resolver_name: "Omniston".into(),
            bid_asset_address: dummy_addr(),
            ask_asset_address: dummy_addr(),
            bid_units: "100".into(),
            ask_units: ask.into(),
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
                    min_ask_amount: min_ask.into(),
                    recommended_min_ask_amount: min_ask.into(),
                    recommended_slippage_bps: rec_slip,
                }),
            },
        }
    }

    #[test]
    fn test_market_wide_slippage() {
        let q = mk_quote("1000", "950", 100, "StonFi");
        let r = compute_slippage(&q, &ExecutionScope::MarketWide);

        // (1000 - 950) / 1000 * 10,000 = 500 bps
        assert_eq!(r.validity, PulseValidity::Valid);
        assert!((r.slippage_bps - 500.0).abs() < 1e-9);
    }

    #[test]
    fn test_protocol_only_synthetic_slippage() {
        let scope = ExecutionScope::ProtocolOnly {
            protocol: "StonFi".into(),
        };
        // Quote says 1000 output, recommended slippage is 20 bps
        let q = mk_quote("1000", "950", 20, "StonFi");
        let r = compute_slippage(&q, &scope);

        // Should return the synthetic/recommended bps (20) rather than global (500)
        assert_eq!(r.validity, PulseValidity::Valid);
        assert_eq!(r.slippage_bps, 20.0);
    }

    #[test]
    fn test_invalid_on_missing_protocol() {
        let scope = ExecutionScope::ProtocolOnly {
            protocol: "StonFi".into(),
        };
        let q = mk_quote("1000", "950", 20, "DeDust"); // No StonFi in quote
        let r = compute_slippage(&q, &scope);

        assert_eq!(r.validity, PulseValidity::Invalid);
        assert_eq!(r.slippage_bps, f64::MAX);
    }
}

use super::input::QuoteInput;
use super::{Pulse, PulseResult, PulseValidity};
use crate::market::types::Quote;

/// Slippage Pulse
///
/// The Slippage Pulse measures **execution quality degradation** for a *single RFQ*.
///
/// Unlike spread or trend, this pulse is **not historical** and **not predictive**.
/// It answers only one question:
///
/// > “How much worse can this trade execute than the quoted expectation?”
///
/// ## Data source
/// From an Omniston RFQ quote:
/// - `ask_units`        → expected output amount
/// - `min_ask_amount`  → guaranteed minimum output
///
/// These values are *resolver-enforced*, not estimates.
///
/// ## Definition
///
/// ```text
/// slippage_bps = (ask_units - min_ask_amount) / ask_units * 10_000
/// ```
///
/// ## Properties
/// - **Instantaneous**: computed per-quote, no rolling window
/// - **Deterministic**: same input → same output
/// - **Fail-safe**: malformed or unsafe quotes are always `Invalid`
///
/// ## Interpretation
/// - `0 bps`        → perfect execution (no downside)
/// - `> 0 bps`      → increasing execution risk
/// - `Invalid`      → execution must be blocked
///
/// ## Design rule
/// This pulse is a **hard safety gate**.
/// If slippage is invalid or exceeds user tolerance, *no execution is allowed*.
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
///
/// This pulse owns **no internal state** and may be reused freely.
/// It is safe to share one instance across all pairs.
pub struct SlippagePulse;

impl Pulse for SlippagePulse {
    type Input = QuoteInput;
    type Output = SlippagePulseResult;

    fn evaluate(&mut self, input: Self::Input) -> Self::Output {
        compute_slippage(&input.quote)
    }
}

/// Core slippage computation.
///
/// ### Safety invariants
/// This function **must never allow execution on bad data**.
///
/// Invalid if:
/// - swap params are missing
/// - numeric parsing fails
/// - `ask_units <= 0`
/// - `min_ask_amount > ask_units` (malformed quote)
///
/// All invalid paths return:
/// - `slippage_bps = f64::MAX`
/// - `validity = Invalid`
///
/// This guarantees downstream threshold checks always fail safely.
fn compute_slippage(quote: &Quote) -> SlippagePulseResult {
    let swap = match &quote.params.swap {
        Some(s) => s,
        None => return SlippagePulseResult::default(),
    };

    let ask_units: f64 = match quote.ask_units.parse() {
        Ok(v) if v > 0.0 => v,
        _ => return SlippagePulseResult::default(),
    };

    let min_ask_amount: f64 = match swap.min_ask_amount.parse() {
        Ok(v) if v >= 0.0 => v,
        _ => return SlippagePulseResult::default(),
    };

    // Malformed or inconsistent quote → block execution
    if min_ask_amount > ask_units {
        return SlippagePulseResult::default();
    }

    let slippage_bps = ((ask_units - min_ask_amount) / ask_units) * 10_000.0;

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

    fn mk_quote(ask: &str, min_ask: &str) -> Quote {
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
                    routes: vec![],
                    min_ask_amount: min_ask.into(),
                    recommended_min_ask_amount: min_ask.into(),
                    recommended_slippage_bps: 0,
                }),
            },
        }
    }

    #[test]
    fn zero_slippage_is_valid_and_exact() {
        let q = mk_quote("1000", "1000");
        let r = compute_slippage(&q);

        assert_eq!(r.validity, PulseValidity::Valid);
        assert_eq!(r.slippage_bps, 0.0);
    }

    #[test]
    fn slippage_matches_expected_formula() {
        let q = mk_quote("1000", "950");
        let r = compute_slippage(&q);

        // (1000 - 950) / 1000 * 10_000 = 500 bps
        assert_eq!(r.validity, PulseValidity::Valid);
        assert!((r.slippage_bps - 500.0).abs() < 1e-9);
    }

    #[test]
    fn missing_swap_params_blocks_execution() {
        let mut q = mk_quote("1000", "950");
        q.params.swap = None;

        let r = compute_slippage(&q);

        assert_eq!(r.validity, PulseValidity::Invalid);
        assert_eq!(r.slippage_bps, f64::MAX);
    }

    #[test]
    fn malformed_quote_min_greater_than_ask_is_invalid() {
        let q = mk_quote("1000", "1100");
        let r = compute_slippage(&q);

        assert_eq!(r.validity, PulseValidity::Invalid);
    }

    #[test]
    fn zero_or_negative_ask_blocks_execution() {
        let q1 = mk_quote("0", "0");
        let q2 = mk_quote("-10", "0");

        assert!(matches!(
            compute_slippage(&q1).validity,
            PulseValidity::Invalid
        ));
        assert!(matches!(
            compute_slippage(&q2).validity,
            PulseValidity::Invalid
        ));
    }

    #[test]
    fn large_numbers_do_not_overflow_or_panic() {
        let q = mk_quote("1000000000000000000", "999999999999999999");
        let r = compute_slippage(&q);

        assert_eq!(r.validity, PulseValidity::Valid);
        assert!(r.slippage_bps >= 0.0);
    }
}

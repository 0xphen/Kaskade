//! Slippage Pulse
//!
//! This module computes the **Slippage Pulse**, which measures execution
//! quality degradation between the *expected* output of a swap and the
//! *guaranteed minimum* output provided by Omniston.
//!
//! In RFQ-based execution (Omniston / STON.fi):
//!
//! - `ask_units`      = expected output amount (what the quote promises)
//! - `min_ask_amount` = guaranteed minimum output (what execution guarantees)
//!
//! Slippage is defined as:
//!
//! ```text
//! slippage_bps = (ask_units - min_ask_amount) / ask_units * 10_000
//! ```
//!
//! Properties:
//! - Instantaneous (no rolling window)
//! - Deterministic
//! - Fail-safe (invalid quotes never allow execution)

use crate::pulse::PulseValidity;
use crate::types::Quote;

/// Result of Slippage Pulse computation.
#[derive(Clone, Debug)]
pub struct SlippagePulseResult {
    /// Slippage in basis points.
    /// 0 bps = no slippage
    /// higher = worse execution quality
    pub slippage_bps: f64,

    /// Whether the result is safe to use.
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

/// Compute Slippage Pulse from an Omniston quote.
///
/// # Safety rules
/// - If swap params are missing → Invalid
/// - If parsing fails → Invalid
/// - If ask_units <= 0 → Invalid
///
/// Invalid pulses always return `slippage_bps = f64::MAX`
/// so downstream threshold checks fail safely.
pub fn compute_slippage_pulse(quote: &Quote) -> SlippagePulseResult {
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

    // Guard against pathological cases
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
    use crate::types::*;

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
    fn zero_slippage_when_min_equals_expected() {
        let q = mk_quote("1000", "1000");
        let r = compute_slippage_pulse(&q);

        assert_eq!(r.validity, PulseValidity::Valid);
        assert!(r.slippage_bps.abs() < 1e-9);
    }

    #[test]
    fn positive_slippage_when_min_is_lower() {
        let q = mk_quote("1000", "950");
        let r = compute_slippage_pulse(&q);

        // (1000 - 950) / 1000 * 10000 = 500 bps
        assert_eq!(r.validity, PulseValidity::Valid);
        assert!((r.slippage_bps - 500.0).abs() < 1e-9);
    }

    #[test]
    fn invalid_when_swap_missing() {
        let mut q = mk_quote("1000", "950");
        q.params.swap = None;

        let r = compute_slippage_pulse(&q);
        assert_eq!(r.validity, PulseValidity::Invalid);
    }
}

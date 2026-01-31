//! Slippage Pulse
//!
//! This module computes the **Slippage Pulse**, which measures execution
//! quality degradation between the *expected* output of a swap and the
//! *guaranteed minimum* output provided by Omniston.
//!
//! In RFQ-based execution (Omniston / STON.fi):
//!
//! - `ask_units`            = expected output amount
//! - `min_ask_amount`       = guaranteed minimum output
//!
//! Slippage is defined as:
//!
//!     slippage_bps = (ask_units - min_ask_amount) / ask_units * 10_000
//!
//! This pulse is **instantaneous**:
//! - No rolling window
//! - No prediction
//! - No smoothing
//!
//! It reflects *pure execution quality* at the moment of execution.

use super::PulseValidity;

/// Result of slippage pulse computation.
#[derive(Clone, Debug)]
pub struct SlippagePulseResult {
    /// Slippage expressed in basis points (bps).
    pub slippage_bps: f64,

    /// Whether the pulse result is valid and usable for execution decisions.
    pub validity: PulseValidity,
}

impl Default for SlippagePulseResult {
    fn default() -> Self {
        Self {
            slippage_bps: 0.0,
            validity: PulseValidity::Invalid,
        }
    }
}

/// Compute the Slippage Pulse.
///
/// # Inputs
/// - `ask_units`: expected output amount from the RFQ quote
/// - `min_ask_amount`: guaranteed minimum output amount
///
/// # Behavior
/// - If inputs are invalid (≤ 0), pulse is `Invalid`
/// - Otherwise, compute slippage in basis points
///
/// # Notes
/// - A slippage of `0.0` means perfect execution quality
/// - Higher slippage means worse execution
/// - Scheduler enforces thresholds; this function only measures
pub fn compute_slippage_pulse(ask_units: f64, min_ask_amount: f64) -> SlippagePulseResult {
    if ask_units <= 0.0 || min_ask_amount <= 0.0 {
        return SlippagePulseResult {
            slippage_bps: f64::MAX,
            validity: PulseValidity::Invalid,
        };
    }

    // Guaranteed amount should never exceed expected amount,
    // but guard against malformed data.
    if min_ask_amount > ask_units {
        return SlippagePulseResult {
            slippage_bps: 0.0,
            validity: PulseValidity::Invalid,
        };
    }

    let slippage_bps = ((ask_units - min_ask_amount) / ask_units) * 10_000.0;

    SlippagePulseResult {
        slippage_bps,
        validity: PulseValidity::Valid,
    }
}

#[cfg(test)]
mod tests {
    use super::super::PulseValidity;
    use super::*;

    #[test]
    fn zero_slippage_is_valid() {
        let res = compute_slippage_pulse(1_000_000.0, 1_000_000.0);

        assert_eq!(res.slippage_bps, 0.0);
        assert!(matches!(res.validity, PulseValidity::Valid));
    }

    #[test]
    fn non_zero_slippage_computes_correctly() {
        let res = compute_slippage_pulse(1_000_000.0, 990_000.0);

        // (1_000_000 - 990_000) / 1_000_000 * 10_000 = 100 bps
        assert!((res.slippage_bps - 100.0).abs() < 1e-9);
        assert!(matches!(res.validity, PulseValidity::Valid));
    }

    #[test]
    fn high_slippage_is_reported_but_valid() {
        let res = compute_slippage_pulse(1_000_000.0, 900_000.0);

        assert!(res.slippage_bps > 500.0);
        assert!(matches!(res.validity, PulseValidity::Valid));
    }

    #[test]
    fn zero_ask_units_is_invalid() {
        let res = compute_slippage_pulse(0.0, 100.0);

        assert_eq!(res.slippage_bps, f64::MAX);
        assert!(matches!(res.validity, PulseValidity::Invalid));
    }

    #[test]
    fn zero_min_amount_is_invalid() {
        let res = compute_slippage_pulse(100.0, 0.0);

        assert_eq!(res.slippage_bps, f64::MAX);
        assert!(matches!(res.validity, PulseValidity::Invalid));
    }

    #[test]
    fn min_amount_greater_than_expected_is_invalid() {
        let res = compute_slippage_pulse(100.0, 110.0);

        assert_eq!(res.slippage_bps, 0.0);
        assert!(matches!(res.validity, PulseValidity::Invalid));
    }
}

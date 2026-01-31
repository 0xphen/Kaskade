//! Trend Pulse
//!
//! The Trend Pulse is a **directional execution safety signal**.
//!
//! Goal:
//! - Prevent execution while price is **moving against the user**
//! - Avoid “falling knife” executions even when spread looks acceptable
//!
//! In RFQ context, trend is measured over **executable prices**,
//! not mid prices or order-book signals.
//!
//! Trend definition (v0.1):
//! - Maintain a rolling window of recent prices (p = ask_units / bid_units)
//! - Compare the current price to the **oldest price** in the window
//! - Measure directional drop in basis points
//!
//!     trend_drop_bps = (p_oldest - p_now) / p_oldest * 10_000
//!
//! Interpretation:
//! - trend_drop_bps <= 0   → flat or rising → OK
//! - trend_drop_bps small  → mild drift → OK
//! - trend_drop_bps large  → falling market → block execution
//!
//! This pulse is **not predictive**.
//! It is a **protective guardrail**.

use crate::pulse::PulseValidity;
use crate::rolling_window::RollingWindow;

/// Output of the Trend Pulse.
#[derive(Clone, Debug)]
pub struct TrendPulseResult {
    /// Current price (ask_units / bid_units)
    pub p_now: f64,

    /// Oldest price in the rolling window
    pub p_oldest: f64,

    /// Downward price movement in basis points
    pub trend_drop_bps: f64,

    /// Whether the pulse is valid (warm-up complete)
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

/// Warm-up configuration for Trend Pulse.
#[derive(Clone, Copy, Debug)]
pub struct TrendWarmup {
    pub min_samples: usize,
    pub min_age_ms: u64,
}

impl Default for TrendWarmup {
    fn default() -> Self {
        Self {
            min_samples: 10,
            min_age_ms: 5_000,
        }
    }
}

/// Compute the Trend Pulse.
///
/// # Inputs
/// - `ts_ms`: timestamp in milliseconds
/// - `bid_units`: input amount
/// - `ask_units`: output amount
/// - `window`: rolling window storing recent prices
/// - `warmup`: warm-up thresholds
///
/// # Output
/// Returns `TrendPulseResult`.
///
/// # Safety
/// - Invalid if bid_units <= 0
/// - Invalid during warm-up
pub fn compute_trend_pulse(
    ts_ms: u64,
    bid_units: f64,
    ask_units: f64,
    window: &mut RollingWindow,
    warmup: TrendWarmup,
) -> TrendPulseResult {
    if bid_units <= 0.0 {
        return TrendPulseResult::default();
    }

    let p_now = ask_units / bid_units;

    // Push latest price into the rolling window
    window.push(ts_ms, p_now);

    // Warm-up safeguard
    if !window.is_warm(warmup.min_samples, warmup.min_age_ms) {
        return TrendPulseResult {
            p_now,
            p_oldest: p_now,
            trend_drop_bps: f64::MAX,
            validity: PulseValidity::Invalid,
        };
    }

    // Oldest price = first element in the window
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
    use crate::rolling_window::RollingWindow;

    fn ms(n: u64) -> u64 {
        n * 1000
    }

    fn warmup() -> TrendWarmup {
        TrendWarmup {
            min_samples: 3,
            min_age_ms: 2_000,
        }
    }

    #[test]
    fn invalid_when_bid_units_zero() {
        let mut w = RollingWindow::new();

        let res = compute_trend_pulse(ms(0), 0.0, 100.0, &mut w, warmup());
        assert!(matches!(res.validity, PulseValidity::Invalid));
    }

    #[test]
    fn warmup_blocks_first_ticks() {
        let mut w = RollingWindow::new();

        let r1 = compute_trend_pulse(ms(0), 100.0, 50.0, &mut w, warmup());
        let r2 = compute_trend_pulse(ms(1), 100.0, 49.5, &mut w, warmup());

        assert!(matches!(r1.validity, PulseValidity::Invalid));
        assert!(matches!(r2.validity, PulseValidity::Invalid));
    }

    #[test]
    fn trend_detects_downward_movement() {
        let mut w = RollingWindow::new();
        let wup = warmup();

        compute_trend_pulse(ms(0), 100.0, 50.0, &mut w, wup); // 0.50
        compute_trend_pulse(ms(1), 100.0, 49.0, &mut w, wup); // 0.49
        let r3 = compute_trend_pulse(ms(2), 100.0, 48.0, &mut w, wup); // 0.48

        assert!(matches!(r3.validity, PulseValidity::Valid));
        assert!(r3.trend_drop_bps > 0.0);
    }

    #[test]
    fn trend_zero_when_flat_or_rising() {
        let mut w = RollingWindow::new();
        let wup = warmup();

        compute_trend_pulse(ms(0), 100.0, 50.0, &mut w, wup);
        compute_trend_pulse(ms(1), 100.0, 51.0, &mut w, wup);
        let r3 = compute_trend_pulse(ms(2), 100.0, 52.0, &mut w, wup);

        assert!(matches!(r3.validity, PulseValidity::Valid));
        assert!(r3.trend_drop_bps <= 0.0);
    }
}

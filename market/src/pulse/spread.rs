//! Spread Pulse (Execution-Quality Signal)
//!
//! This module implements **Spread Pulse**, a lightweight execution-quality signal for
//! RFQ-style pricing (like Omniston).
//!
//! ## What “spread” means in RFQ context
//! In an RFQ stream there is no public order book with best bid/ask.
//! You receive *executable quotes* for a specific input amount.
//!
//! So “spread” here is defined as:
//! > How far is the current executable price from the best executable price
//! > observed recently (within a rolling window)?
//!
//! This gives you a **market-quality / execution-quality** indicator:
//! - If the quote is near the best recent quote → “tight spread” → good time to execute
//! - If the quote is worse than recent best → “wide spread” → avoid execution
//!
//! ## Warm-up guard
//! At startup, the window is empty. The first quote would produce:
//! - p_best == p_now
//! - spread == 0
//! which would incorrectly mark conditions as “perfect” and may cause immediate execution.
//!
//! To avoid this, the pulse remains **Invalid** until the rolling window is “warm”
//! (has enough samples and enough time coverage).
//!
//! ## Determinism
//! This computation is pure and deterministic given:
//! - (ts_ms, bid_units, ask_units)
//! - rolling window state
//!
//! Any I/O (websocket, storage, logging) must live outside this module.

use super::PulseValidity;
use crate::rolling_window::RollingWindow;

/// Spread pulse output used by the scheduler and for observability.
///
/// `spread_bps` is computed as:
/// ```text
/// spread_bps = ((p_best - p_now) / p_best) * 10_000
/// ```
/// where:
/// - p_now  = ask_units / bid_units
/// - p_best = max(price) over the rolling window
///
/// Interpretation:
/// - `spread_bps` ≈ 0  → current quote is as good as the best recent quote
/// - larger `spread_bps` → current quote is worse than recent best (avoid)
///
/// Important:
/// - This is an RFQ execution-quality spread (not LOB bid/ask spread).
#[derive(Clone, Debug)]
pub struct SpreadPulseResult {
    /// Current quote price (amount_out / amount_in).
    pub p_now: f64,
    /// Best observed price within the rolling window.
    pub p_best: f64,
    /// Degradation from best recent price, in basis points.
    pub spread_bps: f64,
    /// Whether the result is safe to use.
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

// TODO: should be configurable
/// Warm-up configuration for Spread Pulse.
#[derive(Clone, Copy, Debug)]
pub struct SpreadWarmup {
    /// Minimum number of samples required before the pulse becomes valid.
    pub min_samples: usize,
    /// Minimum timespan covered by the window required before valid.
    pub min_age_ms: u64,
}

impl Default for SpreadWarmup {
    fn default() -> Self {
        Self {
            min_samples: 10,
            min_age_ms: 5_000,
        }
    }
}

/// Compute Spread Pulse given RFQ quote data and a rolling window.
///
/// # Inputs
/// - `ts_ms`: timestamp in milliseconds (monotonic or wall-clock is OK as long as it
///           increases for a given stream)
/// - `bid_units`: amount_in (what you pay)
/// - `ask_units`: amount_out (what you receive)
/// - `window`: rolling window tracking recent prices
/// - `warmup`: warm-up thresholds (samples + age)
///
/// # Output
/// Returns a `SpreadPulseResult`.
///
/// # Safety & Edge Cases
/// - If `bid_units <= 0`, result is Invalid and spread is set to `f64::MAX`.
/// - During warm-up, result is Invalid and spread is set to `f64::MAX`.
///
/// # Why `f64::MAX` for invalid spread?
/// Because it guarantees downstream threshold checks fail safely:
/// `spread_bps > max_spread_bps` → ineligible.
pub fn compute_spread_pulse(
    ts_ms: u64,
    bid_units: f64,
    ask_units: f64,
    window: &mut RollingWindow,
    warmup: SpreadWarmup,
) -> SpreadPulseResult {
    if bid_units <= 0.0 {
        return SpreadPulseResult::default();
    }

    let p_now = ask_units / bid_units;

    // Update rolling window with the newest price
    window.push(ts_ms, p_now);

    // Warm-up guard: do not allow “first tick == perfect conditions”
    if !window.is_warm(warmup.min_samples, warmup.min_age_ms) {
        return SpreadPulseResult {
            p_now,
            p_best: p_now,
            spread_bps: f64::MAX,
            validity: super::PulseValidity::Invalid,
        };
    }

    let p_best = window.max().unwrap_or(p_now);

    // Compute degradation from best
    let spread_bps = if p_best > 0.0 {
        ((p_best - p_now) / p_best) * 10_000.0
    } else {
        f64::MAX
    };

    SpreadPulseResult {
        p_now,
        p_best,
        spread_bps,
        validity: super::PulseValidity::Valid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pulse::PulseValidity;
    use crate::rolling_window::RollingWindow;

    fn ms(n: u64) -> u64 {
        n
    }

    fn test_warmup() -> SpreadWarmup {
        SpreadWarmup {
            min_samples: 3,
            min_age_ms: 2_000,
        }
    }

    #[test]
    fn invalid_when_bid_units_is_zero() {
        let mut window = RollingWindow::new();

        let res = compute_spread_pulse(ms(0), 0.0, 100.0, &mut window, test_warmup());

        assert_eq!(res.validity, PulseValidity::Invalid);
        assert_eq!(res.spread_bps, f64::MAX);
        assert_eq!(res.p_now, 0.0);
        assert_eq!(res.p_best, 0.0);
    }

    #[test]
    fn warmup_blocks_first_ticks_even_if_spread_would_be_zero() {
        let mut window = RollingWindow::new();

        let r1 = compute_spread_pulse(ms(0), 100.0, 50.0, &mut window, test_warmup());
        assert_eq!(r1.validity, PulseValidity::Invalid);
        assert_eq!(r1.spread_bps, f64::MAX);

        let r2 = compute_spread_pulse(ms(1_000), 100.0, 51.0, &mut window, test_warmup());
        assert_eq!(r2.validity, PulseValidity::Invalid);
        assert_eq!(r2.spread_bps, f64::MAX);
    }

    #[test]
    fn becomes_valid_after_min_samples_and_min_age() {
        let mut window = RollingWindow::new();

        compute_spread_pulse(ms(0), 100.0, 50.0, &mut window, test_warmup());
        compute_spread_pulse(ms(1_000), 100.0, 51.0, &mut window, test_warmup());
        let r3 = compute_spread_pulse(ms(2_000), 100.0, 52.0, &mut window, test_warmup());

        assert_eq!(r3.validity, PulseValidity::Valid);
        // p_best should be max of {0.5, 0.51, 0.52} = 0.52
        assert!((r3.p_best - 0.52).abs() < 1e-12);
        // p_now is 0.52 => spread ~ 0
        assert!(r3.spread_bps.abs() < 1e-9);
    }

    #[test]
    fn spread_increases_when_current_quote_is_worse_than_recent_best() {
        let mut window = RollingWindow::new();
        let w = test_warmup();

        // Warm up with good prices
        compute_spread_pulse(ms(0), 100.0, 50.0, &mut window, w); // 0.50
        compute_spread_pulse(ms(1_000), 100.0, 52.0, &mut window, w); // 0.52 (best)
        let r3 = compute_spread_pulse(ms(2_000), 100.0, 51.0, &mut window, w); // 0.51

        assert_eq!(r3.validity, PulseValidity::Valid);
        assert!((r3.p_best - 0.52).abs() < 1e-12);
        assert!((r3.p_now - 0.51).abs() < 1e-12);

        // Expected: ((0.52 - 0.51) / 0.52) * 10000 = ~192.3077 bps
        assert!(r3.spread_bps > 150.0);
        assert!(r3.spread_bps < 250.0);
    }

    #[test]
    fn spread_is_zero_when_current_equals_best() {
        let mut window = RollingWindow::new();
        let w = test_warmup();

        compute_spread_pulse(ms(0), 100.0, 50.0, &mut window, w); // 0.50
        compute_spread_pulse(ms(1_000), 100.0, 52.0, &mut window, w); // 0.52
        let r3 = compute_spread_pulse(ms(2_000), 100.0, 52.0, &mut window, w); // 0.52

        assert_eq!(r3.validity, PulseValidity::Valid);
        assert!(r3.spread_bps.abs() < 1e-9);
    }

    #[test]
    fn window_max_is_respected_after_many_updates() {
        let mut window = RollingWindow::new();
        let w = SpreadWarmup {
            min_samples: 5,
            min_age_ms: 4_000,
        };

        // Build up: max should become 3.0
        compute_spread_pulse(ms(0), 1.0, 1.0, &mut window, w); // 1.0
        compute_spread_pulse(ms(1_000), 1.0, 3.0, &mut window, w); // 3.0 (max)
        compute_spread_pulse(ms(2_000), 1.0, 2.0, &mut window, w); // 2.0
        compute_spread_pulse(ms(3_000), 1.0, 2.5, &mut window, w); // 2.5
        let r5 = compute_spread_pulse(ms(4_000), 1.0, 2.2, &mut window, w); // valid now

        assert_eq!(r5.validity, PulseValidity::Valid);
        assert!((r5.p_best - 3.0).abs() < 1e-12);
    }
}

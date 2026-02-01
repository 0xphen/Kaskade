use crate::pulse::input::PriceInput;
use crate::pulse::{Pulse, PulseResult, PulseValidity};
use crate::rolling_window::RollingWindow;

/// Spread Pulse
///
/// The Spread Pulse measures **price degradation relative to the best recent
/// executable price** for a trading pair.
///
/// ## What it answers
/// > “How much worse is the current quote compared to the best price we’ve seen recently?”
///
/// This is **not a bid/ask order-book spread**.
/// It is a **temporal execution spread** derived from RFQ prices.
///
/// ## Price definition
///
/// ```text
/// p = ask_units / bid_units
/// ```
///
/// Higher `p` means a better execution rate for the buyer.
///
/// ## Spread definition
///
/// ```text
/// spread_bps = (p_best - p_now) / p_best * 10_000
/// ```
///
/// where:
/// - `p_now`  = current price
/// - `p_best` = maximum price observed in the rolling window
///
/// ## Warm-up guard
/// The pulse is marked `Invalid` until:
/// - a minimum number of samples is collected
/// - the window spans a minimum time duration
///
/// This prevents **false zero-spread signals** at startup.
///
/// ## Interpretation
/// - `0 bps`        → best recent price
/// - small spread   → acceptable degradation
/// - large spread   → poor execution quality
///
/// ## Design properties
/// - Rolling-window based
/// - Deterministic
/// - Bounded memory
/// - Fail-safe (invalid data never allows execution)
#[derive(Clone, Debug)]
pub struct SpreadPulseResult {
    /// Current price (ask_units / bid_units).
    pub p_now: f64,

    /// Best observed price in the rolling window.
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
///
/// Encodes how much history is required before the pulse
/// is allowed to produce a valid signal.
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
///
/// Holds a rolling window of recent prices.
pub struct SpreadPulse {
    window: RollingWindow,
    warmup: SpreadWarmup,
}

impl SpreadPulse {
    pub fn new(warmup: SpreadWarmup) -> Self {
        Self {
            window: RollingWindow::new(),
            warmup,
        }
    }
}

impl Pulse for SpreadPulse {
    type Input = PriceInput;
    type Output = SpreadPulseResult;

    fn evaluate(&mut self, input: Self::Input) -> Self::Output {
        compute_spread(
            input.ts_ms,
            input.bid_units,
            input.ask_units,
            &mut self.window,
            self.warmup,
        )
    }
}

/// Core spread computation.
///
/// Safety rules:
/// - `bid_units <= 0` → Invalid
/// - Warm-up not satisfied → Invalid
///
/// Invalid cases always return `spread_bps = f64::MAX`
/// to guarantee downstream threshold checks fail.
fn compute_spread(
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
    window.push(ts_ms, p_now);

    if !window.is_warm(warmup.min_samples, warmup.min_age_ms) {
        return SpreadPulseResult {
            p_now,
            p_best: p_now,
            spread_bps: f64::MAX,
            validity: PulseValidity::Invalid,
        };
    }

    let p_best = window.max().unwrap_or(p_now);

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
    use crate::pulse::PulseValidity;

    fn ms(n: u64) -> u64 {
        n
    }

    fn warmup() -> SpreadWarmup {
        SpreadWarmup {
            min_samples: 3,
            min_age_ms: 2_000,
        }
    }

    #[test]
    fn invalid_when_bid_units_is_zero() {
        let mut window = RollingWindow::new();

        let res = compute_spread(ms(0), 0.0, 100.0, &mut window, warmup());

        assert_eq!(res.validity, PulseValidity::Invalid);
        assert_eq!(res.spread_bps, f64::MAX);
    }

    #[test]
    fn warmup_blocks_early_zero_spread() {
        let mut window = RollingWindow::new();

        let r1 = compute_spread(ms(0), 100.0, 50.0, &mut window, warmup());
        let r2 = compute_spread(ms(1_000), 100.0, 50.0, &mut window, warmup());

        assert_eq!(r1.validity, PulseValidity::Invalid);
        assert_eq!(r2.validity, PulseValidity::Invalid);
    }

    #[test]
    fn becomes_valid_only_after_min_samples_and_min_age() {
        let mut window = RollingWindow::new();

        compute_spread(ms(0), 100.0, 50.0, &mut window, warmup());
        compute_spread(ms(1_000), 100.0, 51.0, &mut window, warmup());
        let r3 = compute_spread(ms(2_000), 100.0, 52.0, &mut window, warmup());

        assert_eq!(r3.validity, PulseValidity::Valid);
        assert!((r3.p_best - 0.52).abs() < 1e-12);
        assert!(r3.spread_bps.abs() < 1e-9);
    }

    #[test]
    fn spread_reflects_degradation_from_recent_best() {
        let mut window = RollingWindow::new();
        let w = warmup();

        compute_spread(ms(0), 100.0, 50.0, &mut window, w); // 0.50
        compute_spread(ms(1_000), 100.0, 52.0, &mut window, w); // 0.52
        let r = compute_spread(ms(2_000), 100.0, 51.0, &mut window, w); // 0.51

        assert_eq!(r.validity, PulseValidity::Valid);
        assert!(r.spread_bps > 0.0);
    }

    #[test]
    fn spread_is_zero_when_current_equals_best() {
        let mut window = RollingWindow::new();
        let w = warmup();

        compute_spread(ms(0), 100.0, 50.0, &mut window, w);
        compute_spread(ms(1_000), 100.0, 52.0, &mut window, w);
        let r = compute_spread(ms(2_000), 100.0, 52.0, &mut window, w);

        assert_eq!(r.validity, PulseValidity::Valid);
        assert!(r.spread_bps.abs() < 1e-9);
    }

    #[test]
    fn window_tracks_true_max_over_time() {
        let mut window = RollingWindow::new();
        let w = SpreadWarmup {
            min_samples: 5,
            min_age_ms: 4_000,
        };

        compute_spread(ms(0), 1.0, 1.0, &mut window, w);
        compute_spread(ms(1_000), 1.0, 3.0, &mut window, w); // best
        compute_spread(ms(2_000), 1.0, 2.0, &mut window, w);
        compute_spread(ms(3_000), 1.0, 2.5, &mut window, w);
        let r = compute_spread(ms(4_000), 1.0, 2.2, &mut window, w);

        assert_eq!(r.validity, PulseValidity::Valid);
        assert!((r.p_best - 3.0).abs() < 1e-12);
    }
}

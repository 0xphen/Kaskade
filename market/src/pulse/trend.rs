use crate::pulse::input::PriceInput;
use crate::pulse::{Pulse, PulseResult, PulseValidity};
use crate::rolling_window::RollingWindow;

/// Trend Pulse
///
/// The Trend Pulse measures **price directionality over time**.
///
/// ## What it answers
/// > “Is the execution price getting worse compared to where it started recently?”
///
/// This pulse detects **downward price trends** that may indicate:
/// - deteriorating execution conditions
/// - adverse momentum
/// - rising execution risk
///
/// ## Price definition
///
/// ```text
/// p = ask_units / bid_units
/// ```
///
/// Higher `p` means a better execution rate for the buyer.
///
/// ## Trend definition
///
/// The pulse compares the **oldest price in the rolling window**
/// with the **current price**:
///
/// ```text
/// trend_drop_bps = (p_oldest - p_now) / p_oldest * 10_000
/// ```
///
/// ## Interpretation
/// - `trend_drop_bps > 0` → price is deteriorating (downward trend)
/// - `trend_drop_bps = 0` → flat
/// - `trend_drop_bps < 0` → improving execution
///
/// ## Warm-up guard
/// The pulse is marked `Invalid` until:
/// - a minimum number of samples is collected
/// - the window spans a minimum time duration
///
/// ## Design properties
/// - Rolling-window based
/// - Directional (not magnitude-focused)
/// - Deterministic
/// - Bounded memory
/// - Fail-safe (invalid data never allows execution)
#[derive(Clone, Debug)]
pub struct TrendPulseResult {
    /// Current price.
    pub p_now: f64,

    /// Oldest price in the rolling window.
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
///
/// Maintains a rolling window of recent prices.
pub struct TrendPulse {
    window: RollingWindow,
    warmup: TrendWarmup,
}

impl TrendPulse {
    pub fn new(warmup: TrendWarmup) -> Self {
        Self {
            window: RollingWindow::new(),
            warmup,
        }
    }
}

impl Pulse for TrendPulse {
    type Input = PriceInput;
    type Output = TrendPulseResult;

    fn evaluate(&mut self, input: Self::Input) -> Self::Output {
        compute_trend(
            input.ts_ms,
            input.bid_units,
            input.ask_units,
            &mut self.window,
            self.warmup,
        )
    }
}

/// Core trend computation.
///
/// Safety rules:
/// - `bid_units <= 0` → Invalid
/// - Warm-up not satisfied → Invalid
///
/// Invalid cases always return `trend_drop_bps = f64::MAX`
/// to guarantee downstream threshold checks fail.
fn compute_trend(
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
    window.push(ts_ms, p_now);

    if !window.is_warm(warmup.min_samples, warmup.min_age_ms) {
        return TrendPulseResult {
            p_now,
            p_oldest: p_now,
            trend_drop_bps: f64::MAX,
            validity: PulseValidity::Invalid,
        };
    }

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
    use crate::pulse::PulseValidity;

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

        let r = compute_trend(ms(0), 0.0, 100.0, &mut w, warmup());
        assert_eq!(r.validity, PulseValidity::Invalid);
        assert_eq!(r.trend_drop_bps, f64::MAX);
    }

    #[test]
    fn warmup_blocks_early_trend_signals() {
        let mut w = RollingWindow::new();

        let r1 = compute_trend(ms(0), 100.0, 50.0, &mut w, warmup());
        let r2 = compute_trend(ms(1), 100.0, 49.0, &mut w, warmup());

        assert_eq!(r1.validity, PulseValidity::Invalid);
        assert_eq!(r2.validity, PulseValidity::Invalid);
    }

    #[test]
    fn detects_downward_trend() {
        let mut w = RollingWindow::new();
        let wup = warmup();

        compute_trend(ms(0), 100.0, 50.0, &mut w, wup); // 0.50
        compute_trend(ms(1), 100.0, 49.0, &mut w, wup); // 0.49
        let r = compute_trend(ms(2), 100.0, 48.0, &mut w, wup); // 0.48

        assert_eq!(r.validity, PulseValidity::Valid);
        assert!(r.trend_drop_bps > 0.0);
    }

    #[test]
    fn flat_or_rising_price_produces_zero_or_negative_trend() {
        let mut w = RollingWindow::new();
        let wup = warmup();

        compute_trend(ms(0), 100.0, 50.0, &mut w, wup);
        compute_trend(ms(1), 100.0, 51.0, &mut w, wup);
        let r = compute_trend(ms(2), 100.0, 52.0, &mut w, wup);

        assert_eq!(r.validity, PulseValidity::Valid);
        assert!(r.trend_drop_bps <= 0.0);
    }

    #[test]
    fn uses_oldest_price_not_best_or_average() {
        let mut w = RollingWindow::new();
        let wup = warmup();

        compute_trend(ms(0), 100.0, 50.0, &mut w, wup); // oldest = 0.50
        compute_trend(ms(1), 100.0, 60.0, &mut w, wup); // spike
        let r = compute_trend(ms(2), 100.0, 55.0, &mut w, wup); // still above oldest

        assert_eq!(r.validity, PulseValidity::Valid);
        assert!(r.trend_drop_bps < 0.0);
    }
}

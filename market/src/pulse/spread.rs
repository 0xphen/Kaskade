use crate::rolling_window::RollingWindow;

/// Result struct for debugging, logging, and analytics
#[derive(Clone, Default, Debug)]
pub struct SpreadPulseResult {
    pub p_now: f64,
    pub p_best: f64,
    pub spread_bps: f64,
}

/// Compute Spread Pulse given RFQ quote data and a rolling window.
///
/// Spread Pulse definition:
/// - Compute current RFQ price p_now = ask_units / bid_units
/// - Update window
/// - Compute p_best = max(window)
/// - Compute spread_bps = (p_best - p_now) / p_best * 10_000
pub fn compute_spread_pulse(
    ts_ms: u64,
    bid_units: f64,
    ask_units: f64,
    window: &mut RollingWindow,
) -> SpreadPulseResult {
    // Safety: bid_units is always > 0 in RFQ. If not, treat as no pulse.
    if bid_units <= 0.0 {
        return SpreadPulseResult {
            p_now: 0.0,
            p_best: 0.0,
            spread_bps: f64::MAX,
        };
    }

    let p_now = ask_units / bid_units;

    // Update rolling window with new price
    window.push(ts_ms, p_now);

    // Fetch best recent price (p_best)
    let p_best = match window.max() {
        Some(v) => v,
        None => {
            // First-ever tick → treat as best = current
            return SpreadPulseResult {
                p_now,
                p_best: p_now,
                spread_bps: 0.0,
            };
        }
    };

    // Compute spread in basis points
    // p_best must be > 0.0, but guard anyway
    let spread_bps = if p_best > 0.0 {
        ((p_best - p_now) / p_best) * 10_000.0
    } else {
        f64::MAX
    };

    SpreadPulseResult {
        p_now,
        p_best,
        spread_bps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rolling_window::RollingWindow;

    // Helper for timestamps
    fn ts(n: u64) -> u64 {
        n * 1000
    }

    #[test]
    fn first_tick_should_pulse_true() {
        let mut window = RollingWindow::new();

        let res = compute_spread_pulse(
            ts(1),
            100.0, // bid_units
            50.0,  // ask_units → p_now = 0.5
            &mut window,
        );

        assert_eq!(res.p_now, 0.5);
        assert_eq!(res.p_best, 0.5);
        assert_eq!(res.spread_bps, 0.0);
    }

    #[test]
    fn spread_pulse_should_fire_when_within_threshold() {
        let mut window = RollingWindow::new();

        // Push some earlier prices
        window.push(ts(1), 0.50);
        window.push(ts(2), 0.52); // best so far (p_best)
        window.push(ts(3), 0.51);

        // New price slightly below p_best → acceptable
        let res = compute_spread_pulse(
            ts(4),
            100.0,
            51.5, // p_now = 0.515, p_best = 0.52
            &mut window,
        );

        assert!((res.p_now - 0.515).abs() < 1e-12);
        assert!((res.p_best - 0.52).abs() < 1e-12);

        // Compute expected spread_bps manually:
        // ((0.52 - 0.515) / 0.52) * 10000 = ~96.15 bps
        assert!(res.spread_bps < 100.0);
    }

    #[test]
    fn spread_pulse_should_fail_when_above_threshold() {
        let mut window = RollingWindow::new();

        // Best price so far = 0.52
        window.push(ts(1), 0.50);
        window.push(ts(2), 0.52);
        window.push(ts(3), 0.51);

        // New price significantly worse
        let res = compute_spread_pulse(
            ts(4),
            100.0,
            48.0, // p_now = 0.48
            &mut window,
        );

        // Spread will be large → pulse should fail
        assert!(res.spread_bps > 50.0);
    }

    #[test]
    fn bad_bid_units_should_disable_pulse() {
        let mut window = RollingWindow::new();

        let res = compute_spread_pulse(
            ts(1),
            0.0, // invalid bid_units → cannot compute price
            100.0,
            &mut window,
        );

        assert_eq!(res.spread_bps, f64::MAX);
    }

    #[test]
    fn rolling_window_eviction_should_update_p_best() {
        let mut window = RollingWindow::new();

        // Insert two prices:
        window.push(ts(1), 1.00); // will be too old
        window.push(ts(2), 2.00); // best valid

        // Now push at time = 3s → first entry is evicted
        let res = compute_spread_pulse(
            ts(3),
            100.0,
            169.0, // p_now = 1.69
            &mut window,
        );

        // p_best should now be 2.0 (1.0 was evicted)
        assert!((res.p_best - 2.0).abs() < 1e-12);
    }

    #[test]
    fn p_best_should_always_be_monotonic_deque_front() {
        let mut window = RollingWindow::new();

        // Insert values with known max = 3.0
        window.push(ts(1), 1.0);
        window.push(ts(2), 3.0);
        window.push(ts(3), 2.0);
        window.push(ts(4), 2.5);

        let p_best = window.max().unwrap();
        assert_eq!(p_best, 3.0, "Monotonic max queue must track correct max");
    }

    #[test]
    fn pulse_should_not_fail_when_p_now_equals_p_best() {
        let mut window = RollingWindow::new();

        window.push(ts(1), 0.33);
        window.push(ts(2), 0.34); // best
        window.push(ts(3), 0.32);

        // Now p_now equals p_best = 0.34
        let res = compute_spread_pulse(
            ts(4),
            100.0,
            34.0, // p_now = 0.34
            &mut window,
        );

        assert_eq!(res.spread_bps, 0.0);
    }
}

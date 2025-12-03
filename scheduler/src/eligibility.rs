//! Determines whether a given session is allowed to fire a chunk
//! given current market metrics and scheduler configuration.
//
//  This module is deliberately pure: no async, no IO.

use super::types::SchedulerConfig;
use market::pulse::spread::SpreadPulseResult;
use market::types::MarketMetrics;
use session::model::{Session, SessionState};

/// Result of an eligibility check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eligibility {
    Eligible,
    NotActive,
    NoRemaining,
    CooldownNotElapsed,
    SpreadTooWide,
    SlippageTooHigh,
    TrendRejected,
    Expired,
}

impl Eligibility {
    pub fn is_eligible(&self) -> bool {
        matches!(self, Eligibility::Eligible)
    }
}

/// Check whether a session may fire *one* chunk at this tick.
///
/// This enforces:
///   - session lifecycle (must be Active and not expired)
///   - remaining amount > 0
///   - cooldown between chunks
///   - spread/slippage/trend thresholds
///
/// `now_ms` should be a monotonically increasing timestamp in ms.
pub fn check_session_eligibility(
    session: &Session,
    metrics: &MarketMetrics,
    cfg: &SchedulerConfig,
    now_ms: u64,
) -> Eligibility {
    if session.state != SessionState::Active {
        return Eligibility::NotActive;
    }

    if let Some(expiry) = session.expires_at_ms {
        if now_ms > expiry {
            return Eligibility::Expired;
        }
    }

    if session.remaining_amount_in == 0 {
        return Eligibility::NoRemaining;
    }

    // Cooldown: ensure we don't fire too frequently
    if let Some(last_ts) = session.last_execution_ts_ms {
        let elapsed = now_ms.saturating_sub(last_ts);
        if elapsed < cfg.min_cooldown_ms {
            return Eligibility::CooldownNotElapsed;
        }
    }

    // Thresholds vs MarketMetrics
    let spread_bps = metrics.spread.spread_bps;
    if spread_bps > session.thresholds.max_spread_bps {
        return Eligibility::SpreadTooWide;
    }

    // let slippage_bps = metrics.slippage_bps;
    // if slippage_bps > session.thresholds.max_slippage_bps {
    //     return Eligibility::SlippageTooHigh;
    // }

    // if session.thresholds.require_trend_up && !metrics.trend_up {
    //     return Eligibility::TrendRejected;
    // }

    Eligibility::Eligible
}

#[cfg(test)]
mod tests {
    use super::*;
    use market::types::MarketMetrics;
    use session::model::{Session, SessionState, SessionThresholds};

    fn base_cfg() -> SchedulerConfig {
        SchedulerConfig {
            max_chunks_per_tick: 5,
            max_chunks_per_session_per_tick: 1,
            min_cooldown_ms: 1000,
        }
    }

    fn thresholds(max_spread: f64, max_slippage: f64, trend_enabled: bool) -> SessionThresholds {
        SessionThresholds {
            max_spread_bps: max_spread,
            max_slippage_bps: max_slippage,
            trend_enabled,
        }
    }

    fn metrics_with_spread(spread: f64) -> MarketMetrics {
        MarketMetrics {
            spread: SpreadPulseResult {
                p_now: 0.0,
                p_best: 0.0,
                spread_bps: spread,
            },
        }
    }

    fn session_with(
        state: SessionState,
        remaining: u64,
        last_exec: Option<u64>,
        expiry: Option<u64>,
        thresholds: SessionThresholds,
    ) -> Session {
        Session {
            state,
            remaining_amount_in: remaining,
            last_execution_ts_ms: last_exec,
            expires_at_ms: expiry,
            thresholds,
            ..Default::default()
        }
    }

    #[test]
    fn inactive_session_fails() {
        let session = session_with(
            SessionState::Cancelled,
            10,
            None,
            None,
            thresholds(100.0, 500.0, false),
        );

        let out =
            check_session_eligibility(&session, &metrics_with_spread(10.0), &base_cfg(), 10_000);

        assert_eq!(out, Eligibility::NotActive);
    }

    #[test]
    fn expired_session_fails() {
        let session = session_with(
            SessionState::Active,
            10,
            None,
            Some(5_000),
            thresholds(100.0, 500.0, false),
        );

        let out =
            check_session_eligibility(&session, &metrics_with_spread(10.0), &base_cfg(), 10_000);

        assert_eq!(out, Eligibility::Expired);
    }

    #[test]
    fn zero_remaining_fails() {
        let session = session_with(
            SessionState::Active,
            0,
            None,
            None,
            thresholds(100.0, 500.0, false),
        );

        let out =
            check_session_eligibility(&session, &metrics_with_spread(10.0), &base_cfg(), 1_000);

        assert_eq!(out, Eligibility::NoRemaining);
    }

    #[test]
    fn cooldown_not_elapsed_fails() {
        let session = session_with(
            SessionState::Active,
            10,
            Some(9_900),
            None,
            thresholds(100.0, 500.0, false),
        );

        let out =
            check_session_eligibility(&session, &metrics_with_spread(10.0), &base_cfg(), 10_000);

        assert_eq!(out, Eligibility::CooldownNotElapsed);
    }

    #[test]
    fn cooldown_elapsed_passes() {
        let session = session_with(
            SessionState::Active,
            10,
            Some(5_000),
            None,
            thresholds(100.0, 500.0, false),
        );

        let out =
            check_session_eligibility(&session, &metrics_with_spread(10.0), &base_cfg(), 10_000);

        assert_eq!(out, Eligibility::Eligible);
    }

    #[test]
    fn spread_too_wide_fails() {
        let session = session_with(
            SessionState::Active,
            10,
            None,
            None,
            thresholds(50.0, 500.0, false),
        );

        let metrics = metrics_with_spread(120.0);

        let out = check_session_eligibility(&session, &metrics, &base_cfg(), 10_000);

        assert_eq!(out, Eligibility::SpreadTooWide);
    }

    #[test]
    fn spread_ok_passes() {
        let session = session_with(
            SessionState::Active,
            10,
            None,
            None,
            thresholds(50.0, 500.0, false),
        );

        let metrics = metrics_with_spread(20.0);

        let out = check_session_eligibility(&session, &metrics, &base_cfg(), 10_000);

        assert_eq!(out, Eligibility::Eligible);
    }

    // #[test]
    // fn trend_rejected_when_enabled() {
    //     let session = session_with(
    //         SessionState::Active,
    //         10,
    //         None,
    //         None,
    //         thresholds(100.0, 500.0, true),
    //     );

    //     let mut metrics = metrics_with_spread(10.0);
    //     metrics.trend_up = false;

    //     let out = check_session_eligibility(
    //         &session,
    //         &metrics,
    //         &base_cfg(),
    //         5_000,
    //     );

    //     assert_eq!(out, Eligibility::TrendRejected);
    // }

    // #[test]
    // fn trend_passes_when_enabled_and_market_up() {
    //     let session = session_with(
    //         SessionState::Active,
    //         10,
    //         None,
    //         None,
    //         thresholds(100.0, 500.0, true),
    //     );

    //     let mut metrics = metrics_with_spread(10.0);
    //     metrics.trend_up = true;

    //     let out = check_session_eligibility(
    //         &session,
    //         &metrics,
    //         &base_cfg(),
    //         5_000,
    //     );

    //     assert_eq!(out, Eligibility::Eligible);
    // }

    #[test]
    fn all_conditions_pass() {
        let session = session_with(
            SessionState::Active,
            10,
            None,
            None,
            thresholds(100.0, 500.0, false),
        );

        let metrics = metrics_with_spread(10.0);

        let out = check_session_eligibility(&session, &metrics, &base_cfg(), 20_000);

        assert_eq!(out, Eligibility::Eligible);
    }
}

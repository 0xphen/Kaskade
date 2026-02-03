use std::collections::HashMap;
use tracing::{debug, instrument};
use uuid::Uuid;

use crate::execution::types::ReservedBatch;
use crate::session::model::Session;

/// Accumulates credit for the session based on its quantum.
///
/// A domain-specific cap is enforced to prevent "burstiness" where a long-inactive
/// session could otherwise accumulate massive credit and monopolize the scheduler.
#[instrument(skip(s), target = "session_logic")]
pub fn accumulate_credit(s: &mut Session) {
    let max_credit = (s.intent.preferred_chunk_bid as i128).saturating_mul(2);

    let next_credit = s.state.deficit.saturating_add(s.state.quantum as i128);

    s.state.deficit = next_credit.min(max_credit);

    debug!(
        session_id = %s.session_id,
        new_deficit = s.state.deficit,
        "credit accumulated"
    );
}

pub fn sum_reserved(batch: &ReservedBatch) -> HashMap<Uuid, (u128, u32)> {
    let mut m: HashMap<Uuid, (u128, u32)> = HashMap::new();
    for u in &batch.users {
        let total_bid: u128 = u.chunks.iter().map(|c| c.bid).sum();
        let total_chunks: u32 = u.chunks.len() as u32;
        m.insert(u.session_id, (total_bid, total_chunks));
    }
    m
}

/// Can this session afford serving a chunk of `cost_bid`?
pub fn can_serve(s: &Session, cost_bid: u128) -> bool {
    s.state.deficit >= cost_bid as i128
}

/// Charge deficit by the served cost.
pub fn charge(s: &mut Session, cost_bid: u128) {
    s.state.deficit = s.state.deficit.saturating_sub(cost_bid as i128);

    debug!(
        session_id = %s.session_id,
        remaining_deficit = s.state.deficit,
        "session charged"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::model::{Session, SessionIntent, SessionState, UserConstraints};
    use uuid::Uuid;

    fn mk_test_session(deficit: i128, quantum: u128, preferred_bid: u128) -> Session {
        Session {
            session_id: Uuid::new_v4(),
            pair_id: "TON/USDT".to_string(),
            active: true,
            intent: SessionIntent {
                constraints: UserConstraints {
                    max_spread_bps: 50.0,
                    max_trend_drop_bps: 100.0,
                    max_slippage_bps: 75.0,
                },
                preferred_chunk_bid: preferred_bid,
                max_bid_per_tick: 1_000_000,
            },
            state: SessionState {
                remaining_bid: 1_000_000,
                remaining_chunks: 10,
                in_flight_bid: 0,
                in_flight_chunks: 0,
                cooldown_until_ms: 0,
                quantum,
                deficit,
                last_served_ms: 0,
                has_pending_batch: false,
            },
        }
    }

    #[test]
    fn test_accumulate_credit_basic() {
        let mut s = mk_test_session(100, 50, 1000);
        accumulate_credit(&mut s);
        assert_eq!(s.state.deficit, 150, "Deficit should increase by quantum");
    }

    #[test]
    fn test_accumulate_credit_respects_cap() {
        // preferred_chunk_bid = 1000, so max_credit = 2000
        let mut s = mk_test_session(1900, 200, 1000);
        accumulate_credit(&mut s);

        assert_eq!(
            s.state.deficit, 2000,
            "Deficit should be capped at 2x preferred_chunk_bid"
        );
    }

    #[test]
    fn test_accumulate_credit_saturating_overflow() {
        // Testing that i128 doesn't panic even if quantum + deficit is massive
        let mut s = mk_test_session(i128::MAX - 10, u128::MAX, 1000);
        accumulate_credit(&mut s);

        // Even with i128::MAX internally, the domain cap should still win
        assert_eq!(s.state.deficit, 2000);
    }

    #[test]
    fn test_can_serve_boundaries() {
        let s = mk_test_session(500, 0, 1000);

        assert!(can_serve(&s, 400), "Should serve below deficit");
        assert!(can_serve(&s, 500), "Should serve exactly at deficit");
        assert!(!can_serve(&s, 501), "Should not serve above deficit");
    }

    #[test]
    fn test_charge_basic() {
        let mut s = mk_test_session(500, 0, 1000);
        charge(&mut s, 200);
        assert_eq!(s.state.deficit, 300);
    }

    #[test]
    fn test_charge_saturating_underflow() {
        let mut s = mk_test_session(100, 0, 1000);
        // Charge more than available (logic should handle via saturating_sub)
        charge(&mut s, 200);

        // Deficit is i128. If using saturating_sub on i128:
        // 100 - 200 = -100. (Note: standard i128 sub allows negative)
        assert_eq!(s.state.deficit, -100);
    }
}

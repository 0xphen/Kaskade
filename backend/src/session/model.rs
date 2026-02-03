use uuid::Uuid;

/// Per-session execution constraints (Gate A + Gate B).
/// Values are expressed in basis points (bps).
#[derive(Clone, Debug)]
pub struct UserConstraints {
    /// Max allowed spread (bps) at execution time.
    pub max_spread_bps: f64,
    /// Max allowed adverse trend / drop metric (bps).
    pub max_trend_drop_bps: f64,
    /// Max allowed estimated slippage (bps).
    pub max_slippage_bps: f64,
}

/// User preferences and per-tick sizing limits for scheduling.
/// These are inputs to the scheduler (hints/caps), not guarantees.
#[derive(Clone, Debug)]
pub struct SessionIntent {
    pub constraints: UserConstraints,

    /// Preferred chunk size used by the scheduler when creating per-tick intents.
    pub preferred_chunk_bid: u128,
    /// Upper bound on volume this session should execute per tick.
    pub max_bid_per_tick: u128,
}

/// Runtime state for a session.
/// `remaining_*` tracks total work left.
/// `in_flight_*` tracks reserved-but-not-committed work (restart-safe).
#[derive(Clone, Debug)]
pub struct SessionState {
    /// Remaining volume to execute for this session overall.
    pub remaining_bid: u128,
    /// Remaining number of chunks to execute overall (if chunk-count-limited).
    pub remaining_chunks: u32,

    /// Reserved volume not yet committed (unwound on commit/recovery).
    pub in_flight_bid: u128,
    /// Reserved chunk count not yet committed (unwound on commit/recovery).
    pub in_flight_chunks: u32,

    /// Backoff timestamp; session is not eligible while now_ms < cooldown_until_ms.
    pub cooldown_until_ms: u64,

    /// DRR quantum added during scheduling to accumulate service credit.
    pub quantum: u128,
    /// DRR deficit (service credit). User can be served if deficit >= cost.
    pub deficit: i128,
    /// Last time this session was served (observability / optional aging).
    pub last_served_ms: u64,

    /// True if a batch is RESERVED but not yet committed / aborted.
    pub has_pending_batch: bool,
}

/// A single user automation session for a specific trading pair.
#[derive(Clone, Debug)]
pub struct Session {
    pub session_id: Uuid,
    pub pair_id: String,
    pub active: bool,
    pub intent: SessionIntent,
    pub state: SessionState,
}

impl Session {
    /// Remaining volume that is not already reserved in-flight.
    pub fn available_bid(&self) -> u128 {
        self.state
            .remaining_bid
            .saturating_sub(self.state.in_flight_bid)
    }

    /// Remaining chunk capacity that is not already reserved in-flight.
    pub fn available_chunks(&self) -> u32 {
        self.state
            .remaining_chunks
            .saturating_sub(self.state.in_flight_chunks)
    }

    /// Hard eligibility check (independent of fairness).
    /// Returns true if the session can be considered for scheduling.
    pub fn is_eligible(&self, now_ms: u64) -> bool {
        self.active
            && self.state.cooldown_until_ms <= now_ms
            && self.available_bid() > 0
            && self.available_chunks() > 0
    }

    /// DRR fairness check: does the session have enough accumulated credit?
    pub fn has_sufficient_credit(&self) -> bool {
        self.state.deficit >= self.intent.preferred_chunk_bid as i128
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn mk_session(
        remaining_bid: u128,
        in_flight_bid: u128,
        remaining_chunks: u32,
        in_flight_chunks: u32,
        cooldown_until_ms: u64,
        active: bool,
    ) -> Session {
        Session {
            session_id: Uuid::new_v4(),
            pair_id: "TON/USDT".to_string(),
            active,
            intent: SessionIntent {
                constraints: UserConstraints {
                    max_spread_bps: 50.0,
                    max_trend_drop_bps: 100.0,
                    max_slippage_bps: 75.0,
                },
                preferred_chunk_bid: 100_000,
                max_bid_per_tick: 1_000_000,
            },
            state: SessionState {
                remaining_bid,
                remaining_chunks,
                in_flight_bid,
                in_flight_chunks,
                cooldown_until_ms,
                quantum: 100_000,
                deficit: 0,
                last_served_ms: 0,
                has_pending_batch: false,
            },
        }
    }

    fn eligible_basic(s: &Session, now_ms: u64) -> bool {
        s.active
            && s.state.cooldown_until_ms <= now_ms
            && s.available_bid() > 0
            && s.available_chunks() > 0
    }

    #[test]
    fn available_bid_is_remaining_minus_in_flight() {
        let s = mk_session(10_000, 2_500, 10, 1, 0, true);
        assert_eq!(s.available_bid(), 7_500);
    }

    #[test]
    fn available_bid_is_zero_when_equal() {
        let s = mk_session(10_000, 10_000, 10, 1, 0, true);
        assert_eq!(s.available_bid(), 0);
    }

    #[test]
    fn available_bid_saturates_to_zero_on_inconsistent_state() {
        // If in_flight exceeds remaining (should not happen in healthy DB state),
        // saturating_sub prevents underflow and clamps availability to 0.
        let s = mk_session(1_000, 2_000, 10, 1, 0, true);
        assert_eq!(s.available_bid(), 0);
    }

    #[test]
    fn available_chunks_is_remaining_minus_in_flight() {
        let s = mk_session(10_000, 0, 10, 3, 0, true);
        assert_eq!(s.available_chunks(), 7);
    }

    #[test]
    fn available_chunks_is_zero_when_equal() {
        let s = mk_session(10_000, 0, 5, 5, 0, true);
        assert_eq!(s.available_chunks(), 0);
    }

    #[test]
    fn available_chunks_saturates_to_zero_on_inconsistent_state() {
        // Same defensive behavior as available_bid().
        let s = mk_session(10_000, 0, 1, 2, 0, true);
        assert_eq!(s.available_chunks(), 0);
    }

    #[test]
    fn eligibility_false_when_inactive() {
        let now = 1_000;
        let s = mk_session(10_000, 0, 10, 0, 0, false);
        assert!(!eligible_basic(&s, now));
    }

    #[test]
    fn eligibility_false_when_in_cooldown() {
        let now = 1_000;
        let s = mk_session(10_000, 0, 10, 0, 2_000, true);
        assert!(!eligible_basic(&s, now));
    }

    #[test]
    fn eligibility_true_at_or_after_cooldown_boundary() {
        let s = mk_session(10_000, 0, 10, 0, 1_000, true);
        assert!(eligible_basic(&s, 1_000)); // boundary inclusive
        assert!(eligible_basic(&s, 1_001));
    }

    #[test]
    fn eligibility_false_when_no_available_bid() {
        let now = 1_000;
        let s = mk_session(10_000, 10_000, 10, 0, 0, true);
        assert_eq!(s.available_bid(), 0);
        assert!(!eligible_basic(&s, now));
    }

    #[test]
    fn eligibility_false_when_no_available_chunks() {
        let now = 1_000;
        let s = mk_session(10_000, 0, 3, 3, 0, true);
        assert_eq!(s.available_chunks(), 0);
        assert!(!eligible_basic(&s, now));
    }

    #[test]
    fn eligibility_true_when_active_not_in_cooldown_and_has_availability() {
        let now = 1_000;
        let s = mk_session(10_000, 2_000, 10, 2, 0, true);
        assert!(eligible_basic(&s, now));
    }

    // --- regression-style combined tests ---

    #[test]
    fn availability_updates_match_expected_cases() {
        // Case A: partial reservation
        let s = mk_session(50_000, 10_000, 10, 4, 0, true);
        assert_eq!(s.available_bid(), 40_000);
        assert_eq!(s.available_chunks(), 6);

        // Case B: fully reserved
        let s = mk_session(50_000, 50_000, 10, 10, 0, true);
        assert_eq!(s.available_bid(), 0);
        assert_eq!(s.available_chunks(), 0);

        // Case C: inconsistent (defensive clamp)
        let s = mk_session(1, 2, 0, 1, 0, true);
        assert_eq!(s.available_bid(), 0);
        assert_eq!(s.available_chunks(), 0);
    }

    #[test]
    fn test_drr_deficit_accumulation() {
        let mut s = mk_session(100_000, 0, 10, 0, 0, true);
        let initial_deficit = s.state.deficit;

        // Simulate one scheduler round
        s.state.deficit += s.state.quantum as i128;

        assert_eq!(s.state.deficit, initial_deficit + 100_000);
    }

    #[test]
    fn eligibility_fails_on_zero_remaining_even_if_no_inflight() {
        // Session has no work left to do at all
        let s = mk_session(0, 0, 0, 0, 0, true);
        assert!(!eligible_basic(&s, 100));
    }

    #[test]
    fn test_drr_credit_flow() {
        let mut s = mk_session(100_000, 0, 10, 0, 0, true);

        // 1. Initially deficit is 0, preferred_chunk is 100k. Should be ineligible.
        assert!(
            !s.has_sufficient_credit(),
            "Should not have credit with 0 deficit"
        );

        // 2. Add quantum (100k). Now it should match the preferred chunk.
        s.state.deficit += s.state.quantum as i128;
        assert!(
            s.has_sufficient_credit(),
            "Should be eligible after adding quantum"
        );

        // 3. Subtract cost (simulate execution). Should become ineligible again.
        s.state.deficit -= 100_000;
        assert!(
            !s.has_sufficient_credit(),
            "Should be ineligible after spending credit"
        );
    }

    #[test]
    fn test_drr_multi_tick_accumulation() {
        let mut s = mk_session(1_000_000, 0, 10, 0, 0, true);
        s.intent.preferred_chunk_bid = 50_000;
        s.state.quantum = 20_000;

        // Tick 1: 20k credit < 50k cost
        s.state.deficit += s.state.quantum as i128;
        assert!(!s.has_sufficient_credit());

        // Tick 2: 40k credit < 50k cost
        s.state.deficit += s.state.quantum as i128;
        assert!(!s.has_sufficient_credit());

        // Tick 3: 60k credit > 50k cost -> ELIGIBLE
        s.state.deficit += s.state.quantum as i128;
        assert!(s.has_sufficient_credit());
    }

    #[test]
    fn test_eligibility_with_residual_dust() {
        // User wants 100k chunks, but only has 500 total left.
        let s = mk_session(500, 0, 10, 0, 0, true);
        // They are still "eligible" for the scheduler to try and squeeze out a final trade.
        assert!(s.is_eligible(100));
    }
}

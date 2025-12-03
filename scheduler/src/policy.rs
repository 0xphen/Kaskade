//! Selection policy: given a list of eligible sessions, choose
//! a *subset* to actually fire on this tick.

use super::state::PairSchedulerState;
use super::types::SchedulerConfig;
use session::model::SessionId;

/// A lightweight handle used during selection.
/// One entry ~= "this session could fire one chunk".
#[derive(Debug, Clone)]
pub struct EligibleHandle {
    pub session_id: SessionId,
    pub chunk_amount_in: u64,
}

/// Select a subset of eligible sessions to fire this tick, using
/// a round-robin starting from the pair's rr_index.
///
/// Constraints enforced:
///   - At most `cfg.max_chunks_per_tick` chunks this tick.
///   - At most 1 chunk per session this tick.
pub fn select_sessions_round_robin(
    pair_state: &mut PairSchedulerState,
    cfg: &SchedulerConfig,
    eligible: &[EligibleHandle],
) -> Vec<EligibleHandle> {
    let n = eligible.len();
    if n == 0 {
        return Vec::new();
    }

    let max_to_take = cfg.max_chunks_per_tick.min(n);
    if max_to_take == 0 {
        return Vec::new();
    }

    // Ensure rr_index is in range
    if pair_state.rr_index >= n {
        pair_state.rr_index = 0;
    }

    let mut selected = Vec::with_capacity(max_to_take);
    let mut seen_sessions = std::collections::HashSet::new();

    let mut taken = 0;
    let mut idx = pair_state.rr_index;

    // Walk through eligible list, wrapping around once.
    // Stop when we've selected enough or examined all entries.
    let max_steps = n; // at most one full loop
    for _ in 0..max_steps {
        if taken >= max_to_take {
            break;
        }

        let handle = &eligible[idx];

        // Ensure we don't select more than 1 chunk per session per tick (MVP constraint).
        if !seen_sessions.contains(&handle.session_id) {
            selected.push(handle.clone());
            seen_sessions.insert(handle.session_id);
            taken += 1;
        }

        idx = (idx + 1) % n;
    }

    // Update rr_index for the next tick.
    pair_state.rr_index = idx;

    selected
}

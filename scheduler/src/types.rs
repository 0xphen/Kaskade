//! Shared types used by the scheduler subsystem.

use std::fmt;
use std::sync::Arc;

use tokio::sync::mpsc::Sender;

use market::types::Pair;
use session::model::SessionId;
use session::{manager::SessionManager, store::SessionStore};

/// Configuration knobs for the scheduler.
///
/// These are global limits and timing parameters.
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Maximum number of chunks the scheduler is allowed to fire per tick
    /// for a given pair.
    pub max_chunks_per_tick: usize,

    /// Maximum number of chunks the scheduler may fire for a *single session*
    /// in one tick.
    pub max_chunks_per_session_per_tick: usize,

    /// Minimum elapsed time (ms) between two chunks for the same session.
    /// Enforced in eligibility logic using last_execution_ts_ms.
    pub min_cooldown_ms: u64,
}

/// A request sent from the scheduler to the executor.
///
/// This represents: "Fire one chunk for this session on this pair".
#[derive(Clone)]
pub struct ExecutionRequest {
    pub session_id: SessionId,
    pub pair: Pair,
    pub amount_in: u64,
}

impl fmt::Debug for ExecutionRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecutionRequest")
            .field("session_id", &self.session_id)
            .field("pair", &self.pair)
            .field("amount_in", &self.amount_in)
            .finish()
    }
}

/// Convenience alias for the executor job queue type.
///
/// The executor will receive ExecutionRequest jobs from the scheduler.
pub type ExecutionSender = Sender<ExecutionRequest>;

/// Trait alias for the SessionManager used by the scheduler.
///
/// You can later replace this with an explicit trait if you want.
pub type SharedSessionManager<S> = Arc<SessionManager<S>>;

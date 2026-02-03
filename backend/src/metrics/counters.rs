use std::sync::Arc;
use std::sync::atomic::AtomicU64;

/// Minimal counters for operational visibility.
#[derive(Clone, Default)]
pub struct Counters {
    pub sched_batches: Arc<AtomicU64>,
    pub sched_selected: Arc<AtomicU64>,

    pub sched_empty: Arc<AtomicU64>,
    pub sched_skip_pending: Arc<AtomicU64>,
    pub sched_no_alloc: Arc<AtomicU64>,

    // skip reasons
    pub sched_skip_inactive: Arc<AtomicU64>,
    pub sched_skip_cooldown: Arc<AtomicU64>,
    pub sched_skip_empty: Arc<AtomicU64>,
    pub sched_skip_constraints: Arc<AtomicU64>,
    pub sched_skip_deficit: Arc<AtomicU64>,
}

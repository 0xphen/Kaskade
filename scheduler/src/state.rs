//! Internal scheduler state.
//! Primarily used to support fair selection policies (e.g. round-robin).

use std::collections::HashMap;

use market::types::Pair;

/// Per-pair scheduler state.
///
/// Currently only stores a round-robin index into the eligible session list.
#[derive(Debug, Default, Clone)]
pub struct PairSchedulerState {
    /// Index of the next session to start from when selecting eligible sessions.
    pub rr_index: usize,
}

/// Global scheduler state keyed by Pair.
///
/// `SchedulerState` is typically wrapped in Arc<Mutex<...>> by SchedulerEngine.
#[derive(Debug, Default)]
pub struct SchedulerState {
    inner: HashMap<Pair, PairSchedulerState>,
}

impl SchedulerState {
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    pub fn pair_mut(&mut self, pair: &Pair) -> &mut PairSchedulerState {
        self.inner.entry(pair.clone()).or_default()
    }
}

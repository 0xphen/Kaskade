pub mod executor;
pub mod types;

use crate::execution::types::ReservedBatch;
use crate::planner::types::PlannedAllocation;
use crate::session::store::SessionStore;
use types::UserResult;

pub async fn commit_batch(
    store: &SessionStore,
    batch: &ReservedBatch,
    results: &[UserResult],
) -> anyhow::Result<()> {
    store.repo.commit_batch(batch, results).await
}

pub async fn reserve_execution(
    store: &SessionStore,
    pair_id: &str,
    now_ms: u64,
    allocations: &[PlannedAllocation],
) -> anyhow::Result<Option<ReservedBatch>> {
    if allocations.is_empty() {
        anyhow::bail!("attempted to reserve empty allocation set");
    }

    for a in allocations {
        if a.chunks.is_empty() {
            anyhow::bail!("allocation with zero chunks");
        }
    }

    store
        .repo
        .reserve_execution(pair_id, now_ms, allocations)
        .await
}

pub fn u128_to_i64(v: u128) -> anyhow::Result<i64> {
    if v > i64::MAX as u128 {
        anyhow::bail!("u128 too large for i64: {v}");
    }
    Ok(v as i64)
}

pub fn u32_to_i64(v: u32) -> anyhow::Result<i64> {
    Ok(v as i64)
}

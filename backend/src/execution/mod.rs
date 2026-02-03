pub mod executor;
pub mod types;

use crate::execution::types::ReservedBatch;
use crate::planner::types::PlannedAllocation;
use crate::session::store::SessionStore;
use types::UserResult;

/// Entry point for execution recovery at the execution layer.
///
/// This function deliberately contains no recovery logic.
/// It acts as a stable orchestration boundary and delegates
/// recovery to the persistence layer.
///
/// Semantics:
/// - must be safe to call on startup
/// - must be idempotent
pub async fn recover_uncommitted(store: &SessionStore) -> anyhow::Result<()> {
    store.repo.recover_uncommitted().await
}

/// Commits the results of a previously reserved batch.
///
/// This function does not interpret results or mutate state directly.
/// All consistency, idempotency, and state transitions are delegated
/// to the repository layer.
///
/// The execution layer treats this as a single atomic commit operation.
pub async fn commit_batch(
    store: &SessionStore,
    batch: &ReservedBatch,
    results: &[UserResult],
) -> anyhow::Result<()> {
    store.repo.commit_batch(batch, results).await
}

/// Attempts to reserve execution capacity for a set of planned allocations.
///
/// This function performs **only input validation and delegation**.
/// All concurrency control, atomicity, and persistence guarantees
/// are enforced by the repository implementation.
///
/// Validation here is intentional:
/// - empty allocation sets indicate a planner bug
/// - allocations with zero chunks are invalid execution requests
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

/// Narrowing helper used at the execution–persistence boundary.
///
/// This function exists to make overflow explicit and non-silent
/// when converting execution-domain values into storage-domain types.
pub fn u128_to_i64(v: u128) -> anyhow::Result<i64> {
    if v > i64::MAX as u128 {
        anyhow::bail!("u128 too large for i64: {v}");
    }
    Ok(v as i64)
}

/// Lossless numeric conversion helper.
///
/// Provided for symmetry and clarity at the boundary layer.
pub fn u32_to_i64(v: u32) -> anyhow::Result<i64> {
    Ok(v as i64)
}

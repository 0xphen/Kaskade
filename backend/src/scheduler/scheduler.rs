//! Scheduler for condition-based execution.
//!
//! Responsibilities:
//! - Select eligible sessions for a single pair each tick (Gate A).
//! - Enforce long-term fairness using Deficit Round Robin (DRR).
//! - Convert selected intents into bounded, chunked allocations via the planner.
//! - Atomically reserve a batch in persistent storage and enqueue it for execution.
//!
//! Non-responsibilities:
//! - Executing swaps (executor worker does this).
//! - Fee calculation/collection (enforced in the on-chain EMC / execution layer).
//! - Final market validation (Gate B happens in the executor per-chunk).
//!
//! Safety/liveness properties:
//! - Work per tick is bounded by `max_attempts` and `max_users_per_batch`.
//! - DRR prevents starvation over time (provided sessions are revisited).
//! - Reservations are restart-safe: if enqueue fails, recovery unwinds RESERVED batches.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::Sender;
use tracing::{debug, field, info, instrument, warn};
use uuid::Uuid;

use crate::execution::reserve_execution;
use crate::execution::types::{ExecutionEvent, ReservedBatch};
use crate::logger::warn_if_slow;
use crate::market::types::MarketMetricsView;
use crate::metrics::counters::Counters;
use crate::planner::sizing::derive_execution_plan;
use crate::planner::types::{PlannedAllocation, SizingPolicy, UserIntent as PlannerUserIntent};
use crate::scheduler::drr;
use crate::session::model::Session;
use crate::session::store::SessionStore;

/// Scheduler drives one scheduling tick for a single trading pair.
///
/// It selects a bounded set of sessions using RR scanning + DRR fairness, applies
/// Gate A constraints, then reserves a batch (DB truth) and enqueues it.
pub struct Scheduler {
    /// Source of truth for sessions (DB-backed) + bounded in-memory cache.
    store: Arc<SessionStore>,

    /// Execution sizing policy (depth utilization, chunk sizes, caps).
    policy: SizingPolicy,

    /// Minimum number of cached candidates before attempting selection.
    /// Helps avoid bias toward a small hot subset.
    candidate_min: usize,

    /// Upper bound on RR scans per tick (CPU bound / liveness).
    max_attempts: usize,

    /// Upper bound on selected users per batch (executor bound).
    max_users_per_batch: usize,

    /// Observability counters (does not affect behavior).
    counters: Counters,
}

impl Scheduler {
    pub fn new(
        store: Arc<SessionStore>,
        candidate_min: usize,
        max_attempts: usize,
        max_users_per_batch: usize,
        counters: Counters,
    ) -> Self {
        Self {
            store,
            policy: SizingPolicy::default(),
            candidate_min,
            max_attempts,
            max_users_per_batch: max_users_per_batch.max(1),
            counters,
        }
    }

    /// Executes one scheduling tick for `pair_id`.
    ///
    /// Flow:
    /// 1) Ensure enough candidates are cached.
    /// 2) Select intents (RR scan + DRR + Gate A).
    /// 3) Planner derives chunked allocations bounded by market depth & caps.
    /// 4) Atomically reserve the batch in the DB.
    /// 5) Enqueue the reserved batch to the executor.
    ///
    /// If enqueue fails (executor down/closed), the batch remains RESERVED and
    /// is safely handled by restart recovery.
    #[instrument(
        skip(self, market, exec_tx),
        target = "scheduler",
        fields(pair_id = %pair_id, batch_id = field::Empty)
    )]
    pub async fn on_tick(
        &self,
        pair_id: &str,
        market: MarketMetricsView,
        exec_tx: Sender<ExecutionEvent>,
        now_ms: u64,
    ) -> anyhow::Result<()> {
        debug!("starting scheduling tick");

        // Load more sessions into the cache if we are below the minimum candidate set.
        self.store.ensure_candidates(self.candidate_min).await?;

        // Gate A + fairness selection.
        let intents = self.pick_intents(pair_id, &market, now_ms).await?;
        if intents.is_empty() {
            self.counters
                .sched_empty
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            debug!("no eligible intents selected in this tick");
            return Ok(());
        }

        // Convert intents into concrete allocations (including chunk splitting).
        // Depth is applied here as a capacity limiter (not as a binary gate).
        let allocations: Vec<PlannedAllocation> =
            derive_execution_plan(&market, &intents, &self.policy);

        if allocations.is_empty() {
            self.counters
                .sched_no_alloc
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            debug!(
                intent_count = intents.len(),
                "planner produced zero allocations"
            );
            return Ok(());
        }

        let batch_opt: Option<ReservedBatch> =
            warn_if_slow("reserve_execution", Duration::from_millis(100), async {
                reserve_execution(self.store.as_ref(), pair_id, now_ms, &allocations).await
            })
            .await?;

        let batch = match batch_opt {
            Some(b) => b,
            None => {
                self.counters
                    .sched_empty
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                tracing::debug!("reserve_execution reserved nothing; ending tick");
                return Ok(());
            }
        };

        let reserved = drr::sum_reserved(&batch);

        for (sid, (total_bid, total_chunks)) in reserved {
            if let Some(mut s) = self.store.get_cached(&sid) {
                s.state.last_served_ms = now_ms;
                s.state.in_flight_bid += total_bid;
                s.state.in_flight_chunks += total_chunks;
                s.state.has_pending_batch = true;

                self.store.upsert_cache(s);
            }
        }

        // Make batch_id available for all logs emitted under this span.
        tracing::Span::current().record("batch_id", field::display(&batch.batch_id));

        // Enqueue for execution; if this fails, recovery will unwind.
        if exec_tx
            .send(ExecutionEvent::Reserved(batch.clone()))
            .await
            .is_err()
        {
            warn!(
                batch_id = %batch.batch_id,
                "executor queue closed; batch will be recovered on restart"
            );
        }

        self.counters
            .sched_batches
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        info!(
            users = %batch.users.len(),
            batch_id = %batch.batch_id,
            "scheduled batch successfully"
        );

        Ok(())
    }

    /// Selects a fair, bounded set of planner-level intents for this tick.
    ///
    /// Selection:
    /// - Round-robin scanning over cached candidates (bounded by `max_attempts`)
    /// - Gate A constraints applied at schedule time
    /// - DRR fairness applied to avoid starvation
    ///
    /// Durability:
    /// - DRR deficit is persisted so restarts/cache evictions do not reset fairness.
    #[instrument(skip(self, _market), target = "scheduler")]
    async fn pick_intents(
        &self,
        pair_id: &str,
        _market: &MarketMetricsView,
        now_ms: u64,
    ) -> anyhow::Result<Vec<PlannerUserIntent>> {
        let mut out = Vec::new();
        let mut served_this_tick = HashSet::<Uuid>::new();

        let mut attempts = 0usize;

        while attempts < self.max_attempts && out.len() < self.max_users_per_batch {
            attempts += 1;

            let sid = match self.store.rotate_candidate() {
                Some(x) => x,
                None => break,
            };

            let mut s = match self.store.get_cached(&sid) {
                Some(x) => x,
                None => continue,
            };

            if served_this_tick.contains(&s.session_id) {
                continue;
            }

            // --- filters (active, cooldown, pending, constraints, availability) ---

            // DRR step 1: accumulate credit ONCE
            drr::accumulate_credit(&mut s);

            let want = s
                .intent
                .preferred_chunk_bid
                .min(s.intent.max_bid_per_tick)
                .min(s.available_bid());

            if want == 0 {
                self.store.upsert_cache(s);
                continue;
            }

            if !drr::can_serve(&s, want) {
                self.store.upsert_cache(s);
                continue;
            }

            // DRR step 2: charge EXACTLY ONCE
            drr::charge(&mut s, want);
            s.state.last_served_ms = now_ms;

            served_this_tick.insert(s.session_id);

            self.store.upsert_cache(s.clone());

            // Persist fairness (correct place)
            self.store
                .persist_fairness(&s.session_id, s.state.deficit, s.state.last_served_ms)
                .await?;

            out.push(PlannerUserIntent {
                session_id: s.session_id,
                desired_bid: want,
                desired_chunks: 1.min(s.available_chunks()),
            });
        }

        Ok(out)
    }
}

/// Gate A: checks whether the current market state satisfies the session's constraints.
///
/// This gate is intentionally conservative: the executor re-checks constraints
/// (Gate B) immediately before each chunk is executed.
pub fn constraints_ok(s: &Session, m: &MarketMetricsView) -> bool {
    m.spread_bps <= s.intent.constraints.max_spread_bps
        && m.trend_drop_bps <= s.intent.constraints.max_trend_drop_bps
        && m.slippage_bps <= s.intent.constraints.max_slippage_bps
}

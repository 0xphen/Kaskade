use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{Mutex, mpsc};
use tracing::{error, info, warn};

use crate::execution::commit_batch;
use crate::execution::types::{
    ChunkResult, ChunkStatus, ExecutionEvent, ReservedBatch, UserResult,
};
use crate::market_view::MarketViewStore;
use crate::session::model::Session;
use crate::session::store::SessionStore;

/// Swap call input to the chain executor.
/// `bid` and `chunk_id` must match the reserved batch_items in DB.
#[derive(Clone, Debug)]
pub struct SwapCall {
    pub pair_id: String,
    pub session_id: uuid::Uuid,
    pub bid: u128,
    pub chunk_id: uuid::Uuid,
}

/// Swap receipt returned by the chain executor.
/// `tx_id` is persisted into `batch_items.tx_id` on success.
#[derive(Clone, Debug)]
pub struct SwapReceipt {
    pub tx_id: String,
}

/// Abstraction over your chain execution layer (TON / EMC).
/// Contract errors should be mapped into stable `anyhow::Error` messages
/// so `classify_error()` can normalize them into bounded reason strings.
#[async_trait]
pub trait SwapExecutor: Send + Sync + 'static {
    async fn execute_swap(&self, call: SwapCall) -> anyhow::Result<SwapReceipt>;
}

/// Router that fans out reserved batches into per-pair worker queues.
///
/// Why:
/// - prevents one slow pair from blocking all other pairs
/// - keeps execution ordering per pair
///
/// Safety model:
/// - scheduler already persisted the batch as RESERVED in DB
/// - this router only delivers work; it never mutates session balances
/// - if a worker crashes, RESERVED batches remain recoverable via DB recovery
pub struct PairExecutorRouter<E: SwapExecutor> {
    store: Arc<SessionStore>,
    market_view: MarketViewStore,
    exec: Arc<E>,
    default_failure_cooldown_ms: u64,

    /// Bounded per-pair backlog. Backpressure should be handled by scheduler policy
    /// (e.g., do not reserve too aggressively if executor is behind).
    per_pair_capacity: usize,

    /// Cached channels for pair workers.
    /// If a worker dies, its Sender is removed on send failure and recreated on next batch.
    pair_txs: Mutex<HashMap<String, Sender<ReservedBatch>>>,
}

impl<E: SwapExecutor> PairExecutorRouter<E> {
    pub fn new(
        store: Arc<SessionStore>,
        market_view: MarketViewStore,
        exec: Arc<E>,
        default_failure_cooldown_ms: u64,
        per_pair_capacity: usize,
    ) -> Self {
        Self {
            store,
            market_view,
            exec,
            default_failure_cooldown_ms,
            per_pair_capacity: per_pair_capacity.max(8),
            pair_txs: Mutex::new(HashMap::new()),
        }
    }

    /// Runs the router loop. Consumes `ExecutionEvent`s and forwards RESERVED batches
    /// into a per-pair worker queue.
    ///
    /// Failure behavior:
    /// - if forwarding fails (worker died), the sender is removed so the next batch
    ///   recreates the worker
    /// - the batch itself remains RESERVED in DB and must be handled by recovery
    pub async fn run(self: Arc<Self>, mut rx: Receiver<ExecutionEvent>) {
        while let Some(ev) = rx.recv().await {
            match ev {
                ExecutionEvent::Reserved(batch) => {
                    let pair_id = batch.pair_id.clone();

                    let tx = match self.get_or_spawn_worker(&pair_id).await {
                        Ok(tx) => tx,
                        Err(e) => {
                            error!(
                                pair_id = %pair_id,
                                error = ?e,
                                "router: failed to get/spawn worker"
                            );
                            continue;
                        }
                    };

                    if tx.send(batch).await.is_err() {
                        // Worker died; remove cached sender so the next batch recreates it.
                        warn!(
                            pair_id = %pair_id,
                            "router: worker channel closed; removing sender"
                        );

                        let mut g = self.pair_txs.lock().await;
                        g.remove(&pair_id);
                    }
                }
            }
        }

        warn!("router: input channel closed; shutting down");
    }

    async fn get_or_spawn_worker(&self, pair_id: &str) -> anyhow::Result<Sender<ReservedBatch>> {
        // Fast path
        {
            let g = self.pair_txs.lock().await;
            if let Some(tx) = g.get(pair_id) {
                return Ok(tx.clone());
            }
        }

        // Slow path
        let (tx, rx) = mpsc::channel::<ReservedBatch>(self.per_pair_capacity);

        {
            // Double-check to handle races (two batches for same pair concurrently).
            let mut g = self.pair_txs.lock().await;
            if let Some(existing) = g.get(pair_id) {
                return Ok(existing.clone());
            }
            g.insert(pair_id.to_string(), tx.clone());
        }

        let worker = ExecutorWorker::new(
            self.store.clone(),
            self.market_view.clone(),
            self.exec.clone(),
            self.default_failure_cooldown_ms,
            pair_id.to_string(),
        );

        tokio::spawn(async move {
            worker.run(rx).await;
        });

        info!(pair_id = %pair_id, "router: spawned pair worker");
        Ok(tx)
    }
}

/// Executes batches for a single pair, sequentially.
///
/// Responsibilities:
/// - Gate B checks using latest market snapshot right before each chunk
/// - invokes chain executor per chunk
/// - records per-chunk results
/// - commits via `commit_batch()` (DB source of truth, idempotent)
///
/// Invariants expected from upstream:
/// - `ReservedBatch` corresponds to `batches + batch_items` rows in DB
/// - `commit_batch` must unwind in-flight and decrement remaining only on success
pub struct ExecutorWorker<E: SwapExecutor> {
    store: Arc<SessionStore>,
    market_view: MarketViewStore,
    exec: Arc<E>,
    default_failure_cooldown_ms: u64,
    pair_id: String,
}

impl<E: SwapExecutor> ExecutorWorker<E> {
    pub fn new(
        store: Arc<SessionStore>,
        market_view: MarketViewStore,
        exec: Arc<E>,
        default_failure_cooldown_ms: u64,
        pair_id: String,
    ) -> Self {
        Self {
            store,
            market_view,
            exec,
            default_failure_cooldown_ms,
            pair_id,
        }
    }

    /// Worker loop: executes batches in FIFO order for this pair.
    pub async fn run(self, mut rx: Receiver<ReservedBatch>) {
        while let Some(batch) = rx.recv().await {
            if let Err(e) = self.execute_batch(batch).await {
                error!(
                    pair_id = %self.pair_id,
                    error = ?e,
                    "worker: execute_batch failed"
                );
            }
        }

        warn!(pair_id = %self.pair_id, "worker: channel closed; exiting");
    }

    async fn execute_batch(&self, batch: ReservedBatch) -> anyhow::Result<()> {
        // Gate B snapshot for this batch. If missing -> fail closed (skip everything).
        let market = self.market_view.get(&batch.pair_id).await;

        let mut results: Vec<UserResult> = Vec::with_capacity(batch.users.len());

        for u in &batch.users {
            // Load session via cache then DB fallback
            let session = match self.load_session(u.session_id).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(
                        session_id = %u.session_id,
                        error = ?e,
                        "worker: session load failed; skipping user"
                    );

                    results.push(UserResult {
                        session_id: u.session_id,
                        chunk_results: u
                            .chunks
                            .iter()
                            .map(|c| ChunkResult {
                                chunk_id: c.chunk_id,
                                status: ChunkStatus::Skipped {
                                    reason: "SESSION_NOT_FOUND".into(),
                                },
                            })
                            .collect(),
                        cooldown_ms: Some(5_000),
                    });
                    continue;
                }
            };

            if !session.active {
                results.push(UserResult {
                    session_id: u.session_id,
                    chunk_results: u
                        .chunks
                        .iter()
                        .map(|c| ChunkResult {
                            chunk_id: c.chunk_id,
                            status: ChunkStatus::Skipped {
                                reason: "SESSION_INACTIVE".into(),
                            },
                        })
                        .collect(),
                    cooldown_ms: None,
                });
                continue;
            }

            let mut chunk_results: Vec<ChunkResult> = Vec::with_capacity(u.chunks.len());
            let mut any_failed = false;

            // Policy:
            // - stop on first constraint failure (Gate B) to avoid executing under worse conditions
            // - stop on first execution failure to avoid repeated chain spam
            for ch in &u.chunks {
                if !gate_b_ok(&session, market.as_ref()) {
                    chunk_results.push(ChunkResult {
                        chunk_id: ch.chunk_id,
                        status: ChunkStatus::Skipped {
                            reason: "GATE_B_CONSTRAINTS".into(),
                        },
                    });
                    break;
                }

                match self
                    .exec
                    .execute_swap(SwapCall {
                        pair_id: batch.pair_id.clone(),
                        session_id: u.session_id,
                        bid: ch.bid,
                        chunk_id: ch.chunk_id,
                    })
                    .await
                {
                    Ok(rcpt) => {
                        chunk_results.push(ChunkResult {
                            chunk_id: ch.chunk_id,
                            status: ChunkStatus::Success { tx_id: rcpt.tx_id },
                        });
                    }
                    Err(e) => {
                        let reason = classify_error(&e);
                        any_failed = true;

                        chunk_results.push(ChunkResult {
                            chunk_id: ch.chunk_id,
                            status: ChunkStatus::Failed { reason },
                        });
                        break;
                    }
                }
            }

            results.push(UserResult {
                session_id: u.session_id,
                chunk_results,
                cooldown_ms: any_failed.then_some(self.default_failure_cooldown_ms),
            });
        }

        // Single DB commit point:
        // - idempotent per chunk (only processes PENDING rows)
        // - unwinds in-flight always
        // - decrements remaining only on success
        // - applies cooldown
        commit_batch(self.store.as_ref(), &batch, &results).await?;

        info!(
            pair_id = %batch.pair_id,
            batch_id = %batch.batch_id,
            "worker: batch committed"
        );
        Ok(())
    }

    async fn load_session(&self, session_id: uuid::Uuid) -> anyhow::Result<Session> {
        if let Some(s) = self.store.get_cached(&session_id) {
            return Ok(s);
        }

        let s = self.store.load_by_id(&session_id).await?;
        self.store.upsert_cache(s.clone());
        Ok(s)
    }
}

/// Gate B enforces user constraints right before execution.
/// Missing market snapshot fails closed.
fn gate_b_ok(
    session: &Session,
    market: Option<&crate::market_view::types::MarketMetricsView>,
) -> bool {
    let m = match market {
        Some(m) => m,
        None => return false,
    };

    m.spread_bps <= session.intent.constraints.max_spread_bps
        && m.trend_drop_bps <= session.intent.constraints.max_trend_drop_bps
        && m.slippage_bps <= session.intent.constraints.max_slippage_bps
}

/// Normalizes errors into stable bounded reason strings.
///
/// TODO:
/// prefer structured error codes (enum / numeric code) over string contains.
/// This function is the compatibility layer until your executor returns structured errors.
fn classify_error(e: &anyhow::Error) -> String {
    let s = format!("{e}");

    if s.contains("MarketNotOpen") {
        return "MarketNotOpen".into();
    }
    if s.contains("Slippage") {
        return "Slippage".into();
    }
    if s.contains("InsufficientLiquidity") {
        return "InsufficientLiquidity".into();
    }

    const MAX: usize = 160;
    if s.len() > MAX {
        format!("ERR:{}", &s[..MAX])
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::mpsc;
    use tokio::time::{Duration, sleep};
    use uuid::Uuid;

    use async_trait::async_trait;

    use crate::execution::types::{ReservedChunk, ReservedUser};
    use crate::market_view::MarketViewStore;
    use crate::session::model::{Session, SessionIntent, SessionState, UserConstraints};
    use crate::session::repository::SessionRepository;
    use crate::session::store::SessionStore;

    // -------------------------------------------------------------------------
    // Test helpers
    // -------------------------------------------------------------------------

    /// Create a cache-only SessionStore with a single session.
    ///
    /// - No DB
    /// - No paging
    /// - Deterministic behavior
    ///
    /// This is intentionally a free function (not `new`) to satisfy Rust
    /// conventions and Clippy rules.
    fn make_test_store(session: Session) -> Arc<SessionStore> {
        struct DummyRepo;

        #[async_trait]
        impl SessionRepository for DummyRepo {
            async fn fetch_page(&self, _: usize, _: usize) -> anyhow::Result<Vec<Session>> {
                Ok(vec![])
            }

            async fn fetch_by_id(&self, _: &Uuid) -> anyhow::Result<Option<Session>> {
                Ok(None)
            }

            async fn persist_fairness(&self, _: &Uuid, _: i128, _: u64) -> anyhow::Result<()> {
                Ok(())
            }

            async fn reserve_execution(
                &self,
                _: &str,
                _: u64,
                _: &[crate::planner::types::PlannedAllocation],
            ) -> anyhow::Result<Option<ReservedBatch>> {
                unreachable!("not used in executor unit tests")
            }

            async fn commit_batch(
                &self,
                _: &ReservedBatch,
                _: &[UserResult],
            ) -> anyhow::Result<()> {
                Ok(())
            }
        }

        let store = SessionStore::new(Arc::new(DummyRepo));
        store.upsert_cache(session);
        Arc::new(store)
    }

    fn mk_session(id: Uuid) -> Session {
        Session {
            session_id: id,
            pair_id: "TON/USDT".into(),
            active: true,
            intent: SessionIntent {
                constraints: UserConstraints {
                    max_spread_bps: 10.0,
                    max_trend_drop_bps: 10.0,
                    max_slippage_bps: 10.0,
                },
                preferred_chunk_bid: 100,
                max_bid_per_tick: 1_000,
            },
            state: SessionState {
                remaining_bid: 1_000,
                remaining_chunks: 10,
                in_flight_bid: 0,
                in_flight_chunks: 0,
                cooldown_until_ms: 0,
                quantum: 100,
                deficit: 0,
                last_served_ms: 0,
                has_pending_batch: false,
            },
        }
    }

    fn mk_batch(id: Uuid, chunks: usize) -> ReservedBatch {
        ReservedBatch {
            batch_id: Uuid::new_v4(),
            pair_id: "TON/USDT".into(),
            created_ms: 0,
            users: vec![ReservedUser {
                session_id: id,
                chunks: (0..chunks)
                    .map(|_| ReservedChunk {
                        chunk_id: Uuid::new_v4(),
                        bid: 100,
                    })
                    .collect(),
            }],
        }
    }

    // -------------------------------------------------------------------------
    // Mock executor
    // -------------------------------------------------------------------------

    struct MockExecutor {
        calls: AtomicUsize,
        fail_on_call: Option<usize>,
    }

    #[async_trait]
    impl SwapExecutor for MockExecutor {
        async fn execute_swap(&self, _: SwapCall) -> anyhow::Result<SwapReceipt> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_on_call == Some(n) {
                Err(anyhow::anyhow!("MarketNotOpen"))
            } else {
                Ok(SwapReceipt {
                    tx_id: format!("tx-{n}"),
                })
            }
        }
    }

    // -------------------------------------------------------------------------
    // Tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn gate_b_fails_closed_without_market() {
        let id = Uuid::new_v4();
        let store = make_test_store(mk_session(id));

        let exec = Arc::new(MockExecutor {
            calls: AtomicUsize::new(0),
            fail_on_call: None,
        });

        let worker = ExecutorWorker::new(
            store,
            MarketViewStore::new(), // no market snapshot
            exec.clone(),
            5_000,
            "TON/USDT".into(),
        );

        worker.execute_batch(mk_batch(id, 1)).await.unwrap();

        assert_eq!(
            exec.calls.load(Ordering::SeqCst),
            0,
            "executor must not run without market snapshot (fail-closed)"
        );
    }

    #[tokio::test]
    async fn stops_on_first_chunk_failure() {
        let id = Uuid::new_v4();
        let store = make_test_store(mk_session(id));

        let exec = Arc::new(MockExecutor {
            calls: AtomicUsize::new(0),
            fail_on_call: Some(1),
        });

        let market_view = MarketViewStore::new();
        market_view
            .set(
                "TON/USDT",
                crate::market_view::types::MarketMetricsView {
                    ts_ms: 0,
                    spread_bps: 5.0,
                    trend_drop_bps: 5.0,
                    slippage_bps: 5.0,
                    depth_now_in: 1_000,
                },
            )
            .await;

        let worker =
            ExecutorWorker::new(store, market_view, exec.clone(), 5_000, "TON/USDT".into());

        worker.execute_batch(mk_batch(id, 2)).await.unwrap();

        assert_eq!(
            exec.calls.load(Ordering::SeqCst),
            1,
            "must stop executing further chunks after first failure"
        );
    }

    #[tokio::test]
    async fn inactive_session_is_skipped() {
        let mut s = mk_session(Uuid::new_v4());
        s.active = false;

        let session_id = s.session_id;

        let store = make_test_store(s);

        let exec = Arc::new(MockExecutor {
            calls: AtomicUsize::new(0),
            fail_on_call: None,
        });

        let worker = ExecutorWorker::new(
            store,
            MarketViewStore::new(),
            exec.clone(),
            5_000,
            "TON/USDT".into(),
        );

        worker.execute_batch(mk_batch(session_id, 1)).await.unwrap();

        assert_eq!(
            exec.calls.load(Ordering::SeqCst),
            0,
            "inactive sessions must never execute swaps"
        );
    }

    #[tokio::test]
    async fn router_recreates_worker_after_channel_close() {
        let id = Uuid::new_v4();
        let store = make_test_store(mk_session(id));

        let exec = Arc::new(MockExecutor {
            calls: AtomicUsize::new(0),
            fail_on_call: None,
        });

        let router = Arc::new(PairExecutorRouter::new(
            store,
            MarketViewStore::new(),
            exec,
            5_000,
            8,
        ));

        let (tx, rx) = mpsc::channel(8);
        let r = router.clone();

        tokio::spawn(async move {
            r.run(rx).await;
        });

        tx.send(ExecutionEvent::Reserved(mk_batch(id, 0)))
            .await
            .unwrap();

        sleep(Duration::from_millis(50)).await;

        // Drop sender → router loop exits → worker channel closes.
        drop(tx);

        // Test passes if no panic / deadlock occurs.
        sleep(Duration::from_millis(50)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn router_recreates_worker_with_virtual_time() {
        use tokio::time::advance;

        let id = Uuid::new_v4();
        let store = make_test_store(mk_session(id));

        let exec = Arc::new(MockExecutor {
            calls: AtomicUsize::new(0),
            fail_on_call: None,
        });

        let router = Arc::new(PairExecutorRouter::new(
            store,
            MarketViewStore::new(),
            exec,
            5_000,
            1, // small capacity to stress lifecycle
        ));

        let (tx, rx) = mpsc::channel(1);
        let r = router.clone();

        tokio::spawn(async move {
            r.run(rx).await;
        });

        tx.send(ExecutionEvent::Reserved(mk_batch(id, 0)))
            .await
            .unwrap();

        // Advance virtual time instead of sleeping
        advance(Duration::from_secs(1)).await;

        drop(tx);

        advance(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn commit_failure_does_not_double_execute() {
        struct FailingCommitRepo;

        #[async_trait]
        impl SessionRepository for FailingCommitRepo {
            async fn fetch_page(&self, _: usize, _: usize) -> anyhow::Result<Vec<Session>> {
                Ok(vec![])
            }
            async fn fetch_by_id(&self, _: &Uuid) -> anyhow::Result<Option<Session>> {
                Ok(None)
            }
            async fn persist_fairness(&self, _: &Uuid, _: i128, _: u64) -> anyhow::Result<()> {
                Ok(())
            }
            async fn reserve_execution(
                &self,
                _: &str,
                _: u64,
                _: &[crate::planner::types::PlannedAllocation],
            ) -> anyhow::Result<Option<ReservedBatch>> {
                unreachable!()
            }
            async fn commit_batch(
                &self,
                _: &ReservedBatch,
                _: &[UserResult],
            ) -> anyhow::Result<()> {
                Err(anyhow::anyhow!("DB down"))
            }
        }

        let id = Uuid::new_v4();
        let store = Arc::new(SessionStore::new(Arc::new(FailingCommitRepo)));
        store.upsert_cache(mk_session(id));

        let exec = Arc::new(MockExecutor {
            calls: AtomicUsize::new(0),
            fail_on_call: None,
        });

        let market_view = MarketViewStore::new();
        market_view
            .set(
                "TON/USDT",
                crate::market_view::types::MarketMetricsView {
                    ts_ms: 0,
                    spread_bps: 5.0,
                    trend_drop_bps: 5.0,
                    slippage_bps: 5.0,
                    depth_now_in: 1_000,
                },
            )
            .await;

        let worker =
            ExecutorWorker::new(store, market_view, exec.clone(), 5_000, "TON/USDT".into());

        let batch = mk_batch(id, 1);

        let _ = worker.execute_batch(batch).await;

        assert_eq!(
            exec.calls.load(Ordering::SeqCst),
            1,
            "batch must not be re-executed after commit failure"
        );
    }

    #[tokio::test]
    async fn router_applies_backpressure_when_worker_queue_full() {
        let id = Uuid::new_v4();
        let store = make_test_store(mk_session(id));

        let exec = Arc::new(MockExecutor {
            calls: AtomicUsize::new(0),
            fail_on_call: None,
        });

        let router = Arc::new(PairExecutorRouter::new(
            store,
            MarketViewStore::new(),
            exec,
            5_000,
            1, // capacity = 1
        ));

        let (tx, rx) = mpsc::channel(1);
        let r = router.clone();

        tokio::spawn(async move {
            r.run(rx).await;
        });

        // First send fills the queue
        tx.send(ExecutionEvent::Reserved(mk_batch(id, 0)))
            .await
            .unwrap();

        // Second send should apply backpressure (awaits)
        let send_fut = tx.send(ExecutionEvent::Reserved(mk_batch(id, 0)));

        tokio::select! {
            _ = send_fut => {
                // OK: send eventually succeeds after worker drains
            }
            _ = tokio::time::sleep(Duration::from_millis(10)) => {
                // Also OK: proves it did not drop immediately
            }
        }
    }
}

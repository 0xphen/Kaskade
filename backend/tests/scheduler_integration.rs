use sqlx::AnyPool;
use sqlx::any::AnyPoolOptions;
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

use backend::{
    execution::types::{ChunkResult, ChunkStatus, ExecutionEvent, ReservedBatch, UserResult},
    market::types::MarketMetricsView,
    metrics::counters::Counters,
    scheduler::scheduler::Scheduler,
    session::{
        repository::SessionRepository, repository_sqlx::SqlxSessionRepository, store::SessionStore,
    },
    time::now_ms,
};

const PAIR: &str = "TON/USDT";

// -----------------------
// DB + helpers
// -----------------------

/// Isolated in-memory DB per test.
/// Unique name prevents test interference during parallel execution.
/// `cache=shared` allows multiple connections within the same pool to see the same in-memory DB.
async fn setup_db() -> AnyPool {
    sqlx::any::install_default_drivers();

    let db_name = Uuid::new_v4().to_string();
    let conn = format!("sqlite:file:{}?mode=memory&cache=shared", db_name);

    let pool = AnyPoolOptions::new()
        .max_connections(5)
        .connect(&conn)
        .await
        .expect("connect sqlite memory db");

    // --- schema ---
    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS sessions (
  session_id TEXT PRIMARY KEY,
  pair_id TEXT NOT NULL,
  active BOOLEAN NOT NULL,
  max_spread_bps REAL NOT NULL,
  max_trend_drop_bps REAL NOT NULL,
  max_slippage_bps REAL NOT NULL,
  preferred_chunk_bid BIGINT NOT NULL,
  max_bid_per_tick BIGINT NOT NULL,
  remaining_bid BIGINT NOT NULL,
  remaining_chunks BIGINT NOT NULL,
  in_flight_bid BIGINT NOT NULL,
  in_flight_chunks BIGINT NOT NULL,
  cooldown_until_ms BIGINT NOT NULL,
  quantum BIGINT NOT NULL,
  deficit BIGINT NOT NULL,
  last_served_ms BIGINT NOT NULL,
  has_pending_batch INTEGER NOT NULL DEFAULT 0
);
"#,
    )
    .execute(&pool)
    .await
    .expect("create sessions");

    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS batches (
  batch_id TEXT PRIMARY KEY,
  pair_id TEXT NOT NULL,
  created_ms BIGINT NOT NULL,
  status TEXT NOT NULL,
  reason TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS batch_items (
  chunk_id TEXT PRIMARY KEY,
  batch_id TEXT NOT NULL,
  session_id TEXT NOT NULL,
  bid BIGINT NOT NULL,
  status TEXT NOT NULL,
  tx_id TEXT NOT NULL,
  error TEXT NOT NULL
);
"#,
    )
    .execute(&pool)
    .await
    .expect("create batches tables");

    pool
}

fn good_market() -> MarketMetricsView {
    MarketMetricsView {
        ts_ms: now_ms(),
        spread_bps: 10.0,
        trend_drop_bps: 5.0,
        max_depth: 1_000_000_000,
    }
}

async fn insert_active_session(pool: &AnyPool, id: Uuid, quantum: i64, deficit: i64) {
    // Chosen to be clearly eligible for Gate A and availability checks.
    // preferred_chunk_bid=100_000, max_bid_per_tick=500_000, remaining_bid=1_000_000, remaining_chunks=10
    sqlx::query(
        r#"
INSERT INTO sessions VALUES
(?, ?, 1, 100, 100, 100,
 100000, 500000,
 1000000, 10,
 0, 0,
 0,
 ?, ?, 0, 0)
"#,
    )
    .bind(id.to_string())
    .bind(PAIR)
    .bind(quantum)
    .bind(deficit)
    .execute(pool)
    .await
    .expect("insert active session");
}

async fn insert_inactive_session(pool: &AnyPool, id: Uuid) {
    sqlx::query(
        r#"
INSERT INTO sessions VALUES
(?, ?, 0, 100, 100, 100,
 100000, 500000,
 1000000, 10,
 0, 0,
 0,
 100000, 0, 0, 0)
"#,
    )
    .bind(id.to_string())
    .bind(PAIR)
    .execute(pool)
    .await
    .expect("insert inactive session");
}

async fn setup_scheduler() -> (
    Arc<AnyPool>,
    Arc<dyn SessionRepository>,
    Arc<SessionStore>,
    Scheduler,
) {
    let pool = Arc::new(setup_db().await);

    let repo: Arc<dyn SessionRepository> = Arc::new(SqlxSessionRepository::new(pool.clone()));
    let store = Arc::new(SessionStore::new(Arc::clone(&repo)));

    let sched = Scheduler::new(
        store.clone(),
        10,    // candidate_min
        1_000, // max_attempts
        16,    // max_users_per_batch
        Counters::default(),
    );

    (pool, repo, store, sched)
}

/// Commit all chunks as success (simulates executor finishing the work).
async fn commit_all_success(repo: &dyn SessionRepository, batch: &ReservedBatch) {
    let results: Vec<UserResult> = batch
        .users
        .iter()
        .map(|u| UserResult {
            session_id: u.session_id,
            cooldown_ms: None,
            chunk_results: u
                .chunks
                .iter()
                .map(|c| ChunkResult {
                    chunk_id: c.chunk_id,
                    status: ChunkStatus::Success {
                        tx_id: "tx".to_string(),
                    },
                })
                .collect(),
        })
        .collect();

    repo.commit_batch(batch, &results)
        .await
        .expect("commit_batch success");
}

// -----------------------
// INTEGRATION TESTS
// -----------------------

#[tokio::test]
async fn schedules_only_eligible_sessions_single_tick() {
    let (pool, _repo, store, sched) = setup_scheduler().await;

    let good = Uuid::new_v4();
    let bad = Uuid::new_v4();

    insert_active_session(&pool, good, 100_000, 0).await;
    insert_inactive_session(&pool, bad).await;

    store
        .ensure_candidates(10)
        .await
        .expect("ensure candidates");

    let (tx, mut rx) = mpsc::channel(8);

    // IMPORTANT: single tick. No executor running here.
    sched
        .on_tick(PAIR, good_market(), tx, now_ms())
        .await
        .expect("on_tick");

    let ev = rx.recv().await.expect("expected reserved event");
    let ExecutionEvent::Reserved(batch) = ev;

    assert_eq!(batch.pair_id, PAIR);
    assert_eq!(batch.users.len(), 1);
    assert_eq!(batch.users[0].session_id, good);
}

#[tokio::test]
async fn drr_prevents_starvation_when_batches_complete() {
    let (pool, repo, store, sched) = setup_scheduler().await;

    let slow = Uuid::new_v4();
    let fast = Uuid::new_v4();

    // Both want 100_000 per selection (preferred_chunk_bid).
    // Slow accumulates slower, but must eventually be served assuming revisits + batches complete.
    insert_active_session(&pool, slow, 20_000, 0).await; // needs multiple rounds to reach want
    insert_active_session(&pool, fast, 200_000, 0).await; // served quickly

    store.ensure_candidates(2).await.expect("ensure candidates");

    let (tx, mut rx) = mpsc::channel(64);

    let mut seen_slow = false;
    let mut seen_fast = false;

    // Multiple ticks WITH commit to clear in_flight.
    for i in 0..60u64 {
        sched
            .on_tick(PAIR, good_market(), tx.clone(), now_ms() + i)
            .await
            .expect("on_tick");

        // If a batch was produced, commit it as success (simulate executor).
        if let Ok(Some(ExecutionEvent::Reserved(batch))) =
            tokio::time::timeout(std::time::Duration::from_millis(10), rx.recv()).await
        {
            for u in &batch.users {
                if u.session_id == slow {
                    seen_slow = true;
                }
                if u.session_id == fast {
                    seen_fast = true;
                }
            }

            commit_all_success(repo.as_ref(), &batch).await;

            if seen_slow && seen_fast {
                break;
            }
        }
    }

    assert!(seen_fast, "fast user should be served");
    assert!(
        seen_slow,
        "slow user must eventually be served (no starvation)"
    );
}

#[tokio::test]
async fn enqueue_failure_does_not_corrupt_state_single_tick() {
    let (pool, _repo, store, sched) = setup_scheduler().await;

    let id = Uuid::new_v4();
    insert_active_session(&pool, id, 100_000, 0).await;

    store.ensure_candidates(1).await.expect("ensure candidates");

    // Channel with dropped receiver -> enqueue fails, but reservation must still be persisted as RESERVED.
    let (tx, rx) = mpsc::channel(1);
    drop(rx);

    sched
        .on_tick(PAIR, good_market(), tx, now_ms())
        .await
        .expect("on_tick");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM batches WHERE status = 'RESERVED'")
        .fetch_one(&*pool)
        .await
        .expect("count reserved batches");

    assert_eq!(count, 1, "batch must remain RESERVED for recovery");
}

// #[tokio::test]
// async fn fairness_cached_after_on_tick_but_not_persisted() {
//     let (pool, repo, store, sched) = setup_scheduler().await;

//     let id = Uuid::new_v4();

//     insert_active_session(&pool, id, 50_000, 0).await;
//     store.ensure_candidates(1).await.expect("ensure candidates");

//     let (tx, mut rx) = mpsc::channel(4);

//     // Run one scheduling tick
//     sched
//         .on_tick(PAIR, good_market(), tx, now_ms())
//         .await
//         .expect("on_tick");

//     let ExecutionEvent::Reserved(batch) = rx.recv().await.expect("expected reserved batch");

//     // 🔹 DB fairness must NOT change yet
//     let row = sqlx::query("SELECT deficit FROM sessions WHERE session_id = ?")
//         .bind(id.to_string())
//         .fetch_one(&pool)
//         .await
//         .unwrap();

//     let db_deficit: i64 = row.get("deficit");
//     assert_eq!(
//         db_deficit, 0,
//         "DB fairness must not change before execution"
//     );

//     // 🔹 Cache fairness MUST reflect DRR
//     let cached = store.get_cached(&id).expect("cached session");
//     assert_eq!(
//         cached.state.deficit, -50_000,
//         "cache deficit must reflect DRR decision"
//     );

//     // Now simulate execution
//     commit_all_success(repo.as_ref(), &batch).await;

//     // 🔹 DB fairness must now be persisted
//     let row = sqlx::query("SELECT deficit FROM sessions WHERE session_id = ?")
//         .bind(id.to_string())
//         .fetch_one(&pool)
//         .await
//         .unwrap();

//     let db_deficit: i64 = row.get("deficit");
//     assert_eq!(
//         db_deficit, -50_000,
//         "DB deficit must be persisted after commit_batch"
//     );
// }

#[tokio::test]
async fn skips_sessions_with_pending_batch() {
    let (pool, _repo, store, sched) = setup_scheduler().await;

    let id = Uuid::new_v4();

    insert_active_session(&pool, id, 100_000, 100_000).await;

    // Simulate an already-reserved batch (lockout)
    sqlx::query("UPDATE sessions SET has_pending_batch = 1 WHERE session_id = ?")
        .bind(id.to_string())
        .execute(&*pool)
        .await
        .unwrap();

    // Reload into cache
    store.ensure_candidates(1).await.unwrap();

    let (tx, mut rx) = mpsc::channel(8);

    sched
        .on_tick(PAIR, good_market(), tx, now_ms())
        .await
        .expect("on_tick");

    // Must not schedule anything
    assert!(
        rx.try_recv().is_err(),
        "session with pending batch must be skipped"
    );
}

#[tokio::test]
async fn recognizes_reserved_batches_as_in_flight_bid() {
    let (pool, _repo, store, sched) = setup_scheduler().await;

    let id = Uuid::new_v4();

    // 1,000,000 total remaining_bid
    insert_active_session(&pool, id, 100_000, 100_000).await;

    // Simulate crash recovery state:
    // 900,000 already reserved, batch not committed
    sqlx::query(
        r#"
        UPDATE sessions
        SET in_flight_bid = 900000,
            has_pending_batch = 1
        WHERE session_id = ?
        "#,
    )
    .bind(id.to_string())
    .execute(&*pool)
    .await
    .unwrap();

    store
        .ensure_candidates(1)
        .await
        .expect("load state from DB");

    let (tx, mut rx) = mpsc::channel(8);

    sched
        .on_tick(PAIR, good_market(), tx, now_ms())
        .await
        .expect("on_tick");

    // Must not schedule — funds already reserved
    assert!(
        rx.try_recv().is_err(),
        "recovery state must respect in-flight bid and pending batch"
    );
}

#[tokio::test]
async fn gate_a_strict_boundary_checks() {
    let (pool, _repo, store, sched) = setup_scheduler().await;

    let id = Uuid::new_v4();

    insert_active_session(&pool, id, 100_000, 100_000).await;

    // Explicitly set max_spread_bps = 10.0
    sqlx::query("UPDATE sessions SET max_spread_bps = 10.0 WHERE session_id = ?")
        .bind(id.to_string())
        .execute(&*pool)
        .await
        .unwrap();

    store.ensure_candidates(1).await.unwrap();

    let (tx, mut rx) = mpsc::channel(8);

    let mut market_ok = good_market();
    market_ok.spread_bps = 10.0;

    sched
        .on_tick(PAIR, market_ok, tx.clone(), now_ms())
        .await
        .unwrap();

    assert!(rx.try_recv().is_ok(), "exact boundary must pass Gate A");

    let mut market_bad = good_market();
    market_bad.spread_bps = 10.1;

    sched.on_tick(PAIR, market_bad, tx, now_ms()).await.unwrap();

    assert!(
        rx.try_recv().is_err(),
        "market exceeding boundary must be rejected"
    );
}

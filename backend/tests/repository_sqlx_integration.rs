use sqlx::any::AnyPoolOptions;
use sqlx::{AnyPool, Row};
use std::sync::Arc;
use tokio::task::JoinSet;
use uuid::Uuid;

use backend::execution::types::{ChunkResult, ChunkStatus, UserResult};
use backend::planner::types::PlannedAllocation;
use backend::session::repository::SessionRepository;
use backend::session::repository_sqlx::SqlxSessionRepository;

/// Helper to setup an isolated, unique in-memory SQLite database.
/// Using a unique name in the connection string prevents "Table already exists"
/// errors during parallel test execution while still allowing shared cache access.
async fn setup_db() -> AnyPool {
    sqlx::any::install_default_drivers();

    let db_name = Uuid::new_v4().to_string();
    let conn_str = format!("sqlite:file:{}?mode=memory&cache=shared", db_name);

    let pool = AnyPoolOptions::new()
        .max_connections(5)
        .connect(&conn_str)
        .await
        .unwrap();

    // Use IF NOT EXISTS as a secondary safety measure
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
  has_pending_batch BOOLEAN NOT NULL DEFAULT 0
);
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

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
    .unwrap();

    pool
}

#[tokio::test]
async fn fetch_by_id_round_trip() {
    let pool = setup_db().await;
    let repo = SqlxSessionRepository::new(pool.clone());

    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO sessions VALUES (?, 'TON/USDT', 1, 50, 100, 75, 100000, 1000000, 1000, 10, 0, 0, 0, 100000, 42, 0, 0)"#,
    )
    .bind(id.to_string())
    .execute(&pool)
    .await
    .unwrap();

    let s = repo.fetch_by_id(&id).await.unwrap().unwrap();
    assert_eq!(s.session_id, id);
    assert_eq!(s.state.deficit, 42);
}

#[tokio::test]
async fn persist_fairness_updates_row() {
    let pool = setup_db().await;
    let repo = SqlxSessionRepository::new(pool.clone());

    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO sessions VALUES (?, 'TON/USDT', 1, 50, 100, 75, 100000, 1000000, 1000, 10, 0, 0, 0, 100000, 0, 0, 0)"#,
    )
    .bind(id.to_string())
    .execute(&pool)
    .await
    .unwrap();

    repo.persist_fairness(&id, 999, 123).await.unwrap();

    let row = sqlx::query("SELECT deficit, last_served_ms FROM sessions WHERE session_id = ?")
        .bind(id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(row.get::<i64, _>("deficit"), 999);
    assert_eq!(row.get::<i64, _>("last_served_ms"), 123);
}

#[tokio::test]
async fn poison_rows_are_skipped() {
    let pool = setup_db().await;
    let repo = SqlxSessionRepository::new(pool.clone());

    // Insert invalid UUID string
    sqlx::query(
        r#"INSERT INTO sessions VALUES ('bad-uuid', 'TON/USDT', 1, 50, 100, 75, 100000, 1000000, 1000, 10, 0, 0, 0, 100000, 0, 0, 0)"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    let good_id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO sessions VALUES (?, 'TON/USDT', 1, 50, 100, 75, 100000, 1000000, 1000, 10, 0, 0, 0, 100000, 0, 0, 0)"#,
    )
    .bind(good_id.to_string())
    .execute(&pool)
    .await
    .unwrap();

    // fetch_page should continue and return valid rows even if one row parsing fails
    let page = repo.fetch_page(10, 0).await.unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].session_id, good_id);
}

#[tokio::test]
async fn persist_fairness_numeric_overflow_error() {
    let pool = setup_db().await;
    let repo = SqlxSessionRepository::new(pool.clone());
    let id = Uuid::new_v4();

    sqlx::query(
        r#"INSERT INTO sessions VALUES (?, 'TON/USDT', 1, 50, 100, 75, 100000, 1000000, 1000, 10, 0, 0, 0, 100000, 0, 0, 0)"#,
    )
    .bind(id.to_string())
    .execute(&pool)
    .await
    .unwrap();

    // Attempting to persist i128::MAX into BIGINT (i64) column.
    // The repository should catch this and return a Result::Err instead of crashing.
    let result = repo.persist_fairness(&id, i128::MAX, 0).await;
    assert!(
        result.is_err(),
        "Repository must return Err on i128 -> i64 overflow"
    );
}

#[tokio::test]
async fn test_concurrent_fairness_updates() {
    let pool = setup_db().await;
    let repo = Arc::new(SqlxSessionRepository::new(pool.clone()));
    let id = Uuid::new_v4();

    sqlx::query(
        r#"INSERT INTO sessions VALUES (?, 'TON/USDT', 1, 50, 100, 75, 100000, 1000000, 1000, 10, 0, 0, 0, 100000, 0, 0, 0)"#,
    )
    .bind(id.to_string())
    .execute(&pool)
    .await
    .unwrap();

    let mut set = JoinSet::new();

    // Simulate 20 concurrent threads trying to update the same session fairness state
    for i in 0..20 {
        let r = Arc::clone(&repo);
        let sid = id;
        set.spawn(async move { r.persist_fairness(&sid, i as i128, i as u64).await });
    }

    while let Some(res) = set.join_next().await {
        res.expect("Task panicked")
            .expect("Concurrent update failed");
    }

    // Verify row still exists and contains one of the valid states
    let row = sqlx::query("SELECT deficit FROM sessions WHERE session_id = ?")
        .bind(id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();

    let deficit = row.get::<i64, _>("deficit");
    assert!((0..20).contains(&deficit));
}

#[tokio::test]
async fn fetch_page_empty_at_offset() {
    let pool = setup_db().await;
    let repo = SqlxSessionRepository::new(pool.clone());

    // Requesting a page when the DB is empty
    let page = repo.fetch_page(10, 0).await.unwrap();
    assert!(page.is_empty(), "Page should be empty for empty table");

    // Seed 2 rows
    for _ in 0..2 {
        sqlx::query(r#"INSERT INTO sessions VALUES (?, 'TON/USDT', 1, 50, 100, 75, 100000, 1000000, 1000, 10, 0, 0, 0, 100000, 0, 0, 0)"#)
            .bind(Uuid::new_v4().to_string())
            .execute(&pool).await.unwrap();
    }

    // Requesting offset exactly at total count
    let page = repo.fetch_page(10, 2).await.unwrap();
    assert!(page.is_empty());
}

#[tokio::test]
async fn reserve_execution_happy_path() {
    let pool = setup_db().await;
    let repo = SqlxSessionRepository::new(pool.clone());

    let session_id = Uuid::new_v4();

    sqlx::query(
        r#"INSERT INTO sessions VALUES
        (?, 'TON/USDT', 1, 50, 100, 75,
         100, 1000,
         1000, 10,
         0, 0,
         0, 100,
         0, 0, 0)"#,
    )
    .bind(session_id.to_string())
    .execute(&pool)
    .await
    .unwrap();

    let alloc = PlannedAllocation {
        session_id,
        total_bid: 600,
        chunks: vec![100, 200, 300],
    };

    let batch = repo
        .reserve_execution("TON/USDT", 12345, std::slice::from_ref(&alloc))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(batch.users.len(), 1);
    assert_eq!(batch.users[0].chunks.len(), 3);

    // Verify in-flight updated
    let row =
        sqlx::query("SELECT in_flight_bid, in_flight_chunks FROM sessions WHERE session_id = ?")
            .bind(session_id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(row.get::<i64, _>("in_flight_bid"), 600);
    assert_eq!(row.get::<i64, _>("in_flight_chunks"), 3);
}

#[tokio::test]
async fn reserve_execution_fails_on_insufficient_bid() {
    let pool = setup_db().await;
    let repo = SqlxSessionRepository::new(pool.clone());
    let session_id = Uuid::new_v4();

    sqlx::query(
        r#"INSERT INTO sessions VALUES
        (?, 'TON/USDT', 1, 50, 100, 75,
         100, 1000,
         200, 10,
         0, 0,
         0, 100,
         0, 0, 0)"#,
    )
    .bind(session_id.to_string())
    .execute(&pool)
    .await
    .unwrap();

    let alloc = PlannedAllocation {
        session_id,
        total_bid: 250,
        chunks: vec![150, 100], // total = 250 > remaining_bid
    };

    let res = repo
        .reserve_execution("TON/USDT", 0, &[alloc])
        .await
        .unwrap();
    assert!(res.is_none(), "reservation must fail");

    // Ensure no partial state
    let row = sqlx::query("SELECT in_flight_bid FROM sessions WHERE session_id = ?")
        .bind(session_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(row.get::<i64, _>("in_flight_bid"), 0);
}

#[tokio::test]
async fn reserve_execution_rolls_back_on_failure() {
    let pool = setup_db().await;
    let repo = SqlxSessionRepository::new(pool.clone());

    let session_id = Uuid::new_v4();

    sqlx::query(
        r#"INSERT INTO sessions VALUES
        (?, 'TON/USDT', 1, 50, 100, 75,
         100, 1000,
         100, 1,
         0, 0,
         0, 100,
         0, 0, 0)"#,
    )
    .bind(session_id.to_string())
    .execute(&pool)
    .await
    .unwrap();

    let alloc = PlannedAllocation {
        session_id,
        total_bid: 200,
        chunks: vec![100, 100], // chunks > remaining_chunks
    };

    let res = repo
        .reserve_execution("TON/USDT", 0, &[alloc])
        .await
        .unwrap();
    assert!(res.is_none());

    let batch_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM batches")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(batch_count, 0, "no batch must be committed");
}

#[tokio::test]
async fn reserve_execution_concurrent_safety() {
    let pool = setup_db().await;
    let repo = Arc::new(SqlxSessionRepository::new(pool.clone()));
    let session_id = Uuid::new_v4();

    sqlx::query(
        r#"INSERT INTO sessions VALUES
        (?, 'TON/USDT', 1, 50, 100, 75,
         100, 1000,
         300, 3,
         0, 0,
         0, 100,
         0, 0, 0)"#,
    )
    .bind(session_id.to_string())
    .execute(&pool)
    .await
    .unwrap();

    let alloc = PlannedAllocation {
        session_id,
        total_bid: 100,
        chunks: vec![100],
    };

    let mut set = JoinSet::new();

    for _ in 0..5 {
        let r = Arc::clone(&repo);
        let a = alloc.clone();
        set.spawn(async move { r.reserve_execution("TON/USDT", 0, &[a]).await });
    }

    let mut success = 0;
    while let Some(res) = set.join_next().await {
        let out = res.unwrap().unwrap();
        if out.is_some() {
            success += 1;
        }
    }

    // Only 3 chunks available → max 3 successes
    assert!(success <= 3);
}

#[tokio::test]
async fn reserve_execution_pair_mismatch_fails() {
    let pool = setup_db().await;
    let repo = SqlxSessionRepository::new(pool.clone());
    let session_id = Uuid::new_v4();

    sqlx::query(
        r#"INSERT INTO sessions VALUES
        (?, 'ETH/USDT', 1, 50, 100, 75,
         100, 1000,
         1000, 10,
         0, 0,
         0, 100,
         0, 0, 0)"#,
    )
    .bind(session_id.to_string())
    .execute(&pool)
    .await
    .unwrap();

    let alloc = PlannedAllocation {
        session_id,
        total_bid: 100,
        chunks: vec![100],
    };

    let res = repo
        .reserve_execution("TON/USDT", 0, &[alloc])
        .await
        .unwrap();
    assert!(res.is_none());
}

#[tokio::test]
async fn commit_batch_all_success() {
    let pool = setup_db().await;
    let repo = SqlxSessionRepository::new(pool.clone());

    let session_id = Uuid::new_v4();

    sqlx::query(
        r#"INSERT INTO sessions VALUES
        (?, 'TON/USDT', 1, 50, 100, 75,
         100, 1000,
         1000, 10,
         0, 0,
         0, 100,
         0, 0, 0)"#,
    )
    .bind(session_id.to_string())
    .execute(&pool)
    .await
    .unwrap();

    let alloc = PlannedAllocation {
        session_id,
        total_bid: 300,
        chunks: vec![100, 200],
    };

    let batch = repo
        .reserve_execution("TON/USDT", 0, &[alloc])
        .await
        .unwrap()
        .unwrap();

    let results = vec![UserResult {
        session_id,
        cooldown_ms: None,
        chunk_results: batch.users[0]
            .chunks
            .iter()
            .map(|c| ChunkResult {
                chunk_id: c.chunk_id,
                status: ChunkStatus::Success { tx_id: "tx".into() },
            })
            .collect(),
    }];

    repo.commit_batch(&batch, &results).await.unwrap();

    let row = sqlx::query("SELECT remaining_bid, in_flight_bid FROM sessions WHERE session_id = ?")
        .bind(session_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(row.get::<i64, _>("remaining_bid"), 700);
    assert_eq!(row.get::<i64, _>("in_flight_bid"), 0);

    let status: String = sqlx::query_scalar("SELECT status FROM batches WHERE batch_id = ?")
        .bind(batch.batch_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(status, "COMMITTED");
}

#[tokio::test]
async fn commit_batch_partial_failure() {
    let pool = setup_db().await;
    let repo = SqlxSessionRepository::new(pool.clone());
    let session_id = Uuid::new_v4();

    sqlx::query(
        r#"INSERT INTO sessions VALUES
        (?, 'TON/USDT', 1, 50, 100, 75,
         100, 1000,
         1000, 10,
         0, 0,
         0, 100,
         0, 0, 0)"#,
    )
    .bind(session_id.to_string())
    .execute(&pool)
    .await
    .unwrap();

    let alloc = PlannedAllocation {
        session_id,
        total_bid: 300,
        chunks: vec![100, 200],
    };

    let batch = repo
        .reserve_execution("TON/USDT", 0, &[alloc])
        .await
        .unwrap()
        .unwrap();

    let chunks = &batch.users[0].chunks;

    let results = vec![UserResult {
        session_id,
        cooldown_ms: Some(5000),
        chunk_results: vec![
            ChunkResult {
                chunk_id: chunks[0].chunk_id,
                status: ChunkStatus::Success {
                    tx_id: "tx1".into(),
                },
            },
            ChunkResult {
                chunk_id: chunks[1].chunk_id,
                status: ChunkStatus::Failed {
                    reason: "MarketClosed".into(),
                },
            },
        ],
    }];

    repo.commit_batch(&batch, &results).await.unwrap();

    let row = sqlx::query("SELECT remaining_bid, in_flight_bid FROM sessions WHERE session_id = ?")
        .bind(session_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();

    // only first chunk decremented
    assert_eq!(row.get::<i64, _>("remaining_bid"), 900);
    assert_eq!(row.get::<i64, _>("in_flight_bid"), 0);
}

#[tokio::test]
async fn commit_batch_all_failed() {
    let pool = setup_db().await;
    let repo = SqlxSessionRepository::new(pool.clone());
    let session_id = Uuid::new_v4();

    sqlx::query(
        r#"INSERT INTO sessions VALUES
        (?, 'TON/USDT', 1, 50, 100, 75,
         100, 1000,
         500, 10,
         0, 0,
         0, 100,
         0, 0, 0)"#,
    )
    .bind(session_id.to_string())
    .execute(&pool)
    .await
    .unwrap();

    let alloc = PlannedAllocation {
        session_id,
        total_bid: 300,
        chunks: vec![100, 200],
    };

    let batch = repo
        .reserve_execution("TON/USDT", 0, &[alloc])
        .await
        .unwrap()
        .unwrap();

    let results = vec![UserResult {
        session_id,
        cooldown_ms: Some(10_000),
        chunk_results: batch.users[0]
            .chunks
            .iter()
            .map(|c| ChunkResult {
                chunk_id: c.chunk_id,
                status: ChunkStatus::Failed {
                    reason: "fail".into(),
                },
            })
            .collect(),
    }];

    repo.commit_batch(&batch, &results).await.unwrap();

    let row = sqlx::query("SELECT remaining_bid, in_flight_bid FROM sessions WHERE session_id = ?")
        .bind(session_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(row.get::<i64, _>("remaining_bid"), 500);
    assert_eq!(row.get::<i64, _>("in_flight_bid"), 0);
}

#[tokio::test]
async fn commit_batch_is_idempotent() {
    let pool = setup_db().await;
    let repo = SqlxSessionRepository::new(pool.clone());
    let session_id = Uuid::new_v4();

    sqlx::query(
        r#"INSERT INTO sessions VALUES
        (?, 'TON/USDT', 1, 50, 100, 75,
         100, 1000,
         500, 10,
         0, 0,
         0, 100,
         0, 0, 0)"#,
    )
    .bind(session_id.to_string())
    .execute(&pool)
    .await
    .unwrap();

    let alloc = PlannedAllocation {
        session_id,
        total_bid: 200,
        chunks: vec![100, 100],
    };

    let batch = repo
        .reserve_execution("TON/USDT", 0, &[alloc])
        .await
        .unwrap()
        .unwrap();

    let results = vec![UserResult {
        session_id,
        cooldown_ms: None,
        chunk_results: batch.users[0]
            .chunks
            .iter()
            .map(|c| ChunkResult {
                chunk_id: c.chunk_id,
                status: ChunkStatus::Success { tx_id: "tx".into() },
            })
            .collect(),
    }];

    // First commit — does real work
    repo.commit_batch(&batch, &results).await.unwrap();

    // Second commit — MUST be a no-op
    repo.commit_batch(&batch, &results).await.unwrap();

    let row = sqlx::query("SELECT remaining_bid FROM sessions WHERE session_id = ?")
        .bind(session_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();

    // 500 - 200 = 300 (should not double-apply)
    assert_eq!(row.get::<i64, _>("remaining_bid"), 300);
}

#[tokio::test]
async fn test_extreme_timestamp_persistence() {
    let pool = setup_db().await;
    let repo = SqlxSessionRepository::new(pool.clone());
    let id = Uuid::new_v4();

    // Setup session
    sqlx::query(r#"INSERT INTO sessions VALUES (?, 'TON/USDT', 1, 50, 100, 75, 100, 1000, 1000, 10, 0, 0, 0, 100, 0, 0, 0)"#)
            .bind(id.to_string()).execute(&pool).await.unwrap();

    // Use a very large u64 timestamp (e.g., year 2262 approx)
    let future_ms: u64 = i64::MAX as u64;
    repo.persist_fairness(&id, 500, future_ms).await.unwrap();

    let row = sqlx::query("SELECT last_served_ms FROM sessions WHERE session_id = ?")
        .bind(id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(row.get::<i64, _>("last_served_ms"), i64::MAX);
}

/// Ensures that failed chunks correctly release 'in_flight' bid/chunks
/// back to the session so they can be re-scheduled later.
#[tokio::test]
async fn test_commit_batch_unwinds_in_flight_on_failure() {
    let pool = setup_db().await;
    let repo = SqlxSessionRepository::new(pool.clone());
    let session_id = Uuid::new_v4();

    sqlx::query(r#"INSERT INTO sessions VALUES (?, 'TON/USDT', 1, 50, 100, 75, 100, 1000, 1000, 10, 0, 0, 0, 100, 0, 0, 0)"#)
            .bind(session_id.to_string()).execute(&pool).await.unwrap();

    // Reserve 500 bid
    let alloc = PlannedAllocation {
        session_id,
        total_bid: 500,
        chunks: vec![500],
    };
    let batch = repo
        .reserve_execution("TON/USDT", 0, &[alloc])
        .await
        .unwrap()
        .unwrap();

    // Simulate a failure for this chunk
    let results = vec![UserResult {
        session_id,
        cooldown_ms: None,
        chunk_results: vec![ChunkResult {
            chunk_id: batch.users[0].chunks[0].chunk_id,
            status: ChunkStatus::Failed {
                reason: "Slippage".into(),
            },
        }],
    }];

    repo.commit_batch(&batch, &results).await.unwrap();

    let row = sqlx::query("SELECT remaining_bid, in_flight_bid FROM sessions WHERE session_id = ?")
        .bind(session_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();

    // remaining_bid should still be 1000 (no deduction for failure)
    assert_eq!(row.get::<i64, _>("remaining_bid"), 1000);
    // in_flight_bid MUST be 0 (unwound)
    assert_eq!(
        row.get::<i64, _>("in_flight_bid"),
        0,
        "In-flight funds must be released on failure"
    );
}

/// Ensures that re-committing a batch doesn't keep pushing the cooldown further out.
#[tokio::test]
async fn test_commit_cooldown_idempotency() {
    let pool = setup_db().await;
    let repo = SqlxSessionRepository::new(pool.clone());
    let session_id = Uuid::new_v4();

    sqlx::query(r#"INSERT INTO sessions VALUES (?, 'TON/USDT', 1, 50, 100, 75, 100, 1000, 1000, 10, 0, 0, 0, 100, 0, 0, 0)"#)
            .bind(session_id.to_string()).execute(&pool).await.unwrap();

    let alloc = PlannedAllocation {
        session_id,
        total_bid: 100,
        chunks: vec![100],
    };
    let batch = repo
        .reserve_execution("TON/USDT", 0, &[alloc])
        .await
        .unwrap()
        .unwrap();

    let results = vec![UserResult {
        session_id,
        cooldown_ms: Some(60000), // 1 minute
        chunk_results: vec![ChunkResult {
            chunk_id: batch.users[0].chunks[0].chunk_id,
            status: ChunkStatus::Success {
                tx_id: "tx1".into(),
            },
        }],
    }];

    // First commit sets cooldown
    repo.commit_batch(&batch, &results).await.unwrap();
    let first_cd: i64 =
        sqlx::query_scalar("SELECT cooldown_until_ms FROM sessions WHERE session_id = ?")
            .bind(session_id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();

    // Second commit (idempotent) should not change the cooldown
    repo.commit_batch(&batch, &results).await.unwrap();
    let second_cd: i64 =
        sqlx::query_scalar("SELECT cooldown_until_ms FROM sessions WHERE session_id = ?")
            .bind(session_id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(
        first_cd, second_cd,
        "Idempotent commit should not alter cooldown"
    );
}

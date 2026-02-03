use sqlx::any::AnyPoolOptions;
use sqlx::{AnyPool, Row};
use std::sync::Arc;
use tokio::task::JoinSet;
use uuid::Uuid;

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
          last_served_ms BIGINT NOT NULL
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
        r#"INSERT INTO sessions VALUES (?, 'TON/USDT', 1, 50, 100, 75, 100000, 1000000, 1000, 10, 0, 0, 0, 100000, 42, 0)"#,
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
        r#"INSERT INTO sessions VALUES (?, 'TON/USDT', 1, 50, 100, 75, 100000, 1000000, 1000, 10, 0, 0, 0, 100000, 0, 0)"#,
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
        r#"INSERT INTO sessions VALUES ('bad-uuid', 'TON/USDT', 1, 50, 100, 75, 100000, 1000000, 1000, 10, 0, 0, 0, 100000, 0, 0)"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    let good_id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO sessions VALUES (?, 'TON/USDT', 1, 50, 100, 75, 100000, 1000000, 1000, 10, 0, 0, 0, 100000, 0, 0)"#,
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
        r#"INSERT INTO sessions VALUES (?, 'TON/USDT', 1, 50, 100, 75, 100000, 1000000, 1000, 10, 0, 0, 0, 100000, 0, 0)"#,
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
        r#"INSERT INTO sessions VALUES (?, 'TON/USDT', 1, 50, 100, 75, 100000, 1000000, 1000, 10, 0, 0, 0, 100000, 0, 0)"#,
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
        sqlx::query(r#"INSERT INTO sessions VALUES (?, 'TON/USDT', 1, 50, 100, 75, 100000, 1000000, 1000, 10, 0, 0, 0, 100000, 0, 0)"#)
            .bind(Uuid::new_v4().to_string())
            .execute(&pool).await.unwrap();
    }

    // Requesting offset exactly at total count
    let page = repo.fetch_page(10, 2).await.unwrap();
    assert!(page.is_empty());
}

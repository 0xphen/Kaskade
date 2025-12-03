use std::sync::Arc;
use uuid::Uuid;

use sqlx::migrate::Migrator;
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};
use std::path::Path;

use session::manager::SessionManager;
use session::model::{Session, SessionState, SessionThresholds};
use session::store::sqlite_store::SQLiteSessionStore;

use market::types::Pair;

static MIGRATOR: Migrator = sqlx::migrate!("db/migrations");

/// Creates a fresh SQLite DB file per test run
async fn setup_db() -> SqlitePool {
    // Create a temporary DB path
    let db_path = "./test_sessions.sqlite";

    // Remove old DB file
    if Path::new(db_path).exists() {
        std::fs::remove_file(db_path).unwrap();
    }

    let opts = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .to_owned();

    let pool = SqlitePool::connect_with(opts).await.unwrap();

    // Run migrations
    MIGRATOR.run(&pool).await.unwrap();

    pool
}

#[tokio::test]
async fn test_session_manager_with_real_sqlite() -> anyhow::Result<()> {
    // Setup REAL SQLite database
    let pool = setup_db().await;

    let store = SQLiteSessionStore::from_pool(pool.clone());
    let store = Arc::new(store);

    // SessionManager loading old sessions from DB
    let manager = SessionManager::new(store.clone()).await?;

    // Ensure DB and memory start empty
    assert_eq!(
        manager
            .iter_active_for_pair(&Pair::new("TON".into(), "USDT".into()))
            .await
            .len(),
        0
    );

    let mut session = Session {
        id: Uuid::new_v4(),
        user_id: 42,
        pair: Pair::new("TON".into(), "USDT".into()),
        created_at_ms: 1_000_000,
        expires_at_ms: Some(2_000_000),

        total_amount_in: 10_000,
        chunk_amount_in: 1_000,
        approved_amount_in: Some(10_000),

        executed_amount_in: 0,
        executed_amount_out: 0,
        remaining_amount_in: 10_000,

        num_executed_chunks: 0,
        last_execution_ts_ms: None,

        wallet_address: Some("EQ_TEST".into()),
        state: SessionState::Created,

        thresholds: SessionThresholds {
            max_spread_bps: 50.0,
            max_slippage_bps: 20.0,
            trend_enabled: true,
        },
    };

    let session_id = manager.create_session(session.clone()).await?;
    println!("Created session ID = {}", session_id);

    // Pre-approve + Activate
    manager
        .mark_pre_approved(session_id, 10_000, "EQ_TEST".into())
        .await?;
    manager.activate_session(session_id).await?;

    // Should now be active
    let active = manager
        .iter_active_for_pair(&Pair::new("TON".into(), "USDT".into()))
        .await;
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].state, SessionState::Active);

    // Execute a chunk (simulate swap)
    manager
        .on_chunk_executed(session_id, 1_000, 1234, 1_500_000)
        .await?;

    // After execution: 9k remaining
    let updated = manager.get_session(session_id).await.unwrap();
    assert_eq!(updated.executed_amount_in, 1_000);
    assert_eq!(updated.remaining_amount_in, 9_000);
    assert_eq!(updated.state, SessionState::Active);

    // Simulate restart → reload from DB
    let manager2 = SessionManager::new(store.clone()).await?;

    let active_after_reload = manager2
        .iter_active_for_pair(&Pair::new("TON".into(), "USDT".into()))
        .await;

    assert_eq!(active_after_reload.len(), 1);
    let s = &active_after_reload[0];

    assert_eq!(s.executed_amount_in, 1_000);
    assert_eq!(s.remaining_amount_in, 9_000);
    assert_eq!(s.state, SessionState::Active);

    // Execute until completion
    manager2
        .on_chunk_executed(session_id, 9_000, 7777, 1_600_000)
        .await?;

    // Session must now be removed entirely
    let none = manager2.get_session(session_id).await;
    assert!(none.is_none());

    let active_final = manager2
        .iter_active_for_pair(&Pair::new("TON".into(), "USDT".into()))
        .await;
    assert!(active_final.is_empty());

    Ok(())
}

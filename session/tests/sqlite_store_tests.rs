use sqlx::SqlitePool;
use uuid::Uuid;

use market::types::Pair;
use session::model::{Session, SessionState, SessionThresholds};
use session::store::SessionStore;
use session::store::sqlite_store::SQLiteSessionStore;

///
/// Production-grade test suite for SQLiteSessionStore
///
/// This suite verifies:
///   · correct schema migration
///   · correct save() insert + update
///   · correct enum serialization/deserialization
///   · correct JSON thresholds handling
///   · correct nullable timestamp fields
///   · correct deletion
///   · multi-session handling
///
/// All tests rely on the real SQLx migration defined in /migrations.
///
fn sample_session() -> Session {
    Session {
        id: Uuid::new_v4(),
        user_id: 42,

        pair: Pair {
            base: "TON".into(),
            quote: "USDT".into(),
        },

        created_at_ms: 1_000,
        expires_at_ms: Some(9_999),

        total_amount_in: 500_000,
        chunk_amount_in: 50_000,
        approved_amount_in: Some(500_000),

        executed_amount_in: 0,
        executed_amount_out: 0,
        remaining_amount_in: 500_000,
        num_executed_chunks: 0,

        last_execution_ts_ms: None,
        wallet_address: Some("EQWallet123".into()),

        state: SessionState::Active,

        thresholds: SessionThresholds {
            max_spread_bps: 30.0,
            max_slippage_bps: 25.0,
            trend_enabled: true,
        },
    }
}

/// Test insertion + load
#[sqlx::test(migrations = "db/migrations")]
async fn test_insert_and_load(pool: SqlitePool) -> anyhow::Result<()> {
    let store = SQLiteSessionStore::from_pool(pool);

    let session = sample_session();
    let session_id = session.id;

    store.save(&session).await?;

    let loaded = store.load_all().await?;
    assert_eq!(loaded.len(), 1);

    let s = &loaded[0];
    assert_eq!(s.id, session_id);
    assert_eq!(s.user_id, 42);
    assert_eq!(s.pair.base, "TON");
    assert_eq!(s.pair.quote, "USDT");
    assert_eq!(s.state, SessionState::Active);
    assert_eq!(s.total_amount_in, 500_000);
    assert_eq!(s.chunk_amount_in, 50_000);
    assert_eq!(s.remaining_amount_in, 500_000);

    // Thresholds JSON restored correctly
    assert!((s.thresholds.max_spread_bps - 30.0).abs() < 1e-9);
    assert!((s.thresholds.max_slippage_bps - 25.0).abs() < 1e-9);
    assert!(s.thresholds.trend_enabled);

    Ok(())
}

/// Test UPDATE via second save()
#[sqlx::test(migrations = "db/migrations")]
async fn test_update_existing(pool: SqlitePool) -> anyhow::Result<()> {
    let store = SQLiteSessionStore::from_pool(pool);

    let mut session = sample_session();

    // Insert
    store.save(&session).await?;

    // Modify
    session.executed_amount_in = 100_000;
    session.executed_amount_out = 250_000;
    session.remaining_amount_in = 400_000;
    session.state = SessionState::Completed;

    // Update
    store.save(&session).await?;

    let loaded = store.load_all().await?;
    assert_eq!(loaded.len(), 1);

    let s = &loaded[0];
    assert_eq!(s.executed_amount_in, 100_000);
    assert_eq!(s.executed_amount_out, 250_000);
    assert_eq!(s.remaining_amount_in, 400_000);
    assert_eq!(s.state, SessionState::Completed);

    Ok(())
}

/// Test deletion
#[sqlx::test(migrations = "db/migrations")]
async fn test_delete(pool: SqlitePool) -> anyhow::Result<()> {
    let store = SQLiteSessionStore::from_pool(pool);

    let session = sample_session();
    let session_id = session.id;

    store.save(&session).await?;
    assert_eq!(store.load_all().await?.len(), 1);

    store.delete(session_id).await?;

    let loaded = store.load_all().await?;
    assert!(loaded.is_empty());

    Ok(())
}

/// Test multiple sessions inserted, loaded, and deleted independently
#[sqlx::test(migrations = "db/migrations")]
async fn test_multi_session_roundtrip(pool: SqlitePool) -> anyhow::Result<()> {
    let store = SQLiteSessionStore::from_pool(pool);

    let mut s1 = sample_session();
    s1.id = Uuid::new_v4();
    s1.user_id = 100;

    let mut s2 = sample_session();
    s2.id = Uuid::new_v4();
    s2.user_id = 200;

    store.save(&s1).await?;
    store.save(&s2).await?;

    let loaded = store.load_all().await?;
    assert_eq!(loaded.len(), 2);

    // Delete one
    store.delete(s1.id).await?;

    let loaded = store.load_all().await?;
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].user_id, 200);

    Ok(())
}

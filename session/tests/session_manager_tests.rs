use std::sync::Arc;

use tokio::test;
use uuid::Uuid;

use market::types::Pair;
use session::manager::SessionManager;
use session::model::{Session, SessionState, SessionThresholds};
use session::store::SessionStore;

mod mock_store;
use mock_store::InMemorySessionStore;

fn sample_session() -> Session {
    Session {
        id: Uuid::new_v4(),
        user_id: 42,
        pair: Pair {
            base: "TON".into(),
            quote: "USDT".into(),
        },
        created_at_ms: 1000,
        expires_at_ms: Some(9999),
        total_amount_in: 10_000,
        chunk_amount_in: 1_000,
        approved_amount_in: None,
        executed_amount_in: 0,
        executed_amount_out: 0,
        remaining_amount_in: 10_000,
        num_executed_chunks: 0,
        last_execution_ts_ms: None,
        wallet_address: None,
        state: SessionState::Created,
        thresholds: SessionThresholds {
            max_spread_bps: 50.0,
            max_slippage_bps: 25.0,
            trend_enabled: false,
        },
    }
}

#[test]
async fn restore_from_store_loads_all_sessions() -> anyhow::Result<()> {
    let store = Arc::new(InMemorySessionStore::default());

    // Pretend DB has 1 session
    let s = sample_session();
    store.save(&s).await?;

    let mgr = SessionManager::new(store.clone()).await?;

    let restored = mgr.get_session(s.id).await;
    assert!(restored.is_some());
    assert_eq!(restored.unwrap().user_id, 42);

    Ok(())
}

#[test]
async fn create_session_stores_and_indexes() -> anyhow::Result<()> {
    let store = Arc::new(InMemorySessionStore::default());
    let mgr = SessionManager::new(store.clone()).await?;

    let s = sample_session();
    let id = mgr.create_session(s.clone()).await?;

    // Should be in memory
    assert!(mgr.get_session(id).await.is_some());

    // Should be in store too
    let from_store = store.map.lock().await.get(&id).cloned();
    assert!(from_store.is_some());

    // Should be indexed by pair
    let actives = mgr.iter_active_for_pair(&s.pair).await;
    assert!(actives.is_empty()); // created != active

    Ok(())
}

#[test]
async fn mark_pre_approved_updates_state_and_persists() -> anyhow::Result<()> {
    let store = Arc::new(InMemorySessionStore::default());
    let mgr = SessionManager::new(store.clone()).await?;

    let s = sample_session();
    let id = mgr.create_session(s.clone()).await?;

    mgr.mark_pre_approved(id, 5000, "EQTEST".into()).await?;

    let sess = mgr.get_session(id).await.unwrap();
    assert_eq!(sess.state, SessionState::WaitingPreApproval);
    assert_eq!(sess.approved_amount_in, Some(5000));
    assert_eq!(sess.wallet_address.as_deref(), Some("EQTEST"));

    // Confirm persisted
    let stored = store.map.lock().await.get(&id).unwrap().clone();
    assert_eq!(stored.approved_amount_in, Some(5000));

    Ok(())
}

#[test]
async fn activate_session_checks_funds_and_persists() -> anyhow::Result<()> {
    let store = Arc::new(InMemorySessionStore::default());
    let mgr = SessionManager::new(store.clone()).await?;

    let mut s = sample_session();
    s.approved_amount_in = Some(10_000);

    let id = mgr.create_session(s.clone()).await?;
    mgr.activate_session(id).await?;

    let sess = mgr.get_session(id).await.unwrap();
    assert_eq!(sess.state, SessionState::Active);

    Ok(())
}

#[test]
async fn cancel_session_removes_everywhere() -> anyhow::Result<()> {
    let store = Arc::new(InMemorySessionStore::default());
    let mgr = SessionManager::new(store.clone()).await?;

    let s = sample_session();
    let id = mgr.create_session(s.clone()).await?;

    mgr.cancel_session(id).await?;

    assert!(mgr.get_session(id).await.is_none());
    assert!(store.map.lock().await.get(&id).is_none());

    Ok(())
}

#[test]
async fn iter_active_for_pair_returns_only_active_ones() -> anyhow::Result<()> {
    let store = Arc::new(InMemorySessionStore::default());
    let mgr = SessionManager::new(store.clone()).await?;

    let mut s1 = sample_session();
    let mut s2 = sample_session();

    // Same pair
    s1.pair = Pair {
        base: "TON".into(),
        quote: "USDT".into(),
    };
    s2.pair = s1.pair.clone();

    s1.approved_amount_in = Some(10_000);
    s2.approved_amount_in = Some(10_000);

    let id1 = mgr.create_session(s1.clone()).await?;
    let id2 = mgr.create_session(s2.clone()).await?;

    mgr.activate_session(id1).await?;
    mgr.activate_session(id2).await?;

    let active = mgr.iter_active_for_pair(&s1.pair).await;
    assert_eq!(active.len(), 2);

    Ok(())
}

#[test]
async fn on_chunk_executed_updates_progress_and_completes() -> anyhow::Result<()> {
    let store = Arc::new(InMemorySessionStore::default());
    let mgr = SessionManager::new(store.clone()).await?;

    let mut s = sample_session();
    s.approved_amount_in = Some(10_000);
    let id = mgr.create_session(s.clone()).await?;

    mgr.activate_session(id).await?;

    // Execute 10 chunks of 1000 each = 10000
    for i in 0..10 {
        mgr.on_chunk_executed(id, 1000, 999, 1000 + i).await?;
    }

    let sess = mgr.get_session(id).await;
    assert!(sess.is_none()); // removed from memory
    assert!(store.map.lock().await.get(&id).is_none()); // removed from store

    Ok(())
}

#[test]
async fn expire_sessions_marks_expired_and_removes() -> anyhow::Result<()> {
    let store = Arc::new(InMemorySessionStore::default());
    let mgr = SessionManager::new(store.clone()).await?;

    let mut s = sample_session();
    s.expires_at_ms = Some(1500);

    let id = mgr.create_session(s.clone()).await?;

    mgr.expire_sessions(2000).await;

    assert!(mgr.get_session(id).await.is_none());
    assert!(store.map.lock().await.get(&id).is_none());

    Ok(())
}

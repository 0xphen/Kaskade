use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use uuid::Uuid;

use crate::model::{Session, SessionId, SessionState};
use crate::store::SessionStore;
use market::types::Pair;

/// Manages the in-memory live set of sessions and persists changes to a store.
pub struct SessionManager<S: SessionStore> {
    sessions: Arc<Mutex<HashMap<SessionId, Session>>>,
    by_pair: Arc<Mutex<HashMap<Pair, Vec<SessionId>>>>,
    store: Arc<S>,
}

impl<S: SessionStore> SessionManager<S> {
    /// Initialize a fresh manager from the store (load_all).
    pub async fn new(store: Arc<S>) -> anyhow::Result<Self> {
        let manager = Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            by_pair: Arc::new(Mutex::new(HashMap::new())),
            store,
        };

        manager.restore_from_store().await?;
        Ok(manager)
    }

    /// Load all previously saved sessions and rebuild indexes.
    async fn restore_from_store(&self) -> anyhow::Result<()> {
        let all: Vec<Session> = self.store.load_all().await?;
        let mut sessions = self.sessions.lock().await;
        let mut by_pair = self.by_pair.lock().await;

        for s in all {
            by_pair.entry(s.pair.clone()).or_default().push(s.id);

            sessions.insert(s.id, s);
        }

        Ok(())
    }

    /// Create a new session, store it, and index it.
    pub async fn create_session(&self, mut session: Session) -> anyhow::Result<SessionId> {
        let id = Uuid::new_v4();
        session.id = id;

        {
            // Save to DB
            self.store.save(&session).await?;
        }

        {
            // Insert into memory
            let mut guard = self.sessions.lock().await;
            guard.insert(id, session.clone());
        }

        {
            // Insert into secondary index
            let mut idx = self.by_pair.lock().await;
            idx.entry(session.pair.clone()).or_default().push(id);
        }

        Ok(id)
    }

    /// Mark the session as pre-approved.
    pub async fn mark_pre_approved(
        &self,
        session_id: SessionId,
        approved: u64,
        wallet: String,
    ) -> anyhow::Result<()> {
        let mut guard = self.sessions.lock().await;
        let s = guard
            .get_mut(&session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        s.approved_amount_in = Some(approved);
        s.wallet_address = Some(wallet);
        s.state = SessionState::WaitingPreApproval;

        self.store.save(s).await?;
        Ok(())
    }

    pub async fn activate_session(&self, session_id: SessionId) -> anyhow::Result<()> {
        let mut guard = self.sessions.lock().await;
        let s = guard
            .get_mut(&session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        if s.approved_amount_in.unwrap_or(0) < s.total_amount_in {
            anyhow::bail!("Insufficient pre-approved funds");
        }

        s.state = SessionState::Active;
        self.store.save(s).await?;

        Ok(())
    }

    /// Cancel session: mark Cancelled, persist, then remove from index.
    pub async fn cancel_session(&self, session_id: SessionId) -> anyhow::Result<()> {
        let mut guard = self.sessions.lock().await;

        let s = guard
            .get_mut(&session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        s.state = SessionState::Cancelled;

        // Persist updated state
        self.store.save(s).await?;

        drop(guard);
        self.remove_from_index(session_id).await?;

        // Also remove from DB entirely after cancellation.
        self.store.delete(session_id).await?;

        // Remove from memory map
        let mut guard = self.sessions.lock().await;
        guard.remove(&session_id);

        Ok(())
    }

    async fn remove_from_index(&self, session_id: SessionId) -> anyhow::Result<()> {
        let sessions_guard = self.sessions.lock().await;
        let session = sessions_guard
            .get(&session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        let mut idx = self.by_pair.lock().await;

        if let Some(list) = idx.get_mut(&session.pair) {
            list.retain(|sid| *sid != session_id);
        }

        Ok(())
    }

    /// Sessions that are active for a specific pair.
    pub async fn iter_active_for_pair(&self, pair: &Pair) -> Vec<Session> {
        let ids_opt = {
            let idx = self.by_pair.lock().await;
            idx.get(pair).cloned()
        };

        let Some(ids) = ids_opt else { return vec![] };

        let sessions = self.sessions.lock().await;

        ids.into_iter()
            .filter_map(|sid| sessions.get(&sid).cloned())
            .filter(|s| s.state == SessionState::Active)
            .collect()
    }

    /// Update after a chunk.
    pub async fn on_chunk_executed(
        &self,
        session_id: SessionId,
        executed_in: u64,
        executed_out: u64,
        now_ms: u64,
    ) -> anyhow::Result<()> {
        let mut guard = self.sessions.lock().await;
        let s = guard
            .get_mut(&session_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        if s.state != SessionState::Active {
            anyhow::bail!("Session not active");
        }

        s.executed_amount_in += executed_in;
        s.executed_amount_out += executed_out;

        if executed_in <= s.remaining_amount_in {
            s.remaining_amount_in -= executed_in;
        } else {
            s.remaining_amount_in = 0;
        }

        s.last_execution_ts_ms = Some(now_ms);
        s.num_executed_chunks += 1;

        if s.remaining_amount_in == 0 {
            s.state = SessionState::Completed;

            // Remove from indexes
            drop(guard);
            self.remove_from_index(session_id).await?;
            self.store.delete(session_id).await?;

            let mut g = self.sessions.lock().await;
            g.remove(&session_id);

            return Ok(());
        }

        // Persist updated progress
        self.store.save(s).await?;
        Ok(())
    }

    /// Expire sessions
    pub async fn expire_sessions(&self, now_ms: u64) {
        let mut guard = self.sessions.lock().await;

        let expired: Vec<_> = guard
            .values_mut()
            .filter(|s| s.is_expired(now_ms))
            .map(|s| {
                s.state = SessionState::Expired;
                s.id
            })
            .collect();

        drop(guard);

        for id in expired {
            let _ = self.store.delete(id).await;
            let _ = self.remove_from_index(id).await;

            let mut guard = self.sessions.lock().await;
            guard.remove(&id);
        }
    }

    /// Helper to fetch a single session.
    pub async fn get_session(&self, id: SessionId) -> Option<Session> {
        let guard = self.sessions.lock().await;
        guard.get(&id).cloned()
    }
}

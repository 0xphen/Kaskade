use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use session::model::{Session, SessionId};
use session::store::SessionStore;

#[derive(Default, Clone)]
pub struct MockStore {
    pub map: Arc<Mutex<HashMap<SessionId, Session>>>,
}

impl MockStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Test convenience
    pub async fn insert_direct(&self, session: Session) {
        self.map.lock().await.insert(session.id, session);
    }
}

#[async_trait]
impl SessionStore for MockStore {
    async fn load_all(&self) -> anyhow::Result<Vec<Session>> {
        Ok(self.map.lock().await.values().cloned().collect())
    }

    async fn save(&self, session: &Session) -> anyhow::Result<()> {
        self.map.lock().await.insert(session.id, session.clone());
        Ok(())
    }

    async fn delete(&self, session_id: SessionId) -> anyhow::Result<()> {
        self.map.lock().await.remove(&session_id);
        Ok(())
    }
}

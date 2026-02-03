use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

use crate::session::model::Session;

#[async_trait]
pub trait SessionRepository: Send + Sync {
    async fn fetch_page(&self, limit: usize, offset: usize) -> Result<Vec<Session>>;

    async fn fetch_by_id(&self, session_id: &Uuid) -> Result<Option<Session>>;

    async fn persist_fairness(
        &self,
        session_id: &Uuid,
        deficit: i128,
        last_served_ms: u64,
    ) -> Result<()>;
}

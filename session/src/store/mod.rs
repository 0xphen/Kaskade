pub mod sqlite_store;

#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    async fn load_all(&self) -> anyhow::Result<Vec<crate::model::Session>>;
    async fn save(&self, session: &crate::model::Session) -> anyhow::Result<()>;
    async fn delete(&self, session_id: crate::model::SessionId) -> anyhow::Result<()>;
}

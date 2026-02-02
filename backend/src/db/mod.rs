pub mod schema;
use std::sync::Arc;

use sqlx::AnyPool;
use sqlx::any::AnyPoolOptions;

#[derive(Clone)]
pub struct Db {
    pub pool: Arc<AnyPool>,
}

impl Db {
    pub async fn connect(database_url: &str) -> anyhow::Result<Self> {
        let pool = AnyPoolOptions::new()
            .max_connections(16)
            .connect(database_url)
            .await?;

        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    pub async fn migrate(&self) -> anyhow::Result<()> {
        schema::migrate(&self.pool).await
    }
}

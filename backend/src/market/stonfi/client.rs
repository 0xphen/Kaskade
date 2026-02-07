use std::time::Duration;

use reqwest::Client;
use tracing::{debug, instrument};

use crate::market::stonfi::errors::StonfiError;
use crate::market::stonfi::types::{Pool, PoolEnvelope};

#[derive(Clone)]
pub struct StonfiClient {
    http: Client,
    url: String,
}

impl StonfiClient {
    pub fn new(url: String) -> Result<Self, StonfiError> {
        let http = Client::builder()
            .timeout(Duration::from_secs(5))
            .pool_idle_timeout(Duration::from_secs(30))
            .tcp_keepalive(Duration::from_secs(30))
            .build()?;

        Ok(Self { http, url })
    }

    #[instrument(
        skip(self),
        fields(pool_address = %pool_address),
        level = "debug"
    )]
    pub async fn fetch_pool(&self, pool_address: &str) -> Result<Pool, StonfiError> {
        let url: String = format!("{}/pools/{}", self.url, pool_address);

        let resp = self.http.get(&url).send().await?.error_for_status()?;

        let envelope: PoolEnvelope = resp.json().await?;

        debug!(
            reserve0 = %envelope.pool.reserve0,
            reserve1 = %envelope.pool.reserve1,
            "stonfi pool fetched"
        );

        Ok(envelope.pool)
    }
}

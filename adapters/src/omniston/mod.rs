pub mod provider;
pub mod types;
pub mod ws;

use async_trait::async_trait;
use tokio::sync::mpsc::Sender;

use crate::omniston::types::QuoteEvent;

#[async_trait]
pub trait WsMarketDataProvider: Send + Sync {
    async fn stream_quotes(
        &self,
        bid_asset: &str,
        ask_asset: &str,
        bid_units: Option<&str>,
        ask_units: Option<&str>,
        sender: Sender<QuoteEvent>,
    ) -> anyhow::Result<()>;
}

use async_trait::async_trait;
use tokio::sync::mpsc::Sender;

use super::{WsMarketDataProvider, types::QuoteEvent, ws::OmnistonWsClient};

pub struct OmnistonProvider {
    client: OmnistonWsClient,
}

impl OmnistonProvider {
    pub fn new(ws_url: String) -> Self {
        Self {
            client: OmnistonWsClient::new(ws_url),
        }
    }
}

#[async_trait]
impl WsMarketDataProvider for OmnistonProvider {
    async fn stream_quotes(
        &self,
        bid_asset: &str,
        ask_asset: &str,
        bid_units: Option<&str>,
        ask_units: Option<&str>,
        sender: Sender<QuoteEvent>,
    ) -> anyhow::Result<()> {
        self.client
            .run_ws_loop(bid_asset, ask_asset, bid_units, ask_units, sender)
            .await
    }
}

use async_trait::async_trait;
use corelib::omniston_models::OmnistonEvent;
use tokio::sync::mpsc::Sender;

#[derive(Debug, Clone)]
pub enum RfqAmount {
    BidUnits(String),
    AskUnits(String),
}

#[derive(Debug, Clone)]
pub struct RfqRequest {
    pub bid_asset: String,
    pub ask_asset: String,
    pub amount: RfqAmount,
}

/// High-level abstraction for Omniston RFQ streaming.
#[async_trait]
pub trait OmnistonApi: Send + Sync {
    async fn request_for_quote_stream(
        &self,
        req: RfqRequest,
        sender: Sender<OmnistonEvent>,
    ) -> anyhow::Result<()>;
}

pub mod normalized_quote;
pub mod parser;
pub mod rfq_ws;

use async_trait::async_trait;
use tokio::sync::mpsc::Sender;

use crate::types::*;

/// High-level abstraction for Omniston RFQ streaming.
#[async_trait]
pub trait OmnistonApi: Send + Sync + 'static {
    async fn request_for_quote_stream(
        &self,
        req: RfqRequest,
        sender: Sender<OmnistonEvent>,
    ) -> anyhow::Result<()>;
}

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tokio_tungstenite::{connect_async, tungstenite::Message};
// Robust logging imports
use tracing::{Span, debug, error, info, instrument, warn};

use super::parser::parse_omniston_event;
use super::{OmnistonApi, RfqAmount, RfqRequest};
use crate::market::types::OmnistonEvent;

/// WebSocket implementation of the Omniston API.
pub struct OmnistonWsClient {
    pub ws_url: String,
}

impl OmnistonWsClient {
    pub fn new(ws_url: String) -> Self {
        Self { ws_url }
    }

    /// Convert an `RfqRequest` into the JSON-RPC `"params"` object.
    fn build_params(req: &RfqRequest) -> serde_json::Value {
        let (key, val) = match &req.amount {
            RfqAmount::BidUnits(v) => ("bid_units", v.clone()),
            RfqAmount::AskUnits(v) => ("ask_units", v.clone()),
        };

        json!({
            "bid_asset_address": { "blockchain": 607, "address": req.bid_asset },
            "ask_asset_address": { "blockchain": 607, "address": req.ask_asset },
            "amount": { key: val },
            "referrer_fee_bps": 0,
            "settlement_methods": [0],
            "settlement_params": {
                "max_price_slippage_bps": 0,
                "max_outgoing_messages": 4,
                "gasless_settlement": 1,
                "flexible_referrer_fee": true,
            }
        })
    }

    /// Send a single RFQ message over the WebSocket connection.
    #[instrument(skip(write, req), fields(bid = %req.bid_asset, ask = %req.ask_asset))]
    async fn send_rfq<E>(
        write: &mut (impl futures::Sink<Message, Error = E> + Unpin),
        req: &RfqRequest,
    ) -> anyhow::Result<()>
    where
        E: std::fmt::Debug + Send + Sync + 'static,
    {
        let params = Self::build_params(req);
        let json_req = json!({
            "jsonrpc": "2.0",
            "id": "1",
            "method": "v1beta7.quote",
            "params": params
        });

        let text = serde_json::to_string(&json_req)?;
        debug!(payload = %text, "Sending v1beta7.quote request");

        write.send(Message::Text(text.into())).await.map_err(|e| {
            error!(error = ?e, "Failed to send RFQ message over WebSocket");
            anyhow::anyhow!("{:?}", e)
        })?;

        Ok(())
    }
}

#[async_trait]
impl OmnistonApi for OmnistonWsClient {
    /// Subscribe to a real-time RFQ stream with resilient reconnection and tracing.
    #[instrument(
        skip(self, sender, req),
        fields(
            url = %self.ws_url,
            bid = %req.bid_asset,
            ask = %req.ask_asset
        )
    )]
    async fn request_for_quote_stream(
        &self,
        req: RfqRequest,
        sender: Sender<OmnistonEvent>,
    ) -> anyhow::Result<()> {
        info!("Starting Omniston RFQ stream worker");

        loop {
            debug!("Attempting connection to Omniston WebSocket");
            match connect_async(&self.ws_url).await {
                Ok((ws, _)) => {
                    info!("WebSocket connection established");
                    let (mut write, mut read) = ws.split();

                    if let Err(e) = Self::send_rfq(&mut write, &req).await {
                        error!(error = ?e, "Initial RFQ subscription failed; retrying connection");
                        // Fall through to the sleep/reconnect logic
                    } else {
                        // Process all messages until this socket dies.
                        while let Some(msg) = read.next().await {
                            let msg = match msg {
                                Ok(m) => m,
                                Err(e) => {
                                    warn!(error = ?e, "WebSocket stream error encountered");
                                    break;
                                }
                            };

                            if msg.is_ping() || msg.is_pong() {
                                debug!("Received keep-alive message");
                                continue;
                            }

                            if !msg.is_text() {
                                debug!(msg_type = ?msg, "Ignoring non-text WebSocket message");
                                continue;
                            }

                            let raw = match msg.to_text() {
                                Ok(t) => t,
                                Err(e) => {
                                    error!(error = ?e, "Failed to extract text from WS message");
                                    continue;
                                }
                            };

                            print!("raw: {:?}", raw);

                            // Only log raw events at TRACE level to avoid production log bloat
                            tracing::trace!(raw_event = %raw, "Received raw WebSocket message");

                            match parse_omniston_event(raw, &super::QuoteSide::Bid) {
                                Ok(Some(ev)) => {
                                    debug!(event_type = ?ev, "Parsed Omniston event, forwarding to channel");
                                    if let Err(e) = sender.send(ev).await {
                                        error!(error = ?e, "MPSC channel receiver dropped; worker shutting down");
                                        return Err(anyhow::anyhow!("Channel closed: {}", e));
                                    }
                                }
                                Ok(None) => debug!("Received non-actionable or filtered event"),
                                Err(e) => {
                                    warn!(error = ?e, raw = %raw, "Failed to parse incoming WebSocket event")
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(error = ?e, "WebSocket connection failed");
                }
            }

            let retry_interval = Duration::from_secs(3);
            warn!(interval = ?retry_interval, "Disconnected; attempting reconnection");
            tokio::time::sleep(retry_interval).await;
        }
    }
}

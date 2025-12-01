use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, Utf8Bytes},
};

use crate::omniston::types::QuoteEvent;

/// Client for connecting to the Omniston RFQ WebSocket API.
///
/// This client:
/// - sends a `v1beta7.quote` subscription request
/// - streams all `quote_updated` events
/// - forwards events into an internal mpsc channel for async processing
pub struct OmnistonWsClient {
    pub ws_url: String,
}

impl OmnistonWsClient {
    pub fn new(ws_url: String) -> Self {
        Self { ws_url }
    }

    /// Send a subscription request to the Omniston WebSocket.
    ///
    /// Omniston uses a JSON-RPC method `v1beta7.quote`
    /// to request a stream of updated RFQ quotes.
    ///
    /// # Parameters
    /// - `write` — a WebSocket Sink (writer half) capable of sending tungstenite Messages.
    /// - `bid_asset`, `ask_asset` — TON jetton addresses.
    /// - `bid_units` — optional amount of bid asset.
    /// - `ask_units` — optional amount of ask asset.
    ///
    /// Only one of `bid_units` or `ask_units` should be set (Omniston rules).
    async fn send_subscription(
        write: &mut (impl futures::Sink<Message, Error = tungstenite::Error> + Unpin),
        bid_asset: &str,
        ask_asset: &str,
        bid_units: Option<&str>,
        ask_units: Option<&str>,
    ) -> anyhow::Result<()> {
        // Build `"params"` for the JSON-RPC request.
        // If bid_units is provided → use { bid_units: X }
        // Else → we use ask_units.
        let params = if let Some(bid) = bid_units {
            serde_json::json!({
                "bid_asset_address": {
                    "blockchain": 607,
                    "address": bid_asset
                },
                "ask_asset_address": {
                    "blockchain": 607,
                    "address": ask_asset
                },
                "amount": { "bid_units": bid },
               // "referrer_address": { "blockchain": 607, "address": "" },
                "referrer_fee_bps": 0,
                "settlement_methods": [0], // Only swap supported
                "settlement_params": {
                    "max_price_slippage_bps": 100,
                    "max_outgoing_messages": 4,
                    "gasless_settlement": 1,
                    "flexible_referrer_fee": true
                }
            })
        } else {
            serde_json::json!({
                "bid_asset_address": {
                    "blockchain": 607,
                    "address": bid_asset
                },
                "ask_asset_address": {
                    "blockchain": 607,
                    "address": ask_asset
                },
                "amount": { "ask_units": ask_units.unwrap() },
                // "referrer_address": { "blockchain": 607, "address": "" },
                "referrer_fee_bps": 0,
                "settlement_methods": [0],
                "settlement_params": {
                    "max_price_slippage_bps": 0,
                    "max_outgoing_messages": 4,
                    "gasless_settlement": 1,
                    "flexible_referrer_fee": true
                }
            })
        };

        let json_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "1",
            "method": "v1beta7.quote",
            "params": params
        });

        let text = serde_json::to_string(&json_req)?;
        write.send(Message::Text(text.into())).await?;

        Ok(())
    }

    /// Main WebSocket event loop.
    ///
    /// Responsibilities:
    /// 1. Connect to Omniston WebSocket (auto-reconnect).
    /// 2. Send subscription request for RFQ streaming.
    /// 3. Continuously read all incoming WebSocket messages.
    /// 4. Extract `"quote_updated"` events from JSON.
    /// 5. Forward `QuoteEvent` into an mpsc channel.
    ///
    /// This loop **never stops** unless the whole application is shut down.
    pub async fn run_ws_loop(
        &self,
        bid_asset: &str,
        ask_asset: &str,
        bid_units: Option<&str>,
        ask_units: Option<&str>,
        sender: Sender<QuoteEvent>,
    ) -> anyhow::Result<()> {
        loop {
            println!("🚀 Connecting to Omniston WebSocket...");

            // Attempt connection
            match connect_async(&self.ws_url).await {
                Ok((ws, _)) => {
                    println!("✅ Connected to Omniston WebSocket");
                    let (mut write, mut read) = ws.split();

                    // Send RFQ subscription
                    Self::send_subscription(&mut write, bid_asset, ask_asset, bid_units, ask_units)
                        .await?;

                    while let Some(msg) = read.next().await {
                        let msg = match msg {
                            Ok(m) => {
                                println!("📩 Incoming: {}", m);
                                m
                            }
                            Err(e) => {
                                eprintln!("❌ WebSocket error: {e}");
                                break;
                            }
                        };

                        if !msg.is_text() {
                            continue;
                        }

                        let raw = msg.to_text()?;
                        let json: Value = serde_json::from_str(raw)?;

                        let event = &json["params"]["result"]["event"];

                        // --------------------------------------------------
                        // 1. ACK HANDLING
                        // --------------------------------------------------
                        if let Some(ack) = event["ack"].as_object() {
                            if let Some(rfq_id) = ack.get("rfq_id").and_then(|v| v.as_str()) {
                                println!("🆗 RFQ ACK received — rfq_id = {}", rfq_id);
                            }
                            continue;
                        }

                        // --------------------------------------------------
                        // 2. NO_QUOTE HANDLING
                        // --------------------------------------------------
                        if event.get("no_quote").is_some() {
                            println!("⚠️ No quote available yet (resolver has no route)");
                            continue;
                        }

                        // --------------------------------------------------
                        // 3. KEEP ALIVE HANDLING
                        // --------------------------------------------------
                        if event.get("keep_alive").is_some() {
                            println!("💓 Keep-alive received");
                            continue;
                        }

                        // --------------------------------------------------
                        // 4. QUOTE UPDATED HANDLING
                        // --------------------------------------------------
                        if let Some(q) = event["quote_updated"].as_object() {
                            let event: QuoteEvent =
                                serde_json::from_value(serde_json::json!({ "quote_updated": q }))?;

                            println!("💰 Quote updated event received — forwarding...");
                            let _ = sender.send(event).await;
                            continue;
                        }

                        // --------------------------------------------------
                        // 5. Unknown event (debug)
                        // --------------------------------------------------
                        println!("❓ Unknown event type: {}", event);
                    }
                }

                // Could not connect (network down etc.)
                Err(e) => eprintln!("❌ WebSocket connection failed: {e}"),
            }

            println!("⏳ Reconnecting in 3 seconds...");
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
}

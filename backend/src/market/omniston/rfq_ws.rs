//! Omniston WebSocket RFQ client.
//!
//! This module implements a resilient, JSON-RPC–based WebSocket client used to
//! subscribe to Omniston RFQ (Request-For-Quote) streams.  
//!
//! Responsibilities:
//! - Establish and maintain a persistent WebSocket connection.
//! - Send a `v1beta7.quote` RFQ subscription built from `RfqRequest`.
//! - Parse incoming messages via `parse_omniston_event` into typed events.
//! - Forward events asynchronously through a `tokio::mpsc::Sender`.
//!
//! The client automatically reconnects on network errors and is suitable for
//! long-running components such as:
//! - Spread monitoring engines
//! - Routing/price discovery engines
//! - Execution engines
//! - Multi-RFQ orchestration systems
//!
//! The client is intentionally minimal: it does not perform retries, rate limit
//! RFQ sends, or manage multiple RFQs — that is the responsibility of higher-level
//! orchestration layers (`OmnistonApi`, `PulseEngine`, etc.).
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tokio_tungstenite::{connect_async, tungstenite::Message};

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
            "bid_asset_address": {
                "blockchain": 607,
                "address": req.bid_asset,
            },
            "ask_asset_address": {
                "blockchain": 607,
                "address": req.ask_asset,
            },
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
        write
            .send(Message::Text(text.into()))
            .await
            .map_err(|e| anyhow::anyhow!("{:?}", e))?;

        Ok(())
    }
}

#[async_trait]
impl OmnistonApi for OmnistonWsClient {
    /// Subscribe to a real-time RFQ stream.
    ///
    /// This loop runs forever unless externally cancelled.  
    /// It reconnects after any socket or protocol error.
    async fn request_for_quote_stream(
        &self,
        req: RfqRequest,
        sender: Sender<OmnistonEvent>,
    ) -> anyhow::Result<()> {
        loop {
            println!("🚀 Connecting to Omniston WebSocket...");
            match connect_async(&self.ws_url).await {
                Ok((ws, _)) => {
                    println!("✅ Connected");
                    let (mut write, mut read) = ws.split();

                    // Subscribe once per connection.
                    Self::send_rfq(&mut write, &req).await?;

                    // Process all messages until this socket dies.
                    while let Some(msg) = read.next().await {
                        let msg = match msg {
                            Ok(m) => m,
                            Err(e) => {
                                eprintln!("❌ WS error: {}", e);
                                break;
                            }
                        };

                        if !msg.is_text() {
                            continue;
                        }

                        let raw = msg.to_text()?;

                        if let Some(ev) = parse_omniston_event(raw, &super::QuoteSide::Bid)? {
                            let _ = sender.send(ev).await;
                        }
                    }
                }

                Err(e) => eprintln!("❌ WebSocket connection failed: {}", e),
            }

            println!("⏳ Reconnecting in 3 seconds...");
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use futures::{SinkExt, StreamExt, channel::mpsc as fmpsc};

//     /// A mock WebSocket sink/stream pair for controlled testing.
//     fn mock_ws() -> (fmpsc::Sender<Message>, fmpsc::Receiver<Message>) {
//         fmpsc::channel(32)
//     }

//     #[tokio::test]
//     async fn build_params_bid_units_is_correct() {
//         let req = RfqRequest {
//             bid_asset: "EQBID".into(),
//             ask_asset: "EQASK".into(),
//             amount: RfqAmount::BidUnits("1000".into()),
//             side: crate::types::QuoteSide::Bid,
//         };

//         let p = OmnistonWsClient::build_params(&req);
//         assert_eq!(p["amount"]["bid_units"], "1000");
//         assert!(p["amount"].get("ask_units").is_none());
//     }

//     #[tokio::test]
//     async fn send_rfq_produces_valid_json() {
//         let req = RfqRequest {
//             bid_asset: "EQBID".into(),
//             ask_asset: "EQASK".into(),
//             amount: RfqAmount::BidUnits("500".into()),
//             side: crate::types::QuoteSide::Bid,
//         };

//         let (mut sink, mut stream) = mock_ws();

//         // Send subscription
//         OmnistonWsClient::send_rfq(&mut sink, &req).await.unwrap();

//         // Assert we received the right JSON
//         let msg = stream.next().await.expect("expected a WS message");

//         match msg {
//             Message::Text(t) => {
//                 let json: serde_json::Value = serde_json::from_str(&t).unwrap();
//                 assert_eq!(json["method"], "v1beta7.quote");
//                 assert_eq!(json["params"]["amount"]["bid_units"], "500");
//             }
//             _ => panic!("expected Message::Text"),
//         }
//     }

//     // -------------------------------------------------------------------------
//     // request_for_quote_stream() — indirect test via parser
//     // -------------------------------------------------------------------------

//     #[tokio::test]
//     async fn parsed_events_are_forwarded_to_channel() {
//         use crate::types::{OmnistonEvent, QuoteSide};

//         let (mut mock_sink, mut mock_stream) = mock_ws();
//         let (sender, mut receiver) = tokio::sync::mpsc::channel(8);

//         // Inject a fake quote_updated event
//         mock_sink
//             .send(Message::Text(
//                 r#"
//         {
//           "jsonrpc": "2.0",
//           "method": "event",
//           "params": {
//             "subscription": 1,
//             "result": {
//               "event": {
//                 "quote_updated": {
//                   "quote_id": "Q1",
//                   "resolver_id": "RID",
//                   "resolver_name": "Omniston",
//                   "bid_asset_address": { "blockchain": 607, "address": "EQBID" },
//                   "ask_asset_address": { "blockchain": 607, "address": "EQASK" },
//                   "bid_units": "100",
//                   "ask_units": "200",
//                   "referrer_address": null,
//                   "referrer_fee_asset": { "blockchain": 607, "address": "EQFEE" },
//                   "referrer_fee_units": "0",
//                   "protocol_fee_asset": { "blockchain": 607, "address": "EQPROT" },
//                   "protocol_fee_units": "0",
//                   "quote_timestamp": 123,
//                   "trade_start_deadline": 456,
//                   "gas_budget": "1000",
//                   "estimated_gas_consumption": "50",
//                   "params": {
//                     "swap": {
//                       "routes": [],
//                       "min_ask_amount": "190",
//                       "recommended_min_ask_amount": "185",
//                       "recommended_slippage_bps": 0
//                     }
//                   }
//                 }
//               }
//             }
//           }
//         }
//         "#
//                 .into(),
//             ))
//             .await
//             .unwrap();

//         let raw = mock_stream.next().await.unwrap().into_text().unwrap();

//         if let Some(ev) = parse_omniston_event(&raw, &crate::types::QuoteSide::Ask).unwrap() {
//             sender.send(ev).await.unwrap();
//         }

//         match receiver.recv().await.unwrap() {
//             OmnistonEvent::QuoteUpdated(q) => assert_eq!(q.quote_id, "Q1"),
//             _ => panic!("expected QuoteUpdated event"),
//         }
//     }
// }

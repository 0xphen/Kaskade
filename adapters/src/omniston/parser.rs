//! Omniston WebSocket Event Parser
//!
//! This module defines a lightweight JSON-RPC event parser used for consuming
//! Omniston's RFQ (Request-For-Quote) WebSocket feed. Omniston sends all quote
//! updates, heartbeat messages, acknowledgements, and control signals through a
//! single `"method": "event"` WebSocket channel. Messages follow the structure:
//!
//! ```jsonc
//! {
//!   "jsonrpc": "2.0",
//!   "method": "event",
//!   "params": {
//!     "subscription": <u64>,
//!     "result": {
//!       "event": { /* event variant */ }
//!     }
//!   }
//! }
//! ```
//!
//! The parser converts each incoming raw JSON string into a strongly-typed
//! `OmnistonEvent` enum variant:
//!
//! - **Ack** – RFQ request acknowledged with an `rfq_id`  
//! - **QuoteUpdated** – Fully typed decoded quote with routes & swap params  
//! - **NoQuote** – Resolver currently cannot route the desired RFQ  
//! - **KeepAlive** – Heartbeat sent every ~5 seconds  
//! - **Unsubscribed** – RFQ stream explicitly closed by server  
//! - **Unknown** – Forward-compatibility fallback for new or undocumented events  
//!
//! The parser also handles:
//!
//! - Initial `"result": <subscription-id>` messages which must be ignored  
//! - Missing/invalid JSON (returns error)  
//! - Missing `"params"` (returns `None`)  
//! - Automatic fallthrough to `Unknown` for event shapes not yet supported  
//!
//! This module is intentionally small, stateless, and pure: the parser does not
//! maintain WebSocket state or RFQ session lifecycle. It is designed to be used
//! by higher-level components, such as the RFQ engine, spread monitor,
//! rolling-window pricing pipeline, or arbitrage detectors.
//!
//! All edge cases, including malformed events, forward-compatibility behavior,
//! and minimal quote deserialization, are covered by the accompanying test
//! suite (`#[cfg(test)]` below).
//!
//! Usage:
//!
//! ```rust
//! if let Some(event) = parse_omniston_event(raw_ws_message)? {
//!     match event {
//!         OmnistonEvent::Ack { rfq_id } => { /* ... */ }
//!         OmnistonEvent::QuoteUpdated(q) => { /* process quote */ }
//!         OmnistonEvent::NoQuote => { /* retry or ignore */ }
//!         OmnistonEvent::KeepAlive => { /* maintain connection */ }
//!         OmnistonEvent::Unknown(v) => { /* log + ignore */ }
//!         _ => {}
//!     }
//! }
//! ```
//!
//! This parser is the core building block for any higher-level logic that needs
//! reliable, typed real-time access to Omniston quotes.

use crate::omniston::models::*;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct RpcEventEnvelope {
    jsonrpc: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<RpcEventParams>,
}

#[derive(Debug, Deserialize)]
struct RpcEventParams {
    subscription: Option<u64>,
    result: RpcEventResult,
}

#[derive(Debug, Deserialize)]
struct RpcEventResult {
    event: Value,
}

pub fn parse_omniston_event(raw: &str) -> anyhow::Result<Option<OmnistonEvent>> {
    let json: Value = serde_json::from_str(raw)?;

    if json.get("method").is_none() && json.get("params").is_none() {
        return Ok(None);
    }

    let env: RpcEventEnvelope = serde_json::from_value(json.clone())?;
    let params = match env.params {
        Some(p) => p,
        None => return Ok(None),
    };

    let event = params.result.event;

    // ACK
    if let Some(ack) = event.get("ack") {
        if let Some(rfq_id) = ack.get("rfq_id").and_then(|v| v.as_str()) {
            return Ok(Some(OmnistonEvent::Ack {
                rfq_id: rfq_id.to_string(),
            }));
        }
    }

    // NO_QUOTE
    if event.get("no_quote").is_some() {
        return Ok(Some(OmnistonEvent::NoQuote));
    }

    // KEEP_ALIVE
    if event.get("keep_alive").is_some() {
        return Ok(Some(OmnistonEvent::KeepAlive));
    }

    // QUOTE_UPDATED
    if let Some(q) = event.get("quote_updated") {
        let ev: QuoteUpdatedEvent =
            serde_json::from_value(serde_json::json!({ "quote_updated": q }))?;
        return Ok(Some(OmnistonEvent::QuoteUpdated(Box::new(
            ev.quote_updated,
        ))));
    }

    // UNSUBSCRIBED (rare)
    if let Some(unsub) = event.get("unsubscribed") {
        let rfq_id = unsub
            .get("rfq_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        return Ok(Some(OmnistonEvent::Unsubscribed { rfq_id }));
    }

    Ok(Some(OmnistonEvent::Unknown(event)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ignore_subscription_result() {
        let raw = r#"{ "jsonrpc":"2.0", "result":324018, "id":"1" }"#;

        let parsed = parse_omniston_event(raw).unwrap();
        assert!(parsed.is_none(), "Subscription result must be ignored");
    }

    //
    // ---------------------------------------------------------------------------
    // 2. Basic ACK event
    // ---------------------------------------------------------------------------
    // { "event": { "ack": { "rfq_id": "ABC123" } } }
    //
    #[test]
    fn parse_ack_event() {
        let raw = json!({
            "jsonrpc":"2.0",
            "method":"event",
            "params":{
                "subscription":1,
                "result":{
                    "event":{ "ack": { "rfq_id":"ABC123" } }
                }
            }
        })
        .to_string();

        let ev = parse_omniston_event(&raw).unwrap().unwrap();
        match ev {
            OmnistonEvent::Ack { rfq_id } => assert_eq!(rfq_id, "ABC123"),
            _ => panic!("Expected Ack event"),
        }
    }

    //
    // ---------------------------------------------------------------------------
    // 3. NoQuote event
    // ---------------------------------------------------------------------------
    // { "event": { "no_quote": {} } }
    //
    #[test]
    fn parse_no_quote_event() {
        let raw = json!({
            "jsonrpc":"2.0",
            "method":"event",
            "params":{
                "subscription":1,
                "result":{
                    "event":{ "no_quote": {} }
                }
            }
        })
        .to_string();

        let ev = parse_omniston_event(&raw).unwrap().unwrap();
        assert!(matches!(ev, OmnistonEvent::NoQuote));
    }

    //
    // ---------------------------------------------------------------------------
    // 4. KeepAlive event
    // ---------------------------------------------------------------------------
    // { "event": { "keep_alive": {} } }
    //
    #[test]
    fn parse_keep_alive_event() {
        let raw = json!({
            "jsonrpc":"2.0",
            "method":"event",
            "params":{
                "subscription":1,
                "result":{
                    "event":{ "keep_alive": {} }
                }
            }
        })
        .to_string();

        let ev = parse_omniston_event(&raw).unwrap().unwrap();
        assert!(matches!(ev, OmnistonEvent::KeepAlive));
    }

    //
    // ---------------------------------------------------------------------------
    // 5. QuoteUpdated event — minimal valid structure
    // ---------------------------------------------------------------------------
    // We don't test routes fully here — only that the parser correctly produces
    // OmnistonEvent::QuoteUpdated and the Quote struct is populated.
    //
    #[test]
    fn parse_quote_updated_event() {
        let raw = json!({
            "jsonrpc":"2.0",
            "method":"event",
            "params":{
                "subscription":10,
                "result":{
                    "event":{
                        "quote_updated":{
                            "quote_id":"QID1",
                            "resolver_id":"RID1",
                            "resolver_name":"Omniston",
                            "bid_asset_address": { "blockchain":607, "address":"EQBID" },
                            "ask_asset_address": { "blockchain":607, "address":"EQASK" },
                            "bid_units":"1000",
                            "ask_units":"9000",
                            "referrer_address": null,
                            "referrer_fee_asset": { "blockchain":607, "address":"EQFEE" },
                            "referrer_fee_units":"0",
                            "protocol_fee_asset": { "blockchain":607, "address":"EQPROT" },
                            "protocol_fee_units":"0",
                            "quote_timestamp":100,
                            "trade_start_deadline":200,
                            "gas_budget":"300",
                            "estimated_gas_consumption":"50",
                            "params":{
                                "swap":{
                                    "routes":[],
                                    "min_ask_amount":"8000",
                                    "recommended_min_ask_amount":"7990",
                                    "recommended_slippage_bps":50
                                }
                            }
                        }
                    }
                }
            }
        })
        .to_string();

        let ev = parse_omniston_event(&raw).unwrap().unwrap();

        match ev {
            OmnistonEvent::QuoteUpdated(q) => {
                assert_eq!(q.quote_id, "QID1");
                assert_eq!(q.bid_units, "1000");
                assert_eq!(q.ask_units, "9000");
                assert_eq!(q.resolver_name, "Omniston");
                assert_eq!(q.params.swap.unwrap().min_ask_amount, "8000");
            }
            _ => panic!("Expected QuoteUpdated event"),
        }
    }

    //
    // ---------------------------------------------------------------------------
    // 6. Unsubscribed event
    // ---------------------------------------------------------------------------
    // { "event": { "unsubscribed": { "rfq_id": "XYZ" } } }
    //
    #[test]
    fn parse_unsubscribed_event() {
        let raw = json!({
            "jsonrpc":"2.0",
            "method":"event",
            "params":{
                "subscription":999,
                "result":{
                    "event":{
                        "unsubscribed":{ "rfq_id":"XYZ" }
                    }
                }
            }
        })
        .to_string();

        let ev = parse_omniston_event(&raw).unwrap().unwrap();

        match ev {
            OmnistonEvent::Unsubscribed { rfq_id } => {
                assert_eq!(rfq_id.unwrap(), "XYZ");
            }
            _ => panic!("Expected Unsubscribed"),
        }
    }

    //
    // ---------------------------------------------------------------------------
    // 7. Unknown event → must still be returned
    // ---------------------------------------------------------------------------
    // Tests forward compatibility.
    //
    #[test]
    fn parse_unknown_event() {
        let raw = json!({
            "jsonrpc":"2.0",
            "method":"event",
            "params":{
                "subscription":7,
                "result":{
                    "event":{
                        "strange_event_type":{
                            "foo":123
                        }
                    }
                }
            }
        })
        .to_string();

        let ev = parse_omniston_event(&raw).unwrap().unwrap();

        match ev {
            OmnistonEvent::Unknown(v) => {
                assert!(v.get("strange_event_type").is_some());
            }
            _ => panic!("Expected Unknown event fallback"),
        }
    }

    //
    // ---------------------------------------------------------------------------
    // 8. Invalid JSON → must return Err
    // ---------------------------------------------------------------------------
    //
    #[test]
    fn parse_invalid_json() {
        let raw = "{ this is not valid JSON ";

        let err = parse_omniston_event(raw);
        assert!(err.is_err(), "Invalid JSON must return an error");
    }

    //
    // ---------------------------------------------------------------------------
    // 9. Missing params → None
    // ---------------------------------------------------------------------------
    // WebSocket pings or malformed messages.
    //
    #[test]
    fn parse_missing_params() {
        let raw = r#"{ "jsonrpc":"2.0", "method":"event" }"#;

        let out = parse_omniston_event(raw).unwrap();
        assert!(out.is_none(), "Missing params must produce None");
    }
}

//! MarketManager
//!
//! This module manages live market state for RFQ-based pairs.
//! Responsibilities:
//!   • Maintain latest normalized market metrics for each trading pair
//!   • Spawn per-pair RFQ WebSocket streams
//!   • Normalize incoming Omniston quote events
//!   • Compute spread pulse via rolling window
//!   • Broadcast market snapshots to all subscribed components
//!
//! MarketManager is designed as an Arc-managed async service, ensuring that
//! long-lived tasks may safely capture `self` without lifetime issues.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{
    Mutex,
    mpsc::{self, Receiver, Sender},
};

use super::{
    omniston::{OmnistonApi, normalized_quote::NormalizedQuote},
    types::{MarketMetrics, OmnistonEvent, Pair, RfqRequest, SubscriptionRequest},
};

use crate::pulse::{PairPulseState, Pulse, PulseValidity, input::*};

/// MarketManager orchestrates RFQ streaming, quote normalization,
/// rolling-window metric computation, and snapshot broadcasting.
pub struct MarketManager<C> {
    /// Latest market metrics indexed by Pair
    pub states: Arc<Mutex<HashMap<Pair, MarketMetrics>>>,

    /// Omniston WebSocket client implementation
    pub omniston_ws_client: Arc<C>,

    /// Component subscribers interested in receiving market snapshots
    pub subscribers: Arc<Mutex<HashMap<Pair, Vec<Sender<MarketMetrics>>>>>,

    /// Per-pair pulse engines (spread, trend, depth)
    pub pulses: Arc<Mutex<HashMap<Pair, PairPulseState>>>,
}

impl<C: OmnistonApi> MarketManager<C> {
    /// Create a new MarketManager wrapped in Arc<Self> for multi-task ownership.
    pub fn new(omniston_ws_client: Arc<C>) -> Arc<Self> {
        Arc::new(Self {
            omniston_ws_client,
            states: Arc::new(Mutex::new(HashMap::new())),
            subscribers: Arc::new(Mutex::new(HashMap::new())),
            pulses: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Subscribe a component to a given trading pair.
    ///
    /// Responsibilities:
    ///   • Store subscriber channel
    ///   • Start RFQ WebSocket for the pair if not already started
    ///   • Spawn event processing task for the pair
    pub async fn subscribe(self: Arc<Self>, request: SubscriptionRequest) {
        let SubscriptionRequest {
            pair,
            amount,
            sender_ch,
        } = request;

        let mut map = self.subscribers.lock().await;

        // Only start a stream if this is the first subscriber for this pair
        if map.get(&pair).is_none() {
            let (bid_asset, ask_asset) = Self::fetch_pair_addresses(&pair);
            map.insert(pair.clone(), vec![sender_ch]);

            println!("🔌 Subscribed to market feed for pair {}", pair.id());

            let req = RfqRequest {
                bid_asset,
                ask_asset,
                amount,
            };

            // RFQ WebSocket channel
            let (tx, rx) = mpsc::channel(50);
            let ws_client = Arc::clone(&self.omniston_ws_client);

            // Spawn RFQ stream task
            let pair_id_clone = pair.id();
            tokio::spawn(async move {
                let _ = ws_client.request_for_quote_stream(req, tx).await;
                println!("🟢 RFQ WebSocket started for {}", pair_id_clone);
            });

            // Spawn event processing task
            let mm = Arc::clone(&self);
            let pair_clone = pair.clone();
            tokio::spawn(async move {
                println!("📥 Receiver task running for {}", pair_clone.id());
                mm.process_event_stream(rx, pair_clone).await;
            });
        } else {
            // Pair already has an active stream, simply add this subscriber
            map.entry(pair.clone()).or_default().push(sender_ch);
            println!("➕ Added subscriber for existing pair stream {}", pair.id());
        }
    }

    /// Process incoming events for a specific trading pair.
    ///
    /// Responsibilities:
    ///   • Normalize Omniston quotes
    ///   • Update rolling window
    ///   • Compute spread pulse
    ///   • Update MarketMetrics
    ///   • Notify all subscribers for that pair
    pub async fn process_event_stream(
        self: Arc<Self>,
        mut event_rx: Receiver<OmnistonEvent>,
        pair: Pair,
    ) {
        while let Some(raw_event) = event_rx.recv().await {
            let OmnistonEvent::QuoteUpdated(quote) = raw_event else {
                continue;
            };

            let nq = NormalizedQuote::from_event(&quote);

            // ---- Build inputs ----
            let price_input = PriceInput {
                ts_ms: nq.ts_ms,
                bid_units: nq.bid_units,
                ask_units: nq.ask_units,
            };

            let quote_input = QuoteInput {
                ts_ms: nq.ts_ms,
                quote: Arc::from(quote),
            };

            // ---- Evaluate pulses (stateful, sequential) ----
            let (spread, trend, depth, slippage) = {
                let mut pulses_guard = self.pulses.lock().await;

                let pulse_state = pulses_guard
                    .entry(pair.clone())
                    .or_insert_with(PairPulseState::default);

                (
                    pulse_state.spread.evaluate(price_input),
                    pulse_state.trend.evaluate(price_input),
                    pulse_state.depth.evaluate(quote_input.clone()),
                    pulse_state.slipage.evaluate(quote_input),
                )
            };

            // ---- Validity gating ----
            if !matches!(
                (
                    spread.validity,
                    trend.validity,
                    depth.validity,
                    slippage.validity
                ),
                (
                    PulseValidity::Valid,
                    PulseValidity::Valid,
                    PulseValidity::Valid,
                    PulseValidity::Valid
                )
            ) {
                // Optional: debug log here
                continue;
            }

            // ---- Update MarketMetrics ----
            let snapshot = {
                let mut states_guard = self.states.lock().await;

                let entry = states_guard
                    .entry(pair.clone())
                    .or_insert_with(MarketMetrics::default);

                entry.spread = spread;
                entry.trend = trend;
                entry.depth = depth;
                entry.slippage = slippage;

                entry.clone()
            };

            // ---- Broadcast ----
            let subs_guard = self.subscribers.lock().await;

            if let Some(channels) = subs_guard.get(&pair) {
                for ch in channels {
                    let _ = ch.send(snapshot.clone()).await;
                }
            }
        }

        println!("⚠️ RFQ stream ended for pair {}", pair.id());
    }

    /// Map a Pair to its blockchain asset addresses.
    ///
    /// TODO: Replace with actual resolver once Omniston finalizes API.
    fn fetch_pair_addresses(_pair: &Pair) -> (String, String) {
        (
            "EQCxE6mUtQJKFnGfaROTKOt1lZbDiiX1kCixRv7Nw2Id_sDs".into(),
            "EQC98_qAmNEptUtPc7W6xdHh_ZHrBUFpw5Ft_IzNU20QAJav".into(),
        )
    }
}

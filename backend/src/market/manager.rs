use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{
    Mutex,
    mpsc::{self, Receiver, Sender},
};
// Production logging imports
use tracing::{Instrument, debug, error, info, instrument, warn};

use super::{
    omniston::{OmnistonApi, normalized_quote::NormalizedQuote},
    types::{MarketMetrics, OmnistonEvent, Pair, RfqRequest, SubscriptionRequest},
};
use crate::market::pulse::{PairPulseState, Pulse, PulseValidity, input::*};

pub struct MarketManager<C> {
    pub states: Arc<Mutex<HashMap<Pair, MarketMetrics>>>,
    pub omniston_ws_client: Arc<C>,
    pub subscribers: Arc<Mutex<HashMap<Pair, Vec<Sender<MarketMetrics>>>>>,
    pub pulses: Arc<Mutex<HashMap<Pair, PairPulseState>>>,
}

impl<C: OmnistonApi> MarketManager<C> {
    pub fn new(omniston_ws_client: Arc<C>) -> Arc<Self> {
        Arc::new(Self {
            omniston_ws_client,
            states: Arc::new(Mutex::new(HashMap::new())),
            subscribers: Arc::new(Mutex::new(HashMap::new())),
            pulses: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    #[instrument(skip(self, request), fields(pair = %request.pair.id()))]
    pub async fn subscribe(self: Arc<Self>, request: SubscriptionRequest) {
        let SubscriptionRequest {
            pair,
            amount,
            sender_ch,
        } = request;
        let mut map = self.subscribers.lock().await;

        if map.get(&pair).is_none() {
            let (bid_asset, ask_asset) = Self::fetch_pair_addresses(&pair);
            map.insert(pair.clone(), vec![sender_ch]);

            info!("Initializing new market feed for pair");

            let req = RfqRequest {
                bid_asset,
                ask_asset,
                amount,
            };
            let (tx, rx) = mpsc::channel(50);
            let ws_client = Arc::clone(&self.omniston_ws_client);

            // Spawn RFQ stream task with its own instrumented span
            let pair_id = pair.id();
            let stream_span = tracing::info_span!("rfq_stream_task", %pair_id);
            tokio::spawn(
                async move {
                    info!("Starting RFQ WebSocket stream");
                    if let Err(e) = ws_client.request_for_quote_stream(req, tx).await {
                        error!(error = ?e, "RFQ WebSocket stream crashed");
                    }
                }
                .instrument(stream_span),
            );

            // Spawn event processing task
            let mm = Arc::clone(&self);
            let pair_clone = pair.clone();
            let processor_span =
                tracing::info_span!("event_processor_task", pair = %pair_clone.id());
            tokio::spawn(
                async move {
                    debug!("Receiver task running for events");
                    mm.process_event_stream(rx, pair_clone).await;
                }
                .instrument(processor_span),
            );
        } else {
            map.entry(pair.clone()).or_default().push(sender_ch);
            debug!("Added new subscriber to existing pair stream");
        }
    }

    #[instrument(skip(self, event_rx), fields(pair = %pair.id()))]
    pub async fn process_event_stream(
        self: Arc<Self>,
        mut event_rx: Receiver<OmnistonEvent>,
        pair: Pair,
    ) {
        info!("Beginning market event processing loop");

        while let Some(raw_event) = event_rx.recv().await {
            let OmnistonEvent::QuoteUpdated(quote) = raw_event else {
                continue;
            };

            let nq = NormalizedQuote::from_event(&quote);
            debug!(quote_id = %nq.ts_ms, "Received new market quote");

            // Evaluate pulses
            let (spread, trend, depth, slippage) = {
                let mut pulses_guard = self.pulses.lock().await;
                let pulse_state = pulses_guard
                    .entry(pair.clone())
                    .or_insert_with(PairPulseState::default);

                (
                    pulse_state.spread.evaluate(PriceInput {
                        ts_ms: nq.ts_ms,
                        bid_units: nq.bid_units.clone(),
                        ask_units: nq.ask_units.clone(),
                    }),
                    pulse_state.trend.evaluate(PriceInput {
                        ts_ms: nq.ts_ms,
                        bid_units: nq.bid_units,
                        ask_units: nq.ask_units,
                    }),
                    pulse_state.depth.evaluate(QuoteInput {
                        ts_ms: nq.ts_ms,
                        quote: Arc::from(quote.clone()),
                    }),
                    pulse_state.slipage.evaluate(QuoteInput {
                        ts_ms: nq.ts_ms,
                        quote: Arc::from(quote),
                    }),
                )
            };

            // Validity gating with warnings for dropped data
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
                warn!(spread = ?spread.validity, trend = ?trend.validity, "Pulse data invalid; skipping broadcast");
                continue;
            }

            // Update Metrics and Broadcast
            let snapshot = {
                let mut states_guard = self.states.lock().await;
                let entry = states_guard
                    .entry(pair.clone())
                    .or_insert_with(MarketMetrics::default);
                entry.spread = spread;
                entry.trend = trend;
                entry.depth = depth;
                entry.slippage = slippage;
                entry.ts_ms = nq.ts_ms;
                entry.clone()
            };

            let subs_guard = self.subscribers.lock().await;
            if let Some(channels) = subs_guard.get(&pair) {
                for ch in channels {
                    if let Err(e) = ch.try_send(snapshot.clone()) {
                        warn!(error = ?e, "Failed to send market snapshot to subscriber (channel full or closed)");
                    }
                }
            }
        }

        warn!("Market stream processing loop terminated");
    }

    fn fetch_pair_addresses(_pair: &Pair) -> (String, String) {
        (
            "EQCxE6mUtQJKFnGfaROTKOt1lZbDiiX1kCixRv7Nw2Id_sDs".into(),
            "EQC98_qAmNEptUtPc7W6xdHh_ZHrBUFpw5Ft_IzNU20QAJav".into(),
        )
    }
}

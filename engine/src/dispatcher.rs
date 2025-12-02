use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::Receiver;

use super::registry::PulseRegistry;
use crate::normalized_quote::NormalizedQuote;
use corelib::omniston_models::Quote;

/// Runs the central pulse-dispatcher loop.
///
/// This task receives **raw `Quote` events**, converts them into
/// `NormalizedQuote` values, and forwards them to the `PulseRegistry`.
/// Each normalized quote is evaluated by all registered engines, and any
/// emitted pulses are dispatched to all handlers registered for that pulse type.
///
/// This function should be spawned as a dedicated Tokio task in the
/// quote-streaming pipeline.
///
/// # Parameters
/// - `quote_rx`: an async channel delivering incoming `Quote` events.
/// - `registry`: shared `PulseRegistry` instance (engines + handlers).
///
/// The loop exits cleanly when the quote channel closes.
pub async fn run_dispatcher(mut quote_rx: Receiver<Quote>, registry: Arc<Mutex<PulseRegistry>>) {
    while let Some(raw_quote) = quote_rx.recv().await {
        let normalized = NormalizedQuote::from_event(&raw_quote);
        match registry.lock() {
            Ok(mut reg) => {
                reg.process_quote(normalized);
            }
            Err(poisoned) => {
                eprintln!("⚠️ PulseRegistry mutex poisoned — recovering.");
                let mut reg = poisoned.into_inner();
                reg.process_quote(normalized);
            }
        }
    }

    eprintln!("📥 Quote channel closed — dispatcher exiting.");
}

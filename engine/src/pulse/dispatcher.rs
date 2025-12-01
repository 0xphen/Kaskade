use std::sync::Arc;
use tokio::sync::mpsc::Receiver;

use crate::normalized_quote::NormalizedQuote;
use crate::pulse::{PulseEngine, PulseSignal};

/// A callback type for reacting to pulses
pub type PulseHandler = Arc<dyn Fn(PulseSignal) + Send + Sync>;

pub struct PulseDispatcher<E: PulseEngine> {
    engine: E,
    rx: Receiver<NormalizedQuote>,
    handler: PulseHandler,
}

impl<E: PulseEngine> PulseDispatcher<E> {
    pub fn new(engine: E, rx: Receiver<NormalizedQuote>, handler: PulseHandler) -> Self {
        Self {
            engine,
            rx,
            handler,
        }
    }

    /// Main loop: consumes quotes and forwards pulses to the handler.
    pub async fn run(mut self) {
        while let Some(q) = self.rx.recv().await {
            if let Some(signal) = self.engine.on_quote(q) {
                (self.handler)(signal);
            }
        }
    }
}

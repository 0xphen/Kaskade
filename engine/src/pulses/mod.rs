pub mod spread;
pub mod types;

use super::normalized_quote::NormalizedQuote;
use types::*;

#[derive(Debug, Clone)]
pub struct SpreadConfig {
    pub window_ms: u64,            // rolling window size
    pub spread_threshold_bps: f64, // pulse trigger threshold
}

#[derive(Debug, Clone)]
pub struct PulseSignal {
    pub pulse_type: PulseType,
    pub details: PulseDetails,
}

pub trait PulseEngine: Send + Sync {
    /// Called for every new normalized quote.
    /// Returns Some(PulseSignal) when the pulse fires.
    fn on_quote(&mut self, quote: NormalizedQuote) -> Option<PulseSignal>;
}

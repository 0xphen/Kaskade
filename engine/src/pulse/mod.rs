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

impl PulseSignal {
    pub fn spread(bid: f64, ask: f64, spread_bps: f64, threshold_bps: f64, ts: u64) -> Self {
        Self {
            pulse_type: PulseType::Spread,
            details: PulseDetails::Spread {
                bid,
                ask,
                spread_bps,
                threshold_bps,
                timestamp: ts,
            },
        }
    }

    pub fn slippage(estimated: f64, threshold: f64, ts: u64) -> Self {
        Self {
            pulse_type: PulseType::Slippage,
            details: PulseDetails::Slippage {
                estimated,
                threshold,
                timestamp: ts,
            },
        }
    }

    pub fn trend(slope: f64, confidence: f64, window: usize, ts: u64) -> Self {
        Self {
            pulse_type: PulseType::Trend,
            details: PulseDetails::Trend {
                slope,
                confidence,
                window_size: window,
                timestamp: ts,
            },
        }
    }
}

pub trait PulseEngine: Send + Sync {
    /// Identifies the type of pulse this engine implements.
    fn kind(&self) -> PulseType;

    /// Called for every new normalized quote.
    /// Returns Some(PulseSignal) when the pulse fires.
    fn on_quote(&mut self, quote: NormalizedQuote) -> Option<PulseSignal>;
}

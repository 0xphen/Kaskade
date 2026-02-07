pub mod depth;
pub mod input;
pub mod slippage;
pub mod spread;
pub mod trend;

use crate::market::types::ExecutionScope;

/// Warm-up requirements for
pub const MIN_SAMPLES: usize = 10;
pub const MIN_AGE_MS: u64 = 5_000;

/// Validity marker for all pulses.
///
/// Invalid pulses must NEVER allow execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PulseValidity {
    #[default]
    Invalid,

    Valid,
}

/// Trait implemented by all pulse result types.
pub trait PulseResult {
    fn validity(&self) -> PulseValidity;
}

/// Core Pulse trait.
///
/// A pulse:
/// - owns internal state
/// - consumes exactly one input tick
/// - produces a result
pub trait Pulse {
    /// Input type consumed per tick
    type Input;

    /// Output type produced per tick
    type Output: PulseResult;

    fn evaluate(&mut self, input: Self::Input) -> Self::Output;
}

pub struct PairPulseState {
    pub spread: spread::SpreadPulse,
    pub trend: trend::TrendPulse,
    pub depth: depth::DepthPulse,
    pub slipage: slippage::SlippagePulse,
}

impl Default for PairPulseState {
    fn default() -> Self {
        let scope = ExecutionScope::ProtocolOnly {
            protocol: "StonFiV2".to_string(),
        };

        Self {
            spread: spread::SpreadPulse::new(spread::SpreadWarmup::default(), scope.clone()),

            trend: trend::TrendPulse::new(trend::TrendWarmup::default(), scope.clone()),

            depth: depth::DepthPulse::new(scope.clone()),

            slipage: slippage::SlippagePulse::new(scope),
        }
    }
}

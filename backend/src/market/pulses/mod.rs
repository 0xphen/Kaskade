//! Market Pulse Abstraction
//!
//! A Pulse is a side-effect-free observer that derives a single market signal
//! (e.g., Spread, Trend, Depth) from raw pool snapshots.

pub mod depth;
pub mod spread;
pub mod trend;

pub use self::depth::DepthPulse;
pub use self::spread::SpreadMonitor;
pub use self::trend::TrendMonitor;

use crate::market::types::PoolSnapshot;

/// Trait for deriving market signals from pool state.
///
/// Implementors are responsible for maintaining their own internal state
/// (like rolling windows) while remaining deterministic.
pub trait MarketPulse {
    /// The specific signal produced by this pulse.
    type Output;

    /// Ingests a new snapshot to update internal metrics.
    fn update(&mut self, snapshot: PoolSnapshot);

    /// Computes and returns the current signal.
    ///
    /// # Safety
    /// This method must be side-effect free and never panic.
    fn compute(&self) -> Self::Output;

    /// Purges all internal history/state.
    fn reset(&mut self);
}

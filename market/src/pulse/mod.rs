pub mod depth;
pub mod slippage;
pub mod spread;
pub mod trend;

/// Indicates whether a pulse result is safe to use for trading decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PulseValidity {
    /// Warming up (not enough history). MUST NOT be used to trigger execution.
    Invalid,
    /// Sufficient history exists. Safe to use for eligibility checks.
    Valid,
}

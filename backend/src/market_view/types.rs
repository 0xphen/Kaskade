use serde::{Deserialize, Serialize};

/// Immutable market snapshot used by scheduler (Gate A) and executor (Gate B).
/// Metrics are computed upstream and treated as advisory constraints only.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketMetricsView {
    /// Snapshot timestamp (ms since epoch)
    pub ts_ms: u64,

    /// Execution constraint metrics (basis points)
    pub spread_bps: f64,
    pub trend_drop_bps: f64,
    pub slippage_bps: f64,

    /// Available input-side liquidity at snapshot time
    pub depth_now_in: u128,
}

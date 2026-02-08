use crate::market::{
    pulses::{
        MarketPulse,
        depth::{DepthPulse, DepthState},
        spread::SpreadMonitor,
        trend::TrendMonitor,
    },
    types::{MarketMetrics, PoolSnapshot},
};

/// Orchestrates all *market-level* pulses for a single STON.fi pool.
///
/// Responsibilities:
/// - Spread → structural friction (rolling)
/// - Trend  → temporal risk (rolling)
/// - Depth  → instantaneous capacity (on-demand)
///
/// Note:
/// - Market validity is determined ONLY by spread + trend
/// - Depth is advisory capacity, not a health signal
pub struct StonfiMarketService {
    spread: SpreadMonitor,
    trend: TrendMonitor,
    depth: DepthPulse,
}

impl StonfiMarketService {
    /// Create a new market service for a single pool.
    ///
    /// - `window_size`       → rolling window length (poll count)
    /// - `min_warmup_ms`     → minimum time span before trend becomes valid
    /// - `max_slippage_bps`  → depth slippage budget
    pub fn new(window_size: usize, min_warmup_ms: u64, max_slippage_bps: f64) -> Self {
        Self {
            spread: SpreadMonitor::new(window_size),
            trend: TrendMonitor::new(window_size, min_warmup_ms),
            depth: DepthPulse::new(max_slippage_bps),
        }
    }

    /// Ingest a new pool snapshot and update rolling market state.
    ///
    /// Called on every poll.
    pub fn tick(&mut self, snapshot: PoolSnapshot) -> MarketMetrics {
        self.spread.update(snapshot.clone());
        self.trend.update(snapshot.clone());

        let spread_state = self.spread.compute();
        let trend_state = self.trend.compute();
        let depth = self.depth.compute_with_snapshot(&snapshot);

        MarketMetrics {
            ts_ms: snapshot.ts_ms,
            spread_bps: spread_state.spread_bps,
            trend_drop_bps: trend_state.trend_drop_bps,
            max_depth: depth.max_dx,

            // Market is valid ONLY if spread + trend are healthy
            validity: spread_state.validity && trend_state.validity,
        }
    }

    /// Compute instantaneous market depth at a specific snapshot.
    ///
    /// This is used by:
    /// - scheduler (to cap batch sizes)
    /// - executor (to sanity-check execution)
    ///
    /// Depth does NOT affect market validity.
    pub fn depth_at(&self, snapshot: &PoolSnapshot) -> DepthState {
        self.depth.compute_with_snapshot(snapshot)
    }

    /// Reset all internal rolling state.
    ///
    /// Used when switching pools or recovering from data gaps.
    pub fn reset(&mut self) {
        self.spread.reset();
        self.trend.reset();
    }
}

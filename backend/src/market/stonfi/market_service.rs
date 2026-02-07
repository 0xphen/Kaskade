use crate::market::{
    pulses::{MarketPulse, spread::SpreadMonitor, trend::TrendMonitor},
    types::{MarketMetrics, PoolSnapshot},
};

/// Orchestrates all *market-level* pulses for a single STON.fi pool.
///
/// This service maintains rolling market state:
/// - Spread  → structural friction
/// - Trend   → temporal risk
///
/// Execution-specific checks (slippage simulation) happen elsewhere.
pub struct StonfiMarketService {
    spread: SpreadMonitor,
    trend: TrendMonitor,
}

impl StonfiMarketService {
    /// Create a new market service for a single pool.
    ///
    /// - `window_size`     → rolling window length (poll count)
    /// - `min_warmup_ms`   → minimum time span before trend becomes valid
    pub fn new(window_size: usize, min_warmup_ms: u64) -> Self {
        Self {
            spread: SpreadMonitor::new(window_size),
            trend: TrendMonitor::new(window_size, min_warmup_ms),
        }
    }

    /// Ingest a new pool snapshot and update market state.
    ///
    /// This should be called on *every poll* (e.g. every N seconds).
    pub fn tick(&mut self, snapshot: PoolSnapshot) -> MarketMetrics {
        // Update rolling pulses
        self.spread.update(snapshot.clone());
        self.trend.update(snapshot.clone());

        // Compute latest views
        let spread_state = self.spread.compute();
        let trend_state = self.trend.compute();

        MarketMetrics {
            ts_ms: snapshot.ts_ms,
            spread_bps: spread_state.spread_bps,
            trend_drop_bps: trend_state.trend_drop_bps,

            // Market is valid only if *both* pulses are healthy
            validity: spread_state.validity && trend_state.validity,
        }
    }

    /// Reset all internal rolling state.
    ///
    /// Used when switching pools or recovering from data gaps.
    pub fn reset(&mut self) {
        self.spread.reset();
        self.trend.reset();
    }
}

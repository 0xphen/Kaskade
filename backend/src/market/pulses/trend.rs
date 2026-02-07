//! STON.fi Trend Pulse.
//!
//! Detects *downward price pressure over time* using mid-price evolution.
//! This pulse protects users from trading into rapid sell-offs.

use std::collections::VecDeque;

use crate::market::{pulses::MarketPulse, types::PoolSnapshot};

#[derive(Debug, Clone, Default)]
pub struct TrendState {
    pub current_mid_price: f64,
    pub reference_mid_price: f64,
    pub trend_drop_bps: f64,
    pub window_duration_ms: u64,
    pub ts_ms: u64,
    pub validity: bool,
}

/// Rolling trend pulse.
pub struct TrendMonitor {
    window: VecDeque<PoolSnapshot>,
    max_size: usize,
    min_liquidity: u128,
    min_warmup_ms: u64,
}

impl TrendMonitor {
    pub fn new(max_size: usize, min_warmup_ms: u64) -> Self {
        Self {
            window: VecDeque::with_capacity(max_size),
            max_size,
            min_liquidity: 100,
            min_warmup_ms,
        }
    }
}

impl MarketPulse for TrendMonitor {
    type Output = TrendState;

    fn update(&mut self, snapshot: PoolSnapshot) {
        if self.window.len() >= self.max_size {
            self.window.pop_front();
        }
        self.window.push_back(snapshot);
    }

    fn compute(&self) -> TrendState {
        if self.window.len() < 2 {
            return TrendState::default();
        }

        let oldest = self.window.front().unwrap();
        let newest = self.window.back().unwrap();
        let duration = newest.ts_ms.saturating_sub(oldest.ts_ms);

        if oldest.reserve0 < self.min_liquidity
            || oldest.reserve1 < self.min_liquidity
            || newest.reserve0 < self.min_liquidity
            || newest.reserve1 < self.min_liquidity
        {
            return TrendState {
                ts_ms: newest.ts_ms,
                validity: false,
                ..Default::default()
            };
        }

        let old_mid = oldest.reserve1 as f64 / oldest.reserve0 as f64;
        let new_mid = newest.reserve1 as f64 / newest.reserve0 as f64;
        let drop_bps = ((old_mid - new_mid) / old_mid) * 10_000.0;

        TrendState {
            current_mid_price: new_mid,
            reference_mid_price: old_mid,
            trend_drop_bps: drop_bps,
            window_duration_ms: duration,
            ts_ms: newest.ts_ms,
            validity: drop_bps.is_finite() && duration >= self.min_warmup_ms,
        }
    }

    fn reset(&mut self) {
        self.window.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(r0: u128, r1: u128, ts: u64) -> PoolSnapshot {
        PoolSnapshot {
            reserve0: r0,
            reserve1: r1,
            ts_ms: ts,
            protocol_fee: 0,
            lp_fee: 0,
        }
    }

    #[test]
    fn detects_downward_trend() {
        let mut m = TrendMonitor::new(5, 1000);

        m.update(snap(1_000, 1_000, 0));
        m.update(snap(1_000, 950, 2_000));

        let t = m.compute();
        assert!(t.validity);
        assert!(t.trend_drop_bps > 499.0); // ≈ 500 bps
        assert!(t.current_mid_price < t.reference_mid_price);
    }

    #[test]
    fn upward_price_is_not_a_risk() {
        let mut m = TrendMonitor::new(5, 1000);

        m.update(snap(1_000, 1_000, 0));
        m.update(snap(1_000, 1_050, 2_000));

        let t = m.compute();
        assert!(t.validity);
        assert!(t.trend_drop_bps < 0.0); // price increased
    }

    #[test]
    fn warmup_gate_blocks_short_windows() {
        let mut m = TrendMonitor::new(5, 5_000);

        m.update(snap(1_000, 1_000, 0));
        m.update(snap(1_000, 900, 1_000)); // only 1s elapsed

        let t = m.compute();
        assert!(!t.validity);
    }

    #[test]
    fn liquidity_gate_blocks_ghost_pools() {
        let mut m = TrendMonitor::new(5, 1000);

        m.update(snap(1, 1_000, 0));
        m.update(snap(1, 900, 2_000));

        let t = m.compute();
        assert!(!t.validity);
    }

    #[test]
    fn insufficient_data_is_invalid() {
        let mut m = TrendMonitor::new(5, 1000);

        m.update(snap(1_000, 1_000, 0));

        let t = m.compute();
        assert!(!t.validity);
    }

    #[test]
    fn rolling_window_uses_oldest_and_newest() {
        let mut m = TrendMonitor::new(3, 1000);

        m.update(snap(1_000, 1_000, 0));
        m.update(snap(1_000, 980, 1_000));
        m.update(snap(1_000, 960, 2_000));
        m.update(snap(1_000, 940, 3_000)); // evicts first

        let t = m.compute();
        assert!(t.validity);
        assert!(t.trend_drop_bps > 100.0);
        assert_eq!(t.window_duration_ms, 2_000); // from ts=1k → 3k
    }

    #[test]
    fn numeric_stability_extreme_reserves() {
        let mut m = TrendMonitor::new(2, 1);

        m.update(snap(u128::MAX / 2, u128::MAX / 3, 0));
        m.update(snap(u128::MAX / 2, u128::MAX / 4, 10));

        let t = m.compute();
        assert!(t.trend_drop_bps.is_finite());
    }

    #[test]
    fn reset_clears_internal_state() {
        let mut m = TrendMonitor::new(5, 1000);

        m.update(snap(1_000, 1_000, 0));
        m.update(snap(1_000, 900, 2_000));
        assert!(m.compute().validity);

        m.reset();
        assert!(!m.compute().validity);
    }
}

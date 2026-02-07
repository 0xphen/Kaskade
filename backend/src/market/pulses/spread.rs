//! STON.fi AMM Spread Pulse.
//!
//! Computes the *structural (infinitesimal) spread* of a constant-product pool.
//! This captures:
//! - LP fees
//! - protocol fees
//! - AMM curvature
//!
//! It intentionally excludes size-dependent slippage.

use std::collections::VecDeque;

use crate::market::{pulses::MarketPulse, types::PoolSnapshot};

/// Derived infinitesimal spread state (ε → 0).
#[derive(Debug, Clone, Default)]
pub struct SpreadState {
    pub mid_price: f64,
    pub buy_price: f64,
    pub sell_price: f64,
    pub spread_bps: f64,
    pub ts_ms: u64,
    pub validity: bool,
}

/// Rolling spread pulse.
pub struct SpreadMonitor {
    window: VecDeque<SpreadState>,
    max_size: usize,
    min_liquidity: u128,
}

impl SpreadMonitor {
    pub fn new(max_size: usize) -> Self {
        Self {
            window: VecDeque::with_capacity(max_size),
            max_size,
            min_liquidity: 100,
        }
    }

    fn derive(snapshot: &PoolSnapshot, min_liquidity: u128) -> SpreadState {
        if snapshot.reserve0 < min_liquidity || snapshot.reserve1 < min_liquidity {
            return SpreadState {
                ts_ms: snapshot.ts_ms,
                validity: false,
                ..Default::default()
            };
        }

        let x = snapshot.reserve0 as f64;
        let y = snapshot.reserve1 as f64;

        let total_fee_bps = snapshot.lp_fee + snapshot.protocol_fee;
        let fee_factor = 1.0 - (total_fee_bps as f64 / 10_000.0);

        let mid = y / x;
        let eps = 1e-9;

        // Buy (token0 → token1)
        let dx_eff = eps * fee_factor;
        let dy = (y * dx_eff) / (x + dx_eff);
        let buy_price = dy / eps;

        // Sell (token1 → token0)
        let dy_eff = eps * fee_factor;
        let dx = (x * dy_eff) / (y + dy_eff);
        let sell_price = eps / dx;

        let spread_bps = ((buy_price - sell_price).abs() / mid) * 10_000.0;

        SpreadState {
            mid_price: mid,
            buy_price,
            sell_price,
            spread_bps,
            ts_ms: snapshot.ts_ms,
            validity: spread_bps.is_finite(),
        }
    }
}

impl MarketPulse for SpreadMonitor {
    type Output = SpreadState;

    fn update(&mut self, snapshot: PoolSnapshot) {
        let state = Self::derive(&snapshot, self.min_liquidity);
        if self.window.len() >= self.max_size {
            self.window.pop_front();
        }
        self.window.push_back(state);
    }

    fn compute(&self) -> Self::Output {
        self.window.back().cloned().unwrap_or_default()
    }

    fn reset(&mut self) {
        self.window.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(r0: u128, r1: u128, lp: u32, proto: u32) -> PoolSnapshot {
        PoolSnapshot {
            reserve0: r0,
            reserve1: r1,
            lp_fee: lp,
            protocol_fee: proto,
            ts_ms: 1_000,
        }
    }

    #[test]
    fn balanced_pool_spread_matches_fees() {
        let mut monitor = SpreadMonitor::new(5);

        // 30 bps total fee → ~60 bps round-trip structural spread
        let snap = snapshot(1_000_000, 1_000_000, 20, 10);
        monitor.update(snap);

        let state = monitor.compute();
        assert!(state.validity);
        assert!((state.spread_bps - 60.0).abs() < 1.0);
    }

    #[test]
    fn asymmetric_pool_has_valid_spread() {
        let mut monitor = SpreadMonitor::new(5);

        let snap = snapshot(2_000_000, 1_000_000, 20, 10);
        monitor.update(snap);

        let state = monitor.compute();
        assert!(state.validity);
        assert!(state.spread_bps > 0.0);
        assert!(state.mid_price > 0.0);
    }

    #[test]
    fn liquidity_gate_blocks_ghost_pools() {
        let mut monitor = SpreadMonitor::new(5);

        let snap = snapshot(1, 1_000_000, 20, 10);
        monitor.update(snap);

        let state = monitor.compute();
        assert!(!state.validity);
    }

    #[test]
    fn zero_fee_pool_has_near_zero_spread() {
        let mut monitor = SpreadMonitor::new(5);

        let snap = snapshot(1_000_000, 1_000_000, 0, 0);
        monitor.update(snap);

        let state = monitor.compute();
        assert!(state.validity);
        assert!(state.spread_bps < 0.1);
    }

    #[test]
    fn rolling_window_keeps_latest_state() {
        let mut monitor = SpreadMonitor::new(2);

        let s1 = snapshot(1_000_000, 1_000_000, 20, 10);
        let s2 = snapshot(1_200_000, 1_000_000, 20, 10);
        let s3 = snapshot(1_500_000, 1_000_000, 20, 10);

        monitor.update(s1);
        monitor.update(s2);
        monitor.update(s3);

        let state = monitor.compute();
        assert!(state.mid_price < 1.0); // y/x < 1 due to higher reserve0
    }

    #[test]
    fn numeric_stability_extreme_reserves() {
        let mut monitor = SpreadMonitor::new(1);

        let snap = snapshot(u128::MAX / 2, u128::MAX / 3, 20, 10);
        monitor.update(snap);

        let state = monitor.compute();
        assert!(state.spread_bps.is_finite());
    }

    #[test]
    fn reset_clears_state() {
        let mut monitor = SpreadMonitor::new(5);

        let snap = snapshot(1_000_000, 1_000_000, 20, 10);
        monitor.update(snap);
        assert!(monitor.compute().validity);

        monitor.reset();
        assert!(!monitor.compute().validity);
    }
}

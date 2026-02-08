//! STON.fi Depth Pulse.
//!
//! Computes *maximum executable trade size* for a given slippage tolerance.
//! This pulse answers: “How much can I trade *right now* safely?”

use crate::market::{pulses::MarketPulse, types::PoolSnapshot};

#[derive(Debug, Clone, Default)]
pub struct DepthState {
    pub max_dx: u128,
    pub slippage_bps: f64,
    pub ts_ms: u64,
    pub validity: bool,
}

/// Stateless depth pulse (computed on demand).
pub struct DepthPulse {
    min_liquidity: u128,
    max_iterations: usize,
    max_slippage_bps: f64,
}

impl DepthPulse {
    pub fn new(max_slippage_bps: f64) -> Self {
        Self {
            min_liquidity: 100,
            max_iterations: 32,
            max_slippage_bps,
        }
    }
}

impl MarketPulse for DepthPulse {
    type Output = DepthState;

    fn update(&mut self, _snapshot: PoolSnapshot) {
        // Stateless pulse — nothing to store
    }

    fn compute(&self) -> DepthState {
        // Depth requires an explicit snapshot
        DepthState::default()
    }

    fn reset(&mut self) {}
}

impl DepthPulse {
    /// Explicit depth computation (used by execution gate).
    pub fn compute_with_snapshot(&self, snap: &PoolSnapshot) -> DepthState {
        if snap.reserve0 < self.min_liquidity || snap.reserve1 < self.min_liquidity {
            return DepthState {
                ts_ms: snap.ts_ms,
                validity: false,
                ..Default::default()
            };
        }

        let x = snap.reserve0 as f64;
        let y = snap.reserve1 as f64;
        let fee_factor = 1.0 - ((snap.lp_fee + snap.protocol_fee) as f64 / 10_000.0);
        let mid = y / x;

        let mut low = 0.0;
        let mut high = x * 0.3;
        let mut best_dx = 0.0;
        let mut best_slippage = 0.0;

        for _ in 0..self.max_iterations {
            let dx = (low + high) / 2.0;
            if dx < 1.0 {
                break;
            }

            let dx_eff = dx * fee_factor;
            let dy = (y * dx_eff) / (x + dx_eff);
            let exec_price = dy / dx;
            let slippage = ((mid - exec_price) / mid * 10_000.0).max(0.0);

            if slippage <= self.max_slippage_bps {
                best_dx = dx;
                best_slippage = slippage;
                low = dx;
            } else {
                high = dx;
            }
        }

        DepthState {
            max_dx: best_dx as u128,
            slippage_bps: best_slippage,
            ts_ms: snap.ts_ms,
            validity: best_dx > 0.0 && best_slippage.is_finite(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(r0: u128, r1: u128) -> PoolSnapshot {
        PoolSnapshot {
            reserve0: r0,
            reserve1: r1,
            lp_fee: 20,
            protocol_fee: 10,
            ts_ms: 1_000,
        }
    }

    #[test]
    fn liquidity_gate_blocks_ghost_pools() {
        let d = DepthPulse::new(50.0);
        let s = snapshot(1, 1_000);

        let res = d.compute_with_snapshot(&s);
        assert!(!res.validity);
    }

    #[test]
    fn depth_respects_configured_slippage_limit() {
        let d = DepthPulse::new(50.0);
        let s = snapshot(1_000_000, 1_000_000);

        let res = d.compute_with_snapshot(&s);
        assert!(res.validity);
        assert!(res.slippage_bps <= 50.0);
    }

    #[test]
    fn higher_slippage_budget_allows_larger_depth() {
        let low_tol = DepthPulse::new(20.0);
        let high_tol = DepthPulse::new(200.0);
        let s = snapshot(1_000_000, 1_000_000);

        let low = low_tol.compute_with_snapshot(&s);
        let high = high_tol.compute_with_snapshot(&s);

        assert!(high.validity);
        assert!(high.max_dx > low.max_dx);
    }

    #[test]
    fn depth_is_monotonic_in_slippage_budget() {
        let s = snapshot(1_000_000, 1_000_000);

        let d1 = DepthPulse::new(30.0).compute_with_snapshot(&s).max_dx;
        let d2 = DepthPulse::new(60.0).compute_with_snapshot(&s).max_dx;
        let d3 = DepthPulse::new(120.0).compute_with_snapshot(&s).max_dx;

        assert!(d1 <= d2);
        assert!(d2 <= d3);
    }

    #[test]
    fn zero_slippage_budget_yields_zero_depth() {
        let d = DepthPulse::new(0.0);
        let s = snapshot(1_000_000, 1_000_000);

        let res = d.compute_with_snapshot(&s);
        assert!(!res.validity || res.max_dx == 0);
    }

    #[test]
    fn numeric_stability_extreme_reserves() {
        let d = DepthPulse::new(100.0);
        let s = snapshot(u128::MAX / 2, u128::MAX / 3);

        let res = d.compute_with_snapshot(&s);
        assert!(res.slippage_bps.is_finite());
    }

    #[test]
    fn depth_timestamp_matches_snapshot() {
        let d = DepthPulse::new(50.0);
        let s = PoolSnapshot {
            reserve0: 1_000_000,
            reserve1: 1_000_000,
            lp_fee: 20,
            protocol_fee: 10,
            ts_ms: 42,
        };

        let res = d.compute_with_snapshot(&s);
        assert_eq!(res.ts_ms, 42);
    }
}

use tracing::{Span, debug, field, instrument};

use crate::market_view::types::MarketMetricsView;
use crate::planner::types::{PlannedAllocation, SizingPolicy, UserIntent};

/// Convert scheduler intents into concrete, bounded per-user allocations for the current tick.
///
/// Applies:
/// - global tick budget (market depth × utilization, capped by hard limit)
/// - per-user cap
/// - minimum chunk threshold (drops dust)
/// - chunk splitting within [min_chunk_bid, max_chunk_bid]
///
/// Notes:
/// - `desired_chunks` is treated as a hint and is currently ignored.
/// - Order of `intents` matters (first-fit into the remaining global budget).
#[instrument(
    target = "planner",
    skip(intents),
    fields(
        intent_count = intents.len(),
        total_bid_allocated = field::Empty,
        market_depth = market.depth_now_in
    )
)]
pub fn derive_execution_plan(
    market: &MarketMetricsView,
    intents: &[UserIntent],
    policy: &SizingPolicy,
) -> Vec<PlannedAllocation> {
    if intents.is_empty() {
        return vec![];
    }

    // Global budget derived from available depth and utilization, bounded by hard cap.
    let depth_cap = (market.depth_now_in as f64 * policy.depth_utilization)
        .floor()
        .max(0.0) as u128;

    let total_cap = depth_cap.min(policy.hard_max_total_bid_per_tick);
    let mut remaining_budget = total_cap;

    debug!(
        depth_cap,
        hard_limit = policy.hard_max_total_bid_per_tick,
        total_cap,
        "calculated global tick budget"
    );

    // If we can't form even one valid chunk, do nothing this tick.
    if total_cap < policy.min_chunk_bid {
        debug!(
            min_required = policy.min_chunk_bid,
            "tick budget too low to form even one chunk; skipping tick"
        );
        return vec![];
    }

    let mut out = Vec::new();

    for u in intents {
        // Stop early once remaining budget cannot produce a valid chunk.
        if remaining_budget < policy.min_chunk_bid {
            debug!("global budget exhausted for this tick");
            break;
        }

        // Scheduler-requested volume for this user this tick.
        let want = u.desired_bid;

        // Ignore requests smaller than minimum executable size.
        if want < policy.min_chunk_bid {
            debug!(session_id = %u.session_id, want, "skipping intent: below min_chunk_bid");
            continue;
        }

        // Cap by per-user limit and remaining global budget.
        let allow = want
            .min(policy.max_bid_per_user_per_tick)
            .min(remaining_budget);

        // Skip if allocation can't produce at least one valid chunk.
        if allow < policy.min_chunk_bid {
            debug!(
                session_id = %u.session_id,
                allow,
                "skipping intent: allowance below min_chunk_bid after capping"
            );
            continue;
        }

        // Split into safe atomic chunks; any remainder < min_chunk is dropped.
        let chunks = split_into_chunks(allow, policy.max_chunk_bid, policy.min_chunk_bid);

        if chunks.is_empty() {
            continue;
        }

        let sum: u128 = chunks.iter().copied().sum();

        out.push(PlannedAllocation {
            session_id: u.session_id,
            total_bid: sum,
            chunks,
        });

        // Consume global budget by the actual allocated amount.
        remaining_budget = remaining_budget.saturating_sub(sum);
    }

    let total_allocated = total_cap.saturating_sub(remaining_budget);

    // Update span metadata for structured logging
    Span::current().record("total_bid_allocated", total_allocated);

    debug!(
        allocations_count = out.len(),
        total_allocated, "execution plan derived"
    );

    out
}

/// Split `total` into chunks within [min_chunk, max_chunk].
/// Any remainder smaller than `min_chunk` is dropped (to avoid dust).
fn split_into_chunks(total: u128, max_chunk: u128, min_chunk: u128) -> Vec<u128> {
    if total < min_chunk {
        return vec![];
    }

    let mut out = Vec::new();
    let mut rem = total;

    while rem >= min_chunk {
        let c = rem.min(max_chunk);
        out.push(c);
        rem -= c;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn market_with_depth(depth_now_in: u128) -> MarketMetricsView {
        MarketMetricsView {
            ts_ms: 0,
            spread_bps: 0.0,
            trend_drop_bps: 0.0,
            slippage_bps: 0.0,
            depth_now_in,
        }
    }

    fn policy(
        hard_max_total: u128,
        depth_util: f64,
        per_user_max: u128,
        max_chunk: u128,
        min_chunk: u128,
    ) -> SizingPolicy {
        SizingPolicy {
            hard_max_total_bid_per_tick: hard_max_total,
            depth_utilization: depth_util,
            max_bid_per_user_per_tick: per_user_max,
            max_chunk_bid: max_chunk,
            min_chunk_bid: min_chunk,
        }
    }

    fn intent(bid: u128) -> UserIntent {
        UserIntent {
            session_id: Uuid::new_v4(),
            desired_bid: bid,
            desired_chunks: 0,
        }
    }

    #[test]
    fn empty_intents_returns_empty() {
        let market = market_with_depth(1_000_000);
        let p = policy(1_000_000, 1.0, 1_000_000, 100_000, 10_000);

        let out = derive_execution_plan(&market, &[], &p);
        assert!(out.is_empty());
    }

    #[test]
    fn global_budget_below_min_chunk_returns_empty() {
        // depth_cap = 50% of 10_000 = 5_000, min_chunk = 10_000 -> cannot form one chunk
        let market = market_with_depth(10_000);
        let p = policy(1_000_000, 0.5, 1_000_000, 100_000, 10_000);

        let out = derive_execution_plan(&market, &[intent(100_000)], &p);
        assert!(out.is_empty());
    }

    #[test]
    fn want_below_min_chunk_is_skipped() {
        let market = market_with_depth(1_000_000);
        let p = policy(1_000_000, 1.0, 1_000_000, 100_000, 10_000);

        let out = derive_execution_plan(&market, &[intent(9_999)], &p);
        assert!(out.is_empty());
    }

    #[test]
    fn per_user_cap_applies() {
        // Total cap is large; per-user cap should limit allocation.
        let market = market_with_depth(10_000_000);
        let p = policy(10_000_000, 1.0, 200_000, 150_000, 10_000);

        let out = derive_execution_plan(&market, &[intent(1_000_000)], &p);
        assert_eq!(out.len(), 1);

        let a = &out[0];
        assert_eq!(a.total_bid, 200_000);
        assert_eq!(a.chunks.iter().copied().sum::<u128>(), 200_000);
        assert!(a.chunks.iter().all(|&c| (10_000..=150_000).contains(&c)));
    }

    #[test]
    fn global_budget_caps_total_across_users() {
        // depth_cap = 1.0 * 250_000 = 250_000, hard max bigger -> total_cap=250_000
        let market = market_with_depth(250_000);
        let p = policy(1_000_000, 1.0, 1_000_000, 200_000, 10_000);

        let i1 = intent(200_000);
        let i2 = intent(200_000);

        let out = derive_execution_plan(&market, &[i1, i2], &p);

        // First user can get 200k; remaining budget 50k; second user gets 50k (>= min_chunk).
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].total_bid, 200_000);
        assert_eq!(out[1].total_bid, 50_000);

        let total: u128 = out.iter().map(|a| a.total_bid).sum();
        assert_eq!(total, 250_000);
    }

    #[test]
    fn chunk_splitting_respects_bounds_and_drops_dust() {
        // allow = 250_000, max_chunk=100_000, min_chunk=60_000
        // chunks: 100k, 100k, remaining 50k (<60k) dropped => total 200k
        let market = market_with_depth(1_000_000);
        let p = policy(1_000_000, 1.0, 1_000_000, 100_000, 60_000);

        let out = derive_execution_plan(&market, &[intent(250_000)], &p);
        assert_eq!(out.len(), 1);

        let a = &out[0];
        assert_eq!(a.chunks, vec![100_000, 100_000]); // deterministic by algorithm
        assert_eq!(a.total_bid, 200_000);
        assert!(a.chunks.iter().all(|&c| (60_000..=100_000).contains(&c)));
    }

    #[test]
    fn stops_when_remaining_budget_cannot_form_chunk() {
        // total_cap = 150_000, min_chunk=100_000
        // user1 takes 100k, remaining 50k (< min_chunk) -> break, user2 not allocated
        let market = market_with_depth(150_000);
        let p = policy(1_000_000, 1.0, 1_000_000, 200_000, 100_000);

        let i1 = intent(100_000);
        let i2 = intent(100_000);

        let out = derive_execution_plan(&market, &[i1, i2], &p);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].total_bid, 100_000);
    }

    #[test]
    fn allocation_sum_equals_total_bid_and_is_nonempty() {
        let market = market_with_depth(1_000_000);
        let p = policy(1_000_000, 1.0, 1_000_000, 120_000, 10_000);

        let out = derive_execution_plan(&market, &[intent(333_333)], &p);
        assert_eq!(out.len(), 1);

        let a = &out[0];
        assert!(!a.chunks.is_empty());
        assert_eq!(a.total_bid, a.chunks.iter().copied().sum::<u128>());
        assert!(a.chunks.iter().all(|&c| (10_000..=120_000).contains(&c)));
    }

    #[test]
    fn budget_exactly_at_threshold() {
        let market = market_with_depth(20_001);
        let p = policy(1_000_000, 1.0, 1_000_000, 100_000, 10_000);

        let out = derive_execution_plan(&market, &[intent(10_001), intent(10_000)], &p);

        assert_eq!(out.len(), 2);

        // User 1: allow=10,001 -> chunks=[10,001]
        assert_eq!(
            out[0].total_bid, 10_001,
            "User 1 should receive full allowed amount"
        );

        // Remaining budget = 20,001 - 10,001 = 10,000
        // User 2: allow=10,000 -> chunks=[10,000]
        assert_eq!(
            out[1].total_bid, 10_000,
            "User 2 should receive full remaining chunk"
        );
    }

    #[test]
    fn hard_limit_overrides_market_depth() {
        let market = market_with_depth(1_000_000); // 1M depth
        let p = policy(50_000, 1.0, 1_000_000, 100_000, 10_000); // Hard cap 50k

        let out = derive_execution_plan(&market, &[intent(100_000)], &p);
        assert_eq!(out[0].total_bid, 50_000);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]
        #[test]
        fn test_execution_plan_invariants(
            market_depth in 0..=1_000_000_000u128,

            // Policy values: min must be <= max
            min_chunk in 1..=100_000u128,
            max_chunk in 100_001..=1_000_000u128,
            per_user in 1..=1_000_000u128,
            hard_limit in 1..=1_000_000_000u128,
            utilization in 0.0..=1.0f64,

            // Random user intents
            intents in prop::collection::vec(0..=2_000_000u128, 1..20)
        ) {
            let market = MarketMetricsView {
                ts_ms: 0, spread_bps: 0.0, trend_drop_bps: 0.0, slippage_bps: 0.0,
                depth_now_in: market_depth,
            };

            let p = SizingPolicy {
                hard_max_total_bid_per_tick: hard_limit,
                depth_utilization: utilization,
                max_bid_per_user_per_tick: per_user,
                max_chunk_bid: max_chunk,
                min_chunk_bid: min_chunk,
            };

            let user_intents: Vec<UserIntent> = intents.into_iter()
                .map(|bid| UserIntent { session_id: uuid::Uuid::new_v4(), desired_bid: bid, desired_chunks: 0 })
                .collect();

            let plan = derive_execution_plan(&market, &user_intents, &p);

            // --- INVARIANT 1: Total allocated never exceeds global budget ---
            let total_allocated: u128 = plan.iter().map(|a| a.total_bid).sum();
            let depth_cap = (market_depth as f64 * utilization).floor() as u128;
            let expected_global_cap = depth_cap.min(hard_limit);

            assert!(total_allocated <= expected_global_cap,
                "Over-allocated! {} > cap {}", total_allocated, expected_global_cap);

            // --- INVARIANT 2: Every chunk is within [min, max] ---
            for alloc in &plan {
                for &chunk in &alloc.chunks {
                    assert!(chunk >= min_chunk, "Chunk {} below min {}", chunk, min_chunk);
                    assert!(chunk <= max_chunk, "Chunk {} exceeds max {}", chunk, max_chunk);
                }

                // --- INVARIANT 3: total_bid is the sum of chunks ---
                assert_eq!(alloc.total_bid, alloc.chunks.iter().sum::<u128>());

                // --- INVARIANT 4: No user exceeds the per-user cap ---
                assert!(alloc.total_bid <= per_user);
            }
        }
    }
}

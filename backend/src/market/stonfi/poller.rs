//! STON.fi Market Poller
//!
//! Periodically polls the STON.fi API for a single pool,
//! converts raw API data into a `PoolSnapshot`, feeds it
//! into the `StonfiMarketService`, and publishes an
//! immutable `MarketMetricsView` into the MarketViewStore.

use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{info, warn};

use crate::market::market_view_store::MarketViewStore;
use crate::market::stonfi::client::StonfiClient;
use crate::market::stonfi::market_service::StonfiMarketService;
use crate::market::types::MarketMetricsView;
use crate::market::types::PoolSnapshot;

/// Runs a market poller loop for a single STON.fi pool.
///
/// Data flow:
/// Pool → Poller → MarketService → MarketViewStore
pub async fn run_stonfi_market_poller(
    pair_id: String,
    pool_address: String, // STON.fi pool address
    poll_every: Duration,
    client: StonfiClient,
    mut market: StonfiMarketService,
    store: MarketViewStore,
) -> Result<()> {
    let mut ticker = interval(poll_every);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    info!(
        pair = %pair_id,
        pool = %pool_address,
        every_ms = poll_every.as_millis(),
        "stonfi market poller started"
    );

    loop {
        ticker.tick().await;

        let resp = client
            .fetch_pool(&pool_address)
            .await
            .with_context(|| format!("failed to fetch pool {}", pool_address))?;

        let reserve0: u128 = resp.reserve0.parse().context("parse reserve0")?;
        let reserve1: u128 = resp.reserve1.parse().context("parse reserve1")?;
        let lp_fee: u32 = resp.lp_fee.parse().context("parse lp_fee")?;
        let protocol_fee: u32 = resp.protocol_fee.parse().context("parse protocol_fee")?;

        let snapshot = PoolSnapshot {
            reserve0,
            reserve1,
            lp_fee,
            protocol_fee,
            ts_ms: crate::time::now_ms(),
        };

        let metrics = market.tick(snapshot);

        let view = MarketMetricsView {
            ts_ms: metrics.ts_ms,
            spread_bps: metrics.spread_bps,
            trend_drop_bps: metrics.trend_drop_bps,
            slippage_bps: 0.0, // computed at execution time
            depth_now_in: 0,   // deferred to later version
        };

        store.set(&pair_id, view.clone()).await;

        if metrics.validity {
            info!(
                pair = %pair_id,
                ts_ms = view.ts_ms,
                spread_bps = view.spread_bps,
                trend_drop_bps = view.trend_drop_bps,
                "market metrics published"
            );
        } else {
            warn!(
                pair = %pair_id,
                ts_ms = view.ts_ms,
                spread_bps = view.spread_bps,
                trend_drop_bps = view.trend_drop_bps,
                "market metrics invalid (gated)"
            );
        }
    }
}

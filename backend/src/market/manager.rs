//! Market subsystem entry-point.
//!
//! Responsible for spawning and managing market pollers
//! (one poller per trading pair).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::info;

use crate::market::market_view_store::MarketViewStore;
use crate::market::stonfi::client::StonfiClient;
use crate::market::stonfi::market_service::StonfiMarketService;
use crate::market::stonfi::poller::run_stonfi_market_poller;

/// MarketManager controls lifecycle of market pollers.
///
/// Guarantees:
/// - One poller per pair
/// - Safe concurrent subscription
#[derive(Clone)]
pub struct MarketManager {
    client: StonfiClient,
    store: MarketViewStore,
    poll_every: Duration,

    // Tracks active pollers to prevent duplicates
    active_pairs: Arc<Mutex<HashSet<String>>>,
}

impl MarketManager {
    /// Create a new market manager.
    pub fn new(client: StonfiClient, store: MarketViewStore, poll_every: Duration) -> Self {
        Self {
            client,
            store,
            poll_every,
            active_pairs: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Subscribe to market data for a STON.fi pair.
    ///
    /// Spawns a background poller task if not already active.
    ///
    /// # Arguments
    /// - `pair_id`
    /// - `pool_address` → STON.fi pool address
    /// - `window_size`  → rolling window length
    /// - `min_warmup_ms`→ trend warm-up duration
    pub async fn subscribe_stonfi_pair(
        &self,
        pair_id: String,
        pool_address: String,
        window_size: usize,
        min_warmup_ms: u64,
    ) -> Result<JoinHandle<Result<()>>> {
        {
            let mut g = self.active_pairs.lock().await;
            if g.contains(&pair_id) {
                return Err(anyhow!("market poller already running for {}", pair_id));
            }
            g.insert(pair_id.clone());
        }

        info!(
            pair = %pair_id,
            pool = %pool_address,
            "starting stonfi market poller"
        );

        let client = self.client.clone();
        let store = self.store.clone();
        let poll_every = self.poll_every;

        let market = StonfiMarketService::new(window_size, min_warmup_ms);

        let handle = tokio::spawn(async move {
            run_stonfi_market_poller(pair_id, pool_address, poll_every, client, market, store).await
        });

        Ok(handle)
    }
}

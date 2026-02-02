pub mod types;

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::market_view::types::MarketMetricsView;

/// In-memory store of the latest market snapshot per trading pair.
/// Used by scheduler (Gate A) and executor (Gate B) for constraint checks.
#[derive(Clone, Default)]
pub struct MarketViewStore {
    inner: Arc<RwLock<HashMap<String, MarketMetricsView>>>,
}

impl MarketViewStore {
    /// Create an empty market view store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the latest snapshot for a trading pair.
    /// Last write wins; snapshots are treated as advisory only.
    pub async fn set(&self, pair_id: &str, v: MarketMetricsView) {
        let mut g = self.inner.write().await;
        g.insert(pair_id.to_string(), v);
    }

    /// Fetch the latest snapshot for a trading pair, if available.
    pub async fn get(&self, pair_id: &str) -> Option<MarketMetricsView> {
        let g = self.inner.read().await;
        g.get(pair_id).cloned()
    }
}

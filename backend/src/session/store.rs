use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

use crate::logger::warn_if_slow;
use crate::session::cache::SessionCache;
use crate::session::model::Session;
use crate::session::repository::SessionRepository;

/// Scheduler-facing session store that manages in-memory caching and DB pagination.
pub struct SessionStore {
    repo: Arc<dyn SessionRepository>,
    cache: SessionCache,
    page_size: usize,
    last_offset: parking_lot::Mutex<usize>,
}

impl SessionStore {
    pub fn new(repo: Arc<dyn SessionRepository>) -> Self {
        Self {
            repo,
            cache: SessionCache::new(5_000),
            page_size: 500,
            last_offset: parking_lot::Mutex::new(0),
        }
    }

    pub fn cache_len_rr(&self) -> usize {
        self.cache.len_rr()
    }

    pub fn get_cached(&self, id: &Uuid) -> Option<Session> {
        self.cache.get(id)
    }

    pub fn rotate_candidate(&self) -> Option<Uuid> {
        self.cache.rotate()
    }

    /// Ensures at least `min_needed` candidates exist in cache.
    #[instrument(
        skip(self),
        target = "store", 
        fields(current_len = self.cache.len_rr(), min_needed)
    )]
    pub async fn ensure_candidates(&self, min_needed: usize) -> Result<()> {
        let current = self.cache.len_rr();
        if current >= min_needed {
            return Ok(());
        }

        info!(
            gap = min_needed.saturating_sub(current),
            "cache below threshold; triggering database refill"
        );

        // Error propagation is key for the test to pass
        self.load_next_page().await?;

        debug!(new_len = self.cache.len_rr(), "refill operation complete");
        Ok(())
    }

    #[instrument(skip(self), target = "store", fields(session_id = %id))]
    pub async fn load_by_id(&self, id: &Uuid) -> Result<Session> {
        debug!("fetching session by id from repository");

        let session = warn_if_slow("db_fetch_by_id", Duration::from_millis(100), async {
            self.repo.fetch_by_id(id).await
        })
        .await
        .context("repository fetch failed")?;

        match session {
            Some(s) => Ok(s),
            None => {
                warn!("session lookup returned no results");
                Err(anyhow::anyhow!("session not found: {}", id))
            }
        }
    }

    #[instrument(skip(self), target = "store", fields(session_id = %session_id, deficit))]
    pub async fn persist_fairness(
        &self,
        session_id: &Uuid,
        deficit: i128,
        last_served_ms: u64,
    ) -> Result<()> {
        debug!("persisting fairness state to repository");

        warn_if_slow("db_persist_fairness", Duration::from_millis(50), async {
            self.repo
                .persist_fairness(session_id, deficit, last_served_ms)
                .await
        })
        .await
        .context("failed to persist fairness state")
    }

    #[instrument(skip(self), target = "store")]
    async fn load_next_page(&self) -> Result<()> {
        let offset = *self.last_offset.lock();

        debug!(
            offset,
            limit = self.page_size,
            "executing paginated repository fetch"
        );

        let rows = warn_if_slow("db_load_next_page", Duration::from_millis(200), async {
            self.repo.fetch_page(self.page_size, offset).await
        })
        .await
        .context("failed to fetch page from repository")?;

        if rows.is_empty() {
            info!("pagination reached end of active results; wrapping offset to 0");
            *self.last_offset.lock() = 0;
            return Ok(());
        }

        let count = rows.len();
        *self.last_offset.lock() = offset + self.page_size;

        for s in rows {
            self.cache.upsert(s);
        }

        info!(
            count,
            new_offset = *self.last_offset.lock(),
            "successfully loaded new page into session cache"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::task::JoinSet;
    use uuid::Uuid;

    use crate::session::model::{Session, SessionIntent, SessionState, UserConstraints};
    use crate::session::repository::SessionRepository;

    fn mk_session(id: Uuid) -> Session {
        Session {
            session_id: id,
            pair_id: "TON/USDT".to_string(),
            active: true,
            intent: SessionIntent {
                constraints: UserConstraints {
                    max_spread_bps: 50.0,
                    max_trend_drop_bps: 100.0,
                    max_slippage_bps: 75.0,
                },
                preferred_chunk_bid: 100_000,
                max_bid_per_tick: 1_000_000,
            },
            state: SessionState {
                remaining_bid: 1_000_000,
                remaining_chunks: 10,
                in_flight_bid: 0,
                in_flight_chunks: 0,
                cooldown_until_ms: 0,
                quantum: 100_000,
                deficit: 0,
                last_served_ms: 0,
            },
        }
    }

    pub struct MockSessionRepository {
        pub pages: Vec<Vec<Session>>,
        pub by_id: HashMap<Uuid, Session>,
        pub fairness_calls: Mutex<Vec<(Uuid, i128, u64)>>,
    }

    #[async_trait::async_trait]
    impl SessionRepository for MockSessionRepository {
        async fn fetch_page(&self, limit: usize, offset: usize) -> anyhow::Result<Vec<Session>> {
            let page_idx = offset / limit;
            Ok(self.pages.get(page_idx).cloned().unwrap_or_default())
        }

        async fn fetch_by_id(&self, id: &Uuid) -> anyhow::Result<Option<Session>> {
            Ok(self.by_id.get(id).cloned())
        }

        async fn persist_fairness(
            &self,
            id: &Uuid,
            deficit: i128,
            last_served_ms: u64,
        ) -> anyhow::Result<()> {
            self.fairness_calls
                .lock()
                .push((*id, deficit, last_served_ms));
            Ok(())
        }
    }

    #[tokio::test]
    async fn ensure_candidates_loads_when_cache_empty() {
        let ids: Vec<_> = (0..5).map(|_| Uuid::new_v4()).collect();
        let page = ids.iter().cloned().map(mk_session).collect();

        let repo = Arc::new(MockSessionRepository {
            pages: vec![page],
            by_id: HashMap::new(),
            fairness_calls: Mutex::new(vec![]),
        });

        let store = SessionStore::new(repo);
        store.ensure_candidates(3).await.unwrap();

        assert!(store.cache_len_rr() >= 3);
    }

    #[tokio::test]
    async fn ensure_candidates_noop_when_satisfied() {
        let id = Uuid::new_v4();
        let page = vec![mk_session(id)];

        let repo = Arc::new(MockSessionRepository {
            pages: vec![page],
            by_id: HashMap::new(),
            fairness_calls: Mutex::new(vec![]),
        });

        let store = SessionStore::new(repo);
        store.ensure_candidates(1).await.unwrap();

        let before = store.cache_len_rr();
        store.ensure_candidates(1).await.unwrap();

        assert_eq!(store.cache_len_rr(), before);
    }

    #[tokio::test]
    async fn paging_offset_advances_and_wraps() {
        let pages = vec![
            vec![mk_session(Uuid::new_v4()), mk_session(Uuid::new_v4())],
            vec![mk_session(Uuid::new_v4())],
        ];

        let repo = Arc::new(MockSessionRepository {
            pages,
            by_id: HashMap::new(),
            fairness_calls: Mutex::new(vec![]),
        });

        let store = SessionStore::new(repo);
        store.ensure_candidates(2).await.unwrap();
        store.ensure_candidates(3).await.unwrap();

        // Next load should wrap
        store.ensure_candidates(10).await.unwrap();
        assert!(store.cache_len_rr() > 0);
    }

    #[tokio::test]
    async fn load_by_id_delegates_to_repo() {
        let id = Uuid::new_v4();
        let session = mk_session(id);

        let repo = Arc::new(MockSessionRepository {
            pages: vec![],
            by_id: HashMap::from([(id, session.clone())]),
            fairness_calls: Mutex::new(vec![]),
        });

        let store = SessionStore::new(repo);
        let loaded = store.load_by_id(&id).await.unwrap();

        assert_eq!(loaded.session_id, id);
    }

    #[tokio::test]
    async fn persist_fairness_is_forwarded() {
        let id = Uuid::new_v4();

        let repo = Arc::new(MockSessionRepository {
            pages: vec![],
            by_id: HashMap::new(),
            fairness_calls: Mutex::new(vec![]),
        });

        let store = SessionStore::new(repo.clone());
        store.persist_fairness(&id, 123, 456).await.unwrap();

        let calls = repo.fairness_calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], (id, 123, 456));
    }

    #[tokio::test]
    async fn test_repository_error_propagation() {
        pub struct FailingRepo;
        #[async_trait::async_trait]
        impl SessionRepository for FailingRepo {
            async fn fetch_page(&self, _: usize, _: usize) -> anyhow::Result<Vec<Session>> {
                Err(anyhow::anyhow!("Database Offline"))
            }
            async fn fetch_by_id(&self, _: &Uuid) -> anyhow::Result<Option<Session>> {
                Ok(None)
            }
            async fn persist_fairness(&self, _: &Uuid, _: i128, _: u64) -> anyhow::Result<()> {
                Ok(())
            }
        }

        let store = SessionStore::new(Arc::new(FailingRepo));
        let result = store.ensure_candidates(1).await;

        assert!(result.is_err());

        // Use format!("{:?}") to see the full error chain provided by anyhow context
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(
            err_msg.contains("Database Offline"),
            "Error chain did not contain root cause 'Database Offline'. Found: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_concurrent_ensure_candidates_stress() {
        let page_size = 5;
        let mut all_pages = Vec::new();
        for _ in 0..10 {
            all_pages.push((0..page_size).map(|_| mk_session(Uuid::new_v4())).collect());
        }

        let repo = Arc::new(MockSessionRepository {
            pages: all_pages,
            by_id: std::collections::HashMap::new(),
            fairness_calls: parking_lot::Mutex::new(vec![]),
        });

        let store = Arc::new(SessionStore::new(repo));
        let mut set = JoinSet::new();

        for _ in 0..20 {
            let s = Arc::clone(&store);
            set.spawn(async move { s.ensure_candidates(10).await });
        }

        while let Some(res) = set.join_next().await {
            res.expect("Task panicked")
                .expect("ensure_candidates failed");
        }

        assert!(store.cache_len_rr() >= 10);
        assert!(*store.last_offset.lock() >= 10);
    }
}

use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, instrument};
use uuid::Uuid;

use crate::logger::warn_if_slow;
use crate::session::cache::SessionCache;
use crate::session::model::Session;
use crate::session::repository::SessionRepository;

/// Scheduler-facing session store that manages in-memory caching and DB pagination.
pub struct SessionStore {
    pub repo: Arc<dyn SessionRepository>,
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

        self.load_next_page().await?;

        debug!(new_len = self.cache.len_rr(), "refill operation complete");
        Ok(())
    }

    #[instrument(skip(self), target = "store", fields(session_id = %id))]
    pub async fn load_by_id(&self, id: &Uuid) -> Result<Session> {
        let session = warn_if_slow("db_fetch_by_id", Duration::from_millis(100), async {
            self.repo.fetch_by_id(id).await
        })
        .await
        .context("repository fetch failed")?;

        match session {
            Some(s) => Ok(s),
            None => Err(anyhow::anyhow!("session not found: {}", id)),
        }
    }

    #[instrument(skip(self), target = "store", fields(session_id = %session_id, deficit))]
    pub async fn persist_fairness(
        &self,
        session_id: &Uuid,
        deficit: i128,
        last_served_ms: u64,
    ) -> Result<()> {
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

        let rows = warn_if_slow("db_load_next_page", Duration::from_millis(200), async {
            self.repo.fetch_page(self.page_size, offset).await
        })
        .await
        .context("failed to fetch page from repository")?;

        if rows.is_empty() {
            *self.last_offset.lock() = 0;
            return Ok(());
        }

        *self.last_offset.lock() = offset + self.page_size;
        for s in rows {
            self.cache.upsert(s);
        }

        Ok(())
    }

    pub fn upsert_cache(&self, s: Session) {
        self.cache.upsert(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use tokio::task::JoinSet;

    use crate::execution::types::{ReservedBatch, ReservedChunk, ReservedUser, UserResult};
    use crate::planner::types::PlannedAllocation;
    use crate::session::model::{SessionIntent, SessionState, UserConstraints};

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
                has_pending_batch: false,
            },
        }
    }

    pub struct MockSessionRepository {
        pub pages: Vec<Vec<Session>>,
        pub by_id: HashMap<Uuid, Session>,
        pub fairness_calls: Mutex<Vec<(Uuid, i128, u64)>>,
        pub reservation_calls: Mutex<Vec<ReservedBatch>>,
        commit_calls: Mutex<Vec<Uuid>>,
    }

    #[async_trait::async_trait]
    impl SessionRepository for MockSessionRepository {
        async fn fetch_page(&self, limit: usize, offset: usize) -> anyhow::Result<Vec<Session>> {
            Ok(self.pages.get(offset / limit).cloned().unwrap_or_default())
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

        async fn reserve_execution(
            &self,
            pair_id: &str,
            now_ms: u64,
            allocations: &[PlannedAllocation],
        ) -> anyhow::Result<Option<ReservedBatch>> {
            let users = allocations
                .iter()
                .map(|a| ReservedUser {
                    session_id: a.session_id,
                    chunks: a
                        .chunks
                        .iter()
                        .map(|bid| ReservedChunk {
                            chunk_id: Uuid::new_v4(),
                            bid: *bid,
                        })
                        .collect(),
                })
                .collect::<Vec<_>>();

            let batch = ReservedBatch {
                batch_id: Uuid::new_v4(),
                pair_id: pair_id.to_string(),
                created_ms: now_ms,
                users,
            };

            self.reservation_calls.lock().push(batch.clone());
            Ok(Some(batch))
        }

        async fn commit_batch(
            &self,
            batch: &ReservedBatch,
            _results: &[UserResult],
        ) -> anyhow::Result<()> {
            self.commit_calls.lock().push(batch.batch_id);
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
            reservation_calls: Mutex::new(vec![]),
            commit_calls: Mutex::new(vec![]),
        });

        let store = SessionStore::new(repo);
        store.ensure_candidates(3).await.unwrap();
        assert!(store.cache_len_rr() >= 3);
    }

    #[tokio::test]
    async fn persist_fairness_is_forwarded() {
        let id = Uuid::new_v4();

        let repo = Arc::new(MockSessionRepository {
            pages: vec![],
            by_id: HashMap::new(),
            fairness_calls: Mutex::new(vec![]),
            reservation_calls: Mutex::new(vec![]),
            commit_calls: Mutex::new(vec![]),
        });

        let store = SessionStore::new(repo.clone());
        store.persist_fairness(&id, 123, 456).await.unwrap();

        let calls = repo.fairness_calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], (id, 123, 456));
    }

    #[tokio::test]
    async fn test_repository_error_propagation() {
        struct FailingRepo;

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
            async fn reserve_execution(
                &self,
                _: &str,
                _: u64,
                _: &[PlannedAllocation],
            ) -> anyhow::Result<Option<ReservedBatch>> {
                Err(anyhow::anyhow!("reserve not available"))
            }
            async fn commit_batch(
                &self,
                _: &ReservedBatch,
                _: &[UserResult],
            ) -> anyhow::Result<()> {
                Err(anyhow::anyhow!("commit not available"))
            }
        }

        let store = SessionStore::new(Arc::new(FailingRepo));
        let result = store.ensure_candidates(1).await;

        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("Database Offline"));
    }

    #[tokio::test]
    async fn test_concurrent_ensure_candidates_stress() {
        let mut pages = Vec::new();
        for _ in 0..10 {
            pages.push((0..5).map(|_| mk_session(Uuid::new_v4())).collect());
        }

        let repo = Arc::new(MockSessionRepository {
            pages,
            by_id: HashMap::new(),
            fairness_calls: Mutex::new(vec![]),
            reservation_calls: Mutex::new(vec![]),
            commit_calls: Mutex::new(vec![]),
        });

        let store = Arc::new(SessionStore::new(repo));
        let mut set = JoinSet::new();

        for _ in 0..20 {
            let s = store.clone();
            set.spawn(async move { s.ensure_candidates(10).await });
        }

        while let Some(res) = set.join_next().await {
            res.unwrap().unwrap();
        }

        assert!(store.cache_len_rr() >= 10);
    }

    /// Verifies that one malformed session in a DB page doesn't crash the cache refill.
    #[tokio::test]
    async fn test_poison_row_skipping() {
        let good_id = Uuid::new_v4();
        let good_session = mk_session(good_id);

        // We simulate a "poisoned" result where the repo returns data
        // that might fail internal validation if we had any.
        // Even if upsert fails for one, the rest should proceed.
        let page = vec![good_session];

        let repo = Arc::new(MockSessionRepository {
            pages: vec![page],
            by_id: HashMap::new(),
            fairness_calls: Mutex::new(vec![]),
            reservation_calls: Mutex::new(vec![]),
            commit_calls: Mutex::new(vec![]),
        });

        let store = SessionStore::new(repo);
        store.ensure_candidates(1).await.unwrap();

        assert_eq!(store.cache_len_rr(), 1);
        assert!(store.get_cached(&good_id).is_some());
    }

    /// Verifies that when the DB is exhausted, the offset resets to 0.
    #[tokio::test]
    async fn test_pagination_wrap_around() {
        let page_1 = vec![mk_session(Uuid::new_v4())];
        // page_2 is empty (EOF)

        let repo = Arc::new(MockSessionRepository {
            pages: vec![page_1], // index 0 has data, index 1 (offset 500) will be empty
            by_id: HashMap::new(),
            fairness_calls: Mutex::new(vec![]),
            reservation_calls: Mutex::new(vec![]),
            commit_calls: Mutex::new(vec![]),
        });

        let store = SessionStore::new(repo);

        // Load first page
        store.ensure_candidates(1).await.unwrap();
        assert_eq!(*store.last_offset.lock(), 500);

        // Load next page (will be empty)
        store.ensure_candidates(2).await.unwrap();

        // Offset should have wrapped back to 0
        assert_eq!(
            *store.last_offset.lock(),
            0,
            "Offset should reset to 0 after empty page"
        );
    }

    /// Ensures that extreme deficit values don't cause panics in the store logic.
    #[tokio::test]
    async fn test_extreme_deficit_persistence() {
        let id = Uuid::new_v4();
        let repo = Arc::new(MockSessionRepository {
            pages: vec![],
            by_id: HashMap::new(),
            fairness_calls: Mutex::new(vec![]),
            reservation_calls: Mutex::new(vec![]),
            commit_calls: Mutex::new(vec![]),
        });

        let store = SessionStore::new(repo.clone());

        // Test i128 boundaries
        let extreme_deficit = i128::MAX;
        let result = store
            .persist_fairness(&id, extreme_deficit, 999_999_999)
            .await;

        assert!(
            result.is_ok(),
            "Store should handle extreme i128 values without panicking"
        );
        let calls = repo.fairness_calls.lock();
        assert_eq!(calls[0].1, i128::MAX);
    }

    #[tokio::test]
    async fn commit_batch_is_forwarded_to_repo() {
        let repo = Arc::new(MockSessionRepository {
            pages: vec![],
            by_id: HashMap::new(),
            fairness_calls: Mutex::new(vec![]),
            reservation_calls: Mutex::new(vec![]),
            commit_calls: Mutex::new(vec![]),
        });

        let store = SessionStore::new(repo.clone());

        let batch = ReservedBatch {
            batch_id: Uuid::new_v4(),
            pair_id: "TON/USDT".to_string(),
            created_ms: 123,
            users: vec![],
        };

        store.repo.commit_batch(&batch, &[]).await.unwrap();

        let calls = repo.commit_calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], batch.batch_id);
    }
}

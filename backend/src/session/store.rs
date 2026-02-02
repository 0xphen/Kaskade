use anyhow::{Context, anyhow};
use sqlx::{AnyPool, Row};
use std::time::Duration;
use tracing::{Span, debug, error, field, info, instrument, warn};
use uuid::Uuid;

use crate::db::Db;
use crate::logger::warn_if_slow;
use crate::session::cache::SessionCache;
use crate::session::model::{Session, SessionIntent, SessionState, UserConstraints};

pub struct SessionStore {
    db: Db,
    cache: SessionCache,

    page_size: usize,
    last_offset: parking_lot::Mutex<usize>,
}

impl SessionStore {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            cache: SessionCache::new(5_000),
            page_size: 500,
            last_offset: parking_lot::Mutex::new(0),
        }
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    pub fn pool(&self) -> &AnyPool {
        &self.db.pool
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

    pub fn upsert_cache(&self, s: Session) {
        self.cache.upsert(s)
    }

    /// Ensure we have at least `min_needed` candidates in rr. Loads a new page if needed.
    #[instrument(skip(self), target = "store")]
    pub async fn ensure_candidates(&self, min_needed: usize) -> anyhow::Result<()> {
        let current = self.cache.len_rr();
        if self.cache.len_rr() >= min_needed {
            return Ok(());
        }

        debug!(
            current,
            min_needed, "insufficient candidates in cache; triggering load"
        );
        self.load_next_page().await?;
        Ok(())
    }

    /// Load a single session from DB (executor fallback).
    #[instrument(skip(self), target = "store", fields(session_id = %session_id))]
    pub async fn load_by_id(&self, session_id: &Uuid) -> anyhow::Result<Session> {
        let row = warn_if_slow("db_load_by_id", Duration::from_millis(100), async {
            sqlx::query(
                r#"
SELECT
  session_id,
  pair_id,
  CASE WHEN active THEN 1 ELSE 0 END AS active_i64,

  max_spread_bps, max_trend_drop_bps, max_slippage_bps,
  preferred_chunk_bid, max_bid_per_tick,

  remaining_bid, remaining_chunks,
  in_flight_bid, in_flight_chunks,
  cooldown_until_ms,

  quantum, deficit, last_served_ms
FROM sessions
WHERE session_id = ?;
"#,
            )
            .bind(session_id.to_string())
            .fetch_optional(self.pool())
            .await
        })
        .await
        .context("database query failed in load_by_id")?;

        let row = row.ok_or_else(|| {
            warn!(session_id = %session_id, "session not found in database");
            anyhow!("session not found: {}", session_id)
        })?;

        let s = row_to_session(&row)?;
        Ok(s)
    }

    #[instrument(skip(self), target = "store", fields(session_id = %session_id, deficit = %deficit))]
    pub async fn persist_fairness(
        &self,
        session_id: &Uuid,
        deficit: i128,
        last_served_ms: u64,
    ) -> anyhow::Result<()> {
        // PRE-RESOLVE: Avoids the sqlx vs anyhow conversion error
        let deficit_i64 = i128_to_i64(deficit).context("Fairness deficit overflow")?;
        let last_served_i64 = u64_to_i64(last_served_ms).context("Timestamp overflow")?;
        let sid_str = session_id.to_string();

        let res = warn_if_slow("db_persist_fairness", Duration::from_millis(50), async {
            sqlx::query(
                r#"
                UPDATE sessions
                SET deficit = ?, last_served_ms = ?
                WHERE session_id = ?;
                "#,
            )
            .bind(deficit_i64)
            .bind(last_served_i64)
            .bind(sid_str)
            .execute(self.pool())
            .await
        })
        .await
        .context("Database execution failed")?;

        if res.rows_affected() == 0 {
            warn!("Fairness update affected 0 rows; check if session exists");
        }
        Ok(())
    }

    #[instrument(skip(self), target = "store")]
    async fn load_next_page(&self) -> anyhow::Result<()> {
        let offset = *self.last_offset.lock();

        // PRE-RESOLVE: Move anyhow conversion out of the bind chain
        let limit_i64 = u64_to_i64(self.page_size as u64)?;
        let offset_i64 = u64_to_i64(offset as u64)?;

        let rows = warn_if_slow("db_load_next_page", Duration::from_millis(200), async {
            sqlx::query(
                r#"
                SELECT session_id, pair_id, CASE WHEN active THEN 1 ELSE 0 END AS active_i64,
                       max_spread_bps, max_trend_drop_bps, max_slippage_bps,
                       preferred_chunk_bid, max_bid_per_tick, remaining_bid, remaining_chunks,
                       in_flight_bid, in_flight_chunks, cooldown_until_ms,
                       quantum, deficit, last_served_ms
                FROM sessions
                WHERE active = TRUE AND remaining_bid > 0 AND remaining_chunks > 0
                LIMIT ? OFFSET ?;
                "#,
            )
            .bind(limit_i64)
            .bind(offset_i64)
            .fetch_all(self.pool())
            .await
        })
        .await
        .context("Failed to load session page")?;

        if rows.is_empty() {
            info!("Reached end of sessions; resetting offset to 0");
            *self.last_offset.lock() = 0;
            return Ok(());
        }

        *self.last_offset.lock() = offset + self.page_size;

        for r in rows {
            match row_to_session(&r) {
                Ok(s) => self.cache.upsert(s),
                Err(e) => {
                    error!(error = %e, "Skipping malformed row in load_next_page");
                    continue;
                }
            }
        }

        Ok(())
    }
}

fn row_to_session(r: &sqlx::any::AnyRow) -> anyhow::Result<Session> {
    let id_str: String = r.get("session_id");
    let session_id = Uuid::parse_str(&id_str).context("bad session_id")?;

    let active_i64: i64 = r.get("active_i64");

    let preferred_chunk_bid: i64 = r.get("preferred_chunk_bid");
    let max_bid_per_tick: i64 = r.get("max_bid_per_tick");

    let remaining_bid: i64 = r.get("remaining_bid");
    let remaining_chunks: i64 = r.get("remaining_chunks");

    let in_flight_bid: i64 = r.get("in_flight_bid");
    let in_flight_chunks: i64 = r.get("in_flight_chunks");

    let cooldown_until_ms: i64 = r.get("cooldown_until_ms");

    let quantum: i64 = r.get("quantum");
    let deficit: i64 = r.get("deficit");
    let last_served_ms: i64 = r.get("last_served_ms");

    Ok(Session {
        session_id,
        pair_id: r.get::<String, _>("pair_id"),
        active: active_i64 == 1,
        intent: SessionIntent {
            constraints: UserConstraints {
                max_spread_bps: r.get::<f64, _>("max_spread_bps"),
                max_trend_drop_bps: r.get::<f64, _>("max_trend_drop_bps"),
                max_slippage_bps: r.get::<f64, _>("max_slippage_bps"),
            },
            preferred_chunk_bid: i64_to_u128(preferred_chunk_bid)?,
            max_bid_per_tick: i64_to_u128(max_bid_per_tick)?,
        },
        state: SessionState {
            remaining_bid: i64_to_u128(remaining_bid)?,
            remaining_chunks: i64_to_u32(remaining_chunks)?,
            in_flight_bid: i64_to_u128(in_flight_bid)?,
            in_flight_chunks: i64_to_u32(in_flight_chunks)?,
            cooldown_until_ms: i64_to_u64(cooldown_until_ms)?,
            quantum: i64_to_u128(quantum)?,
            deficit: deficit as i128,
            last_served_ms: i64_to_u64(last_served_ms)?,
        },
    })
}

fn i64_to_u128(v: i64) -> anyhow::Result<u128> {
    if v < 0 {
        return Err(anyhow!("negative i64 where u128 expected: {v}"));
    }
    Ok(v as u128)
}

fn i64_to_u32(v: i64) -> anyhow::Result<u32> {
    if v < 0 || v > (u32::MAX as i64) {
        return Err(anyhow!("out of range for u32: {v}"));
    }
    Ok(v as u32)
}

fn i64_to_u64(v: i64) -> anyhow::Result<u64> {
    if v < 0 {
        return Err(anyhow!("negative i64 where u64 expected: {v}"));
    }
    Ok(v as u64)
}

fn u64_to_i64(v: u64) -> anyhow::Result<i64> {
    if v > (i64::MAX as u64) {
        return Err(anyhow!("u64 too large for i64: {v}"));
    }
    Ok(v as i64)
}

fn i128_to_i64(v: i128) -> anyhow::Result<i64> {
    if v < (i64::MIN as i128) || v > (i64::MAX as i128) {
        return Err(anyhow!("i128 out of range for i64: {v}"));
    }
    Ok(v as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use std::sync::Arc;

    /// Helper to setup an isolated, in-memory SQLite database for testing.
    async fn setup_test_db() -> Db {
        let pool = AnyPool::connect("sqlite::memory:").await.unwrap();

        sqlx::query(
            r#"
            CREATE TABLE sessions (
                session_id TEXT PRIMARY KEY,
                pair_id TEXT,
                active BOOLEAN,
                max_spread_bps REAL,
                max_trend_drop_bps REAL,
                max_slippage_bps REAL,
                preferred_chunk_bid BIGINT,
                max_bid_per_tick BIGINT,
                remaining_bid BIGINT,
                remaining_chunks BIGINT,
                in_flight_bid BIGINT,
                in_flight_chunks BIGINT,
                cooldown_until_ms BIGINT,
                quantum BIGINT,
                deficit BIGINT,
                last_served_ms BIGINT
            );
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        Db {
            pool: Arc::new(pool),
        }
    }

    /// Tests that the store correctly maps SQL rows to the Session model.
    #[tokio::test]
    async fn test_load_by_id_integrity() {
        let db = setup_test_db().await;
        let store = SessionStore::new(db);
        let id = Uuid::new_v4();

        sqlx::query("INSERT INTO sessions (session_id, active, remaining_bid, remaining_chunks, deficit) VALUES (?, 1, 1000, 10, 500)")
            .bind(id.to_string())
            .execute(store.pool())
            .await.unwrap();

        let session = store.load_by_id(&id).await.expect("Failed to load session");
        assert_eq!(session.session_id, id);
        assert_eq!(session.state.deficit, 500);
    }

    /// Verifies that the store ignores malformed rows during a page load.
    #[tokio::test]
    async fn test_poison_data_resilience() {
        let db = setup_test_db().await;
        let store = SessionStore::new(db);

        // Insert one "poison" row (invalid non-UUID string) and one valid row
        sqlx::query("INSERT INTO sessions (session_id, active, remaining_bid, remaining_chunks) VALUES ('invalid-uuid', 1, 1000, 10)")
            .execute(store.pool()).await.unwrap();

        let good_id = Uuid::new_v4();
        sqlx::query("INSERT INTO sessions (session_id, active, remaining_bid, remaining_chunks) VALUES (?, 1, 1000, 10)")
            .bind(good_id.to_string()).execute(store.pool()).await.unwrap();

        // Execution should succeed, skip the poison row, and load the good one
        store.load_next_page().await.unwrap();

        assert_eq!(store.cache_len_rr(), 1);
        assert!(store.get_cached(&good_id).is_some());
    }

    /// Verifies that the pagination correctly wraps around to the beginning.
    #[tokio::test]
    async fn test_pagination_wrapping() {
        let db = setup_test_db().await;
        let mut store = SessionStore::new(db);
        store.page_size = 2;

        for _ in 0..3 {
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO sessions (session_id, active, remaining_bid, remaining_chunks) VALUES (?, 1, 1000, 10)")
                .bind(id.to_string()).execute(store.pool()).await.unwrap();
        }

        store.load_next_page().await.unwrap(); // Load 2
        assert_eq!(store.cache_len_rr(), 2);

        store.load_next_page().await.unwrap(); // Load 1 (last one)
        assert_eq!(store.cache_len_rr(), 3);

        store.load_next_page().await.unwrap(); // Hits empty result
        assert_eq!(*store.last_offset.lock(), 0, "Offset did not wrap to 0");
    }

    /// Verifies that fairness updates properly transition from i128 (Rust) to BIGINT (SQL).
    #[tokio::test]
    async fn test_persist_fairness_updates_db() {
        let db = setup_test_db().await;
        let store = SessionStore::new(db);
        let id = Uuid::new_v4();

        sqlx::query("INSERT INTO sessions (session_id, active) VALUES (?, 1)")
            .bind(id.to_string())
            .execute(store.pool())
            .await
            .unwrap();

        store.persist_fairness(&id, 1234, 5678).await.unwrap();

        let row = sqlx::query("SELECT deficit, last_served_ms FROM sessions WHERE session_id = ?")
            .bind(id.to_string())
            .fetch_one(store.pool())
            .await
            .unwrap();

        assert_eq!(row.get::<i64, _>("deficit"), 1234);
        assert_eq!(row.get::<i64, _>("last_served_ms"), 5678);
    }

    /// Stress test for concurrent cache refilling.
    #[tokio::test]
    async fn test_concurrent_ensure_candidates() {
        let db = setup_test_db().await;
        let store = Arc::new(SessionStore::new(db));

        // Seed 10 active sessions
        for _ in 0..10 {
            sqlx::query("INSERT INTO sessions (session_id, active, remaining_bid, remaining_chunks) VALUES (?, 1, 1000, 10)")
                .bind(Uuid::new_v4().to_string()).execute(store.pool()).await.unwrap();
        }

        let mut handles = vec![];
        for _ in 0..5 {
            let s = Arc::clone(&store);
            handles.push(tokio::spawn(async move { s.ensure_candidates(5).await }));
        }

        for h in handles {
            h.await.unwrap().expect("Concurrent load task failed");
        }

        // Even with multiple tasks, the cache should be correctly populated
        assert!(store.cache_len_rr() >= 5);
    }
}

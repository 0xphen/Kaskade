use anyhow::{Context, anyhow};
use async_trait::async_trait;
use sqlx::{AnyPool, Row};
use uuid::Uuid;

use crate::execution::types::{ChunkStatus, ReservedBatch, UserResult};
use crate::execution::{u32_to_i64, u128_to_i64};
use crate::planner::types::PlannedAllocation;
use crate::session::model::{Session, SessionIntent, SessionState, UserConstraints};
use crate::session::repository::SessionRepository;
use crate::time::now_ms;

/// SQLx-backed implementation of SessionRepository.
/// Responsible only for persistence and row mapping.
pub struct SqlxSessionRepository {
    pool: AnyPool,
}

impl SqlxSessionRepository {
    pub fn new(pool: AnyPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }
}

#[async_trait]
impl SessionRepository for SqlxSessionRepository {
    async fn fetch_page(&self, limit: usize, offset: usize) -> anyhow::Result<Vec<Session>> {
        let rows = sqlx::query(
            r#"
SELECT
  session_id, pair_id, CASE WHEN active THEN 1 ELSE 0 END AS active_i64,
  max_spread_bps, max_trend_drop_bps, max_slippage_bps,
  preferred_chunk_bid, max_bid_per_tick,
  remaining_bid, remaining_chunks,
  in_flight_bid, in_flight_chunks,
  cooldown_until_ms,
  quantum, deficit, last_served_ms,
  CAST(has_pending_batch AS INTEGER) AS has_pending_batch
FROM sessions
WHERE active = TRUE AND remaining_bid > 0 AND remaining_chunks > 0
LIMIT ? OFFSET ?;
"#,
        )
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::new();
        for r in rows {
            match row_to_session(&r) {
                Ok(s) => out.push(s),
                Err(e) => {
                    // poison-row resilience: skip but don’t fail the batch
                    tracing::warn!(error = %e, "skipping malformed session row");
                }
            }
        }

        Ok(out)
    }

    async fn fetch_by_id(&self, session_id: &Uuid) -> anyhow::Result<Option<Session>> {
        let row = sqlx::query(
            r#"
SELECT
  session_id, pair_id, CASE WHEN active THEN 1 ELSE 0 END AS active_i64,
  max_spread_bps, max_trend_drop_bps, max_slippage_bps,
  preferred_chunk_bid, max_bid_per_tick,
  remaining_bid, remaining_chunks,
  in_flight_bid, in_flight_chunks,
  cooldown_until_ms,
  quantum, deficit, last_served_ms, 
  CAST(has_pending_batch AS INTEGER) AS has_pending_batch
FROM sessions
WHERE session_id = ?;
"#,
        )
        .bind(session_id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(r) => Ok(Some(row_to_session(&r)?)),
            None => Ok(None),
        }
    }

    async fn persist_fairness(
        &self,
        session_id: &Uuid,
        deficit: i128,
        last_served_ms: u64,
    ) -> anyhow::Result<()> {
        let deficit_i64 = i128_to_i64(deficit)?;
        let last_served_i64 = u64_to_i64(last_served_ms)?;

        sqlx::query(
            r#"
UPDATE sessions
SET deficit = ?, last_served_ms = ?
WHERE session_id = ?;
"#,
        )
        .bind(deficit_i64)
        .bind(last_served_i64)
        .bind(session_id.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn reserve_execution(
        &self,
        pair_id: &str,
        now_ms: u64,
        allocations: &[PlannedAllocation],
    ) -> anyhow::Result<Option<ReservedBatch>> {
        use crate::execution::types::{ReservedBatch, ReservedChunk, ReservedUser};

        let mut tx = self.pool.begin().await?;

        let mut users_out: Vec<ReservedUser> = Vec::new();
        let mut total_reserved_any = false;

        // We'll only create batch row if at least one reservation succeeds.
        let batch_id = Uuid::new_v4();

        for a in allocations {
            let total_bid: u128 = a.chunks.iter().copied().sum();
            let total_chunks = a.chunks.len() as u32;

            // Try to reserve this session.
            let res = sqlx::query(
                r#"
UPDATE sessions
SET in_flight_bid     = in_flight_bid + ?,
    in_flight_chunks  = in_flight_chunks + ?,
    has_pending_batch = 1
WHERE session_id = ?
  AND pair_id = ?
  AND active = 1
  AND has_pending_batch = 0
  AND (remaining_bid - in_flight_bid) >= ?
  AND (remaining_chunks - in_flight_chunks) >= ?;
"#,
            )
            .bind(u128_to_i64(total_bid)?)
            .bind(u32_to_i64(total_chunks)?)
            .bind(a.session_id.to_string())
            .bind(pair_id)
            .bind(u128_to_i64(total_bid)?)
            .bind(u32_to_i64(total_chunks)?)
            .execute(&mut *tx)
            .await?;

            // CAS miss: not an error, skip this session.
            if res.rows_affected() != 1 {
                tracing::debug!(
                    session_id = %a.session_id,
                    pair_id = %pair_id,
                    want_bid = %total_bid,
                    want_chunks = %total_chunks,
                    "reserve CAS miss; skipping session"
                );
                continue;
            }

            if !total_reserved_any {
                sqlx::query(
                    r#"
INSERT INTO batches(batch_id, pair_id, created_ms, status, reason)
VALUES (?, ?, ?, 'RESERVED', '');
"#,
                )
                .bind(batch_id.to_string())
                .bind(pair_id)
                .bind(u64_to_i64(now_ms)?)
                .execute(&mut *tx)
                .await?;

                total_reserved_any = true;
            }

            // Create batch items for this reserved session
            let mut chunks_out = Vec::new();
            for bid in &a.chunks {
                let chunk_id = Uuid::new_v4();
                sqlx::query(
                    r#"
INSERT INTO batch_items(chunk_id, batch_id, session_id, bid, status, tx_id, error)
VALUES (?, ?, ?, ?, 'PENDING', '', '');
"#,
                )
                .bind(chunk_id.to_string())
                .bind(batch_id.to_string())
                .bind(a.session_id.to_string())
                .bind(u128_to_i64(*bid)?)
                .execute(&mut *tx)
                .await?;

                chunks_out.push(ReservedChunk {
                    chunk_id,
                    bid: *bid,
                });
            }

            users_out.push(ReservedUser {
                session_id: a.session_id,
                chunks: chunks_out,
            });
        }

        // If nothing reserved, rollback and return None (no empty batch).
        if !total_reserved_any {
            tx.rollback().await?;
            return Ok(None);
        }

        tx.commit().await?;

        Ok(Some(ReservedBatch {
            batch_id,
            pair_id: pair_id.to_string(),
            created_ms: now_ms,
            users: users_out,
        }))
    }

    async fn commit_batch(
        &self,
        batch: &ReservedBatch,
        results: &[UserResult],
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;

        let row = sqlx::query("SELECT status FROM batches WHERE batch_id = ?")
            .bind(batch.batch_id.to_string())
            .fetch_one(&mut *tx)
            .await?;

        let status: String = row.get(0);
        match status.as_str() {
            "RESERVED" => {}
            "COMMITTED" | "ABORTED" => {
                tx.commit().await?;
                return Ok(());
            }
            other => return Err(anyhow!("unexpected batch status: {}", other)),
        }

        let now = now_ms();
        let now_i64 = u64_to_i64(now)?;

        use std::collections::HashSet;
        let mut touched_sessions = HashSet::new();

        for ur in results {
            touched_sessions.insert(ur.session_id);

            // Optional cooldown
            if let Some(cd) = ur.cooldown_ms {
                let until = now.saturating_add(cd);
                let until_i64 = u64_to_i64(until)?;

                sqlx::query(
                    r#"
UPDATE sessions
SET cooldown_until_ms =
  CASE WHEN cooldown_until_ms > ? THEN cooldown_until_ms ELSE ? END
WHERE session_id = ?;
"#,
                )
                .bind(until_i64)
                .bind(until_i64)
                .bind(ur.session_id.to_string())
                .execute(&mut *tx)
                .await?;
            }

            for cr in &ur.chunk_results {
                let row = sqlx::query(
                    r#"
SELECT bid, status
FROM batch_items
WHERE batch_id = ? AND chunk_id = ?;
"#,
                )
                .bind(batch.batch_id.to_string())
                .bind(cr.chunk_id.to_string())
                .fetch_one(&mut *tx)
                .await?;

                let bid: i64 = row.get(0);
                let cur_status: String = row.get(1);

                // Idempotency at chunk level
                if cur_status != "PENDING" {
                    continue;
                }

                match &cr.status {
                    ChunkStatus::Success { tx_id } => {
                        sqlx::query(
                            r#"
UPDATE batch_items
SET status='SUCCESS', tx_id=?, error=''
WHERE batch_id=? AND chunk_id=?;
"#,
                        )
                        .bind(tx_id)
                        .bind(batch.batch_id.to_string())
                        .bind(cr.chunk_id.to_string())
                        .execute(&mut *tx)
                        .await?;

                        sqlx::query(
                            r#"
UPDATE sessions
SET in_flight_bid    = in_flight_bid - ?,
    in_flight_chunks = in_flight_chunks - 1,
    remaining_bid    = remaining_bid - ?,
    remaining_chunks = remaining_chunks - 1,
    last_served_ms   = ?
WHERE session_id = ?;
"#,
                        )
                        .bind(bid)
                        .bind(bid)
                        .bind(now_i64)
                        .bind(ur.session_id.to_string())
                        .execute(&mut *tx)
                        .await?;
                    }

                    ChunkStatus::Failed { reason } | ChunkStatus::Skipped { reason } => {
                        let status = match cr.status {
                            ChunkStatus::Failed { .. } => "FAILED",
                            _ => "SKIPPED",
                        };

                        sqlx::query(
                            r#"
UPDATE batch_items
SET status=?, tx_id='', error=?
WHERE batch_id=? AND chunk_id=?;
"#,
                        )
                        .bind(status)
                        .bind(reason)
                        .bind(batch.batch_id.to_string())
                        .bind(cr.chunk_id.to_string())
                        .execute(&mut *tx)
                        .await?;

                        // Unwind in-flight only
                        sqlx::query(
                            r#"
UPDATE sessions
SET in_flight_bid    = in_flight_bid - ?,
    in_flight_chunks = in_flight_chunks - 1
WHERE session_id = ?;
"#,
                        )
                        .bind(bid)
                        .bind(ur.session_id.to_string())
                        .execute(&mut *tx)
                        .await?;
                    }
                }
            }
        }

        // Release per-session exclusive lock
        for sid in touched_sessions {
            sqlx::query(
                r#"
UPDATE sessions
SET has_pending_batch = 0
WHERE session_id = ?;
"#,
            )
            .bind(sid.to_string())
            .execute(&mut *tx)
            .await?;
        }

        // Commit batch
        sqlx::query(
            r#"
UPDATE batches
SET status='COMMITTED', reason=''
WHERE batch_id=?;
"#,
        )
        .bind(batch.batch_id.to_string())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }
}

/* =========================
Row mapping + conversions
========================= */

fn row_to_session(r: &sqlx::any::AnyRow) -> anyhow::Result<Session> {
    let id_str: String = r.get("session_id");
    let session_id = Uuid::parse_str(&id_str).context("invalid session_id")?;

    let active_i64: i64 = r.get("active_i64");

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
            preferred_chunk_bid: i64_to_u128(r.get("preferred_chunk_bid"))?,
            max_bid_per_tick: i64_to_u128(r.get("max_bid_per_tick"))?,
        },
        state: SessionState {
            remaining_bid: i64_to_u128(r.get("remaining_bid"))?,
            remaining_chunks: i64_to_u32(r.get("remaining_chunks"))?,
            in_flight_bid: i64_to_u128(r.get("in_flight_bid"))?,
            in_flight_chunks: i64_to_u32(r.get("in_flight_chunks"))?,
            cooldown_until_ms: i64_to_u64(r.get("cooldown_until_ms"))?,
            quantum: i64_to_u128(r.get("quantum"))?,
            deficit: r.get::<i64, _>("deficit") as i128,
            last_served_ms: i64_to_u64(r.get("last_served_ms"))?,
            has_pending_batch: r.get::<i64, _>("has_pending_batch") != 0,
        },
    })
}

/* =========================
Numeric safety helpers
========================= */

fn i64_to_u128(v: i64) -> anyhow::Result<u128> {
    if v < 0 {
        return Err(anyhow!("negative i64 where u128 expected: {v}"));
    }
    Ok(v as u128)
}

fn i64_to_u32(v: i64) -> anyhow::Result<u32> {
    if v < 0 || v > u32::MAX as i64 {
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
    if v > i64::MAX as u64 {
        return Err(anyhow!("u64 too large for i64: {v}"));
    }
    Ok(v as i64)
}

fn i128_to_i64(v: i128) -> anyhow::Result<i64> {
    if v < i64::MIN as i128 || v > i64::MAX as i128 {
        return Err(anyhow!("i128 out of range for i64: {v}"));
    }
    Ok(v as i64)
}

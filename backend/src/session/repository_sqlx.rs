use anyhow::{Context, anyhow};
use async_trait::async_trait;
use sqlx::{AnyPool, Row};
use uuid::Uuid;

use crate::session::model::{Session, SessionIntent, SessionState, UserConstraints};
use crate::session::repository::SessionRepository;

/// SQLx-backed implementation of SessionRepository.
/// Responsible only for persistence and row mapping.
pub struct SqlxSessionRepository {
    pool: AnyPool,
}

impl SqlxSessionRepository {
    pub fn new(pool: AnyPool) -> Self {
        Self { pool }
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
  quantum, deficit, last_served_ms
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
  quantum, deficit, last_served_ms
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

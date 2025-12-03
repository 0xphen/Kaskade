//! SQLiteSessionStore
//! --------------------
//! This module provides a **SQLite-backed implementation** of the `SessionStore`
//! trait used by the session::manager subsystem. It is responsible for durable
//! persistence of user sessions so that:
//!
//!  - sessions survive restarts
//!  - progress is tracked across chunks
//!  - cancellations / expirations clean up storage
//!  - scheduler + executor operate purely in-memory, using SessionManager
use async_trait::async_trait;
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

use super::SessionStore;
use crate::model::{Session, SessionId, SessionState, SessionThresholds};

/// SQLite-based persistence backend for sessions.
///
/// This struct implements the `SessionStore` trait and provides:
///
///   - schema creation on startup
///   - loading persisted sessions (`load_all`)
///   - upsert semantics (`save`)
///   - permanent removal (`delete`)
pub struct SQLiteSessionStore {
    pool: SqlitePool,
}

impl SQLiteSessionStore {
    pub fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Create a new SQLite-backed store and ensure schema exists.
    pub async fn new(path: &str) -> anyhow::Result<Self> {
        let pool = SqlitePool::connect(path).await?;

        // Creates table if it does not exist
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                user_id INTEGER NOT NULL,

                pair_base TEXT NOT NULL,
                pair_quote TEXT NOT NULL,

                created_at_ms INTEGER NOT NULL,
                expires_at_ms INTEGER,

                total_amount_in INTEGER NOT NULL,
                chunk_amount_in INTEGER NOT NULL,
                approved_amount_in INTEGER,
                executed_amount_in INTEGER NOT NULL,
                executed_amount_out INTEGER NOT NULL,
                remaining_amount_in INTEGER NOT NULL,
                num_executed_chunks INTEGER NOT NULL,
                last_execution_ts_ms INTEGER,

                wallet_address TEXT,
                state TEXT NOT NULL,
                thresholds_json TEXT NOT NULL
            );
        "#,
        )
        .execute(&pool)
        .await?;

        Ok(Self { pool })
    }
}

#[async_trait]
impl SessionStore for SQLiteSessionStore {
    /// Load all sessions from persistent storage.
    ///
    /// This is called once at startup by SessionManager to reconstruct
    /// the in-memory live session set and rebuild secondary indexes.
    async fn load_all(&self) -> anyhow::Result<Vec<Session>> {
        let rows = sqlx::query("SELECT * FROM sessions")
            .fetch_all(&self.pool)
            .await?;

        let mut sessions = Vec::with_capacity(rows.len());

        for row in rows {
            let id_str: String = row.get("id");
            let id = uuid::Uuid::parse_str(&id_str)?;
            let user_id: i64 = row.get("user_id");
            let pair_base: String = row.get("pair_base");
            let pair_quote: String = row.get("pair_quote");
            let pair = market::types::Pair {
                base: pair_base,
                quote: pair_quote,
            };
            let created_at_ms = row.get::<i64, _>("created_at_ms") as u64;
            let expires_at_ms = row.get::<Option<i64>, _>("expires_at_ms").map(|v| v as u64);

            let total_amount_in = row.get::<i64, _>("total_amount_in") as u64;
            let chunk_amount_in = row.get::<i64, _>("chunk_amount_in") as u64;

            let approved_amount_in = row
                .get::<Option<i64>, _>("approved_amount_in")
                .map(|v| v as u64);

            let executed_amount_in = row.get::<i64, _>("executed_amount_in") as u64;
            let executed_amount_out = row.get::<i64, _>("executed_amount_out") as u64;

            let remaining_amount_in = row.get::<i64, _>("remaining_amount_in") as u64;

            let num_executed_chunks = row.get::<i64, _>("num_executed_chunks") as u64;

            let last_execution_ts_ms = row
                .get::<Option<i64>, _>("last_execution_ts_ms")
                .map(|v| v as u64);

            let wallet_address: Option<String> = row.get("wallet_address");

            let state_str: String = row.get("state");
            let state = SessionState::from_str(&state_str)
                .map_err(|e| anyhow::anyhow!("Invalid session state '{}': {}", state_str, e))?;

            let thresholds_json: String = row.get("thresholds_json");
            let thresholds: SessionThresholds =
                serde_json::from_str(&thresholds_json).map_err(|e| {
                    anyhow::anyhow!("Invalid thresholds JSON '{}': {}", thresholds_json, e)
                })?;

            sessions.push(Session {
                id,
                user_id: user_id as u64,
                pair,
                created_at_ms,
                expires_at_ms,
                total_amount_in,
                chunk_amount_in,
                approved_amount_in,
                executed_amount_in,
                executed_amount_out,
                remaining_amount_in,
                num_executed_chunks,
                last_execution_ts_ms,
                wallet_address,
                state,
                thresholds,
            });
        }

        Ok(sessions)
    }

    /// Store or update a session.
    ///
    /// `save()` uses INSERT OR UPDATE semantics:
    /// - New session → inserted
    /// - Existing session → updated
    async fn save(&self, session: &Session) -> anyhow::Result<()> {
        let thresholds_json = serde_json::to_string(&session.thresholds)?;

        sqlx::query(
            r#"
            INSERT INTO sessions (
                id, user_id, pair_base, pair_quote,
                created_at_ms, expires_at_ms,
                total_amount_in, chunk_amount_in,
                approved_amount_in,
                executed_amount_in, executed_amount_out,
                remaining_amount_in, num_executed_chunks,
                last_execution_ts_ms, wallet_address,
                state, thresholds_json
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                user_id = excluded.user_id,
                pair_base = excluded.pair_base,
                pair_quote = excluded.pair_quote,
                created_at_ms = excluded.created_at_ms,
                expires_at_ms = excluded.expires_at_ms,
                total_amount_in = excluded.total_amount_in,
                chunk_amount_in = excluded.chunk_amount_in,
                approved_amount_in = excluded.approved_amount_in,
                executed_amount_in = excluded.executed_amount_in,
                executed_amount_out = excluded.executed_amount_out,
                remaining_amount_in = excluded.remaining_amount_in,
                num_executed_chunks = excluded.num_executed_chunks,
                last_execution_ts_ms = excluded.last_execution_ts_ms,
                wallet_address = excluded.wallet_address,
                state = excluded.state,
                thresholds_json = excluded.thresholds_json;
        "#,
        )
        .bind(session.id.to_string())
        .bind(session.user_id as i64)
        .bind(&session.pair.base)
        .bind(&session.pair.quote)
        .bind(session.created_at_ms as i64)
        .bind(session.expires_at_ms.map(|v| v as i64))
        .bind(session.total_amount_in as i64)
        .bind(session.chunk_amount_in as i64)
        .bind(session.approved_amount_in.map(|v| v as i64))
        .bind(session.executed_amount_in as i64)
        .bind(session.executed_amount_out as i64)
        .bind(session.remaining_amount_in as i64)
        .bind(session.num_executed_chunks as i64)
        .bind(session.last_execution_ts_ms.map(|v| v as i64))
        .bind(&session.wallet_address)
        .bind(session.state.to_string())
        .bind(thresholds_json)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Permanently delete a session by ID.
    ///
    /// Called by SessionManager when:
    ///   - session is completed
    ///   - session is cancelled
    ///   - session expires
    async fn delete(&self, session_id: SessionId) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}

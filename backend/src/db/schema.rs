use sqlx::AnyPool;

pub async fn migrate(pool: &AnyPool) -> anyhow::Result<()> {
    // Sessions
    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS sessions (
  session_id TEXT PRIMARY KEY,
  pair_id TEXT NOT NULL,
  active INTEGER NOT NULL CHECK (active IN (0,1)),
  max_spread_bps REAL NOT NULL,
  max_trend_drop_bps REAL NOT NULL,
  max_slippage_bps REAL NOT NULL,
  preferred_chunk_bid BIGINT NOT NULL,
  max_bid_per_tick BIGINT NOT NULL,
  remaining_bid BIGINT NOT NULL,
  remaining_chunks BIGINT NOT NULL,
  in_flight_bid BIGINT NOT NULL,
  in_flight_chunks BIGINT NOT NULL,
  cooldown_until_ms BIGINT NOT NULL,
  quantum BIGINT NOT NULL,
  deficit BIGINT NOT NULL,
  last_served_ms BIGINT NOT NULL,
  has_pending_batch INTEGER NOT NULL DEFAULT 0 CHECK (has_pending_batch IN (0,1))
);
"#,
    )
    .execute(pool)
    .await?;

    // Batches
    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS batches (
  batch_id TEXT PRIMARY KEY,
  pair_id TEXT NOT NULL,
  created_ms BIGINT NOT NULL,
  status TEXT NOT NULL,
  reason TEXT NOT NULL
);
"#,
    )
    .execute(pool)
    .await?;

    // Batch items
    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS batch_items (
  chunk_id TEXT PRIMARY KEY,
  batch_id TEXT NOT NULL,
  session_id TEXT NOT NULL,
  bid BIGINT NOT NULL,

  status TEXT NOT NULL,
  tx_id TEXT NOT NULL,
  error TEXT NOT NULL
);
"#,
    )
    .execute(pool)
    .await?;

    sqlx::query(r#"CREATE INDEX IF NOT EXISTS idx_sessions_pair ON sessions(pair_id);"#)
        .execute(pool)
        .await?;

    sqlx::query(r#"CREATE INDEX IF NOT EXISTS idx_batch_items_batch ON batch_items(batch_id);"#)
        .execute(pool)
        .await?;

    Ok(())
}

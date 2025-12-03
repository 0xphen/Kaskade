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

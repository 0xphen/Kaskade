use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct ReservedChunk {
    pub chunk_id: Uuid,
    pub bid: u128,
}

#[derive(Clone, Debug)]
pub struct ReservedUser {
    pub session_id: Uuid,
    pub chunks: Vec<ReservedChunk>,
}

#[derive(Clone, Debug)]
pub struct ReservedBatch {
    pub batch_id: Uuid,
    pub pair_id: String,
    pub created_ms: u64,
    pub users: Vec<ReservedUser>,
}

#[derive(Clone, Debug)]
pub enum ExecutionEvent {
    Reserved(ReservedBatch),
}

#[derive(Clone, Debug)]
pub enum ChunkStatus {
    Success { tx_id: String },
    Failed { reason: String },
    Skipped { reason: String },
}

#[derive(Clone, Debug)]
pub struct ChunkResult {
    pub chunk_id: Uuid,
    pub status: ChunkStatus,
}

#[derive(Clone, Debug)]
pub struct UserResult {
    pub session_id: Uuid,
    pub chunk_results: Vec<ChunkResult>,
    pub cooldown_ms: Option<u64>,
}

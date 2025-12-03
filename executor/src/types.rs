//! Common types and small abstraction traits used by the executor.

use async_trait::async_trait;
use thiserror::Error;

use market::types::{MarketMetrics, Pair};
use scheduler::types::ExecutionRequest;
use session::model::{Session, SessionId};

/// Re-export the job type produced by the scheduler.
pub type ExecutionJob = ExecutionRequest;

/// Unique identifier for a user (Telegram chat_id).
pub type UserId = u64;

/// Low-level TON message representation (BOC string for MVP).
#[derive(Debug, Clone)]
pub struct TonMessage {
    /// Serialized BOC or payload that wallets / RPC can understand.
    pub boc: String,
}

/// Result of building a swap transfer via Omniston gRPC.
#[derive(Debug, Clone)]
pub struct BuiltTransfer {
    /// The TON messages to be sent to the network.
    pub messages: Vec<TonMessage>,
    /// Expected output amount (e.g. STON) from this chunk, if executed now.
    pub expected_amount_out: u64,
}

/// What happened when a worker processed a job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionOutcome {
    /// Swap successfully sent to the network.
    Executed { tx_hash: String },
    /// Job was dropped because conditions were no longer good (stale).
    ConditionsNotMet,
    /// Job was dropped because the session no longer exists or is not active.
    SessionNotReady,
    /// Job failed due to internal error (network, gRPC, etc.).
    Failed,
}

/// Errors that can occur during execution.
#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("session not found")]
    SessionNotFound,

    #[error("session not active (state mismatch)")]
    SessionNotActive,

    #[error("no remaining amount for this session")]
    NoRemaining,

    #[error("session has insufficient approval: approved={approved}, required={required}")]
    InsufficientApproval { approved: u64, required: u64 },

    #[error("market snapshot unavailable")]
    MissingMarketSnapshot,

    #[error("conditions no longer met")]
    ConditionsNotMet,

    #[error("swap builder error: {0}")]
    SwapBuilder(String),

    #[error("ton client error: {0}")]
    TonClient(String),

    #[error("notifier error: {0}")]
    Notifier(String),

    #[error("other: {0}")]
    Other(String),
}

/// Minimal read-only view of the latest market state for a pair.
///
/// This is deliberately small so `executor` does not depend on the full
/// `MarketManager` implementation.
#[async_trait]
pub trait MarketReader: Send + Sync {
    async fn latest_metrics(&self, pair: &Pair) -> Option<MarketMetrics>;
}

/// Abstraction over Omniston gRPC / RFQ swap building.
#[async_trait]
pub trait SwapBuilder: Send + Sync {
    async fn build_transfer(
        &self,
        session: &Session,
        pair: &Pair,
        amount_in: u64,
        metrics: &MarketMetrics,
    ) -> Result<BuiltTransfer, ExecutionError>;
}

/// Abstraction over TON transport: send BOCs to network.
///
/// In reality this might:
///   - call a TON HTTP API
///   - use liteclient / tonlibjson
///   - or hand BOCs to another backend.
#[async_trait]
pub trait TonClient: Send + Sync {
    /// Submit one or more TON messages to the network.
    ///
    /// Returns a tx hash or unique ID if available.
    async fn submit_messages(&self, messages: &[TonMessage]) -> Result<String, ExecutionError>;
}

/// Abstraction over user-facing notifications (Telegram, CLI, etc.).
///
/// NOTE: With pre-approval, we do not ask the user to sign each chunk:
/// they have already allowed the system to swap for them.
/// We only notify them about executed chunks.
#[async_trait]
pub trait Notifier: Send + Sync {
    /// Called when a chunk has been executed (or at least submitted).
    async fn notify_chunk_executed(
        &self,
        user_id: UserId,
        session: &Session,
        pair: &Pair,
        amount_in: u64,
        est_amount_out: u64,
        tx_hash: &str,
    ) -> Result<(), ExecutionError>;
}

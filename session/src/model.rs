use std::fmt;
use std::str::FromStr;

use market::types::{MarketMetrics, Pair};
use serde::{Deserialize, Serialize};

pub type SessionId = uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Created,
    WaitingPreApproval,
    Active,
    Completed,
    Cancelled,
    Expired,
}

impl fmt::Display for SessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            SessionState::Created => "Created",
            SessionState::WaitingPreApproval => "WaitingPreApproval",
            SessionState::Active => "Active",
            SessionState::Completed => "Completed",
            SessionState::Cancelled => "Cancelled",
            SessionState::Expired => "Expired",
        };
        f.write_str(s)
    }
}

impl FromStr for SessionState {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Created" => Ok(SessionState::Created),
            "WaitingPreApproval" => Ok(SessionState::WaitingPreApproval),
            "Active" => Ok(SessionState::Active),
            "Completed" => Ok(SessionState::Completed),
            "Cancelled" => Ok(SessionState::Cancelled),
            "Expired" => Ok(SessionState::Expired),
            other => Err(anyhow::anyhow!("Invalid SessionState value: {}", other)),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionThresholds {
    pub max_spread_bps: f64,
    pub max_slippage_bps: f64,
    pub trend_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: SessionId,

    // Identity
    pub user_id: u64, // Telegram chat_id
    pub pair: Pair,

    // Config
    pub total_amount_in: u64,
    pub chunk_amount_in: u64,
    pub thresholds: SessionThresholds,
    pub created_at_ms: u64,
    pub expires_at_ms: Option<u64>,

    // Pre-approval metadata
    pub approved_amount_in: Option<u64>,
    pub wallet_address: Option<String>,

    // Progress
    pub remaining_amount_in: u64,
    pub executed_amount_in: u64,
    pub executed_amount_out: u64,
    pub num_executed_chunks: u64,
    pub last_execution_ts_ms: Option<u64>,

    // Lifecycle
    pub state: SessionState,
}

impl Session {
    pub fn can_fire_chunk(&self, metrics: &MarketMetrics, now_ms: u64) -> bool {
        if self.state != SessionState::Active {
            return false;
        }
        if self.is_expired(now_ms) || !self.has_remaining() {
            return false;
        }

        // let spread_ok = metrics.spread.spread_bps <= self.thresholds.max_spread_bps;
        // let slippage_ok = metrics.slippage_bps <= self.thresholds.max_slippage_bps;
        // let trend_ok = !self.thresholds.require_trend_up || metrics.trend_up;

        metrics.spread.spread_bps <= self.thresholds.max_spread_bps
    }

    pub fn has_remaining(&self) -> bool {
        debug_assert!(
            self.executed_amount_in + self.remaining_amount_in == self.total_amount_in,
            "inconsistent session accounting"
        );

        self.remaining_amount_in > 0
    }

    /// Returns true if the session has an expiry set and we have passed it.
    pub fn is_expired(&self, now_ms: u64) -> bool {
        match self.expires_at_ms {
            Some(expiry) => now_ms >= expiry,
            None => false,
        }
    }
}

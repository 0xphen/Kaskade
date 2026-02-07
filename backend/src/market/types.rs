use serde::{Deserialize, Serialize};

/// Snapshot of raw STON.fi pool state
#[derive(Debug, Clone)]
pub struct PoolSnapshot {
    pub reserve0: u128,
    pub reserve1: u128,
    pub lp_fee: u32,       // e.g. 20 = 0.20%
    pub protocol_fee: u32, // e.g. 10 = 0.10%
    pub ts_ms: u64,
}

/// Combined market metrics for a pool at a specific time.
///
/// This represents *market health*, not execution results.
#[derive(Debug, Clone, Default)]
pub struct MarketMetrics {
    pub ts_ms: u64,

    /// Structural friction (fees + AMM curvature).
    pub spread_bps: f64,

    /// Downward price pressure (positive = drop).
    pub trend_drop_bps: f64,

    /// Market is healthy enough to trade.
    pub validity: bool,
}

#[derive(Debug, Clone)]
pub enum RfqAmount {
    BidUnits(String),
    AskUnits(String),
}

#[derive(Debug, Clone)]
pub struct RfqRequest {
    pub bid_asset: String,
    pub ask_asset: String,
    pub amount: RfqAmount,
}

/// TON Jetton or native asset address used by Omniston.
#[derive(Debug, Clone, Deserialize)]
pub struct AssetAddress {
    pub blockchain: u32,
    pub address: String,
}

/// Single protocol execution inside a swap step.
#[derive(Debug, Clone, Deserialize)]
pub struct RouteChunk {
    pub protocol: String,
    pub bid_amount: String,
    pub ask_amount: String,
    pub extra_version: u32,
    pub extra: Vec<u8>,
}

/// Swap route step (bid_asset → ask_asset).
#[derive(Debug, Clone, Deserialize)]
pub struct RouteStep {
    pub bid_asset_address: AssetAddress,
    pub ask_asset_address: AssetAddress,
    pub chunks: Vec<RouteChunk>,
}

/// Full route (possibly multi-hop)
#[derive(Debug, Clone, Deserialize)]
pub struct Route {
    pub steps: Vec<RouteStep>,
}

/// Swap-specific parameters inside `"params.swap"`
#[derive(Debug, Clone, Deserialize)]
pub struct SwapParams {
    pub routes: Vec<Route>,
    pub min_ask_amount: String,
    pub recommended_min_ask_amount: String,
    pub recommended_slippage_bps: u32,
}

/// Container for `params.swap`, `.escrow`, etc.
#[derive(Debug, Clone, Deserialize)]
pub struct QuoteParams {
    pub swap: Option<SwapParams>,
}

/// Typed representation of `"quote_updated"`
#[derive(Debug, Clone, Deserialize)]
pub struct Quote {
    pub quote_id: String,
    pub resolver_id: String,
    pub resolver_name: String,

    pub bid_asset_address: AssetAddress,
    pub ask_asset_address: AssetAddress,

    pub bid_units: String,
    pub ask_units: String,

    pub referrer_address: Option<AssetAddress>,

    pub referrer_fee_asset: AssetAddress,
    pub referrer_fee_units: String,

    pub protocol_fee_asset: AssetAddress,
    pub protocol_fee_units: String,

    pub quote_timestamp: u64,
    pub trade_start_deadline: i64,

    pub gas_budget: String,
    pub estimated_gas_consumption: String,

    pub params: QuoteParams,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuoteUpdatedEvent {
    pub quote_updated: Quote,
}

/// Unified Omniston event enum for your engine.
#[derive(Debug, Clone)]
pub enum OmnistonEvent {
    Ack { rfq_id: String },
    QuoteUpdated(Box<Quote>),
    NoQuote,
    KeepAlive,
    Unsubscribed { rfq_id: Option<String> },
    Unknown(serde_json::Value),
}

#[derive(Debug, Clone, Deserialize, Default, Eq, PartialEq)]
pub enum QuoteSide {
    Ask,

    #[default]
    Bid,
}

#[derive(Debug, Clone, Eq, PartialEq, std::hash::Hash)]
pub struct Pair {
    pub base: String,
    pub quote: String,
}

impl Pair {
    pub fn new(base: String, quote: String) -> Self {
        Self { base, quote }
    }
}

impl Pair {
    pub fn id(&self) -> String {
        format!("{}/{}", self.base, self.quote)
    }
}

pub struct SubscriptionRequest {
    pub pair: Pair,
    pub amount: RfqAmount,
    pub sender_ch: tokio::sync::mpsc::Sender<MarketMetrics>,
}

/// Immutable market snapshot used by scheduler (Gate A) and executor (Gate B).
/// Metrics are computed upstream and treated as advisory constraints only.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketMetricsView {
    /// Snapshot timestamp (ms since epoch)
    pub ts_ms: u64,

    /// Execution constraint metrics (basis points)
    pub spread_bps: f64,
    pub trend_drop_bps: f64,
    pub slippage_bps: f64,

    /// Available input-side liquidity at snapshot time
    pub depth_now_in: u128,
}

#[derive(Clone, Debug)]
pub enum ExecutionScope {
    MarketWide,
    ProtocolOnly { protocol: String },
}

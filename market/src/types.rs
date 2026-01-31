use super::pulse::{slippage::SlippagePulseResult, spread::SpreadPulseResult};
use serde::Deserialize;

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

#[derive(Clone, Default, Debug)]
pub struct MarketMetrics {
    pub spread: SpreadPulseResult,
    pub slippage: SlippagePulseResult,
    // pub trend: TrendPulseResult,
    // pub depth: DepthPulseResult,
}

use serde::Deserialize;

use super::QuoteSide;
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

    pub quote_timestamp: i64,
    pub trade_start_deadline: i64,

    pub gas_budget: String,
    pub estimated_gas_consumption: String,

    pub params: QuoteParams,

    #[serde(default)]
    pub side: QuoteSide,
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

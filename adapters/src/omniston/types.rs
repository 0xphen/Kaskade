use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct Address {
    pub blockchain: u64,
    pub address: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SwapChunk {
    pub protocol: u32,
    pub bid_amount: String,
    pub ask_amount: String,
    pub extra_version: u32,
    pub extra: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SwapStep {
    pub bid_asset_address: Address,
    pub ask_asset_address: Address,
    pub chunks: Vec<SwapChunk>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SwapRoute {
    pub steps: Vec<SwapStep>,
    pub gas_budget: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SwapParams {
    pub routes: Vec<SwapRoute>,
    pub min_ask_amount: String,
    pub recommended_min_ask_amount: String,
    pub recommended_slippage_bps: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuoteUpdated {
    pub quote_id: String,

    pub bid_asset_address: Address,
    pub ask_asset_address: Address,

    pub bid_units: String,
    pub ask_units: String,

    pub referrer_address: Address,
    pub referrer_fee_units: String,
    pub protocol_fee_units: String,

    pub quote_timestamp: u64,
    pub trade_start_deadline: u64,

    #[serde(rename = "params")]
    pub params: QuoteParamsWrapper,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuoteParamsWrapper {
    #[serde(rename = "swap")]
    pub swap: SwapParams,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuoteEvent {
    #[serde(rename = "quote_updated")]
    pub quote: QuoteUpdated,
}

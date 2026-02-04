use crate::market::types::Quote;
use std::sync::Arc;

/// Price-based pulse input (Spread, Trend).
#[derive(Clone, Copy, Debug)]
pub struct PriceInput {
    pub ts_ms: u64,
    pub bid_units: f64,
    pub ask_units: f64,
}

/// Quote-based pulse input (Depth, Slippage).
#[derive(Clone, Debug)]
pub struct QuoteInput {
    pub ts_ms: u64,
    pub quote: Arc<Quote>,
}

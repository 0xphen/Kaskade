#[derive(Debug, Clone)]
pub enum PulseType {
    Spread,
    Slippage,
    Trend,
    Volatility,
    Depth,
    Imbalance,
    TimeDecay,
}

#[derive(Debug, Clone)]
pub enum PulseDetails {
    Spread {
        bid: f64,
        ask: f64,
        spread_bps: f64,
        threshold_bps: f64,
        timestamp: u64,
    },
    Slippage {
        estimated: f64,
        threshold: f64,
        timestamp: u64,
    },
    Trend {
        slope: f64,
        confidence: f64,
        window_size: usize,
        timestamp: u64,
    },
    // Add more pulse types in future
}

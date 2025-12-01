use super::{PulseDetails, PulseEngine, PulseSignal, PulseType, SpreadConfig};
use crate::{normalized_quote::NormalizedQuote, rolling_window::RollingWindow};
use corelib::models::QuoteSide;

pub struct SpreadEngine {
    config: SpreadConfig,
    ask_window: RollingWindow<NormalizedQuote>,
    bid_window: RollingWindow<NormalizedQuote>,
}

impl SpreadEngine {
    pub fn new(config: SpreadConfig) -> Self {
        Self {
            ask_window: RollingWindow::new(config.window_ms),
            bid_window: RollingWindow::new(config.window_ms),
            config,
        }
    }
}

impl PulseEngine for SpreadEngine {
    /// Process a new normalized quote and check whether a spread pulse fires.
    ///
    /// Returns:
    ///     `Some(PulseSignal)` if a pulse is detected,
    ///     `None` if no conditions are met.
    ///
    /// Logic:
    /// 1. Insert quote into its rolling bid/ask window.
    /// 2. When both sides have recent quotes, compute spread.
    /// 3. Convert spread → basis points.
    /// 4. Compare against configured spread threshold.
    /// 5. Emit PulseSignal on success.
    fn on_quote(&mut self, q: NormalizedQuote) -> Option<PulseSignal> {
        match q.side {
            QuoteSide::Ask => self.ask_window.push(q.ts_ms, q.clone()),
            QuoteSide::Bid => self.bid_window.push(q.ts_ms, q.clone()),
        }

        let ask = self.ask_window.latest()?;
        let bid = self.bid_window.latest()?;

        // Check freshness — prevent stale mismatched quotes
        if (ask.ts_ms as i64 - bid.ts_ms as i64).abs() > self.config.window_ms as i64 {
            return None;
        }

        let spread = ask.price - bid.price;

        if ask.price <= 0.0 {
            return None;
        }

        let spread_bps = (spread / ask.price) * 10_000.0; // Convert spread to basis points

        // Pulse condition: spread must be BELOW threshold
        if spread_bps < self.config.spread_threshold_bps {
            return Some(PulseSignal {
                pulse_type: PulseType::Spread,
                details: PulseDetails::Spread {
                    bid: bid.price,
                    ask: ask.price,
                    spread_bps,
                    threshold_bps: self.config.spread_threshold_bps,
                    timestamp: ask.ts_ms.max(bid.ts_ms),
                },
            });
        }

        None
    }
}

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
    fn kind(&self) -> PulseType {
        PulseType::Spread
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::normalized_quote::NormalizedQuote;
    use corelib::models::QuoteSide;

    fn q(ts: u64, price: f64, side: QuoteSide) -> NormalizedQuote {
        NormalizedQuote {
            ts_ms: ts,
            price,
            amount_in: 1.0,
            amount_out: price,
            side,
        }
    }

    fn make_engine(window_ms: u64, threshold_bps: f64) -> SpreadEngine {
        SpreadEngine::new(SpreadConfig {
            window_ms,
            spread_threshold_bps: threshold_bps,
        })
    }

    // ---------------------------------------------------------
    // 1. Basic pulse detection when spread < threshold
    // ---------------------------------------------------------
    #[test]
    fn test_pulse_triggers_when_spread_below_threshold() {
        let mut engine = make_engine(1000, 5.0); // 5 bps threshold

        // Bid quote → cheaper
        let bid = q(1, 100.0, QuoteSide::Bid);

        // Ask quote → slightly more expensive
        let ask = q(2, 100.03, QuoteSide::Ask);

        engine.on_quote(bid);
        let signal = engine.on_quote(ask).expect("Expected pulse");

        if let PulseDetails::Spread { spread_bps, .. } = signal.details {
            assert!(spread_bps < 5.0); // 3 bps
        } else {
            panic!("Incorrect pulse type");
        }
    }

    // ---------------------------------------------------------
    // 2. No pulse when spread ABOVE threshold
    // ---------------------------------------------------------
    #[test]
    fn test_no_pulse_when_spread_above_threshold() {
        let mut engine = make_engine(1000, 2.0); // 2 bps threshold

        let bid = q(1, 100.0, QuoteSide::Bid);
        let ask = q(2, 100.10, QuoteSide::Ask); // 10 bps → too big

        engine.on_quote(bid);
        let signal = engine.on_quote(ask);

        assert!(signal.is_none(), "Spread too high → must not pulse");
    }

    // ---------------------------------------------------------
    // 3. No pulse when timestamps differ too much
    // ---------------------------------------------------------
    #[test]
    fn test_reject_stale_quotes() {
        let mut engine = make_engine(1000, 10.0);

        let bid = q(1, 100.0, QuoteSide::Bid);
        let ask = q(3000, 100.02, QuoteSide::Ask); // 2000ms difference → stale

        engine.on_quote(bid);
        let result = engine.on_quote(ask);

        assert!(result.is_none(), "Stale quotes must not trigger pulse");
    }

    // ---------------------------------------------------------
    // 4. Pulse only after both bid and ask exist
    // ---------------------------------------------------------
    #[test]
    fn test_waits_for_both_sides() {
        let mut engine = make_engine(1000, 5.0);

        let bid = q(1, 100.0, QuoteSide::Bid);
        let ask = q(2, 100.04, QuoteSide::Ask);

        // Only bid inserted
        let early = engine.on_quote(bid);
        assert!(early.is_none(), "Cannot pulse without ask");

        // Now ask comes
        let final_signal = engine.on_quote(ask);
        assert!(
            final_signal.is_some(),
            "Pulse should trigger once ask arrives"
        );
    }

    // ---------------------------------------------------------
    // 5. Zero or invalid ask price → skip
    // ---------------------------------------------------------
    #[test]
    fn test_ignores_zero_price() {
        let mut engine = make_engine(1000, 5.0);

        engine.on_quote(q(1, 100.0, QuoteSide::Bid));
        let result = engine.on_quote(q(2, 0.0, QuoteSide::Ask));

        assert!(result.is_none(), "Zero price ask must be ignored");
    }

    // ---------------------------------------------------------
    // 6. Exact threshold should NOT fire (strict <)
    // ---------------------------------------------------------
    #[test]
    fn test_no_pulse_when_spread_at_threshold_with_tolerance() {
        let mut engine = make_engine(1000, 5.0);
        let bid = q(1, 100.0, QuoteSide::Bid);
        let ask = q(2, 100.05, QuoteSide::Ask);

        engine.on_quote(bid);
        let signal = engine.on_quote(ask);

        if let Some(sig) = signal {
            let spread_bps = match sig.details {
                PulseDetails::Spread { spread_bps, .. } => spread_bps,
                _ => panic!("wrong pulse type"),
            };
            assert!(
                (spread_bps - 5.0).abs() < 0.01,
                "Spread should be approximately 5bps"
            );
        } else {
            panic!("Expected pulse based on approximate threshold");
        }
    }
}

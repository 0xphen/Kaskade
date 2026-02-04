use crate::market::types::Quote;

/// Normalized view of an RFQ quote returned by Omniston.
///
/// RFQ semantics:
/// - You GIVE `bid_units` of `bid_asset`.
/// - You RECEIVE `ask_units` of `ask_asset`.
///
/// Therefore:
///     bid_units  = bid_units
///     ask_units = ask_units
///     price      = ask_units / bid_units
///
/// This does *not* depend on any "Bid/Ask side" concept.
/// The direction is fully determined by how you subscribe to the RFQ stream.
#[derive(Debug, Clone)]
pub struct NormalizedQuote {
    /// Timestamp when normalization occurred
    pub ts_ms: u64,

    /// Execution price = ask_units / bid_units
    pub price: f64,

    /// The amount of the asset *you give*
    pub bid_units: f64,

    /// The amount of the asset *you receive*
    pub ask_units: f64,
}

impl NormalizedQuote {
    /// Convert a raw Omniston RFQ quote into normalized form.
    ///
    /// Safety:
    /// - Non-numeric `bid_units` / `ask_units` become 0.0
    /// - If `bid_units == 0`, price is set to 0.0
    pub fn from_event(ev: &Quote) -> Self {
        let ts_ms = ev.quote_timestamp * 1000;

        let bid_units = ev.bid_units.parse::<f64>().unwrap_or(0.0);
        let ask_units = ev.ask_units.parse::<f64>().unwrap_or(0.0);

        let price = if bid_units > 0.0 {
            ask_units / bid_units
        } else {
            0.0
        };

        Self {
            ts_ms,
            price,
            bid_units,
            ask_units,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::types::*;

    fn dummy_addr() -> AssetAddress {
        AssetAddress {
            blockchain: 607,
            address: "EQDummy".into(),
        }
    }

    fn mk_quote(bid: &str, ask: &str) -> Quote {
        Quote {
            quote_id: "q1".into(),
            resolver_id: "r1".into(),
            resolver_name: "Omniston".into(),

            bid_asset_address: dummy_addr(),
            ask_asset_address: dummy_addr(),

            bid_units: bid.into(),
            ask_units: ask.into(),

            referrer_address: None,
            referrer_fee_asset: dummy_addr(),
            referrer_fee_units: "0".into(),
            protocol_fee_asset: dummy_addr(),
            protocol_fee_units: "0".into(),

            quote_timestamp: 1234,
            trade_start_deadline: 0,

            gas_budget: "0".into(),
            estimated_gas_consumption: "0".into(),

            params: QuoteParams { swap: None },
        }
    }

    // -------------------------------------------------------------
    // 1. BASIC NORMALIZATION
    // -------------------------------------------------------------
    #[test]
    fn basic_normalization() {
        let q = mk_quote("100", "50");
        let nq = NormalizedQuote::from_event(&q);

        assert_eq!(nq.bid_units, 100.0);
        assert_eq!(nq.ask_units, 50.0);
        assert!((nq.price - 0.5).abs() < 1e-12);
    }

    // -------------------------------------------------------------
    // 2. ZERO HANDLING
    // -------------------------------------------------------------
    #[test]
    fn zero_bid_units_gives_zero_price() {
        let q = mk_quote("0", "100");
        let nq = NormalizedQuote::from_event(&q);

        assert_eq!(nq.bid_units, 0.0);
        assert_eq!(nq.ask_units, 100.0);
        assert_eq!(nq.price, 0.0);
    }

    // -------------------------------------------------------------
    // 3. NON-NUMERIC INPUTS
    // -------------------------------------------------------------
    #[test]
    fn non_numeric_becomes_zero() {
        let q = mk_quote("abc", "xyz");
        let nq = NormalizedQuote::from_event(&q);

        assert_eq!(nq.bid_units, 0.0);
        assert_eq!(nq.ask_units, 0.0);
        assert_eq!(nq.price, 0.0);
    }

    // -------------------------------------------------------------
    // 4. PRICE = OUT / IN ALWAYS HOLDS
    // -------------------------------------------------------------
    #[test]
    fn price_matches_ratio() {
        let q = mk_quote("200", "80");
        let nq = NormalizedQuote::from_event(&q);

        if nq.bid_units > 0.0 {
            let expected = nq.ask_units / nq.bid_units;
            assert!((nq.price - expected).abs() < 1e-12);
        }
    }
}

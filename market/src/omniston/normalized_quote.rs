use crate::types::Quote;
use chrono::Utc;

/// Normalized view of an RFQ quote returned by Omniston.
///
/// RFQ semantics:
/// - You GIVE `bid_units` of `bid_asset`.
/// - You RECEIVE `ask_units` of `ask_asset`.
///
/// Therefore:
///     amount_in  = bid_units
///     amount_out = ask_units
///     price      = amount_out / amount_in
///
/// This does *not* depend on any "Bid/Ask side" concept.
/// The direction is fully determined by how you subscribe to the RFQ stream.
#[derive(Debug, Clone)]
pub struct NormalizedQuote {
    /// Timestamp when normalization occurred
    pub ts_ms: u64,

    /// Execution price = amount_out / amount_in
    pub price: f64,

    /// The amount of the asset *you give*
    pub amount_in: f64,

    /// The amount of the asset *you receive*
    pub amount_out: f64,
}

impl NormalizedQuote {
    /// Convert a raw Omniston RFQ quote into normalized form.
    ///
    /// Safety:
    /// - Non-numeric `bid_units` / `ask_units` become 0.0
    /// - If `amount_in == 0`, price is set to 0.0
    pub fn from_event(ev: &Quote) -> Self {
        let ts_ms = Utc::now().timestamp_millis() as u64;

        let amount_in = ev.bid_units.parse::<f64>().unwrap_or(0.0);
        let amount_out = ev.ask_units.parse::<f64>().unwrap_or(0.0);

        let price = if amount_in > 0.0 {
            amount_out / amount_in
        } else {
            0.0
        };

        Self {
            ts_ms,
            price,
            amount_in,
            amount_out,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

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

            quote_timestamp: 0,
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

        assert_eq!(nq.amount_in, 100.0);
        assert_eq!(nq.amount_out, 50.0);
        assert!((nq.price - 0.5).abs() < 1e-12);
    }

    // -------------------------------------------------------------
    // 2. ZERO HANDLING
    // -------------------------------------------------------------
    #[test]
    fn zero_amount_in_gives_zero_price() {
        let q = mk_quote("0", "100");
        let nq = NormalizedQuote::from_event(&q);

        assert_eq!(nq.amount_in, 0.0);
        assert_eq!(nq.amount_out, 100.0);
        assert_eq!(nq.price, 0.0);
    }

    // -------------------------------------------------------------
    // 3. NON-NUMERIC INPUTS
    // -------------------------------------------------------------
    #[test]
    fn non_numeric_becomes_zero() {
        let q = mk_quote("abc", "xyz");
        let nq = NormalizedQuote::from_event(&q);

        assert_eq!(nq.amount_in, 0.0);
        assert_eq!(nq.amount_out, 0.0);
        assert_eq!(nq.price, 0.0);
    }

    // -------------------------------------------------------------
    // 4. PRICE = OUT / IN ALWAYS HOLDS
    // -------------------------------------------------------------
    #[test]
    fn price_matches_ratio() {
        let q = mk_quote("200", "80");
        let nq = NormalizedQuote::from_event(&q);

        if nq.amount_in > 0.0 {
            let expected = nq.amount_out / nq.amount_in;
            assert!((nq.price - expected).abs() < 1e-12);
        }
    }

    // -------------------------------------------------------------
    // 5. TIMESTAMP MUST BE NON-ZERO AND INCREASING
    // -------------------------------------------------------------
    #[test]
    fn timestamp_validity() {
        let q = mk_quote("100", "50");

        let n1 = NormalizedQuote::from_event(&q);
        let n2 = NormalizedQuote::from_event(&q);

        assert!(n1.ts_ms > 0);
        assert!(n2.ts_ms > 0);
        assert!(n2.ts_ms >= n1.ts_ms);
    }
}

use corelib::QuoteSide;
use corelib::omniston_models::Quote;

/// A normalized view of an Omniston RFQ quote.
///
/// Omniston `Quote` objects use two directional units:
/// - `bid_units`: the amount of `bid_asset` involved
/// - `ask_units`: the amount of `ask_asset` involved
///
/// However, depending on the RFQ direction (Bid or Ask), these fields represent
/// different economic meanings.
///
/// ## Normalization Rules
///
/// ### `QuoteSide::Bid`
/// Represents *selling* the bid asset to receive the ask asset.
/// ```text
/// amount_in  = bid_units
/// amount_out = ask_units
/// price      = ask_units / bid_units
/// ```
///
/// ### `QuoteSide::Ask`
/// Represents *buying* the ask asset using the bid asset.
/// ```text
/// amount_in  = ask_units
/// amount_out = bid_units
/// price      = bid_units / ask_units
/// ```
///
/// This struct unifies both directions into a single representation:
/// ```text
/// amount_in  -> amount_out @ price
/// ```
///
/// This is the canonical representation used by spread engines,
/// rolling windows, and trading models.
#[derive(Debug, Clone)]
pub struct NormalizedQuote {
    /// Timestamp (UTC) when this quote was normalized, in milliseconds.
    pub ts_ms: u64,

    /// Normalized price (amount_out / amount_in).
    pub price: f64,

    /// Input amount for this quote (always the `asset you give`).
    pub amount_in: f64,

    /// Output amount for this quote (always the `asset you get`).
    pub amount_out: f64,

    /// Whether this quote originated from a bid-side or ask-side RFQ.
    pub side: QuoteSide,
}

impl NormalizedQuote {
    /// Convert an Omniston `Quote` into a normalized form usable across engines.
    ///
    /// Automatically interprets Omniston `bid_units` and `ask_units`
    /// depending on RFQ direction (`QuoteSide`).
    ///
    /// # Arguments
    /// - `ev`: Raw Omniston quote
    ///
    /// # Returns
    /// A fully normalized `NormalizedQuote` with price and amounts resolved.
    ///
    /// # Panics
    /// Never panics. Invalid numeric strings are treated as 0.0.
    pub fn from_event(ev: &Quote) -> Self {
        let now = chrono::Utc::now().timestamp_millis() as u64;

        let bid = ev.bid_units.parse::<f64>().unwrap_or(0.0);
        let ask = ev.ask_units.parse::<f64>().unwrap_or(0.0);

        let (amount_in, amount_out, price) = match ev.side {
            QuoteSide::Bid => {
                // Selling bid_asset → receiving ask_asset
                let amount_in = bid;
                let amount_out = ask;
                let price = if amount_in > 0.0 {
                    amount_out / amount_in
                } else {
                    0.0
                };
                (amount_in, amount_out, price)
            }

            QuoteSide::Ask => {
                // Buying ask_asset → paying bid_asset
                let amount_in = ask;
                let amount_out = bid;
                let price = if amount_in > 0.0 {
                    amount_out / amount_in
                } else {
                    0.0
                };
                (amount_in, amount_out, price)
            }
        };

        Self {
            ts_ms: now,
            price,
            amount_in,
            amount_out,
            side: ev.side.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corelib::models::QuoteSide;
    use corelib::omniston_models::{AssetAddress, Quote, QuoteParams};

    fn dummy_addr() -> AssetAddress {
        AssetAddress {
            blockchain: 607,
            address: "EQDummy".into(),
        }
    }

    fn make_quote(bid: &str, ask: &str, side: QuoteSide) -> Quote {
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

            side, // <-- REQUIRED
        }
    }

    #[test]
    fn test_bid_side_normalization() {
        // Bid side → amount_in = bid_units
        let q = make_quote("100", "50", QuoteSide::Bid);

        let nq = NormalizedQuote::from_event(&q);

        assert_eq!(nq.amount_in, 100.0);
        assert_eq!(nq.amount_out, 50.0);
        assert_eq!(nq.price, 0.5);
    }

    #[test]
    fn test_ask_side_normalization() {
        // Ask side → amount_in = ask_units
        let q = make_quote("50", "100", QuoteSide::Ask);

        let nq = NormalizedQuote::from_event(&q);

        assert_eq!(nq.amount_in, 100.0); // ask_units = what user is giving
        assert_eq!(nq.amount_out, 50.0); // bid_units = what user receives
        assert_eq!(nq.price, 0.5);
    }

    #[test]
    fn test_zero_behavior() {
        // Zero units (still Ask or Bid side is explicit)
        let q = make_quote("0", "100", QuoteSide::Ask);

        let nq = NormalizedQuote::from_event(&q);

        assert_eq!(nq.amount_in, 100.0); // ask side → use ask_units
        assert_eq!(nq.amount_out, 0.0);
        assert_eq!(nq.price, 0.0);
    }
}

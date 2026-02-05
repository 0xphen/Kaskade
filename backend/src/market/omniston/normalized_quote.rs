use crate::market::types::{ExecutionScope, Quote, RouteChunk};

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
#[derive(Debug, Clone, Default)]
pub struct NormalizedQuote {
    /// Timestamp when normalization occurred (milliseconds)
    pub ts_ms: u64,

    /// Execution price = ask_units / bid_units
    pub price: f64,

    /// The amount of the asset *you give*
    pub bid_units: f64,

    /// The amount of the asset *you receive*
    pub ask_units: f64,
}

impl NormalizedQuote {
    /// Convert a raw Omniston RFQ quote into normalized form based on the execution scope.
    ///
    /// If scope is `MarketWide`, it uses the total quote amounts.
    /// If scope is `ProtocolOnly`, it aggregates only chunks belonging to that protocol.
    pub fn from_event(ev: &Quote, scope: &ExecutionScope) -> Self {
        match scope {
            ExecutionScope::MarketWide => Self::market_scope(ev),
            ExecutionScope::ProtocolOnly { protocol } => Self::protocol_only_scope(ev, protocol),
        }
    }

    /// Normalizes the quote using the aggregate totals provided by the resolver.
    fn market_scope(ev: &Quote) -> Self {
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

    /// Normalizes the quote by isolating liquidity from a specific protocol across all possible routes.
    ///
    /// Since Omniston can return multiple alternative execution paths (routes), we must
    /// traverse all of them to find the specific protocol's contribution. We select the
    /// route that provides the highest liquidity for the target protocol.
    fn protocol_only_scope(quote: &Quote, protocol_name: &str) -> Self {
        let ts_ms = quote.quote_timestamp * 1000;

        // We track the 'best' version of the protocol's liquidity found across different routes.
        // In many RFQs, routes are alternative options (OR logic), so we pick the strongest one.
        let mut best_stonfi_bid = 0.0;
        let mut best_stonfi_ask = 0.0;

        if let Some(swap_params) = &quote.params.swap {
            for route in &swap_params.routes {
                let mut current_route_bid = 0.0;
                let mut current_route_ask = 0.0;

                // Iterate through each hop (step) in the current path
                for step in &route.steps {
                    for chunk in &step.chunks {
                        if chunk.protocol.contains(protocol_name) {
                            // Accumulate the liquidity units for this specific protocol within this route
                            current_route_bid += chunk.bid_amount.parse::<f64>().unwrap_or(0.0);
                            current_route_ask += chunk.ask_amount.parse::<f64>().unwrap_or(0.0);
                        }
                    }
                }

                // After checking all steps in a route, compare it to the best protocol-specific
                // route found so far. We prioritize the route with the most 'bid_units' (depth).
                if current_route_bid > best_stonfi_bid {
                    best_stonfi_bid = current_route_bid;
                    best_stonfi_ask = current_route_ask;
                }
            }
        }

        // Calculate the protocol-native price.
        // This is isolated from the global market price of the quote.
        let price = if best_stonfi_bid > 0.0 {
            best_stonfi_ask / best_stonfi_bid
        } else {
            0.0
        };

        Self {
            ts_ms,
            price,
            bid_units: best_stonfi_bid,
            ask_units: best_stonfi_ask,
        }
    }

    /// Helper to create a NormalizedQuote from a single specific chunk.
    pub fn from_chunk(chunk: &RouteChunk, ts_ms: u64) -> Self {
        let bid_units = chunk.bid_amount.parse::<f64>().unwrap_or(0.0);
        let ask_units = chunk.ask_amount.parse::<f64>().unwrap_or(0.0);

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

    /// Helper to build a complex route for testing
    fn mk_route(chunks: Vec<(&str, &str, &str)>) -> Route {
        Route {
            steps: vec![RouteStep {
                bid_asset_address: dummy_addr(),
                ask_asset_address: dummy_addr(),
                chunks: chunks
                    .into_iter()
                    .map(|(p, b, a)| RouteChunk {
                        protocol: p.into(),
                        bid_amount: b.into(),
                        ask_amount: a.into(),
                        extra_version: 1,
                        extra: vec![],
                    })
                    .collect(),
            }],
        }
    }

    fn mk_base_quote() -> Quote {
        Quote {
            quote_id: "q1".into(),
            resolver_id: "r1".into(),
            resolver_name: "Omniston".into(),
            bid_asset_address: dummy_addr(),
            ask_asset_address: dummy_addr(),
            bid_units: "0".into(),
            ask_units: "0".into(),
            referrer_address: None,
            referrer_fee_asset: dummy_addr(),
            referrer_fee_units: "0".into(),
            protocol_fee_asset: dummy_addr(),
            protocol_fee_units: "0".into(),
            quote_timestamp: 1000,
            trade_start_deadline: 0,
            gas_budget: "0".into(),
            estimated_gas_consumption: "0".into(),
            params: QuoteParams { swap: None },
        }
    }

    #[test]
    fn test_protocol_only_multi_route_selection() {
        let mut q = mk_base_quote();

        // Route 0: DeDust (Higher total depth, but wrong protocol)
        let r0 = mk_route(vec![("DeDust", "500", "250")]);
        // Route 1: StonFi (Target protocol)
        let r1 = mk_route(vec![("StonFiV2", "100", "50")]);

        q.params.swap = Some(SwapParams {
            routes: vec![r0, r1],
            min_ask_amount: "50".into(),
            recommended_min_ask_amount: "50".into(),
            recommended_slippage_bps: 10,
        });

        let scope = ExecutionScope::ProtocolOnly {
            protocol: "StonFi".into(),
        };
        let nq = NormalizedQuote::from_event(&q, &scope);

        // Should ignore Route 0 and correctly find StonFi in Route 1
        assert_eq!(nq.bid_units, 100.0);
        assert_eq!(nq.ask_units, 50.0);
        assert_eq!(nq.price, 0.5);
    }

    #[test]
    fn test_protocol_only_multi_hop_aggregation() {
        let mut q = mk_base_quote();

        // A single route where StonFi is used in two different chunks/hops
        let r0 = mk_route(vec![
            ("StonFiV1", "100", "50"),
            ("StonFiV2", "200", "100"),
            ("DeDust", "999", "999"), // Should be ignored
        ]);

        q.params.swap = Some(SwapParams {
            routes: vec![r0],
            min_ask_amount: "150".into(),
            recommended_min_ask_amount: "150".into(),
            recommended_slippage_bps: 10,
        });

        let scope = ExecutionScope::ProtocolOnly {
            protocol: "StonFi".into(),
        };
        let nq = NormalizedQuote::from_event(&q, &scope);

        // Should sum both StonFi chunks: 100 + 200 = 300
        assert_eq!(nq.bid_units, 300.0);
        assert_eq!(nq.ask_units, 150.0);
        assert_eq!(nq.price, 0.5);
    }

    #[test]
    fn test_non_numeric_graceful_failure() {
        let mut q = mk_base_quote();
        let r0 = mk_route(vec![("StonFi", "not_a_number", "50")]);

        q.params.swap = Some(SwapParams {
            routes: vec![r0],
            min_ask_amount: "0".into(),
            recommended_min_ask_amount: "0".into(),
            recommended_slippage_bps: 10,
        });

        let scope = ExecutionScope::ProtocolOnly {
            protocol: "StonFi".into(),
        };
        let nq = NormalizedQuote::from_event(&q, &scope);

        // Should not panic, should return 0.0 for bid
        assert_eq!(nq.bid_units, 0.0);
        assert_eq!(nq.price, 0.0);
    }
}

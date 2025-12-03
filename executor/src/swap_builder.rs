//! Omniston gRPC adapter for building swap transfers.
//!
//! This module is responsible for:
//!   - Taking `Session`, `Pair`, `amount_in`, and `MarketMetrics`
//!   - Calling the Omniston `buildTransfer` / RFQ swap API
//!   - Mapping the response into `BuiltTransfer` (BOCs + est_out)

use async_trait::async_trait;

use crate::types::{BuiltTransfer, ExecutionError, SwapBuilder, TonMessage};
use market::types::{MarketMetrics, Pair};
use session::model::Session;

pub struct OmnistonSwapBuilder<C> {
    pub client: C,
}

impl<C> OmnistonSwapBuilder<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }
}

#[async_trait]
impl<C> SwapBuilder for OmnistonSwapBuilder<C>
where
    C: Send + Sync,
{
    async fn build_transfer(
        &self,
        _session: &Session,
        _pair: &Pair,
        amount_in: u64,
        _metrics: &MarketMetrics,
    ) -> Result<BuiltTransfer, ExecutionError> {
        // TODO: integrate your real Omniston gRPC client:
        //
        //   1. Decide direction (base -> quote) from `pair` + session config.
        //   2. Call RFQ quote / buildTransfer:
        //        - amount_in
        //        - slippage params
        //        - chosen route / resolver
        //   3. Map response:
        //        - Vec<TonMessage> from BOCs in response
        //        - expected_amount_out from quote
        //
        // For now, return a stub so the executor compiles and can be tested.

        let fake_msg = TonMessage {
            boc: format!("FAKE_BOC_FOR_{}_IN", amount_in),
        };

        Ok(BuiltTransfer {
            messages: vec![fake_msg],
            expected_amount_out: amount_in, // 1:1 placeholder
        })
    }
}

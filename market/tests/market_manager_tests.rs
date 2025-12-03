use std::sync::Arc;
use tokio::sync::mpsc;

use market::{
    manager::MarketManager,
    omniston::OmnistonApi,
    types::{
        AssetAddress, MarketMetrics, OmnistonEvent, Pair, Quote, QuoteParams, RfqAmount,
        RfqRequest, SubscriptionRequest,
    },
};

#[derive(Clone)]
struct MockOmnistonClient;

#[async_trait::async_trait]
impl OmnistonApi for MockOmnistonClient {
    async fn request_for_quote_stream(
        &self,
        _req: RfqRequest,
        sender: mpsc::Sender<OmnistonEvent>,
    ) -> anyhow::Result<()> {
        // simulate async quote delivery
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;

            let q = Quote {
                quote_id: "q1".into(),
                resolver_id: "mock".into(),
                resolver_name: "mock".into(),

                bid_asset_address: dummy_addr(),
                ask_asset_address: dummy_addr(),

                bid_units: "100".into(),
                ask_units: "90".into(),

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
            };

            let _ = sender.send(OmnistonEvent::QuoteUpdated(Box::new(q))).await;
        });

        Ok(())
    }
}

fn dummy_addr() -> AssetAddress {
    AssetAddress {
        blockchain: 607,
        address: "EQDummy".into(),
    }
}

/// Utility: create test pair
fn test_pair() -> Pair {
    Pair {
        base: "TON".into(),
        quote: "STON".into(),
    }
}

#[tokio::test]
async fn subscription_registers_correctly() {
    let mm = MarketManager::new(Arc::new(MockOmnistonClient), 5000);

    let (tx, _rx) = mpsc::channel(10);

    let req = SubscriptionRequest {
        pair: test_pair(),
        amount: RfqAmount::BidUnits("1000".into()),
        sender_ch: tx,
    };

    let mm2 = mm.clone();
    mm2.subscribe(req).await;

    let subs_map = mm.subscribers.lock().await;

    assert!(subs_map.contains_key(&test_pair()));
    assert_eq!(subs_map[&test_pair()].len(), 1);
}

#[tokio::test]
async fn spread_metrics_are_updated_from_event_stream() {
    let mm = MarketManager::new(Arc::new(MockOmnistonClient), 5000);

    let (tx, rx) = mpsc::channel(10);

    // manually spawn processor (bypasses subscribe())
    let mm_clone = Arc::clone(&mm);
    let pair = test_pair();
    tokio::spawn(async move {
        mm_clone.process_event_stream(rx, pair).await;
    });

    // Simulate sending a QuoteUpdated event
    let q = Quote {
        quote_id: "q1".into(),
        resolver_id: "mock".into(),
        resolver_name: "mock".into(),

        bid_asset_address: dummy_addr(),
        ask_asset_address: dummy_addr(),

        bid_units: "100".into(),
        ask_units: "90".into(),

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
    };

    tx.send(OmnistonEvent::QuoteUpdated(Box::new(q)))
        .await
        .unwrap();

    // Give async code time to process
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let states_map = mm.states.lock().await;

    let metrics = states_map.get(&test_pair()).unwrap();

    assert!(metrics.spread.p_now > 0.0);
    assert!(metrics.spread.p_best > 0.0);
}

#[tokio::test]
async fn subscribers_receive_broadcasted_snapshots() {
    let mm = MarketManager::new(Arc::new(MockOmnistonClient), 5000);
    let pair = test_pair();

    let (sub_tx, mut sub_rx) = mpsc::channel(10);

    // Subscribe normally
    let req = SubscriptionRequest {
        pair: pair.clone(),
        amount: RfqAmount::BidUnits("1000".into()),
        sender_ch: sub_tx,
    };
    mm.clone().subscribe(req).await;

    // Wait for initial tasks to start
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Expect a broadcast from mock client
    let snapshot = sub_rx.recv().await.expect("did not receive snapshot");

    assert!(snapshot.spread.p_now > 0.0);
}

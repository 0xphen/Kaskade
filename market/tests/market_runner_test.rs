// //! Integration test runner for MarketManager.
// //!
// //! This test:
// //!   • builds a mock Omniston client
// //!   • starts MarketManager
// //!   • subscribes to a pair
// //!   • logs all market snapshots received
// //!
// //! This verifies:
// //!   • RFQ stream task runs
// //!   • event processor runs
// //!   • snapshots are broadcast to subscribers

// use std::sync::Arc;
// use tokio::sync::mpsc;

// use market::{
//     manager::MarketManager,
//     omniston::rfq_ws::OmnistonWsClient,
//     types::{MarketMetrics, Pair, RfqAmount, SubscriptionRequest},
// };

// #[tokio::test]
// async fn mini_market_runner() {
//     let mock_client = Arc::new(OmnistonWsClient::new("wss://omni-ws.ston.fi".into()));
//     let market = MarketManager::new(mock_client);

//     // -----------------------------------------------
//     // 2) Prepare a subscriber channel
//     // -----------------------------------------------
//     let (tx, mut rx) = mpsc::channel::<MarketMetrics>(100);

//     let pair = Pair::new("TON".into(), "STON".into());

//     let req = SubscriptionRequest {
//         pair: pair.clone(),
//         amount: RfqAmount::BidUnits("1000000".into()),
//         sender_ch: tx,
//     };

//     println!("🧪 Subscribing to {}", pair.id());
//     market.clone().subscribe(req).await;

//     // -----------------------------------------------
//     // 3) Collect a few market snapshots
//     // -----------------------------------------------
//     let mut count = 0;
//     println!("🔎 Test consumer waiting for snapshots…");

//     while let Some(snapshot) = rx.recv().await {
//         println!("📈 Snapshot {} received: {:?}", count + 1, snapshot);
//         count += 1;

//         if count >= 5 {
//             println!("✅ Received enough snapshots, stopping.");
//             break;
//         }
//     }

//     assert!(count > 0, "Expected to receive at least one snapshot");
// }

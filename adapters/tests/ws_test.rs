// use adapters::omniston::types::QuoteEvent;
// use adapters::omniston::ws::OmnistonWsClient;
// use tokio::sync::mpsc;

// #[tokio::test]
// async fn test_omniston_ws_manual() {
//     let ws_url = "wss://omni-ws.ston.fi";
//     let client = OmnistonWsClient::new(ws_url.to_string());

//     let (sender, mut receiver) = mpsc::channel::<QuoteEvent>(100);

//     // spawn the websocket loop
//     tokio::spawn(async move {
//         let _ = client
//             .run_ws_loop(
//                 "EQCxE6mUtQJKFnGfaROTKOt1lZbDiiX1kCixRv7Nw2Id_sDs",
//                 "EQC98_qAmNEptUtPc7W6xdHh_ZHrBUFpw5Ft_IzNU20QAJav",
//                 Some("100000000"),
//                 None, // or 1000000 depending on decimals
//                 sender,
//             )
//             .await;
//     });

//     // 👀 Read & print messages as they arrive
//     // We do NOT assert—this is a manual live WS test.
//     for _ in 0..10 {
//         if let Some(event) = receiver.recv().await {
//             println!("🎉 RECEIVED QUOTE EVENT:\n{:#?}", event);
//         } else {
//             println!("⚠ Channel closed");
//             break;
//         }
//     }

//     // Test completes automatically after printing events
// }

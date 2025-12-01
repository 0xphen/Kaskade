// use adapters::omniston::{
//     api::{OmnistonApi, RfqAmount, RfqRequest},
//     models::OmnistonEvent,
//     ws_client::OmnistonWsClient,
// };
// use tokio::sync::mpsc;

// #[tokio::test]
// async fn test_rfq_stream() {
//     let client = OmnistonWsClient::new("wss://omni-ws.ston.fi".into());

//     let (tx, mut rx) = mpsc::channel(50);

//     let req = RfqRequest {
//         bid_asset: "EQCxE6mUtQJKFnGfaROTKOt1lZbDiiX1kCixRv7Nw2Id_sDs".into(),
//         ask_asset: "EQC98_qAmNEptUtPc7W6xdHh_ZHrBUFpw5Ft_IzNU20QAJav".into(),
//         amount: RfqAmount::BidUnits("1000000".into()),
//     };

//     tokio::spawn(async move {
//         let _ = client.request_for_quote_stream(req, tx).await;
//     });

//     while let Some(ev) = rx.recv().await {
//         println!("EVENT: {ev:?}");
//     }
// }

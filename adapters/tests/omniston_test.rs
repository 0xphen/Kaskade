// use adapters::omniston::{
//     api::{OmnistonApi, RfqAmount, RfqRequest},
//     ws_client::OmnistonWsClient,
// };
// use corelib::omniston_models::OmnistonEvent;
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

// pub async fn run_pulse_engine<E: PulseEngine>(
//     mut engine: E,
//     mut recv: Receiver<NormalizedQuote>,
// ) {
//     while let Some(quote) = recv.recv().await {
//         if let Some(signal) = engine.on_quote(quote).await {
//             println!("⚡ Pulse triggered: {:?}", signal);
//             // send to swap executor...
//         }
//     }
// }

use adapters::omniston::types::QuoteEvent;
use adapters::omniston::ws::OmnistonWsClient;
use tokio::sync::mpsc;

#[tokio::test]
async fn test_omniston_ws_manual() {
    let ws_url = "wss://omni-ws.ston.fi";
    let client = OmnistonWsClient::new(ws_url.to_string());

    let (sender, mut receiver) = mpsc::channel::<QuoteEvent>(100);

    // spawn the websocket loop
    tokio::spawn(async move {
        let _ = client
            .run_ws_loop(
                // TON → jUSDT example
                "EQDB8JYMzpiOxjCx7leP5nYkchF72PdbWT1LV7ym1uAedINh",
                "EQCH-yP4S3nA_j7K7EIV1QIhVTWMyNJfxeYzacUH76ks2hUF",
                Some("1000000000"), // bid_units = 1 USDT
                None,
                sender,
            )
            .await;
    });

    // 👀 Read & print messages as they arrive
    // We do NOT assert—this is a manual live WS test.
    for _ in 0..10 {
        if let Some(event) = receiver.recv().await {
            println!("🎉 RECEIVED QUOTE EVENT:\n{:#?}", event);
        } else {
            println!("⚠ Channel closed");
            break;
        }
    }

    // Test completes automatically after printing events
}

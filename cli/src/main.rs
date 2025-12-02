pub mod cli;

use clap::Parser;
use std::sync::Arc;
use std::sync::Mutex;

use adapters::omniston::ws_client::OmnistonWsClient;
use cli::*;
use engine::dispatcher;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let registry = Arc::new(Mutex::new(build_registry_from_cli(&cli)));

    let (quote_tx, quote_rx) = tokio::sync::mpsc::channel(1024);

    let registry_clone = Arc::clone(&registry);
    tokio::spawn(async move {
        dispatcher::run_dispatcher(quote_rx, registry_clone).await;
    });

    let client = OmnistonWsClient::new("wss://omni-ws.ston.fi".into());

    // 4. Start WebSocket client, normalize quotes, send into quote_tx
    // omniston_ws_client.run_and_normalize(quote_tx).await?;

    Ok(())
}

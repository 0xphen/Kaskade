use std::sync::Arc;
use std::time::Duration;

use backend::{
    config::AppConfig,
    db::Db,
    execution::{
        executor::{PairExecutorRouter, SwapExecutor},
        recover_uncommitted,
        types::{self, ExecutionEvent, SwapReceipt},
    },
    logger::init_tracing,
    market::manager::MarketManager,
    market::omniston::rfq_ws::OmnistonWsClient,
    market::types::{MarketMetrics, Pair, RfqAmount, SubscriptionRequest},
    market_view::{MarketViewStore, types::MarketMetricsView},
    metrics::counters::Counters,
    scheduler::scheduler::Scheduler,
    session::repository_sqlx::SqlxSessionRepository,
    session::store::SessionStore,
    time::now_ms,
};
use tokio::sync::mpsc;

#[derive(Clone)]
struct DummySwapExecutor;

#[async_trait::async_trait]
impl SwapExecutor for DummySwapExecutor {
    async fn execute_swap(&self, call: types::SwapCall) -> anyhow::Result<SwapReceipt> {
        // TODO: Replace with real TON / EMC execution.
        // Map chain errors into stable strings for classify_error(), e.g:
        // - market closed => Err(anyhow::anyhow!("MarketNotOpen"))
        // - slippage => Err(anyhow::anyhow!("Slippage"))
        let _ = call;
        Ok(SwapReceipt {
            tx_id: "dummy_tx".to_string(),
        })
    }
}

/// Initializes DB, runs migrations, constructs repository/store, and performs
/// restart recovery to unwind any RESERVED-but-uncommitted batches.
async fn init_store(cfg: &AppConfig) -> anyhow::Result<Arc<SessionStore>> {
    let db = Db::connect(&cfg.database_url).await?;
    db.migrate().await?;

    let repo = Arc::new(SqlxSessionRepository::new(db.pool.clone()));
    let store = Arc::new(SessionStore::new(repo));

    // Safety: unwind in-flight leakage from RESERVED batches on restart.
    recover_uncommitted(&store).await?;

    Ok(store)
}

/// Starts the per-pair executor router and returns the scheduler->router sender.
fn start_executor_router(
    store: Arc<SessionStore>,
    market_view: MarketViewStore,
    cfg: &AppConfig,
) -> mpsc::Sender<ExecutionEvent> {
    let (exec_tx, exec_rx) = mpsc::channel::<ExecutionEvent>(cfg.exec_queue_capacity);

    let exec_impl = Arc::new(DummySwapExecutor);

    let router = Arc::new(PairExecutorRouter::new(
        store,
        market_view,
        exec_impl,
        cfg.default_failure_cooldown_ms,
        128, // per-pair queue capacity
    ));

    tokio::spawn(router.run(exec_rx));

    exec_tx
}

/// Starts the scheduler loop (fixed cadence). Each tick reads the latest market snapshot
/// and calls scheduler.on_tick().
fn start_scheduler_loop(
    scheduler: Scheduler,
    market_view: MarketViewStore,
    exec_tx: mpsc::Sender<ExecutionEvent>,
    pair_id: String,
    interval: Duration,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);

        loop {
            ticker.tick().await;

            let Some(market) = market_view.get(&pair_id).await else {
                // No market snapshot yet → skip scheduling.
                continue;
            };

            if let Err(e) = scheduler
                .on_tick(&pair_id, market, exec_tx.clone(), now_ms())
                .await
            {
                tracing::error!(error=?e, pair_id=%pair_id, "scheduler tick failed");
            }
        }
    });
}

/// Starts Omniston market subscription and continuously updates MarketViewStore.
fn start_market_feed(market_view: MarketViewStore, pair: Pair) {
    let pair_id = pair.id();

    let omniston_ws_client = Arc::new(OmnistonWsClient::new("wss://omni-ws.ston.fi".into()));
    let market = MarketManager::new(omniston_ws_client);

    let (tx, mut rx) = mpsc::channel::<MarketMetrics>(100);

    let req = SubscriptionRequest {
        pair: pair.clone(),
        amount: RfqAmount::BidUnits("200000000".into()),
        sender_ch: tx,
    };

    tokio::spawn(async move {
        market.subscribe(req).await;
    });

    // Consume metrics and update the shared MarketViewStore.
    tokio::spawn(async move {
        while let Some(snapshot) = rx.recv().await {
            let view = MarketMetricsView {
                spread_bps: snapshot.spread.spread_bps,
                trend_drop_bps: snapshot.trend.trend_drop_bps,
                slippage_bps: snapshot.slippage.slippage_bps,
                depth_now_in: snapshot.depth.depth_now as u128,
                ts_ms: snapshot.ts_ms,
            };

            market_view.set(&pair_id, view).await;
        }

        // If rx closes, the market feed task died or unsubscribed.
        tracing::warn!(pair_id=%pair_id, "market feed channel closed; market updates stopped");
    });
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    sqlx::any::install_default_drivers();

    let is_production = std::env::var("APP_ENV").unwrap_or_default() == "production";
    init_tracing(is_production);

    tracing::info!("Starting Kaskade backend...");

    let cfg = AppConfig::from_env();

    // Choose the pair you want to run (single-pair bootstrap).
    let pair = Pair::new("TON".into(), "STON".into());
    let pair_id = pair.id();

    let market_view = MarketViewStore::new();

    let store = init_store(&cfg).await?;

    let exec_tx = start_executor_router(store.clone(), market_view.clone(), &cfg);

    let scheduler = Scheduler::new(
        store,
        cfg.scheduler_candidate_min,
        cfg.scheduler_max_attempts,
        cfg.scheduler_max_users_per_batch,
        Counters::default(),
    );

    start_scheduler_loop(
        scheduler,
        market_view.clone(),
        exec_tx,
        pair_id.clone(),
        Duration::from_millis(250),
    );

    start_market_feed(market_view, pair);

    tracing::info!(pair_id=%pair_id, "Backend started; waiting for shutdown signal");

    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutdown signal received");

    Ok(())
}

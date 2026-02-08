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
    market::{market_view_store::MarketViewStore, stonfi::StonfiClient, types::Pair},
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
                // No market snapshot yet -> skip scheduling.
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

fn setup_market_manager(market_view: MarketViewStore, cfg: &AppConfig) -> MarketManager {
    let stonfi_client = StonfiClient::new(cfg.stonfi_http_endpoint.clone()).unwrap();

    let market = MarketManager::new(stonfi_client, market_view, Duration::from_secs(3));

    market
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

    let market_manager = Arc::new(setup_market_manager(market_view, &cfg));

    let mm = Arc::clone(&market_manager);
    let pair_id_clone = pair_id.clone();
    let pool_addr = "EQAdPJcaFwTk7CfJIeE9HElAyjBqx_tni6_m8cDCv9X0SOwn".to_string();

    tokio::spawn(async move {
        if let Err(e) = mm
            .subscribe_stonfi_pair(
                pair_id_clone,
                pool_addr,
                cfg.window_size,
                cfg.min_warm_up,
                cfg.max_slippage_bps,
            )
            .await
        {
            tracing::error!(error=?e, "failed to subscribe stonfi pair");
        }
    });

    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutdown signal received");

    Ok(())
}

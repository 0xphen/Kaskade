// mod mock_store;

// use market::pulse::{PulseValidity, slippage::SlippagePulseResult, spread::SpreadPulseResult};
// use market::types::{MarketMetrics, Pair};
// use session::model::{Session, SessionId, SessionState, SessionThresholds};
// use session::store::SessionStore;

// fn mk_pair(id: &str) -> Pair {
//     Pair {
//         base: id.into(),
//         quote: "TON".into(),
//     }
// }

// fn mk_metrics(spread: f64) -> MarketMetrics {
//     MarketMetrics {
//         spread: SpreadPulseResult {
//             p_now: 0.0,
//             p_best: 0.0,
//             spread_bps: spread,
//             validity: PulseValidity::Valid,
//         },
//         slippage: SlippagePulseResult {
//             slippage_bps: 0.0,
//             validity: PulseValidity::Invalid,
//         },
//     }
// }

// fn mk_active_session(pair: &Pair, chunk: u64) -> Session {
//     Session {
//         id: SessionId::new_v4(),
//         user_id: 1,
//         pair: pair.clone(),
//         total_amount_in: 100,
//         chunk_amount_in: chunk,
//         thresholds: SessionThresholds {
//             max_spread_bps: 500.0,
//             max_slippage_bps: 500.0,
//             trend_enabled: false,
//         },
//         created_at_ms: 0,
//         expires_at_ms: None,
//         approved_amount_in: Some(100),
//         wallet_address: Some("addr".into()),
//         remaining_amount_in: 100,
//         executed_amount_in: 0,
//         executed_amount_out: 0,
//         num_executed_chunks: 0,
//         last_execution_ts_ms: None,
//         state: SessionState::Active,
//     }
// }

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use std::sync::Arc;
//     use tokio::sync::mpsc;

//     use mock_store::MockStore;
//     use scheduler::engine::SchedulerEngine;
//     use scheduler::types::*;
//     use session::manager::SessionManager;

//     fn mk_config() -> SchedulerConfig {
//         SchedulerConfig {
//             max_chunks_per_tick: 2,
//             max_chunks_per_session_per_tick: 1,
//             min_cooldown_ms: 500,
//         }
//     }

//     /// Build a scheduler + execution queue for tests
//     async fn make_scheduler(
//         store: MockStore,
//     ) -> (SchedulerEngine<MockStore>, mpsc::Receiver<ExecutionRequest>) {
//         let (tx, rx) = mpsc::channel(32);
//         let manager = SessionManager::new(Arc::new(store)).await;

//         let engine = SchedulerEngine::new(mk_config(), Arc::new(manager.unwrap()), tx);
//         (engine, rx)
//     }

//     #[tokio::test]
//     async fn no_sessions_no_jobs() {
//         let store = MockStore::new();
//         let (engine, mut rx) = make_scheduler(store).await;

//         let pair = mk_pair("STON");
//         let metrics = mk_metrics(10.0);

//         engine.on_market_tick(pair, metrics, 1000).await;

//         assert!(rx.try_recv().is_err());
//     }

//     #[tokio::test]
//     async fn eligible_session_produces_one_execution_request() {
//         let store = MockStore::new();
//         let pair = mk_pair("STON");

//         let session = mk_active_session(&pair, 10);
//         store.save(&session).await.unwrap();

//         let (engine, mut rx) = make_scheduler(store).await;

//         let metrics = mk_metrics(10.0);

//         engine.on_market_tick(pair.clone(), metrics, 1000).await;

//         let req = rx.try_recv().expect("Expected execution");

//         let sid = req.session_id.to_string();

//         assert_eq!(req.pair, pair);
//         assert_eq!(req.amount_in, 10);
//     }

//     #[tokio::test]
//     async fn spread_too_wide_produces_no_jobs() {
//         let store = MockStore::new();
//         let pair = mk_pair("STON");

//         let mut s = mk_active_session(&pair, 10);
//         s.thresholds.max_spread_bps = 5.0;
//         store.save(&s).await.unwrap();

//         let (engine, mut rx) = make_scheduler(store).await;

//         let bad_metrics = mk_metrics(100.0);

//         engine.on_market_tick(pair.clone(), bad_metrics, 1000).await;

//         assert!(rx.try_recv().is_err());
//     }

//     #[tokio::test]
//     async fn scheduler_enforces_max_chunks_per_tick() {
//         let store = MockStore::new();
//         let pair = mk_pair("STON");

//         for i in 0..3 {
//             let s = mk_active_session(&pair, 10);
//             store.save(&s).await.unwrap();
//         }

//         let (engine, mut rx) = make_scheduler(store).await;
//         let metrics = mk_metrics(1.0);

//         engine.on_market_tick(pair.clone(), metrics, 1000).await;

//         let mut count = 0;
//         while rx.try_recv().is_ok() {
//             count += 1;
//         }

//         assert_eq!(count, 2);
//     }

//     #[tokio::test]
//     async fn scheduler_respects_cooldown() {
//         let store = MockStore::new();
//         let pair = mk_pair("STON");

//         let mut s = mk_active_session(&pair, 10);
//         s.last_execution_ts_ms = Some(900); // under cooldown
//         store.save(&s).await.unwrap();

//         let (engine, mut rx) = make_scheduler(store).await;
//         let metrics = mk_metrics(10.0);

//         engine.on_market_tick(pair.clone(), metrics, 1000).await;

//         assert!(rx.try_recv().is_err());
//     }

//     #[tokio::test]
//     async fn round_robin_rotates_selection() {
//         let store = MockStore::new();
//         let pair = mk_pair("STON");

//         store.save(&mk_active_session(&pair, 10)).await.unwrap();
//         store.save(&mk_active_session(&pair, 10)).await.unwrap();
//         store.save(&mk_active_session(&pair, 10)).await.unwrap();

//         let (engine, mut rx) = make_scheduler(store.clone()).await;
//         let metrics = mk_metrics(1.0);

//         // Tick #1
//         engine
//             .on_market_tick(pair.clone(), metrics.clone(), 1000)
//             .await;
//         let first = rx.try_recv().unwrap().session_id.to_string();

//         // Tick #2 (later time)
//         engine
//             .on_market_tick(pair.clone(), metrics.clone(), 2000)
//             .await;
//         let second = rx.try_recv().unwrap().session_id.to_string();

//         assert_ne!(first, second);
//     }

//     #[tokio::test]
//     async fn cooldown_does_not_affect_other_sessions() {
//         let store = MockStore::new();
//         let pair = mk_pair("STON");

//         let mut s1 = mk_active_session(&pair, 10);
//         s1.last_execution_ts_ms = Some(1900);
//         store.save(&s1).await.unwrap();

//         store.save(&mk_active_session(&pair, 10)).await.unwrap();

//         let (engine, mut rx) = make_scheduler(store).await;
//         let metrics = mk_metrics(1.0);

//         engine
//             .on_market_tick(pair.clone(), metrics.clone(), 2000)
//             .await;

//         let _sid = rx.try_recv().unwrap().session_id.to_string();
//     }
// }

//! The main scheduler engine.
//!
//! For each market tick (per pair), it:
//!   1. Fetches active sessions from SessionManager.
//!   2. Checks eligibility using `eligibility`.
//!   3. Uses `policy` to pick a subset to fire.
//!   4. Emits ExecutionRequest jobs to the executor queue.

use std::sync::Arc;

use tokio::sync::Mutex;

use super::eligibility::check_session_eligibility;
use super::policy::{EligibleHandle, select_sessions_round_robin};
use super::state::SchedulerState;
use super::types::{ExecutionRequest, ExecutionSender, SchedulerConfig, SharedSessionManager};
use market::types::{MarketMetrics, Pair};
use session::store::SessionStore;

pub struct SchedulerEngine<S: SessionStore> {
    cfg: SchedulerConfig,
    session_manager: SharedSessionManager<S>,
    state: Arc<Mutex<SchedulerState>>,
    exec_tx: ExecutionSender,
}

impl<S: SessionStore> SchedulerEngine<S> {
    pub fn new(
        cfg: SchedulerConfig,
        session_manager: SharedSessionManager<S>,
        exec_tx: ExecutionSender,
    ) -> Self {
        Self {
            cfg,
            session_manager,
            state: Arc::new(Mutex::new(SchedulerState::new())),
            exec_tx,
        }
    }

    /// Handle a market tick for a given pair.
    ///
    /// This should be called whenever `MarketManager` produces a new MarketMetrics
    /// snapshot for `pair`.
    pub async fn on_market_tick(&self, pair: Pair, metrics: MarketMetrics, now_ms: u64) {
        #[cfg(debug_assertions)]
        println!(
            "📊 [Scheduler] Tick received for {} — spread={:.2} bps",
            pair.id(),
            metrics.spread.spread_bps
        );

        let sessions = self.session_manager.iter_active_for_pair(&pair).await;

        if sessions.is_empty() {
            #[cfg(debug_assertions)]
            println!("🟦 [Scheduler] No active sessions for {}", pair.id());
            return;
        }

        let mut eligible_handles = Vec::new();

        for session in sessions.iter() {
            let reason = check_session_eligibility(session, &metrics, &self.cfg, now_ms);

            if reason.is_eligible() {
                let h = EligibleHandle {
                    session_id: session.id,
                    chunk_amount_in: session.chunk_amount_in,
                };
                eligible_handles.push(h);

                #[cfg(debug_assertions)]
                println!(
                    "✅ [Eligible] Session {} — chunk={} for {}",
                    session.id,
                    session.chunk_amount_in,
                    pair.id()
                );
            } else {
                #[cfg(debug_assertions)]
                println!("🚫 [Ineligible] Session {} — {:?}", session.id, reason);
            }
        }

        if eligible_handles.is_empty() {
            #[cfg(debug_assertions)]
            println!(
                "🟧 [Scheduler] No eligible sessions this tick for {}",
                pair.id()
            );
            return;
        }

        // Apply selection policy with per-pair round-robin state
        let selected = {
            let mut state_guard = self.state.lock().await;
            let pair_state = state_guard.pair_mut(&pair);

            let chosen = select_sessions_round_robin(pair_state, &self.cfg, &eligible_handles);

            #[cfg(debug_assertions)]
            println!(
                "🔄 [RR] Selected {} session(s) this tick for {}",
                chosen.len(),
                pair.id()
            );

            chosen
        };

        if selected.is_empty() {
            #[cfg(debug_assertions)]
            println!("🟨 [Scheduler] RR produced 0 selections for {}", pair.id());
            return;
        }

        // Emit jobs to executor
        for handle in selected {
            let req = ExecutionRequest {
                session_id: handle.session_id,
                pair: pair.clone(),
                amount_in: handle.chunk_amount_in,
            };

            #[cfg(debug_assertions)]
            println!(
                "🚀 [Dispatch] -> Executor: session={} chunk={} pair={}",
                req.session_id,
                req.amount_in,
                req.pair.id()
            );

            if let Err(e) = self.exec_tx.send(req).await {
                #[cfg(debug_assertions)]
                eprintln!("💥 [Scheduler] ERROR sending execution request: {:?}", e);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    /// Database connection string.
    pub database_url: String,

    // =========================
    // Scheduler configuration
    // =========================
    /// Minimum number of session candidates that should be present
    /// in the in-memory scheduling cache.
    ///
    /// If the cache has fewer than this many sessions, the scheduler
    /// will load another page of eligible sessions from the database.
    ///
    /// Purpose:
    /// - keep scheduling fast
    /// - avoid DB round-trips during every tick
    pub scheduler_candidate_min: usize,

    /// Maximum number of candidate inspections per scheduler tick.
    ///
    /// This bounds CPU usage and prevents unbounded scanning.
    /// The scheduler may need to inspect many sessions in order
    /// to find a smaller number that are actually eligible
    /// (pass constraints, not in cooldown, sufficient DRR deficit).
    ///
    /// IMPORTANT:
    /// - This controls *how wide we search*
    /// - Too low => practical starvation (users never revisited)
    /// - Should scale with cache size
    pub scheduler_max_attempts: usize,

    /// Maximum number of distinct users that may be scheduled
    /// into a single execution batch.
    ///
    /// This controls *how much work we schedule*, not how much we scan.
    ///
    /// Purpose:
    /// - limit batch size
    /// - bound DB transaction size
    /// - bound executor latency
    /// - reduce blast radius on failures
    pub scheduler_max_users_per_batch: usize,

    // =========================
    // Execution configuration
    // =========================
    /// Capacity of the async channel between scheduler and executor.
    ///
    /// Acts as backpressure:
    /// - if executor slows down, scheduler naturally blocks
    /// - prevents unbounded memory growth
    pub exec_queue_capacity: usize,

    /// Cooldown duration (in milliseconds) applied to a session
    /// after a failed or rejected execution.
    ///
    /// Purpose:
    /// - prevent repeated failing retries
    /// - protect the chain and executor
    /// - allow market conditions to change
    pub default_failure_cooldown_ms: u64,
}

impl AppConfig {
    pub fn from_env() -> Self {
        let database_url =
            std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://kaskade_dev.db".to_string());

        Self {
            database_url,

            // Scheduler defaults:
            // - scan widely for fairness (DRR)
            // - schedule conservatively per tick
            scheduler_candidate_min: 200,
            scheduler_max_attempts: 5_000,
            scheduler_max_users_per_batch: 64,

            // Execution defaults:
            exec_queue_capacity: 256,
            default_failure_cooldown_ms: 10_000,
        }
    }
}

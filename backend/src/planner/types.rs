use uuid::Uuid;

/// System-level execution sizing policy.
/// Defines hard safety bounds for how much volume can be allocated per tick
/// and how allocations are split into executable chunks.
#[derive(Clone, Debug)]
pub struct SizingPolicy {
    /// Absolute cap on total bid volume per scheduler tick,
    /// independent of market depth or user demand.
    pub hard_max_total_bid_per_tick: u128,

    /// Fraction of currently available market depth that may be consumed
    /// in a single tick (e.g. 0.25 = use at most 25% of depth).
    pub depth_utilization: f64,

    /// Maximum bid volume a single user may execute per tick,
    /// used to smooth large orders and prevent dominance.
    pub max_bid_per_user_per_tick: u128,

    /// Upper and lower bounds for a single executable chunk.
    /// Chunks larger than `max_chunk_bid` are split; chunks smaller than
    /// `min_chunk_bid` are not produced.
    pub max_chunk_bid: u128,
    pub min_chunk_bid: u128,
}

impl Default for SizingPolicy {
    fn default() -> Self {
        Self {
            hard_max_total_bid_per_tick: 50_000_000,
            depth_utilization: 0.25,
            max_bid_per_user_per_tick: 10_000_000,
            max_chunk_bid: 2_000_000,
            min_chunk_bid: 100_000,
        }
    }
}

/// Planner input describing the scheduler’s desired allocation
/// for a single user in the current tick.
#[derive(Clone, Debug)]
pub struct UserIntent {
    /// Session identifier the intent belongs to.
    pub session_id: Uuid,

    /// Target bid volume the scheduler would like to allocate
    /// for this user in the current tick (upper bound, not guaranteed).
    pub desired_bid: u128,

    /// Hint for desired chunk count; planner may ignore or adjust
    /// based on sizing policy and safety constraints.
    pub desired_chunks: u32,
}

/// Planner output representing the concrete, bounded execution plan
/// for a single user after applying all system and market constraints.
#[derive(Clone, Debug)]
pub struct PlannedAllocation {
    /// Session identifier this allocation applies to.
    pub session_id: Uuid,

    /// Total bid volume actually allocated for execution this tick.
    pub total_bid: u128,

    /// Ordered list of executable chunks whose sum equals `total_bid`.
    /// Chunks are executed sequentially and may partially succeed.
    pub chunks: Vec<u128>,
}

use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

use crate::session::model::Session;

/// Bounded in-memory session cache used by the scheduler.
///
/// Guarantees:
/// - Memory usage is bounded by `max_cached`.
/// - Sessions are rotated fairly using a round-robin ring.
/// - On overflow, evicts a "cold" session from a bounded sample:
///   lowest DRR deficit first, then oldest `last_served_ms`.
pub struct SessionCache {
    /// Max sessions held in memory.
    max_cached: usize,
    /// Number of RR entries sampled when selecting an eviction victim.
    eviction_scan: usize,

    /// Session storage by id.
    map: Mutex<HashMap<Uuid, Session>>,
    /// Candidate rotation ring (ids only).
    rr: Mutex<VecDeque<Uuid>>,
}

impl SessionCache {
    pub fn new(max_cached: usize) -> Self {
        Self {
            max_cached,
            eviction_scan: 64,
            map: Mutex::new(HashMap::new()),
            rr: Mutex::new(VecDeque::new()),
        }
    }

    /// Configure how many RR entries are sampled when choosing an eviction victim.
    /// A minimum of 8 is enforced to avoid pathological eviction.
    pub fn set_eviction_scan(&mut self, n: usize) {
        let old = self.eviction_scan;
        self.eviction_scan = n.max(8);

        debug!(
            old,
            new = self.eviction_scan,
            "cache eviction scan window updated"
        );
    }

    /// Number of ids in the RR ring.
    pub fn len_rr(&self) -> usize {
        self.rr.lock().len()
    }

    /// Clears both the backing map and the RR ring.
    /// Use when rebuilding cache from persistent storage.
    #[instrument(skip(self), target = "cache")]
    pub fn clear(&self) {
        let count = self.map.lock().len();

        self.map.lock().clear();
        self.rr.lock().clear();

        info!(count, "session cache cleared");
    }

    /// Returns a cloned session if it is cached.
    pub fn get(&self, id: &Uuid) -> Option<Session> {
        self.map.lock().get(id).cloned()
    }

    /// Rotates the RR ring and returns the next candidate id.
    pub fn rotate(&self) -> Option<Uuid> {
        let mut rr = self.rr.lock();
        let id = rr.pop_front()?;
        rr.push_back(id);
        Some(id)
    }

    /// Insert or update a session and ensure it appears exactly once in the RR ring.
    /// If inserting a new session would exceed capacity, evicts a cold entry first.
    #[instrument(
        skip(self, s),
        target = "cache", 
        fields(session_id = %s.session_id, pair_id = %s.pair_id)
    )]
    pub fn upsert(&self, s: Session) {
        let mut map = self.map.lock();
        let mut rr = self.rr.lock();

        let session_id = s.session_id;
        let is_new = !map.contains_key(&session_id);

        if is_new && map.len() >= self.max_cached {
            let victim = if let Some(v) = pick_victim(&map, &rr, self.eviction_scan) {
                v
            } else if let Some(evict) = rr.pop_front() {
                evict
            } else {
                // This state shouldn't be reachable if max_cached > 0
                return;
            };

            map.remove(&victim);
            rr.retain(|x| *x != victim);

            info!(
                evicted_id = %victim,
                cache_size = map.len(),
                "cache capacity reached; evicted cold session"
            );
        }

        map.insert(session_id, s);

        if !rr.contains(&session_id) {
            rr.push_back(session_id);
            debug!(cache_size = map.len(), "new session added to cache");
        } else {
            debug!("existing session updated in cache");
        }
    }
}

/// Select an eviction victim from a bounded prefix of the RR ring.
/// Criteria:
/// 1) lowest DRR deficit (colder)
/// 2) if tie, oldest `last_served_ms`
fn pick_victim(map: &HashMap<Uuid, Session>, rr: &VecDeque<Uuid>, scan: usize) -> Option<Uuid> {
    let mut best: Option<(Uuid, i128, u64)> = None;

    let n = rr.len().min(scan);
    for i in 0..n {
        let id = *rr.get(i)?;
        let s = map.get(&id)?;

        let cand = (id, s.state.deficit, s.state.last_served_ms);

        best = match best {
            None => Some(cand),
            Some((bid, bdef, blast)) => {
                if cand.1 < bdef || (cand.1 == bdef && cand.2 < blast) {
                    Some(cand)
                } else {
                    Some((bid, bdef, blast))
                }
            }
        };
    }

    best.map(|x| x.0)
}

/* =========================
 * DRR SAFE MATH HELPERS
 * ========================= */

/// Saturating addition of quantum to deficit.
pub fn drr_add(deficit: i128, quantum: u128) -> i128 {
    let q = quantum.min(i128::MAX as u128) as i128;
    deficit.saturating_add(q)
}

/// Saturating subtraction of execution cost from deficit.
pub fn drr_charge(deficit: i128, cost: u128) -> i128 {
    let c = cost.min(i128::MAX as u128) as i128;
    deficit.saturating_sub(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::model::{Session, SessionIntent, SessionState, UserConstraints};

    fn mk_session(id: Uuid, deficit: i128, last_served_ms: u64) -> Session {
        Session {
            session_id: id,
            pair_id: "TON/USDT".to_string(),
            active: true,
            intent: SessionIntent {
                constraints: UserConstraints {
                    max_spread_bps: 50.0,
                    max_trend_drop_bps: 100.0,
                    max_slippage_bps: 75.0,
                },
                preferred_chunk_bid: 100_000,
                max_bid_per_tick: 1_000_000,
            },
            state: SessionState {
                remaining_bid: 1_000_000,
                remaining_chunks: 10,
                in_flight_bid: 0,
                in_flight_chunks: 0,
                cooldown_until_ms: 0,
                quantum: 100_000,
                has_pending_batch: false,
                deficit,
                last_served_ms,
            },
        }
    }

    fn map_keys(cache: &SessionCache) -> Vec<Uuid> {
        cache.map.lock().keys().copied().collect()
    }

    #[test]
    fn clear_empties_both_map_and_rr() {
        let cache = SessionCache::new(10);
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();

        cache.upsert(mk_session(a, 0, 0));
        cache.upsert(mk_session(b, 0, 0));

        cache.clear();

        assert!(cache.get(&a).is_none());
        assert!(cache.get(&b).is_none());
        assert_eq!(cache.len_rr(), 0);
        assert!(cache.rotate().is_none());
    }

    #[test]
    fn bounded_scan_limits_eviction_candidate_pool() {
        let mut cache = SessionCache::new(12);
        cache.set_eviction_scan(8);

        let mut ids = Vec::new();

        // First 8 (scan window)
        for i in 0..8 {
            let id = Uuid::new_v4();
            ids.push(id);
            cache.upsert(mk_session(id, i as i128 * 10, 0));
        }

        // Coldest outside scan window
        let cold_outside = Uuid::new_v4();
        cache.upsert(mk_session(cold_outside, -999, 0));
        ids.push(cold_outside);

        // Fill to capacity
        for _ in 0..3 {
            let id = Uuid::new_v4();
            cache.upsert(mk_session(id, 100, 0));
            ids.push(id);
        }

        // Trigger eviction
        let incoming = Uuid::new_v4();
        cache.upsert(mk_session(incoming, 1, 0));

        let keys = map_keys(&cache);
        assert!(keys.contains(&cold_outside));
        assert!(!keys.contains(&ids[0])); // coldest in-window
        assert!(keys.contains(&incoming));
    }

    #[test]
    fn drr_math_overflow_safety() {
        let near_max = i128::MAX - 5;
        let out = drr_add(near_max, u128::MAX);
        assert_eq!(out, i128::MAX);

        let near_min = i128::MIN + 5;
        let out = drr_charge(near_min, u128::MAX);
        assert_eq!(out, i128::MIN);
    }
}

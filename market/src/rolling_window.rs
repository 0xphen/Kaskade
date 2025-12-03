use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

pub const DEFAULT_MAX_AGE_MS: u64 = 30_000;

/// A timestamped value used inside the rolling window
#[derive(Clone, Debug)]
pub struct TimedValue<T> {
    pub ts_ms: u64,
    pub value: T,
}

/// Rolling window with monotonic max queue for O(1) max()
#[derive(Default)]
pub struct RollingWindow {
    /// All values in the window (ordered by time)
    values: VecDeque<TimedValue<f64>>,

    /// Monotonic deque storing decreasing values for fast max lookup
    /// The front always holds the maximum element in the current window
    max_queue: VecDeque<TimedValue<f64>>,

    /// Maximum age
    max_age_ms: u64,
}

impl RollingWindow {
    pub fn new() -> Self {
        Self {
            values: VecDeque::new(),
            max_queue: VecDeque::new(),
            max_age_ms: DEFAULT_MAX_AGE_MS,
        }
    }

    pub fn push(&mut self, ts_ms: u64, price: f64) {
        let val = TimedValue {
            ts_ms,
            value: price,
        };

        self.values.push_back(val.clone());

        // Maintain monotonic decreasing max_queue
        while let Some(back) = self.max_queue.back() {
            if back.value < price {
                self.max_queue.pop_back();
            } else {
                break;
            }
        }
        self.max_queue.push_back(val);

        self.evict_old(ts_ms);
    }

    /// Evict values older than max_age
    fn evict_old(&mut self, now_ms: u64) {
        while let Some(front) = self.values.front() {
            if now_ms - front.ts_ms > self.max_age_ms {
                let removed = self.values.pop_front().unwrap();

                if let Some(max_front) = self.max_queue.front() {
                    if max_front.ts_ms == removed.ts_ms {
                        self.max_queue.pop_front();
                    }
                }
            } else {
                break;
            }
        }
    }

    pub fn max(&self) -> Option<f64> {
        self.max_queue.front().map(|v| v.value)
    }

    pub fn latest(&self) -> Option<f64> {
        self.values.back().map(|v| v.value)
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

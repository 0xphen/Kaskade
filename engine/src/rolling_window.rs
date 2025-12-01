use std::collections::VecDeque;

#[derive(Debug)]
pub struct RollingWindow<T> {
    window: VecDeque<(u64, T)>, // (timestamp_ms, item)
    max_age_ms: u64,
}

impl<T> RollingWindow<T> {
    pub fn new(max_age_ms: u64) -> Self {
        Self {
            window: VecDeque::new(),
            max_age_ms,
        }
    }

    pub fn push(&mut self, ts_ms: u64, item: T) {
        self.window.push_back((ts_ms, item));
        self.evict_old(ts_ms);
    }

    pub fn latest(&self) -> Option<&T> {
        self.window.back().map(|(_, item)| item)
    }

    fn evict_old(&mut self, now_ms: u64) {
        while let Some((ts, _)) = self.window.front() {
            if now_ms - ts > self.max_age_ms {
                self.window.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.window.is_empty()
    }
}

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct AtomicRateLimiter {
    rate: u32,
    window_start: AtomicU64,
    count: AtomicU32,
    window_duration_ms: u64,
}

impl AtomicRateLimiter {
    pub fn new(rate: u32, window: Duration) -> Self {
        Self {
            rate,
            window_start: AtomicU64::new(current_millis()),
            count: AtomicU32::new(0),
            window_duration_ms: window.as_millis() as u64,
        }
    }

    pub fn check(&self) -> bool {
        let now = current_millis();

        loop {
            let start = self.window_start.load(Ordering::Relaxed);

            if now >= start + self.window_duration_ms {
                if self
                    .window_start
                    .compare_exchange(start, now, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    self.count.store(1, Ordering::Relaxed);
                    return true;
                }
                continue;
            }

            let c = self.count.fetch_add(1, Ordering::Relaxed);
            return c < self.rate;
        }
    }
}

fn current_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

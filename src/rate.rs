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
        let window_start = self.window_start.load(Ordering::Relaxed);

        if now >= window_start + self.window_duration_ms {
            // New window
            // Try to advance window. If another thread did it, we just add to that new window.
            let _ = self.window_start.compare_exchange(
                window_start,
                now,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
            // Reset count if we successfully changed window, or if the window changed heavily.
            // Actually, simpler approach:
            // If window changed, we can try to reset count.
            // But to be safe and lock-free:
            // If we see old window, we try to update window and reset count.
            // The simplest "lock-free" approximation is:
        }

        // Let's use a standard atomic window algorithm.
        // If (now - window_start) > window:
        //    reset window_start = now
        //    count = 0
        //
        // This requires CAS loop if we want strict correctness, or relaxed if we tolerate some burst.
        // Given "Safe for TCP + UDP", let's do a proper CAS loop for window rotation.

        loop {
            let start = self.window_start.load(Ordering::Relaxed);
            if now >= start + self.window_duration_ms {
                // Try to rotate window
                if self
                    .window_start
                    .compare_exchange(start, now, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    // We won the race to rotate. Reset count.
                    self.count.store(1, Ordering::Relaxed);
                    return true;
                }
                // If we lost, loop again to see new state
            } else {
                // Current window
                let c = self.count.fetch_add(1, Ordering::Relaxed);
                if c < self.rate {
                    return true;
                } else {
                    return false;
                }
            }
        }
    }
}

fn current_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub struct RateLimiter {
    max_requests: usize,
    window: Duration,
    requests: VecDeque<Instant>,
}

impl RateLimiter {
    pub fn new(max_requests: usize, window: Duration) -> Self {
        Self {
            max_requests,
            window,
            requests: VecDeque::new(),
        }
    }

    /// Returns true if request is allowed
    pub fn allow(&mut self) -> bool {
        let now = Instant::now();

        // Remove expired timestamps
        while let Some(&front) = self.requests.front() {
            if now.duration_since(front) > self.window {
                self.requests.pop_front();
            } else {
                break;
            }
        }

        if self.requests.len() < self.max_requests {
            self.requests.push_back(now);
            true
        } else {
            false
        }
    }
}

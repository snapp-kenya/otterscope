//! Per-project token-bucket limiter for ingest. Keys are ingest projects, so
//! one noisy source can't starve others. A non-positive rate means unlimited.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub(crate) struct RateLimiter {
    per_sec: f64,
    burst: f64,
    buckets: Mutex<HashMap<String, Bucket>>,
    now: Arc<dyn Fn() -> Instant + Send + Sync>,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    /// `burst <= 0` defaults to 2x the rate (minimum 1).
    pub(crate) fn new(per_sec: f64, burst: f64) -> Option<Self> {
        if per_sec <= 0.0 {
            return None;
        }
        let burst = if burst <= 0.0 { per_sec * 2.0 } else { burst };
        let burst = if burst < 1.0 { 1.0 } else { burst };
        Some(Self {
            per_sec,
            burst,
            buckets: Mutex::new(HashMap::new()),
            now: Arc::new(Instant::now),
        })
    }

    /// Consume a token for `key`, reporting whether the request may proceed.
    pub(crate) fn allow(&self, key: &str) -> bool {
        let now = (self.now)();
        let mut buckets = match self.buckets.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            tokens: self.burst,
            last: now,
        });
        let refill = now.saturating_duration_since(bucket.last).as_secs_f64() * self.per_sec;
        bucket.tokens = (bucket.tokens + refill).min(self.burst);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use super::RateLimiter;

    fn limiter(rate: f64, burst: f64, clock: &Arc<Mutex<Instant>>) -> RateLimiter {
        let mut limiter = RateLimiter::new(rate, burst).unwrap();
        let clock = Arc::clone(clock);
        limiter.now = Arc::new(move || *clock.lock().unwrap());
        limiter
    }

    #[test]
    fn burst_then_deny_then_refill() {
        let clock = Arc::new(Mutex::new(Instant::now()));
        let limiter = limiter(10.0, 3.0, &clock);
        for i in 0..3 {
            assert!(limiter.allow("p"), "burst token {i} denied");
        }
        assert!(!limiter.allow("p"), "4th request should be denied");

        *clock.lock().unwrap() += Duration::from_millis(100);
        assert!(limiter.allow("p"), "refilled token should be allowed");
        assert!(!limiter.allow("p"), "only one token should have refilled");
    }

    #[test]
    fn buckets_are_per_key() {
        let clock = Arc::new(Mutex::new(Instant::now()));
        let limiter = limiter(1.0, 1.0, &clock);
        assert!(limiter.allow("a") && limiter.allow("b"));
        assert!(!limiter.allow("a") && !limiter.allow("b"));
    }

    #[test]
    fn zero_rate_is_unlimited() {
        assert!(RateLimiter::new(0.0, 0.0).is_none());
        assert!(RateLimiter::new(-1.0, 5.0).is_none());
    }

    #[test]
    fn burst_defaults_to_double_rate_with_floor_of_one() {
        let limiter = RateLimiter::new(0.25, 0.0).unwrap();
        assert_eq!(limiter.burst, 1.0);
        let limiter = RateLimiter::new(4.0, 0.0).unwrap();
        assert_eq!(limiter.burst, 8.0);
    }
}

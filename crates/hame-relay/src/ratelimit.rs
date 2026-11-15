//! In-memory token-bucket rate limiting, keyed by strings such as
//! `ip:203.0.113.7` or `login:alice@example.com`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A limit: `burst` requests at once, refilled at `per_hour`.
#[derive(Clone, Copy, Debug)]
pub struct Limit {
    pub burst: u32,
    pub per_hour: u32,
}

impl Limit {
    /// `n` at once and `n` per minute.
    pub fn per_minute(n: u32) -> Self {
        Self {
            burst: n.max(1),
            per_hour: n.max(1).saturating_mul(60),
        }
    }
}

struct Bucket {
    tokens: f64,
    updated: Instant,
}

/// A set of buckets.
#[derive(Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    /// Takes one token from `key`'s bucket. On refusal, returns the seconds
    /// until a token is available.
    pub fn check(&self, key: &str, limit: Limit) -> Result<(), u64> {
        let now = Instant::now();
        let Ok(mut map) = self.buckets.lock() else {
            return Ok(());
        };
        let rate = f64::from(limit.per_hour.max(1)) / 3600.0;
        let burst = f64::from(limit.burst);
        let bucket = map.entry(key.to_string()).or_insert(Bucket {
            tokens: burst,
            updated: now,
        });
        let elapsed = now.duration_since(bucket.updated).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * rate).min(burst);
        bucket.updated = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            Err(((1.0 - bucket.tokens) / rate).ceil() as u64)
        }
    }

    /// Forgets buckets idle for longer than `idle`.
    pub fn sweep(&self, idle: Duration) {
        let now = Instant::now();
        if let Ok(mut map) = self.buckets.lock() {
            map.retain(|_, b| now.duration_since(b.updated) < idle);
        }
    }
}

/// A per-connection limiter (no map, no lock).
pub struct LocalBucket {
    tokens: f64,
    burst: f64,
    per_sec: f64,
    updated: Instant,
}

impl LocalBucket {
    pub fn new(per_sec: u32, burst: u32) -> Self {
        Self {
            tokens: f64::from(burst.max(1)),
            burst: f64::from(burst.max(1)),
            per_sec: f64::from(per_sec.max(1)),
            updated: Instant::now(),
        }
    }

    pub fn take(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.updated).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.per_sec).min(self.burst);
        self.updated = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_refuses_after_burst() {
        let rl = RateLimiter::default();
        let limit = Limit::per_minute(3);
        assert!(rl.check("k", limit).is_ok());
        assert!(rl.check("k", limit).is_ok());
        assert!(rl.check("k", limit).is_ok());
        let wait = rl.check("k", limit).unwrap_err();
        assert!((1..=20).contains(&wait), "{wait}");
        assert!(rl.check("other", limit).is_ok());
    }
}

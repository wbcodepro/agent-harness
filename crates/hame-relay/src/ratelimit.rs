//! In-memory token-bucket rate limiting, keyed by strings such as
//! `ip:203.0.113.7` or `login:alice@example.com`.

use std::collections::HashMap;
use std::net::IpAddr;
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

    fn rate_per_sec(self) -> f64 {
        f64::from(self.per_hour.max(1)) / 3600.0
    }
}

/// The rate-limit key for a client address: IPv4 addresses as they are,
/// IPv6 addresses by their /64 (one subscriber usually holds a whole /64),
/// IPv4-mapped IPv6 as IPv4.
pub fn ip_key(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return v4.to_string();
            }
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
    }
}

struct Bucket {
    tokens: f64,
    burst: f64,
    rate: f64,
    updated: Instant,
}

impl Bucket {
    fn refill(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.updated).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate).min(self.burst);
        self.updated = now;
    }

    /// A bucket that has refilled completely is the same as no bucket.
    fn is_full_at(&self, now: Instant) -> bool {
        let elapsed = now.duration_since(self.updated).as_secs_f64();
        self.tokens + elapsed * self.rate >= self.burst
    }
}

/// A set of buckets, at most `max_entries` of them.
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, Bucket>>,
    max_entries: usize,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(100_000)
    }
}

impl RateLimiter {
    pub fn new(max_entries: usize) -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            max_entries: max_entries.max(16),
        }
    }

    /// Takes one token from `key`'s bucket. On refusal, returns the seconds
    /// until a token is available.
    pub fn check(&self, key: &str, limit: Limit) -> Result<(), u64> {
        self.take(key, limit, true)
    }

    /// Whether `key`'s bucket has a token, without taking it.
    pub fn peek(&self, key: &str, limit: Limit) -> Result<(), u64> {
        self.take(key, limit, false)
    }

    fn take(&self, key: &str, limit: Limit, consume: bool) -> Result<(), u64> {
        let now = Instant::now();
        let Ok(mut map) = self.buckets.lock() else {
            return Ok(());
        };
        let rate = limit.rate_per_sec();
        let burst = f64::from(limit.burst.max(1));
        if !map.contains_key(key) {
            if !consume {
                return Ok(());
            }
            if map.len() >= self.max_entries {
                evict(&mut map, now, self.max_entries);
            }
            map.insert(
                key.to_string(),
                Bucket {
                    tokens: burst,
                    burst,
                    rate,
                    updated: now,
                },
            );
        }
        let Some(bucket) = map.get_mut(key) else {
            return Ok(());
        };
        bucket.burst = burst;
        bucket.rate = rate;
        bucket.refill(now);
        if bucket.tokens >= 1.0 {
            if consume {
                bucket.tokens -= 1.0;
            }
            Ok(())
        } else {
            Err(((1.0 - bucket.tokens) / rate).ceil() as u64)
        }
    }

    /// Forgets buckets idle for longer than `idle` or refilled completely.
    pub fn sweep(&self, idle: Duration) {
        let now = Instant::now();
        if let Ok(mut map) = self.buckets.lock() {
            map.retain(|_, b| now.duration_since(b.updated) < idle && !b.is_full_at(now));
        }
    }

    pub fn len(&self) -> usize {
        self.buckets.lock().map(|m| m.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Makes room: first drops buckets that have refilled (no information is
/// lost), then the least recently used tenth.
fn evict(map: &mut HashMap<String, Bucket>, now: Instant, max: usize) {
    map.retain(|_, b| !b.is_full_at(now));
    if map.len() < max {
        return;
    }
    let mut ages: Vec<(Instant, String)> =
        map.iter().map(|(k, b)| (b.updated, k.clone())).collect();
    ages.sort_unstable_by_key(|(t, _)| *t);
    for (_, k) in ages.into_iter().take(max / 10 + 1) {
        map.remove(&k);
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
        assert!(rl.peek("k", limit).is_ok());
        assert!(rl.check("k", limit).is_ok());
        let wait = rl.check("k", limit).unwrap_err();
        assert!((1..=20).contains(&wait), "{wait}");
        assert!(rl.peek("k", limit).is_err());
        assert!(rl.check("other", limit).is_ok());
    }

    #[test]
    fn ipv6_is_bucketed_by_64() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:ffff:1:2:3".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(ip_key(a), ip_key(b));
        assert_ne!(ip_key(a), ip_key(c));
        let mapped: IpAddr = "::ffff:203.0.113.9".parse().unwrap();
        assert_eq!(ip_key(mapped), "203.0.113.9");
    }

    #[test]
    fn the_map_stays_bounded_and_keeps_active_buckets() {
        let rl = RateLimiter::new(100);
        let limit = Limit::per_minute(1);
        // An attacker's own bucket, empty.
        assert!(rl.check("victim", limit).is_ok());
        assert!(rl.check("victim", limit).is_err());
        for i in 0..1000 {
            let _ = rl.check(&format!("k{i}"), limit);
            // The active bucket is used again and again.
            if i % 5 == 0 {
                let _ = rl.check("victim", limit);
            }
        }
        assert!(rl.len() <= 100, "{}", rl.len());
        assert!(
            rl.check("victim", limit).is_err(),
            "evicted an active, empty bucket"
        );
    }
}

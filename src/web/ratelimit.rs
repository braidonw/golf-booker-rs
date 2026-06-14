//! A small in-memory limiter for login attempts, keyed by username.
//!
//! Defence-in-depth: the app is tailnet-only and behind a proxy (so per-IP
//! limiting is moot — every request arrives from the proxy), but throttling
//! repeated failures per username still blunts password guessing. State is
//! process-local, which is fine for a single instance.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Failed attempts allowed within [`WINDOW`] before a username is throttled.
const MAX_FAILURES: u32 = 5;
/// Sliding window for counting failures (and the cool-down once tripped).
const WINDOW: Duration = Duration::from_secs(300);

struct Attempts {
    failures: u32,
    window_start: Instant,
}

pub struct LoginLimiter {
    inner: Mutex<HashMap<String, Attempts>>,
    max_failures: u32,
    window: Duration,
}

impl LoginLimiter {
    fn new(max_failures: u32, window: Duration) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            max_failures,
            window,
        }
    }

    /// Whether a login attempt for `key` is currently allowed.
    pub fn allowed(&self, key: &str) -> bool {
        self.allowed_at(key, Instant::now())
    }

    /// Record a failed attempt for `key`.
    pub fn record_failure(&self, key: &str) {
        self.record_failure_at(key, Instant::now());
    }

    /// Clear a key's failures after a successful login.
    pub fn record_success(&self, key: &str) {
        self.inner.lock().unwrap().remove(key);
    }

    fn allowed_at(&self, key: &str, now: Instant) -> bool {
        let map = self.inner.lock().unwrap();
        match map.get(key) {
            // Stale window: a fresh attempt is allowed.
            Some(a) if now.duration_since(a.window_start) >= self.window => true,
            Some(a) => a.failures < self.max_failures,
            None => true,
        }
    }

    fn record_failure_at(&self, key: &str, now: Instant) {
        let mut map = self.inner.lock().unwrap();
        // Drop entries whose window has fully elapsed, so the map can't grow
        // unbounded with one-off (or attacker-supplied) usernames. This also
        // resets a lapsed key: it's re-created fresh just below.
        map.retain(|_, a| now.duration_since(a.window_start) < self.window);
        let entry = map.entry(key.to_string()).or_insert(Attempts {
            failures: 0,
            window_start: now,
        });
        entry.failures += 1;
    }
}

/// Process-wide limiter for the login endpoint.
pub fn login_limiter() -> &'static LoginLimiter {
    use std::sync::OnceLock;
    static LIMITER: OnceLock<LoginLimiter> = OnceLock::new();
    LIMITER.get_or_init(|| LoginLimiter::new(MAX_FAILURES, WINDOW))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttles_after_max_failures() {
        let lim = LoginLimiter::new(3, Duration::from_secs(300));
        let t0 = Instant::now();
        assert!(lim.allowed_at("bob", t0));
        for _ in 0..3 {
            lim.record_failure_at("bob", t0);
        }
        assert!(
            !lim.allowed_at("bob", t0),
            "should be throttled after 3 fails"
        );
        // A different user is unaffected.
        assert!(lim.allowed_at("alice", t0));
    }

    #[test]
    fn window_resets_after_expiry() {
        let lim = LoginLimiter::new(3, Duration::from_secs(300));
        let t0 = Instant::now();
        for _ in 0..3 {
            lim.record_failure_at("bob", t0);
        }
        assert!(!lim.allowed_at("bob", t0));
        let later = t0 + Duration::from_secs(301);
        assert!(lim.allowed_at("bob", later), "window should have reset");
    }

    #[test]
    fn evicts_stale_entries_to_bound_the_map() {
        let lim = LoginLimiter::new(3, Duration::from_secs(300));
        let t0 = Instant::now();
        lim.record_failure_at("old", t0);
        assert_eq!(lim.inner.lock().unwrap().len(), 1);

        // A later failure for a different key prunes the now-stale "old" entry.
        let later = t0 + Duration::from_secs(301);
        lim.record_failure_at("new", later);
        let map = lim.inner.lock().unwrap();
        assert!(!map.contains_key("old"), "stale entry should be evicted");
        assert!(map.contains_key("new"));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn success_clears_failures() {
        let lim = LoginLimiter::new(3, Duration::from_secs(300));
        let t0 = Instant::now();
        for _ in 0..3 {
            lim.record_failure_at("bob", t0);
        }
        assert!(!lim.allowed_at("bob", t0));
        lim.record_success("bob");
        assert!(lim.allowed_at("bob", t0));
    }
}

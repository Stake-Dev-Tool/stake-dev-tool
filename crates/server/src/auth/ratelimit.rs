//! In-memory fixed-window rate limiter for failed logins.
//!
//! NOTE: state is process-local. A multi-instance deployment behind a load
//! balancer would let an attacker get `instances × limit` attempts; enforcing
//! the cap globally there needs a shared store (e.g. Redis). Fine for the
//! single-node self-host and hosted setups M1 targets.

use std::time::{Duration, Instant};

use dashmap::DashMap;

const MAX_FAILURES: u32 = 10;
const WINDOW: Duration = Duration::from_secs(15 * 60);

/// Tracks failed login attempts per `(client IP, lowercased email)`.
pub struct LoginRateLimiter {
    windows: DashMap<(String, String), Window>,
    max_failures: u32,
    window: Duration,
}

struct Window {
    started: Instant,
    failures: u32,
}

impl LoginRateLimiter {
    pub fn new() -> Self {
        Self::with_limits(MAX_FAILURES, WINDOW)
    }

    /// Same fixed-window limiter with an explicit budget and window — used for
    /// the email flows (forgot-password / resend-verification: 5 per hour per
    /// `(ip, email)`), which reuse this module's pattern with different limits.
    pub fn with_limits(max_failures: u32, window: Duration) -> Self {
        Self {
            windows: DashMap::new(),
            max_failures,
            window,
        }
    }

    fn key(ip: &str, email: &str) -> (String, String) {
        (ip.to_string(), email.to_lowercase())
    }

    /// True when this `(ip, email)` has exhausted its failure budget within the
    /// current window.
    pub fn is_blocked(&self, ip: &str, email: &str) -> bool {
        match self.windows.get(&Self::key(ip, email)) {
            Some(w) if w.started.elapsed() < self.window => w.failures >= self.max_failures,
            _ => false,
        }
    }

    /// Records one failed attempt, opening a fresh window if the previous one has
    /// elapsed.
    pub fn record_failure(&self, ip: &str, email: &str) {
        let mut entry = self
            .windows
            .entry(Self::key(ip, email))
            .or_insert_with(|| Window {
                started: Instant::now(),
                failures: 0,
            });
        if entry.started.elapsed() >= self.window {
            entry.started = Instant::now();
            entry.failures = 0;
        }
        entry.failures += 1;
    }

    /// Clears the window after a successful login so an honest user who mistyped a
    /// few times isn't left throttled.
    pub fn clear(&self, ip: &str, email: &str) {
        self.windows.remove(&Self::key(ip, email));
    }
}

impl Default for LoginRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_the_failure_budget_is_spent() {
        let limiter = LoginRateLimiter::new();
        for _ in 0..MAX_FAILURES {
            assert!(!limiter.is_blocked("ip", "user@example.com"));
            limiter.record_failure("ip", "user@example.com");
        }
        assert!(limiter.is_blocked("ip", "user@example.com"));
        // Email match is case-insensitive.
        assert!(limiter.is_blocked("ip", "USER@example.com"));
        // A different account on the same IP is unaffected.
        assert!(!limiter.is_blocked("ip", "other@example.com"));
        // Clearing frees the account again.
        limiter.clear("ip", "user@example.com");
        assert!(!limiter.is_blocked("ip", "user@example.com"));
    }
}

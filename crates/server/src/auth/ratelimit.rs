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

/// Per-account failed-login budget aggregated across ALL client IPs, keyed by
/// email alone. Deliberately higher than the per-`(ip,email)` budget: one honest
/// account failing from a couple of networks stays under it, while an attacker
/// rotating source IPs to guess a single account trips it. Same 15-minute window.
const MAX_FAILURES_PER_ACCOUNT: u32 = 20;

/// Generous per-IP budget for the unauthenticated abuse-prone endpoints
/// (registration, device-code start), keyed by IP alone. High enough that a
/// human — or a small office behind one NAT — never notices, low enough to blunt
/// scripted account/device-code spam.
const MAX_ATTEMPTS_PER_IP: u32 = 30;
const IP_WINDOW: Duration = Duration::from_secs(60 * 60);

/// Sentinel occupying the unused half of the `(ip, email)` key when a limiter is
/// scoped to only one dimension (account-only or IP-only). Each such limiter is a
/// distinct instance, so the sentinel only needs to be stable, not unique.
const ANY: &str = "";

/// Fixed-window failure limiter. The general form keys on `(client IP, lowercased
/// email)`; the account- and IP-scoped helpers reuse the same machinery with one
/// half of the key pinned to [`ANY`].
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

    /// The per-account limiter (email only, across every client IP) that stops IP
    /// rotation from brute-forcing a single account. See [`MAX_FAILURES_PER_ACCOUNT`].
    pub fn per_account() -> Self {
        Self::with_limits(MAX_FAILURES_PER_ACCOUNT, WINDOW)
    }

    /// The per-IP limiter for registration / device-code start. See
    /// [`MAX_ATTEMPTS_PER_IP`].
    pub fn per_ip() -> Self {
        Self::with_limits(MAX_ATTEMPTS_PER_IP, IP_WINDOW)
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

    // --- account-scoped (email only, across every IP) -----------------------

    /// True when this account has exhausted its across-IP failure budget.
    pub fn is_account_blocked(&self, email: &str) -> bool {
        self.is_blocked(ANY, email)
    }

    /// Records one failed attempt against the account (any IP).
    pub fn record_account_failure(&self, email: &str) {
        self.record_failure(ANY, email);
    }

    /// Clears the account's window after a successful login.
    pub fn clear_account(&self, email: &str) {
        self.clear(ANY, email);
    }

    // --- IP-scoped (client IP only) -----------------------------------------

    /// True when this IP has exhausted its endpoint budget.
    pub fn is_ip_blocked(&self, ip: &str) -> bool {
        self.is_blocked(ip, ANY)
    }

    /// Records one attempt from this IP (counts successes too — the cap is on
    /// total requests to an abuse-prone endpoint, not just failures).
    pub fn record_ip_attempt(&self, ip: &str) {
        self.record_failure(ip, ANY);
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

    #[test]
    fn account_limiter_blocks_across_rotating_ips() {
        let limiter = LoginRateLimiter::per_account();
        // The account limiter keys on the email alone, so failures from any number
        // of distinct source IPs all accumulate against the one account.
        for _ in 0..MAX_FAILURES_PER_ACCOUNT {
            assert!(!limiter.is_account_blocked("target@example.com"));
            limiter.record_account_failure("target@example.com");
        }
        assert!(limiter.is_account_blocked("target@example.com"));
        // Case-insensitive on the email; a different account is unaffected.
        assert!(limiter.is_account_blocked("TARGET@example.com"));
        assert!(!limiter.is_account_blocked("other@example.com"));
        limiter.clear_account("target@example.com");
        assert!(!limiter.is_account_blocked("target@example.com"));
    }

    #[test]
    fn ip_limiter_blocks_after_the_endpoint_budget() {
        let limiter = LoginRateLimiter::per_ip();
        for _ in 0..MAX_ATTEMPTS_PER_IP {
            assert!(!limiter.is_ip_blocked("203.0.113.7"));
            limiter.record_ip_attempt("203.0.113.7");
        }
        assert!(limiter.is_ip_blocked("203.0.113.7"));
        // A different IP has its own budget.
        assert!(!limiter.is_ip_blocked("203.0.113.8"));
    }
}

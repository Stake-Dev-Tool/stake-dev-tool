//! Workspace custom play domains: resolving a request Host of the form
//! `<label>.<custom_play_domain>` to the owning workspace, backed by a small
//! in-process cache so TLS-handshake storms and wildcard-pointed junk labels
//! never hammer Postgres.
//!
//! A workspace owner attaches a domain they control (e.g. `play.acme.com`, stored
//! on `workspaces.custom_play_domain`). Its wildcard record (`*.play.acme.com`)
//! points at this server, so `demo.play.acme.com` arrives here. We peel off the
//! single leading label and look the *rest* up as a registered custom domain. On
//! a hit the request is dispatched to the share router, scoped to that workspace
//! (see [`super::resolve`]), and — during the TLS handshake — the on-demand cert
//! is approved (see `api::domains::tls_check`).

use std::time::{Duration, Instant};

use dashmap::DashMap;
use sqlx::PgPool;
use uuid::Uuid;

/// How long a domain -> workspace resolution (positive or negative) is trusted.
const TTL: Duration = Duration::from_secs(60);

/// Cache of custom-domain resolutions, keyed by the domain *suffix* (the Host
/// with its single leading label removed, e.g. `play.acme.com`). The value is the
/// owning workspace id, or `None` when no workspace claims that domain — negative
/// results are cached too, so wildcard-pointed junk labels and probing bots don't
/// generate a DB hit each. Entries expire after [`TTL`]; a domain set/clear calls
/// [`CustomDomainCache::clear`] so the change takes effect immediately.
#[derive(Default)]
pub struct CustomDomainCache {
    entries: DashMap<String, (Option<Uuid>, Instant)>,
}

impl CustomDomainCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop every cached entry. Called when a workspace's custom domain changes so
    /// a set/clear is reflected immediately rather than after the TTL.
    pub fn clear(&self) {
        self.entries.clear();
    }

    /// A live (non-expired) cached resolution for `suffix`, if any.
    fn get(&self, suffix: &str) -> Option<Option<Uuid>> {
        let entry = self.entries.get(suffix)?;
        (entry.1.elapsed() < TTL).then_some(entry.0)
    }

    fn put(&self, suffix: String, workspace_id: Option<Uuid>) {
        self.entries.insert(suffix, (workspace_id, Instant::now()));
    }
}

/// Resolve a request Host against registered custom play domains.
///
/// Splits off exactly one leading label — `<label>.<suffix>` — and returns
/// `(label, workspace_id)` when some workspace has claimed exactly `<suffix>`.
/// Only a *single* leading label is accepted, so `a.b.play.acme.com` (two labels
/// over a `play.acme.com` domain) does not resolve. Returns `None` for the apex,
/// bare/IP/single-label hosts, and unknown suffixes.
///
/// Uses and populates the 60s [`CustomDomainCache`]; a transient DB error yields
/// `None` (fail closed — no cert issued, no share served) and is *not* cached.
pub async fn resolve_custom_host(
    pool: &PgPool,
    cache: &CustomDomainCache,
    host: &str,
) -> Option<(String, Uuid)> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    // An IP literal is never a custom domain (and cheap to reject early).
    if host.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }
    let (label, suffix) = host.split_once('.')?;
    // Custom domains are validated to at least two labels, so a resolvable suffix
    // always itself contains a dot; this also skips a DB probe for `foo.localhost`.
    if label.is_empty() || !suffix.contains('.') {
        return None;
    }

    if let Some(cached) = cache.get(suffix) {
        return cached.map(|id| (label.to_string(), id));
    }

    match sqlx::query_scalar::<_, Uuid>("SELECT id FROM workspaces WHERE custom_play_domain = $1")
        .bind(suffix)
        .fetch_optional(pool)
        .await
    {
        Ok(found) => {
            cache.put(suffix.to_string(), found);
            found.map(|id| (label.to_string(), id))
        }
        Err(e) => {
            tracing::warn!(error = %e, suffix, "custom-domain: lookup failed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_returns_within_ttl_and_clears() {
        let cache = CustomDomainCache::new();
        let id = Uuid::new_v4();
        assert_eq!(cache.get("play.acme.com"), None);
        cache.put("play.acme.com".to_string(), Some(id));
        assert_eq!(cache.get("play.acme.com"), Some(Some(id)));
        // Negative results cache too.
        cache.put("nope.example.com".to_string(), None);
        assert_eq!(cache.get("nope.example.com"), Some(None));
        cache.clear();
        assert_eq!(cache.get("play.acme.com"), None);
        assert_eq!(cache.get("nope.example.com"), None);
    }
}

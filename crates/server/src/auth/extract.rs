//! Axum extractors that turn a request's credentials into a `CurrentUser`.
//!
//! `CurrentUser` accepts either a session cookie or an `Authorization: Bearer
//! sdt_pat_…` token. `SessionUser` additionally requires the cookie, so a PAT
//! can't reach session-only endpoints (e.g. minting more tokens). `ClientIp`
//! resolves a best-effort client address for login rate-limiting.

use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};

use axum::async_trait;
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum_extra::TypedHeader;
use axum_extra::extract::cookie::CookieJar;
use axum_extra::headers::Authorization;
use axum_extra::headers::authorization::Bearer;
use uuid::Uuid;

use crate::AppState;
use crate::auth::{sessions, tokens};
use crate::config::TrustedProxies;
use crate::error::{ApiError, ApiResult};

/// Which credential authenticated the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSource {
    Session,
    ApiToken,
}

/// An authenticated caller: the user, how they authenticated, and their scopes
/// (a session implicitly has full scopes).
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub user_id: Uuid,
    pub source: AuthSource,
    pub scopes: Vec<String>,
}

impl CurrentUser {
    /// True when the caller holds `scope` or the catch-all `full` scope.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope || s == "full")
    }

    /// 403 `insufficient_scope` unless the caller holds `scope` (or `full`).
    /// Sessions carry `full`, so they always pass; PATs must have been minted
    /// with the scope. Apply *after* the membership check so a non-member still
    /// gets a 404 rather than leaking the workspace's existence via a 403.
    pub fn require_scope(&self, scope: &str) -> ApiResult<()> {
        if self.has_scope(scope) {
            Ok(())
        } else {
            Err(ApiError::forbidden(
                "insufficient_scope",
                format!("this action requires the \"{scope}\" scope"),
            ))
        }
    }
}

/// Core resolution shared by both extractors. A present-but-invalid Bearer token
/// is rejected outright rather than falling through to the cookie.
async fn authenticate(parts: &mut Parts, state: &AppState) -> ApiResult<CurrentUser> {
    if let Ok(Some(TypedHeader(auth))) =
        Option::<TypedHeader<Authorization<Bearer>>>::from_request_parts(parts, state).await
    {
        if let Some(token) = tokens::lookup_token(&state.pool, auth.token()).await? {
            return Ok(CurrentUser {
                user_id: token.user_id,
                source: AuthSource::ApiToken,
                scopes: token.scopes,
            });
        }
        return Err(ApiError::unauthorized(
            "invalid_token",
            "the bearer token is invalid, expired, or revoked",
        ));
    }

    let jar = CookieJar::from_headers(&parts.headers);
    if let Some(cookie) = jar.get(sessions::SESSION_COOKIE)
        && let Some(user_id) = sessions::lookup_session(&state.pool, cookie.value()).await?
    {
        return Ok(CurrentUser {
            user_id,
            source: AuthSource::Session,
            scopes: vec!["full".to_string()],
        });
    }

    Err(ApiError::unauthorized(
        "unauthenticated",
        "authentication required",
    ))
}

#[async_trait]
impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        authenticate(parts, state).await
    }
}

/// A caller authenticated specifically by a browser session. Reusing
/// `CurrentUser`'s logic, it rejects API-token callers with 403.
pub struct SessionUser(pub CurrentUser);

#[async_trait]
impl FromRequestParts<AppState> for SessionUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = authenticate(parts, state).await?;
        if user.source != AuthSource::Session {
            return Err(ApiError::forbidden(
                "session_required",
                "this action requires a browser session, not an API token",
            ));
        }
        Ok(SessionUser(user))
    }
}

/// Best-effort client IP for rate-limit keying.
pub struct ClientIp(pub String);

#[async_trait]
impl FromRequestParts<AppState> for ClientIp {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(ClientIp(resolve_ip(parts, &state.config.trusted_proxies)))
    }
}

/// Resolve the client IP used to key rate limiters.
///
/// `X-Forwarded-For` / `X-Real-IP` are spoofable by anyone who can reach the
/// server, so they are honored ONLY when the direct socket peer is a configured
/// trusted proxy ([`TrustedProxies`]). From any other peer the forwarding headers
/// are ignored and the socket address is used — otherwise a brute-forcer could
/// rotate fake `X-Forwarded-For` values to dodge the per-IP login limiter.
///
/// The real server always attaches [`ConnectInfo`] (via
/// `into_make_service_with_connect_info`), so the header is only consulted for a
/// trusted peer. The no-peer branch is reached solely by in-process tests that
/// drive the router with `oneshot`; there we fall back to the forwarding headers,
/// then a constant, so keying still works.
fn resolve_ip(parts: &Parts, trusted: &TrustedProxies) -> String {
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip());

    match peer {
        Some(peer_ip) => {
            if trusted.contains(peer_ip)
                && let Some(forwarded) = forwarded_ip(parts, trusted)
            {
                return forwarded;
            }
            peer_ip.to_string()
        }
        None => forwarded_ip(parts, trusted).unwrap_or_else(|| "unknown".to_string()),
    }
}

/// The real client from `X-Forwarded-For`, walking the list **right-to-left** and
/// skipping our own trusted proxies. Our reverse proxy appends the address it
/// accepted the connection from as the rightmost entry, so the rightmost
/// non-trusted address is the true client; an attacker can only *prepend* fake
/// entries on the left, which this never reaches. Garbage entries are skipped
/// rather than trusted. Falls back to `X-Real-IP`, then `None`.
fn forwarded_ip(parts: &Parts, trusted: &TrustedProxies) -> Option<String> {
    if let Some(xff) = parts
        .headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
    {
        for entry in xff.rsplit(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            match entry.parse::<IpAddr>() {
                // A hop through one of our own proxies — keep walking left.
                Ok(ip) if trusted.contains(ip) => continue,
                Ok(ip) => return Some(ip.to_string()),
                // Unparseable entry: never trust it, keep walking.
                Err(_) => continue,
            }
        }
    }
    if let Some(xri) = parts.headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        let ip = xri.trim();
        if !ip.is_empty() {
            return Some(ip.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    /// Build request `Parts` with the given headers and (optional) socket peer.
    fn parts(peer: Option<&str>, headers: &[(&str, &str)]) -> Parts {
        let mut builder = Request::builder().uri("/");
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        let (mut parts, _) = builder
            .body(axum::body::Body::empty())
            .unwrap()
            .into_parts();
        if let Some(peer) = peer {
            let addr: SocketAddr = peer.parse().unwrap();
            parts.extensions.insert(ConnectInfo(addr));
        }
        parts
    }

    fn trusted(list: &str) -> TrustedProxies {
        TrustedProxies::parse(list).unwrap()
    }

    #[test]
    fn no_peer_no_headers_is_unknown() {
        assert_eq!(resolve_ip(&parts(None, &[]), &trusted("")), "unknown");
    }

    #[test]
    fn no_peer_falls_back_to_forwarding_headers() {
        // Only reachable via in-process `oneshot` tests — no socket peer exists.
        let p = parts(None, &[("x-forwarded-for", "9.9.9.9")]);
        assert_eq!(resolve_ip(&p, &trusted("")), "9.9.9.9");
    }

    #[test]
    fn untrusted_peer_ignores_forwarded_header() {
        // The spoofable header is dropped; keying stays on the real socket addr.
        let p = parts(Some("203.0.113.9:44321"), &[("x-forwarded-for", "1.1.1.1")]);
        assert_eq!(resolve_ip(&p, &trusted("127.0.0.1/32")), "203.0.113.9");
    }

    #[test]
    fn trusted_peer_honors_forwarded_header() {
        let p = parts(Some("127.0.0.1:8080"), &[("x-forwarded-for", "1.1.1.1")]);
        assert_eq!(resolve_ip(&p, &trusted("127.0.0.1/32")), "1.1.1.1");
    }

    #[test]
    fn trusted_peer_prefers_xff_then_real_ip_then_peer() {
        // XFF with only a comma/empty value falls through to X-Real-IP.
        let p = parts(
            Some("127.0.0.1:8080"),
            &[("x-forwarded-for", "   "), ("x-real-ip", "2.2.2.2")],
        );
        assert_eq!(resolve_ip(&p, &trusted("127.0.0.1/32")), "2.2.2.2");
        // No forwarding headers at all → the trusted peer's own address.
        let p = parts(Some("127.0.0.1:8080"), &[]);
        assert_eq!(resolve_ip(&p, &trusted("127.0.0.1/32")), "127.0.0.1");
    }

    #[test]
    fn trusted_peer_takes_rightmost_xff_ignoring_prepended_spoof() {
        // The proxy appends the real client (2.2.2.2) as the rightmost entry; the
        // attacker's prepended 1.1.1.1 is never reached.
        let p = parts(
            Some("127.0.0.1:8080"),
            &[("x-forwarded-for", "1.1.1.1, 2.2.2.2")],
        );
        assert_eq!(resolve_ip(&p, &trusted("127.0.0.1/32")), "2.2.2.2");
    }

    #[test]
    fn walk_skips_trusted_proxy_hops_from_the_right() {
        // Two trusted hops appended after the real client: 9.9.9.9 is returned.
        let p = parts(
            Some("127.0.0.1:8080"),
            &[("x-forwarded-for", "9.9.9.9, 10.0.0.1, 127.0.0.1")],
        );
        assert_eq!(
            resolve_ip(&p, &trusted("127.0.0.1/32, 10.0.0.0/8")),
            "9.9.9.9"
        );
    }

    #[test]
    fn garbage_xff_entries_are_skipped_not_trusted() {
        // A junk rightmost entry is skipped in favor of the next valid address.
        let p = parts(
            Some("127.0.0.1:8080"),
            &[("x-forwarded-for", "8.8.8.8, not-an-ip")],
        );
        assert_eq!(resolve_ip(&p, &trusted("127.0.0.1/32")), "8.8.8.8");
    }
}

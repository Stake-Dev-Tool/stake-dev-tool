//! Workspace custom play domains: the owner-only endpoint that attaches/clears a
//! domain, and the unauthenticated TLS-ask endpoint Caddy calls during on-demand
//! certificate handshakes.
//!
//! A workspace owner points a domain they control (e.g. `play.acme.com`) at this
//! server via a wildcard DNS record (`*.play.acme.com`). Once attached, the
//! workspace's share links resolve at `https://<slug>.play.acme.com/` (in
//! addition to the platform's own `<slug>.<play_domain>` host); the per-hostname
//! certificate is issued on the first visit, gated by [`tls_check`]. The Host
//! dispatch that actually serves those requests lives in [`crate::http`] +
//! [`crate::share::custom`].

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use protocol::{Role, WorkspaceDomain};
use serde::Deserialize;

use crate::AppState;
use crate::api::workspaces::{require_membership, workspace_by_slug};
use crate::auth::extract::CurrentUser;
use crate::error::{ApiError, ApiResult, is_unique_violation};
use crate::share;

/// `PUT /api/workspaces/:slug/domain` — attach (or clear) the workspace's custom
/// play domain. Owner-only. Body `{ "domain": "play.acme.com" }` sets it,
/// `{ "domain": null }` clears it. The domain is validated + lowercased; a
/// collision with another workspace is `409 domain_taken`.
pub async fn set_domain(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
    Json(req): Json<WorkspaceDomain>,
) -> ApiResult<Json<WorkspaceDomain>> {
    let workspace = workspace_by_slug(&state.pool, &slug).await?;
    let role = require_membership(&state.pool, workspace.id, user.user_id).await?;
    if role != Role::Owner {
        return Err(ApiError::forbidden(
            "forbidden",
            "only an owner can change the custom play domain",
        ));
    }

    // Trim + treat empty as clearing; otherwise validate/normalize.
    let normalized = match req
        .domain
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => None,
        Some(raw) => {
            let app_host = state.config.app_host();
            Some(validate_custom_domain(
                raw,
                state.config.play_domain.as_deref(),
                app_host.as_deref(),
            )?)
        }
    };

    let result = sqlx::query("UPDATE workspaces SET custom_play_domain = $2 WHERE id = $1")
        .bind(workspace.id)
        .bind(normalized.as_deref())
        .execute(&state.pool)
        .await;
    if let Err(e) = result {
        if is_unique_violation(&e) {
            return Err(ApiError::conflict(
                "domain_taken",
                "that domain is already attached to another workspace",
            ));
        }
        return Err(e.into());
    }

    // A change must take effect immediately, not after the resolver's 60s TTL.
    state.custom_domains.clear();
    Ok(Json(WorkspaceDomain { domain: normalized }))
}

/// `GET /api/tls-check?domain=<host>` — the on-demand-TLS ask endpoint. UNAUTH:
/// Caddy calls it internally during a TLS handshake to decide whether to issue a
/// certificate for the SNI host. `200` when `<host>` is `<label>.<domain>` for a
/// registered custom play `<domain>` (a single leading label), else `404`.
///
/// Approval intentionally does NOT require a share link to already exist for the
/// label: slugs are created after DNS/cert warm-up, and any wildcard-pointed junk
/// label still resolves to our branded 404 page — harmless. Requiring only that
/// *some workspace claims the domain suffix* keeps the check a single cached
/// lookup while never issuing a cert for a domain nobody attached.
pub async fn tls_check(
    State(state): State<AppState>,
    Query(params): Query<TlsCheckParams>,
) -> StatusCode {
    let Some(domain) = params.domain else {
        return StatusCode::NOT_FOUND;
    };
    let host = domain.trim();
    if host.is_empty() {
        return StatusCode::NOT_FOUND;
    }
    match share::custom::resolve_custom_host(&state.pool, &state.custom_domains, host).await {
        Some((_, workspace_id)) => {
            tracing::info!(host = %host, %workspace_id, "tls-check: approved on-demand certificate");
            StatusCode::OK
        }
        None => StatusCode::NOT_FOUND,
    }
}

#[derive(Deserialize)]
pub struct TlsCheckParams {
    domain: Option<String>,
}

/// Validate and normalize a candidate custom play domain. Returns the lowercase
/// domain, or an `invalid_domain` 422. Rules: a syntactically valid DNS name
/// (≥ 2 labels, each 1-63 chars of `[a-z0-9-]` with no leading/trailing hyphen,
/// total ≤ 253, not an IP literal), and NOT equal to / nested under the
/// platform's own `play_domain` or `app_host` (which would let a tenant hijack
/// our own space).
fn validate_custom_domain(
    raw: &str,
    play_domain: Option<&str>,
    app_host: Option<&str>,
) -> ApiResult<String> {
    let domain = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    if !is_valid_dns_name(&domain) {
        return Err(ApiError::unprocessable(
            "invalid_domain",
            "enter a valid domain like play.acme.com: at least two labels of letters, digits and \
             hyphens, each 1-63 characters with no leading or trailing hyphen",
        ));
    }
    for base in [play_domain, app_host].into_iter().flatten() {
        if domain == base || domain.ends_with(&format!(".{base}")) {
            return Err(ApiError::unprocessable(
                "invalid_domain",
                "that domain overlaps this platform's own domain; use a domain you control",
            ));
        }
    }
    Ok(domain)
}

/// A syntactically valid lowercase DNS name of at least two labels, excluding IP
/// literals. (Presentation-form only; does not resolve the name.)
fn is_valid_dns_name(domain: &str) -> bool {
    if domain.is_empty() || domain.len() > 253 {
        return false;
    }
    // An IP literal (`1.2.3.4`, `::1`) is never a play domain.
    if domain.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    labels.iter().all(|label| {
        let bytes = label.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 63
            && bytes
                .iter()
                .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && bytes.first() != Some(&b'-')
            && bytes.last() != Some(&b'-')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAY: Option<&str> = Some("play.test");

    #[test]
    fn accepts_and_lowercases_valid_domains() {
        assert_eq!(
            validate_custom_domain("Play.Acme.COM", PLAY, None).unwrap(),
            "play.acme.com"
        );
        assert_eq!(
            validate_custom_domain("demo-1.games.example.co.uk.", PLAY, None).unwrap(),
            "demo-1.games.example.co.uk"
        );
    }

    #[test]
    fn rejects_bad_syntax_and_ips() {
        for bad in [
            "nodot",           // single label
            "-lead.com",       // leading hyphen
            "trail-.com",      // trailing hyphen
            "under_score.com", // underscore
            "a..b.com",        // empty label
            "1.2.3.4",         // IPv4 literal
            "space bar.com",   // space
        ] {
            assert!(
                validate_custom_domain(bad, PLAY, None).is_err(),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_our_own_space() {
        let app = Some("app.example.com");
        // Equal to or nested under the play domain.
        assert!(validate_custom_domain("play.test", PLAY, app).is_err());
        assert!(validate_custom_domain("foo.play.test", PLAY, app).is_err());
        // Equal to or nested under the app host.
        assert!(validate_custom_domain("app.example.com", PLAY, app).is_err());
        assert!(validate_custom_domain("x.app.example.com", PLAY, app).is_err());
        // A genuinely different domain is fine.
        assert!(validate_custom_domain("play.acme.com", PLAY, app).is_ok());
    }
}

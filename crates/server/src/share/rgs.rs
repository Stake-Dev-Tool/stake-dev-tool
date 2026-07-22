//! Forward a visitor's `/api/rgs/*` and `/bet/replay/*` traffic into the share
//! tenant's LGS router (the same materialize + `router_for` machinery as M4),
//! enforcing the per-link visitor-session cap on the way in and folding wallet
//! plays into the link's lifetime counters on the way out.

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use crate::AppState;
use crate::lgs_host::RevisionRef;

use super::pages;
use super::resolve::{self, ResolvedShare};
use super::runtime::{Admit, runtime};
use super::tenants;

/// Max wallet body we will buffer to read `sessionID` / counters. Wallet bodies
/// are a few hundred bytes; this only guards against a hostile client.
const MAX_BODY: usize = 1 << 20; // 1 MiB

/// Dispatch a `/api/rgs/*` request. Access (password gate) is assumed already
/// granted by the caller.
pub(super) async fn dispatch_rgs(
    state: &AppState,
    link: &ResolvedShare,
    client_ip: &str,
    req: Request,
) -> Response {
    let path = req.uri().path().to_string();
    let is_wallet = path.contains("/wallet/");
    let is_play = path.ends_with("/wallet/play");

    let (parts, body) = req.into_parts();
    let body_bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return pages::api_error(
                StatusCode::BAD_REQUEST,
                "body_read_error",
                "could not read the request body",
            );
        }
    };

    // Visitor-session accounting on wallet traffic: the client-provided
    // `sessionID` is the visitor identity. New sessions are capped + rate-limited
    // and bump the lifetime `sessions_count`.
    let mut forward_body = body_bytes.to_vec();
    if is_wallet && let Some(session_id) = extract_session_id(&body_bytes) {
        match runtime().note_session(link.id, &session_id, link.max_concurrent_sessions) {
            Admit::OverCap => return too_many_json(),
            Admit::Created => {
                // A brand-new session must clear the workspace's live plan cap: an
                // unpaid (Free) workspace has a 0 concurrent-session cap, so its
                // existing links stop serving new play sessions cleanly. Checked
                // only on Created so the lookup fires once per visitor, not per call.
                if workspace_sessions_blocked(state, link.workspace_id).await {
                    runtime().forget_session(link.id, &session_id);
                    return unavailable_json();
                }
                if !runtime().allow_new_session(link.id, client_ip) {
                    return too_many_json();
                }
                increment_sessions(&state.pool, link.id).await;
            }
            Admit::Existing => {}
        }
        // Namespace the id so a visitor can never present a workbench session id
        // (defense in depth — the share registry is separate regardless).
        if let Some(rewritten) = namespace_session(&body_bytes, &session_id) {
            forward_body = rewritten;
        }
    }

    let router = match tenant_router(state, link).await {
        Ok(router) => router,
        Err(page) => return page,
    };

    let inner = rebuild(
        parts.method,
        parts.uri,
        parts.version,
        parts.headers,
        forward_body,
    );
    let response = match router.oneshot(inner).await {
        Ok(response) => response,
        Err(infallible) => match infallible {},
    };

    if is_play && response.status() == StatusCode::OK {
        return account_play(&state.pool, link.id, response).await;
    }
    response.into_response()
}

/// Dispatch a `/bet/replay/*` request (read-only round replay). No session
/// accounting or counters.
pub(super) async fn dispatch_replay(
    state: &AppState,
    link: &ResolvedShare,
    req: Request,
) -> Response {
    let router = match tenant_router(state, link).await {
        Ok(router) => router,
        Err(page) => return page,
    };
    // Rebuild to drop the outer matched-path extension (see `rebuild`).
    let (parts, body) = req.into_parts();
    let body_bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(bytes) => bytes.to_vec(),
        Err(_) => Vec::new(),
    };
    let inner = rebuild(
        parts.method,
        parts.uri,
        parts.version,
        parts.headers,
        body_bytes,
    );
    match router.oneshot(inner).await {
        Ok(response) => response.into_response(),
        Err(infallible) => match infallible {},
    }
}

/// Resolve the link's revision and build (or reuse) its tenant router.
async fn tenant_router(state: &AppState, link: &ResolvedShare) -> Result<axum::Router, Response> {
    let (number, revision_id) = resolve::resolve_revision(&state.pool, link).await?;
    let host = tenants::host_for(state);
    let rev = RevisionRef {
        workspace_id: link.workspace_id,
        game_id: link.game_id,
        game_slug: &link.game_slug,
        number,
        revision_id,
    };
    host.router_for_revision(state.store.as_ref(), &state.pool, &rev)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "share: failed to build tenant router");
            pages::internal()
        })
}

/// Rebuild a request from its parts, dropping all extensions. axum accumulates
/// matched path params across nested routers as an extension; forwarding the
/// original `Parts` would leak this router's wildcard capture into the inner LGS
/// `Path` extractor. The inner LGS reads no request extensions, so dropping them
/// is safe. (Same technique as `lgs_host::dispatch`.)
fn rebuild(
    method: axum::http::Method,
    uri: axum::http::Uri,
    version: axum::http::Version,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
) -> Request {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .version(version)
        .body(Body::from(body))
        .expect("rebuilding a request from validated parts cannot fail");
    *req.headers_mut() = headers;
    req
}

/// Read `sessionID` out of a wallet JSON body.
fn extract_session_id(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    value.get("sessionID")?.as_str().map(|id| id.to_string())
}

/// Rewrite the body's `sessionID` to `visitor:<id>`. Returns `None` (forward the
/// original bytes) if already namespaced or if the body isn't a JSON object.
fn namespace_session(body: &[u8], session_id: &str) -> Option<Vec<u8>> {
    if session_id.starts_with("visitor:") {
        return None;
    }
    let mut value: Value = serde_json::from_slice(body).ok()?;
    let obj = value.as_object_mut()?;
    obj.insert(
        "sessionID".to_string(),
        Value::String(format!("visitor:{session_id}")),
    );
    serde_json::to_vec(&value).ok()
}

/// Buffer a wallet `/play` response, fold its bet/win into the link counters, and
/// return the (byte-identical) response. Counter fidelity: `round.amount` is the
/// player's selected base stake and `round.payout` the win, both in wallet units
/// — exact for standard base-mode play (cost 1); for bonus-buy modes (cost > 1)
/// the charged total is `amount × mode_cost`, so `total_bet` aggregates base
/// stakes rather than charged totals. This is a documented, best-effort analytics
/// approximation (per the M5 contract).
async fn account_play(pool: &PgPool, share_id: Uuid, response: Response) -> Response {
    let (parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(bytes) => bytes,
        Err(_) => {
            // Give up on counters, but don't drop the response.
            return Response::from_parts(parts, Body::empty());
        }
    };

    let (bet, win) = parse_bet_win(&bytes);
    increment_play(pool, share_id, bet, win).await;

    Response::from_parts(parts, Body::from(bytes))
}

/// Extract `(bet, win)` from a play response JSON; `(0, 0)` if absent.
fn parse_bet_win(body: &[u8]) -> (i64, i64) {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return (0, 0);
    };
    let round = value.get("round");
    let bet = round
        .and_then(|r| r.get("amount"))
        .and_then(|a| a.as_i64())
        .unwrap_or(0);
    let win = round
        .and_then(|r| r.get("payout"))
        .and_then(|p| p.as_i64())
        .unwrap_or(0);
    (bet.max(0), win.max(0))
}

async fn increment_sessions(pool: &PgPool, share_id: Uuid) {
    if let Err(e) =
        sqlx::query("UPDATE share_links SET sessions_count = sessions_count + 1 WHERE id = $1")
            .bind(share_id)
            .execute(pool)
            .await
    {
        tracing::warn!(error = %e, "share: failed to bump sessions_count");
    }
}

async fn increment_play(pool: &PgPool, share_id: Uuid, bet: i64, win: i64) {
    // `$2::bigint::numeric` lets us add i64 deltas to NUMERIC columns without the
    // sqlx bigdecimal/rust_decimal feature (not enabled in this workspace).
    if let Err(e) = sqlx::query(
        "UPDATE share_links \
         SET spins_count = spins_count + 1, \
             total_bet = total_bet + $2::bigint::numeric, \
             total_win = total_win + $3::bigint::numeric \
         WHERE id = $1",
    )
    .bind(share_id)
    .bind(bet)
    .bind(win)
    .execute(pool)
    .await
    {
        tracing::warn!(error = %e, "share: failed to bump play counters");
    }
}

fn too_many_json() -> Response {
    pages::api_error(
        StatusCode::TOO_MANY_REQUESTS,
        "too_many_sessions",
        "this demo is at capacity, please try again later",
    )
}

/// A brand-new visitor session is refused because the workspace's live plan
/// forbids new sessions (a 0 concurrent-session cap — the Free/unpaid state). A
/// clean 403 JSON body, never a 500.
fn unavailable_json() -> Response {
    pages::api_error(
        StatusCode::FORBIDDEN,
        "unavailable",
        "this demo is not currently available",
    )
}

/// True when the workspace's resolved plan forbids new visitor sessions — its
/// concurrent-session cap is `Some(0)`, which is the Free (unpaid) state.
/// Self-hosted instances (billing disabled) resolve to Unlimited → always false,
/// with no DB hit.
async fn workspace_sessions_blocked(state: &AppState, workspace_id: Uuid) -> bool {
    matches!(
        crate::billing::plan_for(state, workspace_id).await,
        Ok(plan) if plan.limits().max_concurrent_share_sessions == Some(0)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_and_namespace_session() {
        let body = br#"{"sessionID":"abc","mode":"base"}"#;
        assert_eq!(extract_session_id(body).as_deref(), Some("abc"));
        let rewritten = namespace_session(body, "abc").unwrap();
        let value: Value = serde_json::from_slice(&rewritten).unwrap();
        assert_eq!(value["sessionID"], "visitor:abc");
        assert_eq!(value["mode"], "base");
        // Already-namespaced ids are left alone.
        assert!(namespace_session(br#"{"sessionID":"visitor:x"}"#, "visitor:x").is_none());
        // Non-JSON bodies have no session id.
        assert!(extract_session_id(b"not json").is_none());
    }

    #[test]
    fn parse_bet_win_reads_round() {
        let body = br#"{"balance":{"amount":9},"round":{"amount":5,"payout":12}}"#;
        assert_eq!(parse_bet_win(body), (5, 12));
        // Missing round -> zeros.
        assert_eq!(parse_bet_win(br#"{"balance":{"amount":9}}"#), (0, 0));
        assert_eq!(parse_bet_win(b"nope"), (0, 0));
    }
}

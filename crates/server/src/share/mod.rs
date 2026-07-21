//! M5 — the public share host: `<slug>.play.<domain>` serves a game's front
//! bundle plus its RGS/replay traffic against the real cloud LGS, to anonymous
//! visitors. Contract: docs/v2/m4-m5-contract.md §M5.
//!
//! This module owns the *host-dispatched* router (mounted by [`crate::http`] via
//! a Host-matching layer, NOT under `/api`). The authenticated dashboard CRUD for
//! creating/listing/patching links + pushing front bundles lives in
//! [`crate::api::shares`].
//!
//! ## Request surface (per resolved, valid link)
//! - `POST /__share/unlock` — password interstitial submit.
//! - `/api/rgs/*` — visitor wallet + RGS traffic → the tenant LGS ([`rgs`]).
//! - `/bet/replay/*` — round replay → the tenant LGS ([`rgs`]).
//! - everything else — front-bundle static files with an `index.html` SPA
//!   fallback ([`statics`]).
//!
//! An unknown/revoked/expired link, or a game with no bundle/revision yet, is a
//! branded [`pages`] response — never a redirect back to the app.

mod pages;
mod resolve;
mod rgs;
mod runtime;
pub(crate) mod slug;
mod statics;
mod tenants;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, post};
use axum::{Extension, Form, Router};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;
use crate::auth::extract::ClientIp;

use resolve::ResolvedShare;
use runtime::runtime;

/// Cookie that unlocks a password-protected link (host-scoped, 12 h server-side
/// TTL). Its value is an opaque random token stored in [`runtime`].
const SHARE_COOKIE: &str = "sdt_share";

/// The subdomain label of the share host, injected as a request extension by the
/// Host-dispatch layer in [`crate::http`].
#[derive(Debug, Clone)]
pub struct ShareHost(pub String);

/// The Host-dispatched share router. It carries [`AppState`] and reads the link
/// slug from the [`ShareHost`] extension.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/__share/unlock", post(unlock))
        .route("/api/rgs/*rest", any(rgs_entry))
        .route("/bet/replay/*rest", any(replay_entry))
        .fallback(static_entry)
}

/// Match a request Host against `<label>.<play_domain>`, returning the single
/// leading label. Caddy preserves the Host header, so it is authoritative; the
/// port (if any) is stripped. Returns `None` for the apex, multi-label hosts, or
/// any non-matching host (which falls through to the app).
pub fn match_share_label(headers: &HeaderMap, play_domain: &str) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let host = host
        .split(':')
        .next()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let suffix = format!(".{play_domain}");
    let label = host.strip_suffix(&suffix)?;
    if label.is_empty() || label.contains('.') {
        return None;
    }
    Some(label.to_string())
}

/// The public `https://<slug>.<play_domain>/` URL for a link, or `None` when no
/// play domain is configured on this instance.
pub(crate) fn public_url(play_domain: Option<&str>, slug: &str) -> Option<String> {
    play_domain.map(|domain| format!("https://{slug}.{domain}/"))
}

/// Best-effort live visitor-session count for a link (this node only).
pub(crate) fn active_sessions(share_id: Uuid) -> i64 {
    runtime().active_sessions(share_id) as i64
}

// ---------------------------------------------------------------------------
// handlers
// ---------------------------------------------------------------------------

async fn rgs_entry(
    Extension(ShareHost(label)): Extension<ShareHost>,
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    req: Request,
) -> Response {
    let link = match resolve::resolve(&state.pool, &label).await {
        Ok(link) => link,
        Err(page) => return page,
    };
    if let Some(locked) = locked_api(&link, req.headers()) {
        return locked;
    }
    rgs::dispatch_rgs(&state, &link, &ip, req).await
}

async fn replay_entry(
    Extension(ShareHost(label)): Extension<ShareHost>,
    State(state): State<AppState>,
    req: Request,
) -> Response {
    let link = match resolve::resolve(&state.pool, &label).await {
        Ok(link) => link,
        Err(page) => return page,
    };
    if let Some(locked) = locked_api(&link, req.headers()) {
        return locked;
    }
    rgs::dispatch_replay(&state, &link, req).await
}

async fn static_entry(
    Extension(ShareHost(label)): Extension<ShareHost>,
    State(state): State<AppState>,
    req: Request,
) -> Response {
    let link = match resolve::resolve(&state.pool, &label).await {
        Ok(link) => link,
        Err(page) => return page,
    };
    // Locked links show the interstitial in place of any asset.
    if link.is_password_protected() && !is_unlocked(req.headers(), link.id) {
        return pages::unlock_form(None);
    }
    let bundle = match resolve::resolve_bundle(&state.pool, &link).await {
        Ok(bundle) => bundle,
        Err(page) => return page,
    };
    statics::serve(&state, link.workspace_id, &bundle, req.uri().path()).await
}

#[derive(Deserialize)]
struct UnlockForm {
    password: String,
}

async fn unlock(
    Extension(ShareHost(label)): Extension<ShareHost>,
    State(state): State<AppState>,
    Form(form): Form<UnlockForm>,
) -> Response {
    let link = match resolve::resolve(&state.pool, &label).await {
        Ok(link) => link,
        Err(page) => return page,
    };
    let Some(hash) = link.password_hash.as_deref() else {
        // Nothing to unlock — send them to the content.
        return redirect_root(&state, None);
    };
    if crate::auth::passwords::verify_password(&form.password, hash) {
        let token = runtime().store_unlock(link.id);
        redirect_root(&state, Some(token))
    } else {
        pages::unlock_form(Some("Incorrect password."))
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// For API-shaped surfaces (RGS/replay): `Some(401)` when the link is locked and
/// the request carries no valid unlock cookie.
fn locked_api(link: &ResolvedShare, headers: &HeaderMap) -> Option<Response> {
    if link.is_password_protected() && !is_unlocked(headers, link.id) {
        return Some(pages::api_error(
            StatusCode::UNAUTHORIZED,
            "locked",
            "this demo requires a password",
        ));
    }
    None
}

fn is_unlocked(headers: &HeaderMap, share_id: Uuid) -> bool {
    let jar = CookieJar::from_headers(headers);
    match jar.get(SHARE_COOKIE) {
        Some(cookie) => runtime().is_unlocked(cookie.value(), share_id),
        None => false,
    }
}

/// A `303 See Other` to `/`, optionally setting the host-scoped unlock cookie
/// (no `Domain` attribute → bound to this exact subdomain).
fn redirect_root(state: &AppState, token: Option<String>) -> Response {
    let mut response = (StatusCode::SEE_OTHER, [(header::LOCATION, "/")]).into_response();
    if let Some(token) = token {
        let secure = if state.config.cookie_secure {
            "; Secure"
        } else {
            ""
        };
        let cookie = format!(
            "{SHARE_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age=43200{secure}"
        );
        if let Ok(value) = header::HeaderValue::from_str(&cookie) {
            response.headers_mut().insert(header::SET_COOKIE, value);
        }
    }
    response
}

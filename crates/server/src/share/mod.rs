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

pub mod custom;
mod pages;
mod resolve;
mod rgs;
mod runtime;
pub(crate) mod slug;
mod statics;
mod tenants;

use std::collections::HashSet;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, post};
use axum::{Extension, Form, Router};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;
use crate::auth::extract::ClientIp;

use resolve::ResolvedShare;
use runtime::{new_visitor_id, runtime};

/// Cookie that unlocks a password-protected link (host-scoped, 12 h server-side
/// TTL). Its value is an opaque random token stored in [`runtime`].
const SHARE_COOKIE: &str = "sdt_share";

/// The subdomain label of the share host, injected as a request extension by the
/// Host-dispatch layer in [`crate::http`].
#[derive(Debug, Clone)]
pub struct ShareHost(pub String);

/// The workspace a request is scoped to. Present ONLY for requests that arrived
/// on a workspace's own custom play domain (`<label>.<custom_play_domain>`);
/// [`resolve`] then scopes the slug lookup to this workspace so one tenant's
/// domain can never serve another tenant's link. Absent for the platform's own
/// `<label>.<play_domain>` host, whose slug lookup stays global.
#[derive(Debug, Clone, Copy)]
pub struct ShareWorkspace(pub Uuid);

/// The Host-dispatched share router. It carries [`AppState`] and reads the link
/// slug from the [`ShareHost`] extension.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/__share/unlock", post(unlock))
        .route("/api/rgs/*rest", any(rgs_entry))
        .route("/bet/replay/*rest", any(replay_entry))
        .fallback(static_entry)
}

/// The request Host with any port and trailing dot stripped, lowercased. `None`
/// when the header is missing or empty. Caddy preserves the Host header, so it is
/// authoritative.
pub fn request_host(headers: &HeaderMap) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let host = host
        .split(':')
        .next()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Match a request Host against `<label>.<play_domain>`, returning the single
/// leading label. Returns `None` for the apex, multi-label hosts, or any
/// non-matching host (which falls through to the app / custom-domain path).
pub fn match_share_label(headers: &HeaderMap, play_domain: &str) -> Option<String> {
    let host = request_host(headers)?;
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
    scope: Option<Extension<ShareWorkspace>>,
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    req: Request,
) -> Response {
    let link = match resolve::resolve(&state.pool, &label, workspace_scope(scope)).await {
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
    scope: Option<Extension<ShareWorkspace>>,
    State(state): State<AppState>,
    req: Request,
) -> Response {
    let link = match resolve::resolve(&state.pool, &label, workspace_scope(scope)).await {
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
    scope: Option<Extension<ShareWorkspace>>,
    State(state): State<AppState>,
    req: Request,
) -> Response {
    let link = match resolve::resolve(&state.pool, &label, workspace_scope(scope)).await {
        Ok(link) => link,
        Err(page) => return page,
    };
    // Locked links show the interstitial in place of any asset.
    if link.is_password_protected() && !is_unlocked(req.headers(), link.id) {
        return pages::unlock_form(None);
    }
    // Front-contract bootstrap: a bare entry-page load with no `sessionID` is
    // redirected to itself with the Stake front-contract params filled in, so a
    // real game front reads `rgs_url` and drives the wallet. Runs after the
    // password gate, before any bundle bytes are served.
    if let Some(redirect) = front_contract_redirect(&state, &link, &req) {
        return redirect;
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
    scope: Option<Extension<ShareWorkspace>>,
    State(state): State<AppState>,
    Form(form): Form<UnlockForm>,
) -> Response {
    let link = match resolve::resolve(&state.pool, &label, workspace_scope(scope)).await {
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

/// The workspace a slug lookup is scoped to, from the optional [`ShareWorkspace`]
/// extension. `None` (play-domain host) keeps the slug lookup global.
fn workspace_scope(scope: Option<Extension<ShareWorkspace>>) -> Option<Uuid> {
    scope.map(|Extension(ShareWorkspace(id))| id)
}

/// Cookie carrying a visitor's stable session id across paramless entry-page
/// loads. Reusing it on a refresh keeps `sessions_count` and the concurrency cap
/// from inflating each time the page reloads. Host-scoped (no `Domain`), 30 days.
const SID_COOKIE: &str = "sdt_share_sid";

/// The Stake front contract expects `sessionID`, `rgs_url`, `lang`, `currency`,
/// `device`, and `social` on the game's entry URL (see the test view's
/// `buildGameUrlFor`). A bare `/` (or `/index.html`) GET that lacks `sessionID`
/// is 302'd to the same path with the *missing* params filled in — so a real game
/// front reads `rgs_url` and drives the wallet instead of silently doing nothing.
/// Params already present (e.g. an explicit `?lang=fr`) are preserved verbatim.
///
/// Returns `None` (serve normally) when the request already carries `sessionID`
/// (the post-redirect load, or the SPA fallback for a client route), or is not a
/// GET to the entry page.
fn front_contract_redirect(
    state: &AppState,
    link: &ResolvedShare,
    req: &Request,
) -> Option<Response> {
    if req.method() != Method::GET {
        return None;
    }
    let path = req.uri().path();
    if path != "/" && path != "/index.html" {
        return None;
    }
    let query = req.uri().query().unwrap_or("");
    let present: HashSet<&str> = query
        .split('&')
        .filter_map(|kv| {
            let key = kv.split('=').next()?;
            (!key.is_empty()).then_some(key)
        })
        .collect();
    if present.contains("sessionID") {
        return None;
    }

    // Reuse the visitor's sid cookie, else mint one and mark it to be set.
    let (sid, set_cookie) = match visitor_sid_cookie(req.headers()) {
        Some(sid) => (sid, false),
        None => (new_visitor_id(), true),
    };

    // rgs_url must be same-origin so wallet calls come back to this share host.
    let host = req.headers().get(header::HOST)?.to_str().ok()?;
    if host.is_empty() {
        return None;
    }
    let rgs_url = format!("{}://{host}/api/rgs/{}", scheme_for(host), link.game_slug);

    // Append only the params the incoming URL is missing, preserving any it has.
    let defaults: [(&str, &str); 6] = [
        ("sessionID", sid.as_str()),
        ("rgs_url", rgs_url.as_str()),
        ("lang", "en"),
        ("currency", "USD"),
        ("device", "desktop"),
        ("social", "false"),
    ];
    let mut qs = query.to_string();
    for (key, value) in defaults {
        if present.contains(key) {
            continue;
        }
        if !qs.is_empty() {
            qs.push('&');
        }
        qs.push_str(key);
        qs.push('=');
        qs.push_str(&encode_query_component(value));
    }
    let location = format!("{path}?{qs}");

    let mut response = (StatusCode::FOUND, [(header::LOCATION, location)]).into_response();
    if set_cookie {
        let secure = if state.config.cookie_secure {
            "; Secure"
        } else {
            ""
        };
        // 30 days = 2_592_000 s. Host-scoped (no Domain attribute).
        let cookie =
            format!("{SID_COOKIE}={sid}; Path=/; HttpOnly; SameSite=Lax; Max-Age=2592000{secure}");
        if let Ok(value) = header::HeaderValue::from_str(&cookie) {
            response.headers_mut().insert(header::SET_COOKIE, value);
        }
    }
    Some(response)
}

/// Read a sane `sdt_share_sid` cookie: non-empty, <= 64 chars, ASCII
/// alphanumeric (so it can never break the query string or inject a header).
/// Anything else is treated as absent so a fresh id is minted.
fn visitor_sid_cookie(headers: &HeaderMap) -> Option<String> {
    let jar = CookieJar::from_headers(headers);
    let value = jar.get(SID_COOKIE)?.value().to_string();
    let ok =
        !value.is_empty() && value.len() <= 64 && value.bytes().all(|b| b.is_ascii_alphanumeric());
    ok.then_some(value)
}

/// `https` for a real host, `http` for localhost / loopback (dev over plain
/// http). Caddy terminates TLS and proxies plain http upstream, so the request's
/// own scheme is unreliable; the Host is the signal.
fn scheme_for(host: &str) -> &'static str {
    let bare = host.split(':').next().unwrap_or(host);
    if bare == "localhost" || bare == "[::1]" || bare.starts_with("127.") {
        "http"
    } else {
        "https"
    }
}

/// Percent-encode a query-parameter value (everything outside the RFC 3986
/// unreserved set), matching how `URLSearchParams` would serialize it.
fn encode_query_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for &b in value.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push(
                    char::from_digit((b >> 4) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
                out.push(
                    char::from_digit((b & 0x0f) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    out
}

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

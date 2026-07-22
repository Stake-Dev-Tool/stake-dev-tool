//! HTTP handlers for the `/api` surface, split by resource. `router()` returns
//! the sub-router `http::build_router` nests under `/api`.

pub mod admin;
pub mod auth;
pub mod billing;
pub mod documents;
pub mod domains;
pub mod invites;
pub mod math;
pub mod shares;
pub mod tokens;
pub mod workspaces;
pub mod ws;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, patch, post, put};

use crate::AppState;
use crate::error::ApiError;

/// Unmatched `/api/*` paths get the JSON error envelope, not the SPA fallback
/// the rest of the router serves.
async fn not_found() -> ApiError {
    ApiError::not_found("not_found", "no such API endpoint")
}

/// All `/api` routes. Note: `POST /invites/:token/accept` takes the token from
/// the path rather than the body — matchit 0.7 (axum 0.7) can't route a static
/// `accept` segment alongside the `:token` param, so a nested accept keeps both
/// the public preview and the accept action under one clean resource path.
///
/// `max_blob_bytes` lifts axum's 2 MiB default body limit on the raw blob
/// upload route only — every JSON route keeps the small default (a giant JSON
/// body would be buffered by the extractor, so the tight default is the
/// protection there).
pub fn router(max_blob_bytes: usize) -> Router<AppState> {
    Router::new()
        // --- auth ---
        .route("/auth/register", post(auth::register))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/auth/providers", get(auth::providers))
        .route("/auth/github/start", get(auth::github_start))
        .route("/auth/github/callback", get(auth::github_callback))
        .route("/auth/device/code", post(auth::device_code))
        .route("/auth/device/token", post(auth::device_token))
        .route("/auth/device/approve", post(auth::device_approve))
        // --- personal API tokens (session-auth only) ---
        .route("/tokens", get(tokens::list).post(tokens::create))
        .route("/tokens/:id", delete(tokens::revoke))
        // --- workspaces ---
        .route(
            "/workspaces",
            get(workspaces::list).post(workspaces::create),
        )
        .route("/workspaces/:slug", get(workspaces::detail))
        .route("/workspaces/:slug/domain", put(domains::set_domain))
        // Unauthenticated: Caddy's on-demand-TLS `ask` endpoint. Kept cheap and
        // secret-free (a cached custom-domain lookup) — it is hit during TLS
        // handshakes for every unknown SNI host.
        .route("/tls-check", get(domains::tls_check))
        .route(
            "/workspaces/:slug/members/:user_id",
            patch(workspaces::update_member).delete(workspaces::remove_member),
        )
        // --- invites ---
        .route(
            "/workspaces/:slug/invites",
            get(invites::list).post(invites::create),
        )
        .route("/workspaces/:slug/invites/:id", delete(invites::revoke))
        .route("/invites/:token", get(invites::preview))
        .route("/invites/:token/accept", post(invites::accept))
        // --- math revisions (M2) ---
        .route("/workspaces/:slug/games", get(math::list_games))
        .route(
            "/workspaces/:slug/games/:game/revisions/check",
            post(math::check),
        )
        .route(
            "/workspaces/:slug/games/:game/blobs/:hash",
            put(math::put_blob)
                .get(math::get_blob)
                .layer(DefaultBodyLimit::max(max_blob_bytes)),
        )
        .route(
            "/workspaces/:slug/games/:game/revisions",
            get(math::list_revisions).post(math::create_revision),
        )
        .route(
            "/workspaces/:slug/games/:game/revisions/:number",
            get(math::revision_detail),
        )
        .route(
            "/workspaces/:slug/games/:game/revisions/:number/diff/:other",
            get(math::revision_diff),
        )
        .route(
            "/workspaces/:slug/games/:game/revisions/:number/files/*path",
            get(math::download_file),
        )
        // --- reserved mount points (stub routers until their milestones land) ---
        .merge(documents::router()) // M3 — document sync + workspace SSE
        .merge(shares::router()) // M5 — share links CRUD
        .merge(billing::router()) // M7 — plans + Polar webhook
        .merge(admin::router()) // instance-admin surface under /admin/…
        .merge(ws::router()) // M4 — cloud LGS under /ws/…
        .fallback(not_found)
}

//! HTTP handlers for the `/api` surface, split by resource. `router()` returns
//! the sub-router `http::build_router` nests under `/api`.

pub mod auth;
pub mod documents;
pub mod invites;
pub mod math;
pub mod tokens;
pub mod workspaces;
pub mod ws;

use axum::Router;
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
pub fn router() -> Router<AppState> {
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
            put(math::put_blob).get(math::get_blob),
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
        .merge(ws::router()) // M4 — cloud LGS under /ws/…
        .fallback(not_found)
}

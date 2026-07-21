//! M4 — cloud-hosted multi-tenant LGS mount.
//!
//! One wildcard route forwards an authenticated, membership-checked request into
//! the per-tenant LGS router for a pinned `(workspace, game, revision)`. The
//! resolution, materialization, tenancy, URI rewrite, and the note that the
//! inner LGS is unauthenticated-by-design all live in [`crate::lgs_host`]. This
//! router is merged into the `/api` router, so the effective mount is
//! `/api/ws/:slug/g/:game/r/:number/*rest`.

use axum::Router;
use axum::routing::any;

use crate::AppState;

pub fn router() -> Router<AppState> {
    // `any` so every method (GET devtool/replay, POST wallet, …) dispatches.
    Router::new().route(
        "/ws/:slug/g/:game/r/:number/*rest",
        any(crate::lgs_host::dispatch),
    )
}

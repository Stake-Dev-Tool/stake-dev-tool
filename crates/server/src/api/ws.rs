//! M4 — cloud-hosted multi-tenant LGS: revision materialization and the
//! per-tenant router dispatch under `/api/ws/:slug/g/:game/r/:number/*rest`.
//! Contract: docs/v2/m4-m5-contract.md. This stub reserves the mount point so
//! the M4 implementation never edits shared router files.

use axum::Router;

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
}

//! M3 — workspace document sync (profiles, saved rounds) + the workspace SSE
//! stream. Contract: docs/v2/m3-contract.md. This stub reserves the mount
//! point so the M3 implementation never edits shared router files.

use axum::Router;

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
}

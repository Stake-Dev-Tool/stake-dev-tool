//! M7 — billing endpoints: checkout initiation, subscription status, and the
//! Polar webhook. Contract: docs/v2/m7-contract.md. This stub reserves the
//! mount point so the M7 implementation never edits shared router files.

use axum::Router;

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
}

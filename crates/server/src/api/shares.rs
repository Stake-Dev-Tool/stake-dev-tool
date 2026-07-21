//! M5 — share links: dashboard CRUD for `<slug>.play.<domain>` hosted game
//! instances. Contract: docs/v2/m4-m5-contract.md §M5. This stub reserves the
//! mount point so the M5 implementation never edits shared router files.

use axum::Router;

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
}

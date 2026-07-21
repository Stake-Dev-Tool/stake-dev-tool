//! The uniform JSON error envelope every `/api` endpoint returns on failure.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Machine-readable `code` plus a human `message`. `code` is a stable string
/// (e.g. `email_taken`) the UI can branch on; `message` is for humans.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

/// Wrapper so failures serialize as `{"error": {"code": ..., "message": ...}}`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct ErrorResponse {
    pub error: ErrorBody,
}

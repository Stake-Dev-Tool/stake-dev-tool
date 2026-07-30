use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("missing field: {0}")]
    MissingField(&'static str),
    #[error("session not found")]
    SessionNotFound,
    #[error("insufficient balance")]
    InsufficientBalance,
    #[error("mode \"{mode}\" not found for game \"{game}\"")]
    ModeNotFound { game: String, mode: String },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("zstd error: {0}")]
    Zstd(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("internal: {0}")]
    Internal(#[from] anyhow::Error),
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::MissingField(_) | AppError::InsufficientBalance => StatusCode::BAD_REQUEST,
            AppError::SessionNotFound | AppError::ModeNotFound { .. } => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        // The cause used to travel in the response body ONLY: server-side all
        // that remained was `tower_http`'s bare "response failed", which made a
        // failing books load invisible unless someone read the client's network
        // tab. Log it here, the one place the cause is still in hand.
        if status.is_server_error() {
            tracing::error!(error = %self, "LGS request failed");
        } else {
            tracing::debug!(error = %self, "LGS request rejected");
        }
        let body = Json(ErrorBody {
            error: self.to_string(),
        });
        (status, body).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;

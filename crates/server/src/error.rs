//! The HTTP error type shared by every `/api` handler. It maps any failure to a
//! status code plus the uniform `{"error": {"code", "message"}}` envelope.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use protocol::{ErrorBody, ErrorResponse};
use thiserror::Error;

pub type ApiResult<T> = Result<T, ApiError>;

/// A failed request: an HTTP status, a stable machine-readable `code`, and a
/// human `message`. Built through the helper constructors so call sites read as
/// `ApiError::conflict("email_taken", "…")`.
#[derive(Debug, Error)]
#[error("{code}: {message}")]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn unauthorized(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, code, message)
    }

    pub fn forbidden(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, code, message)
    }

    pub fn not_found(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, message)
    }

    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    pub fn unprocessable(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, message)
    }

    pub fn too_many_requests(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, code, message)
    }

    /// A 500 that never leaks internals to the client: the real detail is logged,
    /// the response carries only a generic message.
    pub fn internal(detail: impl std::fmt::Display) -> Self {
        tracing::error!(detail = %detail, "internal server error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal server error",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorResponse {
            error: ErrorBody {
                code: self.code.to_string(),
                message: self.message,
            },
        };
        (self.status, Json(body)).into_response()
    }
}

/// Any database error becomes an opaque 500 (logged); callers that need to react
/// to a specific SQLSTATE (e.g. a unique violation) inspect the `sqlx::Error`
/// before it reaches this conversion.
impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        ApiError::internal(err)
    }
}

/// True when the error is a Postgres unique-constraint violation (SQLSTATE
/// 23505) — used to turn a failed insert into a clean 409.
pub fn is_unique_violation(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(db) if db.is_unique_violation())
}

//! Handlers for `/api/tokens`. Session-auth only: a PAT must not mint PATs, so
//! these use the `SessionUser` extractor which rejects Bearer callers.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use protocol::{CreateTokenRequest, CreatedToken, TokensResponse};
use uuid::Uuid;

use crate::AppState;
use crate::auth::extract::SessionUser;
use crate::auth::tokens;
use crate::error::{ApiError, ApiResult};

/// Lists the caller's tokens (metadata only, no secrets).
pub async fn list(
    State(state): State<AppState>,
    SessionUser(user): SessionUser,
) -> ApiResult<Json<TokensResponse>> {
    let tokens = tokens::list_tokens(&state.pool, user.user_id).await?;
    Ok(Json(TokensResponse { tokens }))
}

/// Mints a token, returning the secret exactly once alongside its metadata.
pub async fn create(
    State(state): State<AppState>,
    SessionUser(user): SessionUser,
    Json(req): Json<CreateTokenRequest>,
) -> ApiResult<Json<CreatedToken>> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::unprocessable(
            "invalid_name",
            "token name must not be empty",
        ));
    }
    if req.scopes.is_empty() {
        return Err(ApiError::unprocessable(
            "invalid_scope",
            "at least one scope is required",
        ));
    }
    tokens::validate_scopes(&req.scopes)?;

    let created = tokens::create_token(
        &state.pool,
        user.user_id,
        name,
        &req.scopes,
        req.expires_in_days,
    )
    .await?;
    Ok(Json(created))
}

/// Revokes a token the caller owns. 404 if it isn't theirs (or doesn't exist).
pub async fn revoke(
    State(state): State<AppState>,
    SessionUser(user): SessionUser,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    if tokens::revoke_token(&state.pool, user.user_id, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("token_not_found", "no such token"))
    }
}

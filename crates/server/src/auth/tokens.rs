//! Personal API tokens (`sdt_pat_…`): creation, listing, revocation, and the
//! hashed lookup the `CurrentUser` extractor uses for Bearer auth.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use protocol::{CreatedToken, TokenInfo};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{API_TOKEN_PREFIX, generate_secret, hash_secret};
use crate::error::{ApiError, ApiResult};

/// Scopes M1 recognizes. `full` is everything a session can do; `push:math` is
/// accepted now but only enforced from M2, so it can be minted early for CI.
pub const KNOWN_SCOPES: &[&str] = &["full", "push:math"];

/// Row shape shared by the token queries; maps straight to `TokenInfo`.
#[derive(sqlx::FromRow)]
struct TokenRow {
    id: Uuid,
    name: String,
    scopes: Vec<String>,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    last_used_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

impl From<TokenRow> for TokenInfo {
    fn from(r: TokenRow) -> Self {
        TokenInfo {
            id: r.id,
            name: r.name,
            scopes: r.scopes,
            created_at: r.created_at,
            expires_at: r.expires_at,
            last_used_at: r.last_used_at,
            revoked_at: r.revoked_at,
        }
    }
}

/// A resolved Bearer token: the owning user and the token's granted scopes.
pub struct TokenAuth {
    pub user_id: Uuid,
    pub scopes: Vec<String>,
}

/// Mints a token for `user_id`. Returns the secret (shown once) plus its info.
pub async fn create_token(
    pool: &PgPool,
    user_id: Uuid,
    name: &str,
    scopes: &[String],
    expires_in_days: Option<i64>,
) -> ApiResult<CreatedToken> {
    let secret = generate_secret(API_TOKEN_PREFIX);
    let hash = hash_secret(&secret);
    let expires_at = expires_in_days.map(|d| Utc::now() + ChronoDuration::days(d));
    let row: TokenRow = sqlx::query_as(
        "INSERT INTO api_tokens (user_id, name, token_hash, scopes, expires_at) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, name, scopes, created_at, expires_at, last_used_at, revoked_at",
    )
    .bind(user_id)
    .bind(name)
    .bind(&hash)
    .bind(scopes)
    .bind(expires_at)
    .fetch_one(pool)
    .await?;
    Ok(CreatedToken {
        token: secret,
        info: row.into(),
    })
}

/// Lists a user's tokens, newest first. Never returns hashes or secrets.
pub async fn list_tokens(pool: &PgPool, user_id: Uuid) -> ApiResult<Vec<TokenInfo>> {
    let rows: Vec<TokenRow> = sqlx::query_as(
        "SELECT id, name, scopes, created_at, expires_at, last_used_at, revoked_at \
         FROM api_tokens WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Revokes a token the caller owns. Idempotent (re-revoking keeps the original
/// timestamp). Returns `false` if no such token belongs to the user (→ 404).
pub async fn revoke_token(pool: &PgPool, user_id: Uuid, token_id: Uuid) -> ApiResult<bool> {
    let result = sqlx::query(
        "UPDATE api_tokens SET revoked_at = COALESCE(revoked_at, now()) \
         WHERE id = $1 AND user_id = $2",
    )
    .bind(token_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Resolves a Bearer secret to its user and scopes, rejecting revoked or expired
/// tokens. Bumps `last_used_at` at most once an hour (best-effort).
pub async fn lookup_token(pool: &PgPool, secret: &str) -> ApiResult<Option<TokenAuth>> {
    let hash = hash_secret(secret);
    let row: Option<(Uuid, Vec<String>)> = sqlx::query_as(
        "SELECT user_id, scopes FROM api_tokens \
         WHERE token_hash = $1 AND revoked_at IS NULL \
           AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await?;
    let Some((user_id, scopes)) = row else {
        return Ok(None);
    };

    let _ = sqlx::query(
        "UPDATE api_tokens SET last_used_at = now() \
         WHERE token_hash = $1 \
           AND (last_used_at IS NULL OR last_used_at < now() - INTERVAL '1 hour')",
    )
    .bind(&hash)
    .execute(pool)
    .await;

    Ok(Some(TokenAuth { user_id, scopes }))
}

/// Validates requested scopes against the known set, returning a clean 422 for
/// anything unrecognized.
pub fn validate_scopes(scopes: &[String]) -> ApiResult<()> {
    for scope in scopes {
        if !KNOWN_SCOPES.contains(&scope.as_str()) {
            return Err(ApiError::unprocessable(
                "invalid_scope",
                format!("unknown scope \"{scope}\""),
            ));
        }
    }
    Ok(())
}

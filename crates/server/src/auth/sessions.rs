//! Cookie-backed sessions: creation, hashed lookup with sliding expiry, and the
//! `Set-Cookie` builders.

use axum_extra::extract::cookie::{Cookie, SameSite};
use chrono::{Duration as ChronoDuration, Utc};
use sqlx::PgPool;
use time::Duration as CookieDuration;
use uuid::Uuid;

use crate::auth::{SESSION_PREFIX, generate_secret, hash_secret};
use crate::error::ApiResult;

/// Session cookie name.
pub const SESSION_COOKIE: &str = "sdt_session";
/// Both the DB expiry and the cookie Max-Age.
const SESSION_TTL_DAYS: i64 = 30;

/// Creates a session for `user_id` and returns the full secret (shown once,
/// stored only as its hash).
pub async fn create_session(pool: &PgPool, user_id: Uuid) -> ApiResult<String> {
    let secret = generate_secret(SESSION_PREFIX);
    let hash = hash_secret(&secret);
    let expires_at = Utc::now() + ChronoDuration::days(SESSION_TTL_DAYS);
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, expires_at, last_seen_at) \
         VALUES ($1, $2, $3, now())",
    )
    .bind(&hash)
    .bind(user_id)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(secret)
}

/// Resolves a session secret to its user id, or `None` if the session is unknown
/// or expired. On a hit it slides `expires_at` (and `last_seen_at`) forward, but
/// at most once an hour so a busy client doesn't write on every request.
pub async fn lookup_session(pool: &PgPool, secret: &str) -> ApiResult<Option<Uuid>> {
    let hash = hash_secret(secret);
    let row: Option<(Uuid,)> =
        sqlx::query_as("SELECT user_id FROM sessions WHERE token_hash = $1 AND expires_at > now()")
            .bind(&hash)
            .fetch_optional(pool)
            .await?;
    let Some((user_id,)) = row else {
        return Ok(None);
    };

    // Best-effort sliding renewal; a failure here must not fail the request.
    let new_expiry = Utc::now() + ChronoDuration::days(SESSION_TTL_DAYS);
    let _ = sqlx::query(
        "UPDATE sessions SET last_seen_at = now(), expires_at = $2 \
         WHERE token_hash = $1 \
           AND (last_seen_at IS NULL OR last_seen_at < now() - INTERVAL '1 hour')",
    )
    .bind(&hash)
    .bind(new_expiry)
    .execute(pool)
    .await;

    Ok(Some(user_id))
}

/// Deletes a session by its secret. A no-op if the secret is unknown.
pub async fn delete_session(pool: &PgPool, secret: &str) -> ApiResult<()> {
    let hash = hash_secret(secret);
    sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
        .bind(&hash)
        .execute(pool)
        .await?;
    Ok(())
}

/// Builds the session cookie: HttpOnly, SameSite=Lax, Path=/, 30-day Max-Age.
/// `Secure` is driven by config so local http dev still works.
pub fn session_cookie(value: String, secure: bool) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, value))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .secure(secure)
        .max_age(CookieDuration::days(SESSION_TTL_DAYS))
        .build()
}

/// Builds an expired twin of the session cookie so logout clears it. Attributes
/// (name, path, secure) must match for the browser to overwrite the original.
pub fn clear_cookie(secure: bool) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, ""))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .secure(secure)
        .max_age(CookieDuration::ZERO)
        .build()
}

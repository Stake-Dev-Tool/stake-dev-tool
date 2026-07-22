//! Signup email verification tokens (`sdt_vrf_…`): mint a one-shot secret bound
//! to a user, then redeem it to stamp `users.email_verified_at`.
//!
//! Same storage discipline as password resets: the raw secret is emailed once,
//! only its sha256 is stored, and mint/redeem are `pub` so tests drive the
//! roundtrip by minting a known token directly.

use chrono::{Duration as ChronoDuration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{EMAIL_VERIFY_PREFIX, generate_secret, hash_secret};
use crate::error::ApiResult;

/// Verification tokens live for 24 hours.
pub const VERIFY_TTL_HOURS: i64 = 24;

/// Mints a verification token for `user_id`, storing its hash, and returns the
/// full secret (to be emailed).
pub async fn create_verification(pool: &PgPool, user_id: Uuid) -> ApiResult<String> {
    let secret = generate_secret(EMAIL_VERIFY_PREFIX);
    let hash = hash_secret(&secret);
    let expires_at = Utc::now() + ChronoDuration::hours(VERIFY_TTL_HOURS);
    sqlx::query(
        "INSERT INTO email_verifications (user_id, token_hash, expires_at) VALUES ($1, $2, $3)",
    )
    .bind(user_id)
    .bind(&hash)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(secret)
}

/// Redeems a verification token: stamps `email_verified_at` (idempotent — keeps
/// the earliest timestamp) and marks the token used. Returns `false` when the
/// token is unknown, expired, or already used.
pub async fn consume_verification(pool: &PgPool, token: &str) -> ApiResult<bool> {
    let hash = hash_secret(token);
    let mut tx = pool.begin().await?;

    let claimed: Option<(Uuid,)> = sqlx::query_as(
        "UPDATE email_verifications SET used_at = now() \
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now() \
         RETURNING user_id",
    )
    .bind(&hash)
    .fetch_optional(&mut *tx)
    .await?;

    let Some((user_id,)) = claimed else {
        tx.rollback().await?;
        return Ok(false);
    };

    sqlx::query(
        "UPDATE users SET email_verified_at = COALESCE(email_verified_at, now()) WHERE id = $1",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(true)
}

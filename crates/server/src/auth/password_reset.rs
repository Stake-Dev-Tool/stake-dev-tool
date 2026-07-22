//! Password reset tokens (`sdt_rst_…`): mint a one-shot secret bound to a user,
//! then redeem it to set a new password and revoke every existing session.
//!
//! The raw secret is emailed once; only its sha256 is stored. The mint/redeem
//! functions are `pub` so tests (which can't read the email) drive the roundtrip
//! by minting a known token directly.

use chrono::{Duration as ChronoDuration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{PASSWORD_RESET_PREFIX, generate_secret, hash_secret};
use crate::error::ApiResult;

/// Reset tokens live for one hour.
pub const RESET_TTL_HOURS: i64 = 1;

/// Mints a reset token for `user_id`, storing its hash, and returns the full
/// secret (to be emailed). Shown once; only the hash is persisted.
pub async fn create_reset(pool: &PgPool, user_id: Uuid) -> ApiResult<String> {
    let secret = generate_secret(PASSWORD_RESET_PREFIX);
    let hash = hash_secret(&secret);
    let expires_at = Utc::now() + ChronoDuration::hours(RESET_TTL_HOURS);
    sqlx::query(
        "INSERT INTO password_resets (user_id, token_hash, expires_at) VALUES ($1, $2, $3)",
    )
    .bind(user_id)
    .bind(&hash)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(secret)
}

/// Redeems a reset token: sets `password_hash`, marks the token used, and deletes
/// all of the user's sessions (revoke-everywhere). Returns `false` when the token
/// is unknown, expired, or already used — the handler maps that to `invalid_token`.
///
/// `password_hash` is the already-argon2-hashed value (the handler validates the
/// plaintext length first). The token is claimed atomically so a race can't
/// redeem it twice.
pub async fn consume_reset(pool: &PgPool, token: &str, password_hash: &str) -> ApiResult<bool> {
    let hash = hash_secret(token);
    let mut tx = pool.begin().await?;

    let claimed: Option<(Uuid,)> = sqlx::query_as(
        "UPDATE password_resets SET used_at = now() \
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

    sqlx::query("UPDATE users SET password_hash = $1, updated_at = now() WHERE id = $2")
        .bind(password_hash)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    // Revoke every session so a compromised account is fully locked out.
    sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(true)
}

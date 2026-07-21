//! RFC 8628-shaped device pairing, for the desktop app to obtain an API token
//! by having the user approve a short code in the browser.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use protocol::CreatedToken;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{DEVICE_PREFIX, generate_secret, generate_user_code, hash_secret, tokens};
use crate::error::{ApiError, ApiResult, is_unique_violation};

/// Device codes live for 15 minutes; clients poll no faster than every 5s.
pub const DEVICE_TTL_MINUTES: i64 = 15;
pub const DEVICE_INTERVAL_SECONDS: i64 = 5;

/// The outcome of a token poll, mapped by the handler to RFC 8628 responses.
pub enum DevicePoll {
    /// Not yet approved (`authorization_pending`).
    Pending,
    /// Polled faster than the interval (`slow_down`).
    SlowDown,
    /// Unknown or past its expiry (`expired_token`).
    Expired,
    /// The user rejected the request (`access_denied`).
    Denied,
    /// Approved: a freshly minted API token, returned exactly once.
    Approved(Box<CreatedToken>),
}

/// Creates a pending device code. Returns `(device_code secret, user_code)`.
/// Retries a few times on the astronomically unlikely `user_code` collision.
pub async fn create_device_code(pool: &PgPool) -> ApiResult<(String, String)> {
    let device_code = generate_secret(DEVICE_PREFIX);
    let hash = hash_secret(&device_code);
    let expires_at = Utc::now() + ChronoDuration::minutes(DEVICE_TTL_MINUTES);

    for _ in 0..5 {
        let user_code = generate_user_code();
        let result = sqlx::query(
            "INSERT INTO device_codes (device_code_hash, user_code, expires_at) \
             VALUES ($1, $2, $3)",
        )
        .bind(&hash)
        .bind(&user_code)
        .bind(expires_at)
        .execute(pool)
        .await;
        match result {
            Ok(_) => return Ok((device_code, user_code)),
            Err(e) if is_unique_violation(&e) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(ApiError::internal(
        "could not allocate a unique device user_code",
    ))
}

#[derive(sqlx::FromRow)]
struct DeviceRow {
    id: Uuid,
    user_id: Option<Uuid>,
    user_code: String,
    approved: bool,
    denied: bool,
    expires_at: DateTime<Utc>,
    last_polled_at: Option<DateTime<Utc>>,
}

/// Polls a device code. On approval it mints an API token and removes the code
/// so a repeat poll can't mint a second one.
pub async fn poll_device(pool: &PgPool, device_code: &str) -> ApiResult<DevicePoll> {
    let hash = hash_secret(device_code);
    let row: Option<DeviceRow> = sqlx::query_as(
        "SELECT id, user_id, user_code, approved, denied, expires_at, last_polled_at \
         FROM device_codes WHERE device_code_hash = $1",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Ok(DevicePoll::Expired);
    };
    if row.expires_at < Utc::now() {
        return Ok(DevicePoll::Expired);
    }
    if row.denied {
        return Ok(DevicePoll::Denied);
    }

    // Approval wins regardless of poll cadence: once the user has said yes, the
    // client should always get its token rather than a spurious slow_down.
    if row.approved {
        let user_id = row
            .user_id
            .ok_or_else(|| ApiError::internal("approved device code has no user"))?;
        let created = tokens::create_token(
            pool,
            user_id,
            &format!("Device {}", row.user_code),
            &["full".to_string()],
            None,
        )
        .await?;
        let _ = sqlx::query("DELETE FROM device_codes WHERE id = $1")
            .bind(row.id)
            .execute(pool)
            .await;
        return Ok(DevicePoll::Approved(Box::new(created)));
    }

    // Still pending: throttle over-eager polling.
    let too_soon = row
        .last_polled_at
        .is_some_and(|t| (Utc::now() - t) < ChronoDuration::seconds(DEVICE_INTERVAL_SECONDS));
    let _ = sqlx::query("UPDATE device_codes SET last_polled_at = now() WHERE id = $1")
        .bind(row.id)
        .execute(pool)
        .await;
    if too_soon {
        Ok(DevicePoll::SlowDown)
    } else {
        Ok(DevicePoll::Pending)
    }
}

/// Binds the approving user to a pending code and marks it approved or denied.
/// Errors 404 if the code is unknown or already expired.
pub async fn approve_device(
    pool: &PgPool,
    user_id: Uuid,
    user_code: &str,
    approve: bool,
) -> ApiResult<()> {
    let normalized = user_code.trim().to_uppercase();
    let result = sqlx::query(
        "UPDATE device_codes SET user_id = $1, approved = $2, denied = $3 \
         WHERE user_code = $4 AND expires_at > now()",
    )
    .bind(user_id)
    .bind(approve)
    .bind(!approve)
    .bind(&normalized)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found(
            "device_code_not_found",
            "no pending device code matches that user code",
        ));
    }
    Ok(())
}

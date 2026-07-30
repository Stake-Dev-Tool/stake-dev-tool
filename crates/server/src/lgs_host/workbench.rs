//! Capability tokens for the CROSS-ORIGIN workbench mount (`/api/wb/:token/…`).
//!
//! ## Why this exists
//! The workbench normally serves the test view *and* the game front from the app
//! origin, so the session cookie authorizes `/api/ws/:slug/g/:game/r/:number/…`
//! like any other dashboard call. A front served from the developer's own dev
//! server (`http://localhost:5173`, say) is a different SITE: the `SameSite=Lax`
//! session cookie is never attached to its calls, so that mount can only ever
//! answer 401 — and its CORS preflight, which carries no credentials at all,
//! fails first.
//!
//! A token minted here moves the authorization into the PATH. The front calls
//! `/api/wb/<token>/api/rgs/…` with no ambient credential whatsoever, which is
//! what lets the mount run a CORS layer that mirrors any origin **without**
//! `allow_credentials` — a page that does not know the token gets nothing, and
//! the browser never attaches the user's cookie to that origin regardless.
//!
//! ## What a token authorizes
//! Exactly one `(user, workspace, game, revision)`, for [`TOKEN_TTL_HOURS`].
//! Membership is NOT baked in: [`resolve`] returns the grant and the caller
//! re-checks membership on every request, so revoking someone's workspace access
//! takes effect immediately instead of at token expiry.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{WORKBENCH_PREFIX, generate_secret, hash_secret};
use crate::error::{ApiError, ApiResult};

/// How long a minted token stays valid. Long enough for a working session (the
/// test view mints one per page load), short enough that a leaked URL — the
/// token rides in the game iframe's `rgs_url` query param — goes stale fast.
pub const TOKEN_TTL_HOURS: i64 = 4;

/// What a token authorizes, re-resolved from the database on every request so a
/// renamed game or deleted revision can never be reached through a stale row.
#[derive(Debug, Clone)]
pub struct WorkbenchGrant {
    pub user_id: Uuid,
    pub workspace_id: Uuid,
    pub game_id: Uuid,
    pub game_slug: String,
    pub revision_id: Uuid,
    pub revision_number: i32,
}

/// Mints a token for `(user, workspace, game, revision)`. Returns the secret —
/// shown to the caller exactly once — and its expiry. Expired rows are swept on
/// the way in, which is enough housekeeping for a table this small.
pub async fn mint(
    pool: &PgPool,
    user_id: Uuid,
    workspace_id: Uuid,
    game_id: Uuid,
    revision_id: Uuid,
) -> ApiResult<(String, DateTime<Utc>)> {
    let _ = sqlx::query("DELETE FROM workbench_tokens WHERE expires_at < now()")
        .execute(pool)
        .await;

    let token = generate_secret(WORKBENCH_PREFIX);
    let expires_at = Utc::now() + ChronoDuration::hours(TOKEN_TTL_HOURS);
    sqlx::query(
        "INSERT INTO workbench_tokens \
         (token_hash, user_id, workspace_id, game_id, revision_id, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(hash_secret(&token))
    .bind(user_id)
    .bind(workspace_id)
    .bind(game_id)
    .bind(revision_id)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok((token, expires_at))
}

#[derive(sqlx::FromRow)]
struct GrantRow {
    user_id: Uuid,
    workspace_id: Uuid,
    game_id: Uuid,
    game_slug: String,
    revision_id: Uuid,
    revision_number: i32,
}

/// Resolves a token to its grant. 401 for an unknown, expired, or malformed
/// token — the same answer in every case, so this never reports whether a token
/// ever existed.
pub async fn resolve(pool: &PgPool, token: &str) -> ApiResult<WorkbenchGrant> {
    let unauthorized = || {
        ApiError::unauthorized(
            "invalid_workbench_token",
            "the workbench token is invalid or has expired",
        )
    };
    if !token.starts_with(WORKBENCH_PREFIX) {
        return Err(unauthorized());
    }

    let row: Option<GrantRow> = sqlx::query_as(
        "SELECT wt.user_id, wt.workspace_id, wt.game_id, g.slug AS game_slug, \
                wt.revision_id, r.number AS revision_number \
         FROM workbench_tokens wt \
         JOIN games g ON g.id = wt.game_id \
         JOIN revisions r ON r.id = wt.revision_id \
         WHERE wt.token_hash = $1 AND wt.expires_at > now()",
    )
    .bind(hash_secret(token))
    .fetch_optional(pool)
    .await?;

    let row = row.ok_or_else(unauthorized)?;
    Ok(WorkbenchGrant {
        user_id: row.user_id,
        workspace_id: row.workspace_id,
        game_id: row.game_id,
        game_slug: row.game_slug,
        revision_id: row.revision_id,
        revision_number: row.revision_number,
    })
}

/// The mount prefix a front must call for `token`, e.g. `/api/wb/sdt_wb_…`.
/// Kept next to the router so the path shape is defined exactly once.
pub fn mount_prefix(token: &str) -> String {
    format!("/api/wb/{token}")
}

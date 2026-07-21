//! Handlers for invites: creating/listing/revoking within a workspace, the
//! public preview, and accepting (which grants membership).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use protocol::{
    AcceptInviteResponse, CreateInviteRequest, CreatedInvite, InviteInfo, InvitePreview,
    InvitesResponse, Role, WorkspaceSummary,
};
use uuid::Uuid;

use crate::AppState;
use crate::api::workspaces::{
    WorkspaceRow, require_admin, require_membership, role_from_db, workspace_by_slug,
};
use crate::auth::extract::{CurrentUser, SessionUser};
use crate::auth::{INVITE_PREFIX, generate_secret, hash_secret};
use crate::error::{ApiError, ApiResult};

const DEFAULT_EXPIRY_DAYS: i64 = 7;

#[derive(sqlx::FromRow)]
struct InviteRow {
    id: Uuid,
    role: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    max_uses: i32,
    uses: i32,
    revoked_at: Option<DateTime<Utc>>,
}

fn invite_info(row: InviteRow) -> ApiResult<InviteInfo> {
    Ok(InviteInfo {
        id: row.id,
        role: role_from_db(&row.role)?,
        created_at: row.created_at,
        expires_at: row.expires_at,
        max_uses: row.max_uses,
        uses: row.uses,
        revoked_at: row.revoked_at,
    })
}

/// Creates an invite (owner/admin only). Role must be admin or member.
pub async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
    Json(req): Json<CreateInviteRequest>,
) -> ApiResult<Json<CreatedInvite>> {
    let workspace = workspace_by_slug(&state.pool, &slug).await?;
    let actor_role = require_membership(&state.pool, workspace.id, user.user_id).await?;
    require_admin(actor_role)?;

    if !matches!(req.role, Role::Admin | Role::Member) {
        return Err(ApiError::unprocessable(
            "invalid_role",
            "invite role must be admin or member",
        ));
    }
    let expires_in_days = req.expires_in_days.unwrap_or(DEFAULT_EXPIRY_DAYS);
    if expires_in_days <= 0 {
        return Err(ApiError::unprocessable(
            "invalid_expiry",
            "expires_in_days must be positive",
        ));
    }
    let max_uses = req.max_uses.unwrap_or(0);
    if max_uses < 0 {
        return Err(ApiError::unprocessable(
            "invalid_max_uses",
            "max_uses must be zero (unlimited) or positive",
        ));
    }

    let secret = generate_secret(INVITE_PREFIX);
    let hash = hash_secret(&secret);
    let expires_at = Utc::now() + ChronoDuration::days(expires_in_days);
    let row = sqlx::query_as::<_, InviteRow>(
        "INSERT INTO invites (workspace_id, token_hash, role, created_by, expires_at, max_uses) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, role, created_at, expires_at, max_uses, uses, revoked_at",
    )
    .bind(workspace.id)
    .bind(&hash)
    .bind(req.role.as_str())
    .bind(user.user_id)
    .bind(expires_at)
    .bind(max_uses)
    .fetch_one(&state.pool)
    .await?;

    let invite_url = format!("{}/invite/{}", state.config.public_base_url(), secret);
    Ok(Json(CreatedInvite {
        invite_url,
        token: secret,
        info: invite_info(row)?,
    }))
}

/// Lists a workspace's invites (owner/admin only). No secrets.
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
) -> ApiResult<Json<InvitesResponse>> {
    let workspace = workspace_by_slug(&state.pool, &slug).await?;
    let actor_role = require_membership(&state.pool, workspace.id, user.user_id).await?;
    require_admin(actor_role)?;

    let rows = sqlx::query_as::<_, InviteRow>(
        "SELECT id, role, created_at, expires_at, max_uses, uses, revoked_at \
         FROM invites WHERE workspace_id = $1 ORDER BY created_at DESC",
    )
    .bind(workspace.id)
    .fetch_all(&state.pool)
    .await?;
    let invites = rows
        .into_iter()
        .map(invite_info)
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(InvitesResponse { invites }))
}

/// Revokes an invite (owner/admin only). Idempotent.
pub async fn revoke(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, id)): Path<(String, Uuid)>,
) -> ApiResult<StatusCode> {
    let workspace = workspace_by_slug(&state.pool, &slug).await?;
    let actor_role = require_membership(&state.pool, workspace.id, user.user_id).await?;
    require_admin(actor_role)?;

    let result = sqlx::query(
        "UPDATE invites SET revoked_at = COALESCE(revoked_at, now()) \
         WHERE id = $1 AND workspace_id = $2",
    )
    .bind(id)
    .bind(workspace.id)
    .execute(&state.pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("invite_not_found", "no such invite"));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(sqlx::FromRow)]
struct PreviewRow {
    role: String,
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
    max_uses: i32,
    uses: i32,
    workspace_name: String,
    inviter: Option<String>,
}

/// Public preview for the accept page. Reveals only the workspace name, the
/// offered role, the inviter, and whether the invite is still usable.
pub async fn preview(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> ApiResult<Json<InvitePreview>> {
    let hash = hash_secret(&token);
    let row = sqlx::query_as::<_, PreviewRow>(
        "SELECT i.role, i.expires_at, i.revoked_at, i.max_uses, i.uses, \
                w.name AS workspace_name, u.display_name AS inviter \
         FROM invites i \
         JOIN workspaces w ON w.id = i.workspace_id \
         LEFT JOIN users u ON u.id = i.created_by \
         WHERE i.token_hash = $1",
    )
    .bind(&hash)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("invite_not_found", "no such invite"))?;

    let valid = row.revoked_at.is_none()
        && row.expires_at > Utc::now()
        && (row.max_uses == 0 || row.uses < row.max_uses);
    Ok(Json(InvitePreview {
        workspace_name: row.workspace_name,
        role: role_from_db(&row.role)?,
        inviter_display_name: row.inviter,
        valid,
    }))
}

#[derive(sqlx::FromRow)]
struct AcceptRow {
    id: Uuid,
    workspace_id: Uuid,
    role: String,
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
    max_uses: i32,
    uses: i32,
}

/// Accepts an invite (session-auth), granting membership at the invite's role.
/// Idempotent if already a member (returns the existing role, consumes no use).
/// Expiry, revocation, and `max_uses` are enforced atomically inside the tx.
pub async fn accept(
    State(state): State<AppState>,
    SessionUser(user): SessionUser,
    Path(token): Path<String>,
) -> ApiResult<Json<AcceptInviteResponse>> {
    let hash = hash_secret(&token);
    let mut tx = state.pool.begin().await?;

    // Lock the invite row so concurrent accepts serialize on it.
    let invite = sqlx::query_as::<_, AcceptRow>(
        "SELECT id, workspace_id, role, expires_at, revoked_at, max_uses, uses \
         FROM invites WHERE token_hash = $1 FOR UPDATE",
    )
    .bind(&hash)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("invite_not_found", "no such invite"))?;

    let workspace = sqlx::query_as::<_, WorkspaceRow>(
        "SELECT id, slug, name, created_at FROM workspaces WHERE id = $1",
    )
    .bind(invite.workspace_id)
    .fetch_one(&mut *tx)
    .await?;

    // Idempotent: an existing member keeps their current role; no use consumed.
    let existing: Option<(String,)> =
        sqlx::query_as("SELECT role FROM memberships WHERE workspace_id = $1 AND user_id = $2")
            .bind(invite.workspace_id)
            .bind(user.user_id)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((role,)) = existing {
        let role = role_from_db(&role)?;
        tx.commit().await?;
        return Ok(Json(AcceptInviteResponse {
            workspace: summary(workspace, role),
        }));
    }

    if invite.revoked_at.is_some() {
        return Err(ApiError::forbidden(
            "invite_revoked",
            "this invite has been revoked",
        ));
    }
    if invite.expires_at <= Utc::now() {
        return Err(ApiError::forbidden(
            "invite_expired",
            "this invite has expired",
        ));
    }
    if invite.max_uses != 0 && invite.uses >= invite.max_uses {
        return Err(ApiError::forbidden(
            "invite_exhausted",
            "this invite has no uses left",
        ));
    }

    // Re-check the guard in SQL so the increment is atomic even under races.
    let consumed = sqlx::query(
        "UPDATE invites SET uses = uses + 1 \
         WHERE id = $1 AND revoked_at IS NULL AND expires_at > now() \
           AND (max_uses = 0 OR uses < max_uses)",
    )
    .bind(invite.id)
    .execute(&mut *tx)
    .await?;
    if consumed.rows_affected() == 0 {
        return Err(ApiError::forbidden(
            "invite_exhausted",
            "this invite has no uses left",
        ));
    }

    let role = role_from_db(&invite.role)?;
    sqlx::query("INSERT INTO memberships (workspace_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(invite.workspace_id)
        .bind(user.user_id)
        .bind(role.as_str())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Json(AcceptInviteResponse {
        workspace: summary(workspace, role),
    }))
}

fn summary(workspace: WorkspaceRow, role: Role) -> WorkspaceSummary {
    WorkspaceSummary {
        id: workspace.id,
        slug: workspace.slug,
        name: workspace.name,
        role,
        created_at: workspace.created_at,
    }
}

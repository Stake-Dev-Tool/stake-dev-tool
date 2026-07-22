//! Handlers for `/api/workspaces`, plus the membership helpers `invites`
//! reuses. Every endpoint takes `CurrentUser` (session or PAT).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use protocol::{
    CreateWorkspaceRequest, Role, UpdateMemberRequest, WorkspaceDetail, WorkspaceMember,
    WorkspaceSummary, WorkspacesResponse,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::AppState;
use crate::auth::extract::CurrentUser;
use crate::error::{ApiError, ApiResult, is_unique_violation};

/// The workspace columns handlers need.
#[derive(sqlx::FromRow)]
pub(crate) struct WorkspaceRow {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    /// Attached custom play domain (lowercase), or `None`. See `api::domains`.
    pub custom_play_domain: Option<String>,
}

#[derive(sqlx::FromRow)]
struct SummaryRow {
    id: Uuid,
    slug: String,
    name: String,
    created_at: DateTime<Utc>,
    role: String,
}

#[derive(sqlx::FromRow)]
struct MemberRow {
    id: Uuid,
    display_name: String,
    role: String,
    joined: DateTime<Utc>,
}

/// Creates a workspace and the caller's owner membership atomically.
pub async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<CreateWorkspaceRequest>,
) -> ApiResult<Json<WorkspaceSummary>> {
    let slug = req.slug.trim();
    validate_slug(slug)?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::unprocessable(
            "invalid_name",
            "workspace name must not be empty",
        ));
    }

    // Soft anti-spam gate: on instances with email configured, a user must have
    // confirmed their address before spinning up a workspace. (Everything else —
    // accepting invites, pushing to existing workspaces — stays open, and
    // self-hosted instances without mail never gate.)
    if state.config.mail.is_some() {
        let verified: Option<Option<DateTime<Utc>>> =
            sqlx::query_scalar("SELECT email_verified_at FROM users WHERE id = $1")
                .bind(user.user_id)
                .fetch_optional(&state.pool)
                .await?;
        if !matches!(verified, Some(Some(_))) {
            return Err(ApiError::forbidden(
                "email_unverified",
                "please verify your email address before creating a workspace",
            ));
        }
    }

    // Trial cap: on a billing-enabled instance a free user gets ONE workspace.
    // A new workspace always starts on the trial, so owning any non-paid
    // (Trial/Expired) workspace blocks creating another — this stops trial
    // rotation (spin up a fresh 14-day trial every fortnight). Paid/comped
    // workspaces don't count, and self-hosted (billing disabled) is never capped.
    if state.config.stripe.is_some() {
        let owned = sqlx::query_scalar::<_, Uuid>(
            "SELECT workspace_id FROM memberships WHERE user_id = $1 AND role = 'owner'",
        )
        .bind(user.user_id)
        .fetch_all(&state.pool)
        .await?;
        for ws in owned {
            if !crate::billing::plan_for(&state, ws).await?.is_paid() {
                return Err(ApiError::forbidden(
                    "trial_workspace_limit",
                    "the free trial is limited to one workspace; upgrade an existing \
                     workspace to a paid plan to create more",
                ));
            }
        }
    }

    let mut tx = state.pool.begin().await?;
    let result = sqlx::query_as::<_, WorkspaceRow>(
        "INSERT INTO workspaces (slug, name, created_by) VALUES ($1, $2, $3) \
         RETURNING id, slug, name, created_at, custom_play_domain",
    )
    .bind(slug)
    .bind(name)
    .bind(user.user_id)
    .fetch_one(&mut *tx)
    .await;
    let workspace = match result {
        Ok(workspace) => workspace,
        Err(e) if is_unique_violation(&e) => {
            return Err(ApiError::conflict(
                "slug_taken",
                "that slug is already taken",
            ));
        }
        Err(e) => return Err(e.into()),
    };
    sqlx::query("INSERT INTO memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')")
        .bind(workspace.id)
        .bind(user.user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Json(WorkspaceSummary {
        id: workspace.id,
        slug: workspace.slug,
        name: workspace.name,
        role: Role::Owner,
        created_at: workspace.created_at,
    }))
}

/// Lists the caller's workspaces with their role in each.
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
) -> ApiResult<Json<WorkspacesResponse>> {
    let rows = sqlx::query_as::<_, SummaryRow>(
        "SELECT w.id, w.slug, w.name, w.created_at, m.role \
         FROM workspaces w JOIN memberships m ON m.workspace_id = w.id \
         WHERE m.user_id = $1 ORDER BY w.created_at",
    )
    .bind(user.user_id)
    .fetch_all(&state.pool)
    .await?;

    let workspaces = rows
        .into_iter()
        .map(|r| {
            Ok(WorkspaceSummary {
                id: r.id,
                slug: r.slug,
                name: r.name,
                role: role_from_db(&r.role)?,
                created_at: r.created_at,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(WorkspacesResponse { workspaces }))
}

/// Full workspace view with its members. Members-only.
pub async fn detail(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
) -> ApiResult<Json<WorkspaceDetail>> {
    let workspace = workspace_by_slug(&state.pool, &slug).await?;
    let role = require_membership(&state.pool, workspace.id, user.user_id).await?;
    let members = load_members(&state.pool, workspace.id).await?;
    Ok(Json(WorkspaceDetail {
        id: workspace.id,
        slug: workspace.slug,
        name: workspace.name,
        created_at: workspace.created_at,
        role,
        members,
        custom_play_domain: workspace.custom_play_domain,
    }))
}

/// Changes a member's role, enforcing the owner-protection and no-ownerless
/// rules.
pub async fn update_member(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, target_id)): Path<(String, Uuid)>,
    Json(req): Json<UpdateMemberRequest>,
) -> ApiResult<Json<WorkspaceMember>> {
    let workspace = workspace_by_slug(&state.pool, &slug).await?;
    let actor_role = require_membership(&state.pool, workspace.id, user.user_id).await?;
    if !matches!(actor_role, Role::Owner | Role::Admin) {
        return Err(ApiError::forbidden(
            "forbidden",
            "only owners and admins can change roles",
        ));
    }
    let target_role = membership_role(&state.pool, workspace.id, target_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found("member_not_found", "no such member in this workspace")
        })?;
    let new_role = req.role;

    // An owner's role can only be touched by that owner themselves.
    if target_role == Role::Owner && user.user_id != target_id {
        return Err(ApiError::forbidden(
            "forbidden",
            "an owner's role can only be changed by that owner",
        ));
    }
    // Only an owner may hand out the owner role.
    if new_role == Role::Owner && actor_role != Role::Owner {
        return Err(ApiError::forbidden(
            "forbidden",
            "only an owner can grant the owner role",
        ));
    }
    // An owner stepping down must leave another owner behind.
    if target_role == Role::Owner
        && new_role != Role::Owner
        && count_owners(&state.pool, workspace.id).await? <= 1
    {
        return Err(ApiError::conflict(
            "last_owner",
            "a workspace must keep at least one owner",
        ));
    }

    sqlx::query("UPDATE memberships SET role = $1 WHERE workspace_id = $2 AND user_id = $3")
        .bind(new_role.as_str())
        .bind(workspace.id)
        .bind(target_id)
        .execute(&state.pool)
        .await?;

    let member = sqlx::query_as::<_, MemberRow>(
        "SELECT u.id, u.display_name, m.role, m.created_at AS joined \
         FROM memberships m JOIN users u ON u.id = m.user_id \
         WHERE m.workspace_id = $1 AND m.user_id = $2",
    )
    .bind(workspace.id)
    .bind(target_id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(member_from_row(member)?))
}

/// Removes a member. Members may remove themselves (leave); an owner can only be
/// removed by themselves, and never the last one.
pub async fn remove_member(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, target_id)): Path<(String, Uuid)>,
) -> ApiResult<StatusCode> {
    let workspace = workspace_by_slug(&state.pool, &slug).await?;
    let actor_role = require_membership(&state.pool, workspace.id, user.user_id).await?;
    let target_role = membership_role(&state.pool, workspace.id, target_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found("member_not_found", "no such member in this workspace")
        })?;
    let is_self = user.user_id == target_id;

    if !is_self && !matches!(actor_role, Role::Owner | Role::Admin) {
        return Err(ApiError::forbidden(
            "forbidden",
            "only owners and admins can remove members",
        ));
    }
    if target_role == Role::Owner && !is_self {
        return Err(ApiError::forbidden(
            "forbidden",
            "an owner can only be removed by themselves",
        ));
    }
    if target_role == Role::Owner && count_owners(&state.pool, workspace.id).await? <= 1 {
        return Err(ApiError::conflict(
            "last_owner",
            "a workspace must keep at least one owner",
        ));
    }

    sqlx::query("DELETE FROM memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace.id)
        .bind(target_id)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- shared helpers (also used by `invites`) ---

/// Loads a workspace by slug, or 404.
pub(crate) async fn workspace_by_slug(pool: &PgPool, slug: &str) -> ApiResult<WorkspaceRow> {
    sqlx::query_as::<_, WorkspaceRow>(
        "SELECT id, slug, name, created_at, custom_play_domain FROM workspaces WHERE slug = $1",
    )
    .bind(slug)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("workspace_not_found", "no such workspace"))
}

/// The caller's role in a workspace, if any.
pub(crate) async fn membership_role(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> ApiResult<Option<Role>> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT role FROM memberships WHERE workspace_id = $1 AND user_id = $2")
            .bind(workspace_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    row.map(|(role,)| role_from_db(&role)).transpose()
}

/// Like `membership_role` but 404s non-members (a non-member must not learn the
/// workspace exists).
pub(crate) async fn require_membership(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> ApiResult<Role> {
    membership_role(pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::not_found("workspace_not_found", "no such workspace"))
}

/// 403 unless the role is owner or admin.
pub(crate) fn require_admin(role: Role) -> ApiResult<()> {
    if matches!(role, Role::Owner | Role::Admin) {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "forbidden",
            "only owners and admins can perform this action",
        ))
    }
}

pub(crate) async fn count_owners(pool: &PgPool, workspace_id: Uuid) -> ApiResult<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM memberships WHERE workspace_id = $1 AND role = 'owner'",
    )
    .bind(workspace_id)
    .fetch_one(pool)
    .await?)
}

async fn load_members(pool: &PgPool, workspace_id: Uuid) -> ApiResult<Vec<WorkspaceMember>> {
    let rows = sqlx::query_as::<_, MemberRow>(
        "SELECT u.id, u.display_name, m.role, m.created_at AS joined \
         FROM memberships m JOIN users u ON u.id = m.user_id \
         WHERE m.workspace_id = $1 ORDER BY m.created_at",
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(member_from_row).collect()
}

fn member_from_row(row: MemberRow) -> ApiResult<WorkspaceMember> {
    Ok(WorkspaceMember {
        id: row.id,
        display_name: row.display_name,
        role: role_from_db(&row.role)?,
        joined: row.joined,
    })
}

/// Decodes a `role` column value; an unexpected value means the DB `CHECK` was
/// bypassed, which is a server bug, not a client error.
pub(crate) fn role_from_db(value: &str) -> ApiResult<Role> {
    Role::from_db(value)
        .ok_or_else(|| ApiError::internal(format!("unexpected role in db: {value}")))
}

/// Validates a slug against `^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$`: 3–40 chars,
/// lowercase alphanumerics and hyphens, not starting or ending with a hyphen.
fn validate_slug(slug: &str) -> ApiResult<()> {
    let is_alnum = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let ok = (3..=40).contains(&slug.len())
        && slug.chars().all(|c| is_alnum(c) || c == '-')
        && slug.chars().next().is_some_and(is_alnum)
        && slug.chars().last().is_some_and(is_alnum);
    if ok {
        Ok(())
    } else {
        Err(ApiError::unprocessable(
            "invalid_slug",
            "slug must be 3-40 characters of lowercase letters, digits, and hyphens, \
             and may not start or end with a hyphen",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::validate_slug;

    #[test]
    fn slug_rules() {
        for good in ["abc", "my-workspace", "a1b2", "x".repeat(40).as_str()] {
            assert!(validate_slug(good).is_ok(), "{good} should be valid");
        }
        for bad in [
            "ab",                    // too short
            "-abc",                  // leading hyphen
            "abc-",                  // trailing hyphen
            "AbC",                   // uppercase
            "a b",                   // space
            "under_score",           // underscore
            "x".repeat(41).as_str(), // too long
        ] {
            assert!(validate_slug(bad).is_err(), "{bad} should be invalid");
        }
    }
}

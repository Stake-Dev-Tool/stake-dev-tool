//! Instance-admin surface: the platform-operator (instance-owner) API. Global
//! stats, workspace plan overrides ("comp subscriptions"), user management, and
//! cross-workspace share moderation. Every route is gated by the [`AdminUser`]
//! extractor and mounted under `/api/admin` by [`crate::api::router`].
//!
//! ## Who is an admin
//! A caller is an admin iff their `users.is_admin` flag is set OR their email is
//! in the `SERVER_ADMIN_EMAILS` config allowlist. The allowlist is the bootstrap:
//! it makes the first admin without any SQL, and an admin can then flip
//! `is_admin` on other accounts through this surface.
//!
//! ## Hiding the surface
//! A non-admin (including a member with a valid session, or a PAT lacking the
//! `full` scope) gets a **404** that is byte-identical to the `/api` fallback —
//! never a 403 — so probing cannot confirm the admin surface even exists.

use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::routing::{get, post, put};
use axum::{Json, Router, async_trait};
use chrono::{DateTime, Duration, Utc};
use protocol::admin::{
    AdminMe, AdminOverride, AdminOverview, AdminShare, AdminSharesResponse, AdminUserRow,
    AdminUsersResponse, AdminWorkspace, AdminWorkspacesResponse, DayCount, SetAdminRequest,
    SetOverrideRequest,
};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::AppState;
use crate::auth::extract::CurrentUser;
use crate::billing::plan;
use crate::error::{ApiError, ApiResult};
use crate::share;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/me", get(me))
        .route("/admin/overview", get(overview))
        .route("/admin/workspaces", get(list_workspaces))
        .route("/admin/workspaces/:id/override", put(set_override))
        .route("/admin/users", get(list_users))
        .route("/admin/users/:id/admin", put(set_admin))
        .route("/admin/shares", get(list_shares))
        .route("/admin/shares/:id/revoke", post(revoke_share))
}

// ---------------------------------------------------------------------------
// AdminUser extractor
// ---------------------------------------------------------------------------

/// An authenticated caller who is an instance admin. Resolving it requires a
/// session (or a `full`-scoped PAT) whose account is flagged `is_admin` or whose
/// email is in `SERVER_ADMIN_EMAILS`. Anything else rejects with a hidden 404.
pub struct AdminUser {
    pub user_id: Uuid,
    /// The account email (original case), kept for the self-demotion guard.
    pub email: String,
}

/// The uniform "route does not exist" 404 — identical to the `/api` fallback, so
/// a non-admin cannot distinguish an admin route from an unrouted path.
fn hidden() -> ApiError {
    ApiError::not_found("not_found", "no such API endpoint")
}

/// True when `email` (case-insensitively) is in the `SERVER_ADMIN_EMAILS` list.
fn is_admin_email(state: &AppState, email: &str) -> bool {
    let email = email.to_ascii_lowercase();
    state.config.admin_emails.contains(&email)
}

#[async_trait]
impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Resolve the caller first: a missing/invalid credential still 401s, the
        // same as any authenticated endpoint. A PAT must carry `full`; a narrower
        // token is treated as a non-admin so the surface stays hidden.
        let user = CurrentUser::from_request_parts(parts, state).await?;
        if !user.has_scope("full") {
            return Err(hidden());
        }
        let row: Option<(bool, String)> =
            sqlx::query_as("SELECT is_admin, email FROM users WHERE id = $1")
                .bind(user.user_id)
                .fetch_optional(&state.pool)
                .await?;
        let (is_admin_flag, email) = row.ok_or_else(hidden)?;
        if is_admin_flag || is_admin_email(state, &email) {
            Ok(AdminUser {
                user_id: user.user_id,
                email,
            })
        } else {
            Err(hidden())
        }
    }
}

/// The `?query=` filter shared by the list endpoints. Absent → match everything.
#[derive(Debug, Deserialize)]
struct AdminQuery {
    #[serde(default)]
    query: Option<String>,
}

// ---------------------------------------------------------------------------
// GET /api/admin/me
// ---------------------------------------------------------------------------

/// Reaching this handler at all means the caller is an admin (the extractor 404s
/// otherwise). The dashboard uses it to decide whether to show the admin nav.
async fn me(_admin: AdminUser) -> Json<AdminMe> {
    Json(AdminMe { is_admin: true })
}

// ---------------------------------------------------------------------------
// GET /api/admin/overview
// ---------------------------------------------------------------------------

async fn overview(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> ApiResult<Json<AdminOverview>> {
    // One round-trip for the scalar totals (each column is a cheap subquery).
    let totals: (i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
           (SELECT count(*) FROM users), \
           (SELECT count(*) FROM workspaces), \
           (SELECT count(*) FROM games), \
           (SELECT count(*) FROM revisions), \
           (SELECT count(*) FROM share_links), \
           (SELECT COALESCE(SUM(size), 0)::bigint FROM blobs), \
           (SELECT COALESCE(SUM(sessions_count), 0)::bigint FROM share_links), \
           (SELECT COALESCE(SUM(spins_count), 0)::bigint FROM share_links)",
    )
    .fetch_one(&state.pool)
    .await?;

    let signups_30d = day_series(&state.pool, "users").await?;
    let pushes_30d = day_series(&state.pool, "revisions").await?;

    Ok(Json(AdminOverview {
        users: totals.0,
        workspaces: totals.1,
        games: totals.2,
        revisions: totals.3,
        share_links: totals.4,
        storage_bytes: totals.5,
        sessions_total: totals.6,
        spins_total: totals.7,
        host: host_stats(&state),
        signups_30d,
        pushes_30d,
    }))
}

/// Probe the host's capacity: the disk backing the blob storage (longest
/// mount-point prefix match; falls back to the largest disk) and memory.
/// Best-effort — `None` on any failure so the overview never breaks.
fn host_stats(state: &AppState) -> Option<protocol::HostStats> {
    use sysinfo::{Disks, System};

    let storage_root = match &state.config.storage {
        crate::config::StorageConfig::Fs { root } => {
            root.canonicalize().unwrap_or_else(|_| root.clone())
        }
        // S3-backed instances still report the local disk (the revision cache
        // lives there); probe the current dir's filesystem.
        crate::config::StorageConfig::S3 { .. } => std::env::current_dir().ok()?,
    };

    let disks = Disks::new_with_refreshed_list();
    let disk = disks
        .list()
        .iter()
        .filter(|d| storage_root.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .or_else(|| disks.list().iter().max_by_key(|d| d.total_space()))?;

    let mut sys = System::new();
    sys.refresh_memory();

    Some(protocol::HostStats {
        disk_total_bytes: disk.total_space(),
        disk_free_bytes: disk.available_space(),
        mem_total_bytes: sys.total_memory(),
        mem_used_bytes: sys.used_memory(),
    })
}

/// Per-day `count(*)` over the last 30 days from `<table>.created_at`, with
/// `generate_series` filling empty days with 0. `table` is only ever a hardcoded
/// literal (`"users"`/`"revisions"`), so interpolating it is safe.
async fn day_series(pool: &PgPool, table: &str) -> ApiResult<Vec<DayCount>> {
    let sql = format!(
        "SELECT to_char(g::date, 'YYYY-MM-DD') AS date, COALESCE(c.count, 0)::bigint AS count \
         FROM generate_series(current_date - INTERVAL '29 days', current_date, INTERVAL '1 day') AS g \
         LEFT JOIN ( \
             SELECT created_at::date AS day, count(*) AS count FROM {table} \
             WHERE created_at >= current_date - INTERVAL '29 days' \
             GROUP BY created_at::date \
         ) c ON c.day = g::date \
         ORDER BY g"
    );
    let rows: Vec<(String, i64)> = sqlx::query_as(&sql).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(date, count)| DayCount { date, count })
        .collect())
}

// ---------------------------------------------------------------------------
// GET /api/admin/workspaces  +  PUT /api/admin/workspaces/:id/override
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct AdminWorkspaceRow {
    id: Uuid,
    slug: String,
    name: String,
    created_at: DateTime<Utc>,
    members: i64,
    games: i64,
    storage_bytes: i64,
    override_plan: Option<String>,
    override_expires_at: Option<DateTime<Utc>>,
    override_note: Option<String>,
    subscription_status: Option<String>,
}

/// The admin workspace-row select, with the caller's `WHERE …/ORDER/LIMIT` (or a
/// single-id `WHERE`) appended.
fn admin_workspace_select(tail: &str) -> String {
    format!(
        "SELECT w.id, w.slug, w.name, w.created_at, \
                (SELECT count(*) FROM memberships m WHERE m.workspace_id = w.id) AS members, \
                (SELECT count(*) FROM games g WHERE g.workspace_id = w.id) AS games, \
                (SELECT COALESCE(SUM(b.size), 0)::bigint FROM blobs b WHERE b.workspace_id = w.id) \
                    AS storage_bytes, \
                po.plan AS override_plan, po.expires_at AS override_expires_at, \
                po.note AS override_note, s.status AS subscription_status \
         FROM workspaces w \
         LEFT JOIN plan_overrides po ON po.workspace_id = w.id \
         LEFT JOIN subscriptions s ON s.workspace_id = w.id {tail}"
    )
}

async fn list_workspaces(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<AdminQuery>,
) -> ApiResult<Json<AdminWorkspacesResponse>> {
    let query = params.query.unwrap_or_default();
    let pattern = format!("%{query}%");
    let rows = sqlx::query_as::<_, AdminWorkspaceRow>(&admin_workspace_select(
        "WHERE ($1 = '' OR w.slug ILIKE $2 OR w.name ILIKE $2) \
         ORDER BY w.created_at DESC LIMIT 200",
    ))
    .bind(&query)
    .bind(&pattern)
    .fetch_all(&state.pool)
    .await?;

    let mut workspaces = Vec::with_capacity(rows.len());
    for row in rows {
        workspaces.push(admin_workspace_view(&state, row).await?);
    }
    Ok(Json(AdminWorkspacesResponse { workspaces }))
}

/// Turn a row into the wire view: `plan` is the RESOLVED label (via `plan_for`,
/// so an active override's effect is reflected), while `override` echoes the raw
/// stored row.
async fn admin_workspace_view(
    state: &AppState,
    row: AdminWorkspaceRow,
) -> ApiResult<AdminWorkspace> {
    let plan = plan::plan_for(state, row.id).await?.label().to_string();
    let plan_override = row.override_plan.map(|plan| AdminOverride {
        plan,
        expires_at: row.override_expires_at,
        note: row.override_note,
    });
    Ok(AdminWorkspace {
        id: row.id,
        slug: row.slug,
        name: row.name,
        created_at: row.created_at,
        members: row.members,
        games: row.games,
        storage_bytes: row.storage_bytes,
        plan,
        plan_override,
        subscription_status: row.subscription_status,
    })
}

async fn load_admin_workspace(state: &AppState, id: Uuid) -> ApiResult<AdminWorkspace> {
    let row = sqlx::query_as::<_, AdminWorkspaceRow>(&admin_workspace_select("WHERE w.id = $1"))
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("workspace_not_found", "no such workspace"))?;
    admin_workspace_view(state, row).await
}

/// Upsert (or, on `plan: null`, delete) a workspace's plan override — the "give a
/// subscription" feature. Returns the refreshed workspace row.
async fn set_override(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<Uuid>,
    Json(req): Json<SetOverrideRequest>,
) -> ApiResult<Json<AdminWorkspace>> {
    // A clear 404 for the operator here — not the hidden-surface case.
    ensure_workspace_exists(&state.pool, id).await?;

    match req.plan.as_deref() {
        // Null plan clears any override.
        None => {
            sqlx::query("DELETE FROM plan_overrides WHERE workspace_id = $1")
                .bind(id)
                .execute(&state.pool)
                .await?;
        }
        Some(plan) => {
            if !matches!(plan, "solo" | "team" | "unlimited") {
                return Err(ApiError::unprocessable(
                    "invalid_plan",
                    "plan must be one of \"solo\", \"team\", \"unlimited\", or null",
                ));
            }
            let expires_at = req
                .expires_in_days
                .map(|days| Utc::now() + Duration::days(days));
            sqlx::query(
                "INSERT INTO plan_overrides (workspace_id, plan, expires_at, note, created_by) \
                 VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT (workspace_id) DO UPDATE SET \
                   plan = EXCLUDED.plan, expires_at = EXCLUDED.expires_at, \
                   note = EXCLUDED.note, created_by = EXCLUDED.created_by",
            )
            .bind(id)
            .bind(plan)
            .bind(expires_at)
            .bind(req.note.as_deref())
            .bind(admin.user_id)
            .execute(&state.pool)
            .await?;
        }
    }

    Ok(Json(load_admin_workspace(&state, id).await?))
}

async fn ensure_workspace_exists(pool: &PgPool, id: Uuid) -> ApiResult<()> {
    let exists: Option<Uuid> = sqlx::query_scalar("SELECT id FROM workspaces WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    exists
        .map(|_| ())
        .ok_or_else(|| ApiError::not_found("workspace_not_found", "no such workspace"))
}

// ---------------------------------------------------------------------------
// GET /api/admin/users  +  PUT /api/admin/users/:id/admin
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct UserRow {
    id: Uuid,
    email: String,
    display_name: String,
    created_at: DateTime<Utc>,
    is_admin: bool,
    workspaces: i64,
}

async fn list_users(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<AdminQuery>,
) -> ApiResult<Json<AdminUsersResponse>> {
    let query = params.query.unwrap_or_default();
    let pattern = format!("%{query}%");
    let rows = sqlx::query_as::<_, UserRow>(
        "SELECT u.id, u.email, u.display_name, u.created_at, u.is_admin, \
                (SELECT count(*) FROM memberships m WHERE m.user_id = u.id) AS workspaces \
         FROM users u \
         WHERE ($1 = '' OR u.email ILIKE $2 OR u.display_name ILIKE $2) \
         ORDER BY u.created_at DESC LIMIT 200",
    )
    .bind(&query)
    .bind(&pattern)
    .fetch_all(&state.pool)
    .await?;

    let users = rows
        .into_iter()
        .map(|r| AdminUserRow {
            id: r.id,
            email: r.email,
            display_name: r.display_name,
            created_at: r.created_at,
            is_admin: r.is_admin,
            workspaces: r.workspaces,
        })
        .collect();
    Ok(Json(AdminUsersResponse { users }))
}

/// Toggle a user's admin flag. The one refusal (`409 last_admin`) keeps the
/// instance from locking itself out: an admin cannot drop their OWN flag while
/// they are the last flagged admin and are not also kept admin by the env
/// allowlist.
async fn set_admin(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<Uuid>,
    Json(req): Json<SetAdminRequest>,
) -> ApiResult<Json<AdminMe>> {
    if !req.is_admin && id == admin.user_id && !is_admin_email(&state, &admin.email) {
        let flagged: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE is_admin = true")
            .fetch_one(&state.pool)
            .await?;
        if flagged <= 1 {
            return Err(ApiError::conflict(
                "last_admin",
                "you are the last admin; grant another admin before removing your own access",
            ));
        }
    }

    let updated: Option<bool> =
        sqlx::query_scalar("UPDATE users SET is_admin = $2 WHERE id = $1 RETURNING is_admin")
            .bind(id)
            .bind(req.is_admin)
            .fetch_optional(&state.pool)
            .await?;
    let is_admin = updated.ok_or_else(|| ApiError::not_found("user_not_found", "no such user"))?;
    Ok(Json(AdminMe { is_admin }))
}

// ---------------------------------------------------------------------------
// GET /api/admin/shares  +  POST /api/admin/shares/:id/revoke
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct AdminShareRow {
    id: Uuid,
    slug: String,
    game: String,
    workspace_slug: String,
    sessions_count: i64,
    spins_count: i64,
    revoked_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    custom_play_domain: Option<String>,
}

async fn list_shares(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<AdminQuery>,
) -> ApiResult<Json<AdminSharesResponse>> {
    let query = params.query.unwrap_or_default();
    let pattern = format!("%{query}%");
    let rows = sqlx::query_as::<_, AdminShareRow>(
        "SELECT s.id, s.slug, g.slug AS game, w.slug AS workspace_slug, \
                s.sessions_count, s.spins_count, s.revoked_at, s.created_at, \
                w.custom_play_domain AS custom_play_domain \
         FROM share_links s \
         JOIN games g ON g.id = s.game_id \
         JOIN workspaces w ON w.id = s.workspace_id \
         WHERE ($1 = '' OR s.slug ILIKE $2 OR w.slug ILIKE $2 OR g.slug ILIKE $2) \
         ORDER BY s.created_at DESC LIMIT 200",
    )
    .bind(&query)
    .bind(&pattern)
    .fetch_all(&state.pool)
    .await?;

    let play_domain = state.config.play_domain.as_deref();
    let shares = rows
        .into_iter()
        .map(|row| {
            // Mirror api::shares: a workspace's custom play domain wins over the
            // platform play domain when building the public URL.
            let url = match &row.custom_play_domain {
                Some(domain) => Some(format!("https://{}.{}/", row.slug, domain)),
                None => share::public_url(play_domain, &row.slug),
            };
            AdminShare {
                url,
                id: row.id,
                slug: row.slug,
                workspace_slug: row.workspace_slug,
                game: row.game,
                sessions_count: row.sessions_count,
                spins_count: row.spins_count,
                revoked_at: row.revoked_at,
                created_at: row.created_at,
            }
        })
        .collect();
    Ok(Json(AdminSharesResponse { shares }))
}

/// Revoke a share link instance-wide (idempotent: revoking an already-revoked
/// link keeps its original `revoked_at` and still returns 200).
async fn revoke_share(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let result = sqlx::query(
        "UPDATE share_links SET revoked_at = COALESCE(revoked_at, now()) WHERE id = $1",
    )
    .bind(id)
    .execute(&state.pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("share_not_found", "no such share link"));
    }
    Ok(StatusCode::OK)
}

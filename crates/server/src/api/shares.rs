//! M5 — share links: dashboard CRUD for `<slug>.play.<domain>` hosted game
//! instances, plus front-bundle push. Contract: docs/v2/m4-m5-contract.md §M5.
//!
//! The Host-dispatched *public* share router (what visitors hit) lives in
//! [`crate::share`]; this module is the authenticated `/api` surface the
//! dashboard uses.
//!
//! ## Authorization
//! Front-bundle writes mirror the math push flow: workspace membership **and** the
//! `push:math` scope (a session's implicit `full` scope satisfies it). Share CRUD
//! requires owner/admin. The membership check always runs before the scope/role
//! check so a non-member gets a 404, never a permission-leaking 403.

use std::collections::HashSet;

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use chrono::{DateTime, Duration, Utc};
use protocol::shares::{
    CreateFrontBundleRequest, CreateShareRequest, FrontBundleCreated, FrontBundleSummary,
    FrontBundlesResponse, ShareLinkView, ShareLinksResponse, UpdateShareRequest,
};
use protocol::{
    CheckRequest, CheckResponse, DeletionResult, ErrorBody, FileEntry, MissingBlobsResponse,
};
use serde_json::{Map, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::AppState;
use crate::api::workspaces::{WorkspaceRow, require_admin, require_membership, workspace_by_slug};
use crate::auth::extract::CurrentUser;
use crate::auth::passwords;
use crate::billing;
use crate::blobs;
use crate::error::{ApiError, ApiResult};
use crate::share;

/// Max files in a front bundle (bigger than a math manifest: a web build has many
/// small assets).
const MAX_BUNDLE_FILES: usize = 2000;
/// How many generated-slug candidates to try before giving up.
const SLUG_RETRIES: usize = 8;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/workspaces/:slug/games/:game/front-bundles/check",
            post(front_bundle_check),
        )
        .route(
            "/workspaces/:slug/games/:game/front-bundles",
            get(list_front_bundles).post(create_front_bundle),
        )
        .route(
            "/workspaces/:slug/games/:game/front-bundles/:id",
            axum::routing::delete(delete_front_bundle),
        )
        .route(
            "/workspaces/:slug/games/:game/shares",
            get(list_shares).post(create_share),
        )
        .route(
            "/workspaces/:slug/games/:game/shares/:id",
            axum::routing::patch(update_share).delete(delete_share),
        )
        .route("/workspaces/:slug/games/:game/feedback", get(list_feedback))
        .route(
            "/workspaces/:slug/games/:game/feedback/:id",
            axum::routing::delete(delete_feedback),
        )
        .route(
            "/workspaces/:slug/games/:game/feedback/:id/screenshot",
            get(feedback_screenshot),
        )
}

// ---------------------------------------------------------------------------
// authorization
// ---------------------------------------------------------------------------

/// Resolve the workspace + require membership (read).
async fn authorize_read(
    state: &AppState,
    user: &CurrentUser,
    slug: &str,
) -> ApiResult<WorkspaceRow> {
    let workspace = workspace_by_slug(&state.pool, slug).await?;
    require_membership(&state.pool, workspace.id, user.user_id).await?;
    Ok(workspace)
}

/// Membership + `push:math` (front-bundle writes).
async fn authorize_push(
    state: &AppState,
    user: &CurrentUser,
    slug: &str,
) -> ApiResult<WorkspaceRow> {
    let workspace = authorize_read(state, user, slug).await?;
    user.require_scope("push:math")?;
    Ok(workspace)
}

/// Membership + owner/admin (share CRUD).
async fn authorize_admin(
    state: &AppState,
    user: &CurrentUser,
    slug: &str,
) -> ApiResult<WorkspaceRow> {
    let workspace = workspace_by_slug(&state.pool, slug).await?;
    let role = require_membership(&state.pool, workspace.id, user.user_id).await?;
    require_admin(role)?;
    Ok(workspace)
}

async fn game_id_by_slug(pool: &PgPool, workspace_id: Uuid, slug: &str) -> ApiResult<Uuid> {
    sqlx::query_scalar("SELECT id FROM games WHERE workspace_id = $1 AND slug = $2")
        .bind(workspace_id)
        .bind(slug)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::not_found("game_not_found", "no such game"))
}

// ---------------------------------------------------------------------------
// front bundles
// ---------------------------------------------------------------------------

/// `POST .../front-bundles/check` — which manifest hashes still need uploading
/// (identical content-addressing to math; bundle blobs upload through the
/// EXISTING `PUT .../games/:game/blobs/:hash` endpoint).
async fn front_bundle_check(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, _game)): Path<(String, String)>,
    Json(req): Json<CheckRequest>,
) -> ApiResult<Json<CheckResponse>> {
    let workspace = authorize_push(&state, &user, &slug).await?;
    validate_bundle_manifest(&req.files)?;
    let missing = missing_hashes(&state.pool, workspace.id, &req.files).await?;
    Ok(Json(CheckResponse { missing }))
}

/// `POST .../front-bundles` — commit a front bundle. Validates the manifest
/// (index.html at root, <= 2000 files, path rules), 409s with the shared
/// `missing_blobs` shape if any referenced blob is absent, then stores the
/// `path -> {hash,size}` manifest. On a history-capped plan (Free keeps 1) the
/// older bundles are pruned in the same transaction — shares pinned to a pruned
/// bundle fall back to serving the latest.
async fn create_front_bundle(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, game_slug)): Path<(String, String)>,
    Json(req): Json<CreateFrontBundleRequest>,
) -> ApiResult<Response> {
    let workspace = authorize_push(&state, &user, &slug).await?;
    validate_bundle_manifest(&req.files)?;
    let game_id = game_id_by_slug(&state.pool, workspace.id, &game_slug).await?;
    let limits = billing::limits_for(&state, workspace.id).await?;

    let missing = missing_hashes(&state.pool, workspace.id, &req.files).await?;
    if !missing.is_empty() {
        let body = MissingBlobsResponse {
            error: ErrorBody {
                code: "missing_blobs".to_string(),
                message: format!(
                    "{} blob(s) must be uploaded before this bundle can commit",
                    missing.len()
                ),
            },
            missing,
        };
        return Ok((StatusCode::CONFLICT, Json(body)).into_response());
    }

    let manifest = build_manifest(&req.files);
    let mut tx = state.pool.begin().await?;
    let row: (Uuid, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO front_bundles (game_id, manifest, created_by) VALUES ($1, $2, $3) \
         RETURNING id, created_at",
    )
    .bind(game_id)
    .bind(Value::Object(manifest))
    .bind(user.user_id)
    .fetch_one(&mut *tx)
    .await?;

    // Plan history cap: keep only the newest `keep` bundles (the Free plan keeps
    // 1 — every front push replaces the previous build). Pinned shares fall back
    // to latest, then the workspace's now-orphaned blobs are GC'd in the same tx.
    let mut freed: Vec<(Vec<u8>, i64)> = Vec::new();
    if let Some(keep) = limits.max_front_bundles_per_game {
        let stale: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM front_bundles WHERE game_id = $1 \
             ORDER BY created_at DESC, id DESC OFFSET $2",
        )
        .bind(game_id)
        .bind(i64::from(keep))
        .fetch_all(&mut *tx)
        .await?;
        if !stale.is_empty() {
            sqlx::query(
                "UPDATE share_links SET front_bundle_id = NULL \
                 WHERE game_id = $1 AND front_bundle_id = ANY($2)",
            )
            .bind(game_id)
            .bind(&stale)
            .execute(&mut *tx)
            .await?;
            sqlx::query("DELETE FROM front_bundles WHERE id = ANY($1)")
                .bind(&stale)
                .execute(&mut *tx)
                .await?;
            freed = blobs::gc_orphaned_blobs(&mut tx, workspace.id).await?;
        }
    }
    tx.commit().await?;

    // Post-commit best-effort cleanup of the pruned bundles' store bytes.
    if !freed.is_empty() {
        blobs::delete_blob_objects(&state.store, workspace.id, &freed).await;
    }

    // Nudge the workspace's SSE subscribers that a front bundle landed (mirrors
    // the M2 `revision_pushed` hook), so the test view's version picker refreshes.
    state.events.publish(
        workspace.id,
        crate::documents::WorkspaceEvent::FrontPushed(protocol::FrontPushedEvent {
            game: game_slug.clone(),
            bundle_id: row.0,
        }),
    );

    Ok((
        StatusCode::CREATED,
        Json(FrontBundleCreated {
            id: row.0,
            created_at: row.1,
        }),
    )
        .into_response())
}

/// Row backing [`FrontBundleSummary`]; the counts are derived from the manifest
/// JSONB (one `jsonb_each` scan aggregated per bundle).
#[derive(sqlx::FromRow)]
struct FrontBundleRow {
    id: Uuid,
    created_at: DateTime<Utc>,
    files_count: i64,
    total_size: i64,
}

/// `GET .../front-bundles` — a game's front bundles, newest first (cap 50).
/// `files_count`/`total_size` come from the stored manifest; the newest bundle is
/// flagged `is_latest` (the one a latest-tracking share serves).
async fn list_front_bundles(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, game_slug)): Path<(String, String)>,
) -> ApiResult<Json<FrontBundlesResponse>> {
    let workspace = authorize_read(&state, &user, &slug).await?;
    let game_id = game_id_by_slug(&state.pool, workspace.id, &game_slug).await?;
    let rows = sqlx::query_as::<_, FrontBundleRow>(
        "SELECT fb.id, fb.created_at, \
                count(*)::bigint AS files_count, \
                COALESCE(sum((m.val ->> 'size')::bigint), 0)::bigint AS total_size \
         FROM front_bundles fb \
         CROSS JOIN LATERAL jsonb_each(fb.manifest) AS m(key, val) \
         WHERE fb.game_id = $1 \
         GROUP BY fb.id \
         ORDER BY fb.created_at DESC \
         LIMIT 50",
    )
    .bind(game_id)
    .fetch_all(&state.pool)
    .await?;
    let bundles = rows
        .into_iter()
        .enumerate()
        .map(|(i, r)| FrontBundleSummary {
            id: r.id,
            created_at: r.created_at,
            files_count: r.files_count,
            total_size: r.total_size,
            // Newest-first, so index 0 is the latest bundle a share serves.
            is_latest: i == 0,
        })
        .collect();
    Ok(Json(FrontBundlesResponse { bundles }))
}

/// `DELETE .../front-bundles/:id` — owner/admin only. Guards: `409 bundle_pinned`
/// when a share pins this exact bundle (message lists the slugs); `409
/// last_bundle` when it is the game's only bundle AND any share for the game
/// exists (they would serve nothing). Otherwise delete it and GC the workspace's
/// now-unreferenced blobs, returning the freed storage. 404 for an unknown or
/// foreign id.
async fn delete_front_bundle(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, game_slug, id)): Path<(String, String, Uuid)>,
) -> ApiResult<Json<DeletionResult>> {
    let workspace = authorize_admin(&state, &user, &slug).await?;
    let game_id = game_id_by_slug(&state.pool, workspace.id, &game_slug).await?;

    let belongs: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM front_bundles WHERE id = $1 AND game_id = $2")
            .bind(id)
            .bind(game_id)
            .fetch_optional(&state.pool)
            .await?;
    if belongs.is_none() {
        return Err(ApiError::not_found(
            "bundle_not_found",
            "no such front bundle in this game",
        ));
    }

    // Guard: a share pinning this exact bundle would lose its build.
    let pinned: Vec<String> =
        sqlx::query_scalar("SELECT slug FROM share_links WHERE front_bundle_id = $1 ORDER BY slug")
            .bind(id)
            .fetch_all(&state.pool)
            .await?;
    if !pinned.is_empty() {
        return Err(ApiError::conflict(
            "bundle_pinned",
            format!(
                "this bundle is pinned by {} share link(s): {}",
                pinned.len(),
                pinned.join(", ")
            ),
        ));
    }

    // Guard: deleting the game's only bundle while a share exists leaves every
    // latest-tracking share with nothing to serve.
    let bundle_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM front_bundles WHERE game_id = $1")
            .bind(game_id)
            .fetch_one(&state.pool)
            .await?;
    if bundle_count <= 1 {
        let share_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM share_links WHERE game_id = $1")
                .bind(game_id)
                .fetch_one(&state.pool)
                .await?;
        if share_count > 0 {
            return Err(ApiError::conflict(
                "last_bundle",
                "this is the game's only front bundle and a share link depends on it — \
                 push a newer bundle or delete the share(s) first",
            ));
        }
    }

    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM front_bundles WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let freed = blobs::gc_orphaned_blobs(&mut tx, workspace.id).await?;
    tx.commit().await?;

    blobs::delete_blob_objects(&state.store, workspace.id, &freed).await;

    let freed_bytes = freed.iter().map(|(_, size)| *size).sum();
    Ok(Json(DeletionResult {
        freed_bytes,
        freed_blobs: freed.len() as i64,
    }))
}

/// Build the stored `{ "<path>": { "hash", "size" } }` manifest object.
fn build_manifest(files: &[FileEntry]) -> Map<String, Value> {
    let mut map = Map::with_capacity(files.len());
    for f in files {
        let mut entry = Map::with_capacity(2);
        entry.insert("hash".to_string(), Value::String(f.hash.clone()));
        entry.insert("size".to_string(), Value::from(f.size));
        map.insert(f.path.clone(), Value::Object(entry));
    }
    map
}

/// The manifest hashes not yet present in this workspace's blobs (deduped hex).
async fn missing_hashes(
    pool: &PgPool,
    workspace_id: Uuid,
    files: &[FileEntry],
) -> ApiResult<Vec<String>> {
    let mut seen = HashSet::new();
    let mut pairs: Vec<(String, Vec<u8>)> = Vec::new();
    for f in files {
        if seen.insert(f.hash.as_str())
            && let Some(bytes) = blobs::from_hex(&f.hash)
        {
            pairs.push((f.hash.clone(), bytes));
        }
    }
    let wanted: Vec<Vec<u8>> = pairs.iter().map(|(_, b)| b.clone()).collect();
    let existing: Vec<Vec<u8>> =
        sqlx::query_scalar("SELECT hash FROM blobs WHERE workspace_id = $1 AND hash = ANY($2)")
            .bind(workspace_id)
            .bind(&wanted)
            .fetch_all(pool)
            .await?;
    let existing: HashSet<Vec<u8>> = existing.into_iter().collect();
    Ok(pairs
        .into_iter()
        .filter(|(_, bytes)| !existing.contains(bytes))
        .map(|(hex, _)| hex)
        .collect())
}

// ---------------------------------------------------------------------------
// manifest validation (front bundle)
// ---------------------------------------------------------------------------

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::unprocessable("invalid_manifest", message)
}

/// Front-bundle manifest rules: 1..=2000 files; every path relative, non-empty,
/// backslash-free, `..`-free, control-char-free, <= 512 chars; no duplicate
/// paths; sizes >= 0; hashes exactly 64 lowercase hex; and a root `index.html`.
/// (Mirrors `api::math::validate_manifest`, which is private to that module.)
fn validate_bundle_manifest(files: &[FileEntry]) -> ApiResult<()> {
    if files.is_empty() {
        return Err(invalid("a bundle must list at least one file"));
    }
    if files.len() > MAX_BUNDLE_FILES {
        return Err(invalid(format!(
            "a bundle must list at most {MAX_BUNDLE_FILES} files"
        )));
    }
    let mut seen = HashSet::with_capacity(files.len());
    let mut has_index = false;
    for f in files {
        validate_path(&f.path)?;
        if !seen.insert(f.path.as_str()) {
            return Err(invalid(format!("duplicate path \"{}\"", f.path)));
        }
        if f.size < 0 {
            return Err(invalid(format!("file \"{}\" has a negative size", f.path)));
        }
        if !blobs::is_hex64_lower(&f.hash) {
            return Err(invalid(format!(
                "file \"{}\" hash must be 64 lowercase hex characters",
                f.path
            )));
        }
        if f.path == "index.html" {
            has_index = true;
        }
    }
    if !has_index {
        return Err(invalid("a bundle must contain a root \"index.html\""));
    }
    Ok(())
}

fn validate_path(path: &str) -> ApiResult<()> {
    if path.is_empty() {
        return Err(invalid("a file path must not be empty"));
    }
    if path.len() > 512 {
        return Err(invalid(format!("path \"{path}\" exceeds 512 characters")));
    }
    if path.starts_with('/') {
        return Err(invalid(format!(
            "path \"{path}\" must be relative (no leading '/')"
        )));
    }
    if path.contains('\\') {
        return Err(invalid(format!(
            "path \"{path}\" must not contain a backslash"
        )));
    }
    if path.chars().any(char::is_control) {
        return Err(invalid(format!(
            "path \"{path}\" must not contain control characters"
        )));
    }
    if path.split('/').any(|seg| seg == "..") {
        return Err(invalid(format!(
            "path \"{path}\" must not contain a '..' segment"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// share CRUD
// ---------------------------------------------------------------------------

/// `POST .../shares` — create a share link.
async fn create_share(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, game_slug)): Path<(String, String)>,
    Json(req): Json<CreateShareRequest>,
) -> ApiResult<Response> {
    let workspace = authorize_admin(&state, &user, &slug).await?;
    let game_id = game_id_by_slug(&state.pool, workspace.id, &game_slug).await?;
    let limits = billing::limits_for(&state, workspace.id).await?;

    // Active-link quota (no-op under the UNLIMITED default). Free allows 1.
    if let Some(max) = limits.max_active_share_links {
        let active =
            active_link_count(&state.pool, workspace.id, limits.max_share_link_days).await?;
        if active >= max as i64 {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "upgrade_required",
                format!(
                    "this plan allows at most {max} active share link(s) — \
                     revoke or delete one, or upgrade for more"
                ),
            ));
        }
    }

    // Optional pins are validated so a bad id fails loudly at create time.
    if let Some(number) = req.revision_number {
        ensure_revision_exists(&state.pool, game_id, number).await?;
    }
    if let Some(bundle_id) = req.front_bundle_id {
        ensure_bundle_belongs(&state.pool, game_id, bundle_id).await?;
    }

    let password_hash = match req.password.as_deref() {
        Some(pw) if !pw.is_empty() => Some(passwords::hash_password(pw)?),
        _ => None,
    };
    let expires_at = plan_expiry(req.expires_in_days, &limits)?;
    let max_sessions = clamp_sessions(req.max_concurrent_sessions.unwrap_or(25), &limits);

    let id = insert_share(
        &state,
        &workspace,
        game_id,
        &req,
        password_hash,
        expires_at,
        max_sessions,
        user.user_id,
    )
    .await?;

    let view = load_share_view(&state, id).await?;
    Ok((StatusCode::CREATED, Json(view)).into_response())
}

/// Insert the row, honoring a custom slug (409 on collision) or retrying a
/// generated one.
#[allow(clippy::too_many_arguments)]
async fn insert_share(
    state: &AppState,
    workspace: &WorkspaceRow,
    game_id: Uuid,
    req: &CreateShareRequest,
    password_hash: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    max_sessions: i32,
    created_by: Uuid,
) -> ApiResult<Uuid> {
    if let Some(custom) = req.slug.as_deref() {
        if !share::slug::is_valid_label(custom) {
            return Err(ApiError::unprocessable(
                "invalid_slug",
                "slug must be 1-40 characters of lowercase letters, digits, and hyphens, \
                 and may not start or end with a hyphen",
            ));
        }
        return match try_insert(
            state,
            workspace.id,
            game_id,
            custom,
            req,
            &password_hash,
            expires_at,
            max_sessions,
            created_by,
        )
        .await?
        {
            Some(id) => Ok(id),
            None => Err(ApiError::conflict(
                "slug_taken",
                "that slug is already taken",
            )),
        };
    }

    for _ in 0..SLUG_RETRIES {
        let candidate = share::slug::generate();
        if let Some(id) = try_insert(
            state,
            workspace.id,
            game_id,
            &candidate,
            req,
            &password_hash,
            expires_at,
            max_sessions,
            created_by,
        )
        .await?
        {
            return Ok(id);
        }
    }
    Err(ApiError::conflict(
        "slug_generation_failed",
        "could not allocate a unique slug, please try again",
    ))
}

/// Try one insert; `Ok(None)` on a slug unique-violation so the caller can retry.
#[allow(clippy::too_many_arguments)]
async fn try_insert(
    state: &AppState,
    workspace_id: Uuid,
    game_id: Uuid,
    slug: &str,
    req: &CreateShareRequest,
    password_hash: &Option<String>,
    expires_at: Option<DateTime<Utc>>,
    max_sessions: i32,
    created_by: Uuid,
) -> ApiResult<Option<Uuid>> {
    let result = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO share_links \
           (workspace_id, game_id, slug, revision_number, front_bundle_id, \
            password_hash, expires_at, max_concurrent_sessions, created_by, \
            feedback_enabled) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id",
    )
    .bind(workspace_id)
    .bind(game_id)
    .bind(slug)
    .bind(req.revision_number)
    .bind(req.front_bundle_id)
    .bind(password_hash)
    .bind(expires_at)
    .bind(max_sessions)
    .bind(created_by)
    .bind(req.feedback_enabled.unwrap_or(false))
    .fetch_one(&state.pool)
    .await;
    match result {
        Ok(id) => Ok(Some(id)),
        Err(e) if crate::error::is_unique_violation(&e) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// `GET .../shares` — list a game's share links with counters.
async fn list_shares(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, game_slug)): Path<(String, String)>,
) -> ApiResult<Json<ShareLinksResponse>> {
    let workspace = authorize_read(&state, &user, &slug).await?;
    let game_id = game_id_by_slug(&state.pool, workspace.id, &game_slug).await?;
    let rows = sqlx::query_as::<_, ShareRow>(&share_select(
        "WHERE s.game_id = $1 ORDER BY s.created_at DESC",
    ))
    .bind(game_id)
    .fetch_all(&state.pool)
    .await?;
    let shares = rows
        .into_iter()
        .map(|row| row.into_view(state.config.play_domain.as_deref()))
        .collect();
    Ok(Json(ShareLinksResponse { shares }))
}

/// `PATCH .../shares/:id` — pin/unpin revision or bundle, set/remove password,
/// set/clear expiry, change the session cap, revoke/un-revoke.
async fn update_share(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, game_slug, id)): Path<(String, String, Uuid)>,
    Json(req): Json<UpdateShareRequest>,
) -> ApiResult<Json<ShareLinkView>> {
    let workspace = authorize_admin(&state, &user, &slug).await?;
    let game_id = game_id_by_slug(&state.pool, workspace.id, &game_slug).await?;
    ensure_share_in_game(&state.pool, id, game_id).await?;
    let limits = billing::limits_for(&state, workspace.id).await?;

    // Validate pins before mutating anything.
    if let Some(Some(number)) = req.revision_number {
        ensure_revision_exists(&state.pool, game_id, number).await?;
    }
    if let Some(Some(bundle_id)) = req.front_bundle_id {
        ensure_bundle_belongs(&state.pool, game_id, bundle_id).await?;
    }

    let mut tx = state.pool.begin().await?;
    if let Some(revision_number) = req.revision_number {
        sqlx::query("UPDATE share_links SET revision_number = $2 WHERE id = $1")
            .bind(id)
            .bind(revision_number)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(front_bundle_id) = req.front_bundle_id {
        sqlx::query("UPDATE share_links SET front_bundle_id = $2 WHERE id = $1")
            .bind(id)
            .bind(front_bundle_id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(password) = &req.password {
        let hash = match password.as_deref() {
            Some(pw) if !pw.is_empty() => Some(passwords::hash_password(pw)?),
            _ => None,
        };
        sqlx::query("UPDATE share_links SET password_hash = $2 WHERE id = $1")
            .bind(id)
            .bind(hash)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(expires_in_days) = req.expires_in_days {
        // Same plan rule as create: on a TTL-capped plan the expiry can never be
        // cleared (`null`) nor pushed past the cap.
        let expires_at = plan_expiry(expires_in_days, &limits)?;
        sqlx::query("UPDATE share_links SET expires_at = $2 WHERE id = $1")
            .bind(id)
            .bind(expires_at)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(max) = req.max_concurrent_sessions {
        let clamped = clamp_sessions(max, &limits);
        sqlx::query("UPDATE share_links SET max_concurrent_sessions = $2 WHERE id = $1")
            .bind(id)
            .bind(clamped)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(revoked) = req.revoked {
        if revoked {
            sqlx::query(
                "UPDATE share_links SET revoked_at = COALESCE(revoked_at, now()) WHERE id = $1",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query("UPDATE share_links SET revoked_at = NULL WHERE id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
    }
    if let Some(enabled) = req.feedback_enabled {
        sqlx::query("UPDATE share_links SET feedback_enabled = $2 WHERE id = $1")
            .bind(id)
            .bind(enabled)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    Ok(Json(load_share_view(&state, id).await?))
}

/// `DELETE .../shares/:id`.
async fn delete_share(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, game_slug, id)): Path<(String, String, Uuid)>,
) -> ApiResult<StatusCode> {
    let workspace = authorize_admin(&state, &user, &slug).await?;
    let game_id = game_id_by_slug(&state.pool, workspace.id, &game_slug).await?;
    ensure_share_in_game(&state.pool, id, game_id).await?;
    sqlx::query("DELETE FROM share_links WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn days_from_now(days: i64) -> DateTime<Utc> {
    Utc::now() + Duration::days(days)
}

/// Resolve a requested expiry (`None` = "never") against the plan's share-link
/// TTL cap. Uncapped plans keep the request verbatim. On a capped plan (Free =
/// 7 days) an omitted/cleared expiry gets the plan maximum — links on that plan
/// ALWAYS expire — and a request beyond the cap is refused with a clear
/// `upgrade_required` so scripts and the UI can explain rather than guess.
fn plan_expiry(
    requested_days: Option<i64>,
    limits: &billing::PlanLimits,
) -> ApiResult<Option<DateTime<Utc>>> {
    let Some(max_days) = limits.max_share_link_days else {
        return Ok(requested_days.map(days_from_now));
    };
    let max_days = i64::from(max_days);
    match requested_days {
        Some(days) if days > max_days => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "upgrade_required",
            format!(
                "share links on this plan last at most {max_days} days — \
                 upgrade to keep a link longer"
            ),
        )),
        Some(days) => Ok(Some(days_from_now(days))),
        None => Ok(Some(days_from_now(max_days))),
    }
}

/// Clamp a requested session cap to `>= 1` and to the plan's per-link cap.
fn clamp_sessions(requested: i32, limits: &billing::PlanLimits) -> i32 {
    let value = requested.max(1);
    match limits.max_concurrent_share_sessions {
        Some(cap) => value.min(cap as i32),
        None => value,
    }
}

/// Non-revoked, non-expired links in a workspace (the quota's "active" count).
/// On a TTL-capped plan links past their plan lifetime (which the share host no
/// longer serves) don't count, so a dead grandfathered link can't hold the slot.
async fn active_link_count(
    pool: &PgPool,
    workspace_id: Uuid,
    ttl_days: Option<u32>,
) -> ApiResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM share_links \
         WHERE workspace_id = $1 AND revoked_at IS NULL \
           AND (expires_at IS NULL OR expires_at > now()) \
           AND ($2::int IS NULL OR created_at + make_interval(days => $2::int) > now())",
    )
    .bind(workspace_id)
    .bind(ttl_days.map(|d| d as i32))
    .fetch_one(pool)
    .await?)
}

async fn ensure_revision_exists(pool: &PgPool, game_id: Uuid, number: i32) -> ApiResult<()> {
    let exists: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM revisions WHERE game_id = $1 AND number = $2")
            .bind(game_id)
            .bind(number)
            .fetch_optional(pool)
            .await?;
    exists.map(|_| ()).ok_or_else(|| {
        ApiError::unprocessable("revision_not_found", "no such revision in this game")
    })
}

async fn ensure_bundle_belongs(pool: &PgPool, game_id: Uuid, bundle_id: Uuid) -> ApiResult<()> {
    let exists: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM front_bundles WHERE id = $1 AND game_id = $2")
            .bind(bundle_id)
            .bind(game_id)
            .fetch_optional(pool)
            .await?;
    exists.map(|_| ()).ok_or_else(|| {
        ApiError::unprocessable("bundle_not_found", "no such front bundle in this game")
    })
}

async fn ensure_share_in_game(pool: &PgPool, id: Uuid, game_id: Uuid) -> ApiResult<()> {
    let exists: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM share_links WHERE id = $1 AND game_id = $2")
            .bind(id)
            .bind(game_id)
            .fetch_optional(pool)
            .await?;
    exists
        .map(|_| ())
        .ok_or_else(|| ApiError::not_found("share_not_found", "no such share link"))
}

/// The columns for a `ShareLinkView`, with the NUMERIC counters cast to `float8`
/// (no bigdecimal/rust_decimal feature is enabled). Joins the workspace so the
/// URL can prefer the workspace's custom play domain. `where_order` is appended.
fn share_select(where_order: &str) -> String {
    format!(
        "SELECT s.id, s.slug, g.slug AS game, s.revision_number, s.front_bundle_id, \
                (s.password_hash IS NOT NULL) AS password_protected, s.expires_at, \
                s.max_concurrent_sessions, s.revoked_at, s.created_at, \
                s.sessions_count, s.spins_count, \
                s.total_bet::float8 AS total_bet, s.total_win::float8 AS total_win, \
                s.feedback_enabled, \
                w.custom_play_domain AS custom_play_domain \
         FROM share_links s JOIN games g ON g.id = s.game_id \
         JOIN workspaces w ON w.id = s.workspace_id {where_order}"
    )
}

#[derive(sqlx::FromRow)]
struct ShareRow {
    id: Uuid,
    slug: String,
    game: String,
    revision_number: Option<i32>,
    front_bundle_id: Option<Uuid>,
    password_protected: bool,
    expires_at: Option<DateTime<Utc>>,
    max_concurrent_sessions: i32,
    revoked_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    sessions_count: i64,
    spins_count: i64,
    total_bet: f64,
    total_win: f64,
    feedback_enabled: bool,
    custom_play_domain: Option<String>,
}

impl ShareRow {
    fn into_view(self, play_domain: Option<&str>) -> ShareLinkView {
        let observed_rtp = if self.total_bet > 0.0 {
            Some(self.total_win / self.total_bet)
        } else {
            None
        };
        // A workspace's attached custom domain takes precedence over the
        // platform play domain (both hosts resolve; this is the URL we surface).
        let url = match &self.custom_play_domain {
            Some(domain) => Some(format!("https://{}.{}/", self.slug, domain)),
            None => share::public_url(play_domain, &self.slug),
        };
        ShareLinkView {
            url,
            active_sessions: share::active_sessions(self.id),
            observed_rtp,
            id: self.id,
            slug: self.slug,
            game: self.game,
            revision_number: self.revision_number,
            front_bundle_id: self.front_bundle_id,
            password_protected: self.password_protected,
            expires_at: self.expires_at,
            max_concurrent_sessions: self.max_concurrent_sessions,
            revoked_at: self.revoked_at,
            created_at: self.created_at,
            sessions_count: self.sessions_count,
            spins_count: self.spins_count,
            total_bet: self.total_bet,
            total_win: self.total_win,
            feedback_enabled: self.feedback_enabled,
        }
    }
}

async fn load_share_view(state: &AppState, id: Uuid) -> ApiResult<ShareLinkView> {
    let row = sqlx::query_as::<_, ShareRow>(&share_select("WHERE s.id = $1"))
        .bind(id)
        .fetch_one(&state.pool)
        .await?;
    Ok(row.into_view(state.config.play_domain.as_deref()))
}

// ---------------------------------------------------------------------------
// visitor feedback (read/delete surface; submission is public, in crate::share)
// ---------------------------------------------------------------------------
// These wire types live here (not in `protocol`) because `drawing` is a
// free-form `serde_json::Value` and `protocol` deliberately has no serde_json
// dependency — the same trade-off as the documents wire types.

/// Newest-first page size for the feedback list.
const FEEDBACK_LIST_LIMIT: i64 = 200;

/// One feedback entry as listed by `GET .../feedback`. `mode` + `event_id` +
/// `revision_number` address the book line the visitor was reacting to (the
/// same `(revision, mode, eventId)` triplet as saved rounds); all three are
/// null when feedback arrived before the first spin. The screenshot bytes are
/// NOT inlined — fetch them via `GET .../feedback/:id/screenshot` when
/// `has_screenshot` is true.
#[derive(serde::Serialize, sqlx::FromRow)]
struct FeedbackView {
    id: Uuid,
    share_id: Uuid,
    share_slug: String,
    session_id: Option<String>,
    author_name: Option<String>,
    message: String,
    drawing: Option<Value>,
    has_screenshot: bool,
    mode: Option<String>,
    event_id: Option<i32>,
    revision_number: Option<i32>,
    viewport_w: Option<i32>,
    viewport_h: Option<i32>,
    created_at: DateTime<Utc>,
}

#[derive(serde::Serialize)]
struct FeedbackListResponse {
    feedback: Vec<FeedbackView>,
}

#[derive(serde::Deserialize)]
struct FeedbackListQuery {
    /// Optional share-link id filter.
    share: Option<Uuid>,
}

const FEEDBACK_SELECT: &str = "SELECT f.id, f.share_link_id AS share_id, s.slug AS share_slug, \
            f.session_id, f.author_name, f.message, f.drawing, \
            (f.screenshot IS NOT NULL) AS has_screenshot, \
            f.mode, f.event_id, f.revision_number, f.viewport_w, f.viewport_h, \
            f.created_at \
     FROM share_feedback f JOIN share_links s ON s.id = f.share_link_id";

/// `GET .../feedback[?share=<id>]` — a game's visitor feedback across all of
/// its share links, newest first (membership; read-only like the shares list).
async fn list_feedback(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, game_slug)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<FeedbackListQuery>,
) -> ApiResult<Json<FeedbackListResponse>> {
    let workspace = authorize_read(&state, &user, &slug).await?;
    let game_id = game_id_by_slug(&state.pool, workspace.id, &game_slug).await?;
    let feedback = match query.share {
        Some(share_id) => {
            sqlx::query_as::<_, FeedbackView>(&format!(
                "{FEEDBACK_SELECT} WHERE s.game_id = $1 AND f.share_link_id = $3 \
                 ORDER BY f.created_at DESC LIMIT $2"
            ))
            .bind(game_id)
            .bind(FEEDBACK_LIST_LIMIT)
            .bind(share_id)
            .fetch_all(&state.pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, FeedbackView>(&format!(
                "{FEEDBACK_SELECT} WHERE s.game_id = $1 \
                 ORDER BY f.created_at DESC LIMIT $2"
            ))
            .bind(game_id)
            .bind(FEEDBACK_LIST_LIMIT)
            .fetch_all(&state.pool)
            .await?
        }
    };
    Ok(Json(FeedbackListResponse { feedback }))
}

/// `DELETE .../feedback/:id` — owner/admin, like the rest of share management.
async fn delete_feedback(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, game_slug, id)): Path<(String, String, Uuid)>,
) -> ApiResult<StatusCode> {
    let workspace = authorize_admin(&state, &user, &slug).await?;
    let game_id = game_id_by_slug(&state.pool, workspace.id, &game_slug).await?;
    let deleted = sqlx::query(
        "DELETE FROM share_feedback f USING share_links s \
         WHERE f.id = $1 AND s.id = f.share_link_id AND s.game_id = $2",
    )
    .bind(id)
    .bind(game_id)
    .execute(&state.pool)
    .await?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::not_found(
            "feedback_not_found",
            "no such feedback entry",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `GET .../feedback/:id/screenshot` — the stored capture bytes (membership).
async fn feedback_screenshot(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, game_slug, id)): Path<(String, String, Uuid)>,
) -> ApiResult<Response> {
    let workspace = authorize_read(&state, &user, &slug).await?;
    let game_id = game_id_by_slug(&state.pool, workspace.id, &game_slug).await?;
    let row: Option<(Option<Vec<u8>>, Option<String>)> = sqlx::query_as(
        "SELECT f.screenshot, f.screenshot_mime \
         FROM share_feedback f JOIN share_links s ON s.id = f.share_link_id \
         WHERE f.id = $1 AND s.game_id = $2",
    )
    .bind(id)
    .bind(game_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((Some(bytes), mime)) = row else {
        return Err(ApiError::not_found(
            "screenshot_not_found",
            "this feedback entry has no screenshot",
        ));
    };
    // Only the mimes the public submit endpoint accepts are ever stored.
    let content_type = match mime.as_deref() {
        Some("image/png") => "image/png",
        Some("image/webp") => "image/webp",
        _ => "image/jpeg",
    };
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, content_type),
            // Immutable per entry; private since it sits behind membership.
            (axum::http::header::CACHE_CONTROL, "private, max-age=86400"),
        ],
        bytes,
    )
        .into_response())
}

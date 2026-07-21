//! Handlers for M2 "Math revisions": content-addressed blob upload with
//! per-workspace dedup, immutable numbered revisions, file diffs, and
//! per-revision bet stats.
//!
//! Writes (`check`, blob `PUT`, `revisions` commit) require workspace
//! membership **and** the `push:math` scope (a session's implicit `full` scope
//! satisfies it). Reads require membership only. The membership check runs
//! before the scope check so a non-member always gets a 404, never a
//! scope-leaking 403.

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use object_store::{ObjectStoreExt, WriteMultipart};
use protocol::{
    BlobUploaded, ChangedFile, CheckRequest, CheckResponse, CreateRevisionRequest, ErrorBody,
    FileDiff, FileEntry, GameSummary, GamesResponse, MissingBlobsResponse, ModeStats,
    ModeStatsDiff, RevisionDetail, RevisionDiff, RevisionStats, RevisionSummary, RevisionsResponse,
    StatsDiff, StatsStatus,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::AppState;
use crate::api::workspaces::{WorkspaceRow, require_membership, workspace_by_slug};
use crate::auth::extract::CurrentUser;
use crate::blobs;
use crate::error::{ApiError, ApiResult};
use crate::stats;

// ---------------------------------------------------------------------------
// Manifest validation (shared, unit-tested)
// ---------------------------------------------------------------------------

fn invalid_manifest(message: impl Into<String>) -> ApiError {
    ApiError::unprocessable("invalid_manifest", message)
}

/// Validates a push manifest: 1..=1000 files; every path relative, non-empty,
/// backslash-free, `..`-free, control-char-free, and <= 512 chars; no duplicate
/// paths; sizes >= 0; hashes exactly 64 lowercase hex; and exactly one root
/// `index.json`. On failure returns `422 invalid_manifest` with a precise
/// message.
fn validate_manifest(files: &[FileEntry]) -> ApiResult<()> {
    if files.is_empty() {
        return Err(invalid_manifest("manifest must list at least one file"));
    }
    if files.len() > 1000 {
        return Err(invalid_manifest("manifest must list at most 1000 files"));
    }
    let mut seen = HashSet::with_capacity(files.len());
    let mut index_count = 0usize;
    for f in files {
        validate_path(&f.path)?;
        if !seen.insert(f.path.as_str()) {
            return Err(invalid_manifest(format!("duplicate path \"{}\"", f.path)));
        }
        if f.size < 0 {
            return Err(invalid_manifest(format!(
                "file \"{}\" has a negative size",
                f.path
            )));
        }
        if !blobs::is_hex64_lower(&f.hash) {
            return Err(invalid_manifest(format!(
                "file \"{}\" hash must be 64 lowercase hex characters",
                f.path
            )));
        }
        if f.path == "index.json" {
            index_count += 1;
        }
    }
    if index_count != 1 {
        return Err(invalid_manifest(
            "manifest must contain exactly one root \"index.json\"",
        ));
    }
    Ok(())
}

fn validate_path(path: &str) -> ApiResult<()> {
    if path.is_empty() {
        return Err(invalid_manifest("a file path must not be empty"));
    }
    if path.len() > 512 {
        return Err(invalid_manifest(format!(
            "path \"{path}\" exceeds 512 characters"
        )));
    }
    if path.starts_with('/') {
        return Err(invalid_manifest(format!(
            "path \"{path}\" must be relative (no leading '/')"
        )));
    }
    if path.contains('\\') {
        return Err(invalid_manifest(format!(
            "path \"{path}\" must not contain a backslash"
        )));
    }
    if path.chars().any(char::is_control) {
        return Err(invalid_manifest(format!(
            "path \"{path}\" must not contain control characters"
        )));
    }
    if path.split('/').any(|seg| seg == "..") {
        return Err(invalid_manifest(format!(
            "path \"{path}\" must not contain a '..' segment"
        )));
    }
    Ok(())
}

/// Game slugs follow the workspace slug rule: `^[a-z0-9][a-z0-9-]{1,38}[a-z0-9]$`.
fn validate_game_slug(slug: &str) -> ApiResult<()> {
    let is_alnum = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let ok = (3..=40).contains(&slug.len())
        && slug.chars().all(|c| is_alnum(c) || c == '-')
        && slug.chars().next().is_some_and(is_alnum)
        && slug.chars().last().is_some_and(is_alnum);
    if ok {
        Ok(())
    } else {
        Err(ApiError::unprocessable(
            "invalid_game_slug",
            "game slug must be 3-40 characters of lowercase letters, digits, and hyphens, \
             and may not start or end with a hyphen",
        ))
    }
}

// ---------------------------------------------------------------------------
// Authorization helpers
// ---------------------------------------------------------------------------

/// Resolve the workspace and require membership (reads).
async fn authorize_read(
    state: &AppState,
    user: &CurrentUser,
    slug: &str,
) -> ApiResult<WorkspaceRow> {
    let workspace = workspace_by_slug(&state.pool, slug).await?;
    require_membership(&state.pool, workspace.id, user.user_id).await?;
    Ok(workspace)
}

/// Resolve the workspace, require membership, then require `push:math`. Scope is
/// checked last so a non-member 404s rather than leaking existence via 403.
async fn authorize_write(
    state: &AppState,
    user: &CurrentUser,
    slug: &str,
) -> ApiResult<WorkspaceRow> {
    let workspace = authorize_read(state, user, slug).await?;
    user.require_scope("push:math")?;
    Ok(workspace)
}

// ---------------------------------------------------------------------------
// check
// ---------------------------------------------------------------------------

pub async fn check(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, _game)): Path<(String, String)>,
    Json(req): Json<CheckRequest>,
) -> ApiResult<Json<CheckResponse>> {
    let workspace = authorize_write(&state, &user, &slug).await?;
    validate_manifest(&req.files)?;
    let missing = missing_hashes(&state.pool, workspace.id, &req.files).await?;
    Ok(Json(CheckResponse { missing }))
}

/// The distinct (hex, bytes) hash pairs of a manifest, in first-seen order.
fn distinct_pairs(files: &[FileEntry]) -> Vec<(String, Vec<u8>)> {
    let mut seen = HashSet::new();
    let mut pairs = Vec::new();
    for f in files {
        if seen.insert(f.hash.as_str())
            && let Some(bytes) = blobs::from_hex(&f.hash)
        {
            pairs.push((f.hash.clone(), bytes));
        }
    }
    pairs
}

/// Which manifest hashes are not yet in this workspace's blobs (deduped hex).
async fn missing_hashes(
    pool: &PgPool,
    workspace_id: Uuid,
    files: &[FileEntry],
) -> ApiResult<Vec<String>> {
    let pairs = distinct_pairs(files);
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
// blob upload / download
// ---------------------------------------------------------------------------

async fn blob_size(pool: &PgPool, workspace_id: Uuid, hash: &[u8]) -> ApiResult<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT size FROM blobs WHERE workspace_id = $1 AND hash = $2")
            .bind(workspace_id)
            .bind(hash)
            .fetch_optional(pool)
            .await?,
    )
}

/// Streams a raw body into the object store, hashing as it goes. 201 on a fresh
/// upload, 200 if the blob already exists (body ignored), 422 on hash mismatch,
/// 413 past the size cap.
pub async fn put_blob(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, _game, hash_hex)): Path<(String, String, String)>,
    request: Request,
) -> ApiResult<Response> {
    let workspace = authorize_write(&state, &user, &slug).await?;
    if !blobs::is_hex64_lower(&hash_hex) {
        return Err(ApiError::unprocessable(
            "invalid_hash",
            "blob hash must be 64 lowercase hex characters",
        ));
    }
    let expected = blobs::from_hex(&hash_hex).expect("validated hex");

    // Idempotent: the workspace already holds this blob — don't re-read the body.
    if let Some(size) = blob_size(&state.pool, workspace.id, &expected).await? {
        return Ok((
            StatusCode::OK,
            Json(BlobUploaded {
                hash: hash_hex,
                size,
            }),
        )
            .into_response());
    }

    let key = blobs::blob_key(workspace.id, &hash_hex);
    let upload = state
        .store
        .put_multipart(&key)
        .await
        .map_err(ApiError::internal)?;
    let mut writer = WriteMultipart::new(upload);
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    let max = state.config.storage_max_blob_bytes;

    let mut stream = request.into_body().into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(e) => {
                writer.abort().await.ok();
                return Err(ApiError::bad_request(
                    "body_read_error",
                    format!("failed to read the request body: {e}"),
                ));
            }
        };
        total += chunk.len() as u64;
        if total > max {
            writer.abort().await.ok();
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                format!("blob exceeds the {max}-byte limit"),
            ));
        }
        hasher.update(&chunk);
        writer.write(&chunk);
    }

    if hasher.finalize().as_slice() != expected.as_slice() {
        writer.abort().await.ok();
        return Err(ApiError::unprocessable(
            "hash_mismatch",
            "the uploaded bytes do not match the declared hash",
        ));
    }
    writer.finish().await.map_err(ApiError::internal)?;

    // Record the blob only after the store write fully succeeds. ON CONFLICT
    // makes concurrent identical uploads idempotent.
    sqlx::query(
        "INSERT INTO blobs (workspace_id, hash, size) VALUES ($1, $2, $3) \
         ON CONFLICT (workspace_id, hash) DO NOTHING",
    )
    .bind(workspace.id)
    .bind(&expected)
    .bind(total as i64)
    .execute(&state.pool)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(BlobUploaded {
            hash: hash_hex,
            size: total as i64,
        }),
    )
        .into_response())
}

pub async fn get_blob(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, _game, hash_hex)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    let workspace = authorize_read(&state, &user, &slug).await?;
    if !blobs::is_hex64_lower(&hash_hex) {
        return Err(ApiError::unprocessable(
            "invalid_hash",
            "blob hash must be 64 lowercase hex characters",
        ));
    }
    let hash = blobs::from_hex(&hash_hex).expect("validated hex");
    let size = blob_size(&state.pool, workspace.id, &hash)
        .await?
        .ok_or_else(|| ApiError::not_found("blob_not_found", "no such blob in this workspace"))?;
    stream_blob(&state, workspace.id, &hash, size).await
}

/// Stream a blob's bytes as `application/octet-stream` with a `content-length`.
async fn stream_blob(
    state: &AppState,
    workspace_id: Uuid,
    hash: &[u8],
    size: i64,
) -> ApiResult<Response> {
    let hash_hex = blobs::to_hex(hash);
    let key = blobs::blob_key(workspace_id, &hash_hex);
    let result = state.store.get(&key).await.map_err(|e| match e {
        object_store::Error::NotFound { .. } => {
            ApiError::not_found("blob_not_found", "blob bytes are missing from the store")
        }
        other => ApiError::internal(other),
    })?;
    let body = Body::from_stream(result.into_stream());
    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(len) = header::HeaderValue::from_str(&size.to_string()) {
        response.headers_mut().insert(header::CONTENT_LENGTH, len);
    }
    Ok(response)
}

// ---------------------------------------------------------------------------
// commit a revision
// ---------------------------------------------------------------------------

pub async fn create_revision(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, game_slug)): Path<(String, String)>,
    Json(req): Json<CreateRevisionRequest>,
) -> ApiResult<Response> {
    let workspace = authorize_write(&state, &user, &slug).await?;
    validate_game_slug(&game_slug)?;
    validate_manifest(&req.files)?;

    let pairs = distinct_pairs(&req.files);
    let mut tx = state.pool.begin().await?;

    // 1. Every referenced blob must already exist in this workspace.
    let wanted: Vec<Vec<u8>> = pairs.iter().map(|(_, b)| b.clone()).collect();
    let existing: Vec<Vec<u8>> =
        sqlx::query_scalar("SELECT hash FROM blobs WHERE workspace_id = $1 AND hash = ANY($2)")
            .bind(workspace.id)
            .bind(&wanted)
            .fetch_all(&mut *tx)
            .await?;
    let existing: HashSet<Vec<u8>> = existing.into_iter().collect();
    let missing: Vec<String> = pairs
        .iter()
        .filter(|(_, bytes)| !existing.contains(bytes))
        .map(|(hex, _)| hex.clone())
        .collect();
    if !missing.is_empty() {
        // Dropping `tx` rolls back. Custom shape: the error envelope + `missing`.
        let body = MissingBlobsResponse {
            error: ErrorBody {
                code: "missing_blobs".to_string(),
                message: format!(
                    "{} blob(s) must be uploaded before this revision can commit",
                    missing.len()
                ),
            },
            missing,
        };
        return Ok((StatusCode::CONFLICT, Json(body)).into_response());
    }

    // 2. Upsert the game (name defaults to the slug), then lock its row so
    //    concurrent pushes to the same game serialize on the numbering.
    sqlx::query(
        "INSERT INTO games (workspace_id, slug, name) VALUES ($1, $2, $2) \
         ON CONFLICT (workspace_id, slug) DO NOTHING",
    )
    .bind(workspace.id)
    .bind(&game_slug)
    .execute(&mut *tx)
    .await?;
    let game_id: Uuid =
        sqlx::query_scalar("SELECT id FROM games WHERE workspace_id = $1 AND slug = $2 FOR UPDATE")
            .bind(workspace.id)
            .bind(&game_slug)
            .fetch_one(&mut *tx)
            .await?;

    // 3. Optimistic concurrency + revision numbering.
    let head: i32 =
        sqlx::query_scalar("SELECT COALESCE(MAX(number), 0) FROM revisions WHERE game_id = $1")
            .bind(game_id)
            .fetch_one(&mut *tx)
            .await?;
    if let Some(parent) = req.parent_number
        && parent != head
    {
        return Err(ApiError::conflict(
            "stale_parent",
            format!("parent_number {parent} is not the current head {head}"),
        ));
    }
    let number = head + 1;

    // 4. Insert the revision and its files atomically.
    let revision_id: Uuid = sqlx::query_scalar(
        "INSERT INTO revisions (game_id, number, message, created_by) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(game_id)
    .bind(number)
    .bind(&req.message)
    .bind(user.user_id)
    .fetch_one(&mut *tx)
    .await?;

    let paths: Vec<String> = req.files.iter().map(|f| f.path.clone()).collect();
    let hashes: Vec<Vec<u8>> = req
        .files
        .iter()
        .map(|f| blobs::from_hex(&f.hash).expect("validated hex"))
        .collect();
    let sizes: Vec<i64> = req.files.iter().map(|f| f.size).collect();
    sqlx::query(
        "INSERT INTO revision_files (revision_id, path, hash, size) \
         SELECT $1, p, h, s FROM UNNEST($2::text[], $3::bytea[], $4::bigint[]) AS t(p, h, s)",
    )
    .bind(revision_id)
    .bind(&paths)
    .bind(&hashes)
    .bind(&sizes)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    // M3 hook: nudge the workspace's SSE subscribers that a revision landed.
    state.events.publish(
        workspace.id,
        crate::documents::WorkspaceEvent::RevisionPushed(protocol::RevisionPushedEvent {
            game: game_slug.clone(),
            number,
        }),
    );

    // 5. Compute stats out of band; tests call the same fn deterministically.
    tokio::spawn(stats::compute_stats_for_revision(
        state.pool.clone(),
        state.store.clone(),
        revision_id,
    ));

    let detail = load_revision_detail(&state.pool, revision_id).await?;
    Ok((StatusCode::CREATED, Json(detail)).into_response())
}

// ---------------------------------------------------------------------------
// listings & detail
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct GameRow {
    id: Uuid,
    slug: String,
    name: String,
    created_at: DateTime<Utc>,
    head_number: Option<i32>,
    revisions_count: i64,
}

pub async fn list_games(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
) -> ApiResult<Json<GamesResponse>> {
    let workspace = authorize_read(&state, &user, &slug).await?;
    let rows = sqlx::query_as::<_, GameRow>(
        "SELECT g.id, g.slug, g.name, g.created_at, \
                MAX(r.number) AS head_number, \
                COUNT(r.id) AS revisions_count \
         FROM games g LEFT JOIN revisions r ON r.game_id = g.id \
         WHERE g.workspace_id = $1 \
         GROUP BY g.id \
         ORDER BY g.created_at",
    )
    .bind(workspace.id)
    .fetch_all(&state.pool)
    .await?;
    let games = rows
        .into_iter()
        .map(|r| GameSummary {
            id: r.id,
            slug: r.slug,
            name: r.name,
            head_number: r.head_number,
            revisions_count: r.revisions_count,
            created_at: r.created_at,
        })
        .collect();
    Ok(Json(GamesResponse { games }))
}

async fn game_id_by_slug(pool: &PgPool, workspace_id: Uuid, slug: &str) -> ApiResult<Uuid> {
    sqlx::query_scalar("SELECT id FROM games WHERE workspace_id = $1 AND slug = $2")
        .bind(workspace_id)
        .bind(slug)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::not_found("game_not_found", "no such game"))
}

async fn revision_id_by_number(pool: &PgPool, game_id: Uuid, number: i32) -> ApiResult<Uuid> {
    sqlx::query_scalar("SELECT id FROM revisions WHERE game_id = $1 AND number = $2")
        .bind(game_id)
        .bind(number)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::not_found("revision_not_found", "no such revision"))
}

#[derive(sqlx::FromRow)]
struct RevisionSummaryRow {
    number: i32,
    message: String,
    author_display_name: Option<String>,
    created_at: DateTime<Utc>,
    files_count: i64,
    total_size: i64,
    stats_status: Option<String>,
}

pub async fn list_revisions(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, game_slug)): Path<(String, String)>,
) -> ApiResult<Json<RevisionsResponse>> {
    let workspace = authorize_read(&state, &user, &slug).await?;
    let game_id = game_id_by_slug(&state.pool, workspace.id, &game_slug).await?;
    let rows = sqlx::query_as::<_, RevisionSummaryRow>(
        "SELECT r.number, r.message, u.display_name AS author_display_name, r.created_at, \
                COUNT(rf.revision_id) AS files_count, \
                COALESCE(SUM(rf.size), 0)::bigint AS total_size, \
                rs.status AS stats_status \
         FROM revisions r \
         LEFT JOIN users u ON u.id = r.created_by \
         LEFT JOIN revision_files rf ON rf.revision_id = r.id \
         LEFT JOIN revision_stats rs ON rs.revision_id = r.id \
         WHERE r.game_id = $1 \
         GROUP BY r.id, u.display_name, rs.status \
         ORDER BY r.number DESC",
    )
    .bind(game_id)
    .fetch_all(&state.pool)
    .await?;
    let revisions = rows
        .into_iter()
        .map(|r| {
            Ok(RevisionSummary {
                number: r.number,
                message: r.message,
                author_display_name: r.author_display_name,
                created_at: r.created_at,
                files_count: r.files_count,
                total_size: r.total_size,
                stats_status: r
                    .stats_status
                    .as_deref()
                    .map(parse_stats_status)
                    .transpose()?,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(RevisionsResponse { revisions }))
}

pub async fn revision_detail(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, game_slug, number)): Path<(String, String, i32)>,
) -> ApiResult<Json<RevisionDetail>> {
    let workspace = authorize_read(&state, &user, &slug).await?;
    let game_id = game_id_by_slug(&state.pool, workspace.id, &game_slug).await?;
    let revision_id = revision_id_by_number(&state.pool, game_id, number).await?;
    Ok(Json(load_revision_detail(&state.pool, revision_id).await?))
}

#[derive(sqlx::FromRow)]
struct RevisionMetaRow {
    number: i32,
    message: String,
    author_display_name: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct FileRow {
    path: String,
    hash: Vec<u8>,
    size: i64,
}

#[derive(sqlx::FromRow)]
struct StatsRow {
    status: String,
    error: Option<String>,
    data: Option<serde_json::Value>,
    updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct StatsData {
    modes: Vec<ModeStats>,
}

fn parse_stats_status(value: &str) -> ApiResult<StatsStatus> {
    match value {
        "pending" => Ok(StatsStatus::Pending),
        "ok" => Ok(StatsStatus::Ok),
        "error" => Ok(StatsStatus::Error),
        other => Err(ApiError::internal(format!(
            "unexpected stats status in db: {other}"
        ))),
    }
}

async fn load_revision_files(pool: &PgPool, revision_id: Uuid) -> ApiResult<Vec<FileEntry>> {
    let rows = sqlx::query_as::<_, FileRow>(
        "SELECT path, hash, size FROM revision_files WHERE revision_id = $1 ORDER BY path",
    )
    .bind(revision_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| FileEntry {
            path: r.path,
            hash: blobs::to_hex(&r.hash),
            size: r.size,
        })
        .collect())
}

async fn load_revision_stats(pool: &PgPool, revision_id: Uuid) -> ApiResult<Option<RevisionStats>> {
    let row = sqlx::query_as::<_, StatsRow>(
        "SELECT status, error, data, updated_at FROM revision_stats WHERE revision_id = $1",
    )
    .bind(revision_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(RevisionStats {
        status: parse_stats_status(&row.status)?,
        error: row.error,
        modes: modes_from_data(row.data),
        updated_at: row.updated_at,
    }))
}

fn modes_from_data(data: Option<serde_json::Value>) -> Vec<ModeStats> {
    data.and_then(|d| serde_json::from_value::<StatsData>(d).ok())
        .map(|d| d.modes)
        .unwrap_or_default()
}

async fn load_revision_detail(pool: &PgPool, revision_id: Uuid) -> ApiResult<RevisionDetail> {
    let meta = sqlx::query_as::<_, RevisionMetaRow>(
        "SELECT r.number, r.message, u.display_name AS author_display_name, r.created_at \
         FROM revisions r LEFT JOIN users u ON u.id = r.created_by WHERE r.id = $1",
    )
    .bind(revision_id)
    .fetch_one(pool)
    .await?;
    Ok(RevisionDetail {
        number: meta.number,
        message: meta.message,
        author_display_name: meta.author_display_name,
        created_at: meta.created_at,
        files: load_revision_files(pool, revision_id).await?,
        stats: load_revision_stats(pool, revision_id).await?,
    })
}

// ---------------------------------------------------------------------------
// diff
// ---------------------------------------------------------------------------

pub async fn revision_diff(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, game_slug, number, other)): Path<(String, String, i32, i32)>,
) -> ApiResult<Json<RevisionDiff>> {
    let workspace = authorize_read(&state, &user, &slug).await?;
    let game_id = game_id_by_slug(&state.pool, workspace.id, &game_slug).await?;
    // "after" = :number, "before" = :other.
    let after_id = revision_id_by_number(&state.pool, game_id, number).await?;
    let before_id = revision_id_by_number(&state.pool, game_id, other).await?;

    let after_files = load_revision_files(&state.pool, after_id).await?;
    let before_files = load_revision_files(&state.pool, before_id).await?;
    let files = diff_files(&before_files, &after_files);

    let before_stats = ok_mode_stats(&state.pool, before_id).await?;
    let after_stats = ok_mode_stats(&state.pool, after_id).await?;
    let stats = diff_stats(before_stats, after_stats);

    Ok(Json(RevisionDiff { files, stats }))
}

fn diff_files(before: &[FileEntry], after: &[FileEntry]) -> FileDiff {
    let before_map: HashMap<&str, &FileEntry> =
        before.iter().map(|f| (f.path.as_str(), f)).collect();
    let after_map: HashMap<&str, &FileEntry> = after.iter().map(|f| (f.path.as_str(), f)).collect();

    let mut added = Vec::new();
    let mut changed = Vec::new();
    let mut unchanged: u32 = 0;
    for f in after {
        match before_map.get(f.path.as_str()) {
            None => added.push(f.clone()),
            Some(b) if b.hash != f.hash => changed.push(ChangedFile {
                path: f.path.clone(),
                before_hash: b.hash.clone(),
                after_hash: f.hash.clone(),
                before_size: b.size,
                after_size: f.size,
            }),
            Some(_) => unchanged += 1,
        }
    }
    let removed = before
        .iter()
        .filter(|f| !after_map.contains_key(f.path.as_str()))
        .cloned()
        .collect();
    FileDiff {
        added,
        removed,
        changed,
        unchanged,
    }
}

/// A revision's per-mode stats, but only when the stats are `ok`.
async fn ok_mode_stats(pool: &PgPool, revision_id: Uuid) -> ApiResult<Option<Vec<ModeStats>>> {
    let row = sqlx::query_as::<_, StatsRow>(
        "SELECT status, error, data, updated_at FROM revision_stats WHERE revision_id = $1",
    )
    .bind(revision_id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some(row) if row.status == "ok" => Ok(Some(modes_from_data(row.data))),
        _ => Ok(None),
    }
}

fn diff_stats(before: Option<Vec<ModeStats>>, after: Option<Vec<ModeStats>>) -> StatsDiff {
    // Ordered union of mode names: after's order first, then before-only modes.
    let mut order: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for set in [&after, &before].into_iter().flatten() {
        for m in set {
            if seen.insert(m.mode.clone()) {
                order.push(m.mode.clone());
            }
        }
    }
    let find = |set: &Option<Vec<ModeStats>>, name: &str| -> Option<ModeStats> {
        set.as_ref()
            .and_then(|v| v.iter().find(|m| m.mode == name).cloned())
    };
    let modes = order
        .into_iter()
        .map(|name| {
            let before = find(&before, &name);
            let after = find(&after, &name);
            ModeStatsDiff {
                mode: name,
                before,
                after,
            }
        })
        .collect();
    StatsDiff { modes }
}

// ---------------------------------------------------------------------------
// file download (pull)
// ---------------------------------------------------------------------------

pub async fn download_file(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, game_slug, number, path)): Path<(String, String, i32, String)>,
) -> ApiResult<Response> {
    let workspace = authorize_read(&state, &user, &slug).await?;
    let game_id = game_id_by_slug(&state.pool, workspace.id, &game_slug).await?;
    let revision_id = revision_id_by_number(&state.pool, game_id, number).await?;
    let row: Option<(Vec<u8>, i64)> = sqlx::query_as(
        "SELECT hash, size FROM revision_files WHERE revision_id = $1 AND path = $2",
    )
    .bind(revision_id)
    .bind(&path)
    .fetch_optional(&state.pool)
    .await?;
    let (hash, size) =
        row.ok_or_else(|| ApiError::not_found("file_not_found", "no such file in this revision"))?;
    stream_blob(&state, workspace.id, &hash, size).await
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, hash: &str, size: i64) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            hash: hash.to_string(),
            size,
        }
    }

    fn h(byte: u8) -> String {
        format!("{byte:02x}").repeat(32) // 64 lowercase hex chars
    }

    #[test]
    fn accepts_a_valid_manifest() {
        let files = vec![
            file("index.json", &h(1), 10),
            file("lookup/base.csv", &h(2), 20),
            file("books_base.jsonl", &h(3), 30),
        ];
        assert!(validate_manifest(&files).is_ok());
    }

    #[test]
    fn rejects_empty_and_oversized_manifests() {
        assert!(validate_manifest(&[]).is_err());
        let many: Vec<FileEntry> = (0..1001)
            .map(|i| file(&format!("f{i}"), &h(1), 1))
            .collect();
        assert!(validate_manifest(&many).is_err());
    }

    #[test]
    fn requires_exactly_one_root_index_json() {
        // none
        assert!(validate_manifest(&[file("data.csv", &h(1), 1)]).is_err());
        // nested index.json does not count as root
        assert!(validate_manifest(&[file("sub/index.json", &h(1), 1)]).is_err());
        // two roots
        let dup = vec![file("index.json", &h(1), 1), file("index.json", &h(2), 1)];
        // (this trips the duplicate-path rule first, still an error)
        assert!(validate_manifest(&dup).is_err());
    }

    #[test]
    fn rejects_bad_paths() {
        let bad = ["/abs.json", "a\\b.csv", "../escape.csv", "dir/../x", ""];
        for p in bad {
            let files = vec![file("index.json", &h(1), 1), file(p, &h(2), 1)];
            assert!(validate_manifest(&files).is_err(), "{p} should be rejected");
        }
        // control char
        let files = vec![file("index.json", &h(1), 1), file("a\nb.csv", &h(2), 1)];
        assert!(validate_manifest(&files).is_err());
        // too long
        let long = "a".repeat(513);
        let files = vec![file("index.json", &h(1), 1), file(&long, &h(2), 1)];
        assert!(validate_manifest(&files).is_err());
    }

    #[test]
    fn rejects_duplicate_paths() {
        let files = vec![
            file("index.json", &h(1), 1),
            file("dup.csv", &h(2), 1),
            file("dup.csv", &h(3), 1),
        ];
        assert!(validate_manifest(&files).is_err());
    }

    #[test]
    fn rejects_bad_hash_and_negative_size() {
        assert!(validate_manifest(&[file("index.json", "abc", 1)]).is_err());
        assert!(validate_manifest(&[file("index.json", &"A".repeat(64), 1)]).is_err());
        assert!(validate_manifest(&[file("index.json", &h(1), -1)]).is_err());
    }

    #[test]
    fn game_slug_rules() {
        for good in ["abc", "sweet-bonanza", "a1b2"] {
            assert!(validate_game_slug(good).is_ok(), "{good}");
        }
        for bad in ["ab", "-abc", "abc-", "AbC", "a_b", "a b"] {
            assert!(validate_game_slug(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn diff_files_classifies_correctly() {
        let before = vec![
            file("index.json", &h(1), 1),
            file("a.csv", &h(2), 2),
            file("gone.csv", &h(9), 9),
        ];
        let after = vec![
            file("index.json", &h(1), 1), // unchanged
            file("a.csv", &h(5), 3),      // changed
            file("new.csv", &h(7), 7),    // added
        ];
        let d = diff_files(&before, &after);
        assert_eq!(d.unchanged, 1);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.added[0].path, "new.csv");
        assert_eq!(d.removed.len(), 1);
        assert_eq!(d.removed[0].path, "gone.csv");
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.changed[0].path, "a.csv");
        assert_eq!(d.changed[0].before_hash, h(2));
        assert_eq!(d.changed[0].after_hash, h(5));
    }
}

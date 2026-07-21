//! M3 — workspace document sync (profiles, saved rounds) + the workspace SSE
//! stream, plus owner-only delete-workspace. Contract: docs/v2/m3-contract.md.
//!
//! Concurrency is optimistic: a `PUT`/`DELETE` carries `base_revision` and only
//! applies when it equals the document's current `revision`; a mismatch is a
//! `409 document_conflict` carrying the current server envelope so the client
//! can reconcile (keep-mine retries with the fresh revision). Every write bumps
//! the per-document `revision` and advances the workspace-global `seq` cursor,
//! then nudges SSE subscribers with a `document` event.
//!
//! Reads require workspace membership; writes additionally require the `full`
//! scope, so a PAT holding only `push:math` can pull documents but not edit them
//! (checked after membership so a non-member still 404s rather than leaking the
//! workspace's existence via a 403). The envelope/request types that wrap the
//! free-form `data` blob live here (they need `serde_json::Value`); the durable
//! payload + event schemas are in `protocol::documents`.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use futures_util::{Stream, StreamExt};
use object_store::path::Path as StorePath;
use object_store::{ObjectStore, ObjectStoreExt};
use protocol::{
    DeleteDocumentRequest, DeleteDocumentResponse, DocumentEvent, DocumentKind, ErrorBody,
    ProfileDocument, PutDocumentResponse, Role, SavedRoundDocument,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::AppState;
use crate::api::workspaces::{WorkspaceRow, require_membership, workspace_by_slug};
use crate::auth::extract::CurrentUser;
use crate::documents::WorkspaceEvent;
use crate::error::{ApiError, ApiResult};

/// Largest accepted `data` payload (64 KiB), measured as the serialized JSON.
const MAX_DOCUMENT_BYTES: usize = 64 * 1024;

/// The columns every envelope query selects, aliasing the author's display name.
const ENVELOPE_COLUMNS: &str = "d.kind, d.doc_id, d.data, d.revision, d.seq, \
     u.display_name AS updated_by_display, d.updated_at, d.deleted_at";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/workspaces/:slug/documents", get(list_documents))
        .route(
            "/workspaces/:slug/documents/:kind/:doc_id",
            get(get_document).put(put_document).delete(delete_document),
        )
        .route("/workspaces/:slug/events", get(events))
        // Merges with the existing `GET /workspaces/:slug` (owned by
        // `api::workspaces`) — different methods on one path combine cleanly.
        .route("/workspaces/:slug", delete(delete_workspace))
}

// ---------------------------------------------------------------------------
// wire types that wrap the free-form `data` (server-local: need serde_json)
// ---------------------------------------------------------------------------

/// A document as returned by the read endpoints. `deleted` is derived from the
/// tombstone column; `data` is the stored payload verbatim (unknown fields the
/// client wrote are preserved).
#[derive(Debug, Clone, Serialize)]
pub struct DocumentEnvelope {
    pub kind: DocumentKind,
    pub doc_id: String,
    pub data: serde_json::Value,
    pub revision: i32,
    pub seq: i64,
    pub updated_by_display: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub deleted: bool,
}

/// `GET /workspaces/:slug/documents` response. `latest_seq` is the workspace's
/// current max `seq` (respecting the `kind` filter) — the cursor the client
/// stores and replays as `?since_seq=` next time.
#[derive(Debug, Clone, Serialize)]
pub struct DocumentsResponse {
    pub documents: Vec<DocumentEnvelope>,
    pub latest_seq: i64,
}

/// `PUT /workspaces/:slug/documents/:kind/:doc_id` body. `base_revision` is the
/// revision the client last saw (`null` = create).
#[derive(Debug, Clone, Deserialize)]
pub struct PutDocumentRequest {
    pub data: serde_json::Value,
    #[serde(default)]
    pub base_revision: Option<i32>,
}

/// The `409 document_conflict` body: the standard error envelope plus the
/// current server document the client must reconcile against.
#[derive(Debug, Clone, Serialize)]
pub struct DocumentConflictResponse {
    pub error: ErrorBody,
    pub current: DocumentEnvelope,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub kind: Option<String>,
    pub since_seq: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct DocumentRow {
    kind: String,
    doc_id: String,
    data: serde_json::Value,
    revision: i32,
    seq: i64,
    updated_by_display: Option<String>,
    updated_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

impl DocumentRow {
    fn into_envelope(self) -> ApiResult<DocumentEnvelope> {
        let kind = DocumentKind::from_db(&self.kind).ok_or_else(|| {
            ApiError::internal(format!("unexpected document kind in db: {}", self.kind))
        })?;
        Ok(DocumentEnvelope {
            kind,
            doc_id: self.doc_id,
            data: self.data,
            revision: self.revision,
            seq: self.seq,
            updated_by_display: self.updated_by_display,
            updated_at: self.updated_at,
            deleted: self.deleted_at.is_some(),
        })
    }
}

// ---------------------------------------------------------------------------
// authorization + validation helpers
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

/// Resolve the workspace, require membership, then require the `full` scope.
/// Scope is checked last so a non-member 404s rather than leaking via a 403.
async fn authorize_write(
    state: &AppState,
    user: &CurrentUser,
    slug: &str,
) -> ApiResult<WorkspaceRow> {
    let workspace = authorize_read(state, user, slug).await?;
    user.require_scope("full")?;
    Ok(workspace)
}

/// Parse a `:kind` path / `?kind=` value, or `422 invalid_kind`.
fn parse_kind(value: &str) -> ApiResult<DocumentKind> {
    DocumentKind::from_db(value).ok_or_else(|| {
        ApiError::unprocessable("invalid_kind", format!("unknown document kind \"{value}\""))
    })
}

/// Enforce the 64 KiB `data` ceiling before any heavier work.
fn enforce_size(data: &serde_json::Value) -> ApiResult<()> {
    let len = serde_json::to_vec(data)
        .map(|v| v.len())
        .unwrap_or(usize::MAX);
    if len > MAX_DOCUMENT_BYTES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            format!("document data exceeds the {MAX_DOCUMENT_BYTES}-byte limit"),
        ));
    }
    Ok(())
}

/// Validate a payload against its kind's schema: required fields enforced,
/// unknown fields tolerated (and preserved when stored). Deserializing by
/// reference avoids cloning the value.
fn validate_payload(kind: DocumentKind, data: &serde_json::Value) -> ApiResult<()> {
    let result = match kind {
        DocumentKind::Profile => ProfileDocument::deserialize(data).map(|_| ()),
        DocumentKind::SavedRound => SavedRoundDocument::deserialize(data).map(|_| ()),
    };
    result.map_err(|e| {
        ApiError::unprocessable(
            "invalid_document",
            format!("invalid {} payload: {e}", kind.as_str()),
        )
    })
}

/// Build the `409 document_conflict` response around the current envelope.
fn conflict_response(current: DocumentEnvelope) -> Response {
    (
        StatusCode::CONFLICT,
        Json(DocumentConflictResponse {
            error: ErrorBody {
                code: "document_conflict".to_string(),
                message: "the document changed since base_revision; reconcile against `current`"
                    .to_string(),
            },
            current,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// read endpoints
// ---------------------------------------------------------------------------

/// List documents. With `since_seq` this is a sync pull: only rows newer than
/// the cursor, tombstones included so deletions propagate. Without it, a plain
/// listing: live documents only. `latest_seq` is always the current max cursor.
pub async fn list_documents(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<DocumentsResponse>> {
    let workspace = authorize_read(&state, &user, &slug).await?;
    let kind_filter = match &query.kind {
        Some(k) => Some(parse_kind(k)?.as_str()),
        None => None,
    };
    let include_tombstones = query.since_seq.is_some();

    let rows = sqlx::query_as::<_, DocumentRow>(&format!(
        "SELECT {ENVELOPE_COLUMNS} FROM documents d \
         LEFT JOIN users u ON u.id = d.updated_by \
         WHERE d.workspace_id = $1 \
           AND ($2::text IS NULL OR d.kind = $2) \
           AND ($3::bigint IS NULL OR d.seq > $3) \
           AND ($4::bool OR d.deleted_at IS NULL) \
         ORDER BY d.seq"
    ))
    .bind(workspace.id)
    .bind(kind_filter)
    .bind(query.since_seq)
    .bind(include_tombstones)
    .fetch_all(&state.pool)
    .await?;

    let documents = rows
        .into_iter()
        .map(DocumentRow::into_envelope)
        .collect::<ApiResult<Vec<_>>>()?;

    let latest_seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(seq), 0) FROM documents \
         WHERE workspace_id = $1 AND ($2::text IS NULL OR kind = $2)",
    )
    .bind(workspace.id)
    .bind(kind_filter)
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(DocumentsResponse {
        documents,
        latest_seq,
    }))
}

/// Fetch one live document. Tombstoned or missing → 404.
pub async fn get_document(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, kind, doc_id)): Path<(String, String, String)>,
) -> ApiResult<Json<DocumentEnvelope>> {
    let workspace = authorize_read(&state, &user, &slug).await?;
    let kind = parse_kind(&kind)?;
    let row = sqlx::query_as::<_, DocumentRow>(&format!(
        "SELECT {ENVELOPE_COLUMNS} FROM documents d \
         LEFT JOIN users u ON u.id = d.updated_by \
         WHERE d.workspace_id = $1 AND d.kind = $2 AND d.doc_id = $3 AND d.deleted_at IS NULL"
    ))
    .bind(workspace.id)
    .bind(kind.as_str())
    .bind(&doc_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("document_not_found", "no such document"))?;
    Ok(Json(row.into_envelope()?))
}

// ---------------------------------------------------------------------------
// write endpoints
// ---------------------------------------------------------------------------

/// Create or update a document under optimistic concurrency. On a create the row
/// starts at `revision = 1`; on an update `base_revision` must equal the current
/// revision or the write is rejected `409` with the current envelope.
pub async fn put_document(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, kind, doc_id)): Path<(String, String, String)>,
    Json(req): Json<PutDocumentRequest>,
) -> ApiResult<Response> {
    let workspace = authorize_write(&state, &user, &slug).await?;
    let kind = parse_kind(&kind)?;
    enforce_size(&req.data)?;
    validate_payload(kind, &req.data)?;

    let mut tx = state.pool.begin().await?;
    let existing = current_row(&mut tx, workspace.id, kind, &doc_id).await?;

    let (revision, seq) = match existing {
        // No row yet → create. `base_revision` is treated as `null` (create).
        None => {
            sqlx::query_as::<_, (i32, i64)>(
                "INSERT INTO documents (workspace_id, kind, doc_id, data, updated_by) \
             VALUES ($1, $2, $3, $4, $5) RETURNING revision, seq",
            )
            .bind(workspace.id)
            .bind(kind.as_str())
            .bind(&doc_id)
            .bind(&req.data)
            .bind(user.user_id)
            .fetch_one(&mut *tx)
            .await?
        }
        // Row exists → the base_revision must match the live revision.
        Some(current) => {
            if req.base_revision != Some(current.revision) {
                return Ok(conflict_response(current.into_envelope()?));
            }
            sqlx::query_as::<_, (i32, i64)>(
                "UPDATE documents \
                 SET data = $4, revision = revision + 1, \
                     seq = nextval(pg_get_serial_sequence('documents', 'seq')), \
                     updated_by = $5, updated_at = now(), deleted_at = NULL \
                 WHERE workspace_id = $1 AND kind = $2 AND doc_id = $3 \
                 RETURNING revision, seq",
            )
            .bind(workspace.id)
            .bind(kind.as_str())
            .bind(&doc_id)
            .bind(&req.data)
            .bind(user.user_id)
            .fetch_one(&mut *tx)
            .await?
        }
    };
    tx.commit().await?;

    state.events.publish(
        workspace.id,
        WorkspaceEvent::Document(DocumentEvent { kind, doc_id, seq }),
    );
    Ok((StatusCode::OK, Json(PutDocumentResponse { revision, seq })).into_response())
}

/// Tombstone a document. Missing → 404; stale `base_revision` → 409 with the
/// current envelope. The row is kept (tombstone) so sync pulls see the deletion.
pub async fn delete_document(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((slug, kind, doc_id)): Path<(String, String, String)>,
    Json(req): Json<DeleteDocumentRequest>,
) -> ApiResult<Response> {
    let workspace = authorize_write(&state, &user, &slug).await?;
    let kind = parse_kind(&kind)?;

    let mut tx = state.pool.begin().await?;
    let Some(current) = current_row(&mut tx, workspace.id, kind, &doc_id).await? else {
        return Err(ApiError::not_found(
            "document_not_found",
            "no such document",
        ));
    };
    if let Some(base) = req.base_revision
        && base != current.revision
    {
        return Ok(conflict_response(current.into_envelope()?));
    }

    let seq: i64 = sqlx::query_scalar(
        "UPDATE documents \
         SET revision = revision + 1, \
             seq = nextval(pg_get_serial_sequence('documents', 'seq')), \
             updated_by = $4, updated_at = now(), deleted_at = now() \
         WHERE workspace_id = $1 AND kind = $2 AND doc_id = $3 \
         RETURNING seq",
    )
    .bind(workspace.id)
    .bind(kind.as_str())
    .bind(&doc_id)
    .bind(user.user_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    state.events.publish(
        workspace.id,
        WorkspaceEvent::Document(DocumentEvent { kind, doc_id, seq }),
    );
    Ok((StatusCode::OK, Json(DeleteDocumentResponse { seq })).into_response())
}

/// Lock and load the current row (if any) for an optimistic write. `FOR UPDATE
/// OF d` locks only the document row — the outer-joined `users` side can't be
/// locked.
async fn current_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    kind: DocumentKind,
    doc_id: &str,
) -> ApiResult<Option<DocumentRow>> {
    Ok(sqlx::query_as::<_, DocumentRow>(&format!(
        "SELECT {ENVELOPE_COLUMNS} FROM documents d \
         LEFT JOIN users u ON u.id = d.updated_by \
         WHERE d.workspace_id = $1 AND d.kind = $2 AND d.doc_id = $3 \
         FOR UPDATE OF d"
    ))
    .bind(workspace_id)
    .bind(kind.as_str())
    .bind(doc_id)
    .fetch_optional(&mut **tx)
    .await?)
}

// ---------------------------------------------------------------------------
// workspace SSE stream
// ---------------------------------------------------------------------------

/// The workspace realtime stream. Membership required (cookie or Bearer). On
/// (re)connect the client is expected to pull `?since_seq=` first, then stream —
/// there is no Last-Event-ID replay. A `Lagged` receiver skips the gap; the
/// stream ends only when the workspace channel closes.
pub async fn events(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let workspace = authorize_read(&state, &user, &slug).await?;
    let rx = state.events.subscribe(workspace.id);

    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(event) => return Some((Ok(event.to_sse_event()), rx)),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(25))
            .text("keep-alive"),
    ))
}

// ---------------------------------------------------------------------------
// delete workspace (owner only) + best-effort blob cleanup
// ---------------------------------------------------------------------------

/// Delete a workspace and everything under it. Owner only. The database rows
/// cascade synchronously; the workspace's blob bytes are swept from the object
/// store by a spawned best-effort task so a slow/unreachable store never blocks
/// (or fails) the response.
pub async fn delete_workspace(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
) -> ApiResult<StatusCode> {
    let workspace = workspace_by_slug(&state.pool, &slug).await?;
    let role = require_membership(&state.pool, workspace.id, user.user_id).await?;
    if role != Role::Owner {
        return Err(ApiError::forbidden(
            "forbidden",
            "only the workspace owner can delete it",
        ));
    }

    sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(workspace.id)
        .execute(&state.pool)
        .await?;

    let store = state.store.clone();
    let workspace_id = workspace.id;
    tokio::spawn(async move { cleanup_blobs(store, workspace_id).await });

    Ok(StatusCode::NO_CONTENT)
}

/// Delete every object under `blobs/<workspace_id>/`. Failures are logged, never
/// surfaced — the DB rows are already gone, so orphaned bytes are harmless.
async fn cleanup_blobs(store: Arc<dyn ObjectStore>, workspace_id: Uuid) {
    let prefix = StorePath::from(format!("blobs/{workspace_id}"));
    let mut listing = store.list(Some(&prefix));
    let mut failures: u64 = 0;
    while let Some(entry) = listing.next().await {
        match entry {
            Ok(meta) => {
                if let Err(e) = store.delete(&meta.location).await {
                    failures += 1;
                    tracing::warn!(error = %e, key = %meta.location, "workspace cleanup: delete failed");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, %workspace_id, "workspace cleanup: list failed");
                return;
            }
        }
    }
    if failures > 0 {
        tracing::warn!(%workspace_id, failures, "workspace blob cleanup finished with failures");
    }
}

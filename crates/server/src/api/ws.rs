//! M4/M6 — cloud-hosted multi-tenant LGS mount and the workbench front serving.
//!
//! The wildcard route forwards an authenticated, membership-checked request into
//! the per-tenant LGS router for a pinned `(workspace, game, revision)`; the
//! resolution, materialization, tenancy and URI rewrite live in
//! [`crate::lgs_host`]. The `front` routes serve the game's latest uploaded
//! front bundle on the app origin (membership-gated), so the workbench can run
//! without any localhost dev server: the test view iframes
//! `/api/ws/:slug/g/:game/front/`. This router is merged into the `/api`
//! router, so paths below are effectively `/api/ws/…`.

use std::collections::HashMap;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;
use crate::api::workspaces::{require_membership, workspace_by_slug};
use crate::auth::extract::CurrentUser;
use crate::blobs;
use crate::error::{ApiError, ApiResult};

pub fn router() -> Router<AppState> {
    Router::new()
        // `any` so every method (GET devtool/replay, POST wallet, …) dispatches.
        .route(
            "/ws/:slug/g/:game/r/:number/*rest",
            any(crate::lgs_host::dispatch),
        )
        // Workbench front bundle: latest bundle, served same-origin so the test
        // view needs no external front URL. Fronts must be built with a
        // relative base (like the V1 GitHub-Pages previews) to load assets
        // under this prefix.
        .route("/ws/:slug/g/:game/front", get(front_index))
        .route("/ws/:slug/g/:game/front/", get(front_index))
        .route("/ws/:slug/g/:game/front/*path", get(front_path))
        // Pinned bundle: same membership-gated streaming as the latest handler
        // above, but for an exact bundle id (the test view's version picker).
        .route(
            "/ws/:slug/g/:game/fronts/:bundle_id",
            get(pinned_front_index),
        )
        .route(
            "/ws/:slug/g/:game/fronts/:bundle_id/",
            get(pinned_front_index),
        )
        .route(
            "/ws/:slug/g/:game/fronts/:bundle_id/*path",
            get(pinned_front_path),
        )
}

#[derive(Deserialize)]
struct ManifestEntry {
    hash: String,
    size: i64,
}

async fn front_index(
    state: State<AppState>,
    user: CurrentUser,
    Path((slug, game)): Path<(String, String)>,
) -> ApiResult<Response> {
    serve_front(state, user, slug, game, String::new()).await
}

async fn front_path(
    state: State<AppState>,
    user: CurrentUser,
    Path((slug, game, path)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    serve_front(state, user, slug, game, path).await
}

async fn pinned_front_index(
    state: State<AppState>,
    user: CurrentUser,
    Path((slug, game, bundle_id)): Path<(String, String, Uuid)>,
) -> ApiResult<Response> {
    serve_pinned_front(state, user, slug, game, bundle_id, String::new()).await
}

async fn pinned_front_path(
    state: State<AppState>,
    user: CurrentUser,
    Path((slug, game, bundle_id, path)): Path<(String, String, Uuid, String)>,
) -> ApiResult<Response> {
    serve_pinned_front(state, user, slug, game, bundle_id, path).await
}

/// Membership-gated serving of the game's LATEST front bundle from the object
/// store: `''` → `index.html`, unknown non-asset paths fall back to
/// `index.html` (SPA routing), missing bundle → a JSON hint.
async fn serve_front(
    State(state): State<AppState>,
    user: CurrentUser,
    slug: String,
    game: String,
    path: String,
) -> ApiResult<Response> {
    let workspace = workspace_by_slug(&state.pool, &slug).await?;
    require_membership(&state.pool, workspace.id, user.user_id).await?;

    let manifest: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT fb.manifest FROM front_bundles fb \
         JOIN games g ON g.id = fb.game_id \
         WHERE g.workspace_id = $1 AND g.slug = $2 \
         ORDER BY fb.created_at DESC LIMIT 1",
    )
    .bind(workspace.id)
    .bind(&game)
    .fetch_optional(&state.pool)
    .await?;
    let Some(manifest) = manifest else {
        return Err(ApiError::not_found(
            "no_front_bundle",
            "no front bundle uploaded for this game yet — push one with \
             `sdt push-front` or from the game page",
        ));
    };
    serve_from_manifest(&state, workspace.id, manifest, &path).await
}

/// Membership-gated serving of an EXACT front bundle by id (byte-identical to
/// [`serve_front`] once the manifest is resolved). 404 for an unknown or foreign
/// bundle id, so a member of one workspace can never reach another's bundle.
async fn serve_pinned_front(
    State(state): State<AppState>,
    user: CurrentUser,
    slug: String,
    game: String,
    bundle_id: Uuid,
    path: String,
) -> ApiResult<Response> {
    let workspace = workspace_by_slug(&state.pool, &slug).await?;
    require_membership(&state.pool, workspace.id, user.user_id).await?;

    let manifest: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT fb.manifest FROM front_bundles fb \
         JOIN games g ON g.id = fb.game_id \
         WHERE g.workspace_id = $1 AND g.slug = $2 AND fb.id = $3",
    )
    .bind(workspace.id)
    .bind(&game)
    .bind(bundle_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(manifest) = manifest else {
        return Err(ApiError::not_found(
            "bundle_not_found",
            "no such front bundle for this game",
        ));
    };
    serve_from_manifest(&state, workspace.id, manifest, &path).await
}

/// Resolve a request path against a bundle manifest and stream the matching blob.
/// `''` → `index.html`; an unknown non-asset path falls back to `index.html`
/// (SPA routing); an unknown asset-looking path is a 404.
async fn serve_from_manifest(
    state: &AppState,
    workspace_id: Uuid,
    manifest: serde_json::Value,
    path: &str,
) -> ApiResult<Response> {
    let entries: HashMap<String, ManifestEntry> = serde_json::from_value(manifest)
        .map_err(|e| ApiError::internal(format!("malformed front bundle manifest: {e}")))?;

    let rel = path.trim_start_matches('/');
    let key = if rel.is_empty() { "index.html" } else { rel };
    let entry = entries.get(key).or_else(|| {
        // SPA fallback for client-routed paths; never for asset-looking ones.
        (!key.contains('.'))
            .then(|| entries.get("index.html"))
            .flatten()
    });
    let Some(entry) = entry else {
        return Err(ApiError::not_found(
            "not_found",
            "no such file in the bundle",
        ));
    };
    let served = if entries.contains_key(key) {
        key
    } else {
        "index.html"
    };
    stream_entry(state, workspace_id, served, entry).await
}

async fn stream_entry(
    state: &AppState,
    workspace_id: Uuid,
    path: &str,
    entry: &ManifestEntry,
) -> ApiResult<Response> {
    use object_store::ObjectStoreExt;

    let key = blobs::blob_key(workspace_id, &entry.hash);
    let result = state
        .store
        .get(&key)
        .await
        .map_err(|e| ApiError::internal(format!("front bundle blob read failed: {e}")))?;
    let stream = result.into_stream();

    // index.html must revalidate (bundle updates swap it); hashed assets are
    // immutable by construction.
    let cache = if path == "index.html" {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type(path)),
            (header::CACHE_CONTROL, cache),
            (header::CONTENT_LENGTH, &entry.size.to_string()),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}

/// Minimal extension → content-type map for game-front assets.
fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "zst" => "application/zstd",
        _ => "application/octet-stream",
    }
}

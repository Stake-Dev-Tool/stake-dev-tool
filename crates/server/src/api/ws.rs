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
//!
//! A second mount, `/api/wb/:token/*rest`, reaches the SAME tenant routers with
//! a capability token in the path instead of the session cookie. It exists for
//! the one case the cookie cannot serve: a game front running on the developer's
//! own dev server, whose calls are cross-site and therefore never carry a
//! `SameSite=Lax` cookie. It is the only route here with a CORS layer — see
//! [`workbench_mount`] and [`crate::lgs_host::workbench`].

use std::collections::HashMap;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderName, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use protocol::{WorkbenchToken, WorkbenchTokenRequest};
use serde::Deserialize;
use tower_http::cors::{AllowOrigin, CorsLayer};
use uuid::Uuid;

use crate::AppState;
use crate::api::workspaces::{require_membership, workspace_by_slug};
use crate::auth::extract::CurrentUser;
use crate::blobs;
use crate::error::{ApiError, ApiResult};
use crate::lgs_host::workbench;

pub fn router() -> Router<AppState> {
    Router::new()
        // `any` so every method (GET devtool/replay, POST wallet, …) dispatches.
        .route(
            "/ws/:slug/g/:game/r/:number/*rest",
            any(crate::lgs_host::dispatch),
        )
        // Mint a capability token for the cross-origin mount below. Session- or
        // PAT-authenticated + membership-checked, like any other write.
        .route("/workbench-tokens", post(mint_workbench_token))
        .merge(workbench_mount())
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

/// The cross-origin LGS mount, isolated in its own router so the CORS layer
/// applies to it and to nothing else.
///
/// The layer mirrors ANY request origin but deliberately does **not** allow
/// credentials: the path token is the whole credential, so the browser must
/// never attach the session cookie here. That combination is what makes
/// mirroring safe — a page that does not hold the token gets nothing, and one
/// that does was handed it by the workbench on the user's behalf.
///
/// Without this layer the preflight would reach `dispatch_workbench` as a plain
/// OPTIONS with no token path it can serve; with it, tower-http answers the
/// preflight itself and the real request follows.
fn workbench_mount() -> Router<AppState> {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::mirror_request())
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            HeaderName::from_static("content-type"),
            HeaderName::from_static("authorization"),
            HeaderName::from_static("x-requested-with"),
        ])
        .expose_headers([HeaderName::from_static("content-type")]);

    Router::new()
        .route("/wb/:token/*rest", any(crate::lgs_host::dispatch_workbench))
        .layer(cors)
}

/// `POST /api/workbench-tokens` — mint a capability token pinned to one
/// `(workspace, game, revision)` for the calling user. The token authorizes the
/// `/api/wb/<token>` mount, which a game front on another origin can call
/// without any cookie. Membership is required to mint AND re-checked on every
/// request the token is later used for.
async fn mint_workbench_token(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<WorkbenchTokenRequest>,
) -> ApiResult<Json<WorkbenchToken>> {
    let workspace = workspace_by_slug(&state.pool, &body.workspace).await?;
    require_membership(&state.pool, workspace.id, user.user_id).await?;

    let game_id: Uuid =
        sqlx::query_scalar("SELECT id FROM games WHERE workspace_id = $1 AND slug = $2")
            .bind(workspace.id)
            .bind(&body.game)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(|| ApiError::not_found("game_not_found", "no such game"))?;
    let revision_id: Uuid =
        sqlx::query_scalar("SELECT id FROM revisions WHERE game_id = $1 AND number = $2")
            .bind(game_id)
            .bind(body.revision)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(|| ApiError::not_found("revision_not_found", "no such revision"))?;

    let (token, expires_at) = workbench::mint(
        &state.pool,
        user.user_id,
        workspace.id,
        game_id,
        revision_id,
    )
    .await?;
    Ok(Json(WorkbenchToken {
        prefix: workbench::mount_prefix(&token),
        token,
        expires_at,
    }))
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

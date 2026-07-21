//! Serve a front bundle's files from the workspace object store: content-type by
//! extension, immutable caching for hashed assets, `no-cache` for `index.html`,
//! and an `index.html` SPA fallback for unknown non-API paths.

use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
// `get(&path)` (the whole-object convenience) lives on `ObjectStoreExt` in
// object_store 0.14, not the base trait — same import the math handlers use.
use object_store::ObjectStoreExt;
use uuid::Uuid;

use crate::AppState;
use crate::blobs;

use super::pages;
use super::resolve::ResolvedBundle;

/// Serve the manifest path for a `GET`/`HEAD` request path (already stripped of
/// its leading slash). Applies the `index.html` fallback and blocks API-ish
/// prefixes from resolving to the shell.
pub(super) async fn serve(
    state: &AppState,
    workspace_id: Uuid,
    bundle: &ResolvedBundle,
    request_path: &str,
) -> Response {
    let rel = request_path.trim_start_matches('/');
    let key = if rel.is_empty() { "index.html" } else { rel };

    if let Some(entry) = bundle.entries.get(key) {
        return stream(state, workspace_id, key, &entry.hash, entry.size).await;
    }

    // Unknown API/replay paths must never fall back to the SPA shell.
    if rel.starts_with("api/") || rel.starts_with("bet/") || rel == "__share/unlock" {
        return pages::api_error(StatusCode::NOT_FOUND, "not_found", "no such endpoint");
    }

    // SPA fallback: any other unknown path serves index.html so client routing
    // resolves deep links.
    match bundle.entries.get("index.html") {
        Some(entry) => stream(state, workspace_id, "index.html", &entry.hash, entry.size).await,
        None => pages::no_bundle(),
    }
}

/// Stream one bundle file from the object store with the right content-type and
/// cache policy.
async fn stream(
    state: &AppState,
    workspace_id: Uuid,
    path: &str,
    hash_hex: &str,
    size: i64,
) -> Response {
    let key = blobs::blob_key(workspace_id, hash_hex);
    let result = match state.store.get(&key).await {
        Ok(result) => result,
        Err(object_store::Error::NotFound { .. }) => {
            return pages::not_found();
        }
        Err(e) => {
            tracing::error!(error = %e, path, "share: failed to read bundle file");
            return pages::internal();
        }
    };

    let mut response = Response::new(Body::from_stream(result.into_stream()));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type(path)),
    );
    if let Ok(len) = header::HeaderValue::from_str(&size.to_string()) {
        headers.insert(header::CONTENT_LENGTH, len);
    }
    // index.html is the mutable entry point (it references the current hashed
    // assets); everything else is content-addressed and safe to cache forever.
    let cache = if is_index(path) {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static(cache),
    );
    response.into_response()
}

fn is_index(path: &str) -> bool {
    path == "index.html" || path.ends_with("/index.html")
}

/// Best-effort content-type from a file extension. Falls back to
/// `application/octet-stream`. (`mime_guess` is not a dependency of this crate, so
/// the common web set is hand-rolled here.)
fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "eot" => "application/vnd.ms-fontobject",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

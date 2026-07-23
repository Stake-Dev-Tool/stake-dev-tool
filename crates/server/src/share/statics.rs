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

/// The tag injected into a feedback-enabled link's `index.html`. The script is
/// served by [`super::feedback`] on this same host (same-origin, no CSP cross
/// concern) and 404s if feedback is later toggled off.
///
/// Deliberately NOT `defer`: the widget must execute before the game's own
/// (module/deferred) scripts so its `getContext` patch can force
/// `preserveDrawingBuffer: true` on the game's WebGL context — without it the
/// screenshot capture reads a black frame. That is also why the tag goes at the
/// END of `<head>` rather than before `</body>` (module scripts always run
/// after a parser-blocking head script, wherever they appear).
const FEEDBACK_TAG: &[u8] = b"<script src=\"/__share/feedback.js\"></script>";

/// Serve the manifest path for a `GET`/`HEAD` request path (already stripped of
/// its leading slash). Applies the `index.html` fallback and blocks API-ish
/// prefixes from resolving to the shell. `inject_feedback` splices the feedback
/// widget tag into any served `index.html`.
pub(super) async fn serve(
    state: &AppState,
    workspace_id: Uuid,
    bundle: &ResolvedBundle,
    request_path: &str,
    inject_feedback: bool,
) -> Response {
    let rel = request_path.trim_start_matches('/');
    let key = if rel.is_empty() { "index.html" } else { rel };

    if let Some(entry) = bundle.entries.get(key) {
        return stream(
            state,
            workspace_id,
            key,
            &entry.hash,
            entry.size,
            inject_feedback,
        )
        .await;
    }

    // Unknown API/replay paths must never fall back to the SPA shell.
    if rel.starts_with("api/") || rel.starts_with("bet/") || rel.starts_with("__share/") {
        return pages::api_error(StatusCode::NOT_FOUND, "not_found", "no such endpoint");
    }

    // SPA fallback: any other unknown path serves index.html so client routing
    // resolves deep links.
    match bundle.entries.get("index.html") {
        Some(entry) => {
            stream(
                state,
                workspace_id,
                "index.html",
                &entry.hash,
                entry.size,
                inject_feedback,
            )
            .await
        }
        None => pages::no_bundle(),
    }
}

/// Stream one bundle file from the object store with the right content-type and
/// cache policy. A feedback-enabled `index.html` is buffered (it is small — the
/// mutable entry point, never a hashed asset) so the widget tag can be spliced in.
async fn stream(
    state: &AppState,
    workspace_id: Uuid,
    path: &str,
    hash_hex: &str,
    size: i64,
    inject_feedback: bool,
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

    let inject = inject_feedback && is_index(path);
    let (body, size) = if inject {
        match result.bytes().await {
            Ok(bytes) => {
                let html = inject_widget_tag(bytes.to_vec());
                let len = html.len() as i64;
                (Body::from(html), len)
            }
            Err(e) => {
                tracing::error!(error = %e, path, "share: failed to buffer index for injection");
                return pages::internal();
            }
        }
    } else {
        (Body::from_stream(result.into_stream()), size)
    };

    let mut response = Response::new(body);
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

/// Splice [`FEEDBACK_TAG`] into an HTML document, ASCII case-insensitive:
/// before `</head>` (so the widget's WebGL `getContext` patch installs before
/// any game module/deferred script runs), else before the last `</body>`, else
/// appended at the end — bundles are arbitrary third-party builds, so never
/// assume a well-formed skeleton.
fn inject_widget_tag(html: Vec<u8>) -> Vec<u8> {
    let at = find_first_ci(&html, b"</head>")
        .or_else(|| find_last_ci(&html, b"</body>"))
        .unwrap_or(html.len());
    let mut out = Vec::with_capacity(html.len() + FEEDBACK_TAG.len());
    out.extend_from_slice(&html[..at]);
    out.extend_from_slice(FEEDBACK_TAG);
    out.extend_from_slice(&html[at..]);
    out
}

fn matches_ci(haystack: &[u8], needle: &[u8], at: usize) -> bool {
    haystack[at..at + needle.len()]
        .iter()
        .zip(needle)
        .all(|(a, b)| a.eq_ignore_ascii_case(b))
}

/// Byte-wise ASCII case-insensitive search for the FIRST occurrence of `needle`.
fn find_first_ci(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| matches_ci(haystack, needle, i))
}

/// Byte-wise ASCII case-insensitive search for the LAST occurrence of `needle`.
fn find_last_ci(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len())
        .rev()
        .find(|&i| matches_ci(haystack, needle, i))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widget_tag_prefers_head_then_body_then_append() {
        // With a <head>: the tag lands at the END of head, before the game's
        // module script (so the WebGL getContext patch installs first).
        let html =
            b"<html><head><meta charset=utf-8></head><body><div>game</div></body></html>".to_vec();
        let out = String::from_utf8(inject_widget_tag(html)).unwrap();
        let tag_at = out.find("<script src=\"/__share/feedback.js\"").unwrap();
        assert!(tag_at < out.find("</head>").unwrap() + "</head>".len());
        assert!(out.contains("<meta charset=utf-8><script"));

        // No head -> before the last </body> (case-insensitive).
        let upper = String::from_utf8(inject_widget_tag(b"<BODY>x</BODY>".to_vec())).unwrap();
        assert!(upper.ends_with("</BODY>"));
        assert!(upper.contains("feedback.js"));

        // Neither -> appended.
        let bare = String::from_utf8(inject_widget_tag(b"<div>fragment</div>".to_vec())).unwrap();
        assert!(bare.starts_with("<div>fragment</div><script"));
    }
}

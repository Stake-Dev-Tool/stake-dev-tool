//! Visitor feedback on the share host (the "feedback widget tied to the exact
//! bet event" item from V2.md §Share v2).
//!
//! Two public surfaces, both host-dispatched and gated on the link's
//! `feedback_enabled` toggle:
//! - `GET /__share/feedback.js` — the self-contained overlay widget
//!   ([`feedback_widget.js`]) that [`super::statics`] injects into the served
//!   `index.html`.
//! - `POST /__share/feedback` — a visitor submission: written note and/or
//!   vector annotation shapes plus a best-effort screenshot, stamped with the
//!   last played round `(revision, mode, eventId)` — the same triplet that
//!   addresses a book line for replay/saved rounds. The widget sends the round
//!   it observed client-side; when it saw none, the per-session record kept by
//!   [`super::rgs`] in [`super::runtime`] fills it in.
//!
//! The dashboard read/delete surface lives in [`crate::api::shares`].

use axum::Json;
use axum::body::Bytes;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::AppState;

use super::pages;
use super::resolve::{self, ResolvedShare};
use super::runtime::runtime;

/// The injected overlay widget, embedded at compile time.
const WIDGET_JS: &str = include_str!("feedback_widget.js");

/// Validation caps. The whole request body is additionally bounded by axum's
/// default 2 MiB body limit; the widget keeps screenshots well under it.
const MAX_MESSAGE_CHARS: usize = 4000;
const MAX_NAME_CHARS: usize = 120;
const MAX_SESSION_CHARS: usize = 128;
const MAX_MODE_CHARS: usize = 64;
const MAX_DRAWING_BYTES: usize = 256 * 1024;
const MAX_SCREENSHOT_BYTES: usize = 1_500_000;
/// Hard per-link cap on stored entries (screenshots live in Postgres; this
/// bounds a hostile visitor's storage impact even inside the rate limit).
const MAX_ENTRIES_PER_LINK: i64 = 500;

/// `GET /__share/feedback.js` — the widget source, only while the link has
/// feedback enabled (so a stale injected tag on a toggled-off link 404s and the
/// overlay never mounts).
pub(super) fn widget(link: &ResolvedShare) -> Response {
    if !link.feedback_enabled {
        return pages::api_error(
            StatusCode::NOT_FOUND,
            "feedback_disabled",
            "feedback is not enabled on this link",
        );
    }
    (
        [
            (
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("text/javascript; charset=utf-8"),
            ),
            (
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-cache"),
            ),
        ],
        WIDGET_JS,
    )
        .into_response()
}

#[derive(Deserialize)]
struct ViewportBody {
    w: Option<i32>,
    h: Option<i32>,
}

/// `POST /__share/feedback` request body, as sent by the widget.
#[derive(Deserialize)]
struct FeedbackBody {
    #[serde(rename = "sessionID")]
    session_id: Option<String>,
    name: Option<String>,
    message: Option<String>,
    /// `{"shapes":[…]}` vector annotation in viewport CSS pixels.
    drawing: Option<Value>,
    /// `data:image/(jpeg|png|webp);base64,…` capture; invalid/oversized ones
    /// are silently dropped (the vector drawing still lands).
    screenshot: Option<String>,
    mode: Option<String>,
    #[serde(rename = "eventId")]
    event_id: Option<i64>,
    viewport: Option<ViewportBody>,
}

/// `POST /__share/feedback` — validate and store one submission. The password
/// gate has already run in the caller (like every API-shaped share surface).
pub(super) async fn submit(
    state: &AppState,
    link: &ResolvedShare,
    client_ip: &str,
    body: Bytes,
) -> Response {
    if !link.feedback_enabled {
        return pages::api_error(
            StatusCode::NOT_FOUND,
            "feedback_disabled",
            "feedback is not enabled on this link",
        );
    }
    if !runtime().allow_feedback(link.id, client_ip) {
        return pages::api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many feedback submissions, please try again later",
        );
    }

    let Ok(body) = serde_json::from_slice::<FeedbackBody>(&body) else {
        return pages::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_body",
            "the request body is not valid feedback JSON",
        );
    };

    let message = body.message.as_deref().unwrap_or("").trim().to_string();
    if message.chars().count() > MAX_MESSAGE_CHARS {
        return pages::api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "message_too_long",
            "the message is too long",
        );
    }
    let author_name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| name.chars().take(MAX_NAME_CHARS).collect::<String>());
    let session_id = body
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|sid| !sid.is_empty() && sid.len() <= MAX_SESSION_CHARS)
        .map(str::to_string);

    // A drawing must be a JSON object of bounded size ({"shapes":[…]}).
    let drawing = match &body.drawing {
        None => None,
        Some(value) if !value.is_object() => {
            return pages::api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_drawing",
                "the drawing must be a JSON object",
            );
        }
        Some(value) => {
            let size = serde_json::to_vec(value).map(|v| v.len()).unwrap_or(0);
            if size > MAX_DRAWING_BYTES {
                return pages::api_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "drawing_too_large",
                    "the drawing is too large",
                );
            }
            Some(value.clone())
        }
    };

    if message.is_empty() && drawing.is_none() {
        return pages::api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "empty_feedback",
            "write a note or draw something first",
        );
    }

    let screenshot = body.screenshot.as_deref().and_then(parse_screenshot);

    // The round reference: what the widget observed client-side, else the
    // server-side per-session record from the wallet path.
    let client_round = match (body.mode.as_deref(), body.event_id) {
        (Some(mode), Some(event_id))
            if !mode.is_empty()
                && mode.len() <= MAX_MODE_CHARS
                && (0..=i64::from(u32::MAX)).contains(&event_id) =>
        {
            Some((mode.to_string(), event_id))
        }
        _ => None,
    };
    let round = client_round.or_else(|| {
        session_id
            .as_deref()
            .and_then(|sid| runtime().last_round(link.id, sid))
    });
    // Stamp the revision the link is currently serving, so the round stays
    // addressable as (revision, mode, eventId) even if the link tracks latest.
    let revision_number = resolve::resolve_revision(&state.pool, link)
        .await
        .ok()
        .map(|(number, _)| number);

    let (viewport_w, viewport_h) = match &body.viewport {
        Some(v) => (
            v.w.filter(|w| (1..=100_000).contains(w)),
            v.h.filter(|h| (1..=100_000).contains(h)),
        ),
        None => (None, None),
    };

    // Per-link storage cap.
    let count: i64 =
        match sqlx::query_scalar("SELECT count(*) FROM share_feedback WHERE share_link_id = $1")
            .bind(link.id)
            .fetch_one(&state.pool)
            .await
        {
            Ok(count) => count,
            Err(e) => {
                tracing::error!(error = %e, "share: failed to count feedback");
                return pages::internal();
            }
        };
    if count >= MAX_ENTRIES_PER_LINK {
        return pages::api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "feedback_full",
            "this link has reached its feedback capacity",
        );
    }

    let (mode, event_id) = match round {
        Some((mode, event_id)) => (Some(mode), Some(event_id as i32)),
        None => (None, None),
    };
    let (screenshot_bytes, screenshot_mime) = match screenshot {
        Some((mime, bytes)) => (Some(bytes), Some(mime)),
        None => (None, None),
    };

    let inserted: Result<Uuid, sqlx::Error> = sqlx::query_scalar(
        "INSERT INTO share_feedback \
           (share_link_id, session_id, author_name, message, drawing, \
            screenshot, screenshot_mime, mode, event_id, revision_number, \
            viewport_w, viewport_h) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
         RETURNING id",
    )
    .bind(link.id)
    .bind(&session_id)
    .bind(&author_name)
    .bind(&message)
    .bind(&drawing)
    .bind(&screenshot_bytes)
    .bind(&screenshot_mime)
    .bind(&mode)
    .bind(event_id)
    .bind(revision_number)
    .bind(viewport_w)
    .bind(viewport_h)
    .fetch_one(&state.pool)
    .await;

    match inserted {
        Ok(id) => (StatusCode::CREATED, Json(json!({ "id": id }))).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "share: failed to insert feedback");
            pages::internal()
        }
    }
}

/// Parse a `data:image/…;base64,` URL into `(mime, bytes)`; `None` for any
/// non-image, malformed, empty, or oversized payload (the caller drops it).
fn parse_screenshot(data_url: &str) -> Option<(String, Vec<u8>)> {
    let rest = data_url.strip_prefix("data:")?;
    let (mime, b64) = rest.split_once(";base64,")?;
    if !matches!(mime, "image/jpeg" | "image/png" | "image/webp") {
        return None;
    }
    // Cheap pre-decode bound: base64 inflates by 4/3.
    if b64.len() > MAX_SCREENSHOT_BYTES / 3 * 4 + 8 {
        return None;
    }
    let bytes = BASE64.decode(b64).ok()?;
    (!bytes.is_empty() && bytes.len() <= MAX_SCREENSHOT_BYTES).then(|| (mime.to_string(), bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_screenshot_accepts_images_only() {
        // A 1x1 transparent PNG.
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
        let (mime, bytes) = parse_screenshot(png).expect("valid png accepted");
        assert_eq!(mime, "image/png");
        assert!(!bytes.is_empty());

        assert!(parse_screenshot("data:text/html;base64,PGI+").is_none());
        assert!(parse_screenshot("data:image/png;base64,!!!not-base64!!!").is_none());
        assert!(parse_screenshot("data:image/png;base64,").is_none());
        assert!(parse_screenshot("not a data url").is_none());
    }
}

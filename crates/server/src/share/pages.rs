//! Minimal, self-contained, branded HTML pages served on the share host for the
//! non-happy paths (unknown/revoked link, expired, no bundle/revision yet, over
//! capacity) and the password interstitial. No external assets, no redirects back
//! to the app — a share host must never leak the dashboard.

use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};

/// Shared page chrome: a centered card on a dark background.
fn shell(title: &str, heading: &str, body: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex, nofollow">
<title>{title}</title>
<style>
:root {{ color-scheme: dark; }}
* {{ box-sizing: border-box; }}
body {{
  margin: 0; min-height: 100vh; display: flex; align-items: center; justify-content: center;
  background: #0b0e14; color: #e6e9ef;
  font: 15px/1.5 system-ui, -apple-system, Segoe UI, Roboto, sans-serif;
  padding: 24px;
}}
.card {{
  width: 100%; max-width: 26rem; background: #141924; border: 1px solid #232a3a;
  border-radius: 14px; padding: 32px; text-align: center;
  box-shadow: 0 10px 40px rgba(0,0,0,.4);
}}
h1 {{ margin: 0 0 8px; font-size: 1.25rem; }}
p {{ margin: 0 0 8px; color: #9aa4b8; }}
form {{ margin-top: 20px; display: flex; flex-direction: column; gap: 12px; }}
input {{
  width: 100%; padding: 11px 12px; border-radius: 9px; border: 1px solid #2c3446;
  background: #0f1420; color: #e6e9ef; font-size: 15px;
}}
input:focus {{ outline: 2px solid #4c7dff; outline-offset: 0; border-color: transparent; }}
button {{
  padding: 11px 12px; border-radius: 9px; border: 0; cursor: pointer;
  background: #4c7dff; color: #fff; font-size: 15px; font-weight: 600;
}}
button:hover {{ background: #3d68ef; }}
.err {{ color: #ff8a8a; }}
.brand {{ margin-top: 22px; font-size: .8rem; color: #5b6577; }}
</style>
</head>
<body>
<div class="card">
<h1>{heading}</h1>
{body}
<div class="brand">Stake Dev Tool</div>
</div>
</body>
</html>"#
    )
}

fn page(status: StatusCode, title: &str, heading: &str, message: &str) -> Response {
    let body = format!("<p>{message}</p>");
    (status, Html(shell(title, heading, &body))).into_response()
}

/// Unknown, revoked, or otherwise nonexistent link.
pub(super) fn not_found() -> Response {
    page(
        StatusCode::NOT_FOUND,
        "Not found",
        "This link isn't available",
        "The share link you followed doesn't exist or has been revoked.",
    )
}

/// Expired link.
pub(super) fn expired() -> Response {
    page(
        StatusCode::NOT_FOUND,
        "Link expired",
        "This link has expired",
        "The share link you followed is no longer active.",
    )
}

/// The link resolves but the game has no front bundle to serve yet.
pub(super) fn no_bundle() -> Response {
    page(
        StatusCode::NOT_FOUND,
        "Not ready",
        "No front build yet",
        "This game hasn't published a front-end build for sharing yet.",
    )
}

/// The link resolves but there is no revision to play against.
pub(super) fn no_revision() -> Response {
    page(
        StatusCode::NOT_FOUND,
        "Not ready",
        "No game build yet",
        "This game hasn't published a playable revision yet.",
    )
}

/// A generic internal error page (never leaks detail).
pub(super) fn internal() -> Response {
    page(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Error",
        "Something went wrong",
        "We couldn't load this demo. Please try again later.",
    )
}

/// The password interstitial. `error` renders an inline message under the form.
pub(super) fn unlock_form(error: Option<&str>) -> Response {
    let err = match error {
        Some(msg) => format!(r#"<p class="err">{msg}</p>"#),
        None => String::new(),
    };
    let body = format!(
        r#"<p>This demo is password protected.</p>
<form method="POST" action="/__share/unlock">
<input type="password" name="password" placeholder="Password" autofocus autocomplete="current-password" required>
<button type="submit">Unlock</button>
{err}
</form>"#
    );
    // 200 so the form renders in-place on the requested URL.
    (
        StatusCode::OK,
        Html(shell("Password required", "Password required", &body)),
    )
        .into_response()
}

/// A tiny JSON body for API-shaped paths (RGS/replay) that fail before dispatch.
pub(super) fn api_error(status: StatusCode, code: &str, message: &str) -> Response {
    let body = format!(r#"{{"error":{{"code":"{code}","message":"{message}"}}}}"#);
    let mut resp = (status, body).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    resp
}

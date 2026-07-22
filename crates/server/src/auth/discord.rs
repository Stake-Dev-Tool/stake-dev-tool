//! Discord OAuth web-flow helpers (optional; active only when configured).
//!
//! Mirrors `github`: thin HTTP helpers, with the account-resolution policy
//! (link existing, else create) living in the `api::auth` handler.

use serde::Deserialize;

use crate::config::DiscordConfig;
use crate::error::ApiError;

/// CSRF state cookie set by `/start` and verified by `/callback`.
pub const DISCORD_STATE_COOKIE: &str = "sdt_dc_state";

const AUTHORIZE_URL: &str = "https://discord.com/oauth2/authorize";
const TOKEN_URL: &str = "https://discord.com/api/oauth2/token";
const USER_URL: &str = "https://discord.com/api/users/@me";

/// The Discord account we care about. `id` is a snowflake (kept as a string —
/// it exceeds a 32-bit int), `email`/`verified` are only present with the
/// `email` scope and a confirmed address.
#[derive(Debug, Deserialize)]
pub struct DiscordUser {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub verified: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
}

/// Builds the `authorize` URL to redirect the browser to, requesting the
/// `identify` and `email` scopes.
pub fn authorize_url(cfg: &DiscordConfig, redirect_uri: &str, state: &str) -> String {
    reqwest::Url::parse_with_params(
        AUTHORIZE_URL,
        &[
            ("client_id", cfg.client_id.as_str()),
            ("response_type", "code"),
            ("scope", "identify email"),
            ("redirect_uri", redirect_uri),
            ("state", state),
        ],
    )
    .map(|url| url.to_string())
    .unwrap_or_else(|_| AUTHORIZE_URL.to_string())
}

/// Exchanges an authorization `code` for an access token.
pub async fn exchange_code(
    client: &reqwest::Client,
    cfg: &DiscordConfig,
    code: &str,
    redirect_uri: &str,
) -> Result<String, ApiError> {
    let response = client
        .post(TOKEN_URL)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&[
            ("client_id", cfg.client_id.as_str()),
            ("client_secret", cfg.client_secret.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .map_err(|e| ApiError::bad_request("discord_exchange_failed", e.to_string()))?;
    let body: TokenResponse = response
        .json()
        .await
        .map_err(|e| ApiError::bad_request("discord_exchange_failed", e.to_string()))?;
    body.access_token
        .ok_or_else(|| ApiError::bad_request("discord_exchange_failed", "no access token returned"))
}

/// Fetches the authenticated Discord user (`/users/@me`).
pub async fn fetch_user(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<DiscordUser, ApiError> {
    client
        .get(USER_URL)
        .bearer_auth(access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| ApiError::bad_request("discord_user_failed", e.to_string()))?
        .json()
        .await
        .map_err(|e| ApiError::bad_request("discord_user_failed", e.to_string()))
}

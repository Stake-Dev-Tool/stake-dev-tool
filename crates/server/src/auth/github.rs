//! GitHub OAuth web-flow helpers (optional; active only when configured).
//!
//! These are thin HTTP helpers; the account-resolution policy (link existing,
//! else create) lives in the `api::auth` handler that orchestrates them.

use serde::Deserialize;

use crate::config::GithubConfig;
use crate::error::ApiError;

/// CSRF state cookie set by `/start` and verified by `/callback`.
pub const GITHUB_STATE_COOKIE: &str = "sdt_gh_state";

const AUTHORIZE_URL: &str = "https://github.com/login/oauth/authorize";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const USER_URL: &str = "https://api.github.com/user";
const EMAILS_URL: &str = "https://api.github.com/user/emails";
/// GitHub rejects API requests without a User-Agent.
const USER_AGENT: &str = "stake-dev-tool";

/// The GitHub account we care about: numeric id (stable) and login (display).
#[derive(Debug, Deserialize)]
pub struct GithubUser {
    pub id: i64,
    pub login: String,
}

#[derive(Debug, Deserialize)]
struct GithubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
}

/// Builds the `authorize` URL to redirect the browser to, requesting the scopes
/// needed to read the account and its verified emails. Uses `reqwest::Url`
/// (the re-exported `url` crate) so parameters are correctly percent-encoded.
pub fn authorize_url(cfg: &GithubConfig, redirect_uri: &str, state: &str) -> String {
    reqwest::Url::parse_with_params(
        AUTHORIZE_URL,
        &[
            ("client_id", cfg.client_id.as_str()),
            ("redirect_uri", redirect_uri),
            ("scope", "read:user user:email"),
            ("state", state),
            ("allow_signup", "true"),
        ],
    )
    .map(|url| url.to_string())
    .unwrap_or_else(|_| AUTHORIZE_URL.to_string())
}

/// Exchanges an authorization `code` for an access token.
pub async fn exchange_code(
    client: &reqwest::Client,
    cfg: &GithubConfig,
    code: &str,
    redirect_uri: &str,
) -> Result<String, ApiError> {
    let response = client
        .post(TOKEN_URL)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .form(&[
            ("client_id", cfg.client_id.as_str()),
            ("client_secret", cfg.client_secret.as_str()),
            ("code", code),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .map_err(|e| ApiError::bad_request("github_exchange_failed", e.to_string()))?;
    let body: TokenResponse = response
        .json()
        .await
        .map_err(|e| ApiError::bad_request("github_exchange_failed", e.to_string()))?;
    body.access_token
        .ok_or_else(|| ApiError::bad_request("github_exchange_failed", "no access token returned"))
}

/// Fetches the authenticated GitHub user.
pub async fn fetch_user(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<GithubUser, ApiError> {
    client
        .get(USER_URL)
        .bearer_auth(access_token)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| ApiError::bad_request("github_user_failed", e.to_string()))?
        .json()
        .await
        .map_err(|e| ApiError::bad_request("github_user_failed", e.to_string()))
}

/// Fetches the user's primary, verified email — the identity we key accounts on.
pub async fn fetch_primary_email(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<String, ApiError> {
    let emails: Vec<GithubEmail> = client
        .get(EMAILS_URL)
        .bearer_auth(access_token)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| ApiError::bad_request("github_email_failed", e.to_string()))?
        .json()
        .await
        .map_err(|e| ApiError::bad_request("github_email_failed", e.to_string()))?;
    emails
        .into_iter()
        .find(|e| e.primary && e.verified)
        .map(|e| e.email)
        .ok_or_else(|| {
            ApiError::bad_request(
                "github_email_failed",
                "no primary verified email on the account",
            )
        })
}

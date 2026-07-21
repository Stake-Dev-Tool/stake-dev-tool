//! Cloud device-flow sign-in, mirroring [`crate::github::auth`] but driving the
//! server's RFC 8628 endpoints (`protocol::Device*`). The minted API token is
//! stored in the OS keyring under a cloud-specific user (`"cloud-token"`) so it
//! lives alongside — and never collides with — the GitHub OAuth token
//! (`"github-oauth-token"`).

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use protocol::{CreatedToken, DeviceCodeResponse, DeviceTokenRequest, User, UserResponse};

use super::config;

const KEYRING_SERVICE: &str = "stake-dev-tool";
const KEYRING_USER: &str = "cloud-token";

/// Interval floor when the server does not (or cannot) dictate one. 5s is the
/// RFC 8628 minimum and the server's advertised interval.
const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;

/// The signed-in user, surfaced once the device is approved.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudAuthState {
    pub user: User,
}

/// One device-flow poll result, shaped like `github::auth::DeviceFlowPoll`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudDeviceFlowPoll {
    /// Present once the user approved the device in the dashboard. Otherwise
    /// `None` and the caller waits `next_interval_secs` before polling again.
    #[serde(default)]
    pub auth: Option<CloudAuthState>,
    /// Seconds the caller must wait before the next poll. Bumped on `slow_down`.
    pub next_interval_secs: u64,
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("stake-dev-tool")
        .timeout(Duration::from_secs(15))
        .build()
        .context("build reqwest client")
}

/// Step 1 of the device flow: ask the server for a device + user code.
///
/// The returned `verification_uri` is the dashboard's `{base}/device` approval
/// page; the calling command surfaces it together with `user_code` so the UI
/// can display and/or open it.
pub async fn request_device_code() -> Result<DeviceCodeResponse> {
    let base = config::get()?;
    let client = http_client()?;
    let res = client
        .post(format!("{base}/api/auth/device/code"))
        .header("Accept", "application/json")
        .send()
        .await
        .context("request device code")?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(anyhow!("device code request failed: {status} {body}"));
    }

    res.json().await.context("parse device code response")
}

/// Step 2 of the device flow: poll once for a token. Call every
/// `next_interval_secs` seconds until the user approves or the code expires.
///
/// Returns:
/// - `Ok(poll)` with `auth: Some(_)` — approved; token stored in the keyring.
/// - `Ok(poll)` with `auth: None` — still pending; poll again after the interval.
/// - `Err(..)` — fatal (expired, denied, network, decode, …).
pub async fn poll_for_token(
    device_code: &str,
    current_interval: u64,
) -> Result<CloudDeviceFlowPoll> {
    let base = config::get()?;
    let client = http_client()?;
    let res = client
        .post(format!("{base}/api/auth/device/token"))
        .header("Accept", "application/json")
        .json(&DeviceTokenRequest {
            device_code: device_code.to_string(),
        })
        .send()
        .await
        .context("poll for token")?;

    let status = res.status();
    let body_text = res.text().await.context("read token body")?;
    tracing::debug!(status = %status, body = %body_text, "cloud device flow poll response");

    // Success: the server minted an API token (returned exactly once). Fetch the
    // user first (like the GitHub poller) so a bad token never gets persisted.
    if status.is_success() {
        let created: CreatedToken =
            serde_json::from_str(&body_text).context("parse device token response")?;
        let user = fetch_me(&base, &created.token).await?;
        store_token(&created.token)?;
        tracing::info!(user = %user.display_name, "cloud device flow: signed in");
        return Ok(CloudDeviceFlowPoll {
            auth: Some(CloudAuthState { user }),
            next_interval_secs: current_interval,
        });
    }

    // Otherwise the server uses the flat RFC 8628 error shape, e.g.
    // `{"error": "authorization_pending"}` (not the `{code,message}` envelope).
    #[derive(Deserialize)]
    struct FlatError {
        #[serde(default)]
        error: Option<String>,
    }
    let parsed: FlatError = serde_json::from_str(&body_text).unwrap_or(FlatError { error: None });

    match parsed.error.as_deref() {
        Some("authorization_pending") => Ok(CloudDeviceFlowPoll {
            auth: None,
            next_interval_secs: current_interval.max(DEFAULT_POLL_INTERVAL_SECS),
        }),
        // The server sends no interval hint with `slow_down`, so back off by the
        // RFC-recommended +5s on top of the current interval.
        Some("slow_down") => {
            let next = current_interval + 5;
            tracing::info!(
                next_interval = next,
                "cloud device flow: slow_down, backing off"
            );
            Ok(CloudDeviceFlowPoll {
                auth: None,
                next_interval_secs: next,
            })
        }
        Some("expired_token") => Err(anyhow!(
            "device code expired before it was approved — start sign-in again"
        )),
        Some("access_denied") => Err(anyhow!("the sign-in request was denied")),
        Some(other) => Err(anyhow!("cloud device flow: {other}")),
        None => Err(anyhow!(
            "cloud device flow: unexpected response — {status} {body_text}"
        )),
    }
}

/// `GET /api/auth/me` with a bearer token, returning the account behind it.
async fn fetch_me(base: &str, token: &str) -> Result<User> {
    let client = http_client()?;
    let res = client
        .get(format!("{base}/api/auth/me"))
        .bearer_auth(token)
        .header("Accept", "application/json")
        .send()
        .await
        .context("fetch current user")?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(anyhow!("fetch current user failed: {status} {body}"));
    }

    let parsed: UserResponse = res.json().await.context("parse user response")?;
    Ok(parsed.user)
}

fn keyring_entry() -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).context("open keyring entry")
}

pub fn store_token(token: &str) -> Result<()> {
    keyring_entry()?
        .set_password(token)
        .context("store cloud token in keyring")
}

pub fn load_token() -> Result<Option<String>> {
    match keyring_entry()?.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e).context("load cloud token from keyring"),
    }
}

pub fn clear_token() -> Result<()> {
    match keyring_entry()?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e).context("clear cloud token from keyring"),
    }
}

/// The currently signed-in cloud user, or `None` when signed out or the stored
/// token no longer validates (treated as signed out, like the GitHub variant).
pub async fn current_user() -> Result<Option<User>> {
    let Some(token) = load_token()? else {
        return Ok(None);
    };
    let base = config::get()?;
    match fetch_me(&base, &token).await {
        Ok(u) => Ok(Some(u)),
        Err(e) => {
            tracing::warn!(error = %e, "stored cloud token appears invalid");
            Ok(None)
        }
    }
}

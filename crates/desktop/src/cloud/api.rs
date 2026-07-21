//! Bearer-authenticated HTTP client for the cloud server, mirroring
//! [`crate::github::api::GithubClient`]: a `reqwest::Client`, a base URL taken
//! from [`super::config`] (not hardcoded), and the keyring-stored device token.
//!
//! Only the M1 server surface (workspaces + invites) is implemented. Of these,
//! [`crate::commands::cloud_list_workspaces`] wires in `list_workspaces` today;
//! the remaining methods land here so the client surface is complete and
//! reviewable, and are exposed as commands during M3 proper.

use std::time::Duration;

use serde::de::DeserializeOwned;

use protocol::{
    CreateInviteRequest, CreateWorkspaceRequest, CreatedInvite, ErrorResponse, Role,
    WorkspaceDetail, WorkspaceSummary, WorkspacesResponse,
};

use super::auth::load_token;
use super::config;

/// Errors surfaced by [`CloudClient`]. `Api` carries the server's structured
/// `{"error": {code, message}}` envelope; the other variants cover transport,
/// non-envelope HTTP failures, decode errors, and the signed-out case.
#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error("not signed in to cloud")]
    NotSignedIn,
    #[error("{code}: {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },
    #[error("cloud request failed: HTTP {status} {body}")]
    Http { status: u16, body: String },
    #[error("failed to decode cloud response: {0}")]
    Decode(String),
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
    #[error("{0}")]
    Other(String),
}

impl CloudError {
    /// Maps a failed HTTP response to a [`CloudError`], decoding the uniform
    /// `{"error": {code, message}}` envelope when present and otherwise keeping
    /// the raw body. Split from the network path so it is unit-testable with
    /// canned JSON.
    pub(crate) fn from_response(status: reqwest::StatusCode, body: &str) -> Self {
        match serde_json::from_str::<ErrorResponse>(body) {
            Ok(envelope) => CloudError::Api {
                status: status.as_u16(),
                code: envelope.error.code,
                message: envelope.error.message,
            },
            Err(_) => CloudError::Http {
                status: status.as_u16(),
                body: body.to_string(),
            },
        }
    }
}

/// Sends a prepared request, returning the decoded body on 2xx or a mapped
/// [`CloudError`]. Reading the body as text first lets the error path decode the
/// envelope regardless of status.
async fn send<T: DeserializeOwned>(rb: reqwest::RequestBuilder) -> Result<T, CloudError> {
    let res = rb.send().await?;
    let status = res.status();
    let body = res.text().await?;
    if !status.is_success() {
        return Err(CloudError::from_response(status, &body));
    }
    serde_json::from_str(&body).map_err(|e| CloudError::Decode(e.to_string()))
}

#[derive(Clone)]
pub struct CloudClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl CloudClient {
    /// Builds a client from the keyring-stored device token and the configured
    /// base URL. Errors with [`CloudError::NotSignedIn`] when signed out.
    pub fn from_stored_token() -> Result<Self, CloudError> {
        let token = load_token()
            .map_err(|e| CloudError::Other(e.to_string()))?
            .ok_or(CloudError::NotSignedIn)?;
        let base_url = config::get().map_err(|e| CloudError::Other(e.to_string()))?;
        let http = reqwest::Client::builder()
            .user_agent("stake-dev-tool")
            .timeout(Duration::from_secs(60))
            .build()?;
        Ok(Self {
            http,
            base_url,
            token,
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base_url))
            .bearer_auth(&self.token)
            .header("Accept", "application/json")
    }

    /// `GET /api/workspaces` — the caller's workspaces with their roles.
    pub async fn list_workspaces(&self) -> Result<WorkspacesResponse, CloudError> {
        send(self.request(reqwest::Method::GET, "/api/workspaces")).await
    }

    /// `GET /api/workspaces/:slug` — full workspace view (members-only).
    #[allow(dead_code)] // Exposed as a command in M3 proper; client method landed early.
    pub async fn workspace_detail(&self, slug: &str) -> Result<WorkspaceDetail, CloudError> {
        send(self.request(reqwest::Method::GET, &format!("/api/workspaces/{slug}"))).await
    }

    /// `POST /api/workspaces` — create a workspace (caller becomes owner).
    #[allow(dead_code)] // Exposed as a command in M3 proper; client method landed early.
    pub async fn create_workspace(
        &self,
        name: &str,
        slug: &str,
    ) -> Result<WorkspaceSummary, CloudError> {
        let req = CreateWorkspaceRequest {
            name: name.to_string(),
            slug: slug.to_string(),
        };
        send(
            self.request(reqwest::Method::POST, "/api/workspaces")
                .json(&req),
        )
        .await
    }

    /// `POST /api/workspaces/:slug/invites` — mint an invite (owner/admin).
    /// Uses the server defaults for expiry and max-uses.
    #[allow(dead_code)] // Exposed as a command in M3 proper; client method landed early.
    pub async fn create_invite(&self, slug: &str, role: Role) -> Result<CreatedInvite, CloudError> {
        let req = CreateInviteRequest {
            role,
            expires_in_days: None,
            max_uses: None,
        };
        send(
            self.request(
                reqwest::Method::POST,
                &format!("/api/workspaces/{slug}/invites"),
            )
            .json(&req),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_error_envelope() {
        let body = r#"{"error":{"code":"slug_taken","message":"that slug is already taken"}}"#;
        match CloudError::from_response(reqwest::StatusCode::CONFLICT, body) {
            CloudError::Api {
                status,
                code,
                message,
            } => {
                assert_eq!(status, 409);
                assert_eq!(code, "slug_taken");
                assert_eq!(message, "that slug is already taken");
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    #[test]
    fn non_envelope_body_falls_back_to_http() {
        match CloudError::from_response(reqwest::StatusCode::BAD_GATEWAY, "upstream boom") {
            CloudError::Http { status, body } => {
                assert_eq!(status, 502);
                assert_eq!(body, "upstream boom");
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[test]
    fn api_error_display_is_human_readable() {
        let body = r#"{"error":{"code":"forbidden","message":"only owners and admins can perform this action"}}"#;
        let err = CloudError::from_response(reqwest::StatusCode::FORBIDDEN, body);
        assert_eq!(
            err.to_string(),
            "forbidden: only owners and admins can perform this action"
        );
    }
}

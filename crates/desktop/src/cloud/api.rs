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
    AcceptInviteRequest, AcceptInviteResponse, CreateInviteRequest, CreateWorkspaceRequest,
    CreatedInvite, ErrorResponse, Role, WorkspaceDetail, WorkspaceSummary, WorkspacesResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

    /// `POST /api/invites/:token/accept` — join the workspace behind an invite.
    ///
    /// Note: `crates/server`'s README marks invite-accept as *session* auth,
    /// which may reject a Bearer device token. The desktop attempts it here and
    /// surfaces any auth error so the UI can fall back to opening the invite URL
    /// in the browser (where the user has a cookie session). See the M3 report.
    pub async fn accept_invite(&self, token: &str) -> Result<AcceptInviteResponse, CloudError> {
        let req = AcceptInviteRequest {
            token: token.to_string(),
        };
        send(
            self.request(
                reqwest::Method::POST,
                &format!("/api/invites/{token}/accept"),
            )
            .json(&req),
        )
        .await
    }

    /// `DELETE /api/workspaces/:slug` — delete a workspace (owner only, 204).
    pub async fn delete_workspace(&self, slug: &str) -> Result<(), CloudError> {
        send_empty(self.request(reqwest::Method::DELETE, &format!("/api/workspaces/{slug}"))).await
    }

    /// `DELETE /api/workspaces/:slug/members/:user_id` — remove a member (used
    /// to leave: pass the caller's own id).
    pub async fn remove_member(&self, slug: &str, user_id: Uuid) -> Result<(), CloudError> {
        send_empty(self.request(
            reqwest::Method::DELETE,
            &format!("/api/workspaces/{slug}/members/{user_id}"),
        ))
        .await
    }

    /// `GET /api/workspaces/:slug/games` — the workspace's games with their
    /// head revision number and count.
    pub async fn list_games(&self, slug: &str) -> Result<Vec<GameSummary>, CloudError> {
        let resp: GamesResponse = send(self.request(
            reqwest::Method::GET,
            &format!("/api/workspaces/{slug}/games"),
        ))
        .await?;
        Ok(resp.games)
    }

    /// `GET /api/workspaces/:slug/games/:game/revisions` — a game's revisions,
    /// newest first (the M2 `{revisions:[…]}` shape).
    pub async fn list_revisions(
        &self,
        slug: &str,
        game: &str,
    ) -> Result<Vec<RevisionSummary>, CloudError> {
        let resp: RevisionsResponse = send(self.request(
            reqwest::Method::GET,
            &format!("/api/workspaces/{slug}/games/{game}/revisions"),
        ))
        .await?;
        Ok(resp.revisions)
    }

    /// `GET /api/workspaces/:slug/games/:game/revisions/:number` — revision
    /// detail: the file manifest plus the server-computed per-mode stats. The
    /// heavy `analysis` payload is intentionally not mirrored here.
    pub async fn revision_detail(
        &self,
        slug: &str,
        game: &str,
        number: i64,
    ) -> Result<RevisionDetail, CloudError> {
        send(self.request(
            reqwest::Method::GET,
            &format!("/api/workspaces/{slug}/games/{game}/revisions/{number}"),
        ))
        .await
    }
}

/// One game row from `GET …/games`. Mirrors the M2 shape leniently (extra
/// fields ignored); the desktop reads `slug` + `head_number` (has-math), and
/// carries `id`/`name`/`revisions_count` through to the cloud browser UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameSummary {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    pub slug: String,
    #[serde(default)]
    pub head_number: Option<i64>,
    #[serde(default)]
    pub revisions_count: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct GamesResponse {
    #[serde(default)]
    games: Vec<GameSummary>,
}

/// One revision row from `GET …/revisions` (newest first). Mirrors the M2
/// `RevisionSummary` leniently and is re-serialized straight to the cloud
/// browser UI, so the field names match the wire (snake_case).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionSummary {
    pub number: i64,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub author_display_name: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub files_count: i64,
    #[serde(default)]
    pub total_size: i64,
    /// `null` until the async stats task has created its row.
    #[serde(default)]
    pub stats_status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RevisionsResponse {
    #[serde(default)]
    revisions: Vec<RevisionSummary>,
}

/// One file in a revision manifest (`RevisionDetail.files`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionFileView {
    pub path: String,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub size: i64,
}

/// One bet mode's computed stats — RTP is a fraction, `max_win` a multiplier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionModeStats {
    pub mode: String,
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub rtp: f64,
    #[serde(default)]
    pub max_win: f64,
    #[serde(default)]
    pub entries: Option<u64>,
    #[serde(default)]
    pub hit_rate: Option<f64>,
}

/// A revision's stats block. The compliance `analysis` sibling on the wire is
/// deliberately dropped — the desktop browser only surfaces the per-mode strip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionStatsView {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub modes: Vec<RevisionModeStats>,
}

/// Revision detail: manifest + stats. Mirrors the M2 `RevisionDetail` leniently
/// (extra fields, incl. `stats.analysis`, ignored).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionDetail {
    pub number: i64,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub author_display_name: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub files: Vec<RevisionFileView>,
    #[serde(default)]
    pub stats: Option<RevisionStatsView>,
}

/// Like [`send`] but for endpoints that return an empty (204) body on success.
async fn send_empty(rb: reqwest::RequestBuilder) -> Result<(), CloudError> {
    let res = rb.send().await?;
    let status = res.status();
    if status.is_success() {
        return Ok(());
    }
    let body = res.text().await?;
    Err(CloudError::from_response(status, &body))
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

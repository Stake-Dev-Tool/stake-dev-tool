//! Wire types for workspaces, memberships, and invites.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// A member's role in a workspace. Serialized lowercase to match the database
/// `CHECK` constraint values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "protocol/")]
pub enum Role {
    Owner,
    Admin,
    Member,
}

impl Role {
    /// The exact string stored in the `role` columns.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Member => "member",
        }
    }

    /// Parses a `role` column value back into the enum. Returns `None` for any
    /// value the `CHECK` constraint would have rejected.
    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Role::Owner),
            "admin" => Some(Role::Admin),
            "member" => Some(Role::Member),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct CreateWorkspaceRequest {
    pub name: String,
    pub slug: String,
}

/// A workspace paired with the calling user's role in it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct WorkspaceSummary {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub role: Role,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct WorkspacesResponse {
    pub workspaces: Vec<WorkspaceSummary>,
}

/// A single member as shown on a workspace detail page.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct WorkspaceMember {
    pub id: Uuid,
    pub display_name: String,
    pub role: Role,
    pub joined: DateTime<Utc>,
}

/// Full workspace view: the workspace, the caller's role, and every member.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct WorkspaceDetail {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub role: Role,
    pub members: Vec<WorkspaceMember>,
    /// The workspace's attached custom play domain (lowercase, e.g.
    /// `play.acme.com`), or `null` when none is set. Share links are served at
    /// `<slug>.<custom_play_domain>` when present. `#[serde(default)]` keeps
    /// older payloads that predate the field deserializing cleanly.
    #[serde(default)]
    pub custom_play_domain: Option<String>,
}

/// `PUT /api/workspaces/:slug/domain` request body and response. `domain: null`
/// clears the workspace's custom play domain; a value sets it (validated +
/// lowercased server-side). The response echoes the stored value.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct WorkspaceDomain {
    #[serde(default)]
    pub domain: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct UpdateMemberRequest {
    pub role: Role,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct CreateInviteRequest {
    pub role: Role,
    pub expires_in_days: Option<i64>,
    pub max_uses: Option<i32>,
}

/// An invite's metadata. Never carries the secret or its hash.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct InviteInfo {
    pub id: Uuid,
    pub role: Role,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub max_uses: i32,
    pub uses: i32,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Response to invite creation: the secret and its shareable URL, shown once.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct CreatedInvite {
    pub invite_url: String,
    pub token: String,
    pub info: InviteInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct InvitesResponse {
    pub invites: Vec<InviteInfo>,
}

/// Public preview shown on the invite-accept page before the visitor signs in.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct InvitePreview {
    pub workspace_name: String,
    pub role: Role,
    pub inviter_display_name: Option<String>,
    pub valid: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct AcceptInviteRequest {
    pub token: String,
}

/// Response to accepting an invite: the workspace the caller now belongs to.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct AcceptInviteResponse {
    pub workspace: WorkspaceSummary,
}

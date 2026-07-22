//! Wire types for M3 "document sync": the per-kind payload schemas (`profile`,
//! `saved_round`), the small write responses, and the workspace SSE event
//! payloads.
//!
//! The HTTP *envelope* types that wrap an opaque `data` blob (the document
//! envelope, the list response, and the `PUT`/conflict bodies) live in the
//! server crate instead — they need `serde_json::Value` for the free-form
//! `data`, and `protocol` has no `serde_json` dependency. The durable schema
//! (the payloads and the realtime events) is here so every surface — desktop,
//! CLI, dashboard — shares one definition.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// The two document kinds. Serialized in `snake_case` to match the database
/// `CHECK` constraint values and the `?kind=` query parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "protocol/")]
pub enum DocumentKind {
    Profile,
    SavedRound,
}

impl DocumentKind {
    /// The exact string stored in the `kind` column / sent on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            DocumentKind::Profile => "profile",
            DocumentKind::SavedRound => "saved_round",
        }
    }

    /// Parses a `kind` value, returning `None` for anything the `CHECK`
    /// constraint would reject.
    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "profile" => Some(DocumentKind::Profile),
            "saved_round" => Some(DocumentKind::SavedRound),
            _ => None,
        }
    }
}

/// One display resolution stored inside a `profile` document. Mirrors the V1
/// desktop `ResolutionPreset`; `enabled`/`builtin` default so older records
/// (and partial payloads) still validate.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct ProfileResolution {
    pub id: String,
    pub label: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub builtin: bool,
}

/// The `profile` document payload (the `data` blob of a `profile` document).
/// Mirrors the V1 `Profile` minus the per-machine `gamePath` and the legacy
/// `teamId`. `game`/`revision` are the loose M2 linkage: a profile may point at
/// a workspace game that has no cloud revisions yet (`revision = null` = latest).
/// Unknown fields are tolerated on write and preserved verbatim in the stored
/// document; the server validates only the fields listed here.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct ProfileDocument {
    pub name: String,
    pub game_slug: String,
    #[serde(default)]
    pub game: Option<String>,
    #[serde(default)]
    pub revision: Option<i32>,
    #[serde(default)]
    pub front_url: Option<String>,
    pub resolutions: Vec<ProfileResolution>,
    pub created_at: i64,
}

/// The `saved_round` document payload. V1 `SavedRound` plus the optional M2
/// revision pin (`revision = null` = "latest at the time"; legacy imports leave
/// it null). Unknown fields are tolerated and preserved.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct SavedRoundDocument {
    pub game_slug: String,
    pub mode: String,
    pub event_id: u32,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub revision: Option<i32>,
    pub created_at: i64,
}

/// Success body of `PUT /workspaces/:slug/documents/:kind/:doc_id`: the new
/// per-document `revision` and the change cursor `seq` the write was assigned.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct PutDocumentResponse {
    pub revision: i32,
    pub seq: i64,
}

/// `DELETE /workspaces/:slug/documents/:kind/:doc_id` request body. When
/// `base_revision` is given it must equal the current revision (same optimistic
/// rule as `PUT`); omit it to delete unconditionally.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct DeleteDocumentRequest {
    #[serde(default)]
    pub base_revision: Option<i32>,
}

/// Success body of a delete: the change cursor `seq` the tombstone was assigned.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct DeleteDocumentResponse {
    pub seq: i64,
}

/// SSE `document` event: a document changed. `data` is intentionally not
/// inlined — the client pulls the new state via `?since_seq=` / GET.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct DocumentEvent {
    pub kind: DocumentKind,
    pub doc_id: String,
    pub seq: i64,
}

/// SSE `revision_pushed` event: a new math revision committed (hooked into the
/// M2 commit path).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct RevisionPushedEvent {
    pub game: String,
    pub number: i32,
}

/// SSE `front_pushed` event: a new front bundle committed for a game (hooked into
/// the M5 front-bundle commit path). The client re-probes the game's front /
/// bundle listing after the nudge.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct FrontPushedEvent {
    pub game: String,
    pub bundle_id: Uuid,
}

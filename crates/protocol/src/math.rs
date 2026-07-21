//! Wire types for M2 "Math revisions": games, content-addressed blobs, push
//! manifests, revisions, file diffs, and per-mode bet stats.
//!
//! Every hash crosses the wire as a **lowercase hex** sha256 string (64 chars).
//! Byte sizes are `i64` (Postgres `BIGINT`); they are validated `>= 0` on the
//! way in. Counts and multipliers follow the shapes the server computes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::error::ErrorBody;

/// One file in a push manifest / revision: its path, content hash, and size.
/// `hash` is a lowercase hex sha256 (64 chars); `size` is bytes.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct FileEntry {
    pub path: String,
    pub hash: String,
    pub size: i64,
}

/// `POST /workspaces/:slug/games/:game/revisions/check` request body.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct CheckRequest {
    pub files: Vec<FileEntry>,
}

/// Response: the subset of manifest hashes not yet in this workspace's blobs
/// (i.e. the ones the client still has to upload). Lowercase hex, deduped.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct CheckResponse {
    pub missing: Vec<String>,
}

/// `PUT /workspaces/:slug/games/:game/blobs/:hash` success body (201 fresh
/// upload, 200 if the blob already existed).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct BlobUploaded {
    pub hash: String,
    pub size: i64,
}

/// `POST /workspaces/:slug/games/:game/revisions` request body. `parent_number`
/// enables optimistic concurrency: when set it must equal the current head.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct CreateRevisionRequest {
    pub message: String,
    pub files: Vec<FileEntry>,
    pub parent_number: Option<i32>,
}

/// The `409 missing_blobs` body: the standard error envelope **plus** a
/// `missing` array of the lowercase-hex hashes that must be uploaded first.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct MissingBlobsResponse {
    pub error: ErrorBody,
    pub missing: Vec<String>,
}

/// A game as listed under `GET /workspaces/:slug/games`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct GameSummary {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    /// Highest revision number, or `null` when the game has no revisions yet.
    pub head_number: Option<i32>,
    pub revisions_count: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct GamesResponse {
    pub games: Vec<GameSummary>,
}

/// Lifecycle of a revision's computed stats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "protocol/")]
pub enum StatsStatus {
    Pending,
    Ok,
    Error,
}

/// A revision as listed under `GET .../revisions` (newest first).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct RevisionSummary {
    pub number: i32,
    pub message: String,
    pub author_display_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub files_count: i64,
    pub total_size: i64,
    /// `null` until the async stats task has created its row.
    pub stats_status: Option<StatsStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct RevisionsResponse {
    pub revisions: Vec<RevisionSummary>,
}

/// Per-mode bet stats derived from a mode's lookup table (weights CSV) alone —
/// no books are read. `cost` is the mode's cost multiplier; `rtp` is
/// `sum(weight * payout) / sum(weight) / cost` (payout being the decimal win
/// multiple, i.e. the CSV column / 100); `max_win` is the largest win multiple;
/// `entries` is the number of lookup rows; `hit_rate` is the share of weight
/// with a non-zero payout.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct ModeStats {
    pub mode: String,
    pub cost: f64,
    pub rtp: f64,
    pub max_win: f64,
    pub entries: u64,
    pub hit_rate: f64,
}

/// A revision's stats as attached to `RevisionDetail`. `modes` is populated only
/// when `status` is `ok`; `error` carries the reason when `status` is `error`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct RevisionStats {
    pub status: StatsStatus,
    pub error: Option<String>,
    pub modes: Vec<ModeStats>,
    pub updated_at: DateTime<Utc>,
}

/// Full revision view: metadata, its file manifest, and its stats (if any).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct RevisionDetail {
    pub number: i32,
    pub message: String,
    pub author_display_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub files: Vec<FileEntry>,
    pub stats: Option<RevisionStats>,
}

/// A file present in both revisions but with different content.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct ChangedFile {
    pub path: String,
    pub before_hash: String,
    pub after_hash: String,
    pub before_size: i64,
    pub after_size: i64,
}

/// File-level diff between two revisions ("before" = `:other`, "after" =
/// `:number`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct FileDiff {
    pub added: Vec<FileEntry>,
    pub removed: Vec<FileEntry>,
    pub changed: Vec<ChangedFile>,
    pub unchanged: u32,
}

/// Before/after stats for a single mode. Either side is `null` when its
/// revision's stats aren't `ok`, or when the mode is absent on that side.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct ModeStatsDiff {
    pub mode: String,
    pub before: Option<ModeStats>,
    pub after: Option<ModeStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct StatsDiff {
    pub modes: Vec<ModeStatsDiff>,
}

/// `GET .../revisions/:number/diff/:other` response.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct RevisionDiff {
    pub files: FileDiff,
    pub stats: StatsDiff,
}

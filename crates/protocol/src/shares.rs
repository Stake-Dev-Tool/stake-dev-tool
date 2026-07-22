//! M5 share-link wire types. Contract: docs/v2/m4-m5-contract.md §M5.
//!
//! These types are namespaced (`protocol::shares::…`) rather than re-exported at
//! the crate root, matching the M7 `billing` module. Crate numeric convention:
//! `i32`/`u32` counts render as TS `number`; `i64`/`u64` totals render as TS
//! `bigint`; `f64` money/rtp render as TS `number`; `Uuid`/`DateTime<Utc>` render
//! as TS `string`.
//!
//! Front bundles reuse the M2 push wire types verbatim: the client uploads bundle
//! files as blobs through the existing `PUT .../games/:game/blobs/:hash` endpoint,
//! then commits a manifest of [`FileEntry`]s here — the exact `check → upload →
//! commit` shape as math revisions, so a `409` carries the shared
//! [`MissingBlobsResponse`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::math::FileEntry;

/// `POST /workspaces/:slug/games/:game/front-bundles` request body — the full
/// manifest of the front build. `check` (which hashes are missing) reuses the
/// math [`crate::math::CheckRequest`]/[`crate::math::CheckResponse`] pair.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct CreateFrontBundleRequest {
    pub files: Vec<FileEntry>,
}

/// `201` response after a front bundle commits.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct FrontBundleCreated {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
}

/// One front bundle as listed under `GET .../front-bundles` (newest first).
/// `files_count`/`total_size` are derived from the stored manifest JSONB;
/// `is_latest` marks the newest bundle (the one a latest-tracking share serves).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct FrontBundleSummary {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub files_count: i64,
    pub total_size: i64,
    pub is_latest: bool,
}

/// `GET /workspaces/:slug/games/:game/front-bundles` response (newest first).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct FrontBundlesResponse {
    pub bundles: Vec<FrontBundleSummary>,
}

/// `POST /workspaces/:slug/games/:game/shares` request body. Every field is
/// optional; omitted fields take their default (generated slug, latest revision,
/// latest bundle, public, never expires, 25 concurrent sessions).
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct CreateShareRequest {
    /// Custom subdomain label. Generated `word-word-nnn` when omitted.
    pub slug: Option<String>,
    /// Pin a revision number; omit to track the game's latest revision.
    pub revision_number: Option<i32>,
    /// Pin a front bundle; omit to serve the game's latest bundle.
    pub front_bundle_id: Option<Uuid>,
    /// Optional password (plaintext; hashed with argon2 server-side).
    pub password: Option<String>,
    /// Expiry as a number of days from now; omit for no expiry.
    pub expires_in_days: Option<i64>,
    /// Concurrent visitor-session cap; omit for the default of 25.
    pub max_concurrent_sessions: Option<i32>,
}

/// `PATCH /workspaces/:slug/games/:game/shares/:id` request body.
///
/// The three nullable-and-optional fields use tri-state JSON semantics: an
/// **absent** key leaves the value unchanged, an explicit **null** clears it
/// (track-latest / remove password / never-expire), and a value sets it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct UpdateShareRequest {
    /// Absent = unchanged, `null` = track latest, `n` = pin revision `n`.
    #[serde(default, deserialize_with = "double_option")]
    #[ts(optional)]
    pub revision_number: Option<Option<i32>>,
    /// Absent = unchanged, `null` = latest bundle, id = pin that bundle.
    #[serde(default, deserialize_with = "double_option")]
    #[ts(optional)]
    pub front_bundle_id: Option<Option<Uuid>>,
    /// Absent = unchanged, `null` = remove password, string = set new password.
    #[serde(default, deserialize_with = "double_option")]
    #[ts(optional)]
    pub password: Option<Option<String>>,
    /// Absent = unchanged, `null` = never expire, `n` = expire `n` days from now.
    #[serde(default, deserialize_with = "double_option")]
    #[ts(optional)]
    pub expires_in_days: Option<Option<i64>>,
    /// Absent = unchanged; sets the concurrent visitor-session cap.
    pub max_concurrent_sessions: Option<i32>,
    /// Absent = unchanged, `true` = revoke, `false` = un-revoke.
    pub revoked: Option<bool>,
}

/// A share link as returned by create/list. `url` is the full
/// `https://<slug>.<play_domain>/` when the instance has a play domain
/// configured, else `null` (a note the dashboard renders instead).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct ShareLinkView {
    pub id: Uuid,
    pub slug: String,
    pub url: Option<String>,
    /// The game slug this link serves (handy for the dashboard's grouping).
    pub game: String,
    /// `null` = tracks the latest revision.
    pub revision_number: Option<i32>,
    /// `null` = serves the latest bundle.
    pub front_bundle_id: Option<Uuid>,
    pub password_protected: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_concurrent_sessions: i32,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub sessions_count: i64,
    pub spins_count: i64,
    pub total_bet: f64,
    pub total_win: f64,
    /// `total_win / total_bet` when `total_bet > 0`, else `null`.
    pub observed_rtp: Option<f64>,
    /// Best-effort count of visitor sessions seen in the last 30 min on this
    /// node (in-memory; not shared across a multi-node deployment).
    pub active_sessions: i64,
}

/// `GET /workspaces/:slug/games/:game/shares` response.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct ShareLinksResponse {
    pub shares: Vec<ShareLinkView>,
}

/// serde helper giving a field tri-state semantics: absent (`None`), explicit
/// null (`Some(None)`), or a value (`Some(Some(v))`). Pair with
/// `#[serde(default, deserialize_with = "double_option")]`.
fn double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Ok(Some(Option::deserialize(deserializer)?))
}

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
///
/// `analysis` is the full Stake-Engine-style compliance view (2★/3★). It is a
/// sibling of the untouched `modes` array and is present only when `status` is
/// `ok` and the revision has at least one mode. Old stats rows persisted before
/// this field existed deserialize with `analysis == None` (serde default).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct RevisionStats {
    pub status: StatsStatus,
    pub error: Option<String>,
    pub modes: Vec<ModeStats>,
    #[serde(default)]
    pub analysis: Option<RevisionAnalysis>,
    pub updated_at: DateTime<Utc>,
}

/// Stake-Engine-style compliance analysis for a whole revision: the global
/// constraint table (checked across modes at two reference bet levels), the
/// star grade derived from it, the cross-mode RTP spread, and the per-mode
/// deep-dive ([`ModeAnalysis`]).
///
/// Star grade: `stars = 3` when every constraint passes its 3★ limit, else `2`
/// when every constraint passes its (stricter) 2★ limit, else `0`. The 3★
/// limits are looser than the 2★ limits (higher-volatility games get more
/// headroom), so a game can be 3★ without being 2★.
///
/// `reference_max_bet_2`/`reference_max_bet_3` (200 / 1000) are documented
/// stand-ins for Stake's per-game bet-level templates: bet-scaled limits
/// (`max_exposure`, `max_bet_cost`) are evaluated at these bets.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct RevisionAnalysis {
    pub two_star_compliant: bool,
    pub three_star_compliant: bool,
    pub stars: u8,
    /// `max(RTP) − min(RTP)` across modes; Stake expects modes to share an RTP.
    pub cross_mode_rtp_variance: f64,
    /// `cross_mode_rtp_variance <= 0.01`.
    pub cross_mode_rtp_pass: bool,
    pub reference_max_bet_2: u64,
    pub reference_max_bet_3: u64,
    pub constraints: Vec<ConstraintRow>,
    pub modes: Vec<ModeAnalysis>,
}

/// One row of the global constraint table. Metrics come in three shapes:
/// * single-value metrics fill `value` (e.g. `max_payout_multiplier`);
/// * bet-scaled metrics fill `value2`/`value3` — the value computed at the 2★
///   and 3★ reference bets (e.g. `max_exposure`, `max_bet_cost`);
/// * range metrics fill `value` and both `limitX_low` bounds (e.g.
///   `base_volatility`, which must sit inside `[low, high]`).
///
/// `limit2`/`limit3` are the upper limits (or range highs). `pass2`/`pass3` are
/// the per-star verdicts.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct ConstraintRow {
    pub key: String,
    pub label: String,
    pub value: Option<f64>,
    pub value2: Option<f64>,
    pub value3: Option<f64>,
    pub limit2_low: Option<f64>,
    pub limit2: f64,
    pub limit3_low: Option<f64>,
    pub limit3: f64,
    pub pass2: bool,
    pub pass3: bool,
}

/// Per-mode deep-dive. All multipliers are "VS BET" decimal multiples
/// (`m = payout / 100`); cost-normalized quantities use `x = m / cost` so that
/// RTP, `std_dev`, CVaR and the ETLs are all expressed "at cost 1×".
///
/// `volatility` is a heuristic label off `std_dev`: `< 8` low, `8..=25` medium,
/// `> 25` high. Streak/among-spins fields are `null` in the degenerate cases
/// their closed forms are undefined (e.g. `worst_zero_streak` when a mode never
/// pays zero).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct ModeAnalysis {
    pub mode: String,
    pub cost: f64,
    pub rtp: f64,
    pub std_dev: f64,
    pub volatility: String,
    pub max_win: f64,
    pub min_win: f64,
    pub zero_prob: f64,
    pub sub_bet_prob: f64,
    pub win_prob: f64,
    pub break_even_miss_prob: f64,
    pub hit_rate: f64,
    /// Distinct payout values, including the zero (losing) payout.
    pub unique_payouts: u64,
    pub entries: u64,
    /// `1 / P(m == max m)`, i.e. "1 in N" odds of hitting the top multiplier.
    pub max_win_odds: f64,
    pub avg_spins_any_win: Option<f64>,
    pub worst_zero_streak: Option<u64>,
    pub avg_spins_profit: Option<f64>,
    pub worst_loss_streak: Option<u64>,
    pub tail_prob_5000: f64,
    pub tail_prob_10000: f64,
    pub cvar: f64,
    pub etl_40: f64,
    pub etl_10000: f64,
    pub etl_sum: f64,
    pub distribution: Vec<DistBucket>,
    pub compliance: Vec<ComplianceCheck>,
}

/// One bucket of a mode's win-multiplier distribution. Membership is
/// `from < m <= to` (the first bucket is `0 < m <= 0.1`; the last, open bucket
/// has `to == null`). The zero payout is excluded entirely (it is `zero_prob`).
/// `count` is the number of distinct payout values in the range.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct DistBucket {
    pub from: f64,
    pub to: Option<f64>,
    pub count: u64,
    pub probability: f64,
    /// `1 / probability` ("1 in N"), or `null` when the bucket is empty.
    pub effective_hit_rate: Option<f64>,
    /// `Σ p·x` over the bucket — the bucket's absolute share of RTP.
    pub rtp_contribution: f64,
}

/// One per-mode compliance check. `expected`/`result` are human-formatted
/// strings for direct display; `pass` is the machine verdict.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct ComplianceCheck {
    pub check: String,
    pub label: String,
    pub expected: String,
    pub result: String,
    pub pass: bool,
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

/// Result of a content-lifecycle deletion (a revision or a front bundle): how
/// much storage the blob GC actually reclaimed. `freed_blobs` counts the blob
/// rows removed; `freed_bytes` sums their sizes. Both are 0 when every referenced
/// blob is still shared by another revision or bundle.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct DeletionResult {
    pub freed_bytes: i64,
    pub freed_blobs: i64,
}

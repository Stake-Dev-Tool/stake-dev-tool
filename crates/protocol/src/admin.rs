//! Instance-admin wire types: the platform-operator surface (global stats,
//! workspace plan overrides, user management, share moderation). Re-exported at
//! the crate root (`protocol::AdminOverview`), and also reachable namespaced
//! (`protocol::admin::AdminOverview`).
//!
//! Numeric convention (as elsewhere in the crate): `u32` counts render as TS
//! `number`, while `i64`/`u64` totals render as TS `bigint`. Every count here
//! comes straight from a SQL `count(*)`/`SUM(...)`, so it is `i64` → `bigint`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// `GET /api/admin/me` response, and the body of `PUT /api/admin/users/:id/admin`.
/// The nav link in the dashboard is gated on a successful `{is_admin: true}` here
/// (every non-admin gets a 404 from the endpoint instead, so the surface stays
/// hidden).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct AdminMe {
    pub is_admin: bool,
}

/// One day's bucket in a 30-day series (`signups_30d`, `pushes_30d`). `date` is a
/// Host machine capacity for the admin overview (scale planning): the disk
/// backing the blob storage and the box's memory.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct HostStats {
    pub disk_total_bytes: u64,
    pub disk_free_bytes: u64,
    pub mem_total_bytes: u64,
    pub mem_used_bytes: u64,
}

/// `YYYY-MM-DD` calendar day; empty days are present with `count: 0`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct DayCount {
    pub date: String,
    pub count: i64,
}

/// `GET /api/admin/overview` — cheap instance-wide totals plus two 30-day series.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct AdminOverview {
    pub users: i64,
    pub workspaces: i64,
    pub games: i64,
    pub revisions: i64,
    pub share_links: i64,
    /// `SUM(blobs.size)` across every workspace (deduplicated per workspace).
    pub storage_bytes: i64,
    /// Lifetime visitor sessions summed over every share link's counter.
    pub sessions_total: i64,
    /// Lifetime spins summed over every share link's counter.
    pub spins_total: i64,
    /// Host machine capacity, for scale planning. `None` when the probe fails
    /// (unsupported platform, restricted container) — the UI hides the card.
    #[serde(default)]
    pub host: Option<HostStats>,
    /// Per-day new-account counts over the last 30 days (from `users.created_at`).
    pub signups_30d: Vec<DayCount>,
    /// Per-day revision-push counts over the last 30 days (`revisions.created_at`).
    pub pushes_30d: Vec<DayCount>,
}

/// A manual plan override ("comp subscription") on a workspace, as stored. Shown
/// on the admin workspace list regardless of expiry; `plan_for` ignores it once
/// `expires_at` has passed.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct AdminOverride {
    pub plan: String,
    /// The comped seat count when `plan == "paid"`; `null` for `"unlimited"`.
    pub seats: Option<u32>,
    pub expires_at: Option<DateTime<Utc>>,
    pub note: Option<String>,
}

/// A workspace row on the admin workspaces list. `plan` is the RESOLVED label
/// (what `plan_for` returns, i.e. including any active override's effect), while
/// `override` echoes the raw stored override row (or `null`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct AdminWorkspace {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub members: i64,
    pub games: i64,
    pub storage_bytes: i64,
    /// Resolved plan label: `"unlimited"` (billing disabled), `"free"`, or
    /// `"paid"`.
    pub plan: String,
    /// The resolved seat count when the plan is `"paid"` (from the comp override
    /// or the subscription); `null` otherwise.
    pub seats: Option<u32>,
    #[serde(rename = "override")]
    #[ts(rename = "override")]
    pub plan_override: Option<AdminOverride>,
    /// The Stripe subscription status verbatim (`"active"`, `"past_due"`, …), or
    /// `null` when there is no subscription row.
    pub subscription_status: Option<String>,
}

/// `PUT /api/admin/workspaces/:id/override` request body. `plan: null` deletes the
/// override row; a value upserts it. `expires_in_days` (optional) sets an expiry
/// that many days out; `note` (optional) is free-form provenance.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct SetOverrideRequest {
    /// `"paid"`, `"unlimited"`, or `null` (which clears the override).
    pub plan: Option<String>,
    /// Required when `plan == "paid"` (the comped seat count, `1..=100`); ignored
    /// for `"unlimited"`.
    #[serde(default)]
    pub seats: Option<u32>,
    #[serde(default)]
    pub expires_in_days: Option<i64>,
    #[serde(default)]
    pub note: Option<String>,
}

/// `GET /api/admin/workspaces` response wrapper.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct AdminWorkspacesResponse {
    pub workspaces: Vec<AdminWorkspace>,
}

/// A user row on the admin users list.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct AdminUserRow {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub created_at: DateTime<Utc>,
    pub is_admin: bool,
    /// Number of workspaces this user is a member of.
    pub workspaces: i64,
}

/// `GET /api/admin/users` response wrapper.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct AdminUsersResponse {
    pub users: Vec<AdminUserRow>,
}

/// `PUT /api/admin/users/:id/admin` request body.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct SetAdminRequest {
    pub is_admin: bool,
}

/// A share link on the admin (cross-workspace) moderation list.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct AdminShare {
    pub id: Uuid,
    pub slug: String,
    /// The public play URL (custom domain preferred, else the platform play
    /// domain), or `null` when no play domain is configured on this instance.
    pub url: Option<String>,
    pub workspace_slug: String,
    pub game: String,
    pub sessions_count: i64,
    pub spins_count: i64,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// `GET /api/admin/shares` response wrapper.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct AdminSharesResponse {
    pub shares: Vec<AdminShare>,
}

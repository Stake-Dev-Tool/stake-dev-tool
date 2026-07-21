//! M7 billing wire types: plan selection, checkout initiation, and the
//! subscription-status view the dashboard renders. Contract: docs/v2/m7-contract.md.
//!
//! These types are namespaced (`protocol::billing::…`) rather than re-exported at
//! the crate root, matching the M5 `shares` module. Numeric byte sizes follow the
//! crate convention: `u32` counts render as TS `number`, `u64`/`i64` byte totals
//! render as TS `bigint`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A purchasable plan. Serialized lowercase to match the `subscriptions.plan`
/// `CHECK` constraint and the Polar product mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "protocol/")]
pub enum PlanId {
    Solo,
    Team,
}

/// Billing cadence chosen at checkout. Serialized lowercase to match the
/// `subscriptions."interval"` `CHECK` constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "protocol/")]
pub enum BillingInterval {
    Monthly,
    Yearly,
}

/// `POST /workspaces/:slug/billing/checkout` request body.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct CheckoutRequest {
    pub plan: PlanId,
    pub interval: BillingInterval,
}

/// `POST /workspaces/:slug/billing/checkout` response: the hosted Polar checkout
/// URL the browser is redirected to.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct CheckoutResponse {
    pub checkout_url: String,
}

/// Effective quota limits for a workspace. `null` on any field means unlimited
/// (self-hosted instances with billing disabled report every field `null`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct BillingLimits {
    pub max_members: Option<u32>,
    pub max_storage_bytes: Option<u64>,
    pub max_active_share_links: Option<u32>,
    pub max_concurrent_share_sessions: Option<u32>,
}

/// Current resource usage, counted cheaply for the billing page. `storage_bytes`
/// is `SUM(blobs.size)` over the workspace (deduplicated), cached in-process for
/// 60s; `active_share_links` is 0 on instances where the M5 share tables are not
/// yet present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct BillingUsage {
    pub members: i64,
    pub storage_bytes: i64,
    pub active_share_links: i64,
}

/// `GET /workspaces/:slug/billing` response.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct BillingStatusResponse {
    /// Whether Polar billing is configured on this instance. `false` → quotas are
    /// never enforced and every limit is unlimited.
    pub enabled: bool,
    /// The resolved plan label: `"trial"`, `"solo"`, `"team"`, `"unlimited"`
    /// (billing disabled), or `"expired"` (trial lapsed, unpaid — reads still
    /// work, writes are blocked with `upgrade_required`).
    pub plan: String,
    /// The subscription status verbatim from Polar (e.g. `"active"`, `"trialing"`,
    /// `"past_due"`, `"canceled"`), or `null` when there is no subscription.
    pub status: Option<String>,
    /// The subscription's billing interval, or `null` when there is no subscription.
    pub interval: Option<BillingInterval>,
    /// The current billing period's end (subscription) or, on the free trial, when
    /// the trial expires. `null` when neither applies.
    pub current_period_end: Option<DateTime<Utc>>,
    pub usage: BillingUsage,
    pub limits: BillingLimits,
}

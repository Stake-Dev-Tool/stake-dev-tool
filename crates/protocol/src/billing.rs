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

/// Billing cadence chosen at checkout. Serialized lowercase to match the
/// `subscriptions."interval"` `CHECK` constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "protocol/")]
pub enum BillingInterval {
    Monthly,
    Yearly,
}

/// `POST /workspaces/:slug/billing/checkout` request body. The single paid plan
/// is billed per seat (Stripe graduated tiers: €3 first seat, €2 each additional);
/// `seats` becomes the subscription quantity. Bounded `1..=100` server-side.
///
/// `storage_units` optionally bundles the storage add-on into the SAME checkout
/// (one unit = +10 GiB for €1/mo, appended as a second line item). Omitted/`0`
/// means seats-only; bounded `0..=100` server-side.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct CheckoutRequest {
    pub interval: BillingInterval,
    pub seats: u32,
    #[serde(default)]
    pub storage_units: u32,
}

/// `POST /workspaces/:slug/billing/seats` request body: the new seat count for an
/// already-subscribed workspace. `seats` (bounded `1..=100` server-side, and never
/// below the current member count) becomes the subscription's seat line-item
/// quantity; Stripe prorates the change (`proration_behavior=create_prorations`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct UpdateSeatsRequest {
    pub seats: u32,
}

/// `POST /workspaces/:slug/billing/checkout` response: the hosted Stripe checkout
/// URL the browser is redirected to.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct CheckoutResponse {
    pub checkout_url: String,
}

/// `POST /workspaces/:slug/billing/portal` response: the Stripe Customer Portal
/// session URL the browser is redirected to. The portal is Stripe-hosted and
/// short-lived (generated per request); it lets a subscriber update their payment
/// method, view invoices, or cancel the subscription.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct PortalResponse {
    pub url: String,
}

/// `POST /workspaces/:slug/billing/storage` request body: how many storage
/// add-on units to purchase, each granting +10 GiB for €1/mo. Bounded `1..=100`
/// server-side (a single Stripe checkout with `quantity = units`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "protocol/")]
pub struct StorageCheckoutRequest {
    pub units: i64,
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
    /// Whether Stripe billing is configured on this instance. `false` → quotas are
    /// never enforced and every limit is unlimited.
    pub enabled: bool,
    /// The resolved plan label: `"free"` (billing enabled, no active subscription
    /// — reads still work, writes are blocked with `upgrade_required`), `"paid"`
    /// (an active seat subscription or comp), or `"unlimited"` (billing disabled,
    /// self-host).
    pub plan: String,
    /// The seat count backing a `"paid"` plan (the subscription quantity or the
    /// comp's seat count). `null` when the plan is not `"paid"`.
    pub seats: Option<u32>,
    /// The subscription status verbatim from Stripe (e.g. `"active"`, `"trialing"`,
    /// `"past_due"`, `"canceled"`), or `null` when there is no subscription. A
    /// storage-only subscription (no plan) reports the sentinel `"storage_only"`.
    pub status: Option<String>,
    /// The subscription's billing interval, or `null` when there is no subscription.
    pub interval: Option<BillingInterval>,
    /// The current billing period's end, or `null` when there is no subscription.
    pub current_period_end: Option<DateTime<Utc>>,
    /// Whether the subscription is scheduled to cancel at the end of the current
    /// period (Stripe's `cancel_at_period_end`) while its `status` stays `active`.
    /// `false` when there is no subscription or it is not scheduled to cancel; the
    /// UI surfaces a calm "your plan ends on `current_period_end`" notice when
    /// `true`.
    pub cancel_at_period_end: bool,
    /// Extra storage granted by the add-on, in GiB (`extra_storage_units × 10`).
    /// `0` when no storage add-on is active. Already folded into
    /// `limits.max_storage_bytes`; surfaced separately so the UI can show it.
    pub extra_storage_gib: i64,
    pub usage: BillingUsage,
    pub limits: BillingLimits,
}

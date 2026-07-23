//! M7 — plans, quotas, and the Stripe billing integration.
//! Contract: docs/v2/m7-contract.md.
//!
//! INTERFACE: [`PlanLimits`] and [`limits_for`] are the surface other modules
//! (invites, math uploads, the M5 share module) call for quota checks. Every
//! plan can write — Free is a usable solo tier bounded by its limits, not a
//! read-only lock. When Stripe is not configured, every workspace resolves to
//! [`PlanLimits::UNLIMITED`] — the permanent behavior on self-hosted instances.
//!
//! Submodules:
//! - [`plan`] — the limits table and plan resolution (`plan_for`).
//! - [`stripe`] — the Stripe REST client (checkout) and price-id ↔ plan mapping.
//! - [`webhook`] — Stripe-Signature verification (hand-rolled HMAC-SHA256) and
//!   event handling.

pub mod plan;
pub mod stripe;
pub mod webhook;

use uuid::Uuid;

use crate::AppState;
use crate::error::ApiResult;

pub use plan::{Plan, plan_for};

/// One storage add-on unit grants this many bytes (+10 GiB per unit).
const STORAGE_UNIT_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// Effective limits for a workspace. `None` = unlimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanLimits {
    pub max_members: Option<u32>,
    pub max_storage_bytes: Option<u64>,
    pub max_active_share_links: Option<u32>,
    pub max_concurrent_share_sessions: Option<u32>,
    /// Math revisions kept per game — each push past the cap prunes the oldest
    /// in the same transaction (Free keeps 1: every push replaces the previous).
    pub max_revisions_per_game: Option<u32>,
    /// Front bundles kept per game, pruned the same way on each front push.
    pub max_front_bundles_per_game: Option<u32>,
    /// Longest lifetime of a share link, in days from creation. Enforced at
    /// create/update time (the expiry is forced ≤ the cap) AND lazily at serve
    /// time against `created_at`, so links minted on a since-lapsed paid plan
    /// wind down too.
    pub max_share_link_days: Option<u32>,
}

impl PlanLimits {
    pub const UNLIMITED: PlanLimits = PlanLimits {
        max_members: None,
        max_storage_bytes: None,
        max_active_share_links: None,
        max_concurrent_share_sessions: None,
        max_revisions_per_game: None,
        max_front_bundles_per_game: None,
        max_share_link_days: None,
    };

    /// Adds the storage add-on (`units × 10 GiB`) to the storage cap. An unlimited
    /// cap (`None`) stays unlimited; a non-positive unit count is a no-op.
    pub fn with_extra_storage_units(mut self, units: i64) -> Self {
        if units > 0
            && let Some(cap) = self.max_storage_bytes
        {
            let extra = (units as u64).saturating_mul(STORAGE_UNIT_BYTES);
            self.max_storage_bytes = Some(cap.saturating_add(extra));
        }
        self
    }
}

/// Resolves the limits that currently apply to a workspace. Returns
/// [`PlanLimits::UNLIMITED`] whenever Stripe billing is not configured; otherwise
/// the limits of the workspace's resolved [`Plan`] (see [`plan_for`]) with any
/// active storage add-on folded into the storage cap.
pub async fn limits_for(state: &AppState, workspace_id: Uuid) -> ApiResult<PlanLimits> {
    let limits = plan_for(state, workspace_id).await?.limits();
    // Billing off → unlimited already; skip the storage lookup entirely.
    if state.config.stripe.is_none() {
        return Ok(limits);
    }
    let units = plan::extra_storage_units(&state.pool, workspace_id).await?;
    Ok(limits.with_extra_storage_units(units))
}

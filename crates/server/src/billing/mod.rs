//! M7 — plans, quotas, and the Stripe billing integration.
//! Contract: docs/v2/m7-contract.md.
//!
//! INTERFACE FREEZE: [`PlanLimits`] and [`limits_for`] are the surface other
//! modules (invites, math uploads, the M5 share module) call for quota checks.
//! Their shape is unchanged; the M7 work only fills in the resolution behind
//! them. When Stripe is not configured, every workspace still resolves to
//! [`PlanLimits::UNLIMITED`] — the permanent behavior on self-hosted instances.
//!
//! Submodules:
//! - [`plan`] — the limits table and plan resolution (`plan_for`, `write_allowed`).
//! - [`stripe`] — the Stripe REST client (checkout) and price-id ↔ plan mapping.
//! - [`webhook`] — Stripe-Signature verification (hand-rolled HMAC-SHA256) and
//!   event handling.

pub mod plan;
pub mod stripe;
pub mod webhook;

use uuid::Uuid;

use crate::AppState;
use crate::error::ApiResult;

pub use plan::{Plan, plan_for, write_allowed};

/// One storage add-on unit grants this many bytes (+10 GiB per unit).
const STORAGE_UNIT_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// Effective limits for a workspace. `None` = unlimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanLimits {
    pub max_members: Option<u32>,
    pub max_storage_bytes: Option<u64>,
    pub max_active_share_links: Option<u32>,
    pub max_concurrent_share_sessions: Option<u32>,
}

impl PlanLimits {
    pub const UNLIMITED: PlanLimits = PlanLimits {
        max_members: None,
        max_storage_bytes: None,
        max_active_share_links: None,
        max_concurrent_share_sessions: None,
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

//! M7 — plans, quotas, and the Polar billing integration.
//! Contract: docs/v2/m7-contract.md.
//!
//! INTERFACE FREEZE: [`PlanLimits`] and [`limits_for`] are the surface other
//! modules (invites, math uploads, the M5 share module) call for quota checks.
//! Their shape is unchanged; the M7 work only fills in the resolution behind
//! them. When Polar is not configured, every workspace still resolves to
//! [`PlanLimits::UNLIMITED`] — the permanent behavior on self-hosted instances.
//!
//! Submodules:
//! - [`plan`] — the limits table and plan resolution (`plan_for`, `write_allowed`).
//! - [`polar`] — the Polar REST client (checkout) and product-id ↔ plan mapping.
//! - [`webhook`] — Standard-Webhooks signature verification (hand-rolled
//!   HMAC-SHA256) and event handling.

pub mod plan;
pub mod polar;
pub mod webhook;

use uuid::Uuid;

use crate::AppState;
use crate::error::ApiResult;

pub use plan::{Plan, plan_for, write_allowed};

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
}

/// Resolves the limits that currently apply to a workspace. Returns
/// [`PlanLimits::UNLIMITED`] whenever Polar billing is not configured; otherwise
/// the limits of the workspace's resolved [`Plan`] (see [`plan_for`]).
pub async fn limits_for(state: &AppState, workspace_id: Uuid) -> ApiResult<PlanLimits> {
    Ok(plan_for(state, workspace_id).await?.limits())
}

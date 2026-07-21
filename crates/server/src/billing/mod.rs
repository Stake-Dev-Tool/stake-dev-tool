//! M7 — plans, quotas, and the Polar billing integration.
//! Contract: docs/v2/m7-contract.md.
//!
//! INTERFACE FREEZE: [`PlanLimits`] and [`limits_for`] are the surface other
//! modules (invites, math uploads, the M5 share module) call for quota checks.
//! Until the Polar integration lands, every workspace resolves to
//! [`PlanLimits::UNLIMITED`], which is also the permanent behavior on
//! self-hosted instances with billing disabled.

use uuid::Uuid;

use crate::AppState;
use crate::error::ApiResult;

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

/// Resolves the limits that currently apply to a workspace. Billing lands in
/// M7; until then (and forever on instances without Polar configured) this is
/// [`PlanLimits::UNLIMITED`].
pub async fn limits_for(_state: &AppState, _workspace_id: Uuid) -> ApiResult<PlanLimits> {
    Ok(PlanLimits::UNLIMITED)
}

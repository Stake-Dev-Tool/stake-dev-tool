//! M7 — billing endpoints: checkout initiation, subscription status, and the
//! Polar webhook. Contract: docs/v2/m7-contract.md.
//!
//! Checkout and the webhook 404 when Polar is not configured (the GitHub-routes
//! pattern); the status endpoint is always reachable by members and simply
//! reports `enabled: false` with unlimited limits on a self-hosted instance.

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use chrono::Duration;
use protocol::Role;
use protocol::billing::{
    BillingLimits, BillingStatusResponse, BillingUsage, CheckoutRequest, CheckoutResponse,
};

use crate::AppState;
use crate::api::workspaces::{require_membership, workspace_by_slug};
use crate::auth::extract::CurrentUser;
use crate::billing::plan::{self, Plan};
use crate::billing::polar;
use crate::error::{ApiError, ApiResult};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/workspaces/:slug/billing", get(status))
        .route("/workspaces/:slug/billing/checkout", post(checkout))
        .route("/billing/webhook", post(crate::billing::webhook::handle))
}

/// Starts a Polar checkout for the workspace (owner-only, billing-enabled-only).
/// The checkout carries `metadata.workspace_id` so the webhook can bind the
/// resulting subscription without ever guessing from an email.
async fn checkout(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
    Json(req): Json<CheckoutRequest>,
) -> ApiResult<Json<CheckoutResponse>> {
    let Some(polar_cfg) = state.config.polar.as_ref() else {
        return Err(ApiError::not_found("not_found", "billing is not enabled"));
    };
    let workspace = workspace_by_slug(&state.pool, &slug).await?;
    let role = require_membership(&state.pool, workspace.id, user.user_id).await?;
    if role != Role::Owner {
        return Err(ApiError::forbidden(
            "forbidden",
            "only the workspace owner can start checkout",
        ));
    }

    let product_id = polar::product_id_for(polar_cfg, req.plan, req.interval);
    let success_url = format!(
        "{}/w/{}?upgraded=1",
        state.config.public_base_url(),
        workspace.slug
    );
    let checkout_url = polar::create_checkout(
        &state.http_client,
        polar_cfg,
        product_id,
        workspace.id,
        &success_url,
    )
    .await?;
    Ok(Json(CheckoutResponse { checkout_url }))
}

/// The workspace's plan, subscription status, usage, and limits (member-only).
async fn status(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
) -> ApiResult<Json<BillingStatusResponse>> {
    let workspace = workspace_by_slug(&state.pool, &slug).await?;
    require_membership(&state.pool, workspace.id, user.user_id).await?;

    let enabled = state.config.polar.is_some();
    let resolved = plan::plan_for(&state, workspace.id).await?;
    let subscription = plan::load_subscription(&state.pool, workspace.id).await?;

    let usage = BillingUsage {
        members: plan::member_count(&state.pool, workspace.id).await?,
        storage_bytes: plan::storage_bytes_cached(&state.pool, workspace.id).await?,
        active_share_links: plan::active_share_links(&state.pool, workspace.id).await?,
    };

    let (status, interval, current_period_end) = match &subscription {
        Some(sub) => (
            Some(sub.status.clone()),
            sub.interval_enum(),
            sub.current_period_end,
        ),
        // No subscription: on the trial, surface when it ends.
        None => {
            let trial_end = (enabled && matches!(resolved, Plan::Trial | Plan::Expired))
                .then(|| workspace.created_at + Duration::days(plan::TRIAL_DAYS));
            (None, None, trial_end)
        }
    };

    Ok(Json(BillingStatusResponse {
        enabled,
        plan: resolved.label().to_string(),
        status,
        interval,
        current_period_end,
        usage,
        limits: billing_limits(resolved.limits()),
    }))
}

fn billing_limits(limits: crate::billing::PlanLimits) -> BillingLimits {
    BillingLimits {
        max_members: limits.max_members,
        max_storage_bytes: limits.max_storage_bytes,
        max_active_share_links: limits.max_active_share_links,
        max_concurrent_share_sessions: limits.max_concurrent_share_sessions,
    }
}

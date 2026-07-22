//! M7 — billing endpoints: checkout initiation, the storage add-on checkout,
//! subscription status, and the Stripe webhook. Contract: docs/v2/m7-contract.md.
//!
//! Checkout and the webhook 404 when Stripe is not configured (the GitHub-routes
//! pattern); the status endpoint is always reachable by members and simply
//! reports `enabled: false` with unlimited limits on a self-hosted instance.

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use protocol::Role;
use protocol::billing::{
    BillingLimits, BillingStatusResponse, BillingUsage, CheckoutRequest, CheckoutResponse,
    StorageCheckoutRequest,
};

use crate::AppState;
use crate::api::workspaces::{WorkspaceRow, require_membership, workspace_by_slug};
use crate::auth::extract::CurrentUser;
use crate::billing::plan;
use crate::billing::stripe;
use crate::config::StripeConfig;
use crate::error::{ApiError, ApiResult};

/// Inclusive bounds on a single storage add-on purchase (units of +10 GiB).
const MIN_STORAGE_UNITS: i64 = 1;
const MAX_STORAGE_UNITS: i64 = 100;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/workspaces/:slug/billing", get(status))
        .route("/workspaces/:slug/billing/checkout", post(checkout))
        .route("/workspaces/:slug/billing/storage", post(buy_storage))
        .route("/billing/webhook", post(crate::billing::webhook::handle))
}

/// Resolves billing config + the owner-gated workspace for a checkout call. 404s
/// when billing is disabled; 403 when the caller is not the workspace owner.
async fn owner_checkout_context<'a>(
    state: &'a AppState,
    user: &CurrentUser,
    slug: &str,
) -> ApiResult<(&'a StripeConfig, WorkspaceRow)> {
    let Some(stripe_cfg) = state.config.stripe.as_ref() else {
        return Err(ApiError::not_found("not_found", "billing is not enabled"));
    };
    let workspace = workspace_by_slug(&state.pool, slug).await?;
    let role = require_membership(&state.pool, workspace.id, user.user_id).await?;
    if role != Role::Owner {
        return Err(ApiError::forbidden(
            "forbidden",
            "only the workspace owner can start checkout",
        ));
    }
    Ok((stripe_cfg, workspace))
}

/// The success URL the browser returns to after a checkout completes (carries the
/// `?upgraded=1` flag the billing page reacts to). The cancel URL is derived from
/// it by `stripe::create_checkout` (the same URL minus the flag).
fn success_url(state: &AppState, workspace: &WorkspaceRow) -> String {
    format!(
        "{}/w/{}?upgraded=1",
        state.config.public_base_url(),
        workspace.slug
    )
}

/// Starts a Stripe checkout for the workspace's plan (owner-only,
/// billing-enabled-only). The checkout carries `metadata.workspace_id` so the
/// webhook can bind the resulting subscription without ever guessing from an email.
async fn checkout(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
    Json(req): Json<CheckoutRequest>,
) -> ApiResult<Json<CheckoutResponse>> {
    let (stripe_cfg, workspace) = owner_checkout_context(&state, &user, &slug).await?;

    let price_id = stripe::price_id_for(stripe_cfg, req.plan, req.interval);
    let success_url = success_url(&state, &workspace);
    let checkout_url = stripe::create_checkout(
        &state.http_client,
        stripe_cfg,
        price_id,
        workspace.id,
        &success_url,
        1,
    )
    .await?;
    Ok(Json(CheckoutResponse { checkout_url }))
}

/// Starts a Stripe checkout for the storage add-on (owner-only,
/// billing-enabled-only). `units` (1..=100) becomes the line-item quantity, each
/// unit granting +10 GiB for €1/mo. The resulting subscription carries only the
/// storage price; the webhook folds its quantity into `extra_storage_units`.
async fn buy_storage(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
    Json(req): Json<StorageCheckoutRequest>,
) -> ApiResult<Json<CheckoutResponse>> {
    if req.units < MIN_STORAGE_UNITS || req.units > MAX_STORAGE_UNITS {
        return Err(ApiError::bad_request(
            "invalid_units",
            format!("units must be between {MIN_STORAGE_UNITS} and {MAX_STORAGE_UNITS}"),
        ));
    }
    let (stripe_cfg, workspace) = owner_checkout_context(&state, &user, &slug).await?;

    let success_url = success_url(&state, &workspace);
    let checkout_url = stripe::create_checkout(
        &state.http_client,
        stripe_cfg,
        &stripe_cfg.price_storage,
        workspace.id,
        &success_url,
        req.units,
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

    let enabled = state.config.stripe.is_some();
    let resolved = plan::plan_for(&state, workspace.id).await?;
    let subscription = plan::load_subscription(&state.pool, workspace.id).await?;

    let usage = BillingUsage {
        members: plan::member_count(&state.pool, workspace.id).await?,
        storage_bytes: plan::storage_bytes_cached(&state.pool, workspace.id).await?,
        active_share_links: plan::active_share_links(&state.pool, workspace.id).await?,
    };

    // The storage add-on (if any) is reflected both as a standalone GiB figure and
    // folded into the effective storage cap — matching what enforcement sees.
    let extra_units = subscription
        .as_ref()
        .map(|s| s.extra_storage_units)
        .unwrap_or(0);
    let extra_storage_gib = extra_units.max(0) * 10;
    let limits = if enabled {
        resolved.limits().with_extra_storage_units(extra_units)
    } else {
        resolved.limits()
    };

    let (status, interval, current_period_end) = match &subscription {
        Some(sub) => (
            Some(sub.status.clone()),
            sub.interval_enum(),
            sub.current_period_end,
        ),
        // No subscription: no plan, so no period end to surface.
        None => (None, None, None),
    };

    Ok(Json(BillingStatusResponse {
        enabled,
        plan: resolved.label().to_string(),
        status,
        interval,
        current_period_end,
        extra_storage_gib,
        usage,
        limits: billing_limits(limits),
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

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
use chrono::Utc;
use protocol::Role;
use protocol::billing::{
    BillingLimits, BillingStatusResponse, BillingUsage, CheckoutRequest, CheckoutResponse,
    PortalResponse, StorageCheckoutRequest, UpdateSeatsRequest,
};
use uuid::Uuid;

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

/// Inclusive bounds on the seat count at checkout (the subscription quantity).
const MIN_SEATS: u32 = 1;
const MAX_SEATS: u32 = 100;

/// A checkout may bundle `0..=100` storage add-on units alongside the seats (0 =
/// no storage line item). The standalone storage endpoint requires `>= 1`.
const MAX_CHECKOUT_STORAGE_UNITS: u32 = 100;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/workspaces/:slug/billing", get(status))
        .route("/workspaces/:slug/billing/checkout", post(checkout))
        .route("/workspaces/:slug/billing/seats", post(update_seats))
        .route("/workspaces/:slug/billing/storage", post(buy_storage))
        .route("/workspaces/:slug/billing/portal", post(portal))
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

/// Starts a Stripe checkout for the workspace's seat subscription (owner-only,
/// billing-enabled-only). `seats` (1..=100) becomes the seat line-item quantity;
/// Stripe's graduated tiers price it (€3 first seat + €2 each additional). An
/// optional `storage_units` (0..=100) bundles the storage add-on into the SAME
/// checkout as a second line item (one unit = +10 GiB for €1/mo), so a new
/// workspace can pick seats and storage together. The checkout carries
/// `metadata.workspace_id` so the webhook can bind the resulting subscription
/// without ever guessing from an email.
async fn checkout(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
    Json(req): Json<CheckoutRequest>,
) -> ApiResult<Json<CheckoutResponse>> {
    if req.seats < MIN_SEATS || req.seats > MAX_SEATS {
        return Err(ApiError::bad_request(
            "invalid_seats",
            format!("seats must be between {MIN_SEATS} and {MAX_SEATS}"),
        ));
    }
    if req.storage_units > MAX_CHECKOUT_STORAGE_UNITS {
        return Err(ApiError::bad_request(
            "invalid_units",
            format!("storage units must be between 0 and {MAX_CHECKOUT_STORAGE_UNITS}"),
        ));
    }
    let (stripe_cfg, workspace) = owner_checkout_context(&state, &user, &slug).await?;

    let price_id = stripe::seat_price_id(stripe_cfg, req.interval);
    // Seats are always present; the storage add-on is appended only when > 0.
    let mut line_items: Vec<(&str, i64)> = vec![(price_id, i64::from(req.seats))];
    if req.storage_units > 0 {
        line_items.push((
            stripe_cfg.price_storage.as_str(),
            i64::from(req.storage_units),
        ));
    }
    let success_url = success_url(&state, &workspace);
    let checkout_url = stripe::create_checkout(
        &state.http_client,
        stripe_cfg,
        &line_items,
        workspace.id,
        &success_url,
    )
    .await?;
    Ok(Json(CheckoutResponse { checkout_url }))
}

/// Changes the seat count on an already-subscribed workspace (owner-only,
/// billing-enabled-only), via Stripe's standard proration. Validates `seats`
/// (1..=100 → `invalid_seats`), requires a live plan-granting subscription
/// (active/trialing, or `past_due` within grace → else `no_subscription`), and
/// refuses dropping below the current member count (`seats_below_members`). On
/// success it fetches the subscription's seat line item from Stripe, updates its
/// quantity with `proration_behavior=create_prorations`, optimistically writes the
/// new seat count to the DB (the webhook confirms it), and returns the fresh
/// billing status (same shape as `GET /billing`).
async fn update_seats(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
    Json(req): Json<UpdateSeatsRequest>,
) -> ApiResult<Json<BillingStatusResponse>> {
    if req.seats < MIN_SEATS || req.seats > MAX_SEATS {
        return Err(ApiError::bad_request(
            "invalid_seats",
            format!("seats must be between {MIN_SEATS} and {MAX_SEATS}"),
        ));
    }
    let (stripe_cfg, workspace) = owner_checkout_context(&state, &user, &slug).await?;

    // Only a live plan subscription can have its seats changed. A workspace with
    // no subscription, a storage-only row, an override comp (no Stripe row to
    // touch), or a lapsed subscription is rejected cleanly.
    let subscription = plan::load_subscription(&state.pool, workspace.id).await?;
    let Some(sub) = subscription.filter(|s| s.grants_plan_now(Utc::now())) else {
        return Err(ApiError::conflict(
            "no_subscription",
            "this workspace has no active seat subscription to change",
        ));
    };

    // Lowering below the current headcount would strand members; refuse it.
    let members = plan::member_count(&state.pool, workspace.id).await?;
    if i64::from(req.seats) < members {
        return Err(ApiError::conflict(
            "seats_below_members",
            format!(
                "This workspace has {members} members — remove members first or keep at least {members} seats."
            ),
        ));
    }

    // Locate the seat line item on the Stripe subscription and set its quantity
    // with prorations. A subscription that somehow carries no seat price has
    // nothing to change → treat as no seat subscription.
    let seat_item = stripe::fetch_seat_item(
        &state.http_client,
        stripe_cfg,
        &sub.provider_subscription_id,
    )
    .await?
    .ok_or_else(|| {
        ApiError::conflict(
            "no_subscription",
            "this workspace has no active seat subscription to change",
        )
    })?;
    stripe::update_seat_quantity(
        &state.http_client,
        stripe_cfg,
        &sub.provider_subscription_id,
        &seat_item.id,
        i64::from(req.seats),
    )
    .await?;

    // Optimistically reflect the new seat count; the webhook re-confirms it.
    sqlx::query("UPDATE subscriptions SET seats = $2, updated_at = now() WHERE workspace_id = $1")
        .bind(workspace.id)
        .bind(i64::from(req.seats))
        .execute(&state.pool)
        .await?;

    Ok(Json(billing_status_payload(&state, workspace.id).await?))
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
        &[(stripe_cfg.price_storage.as_str(), req.units)],
        workspace.id,
        &success_url,
    )
    .await?;
    Ok(Json(CheckoutResponse { checkout_url }))
}

/// Opens a Stripe Customer Portal session for the workspace's subscription
/// (owner-only, billing-enabled-only). Requires a subscription row carrying a
/// Stripe customer id (else 409 `no_subscription`) — the customer id is captured
/// by the webhook when the subscription is created. Returns `{ url }`, a
/// short-lived Stripe-hosted page where the customer can update their payment
/// method, view invoices, or cancel; the portal returns them to the workspace's
/// billing page. The account's default portal configuration is used unless
/// `STRIPE_PORTAL_CONFIGURATION` is set.
async fn portal(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
) -> ApiResult<Json<PortalResponse>> {
    let (stripe_cfg, workspace) = owner_checkout_context(&state, &user, &slug).await?;

    // A portal session needs the Stripe customer id. A workspace with no
    // subscription, or a comp override (no Stripe row at all), has nothing to
    // manage — reject cleanly.
    let customer_id = plan::load_subscription(&state.pool, workspace.id)
        .await?
        .and_then(|sub| sub.provider_customer_id)
        .ok_or_else(|| {
            ApiError::conflict(
                "no_subscription",
                "this workspace has no subscription to manage",
            )
        })?;

    let return_url = format!(
        "{}/w/{}/billing",
        state.config.public_base_url(),
        workspace.slug
    );
    let url = stripe::create_portal_session(
        &state.http_client,
        stripe_cfg,
        &customer_id,
        &return_url,
        stripe_cfg.portal_configuration.as_deref(),
    )
    .await?;
    Ok(Json(PortalResponse { url }))
}

/// The workspace's plan, subscription status, usage, and limits (member-only).
async fn status(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
) -> ApiResult<Json<BillingStatusResponse>> {
    let workspace = workspace_by_slug(&state.pool, &slug).await?;
    require_membership(&state.pool, workspace.id, user.user_id).await?;
    Ok(Json(billing_status_payload(&state, workspace.id).await?))
}

/// Builds the `GET /billing` payload for a workspace: resolved plan, subscription
/// status/interval/period, usage counters, storage add-on, and effective limits.
/// Shared by the status endpoint and the seat-change endpoint (which returns the
/// fresh status after updating the subscription).
async fn billing_status_payload(
    state: &AppState,
    workspace_id: Uuid,
) -> ApiResult<BillingStatusResponse> {
    let enabled = state.config.stripe.is_some();
    let resolved = plan::plan_for(state, workspace_id).await?;
    let subscription = plan::load_subscription(&state.pool, workspace_id).await?;

    let usage = BillingUsage {
        members: plan::member_count(&state.pool, workspace_id).await?,
        storage_bytes: plan::storage_bytes_cached(&state.pool, workspace_id).await?,
        active_share_links: plan::active_share_links(&state.pool, workspace_id).await?,
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
    // Scheduled cancellation is meaningful only while a subscription exists.
    let cancel_at_period_end = subscription
        .as_ref()
        .map(|sub| sub.cancel_at_period_end)
        .unwrap_or(false);

    Ok(BillingStatusResponse {
        enabled,
        plan: resolved.label().to_string(),
        seats: resolved.seats(),
        status,
        interval,
        current_period_end,
        cancel_at_period_end,
        extra_storage_gib,
        usage,
        limits: billing_limits(limits),
    })
}

fn billing_limits(limits: crate::billing::PlanLimits) -> BillingLimits {
    BillingLimits {
        max_members: limits.max_members,
        max_storage_bytes: limits.max_storage_bytes,
        max_active_share_links: limits.max_active_share_links,
        max_concurrent_share_sessions: limits.max_concurrent_share_sessions,
    }
}

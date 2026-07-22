//! Plan limits (the single source of truth for quotas) and plan resolution.
//!
//! `plan_for` maps a workspace to its effective [`Plan`]; [`Plan::limits`] turns
//! that into the frozen [`PlanLimits`]. Writes on a `Free` (unsubscribed)
//! workspace are gated separately by [`write_allowed`] so the frozen limits
//! struct stays untouched (a `Free` workspace keeps read limits but cannot push).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use protocol::billing::BillingInterval;

use super::PlanLimits;
use crate::AppState;
use crate::error::{ApiError, ApiResult};

const GIB: u64 = 1024 * 1024 * 1024;
/// How long a `past_due` subscription keeps its plan past `current_period_end`.
pub const GRACE_DAYS: i64 = 7;
/// How long the per-workspace storage total is memoized for the usage endpoint.
const STORAGE_CACHE_TTL: StdDuration = StdDuration::from_secs(60);

/// The resolved plan for a workspace at a point in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// Billing disabled (self-host) — no limits, no gating.
    Unlimited,
    /// Billing enabled with no active subscription and no admin override: reads
    /// work, writes are blocked. The hosted default until a workspace subscribes.
    Free,
    Solo,
    Team,
}

impl Plan {
    /// The quota limits for this plan. `Free` keeps read limits — writes are
    /// refused by [`write_allowed`], not by the limits.
    pub fn limits(self) -> PlanLimits {
        match self {
            Plan::Unlimited => PlanLimits::UNLIMITED,
            Plan::Free => PlanLimits {
                max_members: Some(3),
                max_storage_bytes: Some(2 * GIB),
                max_active_share_links: Some(2),
                max_concurrent_share_sessions: Some(5),
            },
            Plan::Solo => PlanLimits {
                max_members: Some(1),
                max_storage_bytes: Some(10 * GIB),
                max_active_share_links: Some(5),
                max_concurrent_share_sessions: Some(5),
            },
            Plan::Team => PlanLimits {
                max_members: Some(10),
                max_storage_bytes: Some(50 * GIB),
                max_active_share_links: Some(25),
                max_concurrent_share_sessions: Some(25),
            },
        }
    }

    /// The label surfaced on the wire (`GET /billing`).
    pub fn label(self) -> &'static str {
        match self {
            Plan::Unlimited => "unlimited",
            Plan::Free => "free",
            Plan::Solo => "solo",
            Plan::Team => "team",
        }
    }

    /// Writes (pushes, new shares, invites) are allowed on every plan except `Free`.
    pub fn writes_allowed(self) -> bool {
        !matches!(self, Plan::Free)
    }
}

/// A workspace's subscription row, as stored by the webhook.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SubscriptionRow {
    pub provider_subscription_id: String,
    pub provider_customer_id: Option<String>,
    pub plan: String,
    pub interval: String,
    pub status: String,
    pub current_period_end: Option<DateTime<Utc>>,
    /// Storage add-on units (one unit = +10 GiB). `0` when no add-on is active.
    pub extra_storage_units: i64,
}

impl SubscriptionRow {
    /// The billing interval as the wire enum, or `None` if the stored value is
    /// somehow outside the `CHECK` set.
    pub fn interval_enum(&self) -> Option<BillingInterval> {
        match self.interval.as_str() {
            "monthly" => Some(BillingInterval::Monthly),
            "yearly" => Some(BillingInterval::Yearly),
            _ => None,
        }
    }
}

/// Loads the subscription for a workspace, if any.
pub async fn load_subscription(
    pool: &PgPool,
    workspace_id: Uuid,
) -> ApiResult<Option<SubscriptionRow>> {
    Ok(sqlx::query_as::<_, SubscriptionRow>(
        // `extra_storage_units` is an INTEGER column; cast to bigint so it decodes
        // into the row's `i64` (sqlx will not coerce int4 → i64 on its own).
        "SELECT provider_subscription_id, provider_customer_id, plan, \"interval\", status, \
                current_period_end, extra_storage_units::bigint AS extra_storage_units \
         FROM subscriptions WHERE workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_optional(pool)
    .await?)
}

/// The workspace's active storage add-on units (one unit = +10 GiB), or `0` when
/// there is no subscription row. Read on the enforcement path, so it queries the
/// column directly rather than materializing the whole [`SubscriptionRow`].
pub async fn extra_storage_units(pool: &PgPool, workspace_id: Uuid) -> ApiResult<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(extra_storage_units, 0)::bigint FROM subscriptions WHERE workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(0))
}

/// Resolves a workspace's effective plan. Billing disabled → `Unlimited` (the
/// self-host short-circuit, kept FIRST so overrides never touch a self-hosted
/// instance). Otherwise a non-expired instance-admin plan override wins next;
/// failing that, an active/trialing (or within-grace `past_due`) subscription
/// grants its plan; failing all of that, the workspace is `Free` (read-only).
pub async fn plan_for(state: &AppState, workspace_id: Uuid) -> ApiResult<Plan> {
    if state.config.stripe.is_none() {
        return Ok(Plan::Unlimited);
    }
    // An instance operator can comp a workspace a plan via `plan_overrides`; a
    // non-expired row is honored before any subscription is even loaded. Expired
    // rows are ignored lazily (no cleanup job).
    if let Some(plan) = active_override(&state.pool, workspace_id, Utc::now()).await? {
        return Ok(plan);
    }
    let subscription = load_subscription(&state.pool, workspace_id).await?;
    Ok(resolve_plan(subscription.as_ref(), Utc::now()))
}

/// The plan granted by a non-expired `plan_overrides` row for this workspace, or
/// `None` when there is no row or it has expired. `'unlimited'` → [`Plan::Unlimited`],
/// `'solo'`/`'team'` → their plans.
async fn active_override(
    pool: &PgPool,
    workspace_id: Uuid,
    now: DateTime<Utc>,
) -> ApiResult<Option<Plan>> {
    let row: Option<(String, Option<DateTime<Utc>>)> =
        sqlx::query_as("SELECT plan, expires_at FROM plan_overrides WHERE workspace_id = $1")
            .bind(workspace_id)
            .fetch_optional(pool)
            .await?;
    let Some((plan, expires_at)) = row else {
        return Ok(None);
    };
    // An expired override is dead weight — fall through to subscription resolution.
    if let Some(exp) = expires_at
        && exp <= now
    {
        return Ok(None);
    }
    Ok(match plan.as_str() {
        "unlimited" => Some(Plan::Unlimited),
        "solo" => Some(Plan::Solo),
        "team" => Some(Plan::Team),
        // A value outside the CHECK set can't occur; treat it as no override.
        _ => None,
    })
}

/// Pure resolution given a loaded subscription. A subscription that currently
/// grants a plan wins; failing that, the workspace is `Free` (read-only).
fn resolve_plan(subscription: Option<&SubscriptionRow>, now: DateTime<Utc>) -> Plan {
    if let Some(sub) = subscription
        && let Some(plan) = active_plan(sub, now)
    {
        return plan;
    }
    Plan::Free
}

/// The plan a subscription currently grants, or `None` if it grants nothing.
/// `storage_only` is the sentinel for a storage-add-on row with no plan (its
/// `plan` column is a placeholder `'solo'`), so it never grants a plan; likewise
/// Stripe's terminal statuses (canceled/unpaid/incomplete_expired/incomplete) and
/// a `past_due` past its 7-day grace fall through to the `Free` resolution.
fn active_plan(sub: &SubscriptionRow, now: DateTime<Utc>) -> Option<Plan> {
    // A storage-only row carries a placeholder plan; it must not grant a plan.
    if sub.status == "storage_only" {
        return None;
    }
    let plan = match sub.plan.as_str() {
        "solo" => Plan::Solo,
        "team" => Plan::Team,
        _ => return None,
    };
    match sub.status.as_str() {
        "active" | "trialing" => Some(plan),
        // Grace: keep the plan until 7 days past the period end.
        "past_due" => match sub.current_period_end {
            Some(end) if now < end + Duration::days(GRACE_DAYS) => Some(plan),
            _ => None,
        },
        _ => None,
    }
}

/// 403 `upgrade_required` when the workspace has no active plan (`Free`);
/// `Ok(())` on every other plan (including billing disabled).
pub async fn write_allowed(state: &AppState, workspace_id: Uuid) -> ApiResult<()> {
    if plan_for(state, workspace_id).await?.writes_allowed() {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "upgrade_required",
            "this workspace has no active plan; subscribe to make changes",
        ))
    }
}

// ---------------------------------------------------------------------------
// Usage counters (for the `GET /billing` endpoint and storage enforcement)
// ---------------------------------------------------------------------------

/// Current member count.
pub async fn member_count(pool: &PgPool, workspace_id: Uuid) -> ApiResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM memberships WHERE workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(pool)
            .await?,
    )
}

/// Live per-workspace stored-bytes total (`SUM(blobs.size)`, deduplicated). Used
/// by the upload/commit quota checks, which must never read stale data.
pub async fn storage_bytes_live(pool: &PgPool, workspace_id: Uuid) -> ApiResult<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(SUM(size), 0)::bigint FROM blobs WHERE workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_one(pool)
    .await?)
}

/// In-process cache of the storage total, one entry per workspace.
static STORAGE_CACHE: LazyLock<Mutex<HashMap<Uuid, (i64, Instant)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Storage total for the usage endpoint, memoized for 60s so a dashboard refresh
/// never triggers a fresh `SUM` on a large table. Enforcement uses the live query.
pub async fn storage_bytes_cached(pool: &PgPool, workspace_id: Uuid) -> ApiResult<i64> {
    // Scope the guard so it is released before the (possible) await + re-lock;
    // std Mutex is not reentrant.
    {
        let cache = STORAGE_CACHE.lock().expect("storage cache mutex");
        if let Some(&(bytes, at)) = cache.get(&workspace_id)
            && at.elapsed() < STORAGE_CACHE_TTL
        {
            return Ok(bytes);
        }
    }
    let bytes = storage_bytes_live(pool, workspace_id).await?;
    STORAGE_CACHE
        .lock()
        .expect("storage cache mutex")
        .insert(workspace_id, (bytes, Instant::now()));
    Ok(bytes)
}

/// Count of active (non-revoked, non-expired) share links. Returns 0 when the M5
/// `share_links` table is not present yet, so the usage endpoint is robust to the
/// share migration landing independently.
pub async fn active_share_links(pool: &PgPool, workspace_id: Uuid) -> ApiResult<i64> {
    let table_exists: bool =
        sqlx::query_scalar("SELECT to_regclass('public.share_links') IS NOT NULL")
            .fetch_one(pool)
            .await?;
    if !table_exists {
        return Ok(0);
    }
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM share_links \
         WHERE workspace_id = $1 AND revoked_at IS NULL \
           AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(workspace_id)
    .fetch_one(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(plan: &str, status: &str, period_end: Option<DateTime<Utc>>) -> SubscriptionRow {
        SubscriptionRow {
            provider_subscription_id: "sub_x".into(),
            provider_customer_id: None,
            plan: plan.into(),
            interval: "monthly".into(),
            status: status.into(),
            current_period_end: period_end,
            extra_storage_units: 0,
        }
    }

    #[test]
    fn plan_limits_match_the_contract_table() {
        assert_eq!(Plan::Free.limits().max_members, Some(3));
        assert_eq!(Plan::Free.limits().max_storage_bytes, Some(2 * GIB));
        assert_eq!(Plan::Free.limits().max_active_share_links, Some(2));
        assert_eq!(Plan::Free.limits().max_concurrent_share_sessions, Some(5));

        assert_eq!(Plan::Solo.limits().max_members, Some(1));
        assert_eq!(Plan::Solo.limits().max_storage_bytes, Some(10 * GIB));
        assert_eq!(Plan::Solo.limits().max_active_share_links, Some(5));

        assert_eq!(Plan::Team.limits().max_members, Some(10));
        assert_eq!(Plan::Team.limits().max_storage_bytes, Some(50 * GIB));
        assert_eq!(Plan::Team.limits().max_concurrent_share_sessions, Some(25));

        // Unlimited is all-None.
        assert_eq!(Plan::Unlimited.limits(), PlanLimits::UNLIMITED);
    }

    #[test]
    fn free_is_the_only_write_gated_plan() {
        assert!(Plan::Unlimited.writes_allowed());
        assert!(Plan::Solo.writes_allowed());
        assert!(Plan::Team.writes_allowed());
        assert!(!Plan::Free.writes_allowed());
    }

    #[test]
    fn resolution_prefers_active_subscription_then_free() {
        let now = Utc::now();

        // Active/trialing grant their plan.
        assert_eq!(
            resolve_plan(Some(&sub("solo", "active", None)), now),
            Plan::Solo
        );
        assert_eq!(
            resolve_plan(Some(&sub("team", "trialing", None)), now),
            Plan::Team
        );

        // No subscription → Free.
        assert_eq!(resolve_plan(None, now), Plan::Free);

        // Canceled/revoked fall through to Free.
        assert_eq!(
            resolve_plan(Some(&sub("team", "canceled", None)), now),
            Plan::Free
        );
        assert_eq!(
            resolve_plan(Some(&sub("solo", "revoked", None)), now),
            Plan::Free
        );
    }

    #[test]
    fn past_due_holds_the_plan_only_within_grace() {
        let now = Utc::now();
        // 1 day into grace → still granted.
        let within = sub("team", "past_due", Some(now - Duration::days(1)));
        assert_eq!(resolve_plan(Some(&within), now), Plan::Team);
        // 8 days past period end (> 7-day grace) → lapses to Free.
        let beyond = sub("team", "past_due", Some(now - Duration::days(8)));
        assert_eq!(resolve_plan(Some(&beyond), now), Plan::Free);
        // past_due with no known period end gets no grace.
        let unknown = sub("solo", "past_due", None);
        assert_eq!(resolve_plan(Some(&unknown), now), Plan::Free);
    }

    #[test]
    fn storage_only_never_grants_a_plan() {
        let now = Utc::now();
        // Placeholder plan='solo' with the storage_only sentinel status: the
        // workspace falls through to Free, not Solo.
        let storage = sub("solo", "storage_only", None);
        assert_eq!(resolve_plan(Some(&storage), now), Plan::Free);
    }

    #[test]
    fn extra_storage_units_add_ten_gib_each_to_a_capped_plan() {
        // Solo's 10 GiB cap + 3 units (30 GiB) = 40 GiB.
        let limits = Plan::Solo.limits().with_extra_storage_units(3);
        assert_eq!(limits.max_storage_bytes, Some(40 * GIB));
        // Other caps are untouched by the storage add-on.
        assert_eq!(limits.max_members, Some(1));

        // Unlimited storage stays unlimited (None), even with units.
        let unlimited = Plan::Unlimited.limits().with_extra_storage_units(5);
        assert_eq!(unlimited.max_storage_bytes, None);

        // Zero / negative unit counts are a no-op.
        assert_eq!(
            Plan::Team
                .limits()
                .with_extra_storage_units(0)
                .max_storage_bytes,
            Some(50 * GIB)
        );
        assert_eq!(
            Plan::Team
                .limits()
                .with_extra_storage_units(-2)
                .max_storage_bytes,
            Some(50 * GIB)
        );
    }
}

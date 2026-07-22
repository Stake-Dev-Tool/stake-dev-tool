//! Plan limits (the single source of truth for quotas) and plan resolution.
//!
//! `plan_for` maps a workspace to its effective [`Plan`]; [`Plan::limits`] turns
//! that into the frozen [`PlanLimits`]. Writes on a lapsed trial are gated
//! separately by [`write_allowed`] so the frozen limits struct stays untouched
//! (an `Expired` workspace keeps Trial *read* limits but cannot push).

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
/// Free trial length, measured from `workspaces.created_at`.
pub const TRIAL_DAYS: i64 = 14;
/// How long a `past_due` subscription keeps its plan past `current_period_end`.
pub const GRACE_DAYS: i64 = 7;
/// How long the per-workspace storage total is memoized for the usage endpoint.
const STORAGE_CACHE_TTL: StdDuration = StdDuration::from_secs(60);

/// The resolved plan for a workspace at a point in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// Billing disabled (self-host) — no limits, no gating.
    Unlimited,
    /// Within the 14-day free trial.
    Trial,
    /// Trial lapsed with no active subscription: reads work, writes are blocked.
    Expired,
    Solo,
    Team,
}

impl Plan {
    /// The quota limits for this plan. `Expired` intentionally shares Trial's
    /// (read) limits — writes are refused by [`write_allowed`], not by the limits.
    pub fn limits(self) -> PlanLimits {
        match self {
            Plan::Unlimited => PlanLimits::UNLIMITED,
            Plan::Trial | Plan::Expired => PlanLimits {
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
            Plan::Trial => "trial",
            Plan::Expired => "expired",
            Plan::Solo => "solo",
            Plan::Team => "team",
        }
    }

    /// Writes (pushes, new shares) are allowed on every plan except `Expired`.
    pub fn writes_allowed(self) -> bool {
        !matches!(self, Plan::Expired)
    }
}

/// A workspace's subscription row, as stored by the webhook.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SubscriptionRow {
    pub polar_subscription_id: String,
    pub polar_customer_id: Option<String>,
    pub plan: String,
    pub interval: String,
    pub status: String,
    pub current_period_end: Option<DateTime<Utc>>,
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
        "SELECT polar_subscription_id, polar_customer_id, plan, \"interval\", status, \
                current_period_end \
         FROM subscriptions WHERE workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_optional(pool)
    .await?)
}

/// Resolves a workspace's effective plan. Billing disabled → `Unlimited` (the
/// self-host short-circuit, kept FIRST so overrides never touch a self-hosted
/// instance). Otherwise a non-expired instance-admin plan override wins next;
/// failing that, an active/trialing (or within-grace `past_due`) subscription
/// grants its plan; failing that, the workspace is on the trial (`Trial` while
/// within 14 days of creation, else `Expired`).
pub async fn plan_for(state: &AppState, workspace_id: Uuid) -> ApiResult<Plan> {
    if state.config.polar.is_none() {
        return Ok(Plan::Unlimited);
    }
    // An instance operator can comp a workspace a plan via `plan_overrides`; a
    // non-expired row is honored before any subscription is even loaded. Expired
    // rows are ignored lazily (no cleanup job).
    if let Some(plan) = active_override(&state.pool, workspace_id, Utc::now()).await? {
        return Ok(plan);
    }
    let subscription = load_subscription(&state.pool, workspace_id).await?;
    let created_at = workspace_created_at(&state.pool, workspace_id).await?;
    Ok(resolve_plan(subscription.as_ref(), created_at, Utc::now()))
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

/// Pure resolution given a loaded subscription and the workspace's creation time.
fn resolve_plan(
    subscription: Option<&SubscriptionRow>,
    created_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Plan {
    if let Some(sub) = subscription
        && let Some(plan) = active_plan(sub, now)
    {
        return plan;
    }
    if created_at + Duration::days(TRIAL_DAYS) > now {
        Plan::Trial
    } else {
        Plan::Expired
    }
}

/// The plan a subscription currently grants, or `None` if it grants nothing
/// (canceled/revoked/incomplete, or `past_due` past its 7-day grace).
fn active_plan(sub: &SubscriptionRow, now: DateTime<Utc>) -> Option<Plan> {
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

/// 403 `upgrade_required` when the workspace's trial has lapsed with no active
/// subscription; `Ok(())` on every other plan (including billing disabled).
pub async fn write_allowed(state: &AppState, workspace_id: Uuid) -> ApiResult<()> {
    if plan_for(state, workspace_id).await?.writes_allowed() {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "upgrade_required",
            "this workspace's trial has ended; upgrade the plan to make changes",
        ))
    }
}

async fn workspace_created_at(pool: &PgPool, workspace_id: Uuid) -> ApiResult<DateTime<Utc>> {
    sqlx::query_scalar::<_, DateTime<Utc>>("SELECT created_at FROM workspaces WHERE id = $1")
        .bind(workspace_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::not_found("workspace_not_found", "no such workspace"))
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
            polar_subscription_id: "sub_x".into(),
            polar_customer_id: None,
            plan: plan.into(),
            interval: "monthly".into(),
            status: status.into(),
            current_period_end: period_end,
        }
    }

    #[test]
    fn plan_limits_match_the_contract_table() {
        assert_eq!(Plan::Trial.limits().max_members, Some(3));
        assert_eq!(Plan::Trial.limits().max_storage_bytes, Some(2 * GIB));
        assert_eq!(Plan::Trial.limits().max_active_share_links, Some(2));
        assert_eq!(Plan::Trial.limits().max_concurrent_share_sessions, Some(5));

        assert_eq!(Plan::Solo.limits().max_members, Some(1));
        assert_eq!(Plan::Solo.limits().max_storage_bytes, Some(10 * GIB));
        assert_eq!(Plan::Solo.limits().max_active_share_links, Some(5));

        assert_eq!(Plan::Team.limits().max_members, Some(10));
        assert_eq!(Plan::Team.limits().max_storage_bytes, Some(50 * GIB));
        assert_eq!(Plan::Team.limits().max_concurrent_share_sessions, Some(25));

        // Expired keeps Trial (read) limits; Unlimited is all-None.
        assert_eq!(Plan::Expired.limits(), Plan::Trial.limits());
        assert_eq!(Plan::Unlimited.limits(), PlanLimits::UNLIMITED);
    }

    #[test]
    fn expired_is_the_only_write_gated_plan() {
        assert!(Plan::Unlimited.writes_allowed());
        assert!(Plan::Trial.writes_allowed());
        assert!(Plan::Solo.writes_allowed());
        assert!(Plan::Team.writes_allowed());
        assert!(!Plan::Expired.writes_allowed());
    }

    #[test]
    fn resolution_prefers_active_subscription_then_trial_window() {
        let now = Utc::now();
        let fresh = now - Duration::days(1);
        let old = now - Duration::days(30);

        // Active/trialing grant their plan regardless of trial age.
        assert_eq!(
            resolve_plan(Some(&sub("solo", "active", None)), old, now),
            Plan::Solo
        );
        assert_eq!(
            resolve_plan(Some(&sub("team", "trialing", None)), old, now),
            Plan::Team
        );

        // No subscription → trial window from creation.
        assert_eq!(resolve_plan(None, fresh, now), Plan::Trial);
        assert_eq!(resolve_plan(None, old, now), Plan::Expired);

        // Canceled/revoked fall through to the (here lapsed) trial.
        assert_eq!(
            resolve_plan(Some(&sub("team", "canceled", None)), old, now),
            Plan::Expired
        );
        assert_eq!(
            resolve_plan(Some(&sub("solo", "revoked", None)), old, now),
            Plan::Expired
        );
    }

    #[test]
    fn past_due_holds_the_plan_only_within_grace() {
        let now = Utc::now();
        let old = now - Duration::days(30);
        // 1 day into grace → still granted.
        let within = sub("team", "past_due", Some(now - Duration::days(1)));
        assert_eq!(resolve_plan(Some(&within), old, now), Plan::Team);
        // 8 days past period end (> 7-day grace) → lapses to Expired.
        let beyond = sub("team", "past_due", Some(now - Duration::days(8)));
        assert_eq!(resolve_plan(Some(&beyond), old, now), Plan::Expired);
        // past_due with no known period end gets no grace.
        let unknown = sub("solo", "past_due", None);
        assert_eq!(resolve_plan(Some(&unknown), old, now), Plan::Expired);
    }
}

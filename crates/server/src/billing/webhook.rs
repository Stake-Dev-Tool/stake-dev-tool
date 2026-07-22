//! Stripe webhook: `Stripe-Signature` verification (hand-rolled HMAC-SHA256 over
//! the existing `sha2`, no new crates) plus idempotent event handling. Contract:
//! docs/v2/m7-contract.md.
//!
//! Verification follows Stripe's scheme (NOT Standard Webhooks): the header is
//! `Stripe-Signature: t=<ts>,v1=<hexsig>[,v1=…]`, the signed content is
//! `{t}.{raw body}`, and the HMAC key is the webhook signing secret used VERBATIM
//! as raw ASCII bytes (the whole `whsec_…` string — Stripe does NOT base64-decode
//! it). Signatures are hex, and a timestamp more than five minutes from now is
//! rejected to blunt replay.
//!
//! After a valid signature: the raw event is inserted into `billing_events` keyed
//! by the Stripe event id (`ON CONFLICT (id) DO NOTHING`, so Stripe's
//! at-least-once retries are idempotent), then handled. Anything short of a bad
//! signature returns `200` so Stripe never retries a poison pill; only a genuine
//! infrastructure failure (the insert itself) is allowed to surface as a `5xx`.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use protocol::billing::{BillingInterval, PlanId};

use crate::AppState;
use crate::config::StripeConfig;
use crate::error::ApiError;

/// SHA-256 block size (RFC 2104), in bytes.
const BLOCK_SIZE: usize = 64;
/// Maximum accepted clock skew between the webhook timestamp and now, in seconds.
const MAX_SKEW_SECS: i64 = 300;

// ---------------------------------------------------------------------------
// HMAC-SHA256 (RFC 2104), implemented by hand over `sha2`.
// ---------------------------------------------------------------------------

/// HMAC-SHA256 of `message` under `key` (RFC 2104). Keys longer than the 64-byte
/// block are first hashed; shorter keys are zero-padded.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        block[..32].copy_from_slice(Sha256::digest(key).as_slice());
    } else {
        block[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] ^= block[i];
        opad[i] ^= block[i];
    }

    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    let digest = outer.finalize();

    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Length-checked constant-time comparison: over equal-length inputs it inspects
/// every byte, accumulating differences so timing never reveals the mismatch
/// position. A length difference short-circuits — signature length is not secret.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Decodes a lowercase/uppercase hex string into bytes, or `None` on any
/// non-hex character or an odd length.
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    for pair in bytes.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

/// Why a webhook signature was rejected. Every variant maps to `401`.
#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    /// The `t=` timestamp was missing or unparseable.
    BadTimestamp,
    /// The timestamp was valid but more than five minutes from now.
    StaleTimestamp,
    /// No `v1=` candidate matched the expected signature.
    NoMatch,
}

/// Verifies a `Stripe-Signature` header. `now_unix` is injected so the skew check
/// is testable. The key is the webhook secret's raw ASCII bytes (verbatim).
pub fn verify_signature(
    secret: &str,
    body: &[u8],
    signature_header: &str,
    now_unix: i64,
) -> Result<(), VerifyError> {
    // Parse the comma-separated `k=v` header into the timestamp and v1 candidates.
    let mut timestamp: Option<&str> = None;
    let mut candidates: Vec<&str> = Vec::new();
    for part in signature_header.split(',') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("t=") {
            timestamp = Some(v);
        } else if let Some(v) = part.strip_prefix("v1=") {
            candidates.push(v);
        }
    }

    let ts: i64 = timestamp
        .ok_or(VerifyError::BadTimestamp)?
        .trim()
        .parse()
        .map_err(|_| VerifyError::BadTimestamp)?;
    if (now_unix - ts).abs() > MAX_SKEW_SECS {
        return Err(VerifyError::StaleTimestamp);
    }

    // Signed content: {t}.{raw body} (body appended raw, may be non-UTF-8). The
    // key is the secret string verbatim (no `whsec_` stripping, no base64 decode).
    let mut signed = Vec::with_capacity(timestamp.map_or(0, str::len) + body.len() + 1);
    signed.extend_from_slice(ts.to_string().as_bytes());
    signed.push(b'.');
    signed.extend_from_slice(body);
    let expected = hmac_sha256(secret.as_bytes(), &signed);

    for candidate in candidates {
        if let Some(provided) = hex_decode(candidate)
            && constant_time_eq(&provided, &expected)
        {
            return Ok(());
        }
    }
    Err(VerifyError::NoMatch)
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn invalid_signature() -> Response {
    ApiError::unauthorized("invalid_signature", "webhook signature verification failed")
        .into_response()
}

/// `POST /api/billing/webhook`. 404 when billing is disabled; 401 on a bad
/// signature; otherwise 200 (the event is recorded and, best-effort, handled).
pub async fn handle(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(stripe) = state.config.stripe.as_ref() else {
        return ApiError::not_found("not_found", "billing is not enabled").into_response();
    };

    let Some(signature) = header_str(&headers, "stripe-signature") else {
        return invalid_signature();
    };

    if verify_signature(
        &stripe.webhook_secret,
        &body,
        signature,
        Utc::now().timestamp(),
    )
    .is_err()
    {
        return invalid_signature();
    }

    // Signature is good: from here on we owe Stripe a 200 unless the DB itself
    // fails (a transient error worth retrying, not a poison pill).
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        // A signature-valid but non-JSON body cannot happen with real Stripe;
        // there is no event id to record it under, so just acknowledge it.
        Err(_) => {
            tracing::warn!("billing webhook body passed signature check but is not JSON");
            return StatusCode::OK.into_response();
        }
    };

    // Stripe's event id (`evt_…`) is the idempotency key and the ledger PK.
    let Some(id) = payload
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        tracing::warn!("billing webhook event has no id");
        return StatusCode::OK.into_response();
    };

    let event_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    match insert_event(&state.pool, &id, &event_type, &payload).await {
        Ok(true) => {}
        // Already seen (Stripe retry) → idempotent no-op.
        Ok(false) => return StatusCode::OK.into_response(),
        // The one place a 5xx is allowed: we could not even record the event.
        Err(e) => return ApiError::internal(e).into_response(),
    }

    match process_event(&state, stripe, &event_type, &payload).await {
        Ok(()) => {
            let _ = mark_processed(&state.pool, &id).await;
        }
        Err(reason) => {
            tracing::warn!(event_id = id, event_type, reason, "billing webhook skipped");
            let _ = mark_error(&state.pool, &id, &reason).await;
        }
    }
    StatusCode::OK.into_response()
}

// ---------------------------------------------------------------------------
// Event processing
// ---------------------------------------------------------------------------

/// Handles one authenticated event. `Err(reason)` marks the stored event with an
/// error but still yields a 200 (never a retry-inducing 5xx).
async fn process_event(
    state: &AppState,
    stripe: &StripeConfig,
    event_type: &str,
    payload: &Value,
) -> Result<(), String> {
    match event_type {
        "customer.subscription.created"
        | "customer.subscription.updated"
        | "customer.subscription.deleted" => {
            let object = payload
                .get("data")
                .and_then(|d| d.get("object"))
                .ok_or_else(|| "subscription event has no data.object".to_string())?;
            let workspace_id = extract_workspace_id(object)
                .ok_or_else(|| "no workspace_id in subscription metadata".to_string())?;
            let subscription_id = object
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| "subscription has no id".to_string())?;
            // A deletion is Stripe's terminal state — force it to `canceled`.
            let status = if event_type == "customer.subscription.deleted" {
                "canceled"
            } else {
                object
                    .get("status")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "subscription has no status".to_string())?
            };
            let period_end = extract_period_end(object);
            let customer = extract_customer_id(object);

            // Split the line items into (optional) plan and (optional) storage.
            let (plan_interval, storage_units, has_storage) = parse_items(stripe, object);
            // Storage counts only while the subscription is live; a dead status
            // (or a deletion) drops the add-on to zero.
            let storage_live = matches!(status, "active" | "trialing" | "past_due");
            let effective_units = if storage_live { storage_units } else { 0 };

            match plan_interval {
                Some((plan, interval)) => upsert_plan(
                    &state.pool,
                    workspace_id,
                    subscription_id,
                    customer.as_deref(),
                    plan,
                    interval,
                    status,
                    period_end,
                    // Only touch stored storage when THIS subscription carries it;
                    // a plan-only sub leaves any separate storage add-on intact.
                    has_storage.then_some(effective_units),
                )
                .await
                .map_err(|e| e.to_string()),
                None => {
                    if !has_storage {
                        return Err(
                            "subscription carries no recognized plan or storage price".to_string()
                        );
                    }
                    upsert_storage_only(
                        &state.pool,
                        workspace_id,
                        subscription_id,
                        customer.as_deref(),
                        period_end,
                        effective_units,
                    )
                    .await
                    .map_err(|e| e.to_string())
                }
            }
        }
        // Audit-only (already stored) and every unrecognized type: stored, ignored.
        // `checkout.session.completed` lands here — the subscription.* events do
        // the actual upsert.
        _ => Ok(()),
    }
}

/// Extracts `workspace_id` from a subscription object's `metadata.workspace_id`
/// (set at checkout via `subscription_data[metadata][workspace_id]`).
fn extract_workspace_id(object: &Value) -> Option<Uuid> {
    let raw = object
        .get("metadata")
        .and_then(|m| m.get("workspace_id"))
        .and_then(Value::as_str)?;
    Uuid::parse_str(raw).ok()
}

/// Stripe's `customer` is the id string when unexpanded, or an object with `id`.
fn extract_customer_id(object: &Value) -> Option<String> {
    match object.get("customer") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(v) => v.get("id").and_then(Value::as_str).map(String::from),
        None => None,
    }
}

/// `current_period_end` is a Unix timestamp (seconds). Read the top-level field
/// (older API versions) and fall back to the first item's field (2025+ API, where
/// the period moved onto the subscription items).
fn extract_period_end(object: &Value) -> Option<DateTime<Utc>> {
    if let Some(ts) = object.get("current_period_end").and_then(Value::as_i64) {
        return DateTime::from_timestamp(ts, 0);
    }
    object
        .get("items")
        .and_then(|i| i.get("data"))
        .and_then(Value::as_array)?
        .iter()
        .find_map(|item| item.get("current_period_end").and_then(Value::as_i64))
        .and_then(|ts| DateTime::from_timestamp(ts, 0))
}

/// Walks `items.data[*]`, resolving each item's price into either a (plan,
/// interval) or the storage add-on quantity. Returns the plan mapping (if any),
/// the summed storage units, and whether the storage price appeared at all.
fn parse_items(
    stripe: &StripeConfig,
    object: &Value,
) -> (Option<(PlanId, BillingInterval)>, i64, bool) {
    let mut plan_interval = None;
    let mut storage_units = 0i64;
    let mut has_storage = false;

    let items = object
        .get("items")
        .and_then(|i| i.get("data"))
        .and_then(Value::as_array);
    let Some(items) = items else {
        return (plan_interval, storage_units, has_storage);
    };

    for item in items {
        // `price` is an object `{ id, … }`; tolerate a bare id string too.
        let price = item
            .get("price")
            .and_then(|p| p.get("id").and_then(Value::as_str).or_else(|| p.as_str()));
        let Some(price) = price else { continue };
        let quantity = item.get("quantity").and_then(Value::as_i64).unwrap_or(1);

        if let Some(pi) = super::stripe::plan_for_price(stripe, price) {
            plan_interval = Some(pi);
        } else if price == stripe.price_storage {
            has_storage = true;
            storage_units += quantity;
        }
    }
    (plan_interval, storage_units, has_storage)
}

fn plan_str(plan: PlanId) -> &'static str {
    match plan {
        PlanId::Solo => "solo",
        PlanId::Team => "team",
    }
}

fn interval_str(interval: BillingInterval) -> &'static str {
    match interval {
        BillingInterval::Monthly => "monthly",
        BillingInterval::Yearly => "yearly",
    }
}

/// Upserts the workspace's plan columns. `storage` is `Some(units)` only when the
/// SAME subscription also carries the storage price (a mixed subscription) — then
/// `extra_storage_units` is set too; `None` leaves any separately-purchased
/// storage add-on untouched.
#[allow(clippy::too_many_arguments)]
async fn upsert_plan(
    pool: &PgPool,
    workspace_id: Uuid,
    subscription_id: &str,
    customer_id: Option<&str>,
    plan: PlanId,
    interval: BillingInterval,
    status: &str,
    period_end: Option<DateTime<Utc>>,
    storage: Option<i64>,
) -> Result<(), sqlx::Error> {
    match storage {
        Some(units) => {
            sqlx::query(
                "INSERT INTO subscriptions \
                   (workspace_id, provider_subscription_id, provider_customer_id, plan, \
                    \"interval\", status, current_period_end, extra_storage_units, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now()) \
                 ON CONFLICT (workspace_id) DO UPDATE SET \
                   provider_subscription_id = EXCLUDED.provider_subscription_id, \
                   provider_customer_id     = EXCLUDED.provider_customer_id, \
                   plan                     = EXCLUDED.plan, \
                   \"interval\"             = EXCLUDED.\"interval\", \
                   status                   = EXCLUDED.status, \
                   current_period_end       = EXCLUDED.current_period_end, \
                   extra_storage_units      = EXCLUDED.extra_storage_units, \
                   updated_at               = now()",
            )
            .bind(workspace_id)
            .bind(subscription_id)
            .bind(customer_id)
            .bind(plan_str(plan))
            .bind(interval_str(interval))
            .bind(status)
            .bind(period_end)
            .bind(units)
            .execute(pool)
            .await?;
        }
        None => {
            // Plan-only: `extra_storage_units` uses the column default (0) on
            // insert and is left untouched on update.
            sqlx::query(
                "INSERT INTO subscriptions \
                   (workspace_id, provider_subscription_id, provider_customer_id, plan, \
                    \"interval\", status, current_period_end, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, now()) \
                 ON CONFLICT (workspace_id) DO UPDATE SET \
                   provider_subscription_id = EXCLUDED.provider_subscription_id, \
                   provider_customer_id     = EXCLUDED.provider_customer_id, \
                   plan                     = EXCLUDED.plan, \
                   \"interval\"             = EXCLUDED.\"interval\", \
                   status                   = EXCLUDED.status, \
                   current_period_end       = EXCLUDED.current_period_end, \
                   updated_at               = now()",
            )
            .bind(workspace_id)
            .bind(subscription_id)
            .bind(customer_id)
            .bind(plan_str(plan))
            .bind(interval_str(interval))
            .bind(status)
            .bind(period_end)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

/// Upserts a storage-only subscription (no plan price). When a plan row already
/// exists, only `extra_storage_units` changes — the plan columns are untouched.
/// When none exists and `units > 0`, a placeholder row is inserted with
/// plan='solo', interval='monthly', status='storage_only' (which `plan_for`
/// treats as NOT plan-granting). When `units == 0` (a canceled add-on) no new row
/// is created — a plain update clears any existing add-on and no-ops otherwise.
async fn upsert_storage_only(
    pool: &PgPool,
    workspace_id: Uuid,
    subscription_id: &str,
    customer_id: Option<&str>,
    period_end: Option<DateTime<Utc>>,
    units: i64,
) -> Result<(), sqlx::Error> {
    if units <= 0 {
        sqlx::query(
            "UPDATE subscriptions SET extra_storage_units = 0, updated_at = now() \
             WHERE workspace_id = $1",
        )
        .bind(workspace_id)
        .execute(pool)
        .await?;
        return Ok(());
    }

    sqlx::query(
        "INSERT INTO subscriptions \
           (workspace_id, provider_subscription_id, provider_customer_id, plan, \"interval\", \
            status, current_period_end, extra_storage_units, updated_at) \
         VALUES ($1, $2, $3, 'solo', 'monthly', 'storage_only', $4, $5, now()) \
         ON CONFLICT (workspace_id) DO UPDATE SET \
           extra_storage_units = EXCLUDED.extra_storage_units, \
           updated_at          = now()",
    )
    .bind(workspace_id)
    .bind(subscription_id)
    .bind(customer_id)
    .bind(period_end)
    .bind(units)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// billing_events ledger
// ---------------------------------------------------------------------------

/// Inserts the raw event; `Ok(true)` when a new row was created, `Ok(false)` when
/// the id was already present (a Stripe retry).
async fn insert_event(
    pool: &PgPool,
    id: &str,
    event_type: &str,
    payload: &Value,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO billing_events (id, type, payload) VALUES ($1, $2, $3) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(id)
    .bind(event_type)
    .bind(payload)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

async fn mark_processed(pool: &PgPool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE billing_events SET processed_at = now(), error = NULL WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn mark_error(pool: &PgPool, id: &str, reason: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE billing_events SET error = $2 WHERE id = $1")
        .bind(id)
        .bind(reason)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    // RFC 4231 HMAC-SHA256 test vectors, cases 1-3.
    #[test]
    fn hmac_matches_rfc4231_case1() {
        let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(
            hex(&mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn hmac_matches_rfc4231_case2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn hmac_matches_rfc4231_case3() {
        // key = 0xaa x 20, data = 0xdd x 50 (key <= block size, long message).
        let mac = hmac_sha256(&[0xaa; 20], &[0xdd; 50]);
        assert_eq!(
            hex(&mac),
            "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"
        );
    }

    #[test]
    fn hmac_handles_key_longer_than_block() {
        // key = 0xaa x 131 (> 64-byte block → hashed first), RFC 4231 case 6.
        let mac = hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First",
        );
        assert_eq!(
            hex(&mac),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn hex_decode_round_trips_and_rejects_bad_input() {
        assert_eq!(hex_decode("00ff10"), Some(vec![0x00, 0xff, 0x10]));
        assert_eq!(hex_decode("ABCD"), Some(vec![0xab, 0xcd]));
        assert_eq!(hex_decode("abc"), None); // odd length
        assert_eq!(hex_decode("zz"), None); // non-hex
    }

    /// Builds a valid `Stripe-Signature` header the way Stripe (and the tests) do:
    /// HMAC over `{t}.{body}` with the secret's raw bytes, hex-encoded.
    fn sign(secret: &str, ts: i64, body: &[u8]) -> String {
        let mut signed = Vec::new();
        signed.extend_from_slice(ts.to_string().as_bytes());
        signed.push(b'.');
        signed.extend_from_slice(body);
        let sig = hex(&hmac_sha256(secret.as_bytes(), &signed));
        format!("t={ts},v1={sig}")
    }

    #[test]
    fn verify_accepts_a_good_signature_and_rejects_tampering() {
        // The secret is used verbatim as the key (whsec_ prefix included).
        let secret = "whsec_stripe_test_secret_key_0123456789";
        let now = 1_700_000_000;
        let body = br#"{"type":"customer.subscription.updated"}"#;
        let header = sign(secret, now, body);

        assert_eq!(verify_signature(secret, body, &header, now), Ok(()));

        // Tampered body → no candidate matches.
        let tampered = br#"{"type":"customer.subscription.deleted"}"#;
        assert_eq!(
            verify_signature(secret, tampered, &header, now),
            Err(VerifyError::NoMatch)
        );

        // Stale timestamp (> 5 min) → rejected before hashing.
        assert_eq!(
            verify_signature(secret, body, &header, now + 400),
            Err(VerifyError::StaleTimestamp)
        );

        // A second candidate in the header still passes if any matches.
        let multi = format!("t={now},v1=deadbeef,v1={}", {
            let mut signed = Vec::new();
            signed.extend_from_slice(now.to_string().as_bytes());
            signed.push(b'.');
            signed.extend_from_slice(body);
            hex(&hmac_sha256(secret.as_bytes(), &signed))
        });
        assert_eq!(verify_signature(secret, body, &multi, now), Ok(()));

        // A wrong key (base64-decoded, as Standard Webhooks would) must NOT verify
        // — this is the scheme difference we must get right.
        assert_ne!(verify_signature("whsec_other", body, &header, now), Ok(()));

        // Missing timestamp → BadTimestamp.
        assert_eq!(
            verify_signature(secret, body, "v1=abcd", now),
            Err(VerifyError::BadTimestamp)
        );
    }

    #[test]
    fn constant_time_eq_basic() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}

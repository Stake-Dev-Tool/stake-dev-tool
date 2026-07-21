//! Polar webhook: Standard-Webhooks signature verification (hand-rolled
//! HMAC-SHA256 over the existing `sha2`, no new crates) plus idempotent event
//! handling. Contract: docs/v2/m7-contract.md.
//!
//! Verification follows the Standard Webhooks spec: the signed content is
//! `{id}.{timestamp}.{raw body}`, the key is the base64-decoded webhook secret
//! (after stripping an optional `whsec_` prefix), and the `webhook-signature`
//! header is a space-separated list of `v1,<base64>` candidates. A timestamp more
//! than five minutes from now is rejected to blunt replay.
//!
//! After a valid signature: the raw event is inserted into `billing_events`
//! (`ON CONFLICT (id) DO NOTHING`, so Polar's at-least-once retries are
//! idempotent), then handled. Anything short of a bad signature returns `200` so
//! Polar never retries a poison pill; only a genuine infrastructure failure
//! (the insert itself) is allowed to surface as a `5xx`.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use protocol::billing::{BillingInterval, PlanId};

use crate::AppState;
use crate::config::PolarConfig;
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
/// position. A length difference short-circuits — base64 signature length is not
/// secret.
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

/// Decodes the webhook signing key: strip an optional `whsec_` prefix, then
/// base64-decode. `None` if the remainder is not valid base64.
fn decode_secret(secret: &str) -> Option<Vec<u8>> {
    let raw = secret.strip_prefix("whsec_").unwrap_or(secret);
    BASE64.decode(raw.trim()).ok()
}

/// Why a webhook signature was rejected. Every variant maps to `401`.
#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    BadTimestamp,
    StaleTimestamp,
    BadSecret,
    NoMatch,
}

/// Verifies a Standard-Webhooks signature. `now_unix` is injected so the skew
/// check is testable.
pub fn verify_signature(
    secret: &str,
    id: &str,
    timestamp: &str,
    body: &[u8],
    signature_header: &str,
    now_unix: i64,
) -> Result<(), VerifyError> {
    let ts: i64 = timestamp
        .trim()
        .parse()
        .map_err(|_| VerifyError::BadTimestamp)?;
    if (now_unix - ts).abs() > MAX_SKEW_SECS {
        return Err(VerifyError::StaleTimestamp);
    }

    let key = decode_secret(secret).ok_or(VerifyError::BadSecret)?;

    // Signed content: id.timestamp.body (body appended raw, may be non-UTF-8).
    let mut signed = Vec::with_capacity(id.len() + timestamp.len() + body.len() + 2);
    signed.extend_from_slice(id.as_bytes());
    signed.push(b'.');
    signed.extend_from_slice(timestamp.as_bytes());
    signed.push(b'.');
    signed.extend_from_slice(body);
    let expected = hmac_sha256(&key, &signed);

    // Header: space-separated "v1,<base64>" entries — any match passes.
    for entry in signature_header.split_whitespace() {
        let candidate = entry.rsplit(',').next().unwrap_or(entry);
        if let Ok(provided) = BASE64.decode(candidate)
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
    let Some(polar) = state.config.polar.as_ref() else {
        return ApiError::not_found("not_found", "billing is not enabled").into_response();
    };

    let (Some(id), Some(timestamp), Some(signature)) = (
        header_str(&headers, "webhook-id"),
        header_str(&headers, "webhook-timestamp"),
        header_str(&headers, "webhook-signature"),
    ) else {
        return invalid_signature();
    };

    if verify_signature(
        &polar.webhook_secret,
        id,
        timestamp,
        &body,
        signature,
        Utc::now().timestamp(),
    )
    .is_err()
    {
        return invalid_signature();
    }

    // Signature is good: from here on we owe Polar a 200 unless the DB itself
    // fails (a transient error worth retrying, not a poison pill).
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            let _ = insert_event(&state.pool, id, "", &Value::Null).await;
            let _ = mark_error(&state.pool, id, "malformed JSON body").await;
            return StatusCode::OK.into_response();
        }
    };

    let event_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    match insert_event(&state.pool, id, &event_type, &payload).await {
        Ok(true) => {}
        // Already seen (Polar retry) → idempotent no-op.
        Ok(false) => return StatusCode::OK.into_response(),
        // The one place a 5xx is allowed: we could not even record the event.
        Err(e) => return ApiError::internal(e).into_response(),
    }

    match process_event(&state, polar, &event_type, &payload).await {
        Ok(()) => {
            let _ = mark_processed(&state.pool, id).await;
        }
        Err(reason) => {
            tracing::warn!(event_id = id, event_type, reason, "billing webhook skipped");
            let _ = mark_error(&state.pool, id, &reason).await;
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
    polar: &PolarConfig,
    event_type: &str,
    payload: &Value,
) -> Result<(), String> {
    match event_type {
        "subscription.created"
        | "subscription.updated"
        | "subscription.active"
        | "subscription.canceled"
        | "subscription.revoked"
        | "subscription.uncanceled" => {
            let data = payload
                .get("data")
                .ok_or_else(|| "subscription event has no data".to_string())?;
            let workspace_id = extract_workspace_id(payload)
                .ok_or_else(|| "no workspace_id in event metadata".to_string())?;
            let product_id = extract_product_id(data)
                .ok_or_else(|| "subscription has no product_id".to_string())?;
            let (plan, interval) = super::polar::plan_for_product(polar, &product_id)
                .ok_or_else(|| format!("unrecognized product_id {product_id}"))?;
            let subscription_id = data
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| "subscription has no id".to_string())?;
            let status = data
                .get("status")
                .and_then(Value::as_str)
                .ok_or_else(|| "subscription has no status".to_string())?;

            upsert_subscription(
                &state.pool,
                workspace_id,
                subscription_id,
                extract_customer_id(data).as_deref(),
                plan,
                interval,
                status,
                extract_period_end(data),
            )
            .await
            .map_err(|e| e.to_string())
        }
        // Audit-only (already stored) and every unrecognized type: stored, ignored.
        _ => Ok(()),
    }
}

/// Extracts `workspace_id` from the event, tolerating either a top-level
/// subscription `metadata` or a nested `checkout`/`subscription` metadata.
fn extract_workspace_id(payload: &Value) -> Option<Uuid> {
    let data = payload.get("data")?;
    let candidates = [
        data.get("metadata"),
        data.get("subscription").and_then(|s| s.get("metadata")),
        data.get("checkout").and_then(|c| c.get("metadata")),
    ];
    for meta in candidates.into_iter().flatten() {
        if let Some(raw) = meta.get("workspace_id").and_then(Value::as_str)
            && let Ok(id) = Uuid::parse_str(raw)
        {
            return Some(id);
        }
    }
    None
}

fn extract_product_id(data: &Value) -> Option<String> {
    data.get("product_id")
        .and_then(Value::as_str)
        .or_else(|| {
            data.get("product")
                .and_then(|p| p.get("id"))
                .and_then(Value::as_str)
        })
        .map(String::from)
}

fn extract_customer_id(data: &Value) -> Option<String> {
    data.get("customer_id")
        .and_then(Value::as_str)
        .or_else(|| {
            data.get("customer")
                .and_then(|c| c.get("id"))
                .and_then(Value::as_str)
        })
        .map(String::from)
}

fn extract_period_end(data: &Value) -> Option<DateTime<Utc>> {
    let raw = data.get("current_period_end").and_then(Value::as_str)?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
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

#[allow(clippy::too_many_arguments)]
async fn upsert_subscription(
    pool: &PgPool,
    workspace_id: Uuid,
    subscription_id: &str,
    customer_id: Option<&str>,
    plan: PlanId,
    interval: BillingInterval,
    status: &str,
    period_end: Option<DateTime<Utc>>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO subscriptions \
           (workspace_id, polar_subscription_id, polar_customer_id, plan, \"interval\", \
            status, current_period_end, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, now()) \
         ON CONFLICT (workspace_id) DO UPDATE SET \
           polar_subscription_id = EXCLUDED.polar_subscription_id, \
           polar_customer_id     = EXCLUDED.polar_customer_id, \
           plan                  = EXCLUDED.plan, \
           \"interval\"          = EXCLUDED.\"interval\", \
           status                = EXCLUDED.status, \
           current_period_end    = EXCLUDED.current_period_end, \
           updated_at            = now()",
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
    Ok(())
}

// ---------------------------------------------------------------------------
// billing_events ledger
// ---------------------------------------------------------------------------

/// Inserts the raw event; `Ok(true)` when a new row was created, `Ok(false)` when
/// the id was already present (a Polar retry).
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

    /// Builds a valid Standard-Webhooks signature the way Polar (and the tests)
    /// do, so verification can be exercised end to end.
    fn sign(key: &[u8], id: &str, ts: i64, body: &[u8]) -> (String, String) {
        let secret = format!("whsec_{}", BASE64.encode(key));
        let mut signed = Vec::new();
        signed.extend_from_slice(id.as_bytes());
        signed.push(b'.');
        signed.extend_from_slice(ts.to_string().as_bytes());
        signed.push(b'.');
        signed.extend_from_slice(body);
        let sig = BASE64.encode(hmac_sha256(key, &signed));
        (secret, format!("v1,{sig}"))
    }

    #[test]
    fn verify_accepts_a_good_signature_and_rejects_tampering() {
        let key = b"0123456789abcdef0123456789abcdef";
        let now = 1_700_000_000;
        let body = br#"{"type":"subscription.active"}"#;
        let (secret, header) = sign(key, "msg_1", now, body);

        assert_eq!(
            verify_signature(&secret, "msg_1", &now.to_string(), body, &header, now),
            Ok(())
        );

        // Tampered body → no candidate matches.
        let tampered = br#"{"type":"subscription.revoked"}"#;
        assert_eq!(
            verify_signature(&secret, "msg_1", &now.to_string(), tampered, &header, now),
            Err(VerifyError::NoMatch)
        );

        // Stale timestamp (> 5 min) → rejected before hashing.
        assert_eq!(
            verify_signature(&secret, "msg_1", &now.to_string(), body, &header, now + 400),
            Err(VerifyError::StaleTimestamp)
        );

        // A second candidate in the header still passes if any matches.
        let multi = format!("v1,AAAA {header}");
        assert_eq!(
            verify_signature(&secret, "msg_1", &now.to_string(), body, &multi, now),
            Ok(())
        );
    }

    #[test]
    fn constant_time_eq_basic() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}

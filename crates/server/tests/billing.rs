//! M7 integration tests: plan resolution, quota enforcement, the Stripe webhook
//! (Stripe-Signature over the hand-rolled HMAC), the storage add-on, and the
//! billing status/checkout endpoints. DB-backed tests self-skip when
//! `TEST_DATABASE_URL` is unset; the HMAC vector tests always run. The dev
//! database persists, so every email/slug is suffixed with a fresh UUID.

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use server::billing::webhook::hmac_sha256;
use server::config::{Config, StorageConfig, StripeConfig};
use server::{AppState, db, http, storage};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

// --- HMAC RFC 4231 vectors (no DB) -----------------------------------------

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[test]
fn hmac_sha256_rfc4231_vectors() {
    // Case 1.
    assert_eq!(
        hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    // Case 2.
    assert_eq!(
        hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    // Case 3.
    assert_eq!(
        hex(&hmac_sha256(&[0xaa; 20], &[0xdd; 50])),
        "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"
    );
}

// --- setup -----------------------------------------------------------------

/// The synthetic webhook signing secret. Stripe uses it VERBATIM as the raw-ASCII
/// HMAC key (no `whsec_` stripping, no base64 decode), so the tests sign with
/// exactly these bytes.
const WEBHOOK_SECRET: &str = "whsec_m7_stripe_test_secret_0123456789ab";

fn stripe_config() -> StripeConfig {
    StripeConfig {
        secret_key: "sk_test_m7".to_string(),
        webhook_secret: WEBHOOK_SECRET.to_string(),
        price_solo_monthly: "price_solo_m".to_string(),
        price_solo_yearly: "price_solo_y".to_string(),
        price_team_monthly: "price_team_m".to_string(),
        price_team_yearly: "price_team_y".to_string(),
        price_storage: "price_storage".to_string(),
    }
}

struct Ctx {
    state: AppState,
    _tmp: tempfile::TempDir,
}

async fn setup(stripe: Option<StripeConfig>) -> Option<Ctx> {
    let database_url = std::env::var("TEST_DATABASE_URL").ok()?;
    let tmp = tempfile::tempdir().unwrap();
    let config = Config {
        bind_addr: "127.0.0.1:0".to_string(),
        database_url: database_url.clone(),
        storage: StorageConfig::Fs {
            root: tmp.path().to_path_buf(),
        },
        cookie_secure: false,
        public_url: Some("https://app.example.com".to_string()),
        github: None,
        discord: None,
        mail: None,
        stripe,
        web_dir: None,
        storage_max_blob_bytes: 8_589_934_592,
        server_math_cache_bytes: 21_474_836_480,
        server_tenant_books_cap_bytes: None,
        play_domain: None,
        admin_emails: Vec::new(),
    };
    let pool = db::connect_lazy(&database_url).expect("lazy pool");
    let store = storage::build_object_store(&config).expect("fs store");
    let state = AppState::new(config, pool, store);
    db::migrate(&state.pool).await.expect("migrations apply");
    Some(Ctx { state, _tmp: tmp })
}

// --- HTTP client (cookies + bearer) ----------------------------------------

struct Client {
    state: AppState,
    cookies: HashMap<String, String>,
}

impl Client {
    fn new(state: &AppState) -> Self {
        Self {
            state: state.clone(),
            cookies: HashMap::new(),
        }
    }

    fn cookie_header(&self) -> Option<String> {
        if self.cookies.is_empty() {
            return None;
        }
        Some(
            self.cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }

    fn apply_set_cookie(&mut self, header: &str) {
        let first = header.split(';').next().unwrap_or("");
        if let Some((name, value)) = first.split_once('=') {
            let (name, value) = (name.trim().to_string(), value.trim().to_string());
            if value.is_empty() || header.to_lowercase().contains("max-age=0") {
                self.cookies.remove(&name);
            } else {
                self.cookies.insert(name, value);
            }
        }
    }

    /// Full-control request: extra headers, optional bearer, raw body.
    async fn raw(
        &mut self,
        method: Method,
        uri: &str,
        content_type: Option<&str>,
        body: Vec<u8>,
        bearer: Option<&str>,
        extra: &[(&str, String)],
    ) -> (StatusCode, Vec<u8>) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = bearer {
            builder = builder.header("authorization", format!("Bearer {token}"));
        } else if let Some(header) = self.cookie_header() {
            builder = builder.header("cookie", header);
        }
        if let Some(ct) = content_type {
            builder = builder.header("content-type", ct);
        }
        for (k, v) in extra {
            builder = builder.header(*k, v);
        }
        let request = builder
            .body(if body.is_empty() {
                Body::empty()
            } else {
                Body::from(body)
            })
            .unwrap();

        let response = http::build_router(self.state.clone())
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        for header in response.headers().get_all("set-cookie") {
            if let Ok(value) = header.to_str() {
                self.apply_set_cookie(value);
            }
        }
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, bytes.to_vec())
    }

    async fn send(
        &mut self,
        method: Method,
        uri: &str,
        body: Option<Value>,
        bearer: Option<&str>,
    ) -> (StatusCode, Value) {
        let (ct, raw) = match &body {
            Some(v) => (Some("application/json"), serde_json::to_vec(v).unwrap()),
            None => (None, Vec::new()),
        };
        let (status, bytes) = self.raw(method, uri, ct, raw, bearer, &[]).await;
        (status, parse_json(&bytes))
    }

    async fn post(&mut self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.send(Method::POST, uri, Some(body), None).await
    }

    async fn get(&mut self, uri: &str) -> (StatusCode, Value) {
        self.send(Method::GET, uri, None, None).await
    }
}

fn parse_json(bytes: &[u8]) -> Value {
    if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(bytes).unwrap_or(Value::Null)
    }
}

fn s(value: &Value) -> &str {
    value.as_str().unwrap()
}

fn unique_email() -> String {
    format!("user-{}@example.com", Uuid::new_v4())
}

fn unique_slug() -> String {
    format!("ws-{}", Uuid::new_v4())
}

/// The dev DB persists between runs, so webhook event ids (the `billing_events`
/// PK) must be unique per run or a replay would be seen as a duplicate.
fn evt_id() -> String {
    format!("evt-{}", Uuid::new_v4())
}

fn sha_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

async fn register(state: &AppState, email: &str, name: &str) -> Client {
    let mut client = Client::new(state);
    let (status, body) = client
        .post(
            "/api/auth/register",
            json!({ "email": email, "password": "password123", "display_name": name }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "register failed: {body}");
    client
}

/// Registers an owner, creates a workspace, and mints a `push:math` PAT.
async fn bootstrap(state: &AppState) -> (Client, String, String) {
    let mut owner = register(state, &unique_email(), "Owner").await;
    let ws = unique_slug();
    let (status, body) = owner
        .post(
            "/api/workspaces",
            json!({ "name": "Billing WS", "slug": &ws }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, created) = owner
        .post(
            "/api/tokens",
            json!({ "name": "ci", "scopes": ["push:math"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let token = s(&created["token"]).to_string();
    (owner, ws, token)
}

// --- direct SQL helpers ----------------------------------------------------

async fn workspace_id(state: &AppState, slug: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM workspaces WHERE slug = $1")
        .bind(slug)
        .fetch_one(&state.pool)
        .await
        .unwrap()
}

async fn insert_subscription(
    state: &AppState,
    ws_id: Uuid,
    plan: &str,
    interval: &str,
    status: &str,
    period_end: Option<DateTime<Utc>>,
) {
    sqlx::query(
        "INSERT INTO subscriptions \
           (workspace_id, provider_subscription_id, provider_customer_id, plan, \"interval\", status, current_period_end) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(ws_id)
    .bind(format!("sub_{}", Uuid::new_v4()))
    .bind(Option::<String>::None)
    .bind(plan)
    .bind(interval)
    .bind(status)
    .bind(period_end)
    .execute(&state.pool)
    .await
    .unwrap();
}

/// Inserts a `blobs` row with a synthetic hash and the given size, to inflate the
/// workspace's stored-bytes total without uploading real data.
async fn insert_fake_blob(state: &AppState, ws_id: Uuid, tag: u8, size: i64) {
    sqlx::query("INSERT INTO blobs (workspace_id, hash, size) VALUES ($1, $2, $3)")
        .bind(ws_id)
        .bind(vec![tag; 32])
        .bind(size)
        .execute(&state.pool)
        .await
        .unwrap();
}

async fn set_created_at(state: &AppState, slug: &str, when: DateTime<Utc>) {
    sqlx::query("UPDATE workspaces SET created_at = $2 WHERE slug = $1")
        .bind(slug)
        .bind(when)
        .execute(&state.pool)
        .await
        .unwrap();
}

// --- webhook signing -------------------------------------------------------

/// The `Stripe-Signature` header for a body, signed with `WEBHOOK_SECRET` (used
/// verbatim as the raw-ASCII key) over `{ts}.{body}`, hex-encoded, at `ts`.
fn signed_headers(ts: i64, body: &[u8]) -> Vec<(&'static str, String)> {
    let mut signed = Vec::new();
    signed.extend_from_slice(ts.to_string().as_bytes());
    signed.push(b'.');
    signed.extend_from_slice(body);
    let sig = hex(&hmac_sha256(WEBHOOK_SECRET.as_bytes(), &signed));
    vec![("stripe-signature", format!("t={ts},v1={sig}"))]
}

/// The Unix seconds for a fixed future instant used as `current_period_end`.
const PERIOD_END_UNIX: i64 = 1_893_456_000; // 2030-01-01T00:00:00Z

/// A realistic Stripe `customer.subscription.*` event whose subscription carries
/// a single plan-price line item, with the workspace id in `metadata`. `evt_id`
/// is the Stripe event id (the `billing_events` idempotency key).
fn subscription_event(
    evt_id: &str,
    event_type: &str,
    ws_id: Uuid,
    price_id: &str,
    status: &str,
    sub_id: &str,
) -> Value {
    subscription_event_items(
        evt_id,
        event_type,
        ws_id,
        status,
        sub_id,
        json!([{ "price": { "id": price_id }, "quantity": 1 }]),
    )
}

/// Like [`subscription_event`] but with an explicit `items.data` array, so a
/// storage-only or mixed subscription can be constructed.
fn subscription_event_items(
    evt_id: &str,
    event_type: &str,
    ws_id: Uuid,
    status: &str,
    sub_id: &str,
    items: Value,
) -> Value {
    json!({
        "id": evt_id,
        "type": event_type,
        "data": {
            "object": {
                "id": sub_id,
                "object": "subscription",
                "status": status,
                "customer": "cus_test_123",
                "current_period_end": PERIOD_END_UNIX,
                "items": { "object": "list", "data": items },
                "metadata": { "workspace_id": ws_id.to_string() }
            }
        }
    })
}

async fn post_webhook(
    client: &mut Client,
    headers: &[(&str, String)],
    body: &[u8],
) -> (StatusCode, Vec<u8>) {
    client
        .raw(
            Method::POST,
            "/api/billing/webhook",
            Some("application/json"),
            body.to_vec(),
            None,
            headers,
        )
        .await
}

// ===========================================================================
// Config-disabled behavior: unlimited + checkout/webhook 404
// ===========================================================================

#[tokio::test]
async fn billing_disabled_is_unlimited_and_routes_404() {
    let Some(ctx) = setup(None).await else {
        return;
    };
    let (mut owner, ws, _token) = bootstrap(&ctx.state).await;

    // Status endpoint works for members and reports unlimited.
    let (status, body) = owner.get(&format!("/api/workspaces/{ws}/billing")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], json!(false));
    assert_eq!(s(&body["plan"]), "unlimited");
    assert_eq!(body["limits"]["max_members"], json!(null));
    assert_eq!(body["limits"]["max_storage_bytes"], json!(null));

    // Checkout 404s when billing is off (like the GitHub routes).
    let (status, _) = owner
        .post(
            &format!("/api/workspaces/{ws}/billing/checkout"),
            json!({ "plan": "solo", "interval": "monthly" }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Webhook 404s too — no signature check even reached.
    let (status, _) = post_webhook(&mut owner, &[], b"{}").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ===========================================================================
// Plan matrix (billing enabled)
// ===========================================================================

#[tokio::test]
async fn plan_matrix_resolves_expected_limits() {
    let Some(ctx) = setup(Some(stripe_config())).await else {
        return;
    };

    // Fresh workspace, no subscription → Trial (2 GiB, 3 members).
    let (mut owner, ws, _t) = bootstrap(&ctx.state).await;
    let (status, body) = owner.get(&format!("/api/workspaces/{ws}/billing")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], json!(true));
    assert_eq!(s(&body["plan"]), "trial");
    assert_eq!(body["status"], json!(null));
    assert_eq!(body["limits"]["max_members"], json!(3));
    assert_eq!(
        body["limits"]["max_storage_bytes"],
        json!(2u64 * 1024 * 1024 * 1024)
    );
    // The trial's current_period_end surfaces the trial's expiry.
    assert!(body["current_period_end"].is_string());

    // trialing subscription → its plan (team).
    let ws_id = workspace_id(&ctx.state, &ws).await;
    insert_subscription(
        &ctx.state,
        ws_id,
        "team",
        "yearly",
        "trialing",
        Some(Utc::now() + Duration::days(20)),
    )
    .await;
    let (_, body) = owner.get(&format!("/api/workspaces/{ws}/billing")).await;
    assert_eq!(s(&body["plan"]), "team");
    assert_eq!(s(&body["status"]), "trialing");
    assert_eq!(s(&body["interval"]), "yearly");
    assert_eq!(body["limits"]["max_members"], json!(10));

    // active solo → solo limits.
    let (mut solo_owner, solo_ws, _t) = bootstrap(&ctx.state).await;
    let solo_id = workspace_id(&ctx.state, &solo_ws).await;
    insert_subscription(
        &ctx.state,
        solo_id,
        "solo",
        "monthly",
        "active",
        Some(Utc::now() + Duration::days(30)),
    )
    .await;
    let (_, body) = solo_owner
        .get(&format!("/api/workspaces/{solo_ws}/billing"))
        .await;
    assert_eq!(s(&body["plan"]), "solo");
    assert_eq!(body["limits"]["max_members"], json!(1));
    assert_eq!(
        body["limits"]["max_storage_bytes"],
        json!(10u64 * 1024 * 1024 * 1024)
    );

    // past_due within the 7-day grace → keeps its plan.
    let (mut grace_owner, grace_ws, _t) = bootstrap(&ctx.state).await;
    let grace_id = workspace_id(&ctx.state, &grace_ws).await;
    insert_subscription(
        &ctx.state,
        grace_id,
        "team",
        "monthly",
        "past_due",
        Some(Utc::now() - Duration::days(1)),
    )
    .await;
    let (_, body) = grace_owner
        .get(&format!("/api/workspaces/{grace_ws}/billing"))
        .await;
    assert_eq!(
        s(&body["plan"]),
        "team",
        "past_due within grace keeps the plan"
    );
    assert_eq!(s(&body["status"]), "past_due");
}

#[tokio::test]
async fn expired_trial_blocks_writes_with_upgrade_required() {
    let Some(ctx) = setup(Some(stripe_config())).await else {
        return;
    };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    // Push the workspace's creation 15 days back → trial lapsed, no subscription.
    set_created_at(&ctx.state, &ws, Utc::now() - Duration::days(15)).await;

    let (_, body) = owner.get(&format!("/api/workspaces/{ws}/billing")).await;
    assert_eq!(s(&body["plan"]), "expired", "{body}");

    // A blob PUT (a write) is refused with 403 upgrade_required.
    let bytes = b"hello world";
    let hash = sha_hex(bytes);
    let (status, err) = owner
        .raw(
            Method::PUT,
            &format!("/api/workspaces/{ws}/games/demo/blobs/{hash}"),
            Some("application/octet-stream"),
            bytes.to_vec(),
            Some(&token),
            &[],
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "{:?}",
        String::from_utf8_lossy(&err)
    );
    assert_eq!(s(&parse_json(&err)["error"]["code"]), "upgrade_required");
}

// ===========================================================================
// Trial workspace cap (one free-trial workspace per user)
// ===========================================================================

#[tokio::test]
async fn trial_user_is_capped_at_one_workspace() {
    let Some(ctx) = setup(Some(stripe_config())).await else {
        return;
    };
    let mut owner = register(&ctx.state, &unique_email(), "Owner").await;

    // The one free-trial workspace: allowed.
    let (status, body) = owner
        .post(
            "/api/workspaces",
            json!({ "name": "First", "slug": unique_slug() }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // A second trial workspace for the same user: refused (no trial rotation).
    let (status, body) = owner
        .post(
            "/api/workspaces",
            json!({ "name": "Second", "slug": unique_slug() }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(s(&body["error"]["code"]), "trial_workspace_limit");
}

#[tokio::test]
async fn paid_workspace_lifts_the_trial_cap() {
    let Some(ctx) = setup(Some(stripe_config())).await else {
        return;
    };
    let mut owner = register(&ctx.state, &unique_email(), "Owner").await;
    let ws1 = unique_slug();
    let (status, _) = owner
        .post("/api/workspaces", json!({ "name": "Paid", "slug": &ws1 }))
        .await;
    assert_eq!(status, StatusCode::OK);

    // Put the first workspace on an active paid plan → the user is no longer a
    // pure free-trial user, so a second (trial) workspace becomes allowed.
    let ws1_id = workspace_id(&ctx.state, &ws1).await;
    insert_subscription(
        &ctx.state,
        ws1_id,
        "solo",
        "monthly",
        "active",
        Some(Utc::now() + Duration::days(30)),
    )
    .await;

    let (status, body) = owner
        .post(
            "/api/workspaces",
            json!({ "name": "Second", "slug": unique_slug() }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

// ===========================================================================
// Enforcement: member cap, storage cap, usage counts
// ===========================================================================

#[tokio::test]
async fn member_cap_blocks_second_accept_on_solo() {
    let Some(ctx) = setup(Some(stripe_config())).await else {
        return;
    };
    let (mut owner, ws, _t) = bootstrap(&ctx.state).await;
    let ws_id = workspace_id(&ctx.state, &ws).await;
    // Solo caps members at 1 — the owner already fills it.
    insert_subscription(
        &ctx.state,
        ws_id,
        "solo",
        "monthly",
        "active",
        Some(Utc::now() + Duration::days(30)),
    )
    .await;

    let (_, invite) = owner
        .post(
            &format!("/api/workspaces/{ws}/invites"),
            json!({ "role": "member" }),
        )
        .await;
    let token = s(&invite["token"]).to_string();

    let mut bob = register(&ctx.state, &unique_email(), "Bob").await;
    let (status, body) = bob
        .post(&format!("/api/invites/{token}/accept"), json!({}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(s(&body["error"]["code"]), "upgrade_required");
}

#[tokio::test]
async fn team_plan_allows_second_member_and_idempotent_reaccept() {
    let Some(ctx) = setup(Some(stripe_config())).await else {
        return;
    };
    let (mut owner, ws, _t) = bootstrap(&ctx.state).await;
    let ws_id = workspace_id(&ctx.state, &ws).await;
    insert_subscription(
        &ctx.state,
        ws_id,
        "team",
        "monthly",
        "active",
        Some(Utc::now() + Duration::days(30)),
    )
    .await;

    let (_, invite) = owner
        .post(
            &format!("/api/workspaces/{ws}/invites"),
            json!({ "role": "member" }),
        )
        .await;
    let token = s(&invite["token"]).to_string();

    let mut bob = register(&ctx.state, &unique_email(), "Bob").await;
    // First accept succeeds (team allows 10).
    let (status, _) = bob
        .post(&format!("/api/invites/{token}/accept"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    // Re-accepting as an existing member is idempotent (member cap not re-checked).
    let (status, _) = bob
        .post(&format!("/api/invites/{token}/accept"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn storage_cap_blocks_commit_over_quota() {
    let Some(ctx) = setup(Some(stripe_config())).await else {
        return;
    };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await; // fresh → Trial (2 GiB)
    let ws_id = workspace_id(&ctx.state, &ws).await;

    // Upload a real (tiny) index.json so the manifest's blob exists.
    let index = br#"{"modes":[]}"#;
    let hash = sha_hex(index);
    let (status, _) = owner
        .raw(
            Method::PUT,
            &format!("/api/workspaces/{ws}/games/demo/blobs/{hash}"),
            Some("application/octet-stream"),
            index.to_vec(),
            Some(&token),
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);

    // Inflate stored bytes past the 2 GiB trial cap with a fake 3 GiB blob row.
    insert_fake_blob(&ctx.state, ws_id, 0xAB, 3 * 1024 * 1024 * 1024).await;

    // Committing a revision now exceeds the cap → 413 storage_quota_exceeded.
    let manifest = json!([{ "path": "index.json", "hash": hash, "size": index.len() }]);
    let (status, body) = owner
        .send(
            Method::POST,
            &format!("/api/workspaces/{ws}/games/demo/revisions"),
            Some(json!({ "message": "over quota", "files": manifest })),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(s(&body["error"]["code"]), "storage_quota_exceeded");
}

#[tokio::test]
async fn storage_cap_blocks_upload_via_content_length() {
    let Some(ctx) = setup(Some(stripe_config())).await else {
        return;
    };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await; // Trial (2 GiB)
    let ws_id = workspace_id(&ctx.state, &ws).await;

    // Leave only 3 bytes of headroom under the 2 GiB cap.
    let cap = 2u64 * 1024 * 1024 * 1024;
    insert_fake_blob(&ctx.state, ws_id, 0xCD, (cap - 3) as i64).await;

    // A 5-byte upload with a declared Content-Length blows the remaining 3 bytes.
    let bytes = b"hello";
    let hash = sha_hex(bytes);
    let (status, body) = owner
        .raw(
            Method::PUT,
            &format!("/api/workspaces/{ws}/games/demo/blobs/{hash}"),
            Some("application/octet-stream"),
            bytes.to_vec(),
            Some(&token),
            &[("content-length", "5".to_string())],
        )
        .await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "{:?}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(
        s(&parse_json(&body)["error"]["code"]),
        "storage_quota_exceeded"
    );
}

#[tokio::test]
async fn storage_cap_blocks_upload_without_content_length() {
    let Some(ctx) = setup(Some(stripe_config())).await else {
        return;
    };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await; // Trial (2 GiB)
    let ws_id = workspace_id(&ctx.state, &ws).await;

    // Leave only 3 bytes of headroom under the 2 GiB cap.
    let cap = 2u64 * 1024 * 1024 * 1024;
    insert_fake_blob(&ctx.state, ws_id, 0xEF, (cap - 3) as i64).await;

    // A 5-byte upload with NO Content-Length (chunked). The pre-check can't fire,
    // so only the authoritative post-stream re-check stops it — regression guard
    // for the quota bypass. Before the fix this returned 201 and stored the blob.
    let bytes = b"hello";
    let hash = sha_hex(bytes);
    let (status, body) = owner
        .raw(
            Method::PUT,
            &format!("/api/workspaces/{ws}/games/demo/blobs/{hash}"),
            Some("application/octet-stream"),
            bytes.to_vec(),
            Some(&token),
            &[],
        )
        .await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "{:?}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(
        s(&parse_json(&body)["error"]["code"]),
        "storage_quota_exceeded"
    );

    // The over-quota blob must NOT have been recorded (the 5-byte upload; the
    // only other row is the multi-GiB fake blob).
    let recorded: i64 =
        sqlx::query_scalar("SELECT count(*) FROM blobs WHERE workspace_id = $1 AND size = 5")
            .bind(ws_id)
            .fetch_one(&ctx.state.pool)
            .await
            .unwrap();
    assert_eq!(recorded, 0, "over-quota blob must not be inserted");
}

#[tokio::test]
async fn usage_endpoint_counts_members_and_storage() {
    let Some(ctx) = setup(Some(stripe_config())).await else {
        return;
    };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;

    // Upload two distinct blobs (10 + 20 bytes).
    for bytes in [b"0123456789".as_slice(), b"0123456789abcdefghij".as_slice()] {
        let hash = sha_hex(bytes);
        let (status, _) = owner
            .raw(
                Method::PUT,
                &format!("/api/workspaces/{ws}/games/demo/blobs/{hash}"),
                Some("application/octet-stream"),
                bytes.to_vec(),
                Some(&token),
                &[],
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    // First (and only) usage read for this fresh workspace → computed live.
    let (status, body) = owner.get(&format!("/api/workspaces/{ws}/billing")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["usage"]["members"], json!(1));
    assert_eq!(body["usage"]["storage_bytes"], json!(30));
    assert_eq!(body["usage"]["active_share_links"], json!(0));
}

// ===========================================================================
// Checkout gating
// ===========================================================================

#[tokio::test]
async fn checkout_is_owner_only() {
    let Some(ctx) = setup(Some(stripe_config())).await else {
        return;
    };
    let (mut owner, ws, _t) = bootstrap(&ctx.state).await;
    let ws_id = workspace_id(&ctx.state, &ws).await;
    // Give room for a second member.
    insert_subscription(
        &ctx.state,
        ws_id,
        "team",
        "monthly",
        "active",
        Some(Utc::now() + Duration::days(30)),
    )
    .await;

    // Add a plain member.
    let (_, invite) = owner
        .post(
            &format!("/api/workspaces/{ws}/invites"),
            json!({ "role": "member" }),
        )
        .await;
    let invite_token = s(&invite["token"]).to_string();
    let mut member = register(&ctx.state, &unique_email(), "Member").await;
    let (status, _) = member
        .post(&format!("/api/invites/{invite_token}/accept"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);

    // A non-owner member cannot start checkout.
    let (status, body) = member
        .post(
            &format!("/api/workspaces/{ws}/billing/checkout"),
            json!({ "plan": "team", "interval": "monthly" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(s(&body["error"]["code"]), "forbidden");
}

// ===========================================================================
// Webhook
// ===========================================================================

#[tokio::test]
async fn webhook_verifies_and_upserts_subscription() {
    let Some(ctx) = setup(Some(stripe_config())).await else {
        return;
    };
    let (mut owner, ws, _t) = bootstrap(&ctx.state).await;
    let ws_id = workspace_id(&ctx.state, &ws).await;

    // provider_subscription_id is UNIQUE, so make it unique per run too.
    let sub_id = format!("sub-{}", Uuid::new_v4());
    let id = evt_id();
    let event = subscription_event(
        &id,
        "customer.subscription.created",
        ws_id,
        "price_solo_m",
        "active",
        &sub_id,
    );
    let body = serde_json::to_vec(&event).unwrap();
    let ts = Utc::now().timestamp();
    let headers = signed_headers(ts, &body);

    let (status, _) = post_webhook(&mut owner, &headers, &body).await;
    assert_eq!(status, StatusCode::OK);

    // Subscription upserted from the payload (price → solo/monthly).
    let row: (String, String, String, String) = sqlx::query_as(
        "SELECT plan, \"interval\", status, provider_subscription_id FROM subscriptions WHERE workspace_id = $1",
    )
    .bind(ws_id)
    .fetch_one(&ctx.state.pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            "solo".into(),
            "monthly".into(),
            "active".into(),
            sub_id.clone()
        )
    );

    // The event is recorded as processed.
    let processed: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT processed_at FROM billing_events WHERE id = $1")
            .bind(&id)
            .fetch_one(&ctx.state.pool)
            .await
            .unwrap();
    assert!(processed.is_some());

    // The status endpoint now reflects the plan (and no storage add-on).
    let (_, view) = owner.get(&format!("/api/workspaces/{ws}/billing")).await;
    assert_eq!(s(&view["plan"]), "solo");
    assert_eq!(s(&view["status"]), "active");
    assert_eq!(view["extra_storage_gib"], json!(0));

    // Replaying the same event id is an idempotent no-op (still 200).
    let (status, _) = post_webhook(&mut owner, &headers, &body).await;
    assert_eq!(status, StatusCode::OK);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM billing_events WHERE id = $1")
        .bind(&id)
        .fetch_one(&ctx.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn webhook_rejects_tampered_and_stale() {
    let Some(ctx) = setup(Some(stripe_config())).await else {
        return;
    };
    let (mut owner, ws, _t) = bootstrap(&ctx.state).await;
    let ws_id = workspace_id(&ctx.state, &ws).await;

    let event = subscription_event(
        &evt_id(),
        "customer.subscription.updated",
        ws_id,
        "price_team_y",
        "active",
        "sub_tamper",
    );
    let body = serde_json::to_vec(&event).unwrap();
    let ts = Utc::now().timestamp();

    // Signature computed over `body`, but a different body is sent → 401.
    let headers = signed_headers(ts, &body);
    let tampered = serde_json::to_vec(&subscription_event(
        &evt_id(),
        "customer.subscription.updated",
        ws_id,
        "price_team_y",
        "canceled",
        "sub_tamper",
    ))
    .unwrap();
    let (status, _) = post_webhook(&mut owner, &headers, &tampered).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A correctly-signed but stale (10 min old) timestamp → 401.
    let stale_ts = ts - 600;
    let stale_headers = signed_headers(stale_ts, &body);
    let (status, _) = post_webhook(&mut owner, &stale_headers, &body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Missing signature header → 401.
    let (status, _) = post_webhook(&mut owner, &[], &body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Nothing was written for the rejected events.
    let subs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM subscriptions WHERE workspace_id = $1")
            .bind(ws_id)
            .fetch_one(&ctx.state.pool)
            .await
            .unwrap();
    assert_eq!(subs, 0);
}

#[tokio::test]
async fn webhook_unknown_workspace_is_recorded_error_but_200() {
    let Some(ctx) = setup(Some(stripe_config())).await else {
        return;
    };
    let mut client = Client::new(&ctx.state);

    // Metadata points at a workspace that does not exist.
    let id = evt_id();
    let event = subscription_event(
        &id,
        "customer.subscription.created",
        Uuid::new_v4(),
        "price_solo_m",
        "active",
        "sub_orphan",
    );
    let body = serde_json::to_vec(&event).unwrap();
    let ts = Utc::now().timestamp();
    let headers = signed_headers(ts, &body);

    // Authentic but unprocessable → 200 (no poison-pill retry), error recorded.
    let (status, _) = post_webhook(&mut client, &headers, &body).await;
    assert_eq!(status, StatusCode::OK);

    let error: Option<String> =
        sqlx::query_scalar("SELECT error FROM billing_events WHERE id = $1")
            .bind(&id)
            .fetch_one(&ctx.state.pool)
            .await
            .unwrap();
    assert!(
        error.is_some(),
        "unknown workspace should be recorded as an error"
    );
}

// ===========================================================================
// Storage add-on
// ===========================================================================

#[tokio::test]
async fn webhook_storage_only_adds_storage_without_granting_a_plan() {
    let Some(ctx) = setup(Some(stripe_config())).await else {
        return;
    };
    let (mut owner, ws, _t) = bootstrap(&ctx.state).await; // fresh → Trial (2 GiB)
    let ws_id = workspace_id(&ctx.state, &ws).await;

    // A storage-only subscription: two units (+20 GiB), no plan price.
    let id = evt_id();
    let event = subscription_event_items(
        &id,
        "customer.subscription.created",
        ws_id,
        "active",
        &format!("sub-{}", Uuid::new_v4()),
        json!([{ "price": { "id": "price_storage" }, "quantity": 2 }]),
    );
    let body = serde_json::to_vec(&event).unwrap();
    let ts = Utc::now().timestamp();
    let (status, _) = post_webhook(&mut owner, &signed_headers(ts, &body), &body).await;
    assert_eq!(status, StatusCode::OK);

    // The plan stays Trial (storage_only never grants a plan), but the trial's
    // 2 GiB cap is lifted to 2 + 20 = 22 GiB and the add-on is surfaced.
    let (_, view) = owner.get(&format!("/api/workspaces/{ws}/billing")).await;
    assert_eq!(s(&view["plan"]), "trial", "{view}");
    assert_eq!(view["extra_storage_gib"], json!(20));
    assert_eq!(
        view["limits"]["max_storage_bytes"],
        json!(22u64 * 1024 * 1024 * 1024)
    );
}

#[tokio::test]
async fn storage_add_on_stacks_on_a_plan_and_is_removed_on_cancel() {
    let Some(ctx) = setup(Some(stripe_config())).await else {
        return;
    };
    let (mut owner, ws, _t) = bootstrap(&ctx.state).await;
    let ws_id = workspace_id(&ctx.state, &ws).await;

    // Plan subscription first (Solo, 10 GiB).
    let plan_evt = subscription_event(
        &evt_id(),
        "customer.subscription.created",
        ws_id,
        "price_solo_m",
        "active",
        &format!("sub-plan-{}", Uuid::new_v4()),
    );
    let plan_body = serde_json::to_vec(&plan_evt).unwrap();
    let ts = Utc::now().timestamp();
    let (status, _) = post_webhook(&mut owner, &signed_headers(ts, &plan_body), &plan_body).await;
    assert_eq!(status, StatusCode::OK);

    // A SEPARATE storage subscription (3 units = +30 GiB) upserts only the
    // storage column, leaving the plan row's plan/status intact.
    let storage_sub = format!("sub-stor-{}", Uuid::new_v4());
    let stor_evt = subscription_event_items(
        &evt_id(),
        "customer.subscription.created",
        ws_id,
        "active",
        &storage_sub,
        json!([{ "price": { "id": "price_storage" }, "quantity": 3 }]),
    );
    let stor_body = serde_json::to_vec(&stor_evt).unwrap();
    let (status, _) = post_webhook(&mut owner, &signed_headers(ts, &stor_body), &stor_body).await;
    assert_eq!(status, StatusCode::OK);

    // Solo (10 GiB) + 30 GiB = 40 GiB; plan still Solo.
    let (_, view) = owner.get(&format!("/api/workspaces/{ws}/billing")).await;
    assert_eq!(s(&view["plan"]), "solo", "{view}");
    assert_eq!(s(&view["status"]), "active");
    assert_eq!(view["extra_storage_gib"], json!(30));
    assert_eq!(
        view["limits"]["max_storage_bytes"],
        json!(40u64 * 1024 * 1024 * 1024)
    );

    // Canceling the storage subscription drops the add-on to 0 but keeps Solo.
    let cancel_evt = subscription_event_items(
        &evt_id(),
        "customer.subscription.deleted",
        ws_id,
        "canceled",
        &storage_sub,
        json!([{ "price": { "id": "price_storage" }, "quantity": 3 }]),
    );
    let cancel_body = serde_json::to_vec(&cancel_evt).unwrap();
    let (status, _) =
        post_webhook(&mut owner, &signed_headers(ts, &cancel_body), &cancel_body).await;
    assert_eq!(status, StatusCode::OK);

    let (_, view) = owner.get(&format!("/api/workspaces/{ws}/billing")).await;
    assert_eq!(s(&view["plan"]), "solo");
    assert_eq!(view["extra_storage_gib"], json!(0));
    assert_eq!(
        view["limits"]["max_storage_bytes"],
        json!(10u64 * 1024 * 1024 * 1024)
    );
}

#[tokio::test]
async fn storage_checkout_validates_bounds_and_owner() {
    let Some(ctx) = setup(Some(stripe_config())).await else {
        return;
    };
    let (mut owner, ws, _t) = bootstrap(&ctx.state).await;

    // Out-of-range unit counts are rejected before any Stripe call (owner path).
    for units in [0, 101, -5] {
        let (status, body) = owner
            .post(
                &format!("/api/workspaces/{ws}/billing/storage"),
                json!({ "units": units }),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "units={units}: {body}");
        assert_eq!(s(&body["error"]["code"]), "invalid_units");
    }

    // A non-owner member cannot start a storage checkout even with valid units.
    let ws_id = workspace_id(&ctx.state, &ws).await;
    insert_subscription(
        &ctx.state,
        ws_id,
        "team",
        "monthly",
        "active",
        Some(Utc::now() + Duration::days(30)),
    )
    .await;
    let (_, invite) = owner
        .post(
            &format!("/api/workspaces/{ws}/invites"),
            json!({ "role": "member" }),
        )
        .await;
    let invite_token = s(&invite["token"]).to_string();
    let mut member = register(&ctx.state, &unique_email(), "Member").await;
    let (status, _) = member
        .post(&format!("/api/invites/{invite_token}/accept"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = member
        .post(
            &format!("/api/workspaces/{ws}/billing/storage"),
            json!({ "units": 2 }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(s(&body["error"]["code"]), "forbidden");
}

#[tokio::test]
async fn storage_checkout_404s_when_billing_disabled() {
    let Some(ctx) = setup(None).await else {
        return;
    };
    let (mut owner, ws, _t) = bootstrap(&ctx.state).await;
    let (status, _) = owner
        .post(
            &format!("/api/workspaces/{ws}/billing/storage"),
            json!({ "units": 2 }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

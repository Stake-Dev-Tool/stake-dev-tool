//! Hardening tests for the login/registration rate limiters and the
//! trusted-proxy `X-Forwarded-For` decision (2026-07-22 security audit).
//!
//! Every DB-backed test self-skips when `TEST_DATABASE_URL` is unset, matching
//! the rest of `crates/server/tests`. Requests are built by hand (rather than via
//! a cookie `Client`) so a test can attach a synthetic socket peer via
//! [`ConnectInfo`] and a spoofed `X-Forwarded-For`, exactly as the audit requires.

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use serde_json::json;
use server::config::{Config, StorageConfig, TrustedProxies};
use server::{AppState, db, http, storage};
use tower::ServiceExt;
use uuid::Uuid;

struct Ctx {
    state: AppState,
    _tmp: tempfile::TempDir,
}

async fn setup(trusted: &str) -> Option<Ctx> {
    let database_url = std::env::var("TEST_DATABASE_URL").ok()?;
    let tmp = tempfile::tempdir().unwrap();
    let config = Config {
        bind_addr: "127.0.0.1:0".to_string(),
        database_url: database_url.clone(),
        storage: StorageConfig::Fs {
            root: tmp.path().to_path_buf(),
        },
        cookie_secure: false,
        public_url: None,
        github: None,
        discord: None,
        mail: None,
        stripe: None,
        web_dir: None,
        storage_max_blob_bytes: 8_589_934_592,
        server_math_cache_bytes: 21_474_836_480,
        server_tenant_books_cap_bytes: None,
        play_domain: None,
        admin_emails: Vec::new(),
        trusted_proxies: TrustedProxies::parse(trusted).unwrap(),
    };
    let pool = db::connect_lazy(&database_url).expect("lazy pool");
    let store = storage::build_object_store(&config).expect("fs store");
    let state = AppState::new(config, pool, store);
    db::migrate(&state.pool).await.expect("migrations apply");
    Some(Ctx { state, _tmp: tmp })
}

fn unique_email() -> String {
    format!("user-{}@example.com", Uuid::new_v4())
}

/// Drive one request through the real router, optionally attaching a socket peer
/// (`ConnectInfo`) and/or an `X-Forwarded-For` header.
async fn send(
    state: &AppState,
    method: Method,
    uri: &str,
    body: serde_json::Value,
    peer: Option<&str>,
    xff: Option<&str>,
) -> StatusCode {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(xff) = xff {
        builder = builder.header("x-forwarded-for", xff);
    }
    let mut req = builder
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    if let Some(peer) = peer {
        let addr: SocketAddr = peer.parse().unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));
    }
    http::build_router(state.clone())
        .oneshot(req)
        .await
        .unwrap()
        .status()
}

async fn login(state: &AppState, email: &str, peer: Option<&str>, xff: Option<&str>) -> StatusCode {
    send(
        state,
        Method::POST,
        "/api/auth/login",
        json!({ "email": email, "password": "wrong-password" }),
        peer,
        xff,
    )
    .await
}

/// Register a real account so login attempts are "wrong password" rather than
/// "unknown email" (both are a uniform 401, but this keeps the intent clear).
async fn register_target(state: &AppState, email: &str) {
    let status = send(
        state,
        Method::POST,
        "/api/auth/register",
        json!({ "email": email, "password": "password123", "display_name": "Target" }),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "register target");
}

// ===========================================================================
// X-Forwarded-For trust
// ===========================================================================

#[tokio::test]
async fn xff_from_untrusted_peer_is_ignored_and_keys_on_socket_addr() {
    // No trusted proxies configured → the socket peer is authoritative.
    let Some(ctx) = setup("").await else {
        return;
    };
    let email = unique_email();
    register_target(&ctx.state, &email).await;

    // Ten failures from ONE socket peer, each carrying a different (spoofed) XFF.
    // If the spoof were honored, each would land in its own bucket and never
    // trip; because it's ignored, they all key on 203.0.113.9 and exhaust it.
    for i in 0..10 {
        let xff = format!("{i}.{i}.{i}.{i}");
        let status = login(&ctx.state, &email, Some("203.0.113.9:5000"), Some(&xff)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    // 11th, still from the same peer with yet another fresh XFF → throttled.
    let status = login(
        &ctx.state,
        &email,
        Some("203.0.113.9:5000"),
        Some("9.9.9.9"),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn xff_from_trusted_proxy_is_honored() {
    // The loopback proxy (our Caddy layout) is trusted, so XFF becomes the key.
    let Some(ctx) = setup("127.0.0.1/32").await else {
        return;
    };
    let email = unique_email();
    register_target(&ctx.state, &email).await;

    // Ten failures for one forwarded client exhaust ITS bucket...
    for i in 0..10 {
        let status = login(&ctx.state, &email, Some("127.0.0.1:8080"), Some("1.1.1.1")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    let status = login(&ctx.state, &email, Some("127.0.0.1:8080"), Some("1.1.1.1")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "1.1.1.1 exhausted");

    // ...while a DIFFERENT forwarded client, via the same trusted proxy, is
    // unaffected — proving the key is the XFF value, not the shared socket peer.
    let status = login(&ctx.state, &email, Some("127.0.0.1:8080"), Some("2.2.2.2")).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "2.2.2.2 has its own budget"
    );
}

#[tokio::test]
async fn account_limiter_blocks_ip_rotation_against_one_account() {
    let Some(ctx) = setup("127.0.0.1/32").await else {
        return;
    };
    let email = unique_email();
    register_target(&ctx.state, &email).await;

    // 20 failures, EACH from a distinct forwarded client IP (so no per-(ip,email)
    // bucket ever reaches its own limit of 10). Must match MAX_FAILURES_PER_ACCOUNT.
    for i in 0..20 {
        let xff = format!("10.0.{}.{}", i / 256, i % 256);
        let status = login(&ctx.state, &email, Some("127.0.0.1:8080"), Some(&xff)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    // A 21st attempt from a brand-new IP is throttled by the per-account limiter,
    // even though that IP's own bucket is empty.
    let status = login(
        &ctx.state,
        &email,
        Some("127.0.0.1:8080"),
        Some("172.16.9.9"),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

// ===========================================================================
// register / device rate limiting (per IP)
// ===========================================================================

#[tokio::test]
async fn register_is_rate_limited_per_ip() {
    let Some(ctx) = setup("").await else {
        return;
    };
    let peer = "198.51.100.5:4000";

    // 30 attempts fill the per-IP budget. Use a weak password so each is rejected
    // BEFORE any DB write (the limiter still counts it — the cap is on requests).
    for i in 0..30 {
        let status = send(
            &ctx.state,
            Method::POST,
            "/api/auth/register",
            json!({ "email": unique_email(), "password": "x", "display_name": "N" }),
            Some(peer),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "attempt {i}");
    }
    // 31st from the same IP → throttled before validation.
    let status = send(
        &ctx.state,
        Method::POST,
        "/api/auth/register",
        json!({ "email": unique_email(), "password": "x", "display_name": "N" }),
        Some(peer),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    // A different IP still has its full budget.
    let status = send(
        &ctx.state,
        Method::POST,
        "/api/auth/register",
        json!({ "email": unique_email(), "password": "x", "display_name": "N" }),
        Some("198.51.100.6:4000"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn device_code_is_rate_limited_per_ip() {
    let Some(ctx) = setup("").await else {
        return;
    };
    let peer = "198.51.100.9:4000";

    // Cheaply exhaust the shared per-IP budget via weak-password registrations
    // (register and device-code start share one per-IP limiter).
    for _ in 0..30 {
        let _ = send(
            &ctx.state,
            Method::POST,
            "/api/auth/register",
            json!({ "email": unique_email(), "password": "x", "display_name": "N" }),
            Some(peer),
            None,
        )
        .await;
    }

    // Device-code start from the exhausted IP is throttled...
    let status = send(
        &ctx.state,
        Method::POST,
        "/api/auth/device/code",
        json!({}),
        Some(peer),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    // ...but a fresh IP can still start a pairing.
    let status = send(
        &ctx.state,
        Method::POST,
        "/api/auth/device/code",
        json!({}),
        Some("198.51.100.10:4000"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

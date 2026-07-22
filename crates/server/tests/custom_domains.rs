//! Integration tests for workspace custom play domains: the owner-only PUT
//! endpoint (+ validation), the `custom` URL a share reports, Host-based serving
//! on `<slug>.<custom_domain>` with per-workspace scoping, and the unauthenticated
//! `/api/tls-check` on-demand-TLS ask endpoint. Driven through the HTTP router
//! with `Host` headers (no DNS). Every test self-skips without `TEST_DATABASE_URL`;
//! the dev database persists, so emails/slugs/domains are UUID-suffixed.
//!
//! The seeding harness (math + front bundle push) mirrors `tests/share.rs`.

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use serde_json::{Value, json};
use server::config::{Config, StorageConfig};
use server::{AppState, db, http, storage};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

const GAME: &str = "demo";
const PLAY_DOMAIN: &str = "play.test";

// --- fixtures (mirror tests/share.rs) ---------------------------------------

const INDEX_1MODE: &[u8] =
    br#"{"modes":[{"name":"base","cost":1,"events":"books_base.zst","weights":"lookup_base.csv"}]}"#;
const LOOKUP: &[u8] = b"0,5000,0\n1,5000,200\n";
const BOOKS_ZST: &[u8] = &[
    40, 181, 47, 253, 32, 58, 149, 1, 0, 148, 2, 123, 34, 105, 100, 34, 58, 48, 44, 34, 101, 118,
    101, 110, 116, 115, 34, 58, 91, 93, 125, 10, 49, 123, 34, 114, 101, 118, 101, 97, 108, 34, 58,
    34, 119, 105, 110, 34, 125, 93, 125, 10, 2, 0, 192, 136, 56, 76, 38,
];
const INDEX_HTML: &[u8] =
    b"<!doctype html><title>demo</title><div id=app></div><script src=\"/app.js\"></script>";
const APP_JS: &[u8] = b"console.log('demo share');\n";

fn rev_files() -> [(&'static str, &'static [u8]); 3] {
    [
        ("index.json", INDEX_1MODE),
        ("lookup_base.csv", LOOKUP),
        ("books_base.zst", BOOKS_ZST),
    ]
}

fn bundle_files() -> [(&'static str, &'static [u8]); 2] {
    [("index.html", INDEX_HTML), ("app.js", APP_JS)]
}

// --- setup ------------------------------------------------------------------

struct Ctx {
    state: AppState,
    _tmp: tempfile::TempDir,
}

async fn setup() -> Option<Ctx> {
    let database_url = std::env::var("TEST_DATABASE_URL").ok()?;
    let tmp = tempfile::tempdir().unwrap();
    let config = Config {
        bind_addr: "127.0.0.1:0".to_string(),
        database_url: database_url.clone(),
        storage: StorageConfig::Fs {
            root: tmp.path().join("blobs"),
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
        play_domain: Some(PLAY_DOMAIN.to_string()),
        admin_emails: Vec::new(),
    };
    let pool = db::connect_lazy(&database_url).expect("lazy pool");
    let store = storage::build_object_store(&config).expect("fs store");
    let state = AppState::new(config, pool, store);
    db::migrate(&state.pool).await.expect("migrations apply");
    Some(Ctx { state, _tmp: tmp })
}

// --- HTTP harness -----------------------------------------------------------

struct Resp {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    fn set_cookie(&self) -> Option<String> {
        self.headers
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    }
}

#[allow(clippy::too_many_arguments)]
async fn send(
    state: &AppState,
    method: Method,
    host: Option<&str>,
    uri: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
    bearer: Option<&str>,
    cookie: Option<&str>,
) -> Resp {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(host) = host {
        builder = builder.header("host", host);
    }
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    if let Some(ct) = content_type {
        builder = builder.header("content-type", ct);
    }
    let request = builder
        .body(if body.is_empty() {
            Body::empty()
        } else {
            Body::from(body)
        })
        .unwrap();
    let response = http::build_router(state.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    Resp {
        status,
        headers,
        body,
    }
}

/// App-host JSON API call (bearer).
async fn api(
    state: &AppState,
    method: Method,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> Resp {
    let (ct, raw) = match body {
        Some(v) => (Some("application/json"), serde_json::to_vec(&v).unwrap()),
        None => (None, Vec::new()),
    };
    send(state, method, None, uri, ct, raw, bearer, None).await
}

async fn share_get(state: &AppState, host: &str, uri: &str) -> Resp {
    send(
        state,
        Method::GET,
        Some(host),
        uri,
        None,
        Vec::new(),
        None,
        None,
    )
    .await
}

async fn share_post_json(state: &AppState, host: &str, uri: &str, body: Value) -> Resp {
    send(
        state,
        Method::POST,
        Some(host),
        uri,
        Some("application/json"),
        serde_json::to_vec(&body).unwrap(),
        None,
        None,
    )
    .await
}

// --- seeding ----------------------------------------------------------------

fn sha_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn manifest(files: &[(&str, &[u8])]) -> Value {
    Value::Array(
        files
            .iter()
            .map(|(p, b)| json!({ "path": p, "hash": sha_hex(b), "size": b.len() }))
            .collect(),
    )
}

fn unique_email() -> String {
    format!("user-{}@example.com", Uuid::new_v4())
}

fn unique_slug() -> String {
    format!("ws-{}", Uuid::new_v4())
}

/// A fresh, syntactically valid custom domain that won't collide across runs.
fn unique_domain() -> String {
    format!("d{}.example.com", Uuid::new_v4().simple())
}

/// Register a fresh owner and return a `push:math` PAT (owner role in any
/// workspace they create satisfies the owner-only domain endpoint too).
async fn owner_token(state: &AppState) -> String {
    let email = unique_email();
    let reg = api(
        state,
        Method::POST,
        "/api/auth/register",
        Some(json!({ "email": email, "password": "password123", "display_name": "Owner" })),
        None,
    )
    .await;
    assert_eq!(reg.status, StatusCode::OK, "register: {:?}", reg.json());
    let session = reg.set_cookie().expect("session cookie");
    let cookie = session.split(';').next().unwrap().to_string();
    let created = send(
        state,
        Method::POST,
        None,
        "/api/tokens",
        Some("application/json"),
        serde_json::to_vec(&json!({ "name": "ci", "scopes": ["push:math"] })).unwrap(),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(
        created.status,
        StatusCode::OK,
        "mint token: {:?}",
        created.json()
    );
    created.json()["token"].as_str().unwrap().to_string()
}

/// Register a fresh user and return their session cookie (implicit `full` scope,
/// so it can accept invites and act as a member).
async fn register_session(state: &AppState) -> String {
    let email = unique_email();
    let reg = api(
        state,
        Method::POST,
        "/api/auth/register",
        Some(json!({ "email": email, "password": "password123", "display_name": "Member" })),
        None,
    )
    .await;
    assert_eq!(
        reg.status,
        StatusCode::OK,
        "register member: {:?}",
        reg.json()
    );
    let session = reg.set_cookie().expect("session cookie");
    session.split(';').next().unwrap().to_string()
}

async fn create_workspace(state: &AppState, token: &str) -> String {
    let ws = unique_slug();
    let resp = api(
        state,
        Method::POST,
        "/api/workspaces",
        Some(json!({ "name": "Domain WS", "slug": ws })),
        Some(token),
    )
    .await;
    assert_eq!(resp.status, StatusCode::OK, "create ws: {:?}", resp.json());
    ws
}

async fn push_math(state: &AppState, ws: &str, token: &str) {
    let files = rev_files();
    let m = manifest(&files);
    let check = api(
        state,
        Method::POST,
        &format!("/api/workspaces/{ws}/games/{GAME}/revisions/check"),
        Some(json!({ "files": m })),
        Some(token),
    )
    .await;
    assert_eq!(
        check.status,
        StatusCode::OK,
        "math check: {:?}",
        check.json()
    );
    upload_missing(state, ws, &check.json(), &files, token).await;
    let commit = api(
        state,
        Method::POST,
        &format!("/api/workspaces/{ws}/games/{GAME}/revisions"),
        Some(json!({ "message": "rev", "files": m })),
        Some(token),
    )
    .await;
    assert_eq!(
        commit.status,
        StatusCode::CREATED,
        "math commit: {:?}",
        commit.json()
    );
}

async fn push_bundle(state: &AppState, ws: &str, token: &str) {
    let files = bundle_files();
    let m = manifest(&files);
    let check = api(
        state,
        Method::POST,
        &format!("/api/workspaces/{ws}/games/{GAME}/front-bundles/check"),
        Some(json!({ "files": m })),
        Some(token),
    )
    .await;
    assert_eq!(
        check.status,
        StatusCode::OK,
        "bundle check: {:?}",
        check.json()
    );
    upload_missing(state, ws, &check.json(), &files, token).await;
    let commit = api(
        state,
        Method::POST,
        &format!("/api/workspaces/{ws}/games/{GAME}/front-bundles"),
        Some(json!({ "files": m })),
        Some(token),
    )
    .await;
    assert_eq!(
        commit.status,
        StatusCode::CREATED,
        "bundle commit: {:?}",
        commit.json()
    );
}

async fn upload_missing(
    state: &AppState,
    ws: &str,
    check: &Value,
    files: &[(&str, &[u8])],
    token: &str,
) {
    let missing: Vec<String> = check["missing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    for (path, bytes) in files {
        let hash = sha_hex(bytes);
        if missing.contains(&hash) {
            let up = send(
                state,
                Method::PUT,
                None,
                &format!("/api/workspaces/{ws}/games/{GAME}/blobs/{hash}"),
                Some("application/octet-stream"),
                bytes.to_vec(),
                Some(token),
                None,
            )
            .await;
            assert_eq!(
                up.status,
                StatusCode::CREATED,
                "upload {path}: {:?}",
                up.json()
            );
        }
    }
}

/// Owner token + workspace with a pushed revision and bundle (a playable share).
async fn seed(state: &AppState) -> (String, String) {
    let token = owner_token(state).await;
    let ws = create_workspace(state, &token).await;
    push_math(state, &ws, &token).await;
    push_bundle(state, &ws, &token).await;
    (ws, token)
}

async fn create_share(state: &AppState, ws: &str, token: &str) -> Value {
    let resp = api(
        state,
        Method::POST,
        &format!("/api/workspaces/{ws}/games/{GAME}/shares"),
        Some(json!({})),
        Some(token),
    )
    .await;
    assert_eq!(
        resp.status,
        StatusCode::CREATED,
        "create share: {:?}",
        resp.json()
    );
    resp.json()
}

/// `PUT /workspaces/:ws/domain` with `domain` (a string or JSON null).
async fn put_domain(state: &AppState, ws: &str, token: &str, domain: Value) -> Resp {
    api(
        state,
        Method::PUT,
        &format!("/api/workspaces/{ws}/domain"),
        Some(json!({ "domain": domain })),
        Some(token),
    )
    .await
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

/// Only an owner can set the custom domain; a member gets 403.
#[tokio::test]
async fn put_domain_owner_only() {
    let Some(ctx) = setup().await else { return };
    let token = owner_token(&ctx.state).await;
    let ws = create_workspace(&ctx.state, &token).await;

    // Owner sets it.
    let domain = unique_domain();
    let ok = put_domain(&ctx.state, &ws, &token, json!(domain)).await;
    assert_eq!(ok.status, StatusCode::OK, "owner set: {:?}", ok.json());
    assert_eq!(ok.json()["domain"].as_str(), Some(domain.as_str()));

    // A member (invited + accepted) is forbidden.
    let inv = api(
        &ctx.state,
        Method::POST,
        &format!("/api/workspaces/{ws}/invites"),
        Some(json!({ "role": "member" })),
        Some(&token),
    )
    .await;
    assert_eq!(
        inv.status,
        StatusCode::OK,
        "create invite: {:?}",
        inv.json()
    );
    let invite_token = inv.json()["token"].as_str().unwrap().to_string();

    let member_cookie = register_session(&ctx.state).await;
    let accept = send(
        &ctx.state,
        Method::POST,
        None,
        &format!("/api/invites/{invite_token}/accept"),
        None,
        Vec::new(),
        None,
        Some(&member_cookie),
    )
    .await;
    assert_eq!(
        accept.status,
        StatusCode::OK,
        "accept invite: {:?}",
        accept.json()
    );

    let denied = send(
        &ctx.state,
        Method::PUT,
        None,
        &format!("/api/workspaces/{ws}/domain"),
        Some("application/json"),
        serde_json::to_vec(&json!({ "domain": unique_domain() })).unwrap(),
        None,
        Some(&member_cookie),
    )
    .await;
    assert_eq!(
        denied.status,
        StatusCode::FORBIDDEN,
        "member: {:?}",
        denied.json()
    );
}

/// Validation rejects a bad hostname, our own play-domain suffix, and an IP.
#[tokio::test]
async fn validation_rejects_bad_domains() {
    let Some(ctx) = setup().await else { return };
    let token = owner_token(&ctx.state).await;
    let ws = create_workspace(&ctx.state, &token).await;

    for bad in [
        json!("not_a_domain"),               // underscore / single label
        json!(format!("foo.{PLAY_DOMAIN}")), // nests under our play domain
        json!(PLAY_DOMAIN),                  // equals our play domain
        json!("1.2.3.4"),                    // IP literal
        json!("-bad.example.com"),           // leading hyphen
    ] {
        let resp = put_domain(&ctx.state, &ws, &token, bad.clone()).await;
        assert_eq!(
            resp.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{bad:?} should be rejected: {:?}",
            resp.json()
        );
        assert_eq!(
            resp.json()["error"]["code"].as_str(),
            Some("invalid_domain")
        );
    }
}

/// Two workspaces cannot claim the same domain — the second gets 409.
#[tokio::test]
async fn duplicate_domain_conflicts() {
    let Some(ctx) = setup().await else { return };
    let token1 = owner_token(&ctx.state).await;
    let ws1 = create_workspace(&ctx.state, &token1).await;
    let token2 = owner_token(&ctx.state).await;
    let ws2 = create_workspace(&ctx.state, &token2).await;

    let domain = unique_domain();
    let first = put_domain(&ctx.state, &ws1, &token1, json!(domain)).await;
    assert_eq!(first.status, StatusCode::OK, "first: {:?}", first.json());

    let second = put_domain(&ctx.state, &ws2, &token2, json!(domain)).await;
    assert_eq!(
        second.status,
        StatusCode::CONFLICT,
        "second: {:?}",
        second.json()
    );
    assert_eq!(
        second.json()["error"]["code"].as_str(),
        Some("domain_taken")
    );
}

/// A share in a domain-bearing workspace reports the custom URL.
#[tokio::test]
async fn share_reports_custom_url() {
    let Some(ctx) = setup().await else { return };
    let (ws, token) = seed(&ctx.state).await;
    let domain = unique_domain();
    let set = put_domain(&ctx.state, &ws, &token, json!(domain)).await;
    assert_eq!(set.status, StatusCode::OK, "set domain: {:?}", set.json());

    let share = create_share(&ctx.state, &ws, &token).await;
    let slug = share["slug"].as_str().unwrap();
    assert_eq!(
        share["url"].as_str(),
        Some(format!("https://{slug}.{domain}/").as_str()),
        "custom url: {:?}",
        share["url"]
    );
}

/// The custom host serves the bundle and drives the wallet; the platform play
/// domain still serves the same slug in parallel.
#[tokio::test]
async fn serves_on_custom_and_play_hosts() {
    let Some(ctx) = setup().await else { return };
    let (ws, token) = seed(&ctx.state).await;
    let domain = unique_domain();
    put_domain(&ctx.state, &ws, &token, json!(domain)).await;
    let share = create_share(&ctx.state, &ws, &token).await;
    let slug = share["slug"].as_str().unwrap();

    let custom_host = format!("{slug}.{domain}");
    let play_host = format!("{slug}.{PLAY_DOMAIN}");

    // Custom host serves the bundle index (sessionID present → no bootstrap
    // redirect).
    let idx = share_get(&ctx.state, &custom_host, "/?sessionID=probe").await;
    assert_eq!(
        idx.status,
        StatusCode::OK,
        "custom host index: {:?}",
        idx.status
    );
    assert_eq!(idx.body, INDEX_HTML);

    // Wallet authenticate works through the custom host.
    let auth = share_post_json(
        &ctx.state,
        &custom_host,
        &format!("/api/rgs/{GAME}/wallet/authenticate"),
        json!({ "sessionID": format!("vis-{}", Uuid::new_v4()), "language": "en" }),
    )
    .await;
    assert_eq!(
        auth.status,
        StatusCode::OK,
        "custom-host authenticate: {:?}",
        auth.json()
    );
    assert_eq!(
        auth.json()["balance"]["amount"].as_u64().unwrap(),
        10_000_000_000
    );

    // The platform play domain still serves the same slug.
    let via_play = share_get(&ctx.state, &play_host, "/?sessionID=probe").await;
    assert_eq!(
        via_play.status,
        StatusCode::OK,
        "play host index: {:?}",
        via_play.status
    );
    assert_eq!(via_play.body, INDEX_HTML);
}

/// Scoping: one workspace's slug served under another workspace's custom domain
/// is a branded 404 — the custom domain only serves its own workspace's links.
#[tokio::test]
async fn custom_domain_scopes_to_its_workspace() {
    let Some(ctx) = setup().await else { return };
    // ws1 owns the custom domain and a share.
    let (ws1, token1) = seed(&ctx.state).await;
    let domain = unique_domain();
    put_domain(&ctx.state, &ws1, &token1, json!(domain)).await;
    let share1 = create_share(&ctx.state, &ws1, &token1).await;
    let slug1 = share1["slug"].as_str().unwrap();

    // ws2 is a different workspace with its own share.
    let (ws2, token2) = seed(&ctx.state).await;
    let share2 = create_share(&ctx.state, &ws2, &token2).await;
    let slug2 = share2["slug"].as_str().unwrap();

    // ws1's own slug under ws1's domain resolves.
    let ok = share_get(
        &ctx.state,
        &format!("{slug1}.{domain}"),
        "/?sessionID=probe",
    )
    .await;
    assert_eq!(
        ok.status,
        StatusCode::OK,
        "own slug resolves: {:?}",
        ok.status
    );
    assert_eq!(ok.body, INDEX_HTML);

    // ws2's slug under ws1's domain is a branded 404 (scoping proof) — the
    // rejection happens at resolution, before any bootstrap redirect.
    let denied = share_get(&ctx.state, &format!("{slug2}.{domain}"), "/").await;
    assert_eq!(
        denied.status,
        StatusCode::NOT_FOUND,
        "cross-workspace: {:?}",
        denied.status
    );
    assert!(String::from_utf8_lossy(&denied.body).contains("Stake Dev Tool"));

    // Sanity: ws2's slug DOES resolve under the platform play domain (global).
    let global = share_get(
        &ctx.state,
        &format!("{slug2}.{PLAY_DOMAIN}"),
        "/?sessionID=probe",
    )
    .await;
    assert_eq!(
        global.status,
        StatusCode::OK,
        "ws2 slug via play domain: {:?}",
        global.status
    );
    assert_eq!(global.body, INDEX_HTML);
}

/// tls-check: 200 for any single label under a registered domain (a share need
/// not exist yet), 404 for unknown suffixes and a missing param.
#[tokio::test]
async fn tls_check_gates_on_registered_domain() {
    let Some(ctx) = setup().await else { return };
    let token = owner_token(&ctx.state).await;
    let ws = create_workspace(&ctx.state, &token).await;
    let domain = unique_domain();
    put_domain(&ctx.state, &ws, &token, json!(domain)).await;

    // Any single label under the registered domain is approved — even one with no
    // share behind it (certs warm up before slugs are created).
    let approved = api(
        &ctx.state,
        Method::GET,
        &format!("/api/tls-check?domain=whatever.{domain}"),
        None,
        None,
    )
    .await;
    assert_eq!(approved.status, StatusCode::OK, "approve registered suffix");

    // Two labels over the domain (not a single leading label) → 404.
    let two_labels = api(
        &ctx.state,
        Method::GET,
        &format!("/api/tls-check?domain=a.b.{domain}"),
        None,
        None,
    )
    .await;
    assert_eq!(two_labels.status, StatusCode::NOT_FOUND, "two-label prefix");

    // An unregistered suffix → 404.
    let unknown = api(
        &ctx.state,
        Method::GET,
        &format!(
            "/api/tls-check?domain=x.unreg-{}.example.com",
            Uuid::new_v4().simple()
        ),
        None,
        None,
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "unknown suffix");

    // Missing param → 404 (never a 5xx).
    let no_param = api(&ctx.state, Method::GET, "/api/tls-check", None, None).await;
    assert_eq!(
        no_param.status,
        StatusCode::NOT_FOUND,
        "missing domain param"
    );
}

/// Clearing the domain stops resolution immediately (the write busts the cache).
#[tokio::test]
async fn clearing_domain_stops_resolution() {
    let Some(ctx) = setup().await else { return };
    let (ws, token) = seed(&ctx.state).await;
    let domain = unique_domain();
    put_domain(&ctx.state, &ws, &token, json!(domain)).await;
    let share = create_share(&ctx.state, &ws, &token).await;
    let slug = share["slug"].as_str().unwrap();
    let host = format!("{slug}.{domain}");

    // Warm both the resolver (serves) and the tls-check (approves) caches.
    let before = share_get(&ctx.state, &host, "/?sessionID=probe").await;
    assert_eq!(before.status, StatusCode::OK);
    assert_eq!(before.body, INDEX_HTML);
    let tls_before = api(
        &ctx.state,
        Method::GET,
        &format!("/api/tls-check?domain={host}"),
        None,
        None,
    )
    .await;
    assert_eq!(tls_before.status, StatusCode::OK);

    // Clear the domain (null). The write busts the cache, so the change is
    // effective immediately rather than after the 60s TTL.
    let cleared = put_domain(&ctx.state, &ws, &token, json!(null)).await;
    assert_eq!(
        cleared.status,
        StatusCode::OK,
        "clear: {:?}",
        cleared.json()
    );
    assert!(cleared.json()["domain"].is_null());

    // The custom host no longer serves the bundle, and tls-check no longer
    // approves it.
    let after = share_get(&ctx.state, &host, "/?sessionID=probe").await;
    assert_ne!(
        after.body, INDEX_HTML,
        "custom host stopped serving the bundle"
    );
    let tls_after = api(
        &ctx.state,
        Method::GET,
        &format!("/api/tls-check?domain={host}"),
        None,
        None,
    )
    .await;
    assert_eq!(
        tls_after.status,
        StatusCode::NOT_FOUND,
        "tls-check stops approving"
    );
}

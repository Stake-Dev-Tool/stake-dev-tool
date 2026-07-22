//! M5 integration tests: share links on wildcard subdomains, driven through the
//! HTTP router with `Host` headers (no DNS needed). Every test self-skips when
//! `TEST_DATABASE_URL` is unset, so `cargo test` stays green with no database.
//! The dev database persists between runs, so emails and workspace slugs are
//! UUID-suffixed.
//!
//! Seeding reuses the math push flow (check → upload → commit) with the SAME real
//! zstd books fixture as `lgs_host.rs`, so wallet play through the share host
//! exercises the full decode → index → read-event path.

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

// --- math fixture (mirrors lgs_host.rs) -------------------------------------

const INDEX_1MODE: &[u8] =
    br#"{"modes":[{"name":"base","cost":1,"events":"books_base.zst","weights":"lookup_base.csv"}]}"#;
const INDEX_2MODE: &[u8] = br#"{"modes":[{"name":"base","cost":1,"events":"books_base.zst","weights":"lookup_base.csv"},{"name":"bonus","cost":100,"events":"books_base.zst","weights":"lookup_base.csv"}]}"#;
const LOOKUP: &[u8] = b"0,5000,0\n1,5000,200\n";
// Real zstd frame of two book rows (see lgs_host.rs).
const BOOKS_ZST: &[u8] = &[
    40, 181, 47, 253, 32, 58, 149, 1, 0, 148, 2, 123, 34, 105, 100, 34, 58, 48, 44, 34, 101, 118,
    101, 110, 116, 115, 34, 58, 91, 93, 125, 10, 49, 123, 34, 114, 101, 118, 101, 97, 108, 34, 58,
    34, 119, 105, 110, 34, 125, 93, 125, 10, 2, 0, 192, 136, 56, 76, 38,
];

// --- front-bundle fixture ---------------------------------------------------

const INDEX_HTML: &[u8] =
    b"<!doctype html><title>demo</title><div id=app></div><script src=\"/app.js\"></script>";
const APP_JS: &[u8] = b"console.log('demo share');\n";

fn rev_files(index: &'static [u8]) -> [(&'static str, &'static [u8]); 3] {
    [
        ("index.json", index),
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
        polar: None,
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
    fn content_type(&self) -> String {
        self.headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }
    fn cache_control(&self) -> String {
        self.headers
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
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

/// App-host JSON API call (no share Host → falls through to the app router).
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

/// Share-host GET.
async fn share_get(state: &AppState, host: &str, uri: &str, cookie: Option<&str>) -> Resp {
    send(
        state,
        Method::GET,
        Some(host),
        uri,
        None,
        Vec::new(),
        None,
        cookie,
    )
    .await
}

/// Share-host JSON POST (e.g. wallet calls).
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

/// Register an owner, mint a `push:math` PAT, and return the token (the owner is
/// owner-role in any workspace they create, so the token also passes share CRUD).
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
    // The registration set a session cookie; mint a PAT via that session.
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

async fn create_workspace(state: &AppState, token: &str) -> String {
    let ws = unique_slug();
    let resp = api(
        state,
        Method::POST,
        "/api/workspaces",
        Some(json!({ "name": "Share WS", "slug": ws })),
        Some(token),
    )
    .await;
    assert_eq!(resp.status, StatusCode::OK, "create ws: {:?}", resp.json());
    ws
}

/// Push a math revision (check → upload missing → commit).
async fn push_math(
    state: &AppState,
    ws: &str,
    files: &[(&str, &[u8])],
    parent: Option<i32>,
    token: &str,
) {
    let m = manifest(files);
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
    upload_missing(state, ws, &check.json(), files, token).await;
    let mut body = json!({ "message": "rev", "files": m });
    if let Some(pn) = parent {
        body["parent_number"] = json!(pn);
    }
    let commit = api(
        state,
        Method::POST,
        &format!("/api/workspaces/{ws}/games/{GAME}/revisions"),
        Some(body),
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

/// Push a front bundle (check → upload missing → commit); returns its id.
async fn push_bundle(state: &AppState, ws: &str, files: &[(&str, &[u8])], token: &str) -> String {
    let m = manifest(files);
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
    upload_missing(state, ws, &check.json(), files, token).await;
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
    commit.json()["id"].as_str().unwrap().to_string()
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

/// Full seed: owner token, workspace, rev1 (base) + rev2 (base+bonus), a bundle.
async fn seed(state: &AppState) -> (String, String) {
    let token = owner_token(state).await;
    let ws = create_workspace(state, &token).await;
    push_math(state, &ws, &rev_files(INDEX_1MODE), None, &token).await;
    push_math(state, &ws, &rev_files(INDEX_2MODE), Some(1), &token).await;
    push_bundle(state, &ws, &bundle_files(), &token).await;
    (ws, token)
}

async fn create_share(state: &AppState, ws: &str, token: &str, body: Value) -> Value {
    let resp = api(
        state,
        Method::POST,
        &format!("/api/workspaces/{ws}/games/{GAME}/shares"),
        Some(body),
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

fn host_for(slug: &str) -> String {
    format!("{slug}.{PLAY_DOMAIN}")
}

async fn counters(state: &AppState, id: &str) -> (i64, i64, f64, f64) {
    let id = Uuid::parse_str(id).unwrap();
    sqlx::query_as::<_, (i64, i64, f64, f64)>(
        "SELECT spins_count, sessions_count, total_bet::float8, total_win::float8 \
         FROM share_links WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .unwrap()
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

/// Static serving: `/` returns the bundle index.html, an asset round-trips with
/// the right content-type + immutable cache, and the app host is unaffected.
#[tokio::test]
async fn static_serving_and_app_host_unaffected() {
    let Some(ctx) = setup().await else { return };
    let (ws, token) = seed(&ctx.state).await;
    let share = create_share(&ctx.state, &ws, &token, json!({})).await;
    let host = host_for(share["slug"].as_str().unwrap());

    // The generated URL is absolute against the play domain.
    let url = share["url"].as_str().unwrap();
    assert!(
        url.starts_with("https://") && url.ends_with(&format!(".{PLAY_DOMAIN}/")),
        "{url}"
    );

    // `/` with a sessionID already present → index.html directly (a bare `/`
    // without one is 302'd through the front-contract bootstrap; see
    // `entry_redirect_injects_front_contract`).
    let root = share_get(&ctx.state, &host, "/?sessionID=probe", None).await;
    assert_eq!(root.status, StatusCode::OK);
    assert_eq!(root.body, INDEX_HTML);
    assert!(
        root.content_type().starts_with("text/html"),
        "{}",
        root.content_type()
    );
    assert_eq!(root.cache_control(), "no-cache");

    // asset round-trip
    let asset = share_get(&ctx.state, &host, "/app.js", None).await;
    assert_eq!(asset.status, StatusCode::OK);
    assert_eq!(asset.body, APP_JS);
    assert!(
        asset.content_type().starts_with("text/javascript"),
        "{}",
        asset.content_type()
    );
    assert!(
        asset.cache_control().contains("immutable"),
        "{}",
        asset.cache_control()
    );

    // unknown non-API path → SPA fallback (index.html, 200)
    let deep = share_get(&ctx.state, &host, "/some/deep/link", None).await;
    assert_eq!(deep.status, StatusCode::OK);
    assert_eq!(deep.body, INDEX_HTML);

    // The app host (no share Host) is unchanged: /healthz still answers.
    let health = api(&ctx.state, Method::GET, "/healthz", None, None).await;
    assert_eq!(health.status, StatusCode::OK);
    // An unknown share label 404s with the branded page (not the app).
    let unknown = share_get(&ctx.state, &host_for("nope-nobody"), "/", None).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert!(String::from_utf8_lossy(&unknown.body).contains("Stake Dev Tool"));
}

/// Wallet flow through the share host: authenticate → balance → play, with the
/// play folding into the link's counters (spins/sessions/bet).
#[tokio::test]
async fn wallet_flow_increments_counters() {
    let Some(ctx) = setup().await else { return };
    let (ws, token) = seed(&ctx.state).await;
    let share = create_share(&ctx.state, &ws, &token, json!({})).await;
    let host = host_for(share["slug"].as_str().unwrap());
    let id = share["id"].as_str().unwrap();

    let sid = format!("vis-{}", Uuid::new_v4());
    let auth = share_post_json(
        &ctx.state,
        &host,
        &format!("/api/rgs/{GAME}/wallet/authenticate"),
        json!({ "sessionID": sid, "language": "en" }),
    )
    .await;
    assert_eq!(
        auth.status,
        StatusCode::OK,
        "authenticate: {:?}",
        auth.json()
    );
    assert_eq!(
        auth.json()["balance"]["amount"].as_u64().unwrap(),
        10_000_000_000
    );

    let bal = share_post_json(
        &ctx.state,
        &host,
        &format!("/api/rgs/{GAME}/wallet/balance"),
        json!({ "sessionID": sid }),
    )
    .await;
    assert_eq!(bal.status, StatusCode::OK, "balance: {:?}", bal.json());

    let play = share_post_json(
        &ctx.state,
        &host,
        &format!("/api/rgs/{GAME}/wallet/play"),
        json!({ "sessionID": sid, "mode": "base", "amount": 1 }),
    )
    .await;
    assert_eq!(play.status, StatusCode::OK, "play: {:?}", play.json());
    assert_eq!(play.json()["round"]["mode"].as_str().unwrap(), "base");

    let (spins, sessions, bet, win) = counters(&ctx.state, id).await;
    assert_eq!(spins, 1, "one spin recorded");
    assert_eq!(sessions, 1, "one visitor session recorded");
    assert_eq!(bet, 1.0, "bet of 1 recorded");
    assert!(win == 0.0 || win == 2.0, "win is 0 (loss) or 2 (2x): {win}");
}

/// Pinned vs latest revision resolution, proven through mode availability: a
/// share pinned to rev1 (base only) cannot play `bonus`; a latest-tracking share
/// (rev2, base+bonus) can.
#[tokio::test]
async fn pinned_vs_latest_revision() {
    let Some(ctx) = setup().await else { return };
    let (ws, token) = seed(&ctx.state).await;

    let pinned = create_share(&ctx.state, &ws, &token, json!({ "revision_number": 1 })).await;
    let latest = create_share(&ctx.state, &ws, &token, json!({})).await;
    let pinned_host = host_for(pinned["slug"].as_str().unwrap());
    let latest_host = host_for(latest["slug"].as_str().unwrap());

    for host in [&pinned_host, &latest_host] {
        let sid = format!("vis-{}", Uuid::new_v4());
        let auth = share_post_json(
            &ctx.state,
            host,
            &format!("/api/rgs/{GAME}/wallet/authenticate"),
            json!({ "sessionID": sid, "language": "en" }),
        )
        .await;
        assert_eq!(auth.status, StatusCode::OK, "auth on {host}");
    }

    // Pinned rev1 has no `bonus` mode → play fails.
    let sid = format!("vis-{}", Uuid::new_v4());
    let _ = share_post_json(
        &ctx.state,
        &pinned_host,
        &format!("/api/rgs/{GAME}/wallet/authenticate"),
        json!({ "sessionID": sid, "language": "en" }),
    )
    .await;
    let bonus_pinned = share_post_json(
        &ctx.state,
        &pinned_host,
        &format!("/api/rgs/{GAME}/wallet/play"),
        json!({ "sessionID": sid, "mode": "bonus", "amount": 1 }),
    )
    .await;
    assert_ne!(
        bonus_pinned.status,
        StatusCode::OK,
        "bonus should fail on rev1"
    );

    // Latest rev2 has `bonus` → play succeeds.
    let sid = format!("vis-{}", Uuid::new_v4());
    let _ = share_post_json(
        &ctx.state,
        &latest_host,
        &format!("/api/rgs/{GAME}/wallet/authenticate"),
        json!({ "sessionID": sid, "language": "en" }),
    )
    .await;
    let bonus_latest = share_post_json(
        &ctx.state,
        &latest_host,
        &format!("/api/rgs/{GAME}/wallet/play"),
        json!({ "sessionID": sid, "mode": "bonus", "amount": 1 }),
    )
    .await;
    assert_eq!(
        bonus_latest.status,
        StatusCode::OK,
        "bonus should play on rev2: {:?}",
        bonus_latest.json()
    );
}

/// Revoked and expired links serve the branded 404/expired page, not content.
#[tokio::test]
async fn revoked_and_expired_are_not_found() {
    let Some(ctx) = setup().await else { return };
    let (ws, token) = seed(&ctx.state).await;

    // Revoked.
    let revoked = create_share(&ctx.state, &ws, &token, json!({})).await;
    let revoked_host = host_for(revoked["slug"].as_str().unwrap());
    let patch = api(
        &ctx.state,
        Method::PATCH,
        &format!(
            "/api/workspaces/{ws}/games/{GAME}/shares/{}",
            revoked["id"].as_str().unwrap()
        ),
        Some(json!({ "revoked": true })),
        Some(&token),
    )
    .await;
    assert_eq!(patch.status, StatusCode::OK, "revoke: {:?}", patch.json());
    let resp = share_get(&ctx.state, &revoked_host, "/", None).await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);

    // Already-expired (negative days).
    let expired = create_share(&ctx.state, &ws, &token, json!({ "expires_in_days": -1 })).await;
    let expired_host = host_for(expired["slug"].as_str().unwrap());
    let resp = share_get(&ctx.state, &expired_host, "/", None).await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
}

/// Password flow: locked → interstitial; wrong password rejected; correct
/// password sets the cookie and unlocks; RGS is blocked while locked.
#[tokio::test]
async fn password_gate() {
    let Some(ctx) = setup().await else { return };
    let (ws, token) = seed(&ctx.state).await;
    let share = create_share(&ctx.state, &ws, &token, json!({ "password": "s3cret" })).await;
    let host = host_for(share["slug"].as_str().unwrap());
    assert_eq!(share["password_protected"].as_bool(), Some(true));

    // Locked: `/` shows the interstitial (200 HTML, no content).
    let locked = share_get(&ctx.state, &host, "/", None).await;
    assert_eq!(locked.status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&locked.body).contains("Password required"));
    assert_ne!(locked.body, INDEX_HTML);

    // Locked: RGS is 401.
    let rgs = share_post_json(
        &ctx.state,
        &host,
        &format!("/api/rgs/{GAME}/wallet/authenticate"),
        json!({ "sessionID": "x", "language": "en" }),
    )
    .await;
    assert_eq!(rgs.status, StatusCode::UNAUTHORIZED);

    // Wrong password → form with error.
    let wrong = send(
        &ctx.state,
        Method::POST,
        Some(&host),
        "/__share/unlock",
        Some("application/x-www-form-urlencoded"),
        b"password=nope".to_vec(),
        None,
        None,
    )
    .await;
    assert_eq!(wrong.status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&wrong.body).contains("Incorrect password"));

    // Correct password → 303 + Set-Cookie.
    let ok = send(
        &ctx.state,
        Method::POST,
        Some(&host),
        "/__share/unlock",
        Some("application/x-www-form-urlencoded"),
        b"password=s3cret".to_vec(),
        None,
        None,
    )
    .await;
    assert_eq!(ok.status, StatusCode::SEE_OTHER);
    let set_cookie = ok.set_cookie().expect("unlock cookie");
    let cookie = set_cookie.split(';').next().unwrap().to_string();
    assert!(cookie.starts_with("sdt_share="));

    // Unlocked: `/` now serves the bundle (sessionID present → no bootstrap
    // redirect; the unlock cookie rides alongside).
    let unlocked = share_get(&ctx.state, &host, "/?sessionID=probe", Some(&cookie)).await;
    assert_eq!(unlocked.status, StatusCode::OK);
    assert_eq!(unlocked.body, INDEX_HTML);
}

/// Concurrent-session cap: new sessions beyond the limit get 429.
#[tokio::test]
async fn concurrent_session_cap() {
    let Some(ctx) = setup().await else { return };
    let (ws, token) = seed(&ctx.state).await;
    let share = create_share(
        &ctx.state,
        &ws,
        &token,
        json!({ "max_concurrent_sessions": 2 }),
    )
    .await;
    let host = host_for(share["slug"].as_str().unwrap());
    assert_eq!(share["max_concurrent_sessions"].as_i64(), Some(2));

    // Two distinct sessions admitted.
    for i in 0..2 {
        let resp = share_post_json(
            &ctx.state,
            &host,
            &format!("/api/rgs/{GAME}/wallet/authenticate"),
            json!({ "sessionID": format!("s{i}"), "language": "en" }),
        )
        .await;
        assert_eq!(resp.status, StatusCode::OK, "session {i} admitted");
    }
    // A third distinct session is over the cap.
    let over = share_post_json(
        &ctx.state,
        &host,
        &format!("/api/rgs/{GAME}/wallet/authenticate"),
        json!({ "sessionID": "s2", "language": "en" }),
    )
    .await;
    assert_eq!(over.status, StatusCode::TOO_MANY_REQUESTS);
    // An existing session still works (does not count against the cap).
    let existing = share_post_json(
        &ctx.state,
        &host,
        &format!("/api/rgs/{GAME}/wallet/balance"),
        json!({ "sessionID": "s0" }),
    )
    .await;
    assert_eq!(existing.status, StatusCode::OK);
}

/// A front bundle without a root index.html is rejected.
#[tokio::test]
async fn front_bundle_requires_index_html() {
    let Some(ctx) = setup().await else { return };
    let token = owner_token(&ctx.state).await;
    let ws = create_workspace(&ctx.state, &token).await;
    push_math(&ctx.state, &ws, &rev_files(INDEX_1MODE), None, &token).await;

    let files: [(&str, &[u8]); 1] = [("app.js", APP_JS)];
    let resp = api(
        &ctx.state,
        Method::POST,
        &format!("/api/workspaces/{ws}/games/{GAME}/front-bundles"),
        Some(json!({ "files": manifest(&files) })),
        Some(&token),
    )
    .await;
    assert_eq!(
        resp.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        resp.json()
    );
    assert_eq!(
        resp.json()["error"]["code"].as_str(),
        Some("invalid_manifest")
    );
}

/// Listing surfaces counters + observed RTP after a play.
#[tokio::test]
async fn list_reports_counters() {
    let Some(ctx) = setup().await else { return };
    let (ws, token) = seed(&ctx.state).await;
    let share = create_share(&ctx.state, &ws, &token, json!({})).await;
    let host = host_for(share["slug"].as_str().unwrap());

    // Two plays so total_bet > 0 and observed_rtp is defined.
    let sid = format!("vis-{}", Uuid::new_v4());
    let _ = share_post_json(
        &ctx.state,
        &host,
        &format!("/api/rgs/{GAME}/wallet/authenticate"),
        json!({ "sessionID": sid, "language": "en" }),
    )
    .await;
    for _ in 0..2 {
        let _ = share_post_json(
            &ctx.state,
            &host,
            &format!("/api/rgs/{GAME}/wallet/play"),
            json!({ "sessionID": sid, "mode": "base", "amount": 1 }),
        )
        .await;
    }

    let list = api(
        &ctx.state,
        Method::GET,
        &format!("/api/workspaces/{ws}/games/{GAME}/shares"),
        None,
        Some(&token),
    )
    .await;
    assert_eq!(list.status, StatusCode::OK);
    let shares = list.json();
    let entry = shares["shares"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == share["id"])
        .expect("share in list");
    assert_eq!(entry["spins_count"].as_i64(), Some(2));
    assert_eq!(entry["total_bet"].as_f64(), Some(2.0));
    // observed_rtp is present (total_bet > 0).
    assert!(entry["observed_rtp"].is_number());
}

/// Pull the `sessionID` value out of a redirect `Location` query.
fn sid_from_location(location: &str) -> String {
    let query = location.split_once('?').map(|(_, q)| q).unwrap_or("");
    for kv in query.split('&') {
        if let Some(v) = kv.strip_prefix("sessionID=") {
            return v.to_string();
        }
    }
    panic!("no sessionID in {location}");
}

/// Front-contract bootstrap: a bare `/` on a share host (no query) is 302'd to
/// itself with the Stake front-contract params — `sessionID`, `rgs_url` pointing
/// at THIS host's `/api/rgs/<game>`, `lang`/`currency`/`device`/`social` — and a
/// `sdt_share_sid` cookie. Following the redirect serves the bundle; a second
/// paramless load carrying the cookie redirects with the SAME sessionID (so a
/// refresh never inflates the session count).
#[tokio::test]
async fn entry_redirect_injects_front_contract() {
    let Some(ctx) = setup().await else { return };
    let (ws, token) = seed(&ctx.state).await;
    let share = create_share(&ctx.state, &ws, &token, json!({})).await;
    let host = host_for(share["slug"].as_str().unwrap());

    // Bare `/` → 302 with the contract params + a fresh sid cookie.
    let boot = share_get(&ctx.state, &host, "/", None).await;
    assert_eq!(
        boot.status,
        StatusCode::FOUND,
        "expected 302, got {:?}",
        boot.status
    );
    let location = boot
        .headers
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    assert!(
        location.contains("sessionID="),
        "location has sessionID: {location}"
    );
    assert!(
        location.contains("rgs_url="),
        "location has rgs_url: {location}"
    );
    // rgs_url targets this share host's /api/rgs/<game>, SCHEME-LESS (the
    // front SDK prefixes https:// itself; a scheme here breaks every front).
    assert!(
        location.contains(GAME),
        "rgs_url targets the game: {location}"
    );
    assert!(
        !location.contains("rgs_url=https") && !location.contains("rgs_url=http"),
        "rgs_url must be scheme-less: {location}"
    );
    assert!(location.contains("lang=en"), "{location}");
    assert!(location.contains("currency=USD"), "{location}");
    assert!(location.contains("device=desktop"), "{location}");
    assert!(location.contains("social=false"), "{location}");

    let set_cookie = boot.set_cookie().expect("sid cookie set");
    assert!(set_cookie.starts_with("sdt_share_sid="), "{set_cookie}");
    let cookie = set_cookie.split(';').next().unwrap().to_string();
    let sid = sid_from_location(&location);

    // Following the redirect (sessionID now present) serves the bundle index.
    let followed = share_get(&ctx.state, &host, &location, Some(&cookie)).await;
    assert_eq!(followed.status, StatusCode::OK, "{:?}", followed.status);
    assert_eq!(followed.body, INDEX_HTML);

    // A second paramless load carrying the cookie reuses the SAME sid and does
    // not re-set the cookie.
    let again = share_get(&ctx.state, &host, "/", Some(&cookie)).await;
    assert_eq!(again.status, StatusCode::FOUND);
    let loc2 = again
        .headers
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    assert_eq!(sid_from_location(&loc2), sid, "reused cookie keeps the sid");
    assert!(again.set_cookie().is_none(), "no new cookie on reuse");
}

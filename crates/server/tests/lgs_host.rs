//! M4 integration tests: multi-tenant cloud LGS hosting, driven through the HTTP
//! API. A revision is seeded with the same push flow as `math_revisions.rs`
//! (check → upload → commit), then played through the mounted tenant router at
//! `/api/ws/:slug/g/:game/r/:number/*rest`.
//!
//! Every test self-skips when `TEST_DATABASE_URL` is unset, so `cargo test`
//! stays green with no database. Slugs/emails are UUID-suffixed since the dev
//! database persists between runs.
//!
//! The books fixture is a REAL zstd frame (`BOOKS_ZST`, precomputed so the test
//! needs no zstd dependency), so `play`/`end-round` exercise the full decompress
//! → index → read-event path, not just config loading.

use std::collections::HashMap;
use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use server::config::{Config, StorageConfig};
use server::{AppState, db, http, storage};
use sha2::{Digest, Sha256};
use tower::ServiceExt; // brings `oneshot` onto Router
use uuid::Uuid;

// --- fixture math folder (index.json + lookup CSV + a real zstd books file) ---

const GAME: &str = "demo";

// One-mode config.
const INDEX_1MODE: &[u8] =
    br#"{"modes":[{"name":"base","cost":1,"events":"books_base.zst","weights":"lookup_base.csv"}]}"#;
// Two-mode config (same slug "demo") — used to prove tenant isolation.
const INDEX_2MODE: &[u8] = br#"{"modes":[{"name":"base","cost":1,"events":"books_base.zst","weights":"lookup_base.csv"},{"name":"bonus","cost":100,"events":"books_base.zst","weights":"lookup_base.csv"}]}"#;
// event 0 = loss (payout 0), event 1 = 2x win (payout 200). Both ids appear in
// the books below, so a weighted pick of either resolves an event.
const LOOKUP: &[u8] = b"0,5000,0\n1,5000,200\n";
// Real zstd frame of:
//   {"id":0,"events":[]}\n{"id":1,"events":[{"reveal":"win"}]}\n
// Precomputed with libzstd (level 19); starts with the zstd magic 28 b5 2f fd.
const BOOKS_ZST: &[u8] = &[
    40, 181, 47, 253, 32, 58, 149, 1, 0, 148, 2, 123, 34, 105, 100, 34, 58, 48, 44, 34, 101, 118,
    101, 110, 116, 115, 34, 58, 91, 93, 125, 10, 49, 123, 34, 114, 101, 118, 101, 97, 108, 34, 58,
    34, 119, 105, 110, 34, 125, 93, 125, 10, 2, 0, 192, 136, 56, 76, 38,
];

fn rev_files(index: &'static [u8]) -> [(&'static str, &'static [u8]); 3] {
    [
        ("index.json", index),
        ("lookup_base.csv", LOOKUP),
        ("books_base.zst", BOOKS_ZST),
    ]
}

struct Ctx {
    state: AppState,
    tmp: tempfile::TempDir,
}

impl Ctx {
    /// The materializer's cache root: `<STORAGE_FS_ROOT>/../cache`. We root the
    /// blob store at `<tmp>/blobs`, so the cache lands inside `<tmp>` and is
    /// cleaned up with the temp dir.
    fn cache_root(&self) -> PathBuf {
        self.tmp.path().join("cache")
    }
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
        play_domain: None,
        admin_emails: Vec::new(),
    };
    let pool = db::connect_lazy(&database_url).expect("lazy pool");
    let store = storage::build_object_store(&config).expect("fs store");
    let state = AppState::new(config, pool, store);
    db::migrate(&state.pool).await.expect("migrations apply");
    Some(Ctx { state, tmp })
}

// --- HTTP client harness (subset of math_revisions.rs) ---

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

    async fn raw(
        &mut self,
        method: Method,
        uri: &str,
        content_type: Option<&str>,
        body: Vec<u8>,
        bearer: Option<&str>,
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
        let (ct, raw_body) = match &body {
            Some(v) => (Some("application/json"), serde_json::to_vec(v).unwrap()),
            None => (None, Vec::new()),
        };
        let (status, bytes) = self.raw(method, uri, ct, raw_body, bearer).await;
        (status, parse_json(&bytes))
    }

    async fn send_bytes(
        &mut self,
        method: Method,
        uri: &str,
        body: Vec<u8>,
        bearer: Option<&str>,
    ) -> (StatusCode, Value) {
        let (status, bytes) = self
            .raw(method, uri, Some("application/octet-stream"), body, bearer)
            .await;
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

fn s(value: &Value) -> &str {
    value.as_str().unwrap()
}

async fn register(state: &AppState, email: &str, display_name: &str) -> Client {
    let mut client = Client::new(state);
    let (status, body) = client
        .post(
            "/api/auth/register",
            json!({ "email": email, "password": "password123", "display_name": display_name }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "register failed: {body}");
    client
}

/// Registers an owner and mints a `push:math` PAT (workspaces are created
/// separately so a test can make several under one owner).
async fn owner_with_token(state: &AppState) -> (Client, String) {
    let mut owner = register(state, &unique_email(), "Owner").await;
    let (status, created) = owner
        .post(
            "/api/tokens",
            json!({ "name": "ci", "scopes": ["push:math"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let token = created["token"].as_str().unwrap().to_string();
    (owner, token)
}

async fn create_workspace(owner: &mut Client) -> String {
    let ws = unique_slug();
    let (status, body) = owner
        .post(
            "/api/workspaces",
            json!({ "name": "Cloud WS", "slug": &ws }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    ws
}

/// Push a revision through check → upload missing blobs → commit.
async fn push(
    client: &mut Client,
    ws: &str,
    game: &str,
    files: &[(&str, &[u8])],
    parent: Option<i32>,
    token: &str,
) {
    let m = manifest(files);
    let (status, body) = client
        .send(
            Method::POST,
            &format!("/api/workspaces/{ws}/games/{game}/revisions/check"),
            Some(json!({ "files": m })),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "check: {body}");
    let missing: Vec<String> = body["missing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| s(v).to_string())
        .collect();
    for (path, bytes) in files {
        let hash = sha_hex(bytes);
        if missing.contains(&hash) {
            let (status, up) = client
                .send_bytes(
                    Method::PUT,
                    &format!("/api/workspaces/{ws}/games/{game}/blobs/{hash}"),
                    bytes.to_vec(),
                    Some(token),
                )
                .await;
            assert_eq!(status, StatusCode::CREATED, "PUT {path}: {up}");
        }
    }
    let mut body = json!({ "message": "rev", "files": m });
    if let Some(pn) = parent {
        body["parent_number"] = json!(pn);
    }
    let (status, detail) = client
        .send(
            Method::POST,
            &format!("/api/workspaces/{ws}/games/{game}/revisions"),
            Some(body),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "commit: {detail}");
}

async fn game_ids(state: &AppState, ws: &str, game: &str) -> (Uuid, Uuid) {
    sqlx::query_as(
        "SELECT w.id, g.id FROM workspaces w JOIN games g ON g.workspace_id = w.id \
         WHERE w.slug = $1 AND g.slug = $2",
    )
    .bind(ws)
    .bind(game)
    .fetch_one(&state.pool)
    .await
    .unwrap()
}

fn ws_url(ws: &str, game: &str, number: i32, rest: &str) -> String {
    format!("/api/ws/{ws}/g/{game}/r/{number}/{rest}")
}

fn modes_rest(game: &str) -> String {
    format!("api/devtool/games/{game}/modes")
}

fn number_dir(cache_root: &std::path::Path, ws_id: Uuid, game_id: Uuid, number: i32) -> PathBuf {
    cache_root
        .join("rev")
        .join(ws_id.to_string())
        .join(game_id.to_string())
        .join(number.to_string())
}

// ---------------------------------------------------------------------------

/// (a) A non-member hitting the tenant mount gets 404 — never learning the
/// workspace/game/revision exists — because membership is the auth boundary.
#[tokio::test]
async fn non_member_dispatch_is_404() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, token) = owner_with_token(&ctx.state).await;
    let ws = create_workspace(&mut owner).await;
    push(&mut owner, &ws, GAME, &rev_files(INDEX_1MODE), None, &token).await;

    let mut outsider = register(&ctx.state, &unique_email(), "Charlie").await;
    let (status, _) = outsider.get(&ws_url(&ws, GAME, 1, &modes_rest(GAME))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// (b) A member GETs the devtool mode list, (c) plays the full wallet flow
/// through the tenant router, and (e) a re-request after the cache dir is
/// deleted re-materializes.
#[tokio::test]
async fn member_devtool_and_wallet_flow_and_rematerialize() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, token) = owner_with_token(&ctx.state).await;
    let ws = create_workspace(&mut owner).await;
    push(&mut owner, &ws, GAME, &rev_files(INDEX_1MODE), None, &token).await;

    // (b) mode list served through the tenant's devtool router.
    let (status, modes) = owner.get(&ws_url(&ws, GAME, 1, &modes_rest(GAME))).await;
    assert_eq!(status, StatusCode::OK, "{modes}");
    let names: Vec<&str> = modes["modes"].as_array().unwrap().iter().map(s).collect();
    assert_eq!(names, ["base"]);

    // Materialization wrote the `.complete` marker synchronously.
    let (ws_id, game_id) = game_ids(&ctx.state, &ws, GAME).await;
    let dir = number_dir(&ctx.cache_root(), ws_id, game_id, 1);
    assert!(dir.join(".complete").exists(), "marker after materialize");
    assert!(
        dir.join(GAME).join("index.json").exists(),
        "index.json under <number>/<game_slug>/"
    );

    // (c) full wallet flow: authenticate → balance → play → end-round.
    let sid = format!("sess-{}", Uuid::new_v4());
    let auth_url = ws_url(&ws, GAME, 1, &format!("api/rgs/{GAME}/wallet/authenticate"));
    let (status, auth) = owner
        .post(&auth_url, json!({ "sessionID": sid, "language": "en" }))
        .await;
    assert_eq!(status, StatusCode::OK, "authenticate: {auth}");
    // Default LGS balance (10_000 * 1_000_000).
    assert_eq!(auth["balance"]["amount"].as_u64().unwrap(), 10_000_000_000);

    let bal_url = ws_url(&ws, GAME, 1, &format!("api/rgs/{GAME}/wallet/balance"));
    let (status, bal) = owner.post(&bal_url, json!({ "sessionID": sid })).await;
    assert_eq!(status, StatusCode::OK, "balance: {bal}");
    assert_eq!(bal["balance"]["amount"].as_u64().unwrap(), 10_000_000_000);

    let play_url = ws_url(&ws, GAME, 1, &format!("api/rgs/{GAME}/wallet/play"));
    let (status, play) = owner
        .post(
            &play_url,
            json!({ "sessionID": sid, "mode": "base", "amount": 1 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "play (real books decode): {play}");
    assert_eq!(s(&play["round"]["mode"]), "base");
    // Bet of 1 was deducted; a payout (0 or 2) is only credited at end-round.
    assert_eq!(play["balance"]["amount"].as_u64().unwrap(), 9_999_999_999);

    let end_url = ws_url(&ws, GAME, 1, &format!("api/rgs/{GAME}/wallet/end-round"));
    let (status, end) = owner.post(&end_url, json!({ "sessionID": sid })).await;
    assert_eq!(status, StatusCode::OK, "end-round: {end}");

    // (e) delete the cache dir; the next request re-materializes it.
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(!dir.join(".complete").exists(), "marker gone after delete");
    let (status, modes) = owner.get(&ws_url(&ws, GAME, 1, &modes_rest(GAME))).await;
    assert_eq!(status, StatusCode::OK, "{modes}");
    assert!(
        dir.join(".complete").exists(),
        "marker recreated (idempotent re-materialize)"
    );
}

/// (d) The same game slug in two workspaces resolves to two isolated tenants
/// serving their own distinct configs.
#[tokio::test]
async fn same_slug_isolated_across_workspaces() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, token) = owner_with_token(&ctx.state).await;

    let ws_a = create_workspace(&mut owner).await;
    push(
        &mut owner,
        &ws_a,
        GAME,
        &rev_files(INDEX_1MODE),
        None,
        &token,
    )
    .await;
    let ws_b = create_workspace(&mut owner).await;
    push(
        &mut owner,
        &ws_b,
        GAME,
        &rev_files(INDEX_2MODE),
        None,
        &token,
    )
    .await;

    let (_, a) = owner.get(&ws_url(&ws_a, GAME, 1, &modes_rest(GAME))).await;
    let (_, b) = owner.get(&ws_url(&ws_b, GAME, 1, &modes_rest(GAME))).await;
    assert_eq!(a["modes"].as_array().unwrap().len(), 1, "ws_a: base only");
    assert_eq!(
        b["modes"].as_array().unwrap().len(),
        2,
        "ws_b: base + bonus"
    );
}

/// (f) Two revisions of one game resolve to two distinct tenants, each with its
/// own materialized math.
#[tokio::test]
async fn distinct_revisions_are_distinct_tenants() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, token) = owner_with_token(&ctx.state).await;
    let ws = create_workspace(&mut owner).await;
    push(&mut owner, &ws, GAME, &rev_files(INDEX_1MODE), None, &token).await;
    push(
        &mut owner,
        &ws,
        GAME,
        &rev_files(INDEX_2MODE),
        Some(1),
        &token,
    )
    .await;

    let (_, r1) = owner.get(&ws_url(&ws, GAME, 1, &modes_rest(GAME))).await;
    let (_, r2) = owner.get(&ws_url(&ws, GAME, 2, &modes_rest(GAME))).await;
    assert_eq!(r1["modes"].as_array().unwrap().len(), 1, "rev1: base only");
    assert_eq!(r2["modes"].as_array().unwrap().len(), 2, "rev2: base+bonus");

    // Each revision materialized into its own tenant directory.
    let (ws_id, game_id) = game_ids(&ctx.state, &ws, GAME).await;
    assert!(
        number_dir(&ctx.cache_root(), ws_id, game_id, 1)
            .join(".complete")
            .exists()
    );
    assert!(
        number_dir(&ctx.cache_root(), ws_id, game_id, 2)
            .join(".complete")
            .exists()
    );
}

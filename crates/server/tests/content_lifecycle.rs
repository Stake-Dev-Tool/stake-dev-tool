//! Content-lifecycle tests: deleting revisions and front bundles to free
//! storage, the front-bundle listing + pinned-bundle serving, and the
//! `front_pushed` SSE event — driven through the HTTP API. Every test self-skips
//! when `TEST_DATABASE_URL` is unset, so `cargo test` stays green with no
//! database. The dev database persists between runs, so all emails and slugs are
//! UUID-suffixed.
//!
//! The blob store is the local fs backend rooted at a temp dir, so the tests can
//! assert on the on-disk object count directly (blobs live at
//! `<root>/blobs/<workspace>/<hex sha256>`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use axum::body::{Body, BodyDataStream};
use axum::http::{Method, Request, StatusCode};
use futures_util::StreamExt;
use serde_json::{Value, json};
use server::config::{Config, StorageConfig};
use server::{AppState, db, http, storage};
use sha2::{Digest, Sha256};
use tower::ServiceExt; // brings `oneshot` onto Router
use uuid::Uuid;

// --- fixtures ---------------------------------------------------------------

const INDEX_JSON: &[u8] =
    br#"{"modes":[{"name":"base","cost":1,"events":"books_base.jsonl","weights":"lookup_base.csv"}]}"#;
const LOOKUP_V1: &[u8] = b"0,9000,0\n1,900,100\n2,90,5000\n3,10,42000\n";
const LOOKUP_V2: &[u8] = b"0,9500,0\n1,400,100\n2,90,5000\n3,10,42000\n";
const BOOKS: &[u8] = b"{\"id\":0,\"events\":[]}\n{\"id\":1,\"events\":[]}\n";

const INDEX_HTML: &[u8] = b"<!doctype html><title>demo</title><div id=app></div>";
const APP_JS: &[u8] = b"console.log('bundle v1');\n";
const APP_JS2: &[u8] = b"console.log('bundle v2 is a different length');\n";

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
        trusted_proxies: Default::default(),
    };
    let pool = db::connect_lazy(&database_url).expect("lazy pool");
    let store = storage::build_object_store(&config).expect("fs store");
    let state = AppState::new(config, pool, store);
    db::migrate(&state.pool).await.expect("migrations apply");
    Some(Ctx { state, _tmp: tmp })
}

// --- HTTP harness (cookie + bearer) -----------------------------------------

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
            let deleting = value.is_empty() || header.to_lowercase().contains("max-age=0");
            if deleting {
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

    async fn get_raw(&mut self, uri: &str, bearer: Option<&str>) -> (StatusCode, Vec<u8>) {
        self.raw(Method::GET, uri, None, Vec::new(), bearer).await
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

/// A unique share-link label (share slugs are globally UNIQUE and the dev DB
/// persists between runs). `<prefix>-<8 hex>` stays within the 2–40 char rule.
fn unique_share_slug(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().to_string()[..8])
}

fn s(value: &Value) -> &str {
    value.as_str().unwrap()
}

async fn register(state: &AppState, email: &str, display_name: &str) -> (Client, Value) {
    let mut client = Client::new(state);
    let (status, body) = client
        .post(
            "/api/auth/register",
            json!({ "email": email, "password": "password123", "display_name": display_name }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "register failed: {body}");
    let user = body["user"].clone();
    (client, user)
}

/// Registers an owner, creates a workspace, and mints a `push:math` PAT.
async fn bootstrap(state: &AppState) -> (Client, String, String) {
    let (mut owner, _) = register(state, &unique_email(), "Owner").await;
    let ws = unique_slug();
    let (status, _) = owner
        .post(
            "/api/workspaces",
            json!({ "name": "Lifecycle WS", "slug": &ws }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
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

/// Creates an additional workspace owned by the same user (its owner-role
/// membership + the existing `push:math` PAT authorize pushes into it).
async fn add_workspace(owner: &mut Client) -> String {
    let ws = unique_slug();
    let (status, _) = owner
        .post(
            "/api/workspaces",
            json!({ "name": "Second WS", "slug": &ws }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    ws
}

// --- push helpers -----------------------------------------------------------

async fn push_math(
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
    upload_missing(client, ws, game, &body, files, token).await;
    let mut commit = json!({ "message": "rev", "files": m });
    if let Some(pn) = parent {
        commit["parent_number"] = json!(pn);
    }
    let (status, body) = client
        .send(
            Method::POST,
            &format!("/api/workspaces/{ws}/games/{game}/revisions"),
            Some(commit),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "commit: {body}");
}

/// Push a front bundle (check → upload → commit); returns the new bundle id.
async fn push_bundle(
    client: &mut Client,
    ws: &str,
    game: &str,
    files: &[(&str, &[u8])],
    token: &str,
) -> String {
    let m = manifest(files);
    let (status, body) = client
        .send(
            Method::POST,
            &format!("/api/workspaces/{ws}/games/{game}/front-bundles/check"),
            Some(json!({ "files": m })),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "bundle check: {body}");
    upload_missing(client, ws, game, &body, files, token).await;
    let (status, body) = client
        .send(
            Method::POST,
            &format!("/api/workspaces/{ws}/games/{game}/front-bundles"),
            Some(json!({ "files": m })),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "bundle commit: {body}");
    s(&body["id"]).to_string()
}

/// Front-bundle commits require an existing game (unlike math push, they do not
/// upsert it), so tests that only exercise bundles first seed the game with a
/// minimal math revision.
async fn seed_game(client: &mut Client, ws: &str, game: &str, token: &str) {
    push_math(
        client,
        ws,
        game,
        &[
            ("index.json", INDEX_JSON),
            ("lookup_base.csv", LOOKUP_V1),
            ("books_base.jsonl", BOOKS),
        ],
        None,
        token,
    )
    .await;
}

async fn upload_missing(
    client: &mut Client,
    ws: &str,
    game: &str,
    check: &Value,
    files: &[(&str, &[u8])],
    token: &str,
) {
    let missing: Vec<String> = check["missing"]
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
            assert_eq!(status, StatusCode::CREATED, "upload {path}: {up}");
        }
    }
}

async fn create_share(client: &mut Client, ws: &str, game: &str, body: Value) -> Value {
    let (status, resp) = client
        .send(
            Method::POST,
            &format!("/api/workspaces/{ws}/games/{game}/shares"),
            Some(body),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "create share: {resp}");
    resp
}

// --- store / db introspection -----------------------------------------------

async fn workspace_uuid(state: &AppState, ws: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM workspaces WHERE slug = $1")
        .bind(ws)
        .fetch_one(&state.pool)
        .await
        .unwrap()
}

fn blob_dir(state: &AppState, ws_id: Uuid) -> PathBuf {
    match &state.config.storage {
        StorageConfig::Fs { root } => root.join("blobs").join(ws_id.to_string()),
        StorageConfig::S3 { .. } => unreachable!("tests use fs storage"),
    }
}

/// Count the on-disk blob objects for a workspace (`<root>/blobs/<ws>/…`).
fn count_store_objects(state: &AppState, ws_id: Uuid) -> usize {
    std::fs::read_dir(blob_dir(state, ws_id))
        .map(|it| it.flatten().count())
        .unwrap_or(0)
}

async fn count_blob_rows(state: &AppState, ws_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM blobs WHERE workspace_id = $1")
        .bind(ws_id)
        .fetch_one(&state.pool)
        .await
        .unwrap()
}

// --- SSE helpers (mirrors tests/documents.rs) -------------------------------

async fn open_events_stream(state: &AppState, ws: &str, cookie: &str) -> BodyDataStream {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/workspaces/{ws}/events"))
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let resp = http::build_router(state.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "SSE connect should be 200");
    resp.into_body().into_data_stream()
}

async fn wait_for_event(stream: &mut BodyDataStream, needle: &str) -> bool {
    let read = async {
        let mut buf = String::new();
        while let Some(chunk) = stream.next().await {
            let Ok(bytes) = chunk else {
                return false;
            };
            buf.push_str(&String::from_utf8_lossy(&bytes));
            if buf.contains(needle) {
                return true;
            }
        }
        false
    };
    tokio::time::timeout(Duration::from_secs(5), read)
        .await
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

/// Deleting a revision with no share pin drops its rows and GCs every blob it
/// solely referenced: the store object count falls to zero and `freed_bytes` is
/// positive.
#[tokio::test]
async fn delete_revision_frees_orphaned_blobs() {
    let Some(ctx) = setup().await else { return };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    let game = "solo";
    let files = [
        ("index.json", INDEX_JSON),
        ("lookup_base.csv", LOOKUP_V1),
        ("books_base.jsonl", BOOKS),
    ];
    push_math(&mut owner, &ws, game, &files, None, &token).await;
    let ws_id = workspace_uuid(&ctx.state, &ws).await;
    assert_eq!(count_store_objects(&ctx.state, ws_id), 3, "3 blobs stored");

    let (status, body) = owner
        .send(
            Method::DELETE,
            &format!("/api/workspaces/{ws}/games/{game}/revisions/1"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["freed_blobs"].as_i64().unwrap(), 3);
    assert!(body["freed_bytes"].as_i64().unwrap() > 0, "{body}");

    assert_eq!(count_store_objects(&ctx.state, ws_id), 0, "store emptied");
    assert_eq!(
        count_blob_rows(&ctx.state, ws_id).await,
        0,
        "blob rows gone"
    );

    // The revision is gone (404), and the game is now empty.
    let (status, _) = owner
        .get(&format!("/api/workspaces/{ws}/games/{game}/revisions/1"))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A blob shared by another revision survives the delete; only the deleted
/// revision's unique blob is freed.
#[tokio::test]
async fn dedup_shared_blob_survives_delete() {
    let Some(ctx) = setup().await else { return };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    let game = "shared";
    // rev1 and rev2 share index.json + books; only lookup differs.
    let rev1 = [
        ("index.json", INDEX_JSON),
        ("lookup_base.csv", LOOKUP_V1),
        ("books_base.jsonl", BOOKS),
    ];
    let rev2 = [
        ("index.json", INDEX_JSON),
        ("lookup_base.csv", LOOKUP_V2),
        ("books_base.jsonl", BOOKS),
    ];
    push_math(&mut owner, &ws, game, &rev1, None, &token).await;
    push_math(&mut owner, &ws, game, &rev2, Some(1), &token).await;
    let ws_id = workspace_uuid(&ctx.state, &ws).await;
    // 4 distinct blobs: index.json, books, lookup_v1, lookup_v2.
    assert_eq!(count_store_objects(&ctx.state, ws_id), 4);

    // Delete rev2 → only LOOKUP_V2 is orphaned; the shared blobs stay.
    let (status, body) = owner
        .send(
            Method::DELETE,
            &format!("/api/workspaces/{ws}/games/{game}/revisions/2"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["freed_blobs"].as_i64().unwrap(),
        1,
        "only the unique blob"
    );
    assert_eq!(
        body["freed_bytes"].as_i64().unwrap(),
        LOOKUP_V2.len() as i64
    );
    assert_eq!(
        count_store_objects(&ctx.state, ws_id),
        3,
        "shared blobs stay"
    );

    // rev1 still downloads its shared files intact.
    let (status, bytes) = owner
        .get_raw(
            &format!("/api/workspaces/{ws}/games/{game}/revisions/1/files/books_base.jsonl"),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, BOOKS);
}

/// A share pinning a revision blocks its deletion with `409 revision_pinned`,
/// and the message names the pinning share slug.
#[tokio::test]
async fn revision_pinned_by_share_is_409_with_slug() {
    let Some(ctx) = setup().await else { return };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    let game = "pinned-rev";
    push_math(
        &mut owner,
        &ws,
        game,
        &[
            ("index.json", INDEX_JSON),
            ("lookup_base.csv", LOOKUP_V1),
            ("books_base.jsonl", BOOKS),
        ],
        None,
        &token,
    )
    .await;
    let pin_slug = unique_share_slug("pinrev");
    let share = create_share(
        &mut owner,
        &ws,
        game,
        json!({ "slug": pin_slug.clone(), "revision_number": 1 }),
    )
    .await;
    assert_eq!(s(&share["slug"]), pin_slug);

    let (status, body) = owner
        .send(
            Method::DELETE,
            &format!("/api/workspaces/{ws}/games/{game}/revisions/1"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(s(&body["error"]["code"]), "revision_pinned");
    assert!(
        s(&body["error"]["message"]).contains(&pin_slug),
        "message should list the slug: {body}"
    );
}

/// Front-bundle deletion: the `last_bundle` guard fires while a share depends on
/// the only bundle; after a second bundle is pushed the first deletes and frees
/// its unique blob (the shared index.html survives); a bundle pinned by a share
/// is refused with `bundle_pinned`.
#[tokio::test]
async fn front_bundle_deletion_and_guards() {
    let Some(ctx) = setup().await else { return };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    let game = "fronts";
    // A revision so a share can be created against the game.
    push_math(
        &mut owner,
        &ws,
        game,
        &[
            ("index.json", INDEX_JSON),
            ("lookup_base.csv", LOOKUP_V1),
            ("books_base.jsonl", BOOKS),
        ],
        None,
        &token,
    )
    .await;
    let ws_id = workspace_uuid(&ctx.state, &ws).await;

    let bundle1 = push_bundle(
        &mut owner,
        &ws,
        game,
        &[("index.html", INDEX_HTML), ("app.js", APP_JS)],
        &token,
    )
    .await;
    // A latest-tracking share now depends on the only bundle (generated slug).
    create_share(&mut owner, &ws, game, json!({})).await;

    // last_bundle guard: cannot delete the game's only bundle while a share exists.
    let (status, body) = owner
        .send(
            Method::DELETE,
            &format!("/api/workspaces/{ws}/games/{game}/front-bundles/{bundle1}"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(s(&body["error"]["code"]), "last_bundle");

    // Push a second bundle sharing index.html but with a different app file.
    let bundle2 = push_bundle(
        &mut owner,
        &ws,
        game,
        &[("index.html", INDEX_HTML), ("app.js", APP_JS2)],
        &token,
    )
    .await;

    // Now bundle1 deletes: APP_JS is orphaned; INDEX_HTML is shared with bundle2.
    let before = count_store_objects(&ctx.state, ws_id);
    let (status, body) = owner
        .send(
            Method::DELETE,
            &format!("/api/workspaces/{ws}/games/{game}/front-bundles/{bundle1}"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["freed_blobs"].as_i64().unwrap(),
        1,
        "only APP_JS freed"
    );
    assert_eq!(body["freed_bytes"].as_i64().unwrap(), APP_JS.len() as i64);
    assert_eq!(count_store_objects(&ctx.state, ws_id), before - 1);

    // bundle_pinned guard: pin a share to bundle2, then refuse its deletion.
    let pin_slug = unique_share_slug("pinbndl");
    let pinned = create_share(
        &mut owner,
        &ws,
        game,
        json!({ "slug": pin_slug.clone(), "front_bundle_id": bundle2 }),
    )
    .await;
    assert_eq!(s(&pinned["front_bundle_id"]), bundle2);
    let (status, body) = owner
        .send(
            Method::DELETE,
            &format!("/api/workspaces/{ws}/games/{game}/front-bundles/{bundle2}"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(s(&body["error"]["code"]), "bundle_pinned");
    assert!(
        s(&body["error"]["message"]).contains(&pin_slug),
        "message should list the slug: {body}"
    );
}

/// The blob GC is per-workspace: deleting a revision in one workspace never
/// touches a byte-identical blob in another workspace.
#[tokio::test]
async fn gc_is_scoped_per_workspace() {
    let Some(ctx) = setup().await else { return };
    let (mut owner, ws1, token) = bootstrap(&ctx.state).await;
    let ws2 = add_workspace(&mut owner).await;
    let game = "twins";
    let files = [
        ("index.json", INDEX_JSON),
        ("lookup_base.csv", LOOKUP_V1),
        ("books_base.jsonl", BOOKS),
    ];
    // Identical content pushed to both workspaces (identical hashes, separate
    // per-workspace blob rows + store keys).
    push_math(&mut owner, &ws1, game, &files, None, &token).await;
    push_math(&mut owner, &ws2, game, &files, None, &token).await;
    let id1 = workspace_uuid(&ctx.state, &ws1).await;
    let id2 = workspace_uuid(&ctx.state, &ws2).await;
    assert_eq!(count_store_objects(&ctx.state, id1), 3);
    assert_eq!(count_store_objects(&ctx.state, id2), 3);

    // Delete ws1's revision → only ws1's blobs go.
    let (status, body) = owner
        .send(
            Method::DELETE,
            &format!("/api/workspaces/{ws1}/games/{game}/revisions/1"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["freed_blobs"].as_i64().unwrap(), 3);

    assert_eq!(count_store_objects(&ctx.state, id1), 0, "ws1 emptied");
    assert_eq!(
        count_store_objects(&ctx.state, id2),
        3,
        "ws2's identical blobs untouched"
    );
    assert_eq!(count_blob_rows(&ctx.state, id2).await, 3);
}

/// The front-bundle listing reports every bundle newest-first with derived
/// file counts + sizes, and flags the newest as `is_latest`.
#[tokio::test]
async fn front_bundle_listing_shape() {
    let Some(ctx) = setup().await else { return };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    let game = "listing";
    seed_game(&mut owner, &ws, game, &token).await;
    let b1 = push_bundle(
        &mut owner,
        &ws,
        game,
        &[("index.html", INDEX_HTML), ("app.js", APP_JS)],
        &token,
    )
    .await;
    let b2 = push_bundle(
        &mut owner,
        &ws,
        game,
        &[("index.html", INDEX_HTML), ("app.js", APP_JS2)],
        &token,
    )
    .await;

    let (status, body) = owner
        .get(&format!("/api/workspaces/{ws}/games/{game}/front-bundles"))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let bundles = body["bundles"].as_array().unwrap();
    assert_eq!(bundles.len(), 2);
    // Newest first: b2 is index 0 and is_latest; b1 is not.
    assert_eq!(s(&bundles[0]["id"]), b2);
    assert_eq!(bundles[0]["is_latest"], json!(true));
    assert_eq!(bundles[0]["files_count"], json!(2));
    assert_eq!(
        bundles[0]["total_size"].as_i64().unwrap(),
        (INDEX_HTML.len() + APP_JS2.len()) as i64
    );
    assert_eq!(s(&bundles[1]["id"]), b1);
    assert_eq!(bundles[1]["is_latest"], json!(false));
}

/// Pinned-bundle serving streams an exact bundle's files (membership-gated) and
/// 404s an unknown or foreign bundle id — while `/front/` still serves latest.
#[tokio::test]
async fn pinned_bundle_serving_and_foreign_404() {
    let Some(ctx) = setup().await else { return };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    let game = "pinserve";
    seed_game(&mut owner, &ws, game, &token).await;
    let b1 = push_bundle(
        &mut owner,
        &ws,
        game,
        &[("index.html", INDEX_HTML), ("app.js", APP_JS)],
        &token,
    )
    .await;
    let b2 = push_bundle(
        &mut owner,
        &ws,
        game,
        &[("index.html", INDEX_HTML), ("app.js", APP_JS2)],
        &token,
    )
    .await;

    // Pinned b1 serves its own app.js (the OLD one), not the latest.
    let (status, bytes) = owner
        .get_raw(
            &format!("/api/ws/{ws}/g/{game}/fronts/{b1}/app.js"),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, APP_JS, "pinned bundle serves its own asset");

    // Pinned b1 index (no path) serves index.html.
    let (status, bytes) = owner
        .get_raw(&format!("/api/ws/{ws}/g/{game}/fronts/{b1}"), Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, INDEX_HTML);

    // `/front/` (latest) still serves b2's app.js unchanged.
    let (status, bytes) = owner
        .get_raw(&format!("/api/ws/{ws}/g/{game}/front/app.js"), Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, APP_JS2, "latest serving unchanged");
    let _ = b2;

    // An unknown bundle id under this game 404s.
    let (status, _) = owner
        .get_raw(
            &format!("/api/ws/{ws}/g/{game}/fronts/{}", Uuid::new_v4()),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A foreign workspace's bundle id (real, but from another ws) 404s here.
    let ws2 = add_workspace(&mut owner).await;
    seed_game(&mut owner, &ws2, game, &token).await;
    let foreign = push_bundle(
        &mut owner,
        &ws2,
        game,
        &[("index.html", INDEX_HTML), ("app.js", APP_JS)],
        &token,
    )
    .await;
    let (status, _) = owner
        .get_raw(
            &format!("/api/ws/{ws}/g/{game}/fronts/{foreign}"),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "foreign bundle id must 404");
}

/// Committing a front bundle publishes a `front_pushed` SSE event to the
/// workspace stream.
#[tokio::test]
async fn front_pushed_event_on_bundle_commit() {
    let Some(ctx) = setup().await else { return };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    let cookie = owner.cookie_header().unwrap();
    let game = "sse-front";
    // Seed the game BEFORE subscribing, so only the front_pushed event flows on
    // the stream (the seed's revision_pushed lands before we start listening).
    seed_game(&mut owner, &ws, game, &token).await;

    let mut stream = open_events_stream(&ctx.state, &ws, &cookie).await;
    push_bundle(
        &mut owner,
        &ws,
        game,
        &[("index.html", INDEX_HTML), ("app.js", APP_JS)],
        &token,
    )
    .await;

    assert!(
        wait_for_event(&mut stream, "front_pushed").await,
        "expected a `front_pushed` SSE event after the bundle commit"
    );
}

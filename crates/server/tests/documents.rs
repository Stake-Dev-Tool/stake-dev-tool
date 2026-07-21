//! M3 integration tests: document sync (profiles, saved rounds), the workspace
//! SSE stream, and owner-only delete-workspace — driven through the HTTP API.
//! Every test self-skips when `TEST_DATABASE_URL` is unset, so `cargo test`
//! stays green with no database. The dev database persists between runs, so all
//! emails and slugs are suffixed with a fresh UUID.

use std::collections::HashMap;
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
        web_dir: None,
        storage_max_blob_bytes: 8_589_934_592,
        server_math_cache_bytes: 21_474_836_480,
        server_tenant_books_cap_bytes: None,
    };
    let pool = db::connect_lazy(&database_url).expect("lazy pool");
    let store = storage::build_object_store(&config).expect("fs store");
    let state = AppState::new(config, pool, store);
    db::migrate(&state.pool).await.expect("migrations apply");
    Some(Ctx { state, _tmp: tmp })
}

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

fn unique_email() -> String {
    format!("user-{}@example.com", Uuid::new_v4())
}

fn unique_slug() -> String {
    format!("ws-{}", Uuid::new_v4())
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

/// Registers an owner and creates a workspace. Returns the owner client (with a
/// full-scope session) and the workspace slug.
async fn bootstrap(state: &AppState) -> (Client, String) {
    let (mut owner, _) = register(state, &unique_email(), "Owner").await;
    let ws = unique_slug();
    let (status, _) = owner
        .post("/api/workspaces", json!({ "name": "Docs WS", "slug": &ws }))
        .await;
    assert_eq!(status, StatusCode::OK);
    (owner, ws)
}

// --- document helpers ---

fn docs_url(ws: &str) -> String {
    format!("/api/workspaces/{ws}/documents")
}
fn doc_url(ws: &str, kind: &str, doc_id: &str) -> String {
    format!("/api/workspaces/{ws}/documents/{kind}/{doc_id}")
}

fn profile_data(name: &str) -> Value {
    json!({
        "name": name,
        "game_slug": "sweet-bonanza",
        "game": "sweet-bonanza",
        "revision": null,
        "front_url": "https://example.com/front",
        "resolutions": [
            { "id": "r1", "label": "Desktop", "width": 1920, "height": 1080, "enabled": true, "builtin": true }
        ],
        "created_at": 1_700_000_000_000i64
    })
}

fn saved_round_data(description: &str) -> Value {
    json!({
        "game_slug": "sweet-bonanza",
        "mode": "base",
        "event_id": 42,
        "description": description,
        "created_at": 1_700_000_000_000i64
    })
}

/// PUT a document. `base` = None omits `base_revision` (a create); Some(n) sends
/// it (an optimistic update).
async fn put_doc(
    client: &mut Client,
    ws: &str,
    kind: &str,
    doc_id: &str,
    data: Value,
    base: Option<i32>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut body = json!({ "data": data });
    if let Some(b) = base {
        body["base_revision"] = json!(b);
    }
    client
        .send(Method::PUT, &doc_url(ws, kind, doc_id), Some(body), bearer)
        .await
}

// --- SSE helpers ---

/// Open the workspace SSE stream as `cookie`. Subscription is live once this
/// returns (the handler subscribes before yielding the response), so an event
/// published afterwards is buffered and delivered on the first poll.
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
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.starts_with("text/event-stream"),
        "SSE content-type was {ct}"
    );
    resp.into_body().into_data_stream()
}

/// Read frames until `needle` appears or the timeout fires.
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

#[tokio::test]
async fn put_get_roundtrip_bumps_revision() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws) = bootstrap(&ctx.state).await;
    let doc_id = Uuid::new_v4().to_string();

    // Create (base omitted = null).
    let (status, body) = put_doc(
        &mut owner,
        &ws,
        "profile",
        &doc_id,
        profile_data("First"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["revision"], json!(1));
    let seq1 = body["seq"].as_i64().unwrap();

    // GET one → the stored envelope.
    let (status, env) = owner.get(&doc_url(&ws, "profile", &doc_id)).await;
    assert_eq!(status, StatusCode::OK, "{env}");
    assert_eq!(env["revision"], json!(1));
    assert_eq!(env["deleted"], json!(false));
    assert_eq!(s(&env["kind"]), "profile");
    assert_eq!(s(&env["data"]["name"]), "First");
    assert_eq!(s(&env["updated_by_display"]), "Owner");
    assert_eq!(env["seq"].as_i64().unwrap(), seq1);

    // Update with the fresh base_revision → revision 2, higher seq.
    let (status, body) = put_doc(
        &mut owner,
        &ws,
        "profile",
        &doc_id,
        profile_data("Second"),
        Some(1),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["revision"], json!(2));
    assert!(body["seq"].as_i64().unwrap() > seq1, "seq must advance");

    let (_, env) = owner.get(&doc_url(&ws, "profile", &doc_id)).await;
    assert_eq!(env["revision"], json!(2));
    assert_eq!(s(&env["data"]["name"]), "Second");
}

#[tokio::test]
async fn stale_base_revision_conflicts_then_force_wins() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws) = bootstrap(&ctx.state).await;
    let doc_id = Uuid::new_v4().to_string();

    put_doc(
        &mut owner,
        &ws,
        "saved_round",
        &doc_id,
        saved_round_data("v1"),
        None,
        None,
    )
    .await;
    // A first update takes it to revision 2.
    let (status, _) = put_doc(
        &mut owner,
        &ws,
        "saved_round",
        &doc_id,
        saved_round_data("v2"),
        Some(1),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // A stale writer still holding base_revision 1 → 409 with the current doc.
    let (status, conflict) = put_doc(
        &mut owner,
        &ws,
        "saved_round",
        &doc_id,
        saved_round_data("stale"),
        Some(1),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
    assert_eq!(s(&conflict["error"]["code"]), "document_conflict");
    assert_eq!(conflict["current"]["revision"], json!(2));
    assert_eq!(s(&conflict["current"]["data"]["description"]), "v2");

    // Keep-mine: retry against the fresh revision → wins.
    let fresh = conflict["current"]["revision"].as_i64().unwrap() as i32;
    let (status, body) = put_doc(
        &mut owner,
        &ws,
        "saved_round",
        &doc_id,
        saved_round_data("forced"),
        Some(fresh),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["revision"], json!(3));
}

#[tokio::test]
async fn delete_tombstone_visible_only_in_sync_pull() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws) = bootstrap(&ctx.state).await;
    let doc_id = Uuid::new_v4().to_string();

    let (_, created) = put_doc(
        &mut owner,
        &ws,
        "profile",
        &doc_id,
        profile_data("Doomed"),
        None,
        None,
    )
    .await;
    let create_seq = created["seq"].as_i64().unwrap();

    // Present in a plain list.
    let (_, list) = owner.get(&docs_url(&ws)).await;
    assert!(
        list["documents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| s(&d["doc_id"]) == doc_id)
    );

    // Delete (tombstone).
    let (status, body) = owner
        .send(
            Method::DELETE,
            &doc_url(&ws, "profile", &doc_id),
            Some(json!({ "base_revision": 1 })),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let delete_seq = body["seq"].as_i64().unwrap();
    assert!(delete_seq > create_seq);

    // Gone from a plain list, and GET one is 404.
    let (_, list) = owner.get(&docs_url(&ws)).await;
    assert!(
        !list["documents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| s(&d["doc_id"]) == doc_id),
        "tombstone must not appear in a plain list"
    );
    let (status, _) = owner.get(&doc_url(&ws, "profile", &doc_id)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // But a sync pull (since_seq) surfaces the tombstone with deleted=true.
    let (_, pull) = owner
        .get(&format!("{}?since_seq={}", docs_url(&ws), create_seq))
        .await;
    let tomb = pull["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| s(&d["doc_id"]) == doc_id)
        .expect("tombstone present in sync pull");
    assert_eq!(tomb["deleted"], json!(true));
}

#[tokio::test]
async fn kind_validation_and_size_limit() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws) = bootstrap(&ctx.state).await;

    // Garbage profile (missing required fields) → 422 invalid_document.
    let (status, body) = put_doc(
        &mut owner,
        &ws,
        "profile",
        &Uuid::new_v4().to_string(),
        json!({ "nonsense": true }),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(s(&body["error"]["code"]), "invalid_document");

    // Unknown kind in the path → 422 invalid_kind.
    let (status, body) = put_doc(
        &mut owner,
        &ws,
        "widget",
        &Uuid::new_v4().to_string(),
        saved_round_data("x"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(s(&body["error"]["code"]), "invalid_kind");

    // A valid saved_round is accepted (unknown extra field tolerated).
    let mut data = saved_round_data("ok");
    data["unknown_extra"] = json!("tolerated");
    let (status, body) = put_doc(
        &mut owner,
        &ws,
        "saved_round",
        &Uuid::new_v4().to_string(),
        data,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Oversize payload (> 64 KiB) → 413.
    let mut big = profile_data("Huge");
    big["blob"] = json!("x".repeat(70_000));
    let (status, body) = put_doc(
        &mut owner,
        &ws,
        "profile",
        &Uuid::new_v4().to_string(),
        big,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(s(&body["error"]["code"]), "payload_too_large");
}

#[tokio::test]
async fn push_math_pat_reads_but_cannot_write() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws) = bootstrap(&ctx.state).await;
    let doc_id = Uuid::new_v4().to_string();
    put_doc(
        &mut owner,
        &ws,
        "profile",
        &doc_id,
        profile_data("Seed"),
        None,
        None,
    )
    .await;

    // Mint a PAT scoped only to push:math (no `full`).
    let (status, created) = owner
        .post(
            "/api/tokens",
            json!({ "name": "ci", "scopes": ["push:math"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let token = s(&created["token"]).to_string();

    // Read is allowed (membership only).
    let (status, list) = owner
        .send(Method::GET, &docs_url(&ws), None, Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert!(!list["documents"].as_array().unwrap().is_empty());

    // Write is refused (needs the `full` scope).
    let (status, body) = put_doc(
        &mut owner,
        &ws,
        "profile",
        &doc_id,
        profile_data("Nope"),
        Some(1),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(s(&body["error"]["code"]), "insufficient_scope");

    // Delete is refused too.
    let (status, _) = owner
        .send(
            Method::DELETE,
            &doc_url(&ws, "profile", &doc_id),
            Some(json!({ "base_revision": 1 })),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn sync_pull_returns_only_newer_with_correct_latest_seq() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws) = bootstrap(&ctx.state).await;

    let (_, a) = put_doc(
        &mut owner,
        &ws,
        "profile",
        &Uuid::new_v4().to_string(),
        profile_data("A"),
        None,
        None,
    )
    .await;
    let seq_a = a["seq"].as_i64().unwrap();
    let (_, b) = put_doc(
        &mut owner,
        &ws,
        "profile",
        &Uuid::new_v4().to_string(),
        profile_data("B"),
        None,
        None,
    )
    .await;
    let seq_b = b["seq"].as_i64().unwrap();

    // Pull everything after A → only B, and latest_seq is B's seq.
    let (status, pull) = owner
        .get(&format!("{}?since_seq={seq_a}", docs_url(&ws)))
        .await;
    assert_eq!(status, StatusCode::OK, "{pull}");
    let docs = pull["documents"].as_array().unwrap();
    assert_eq!(docs.len(), 1, "only the newer doc");
    assert_eq!(docs[0]["seq"].as_i64().unwrap(), seq_b);
    assert_eq!(pull["latest_seq"].as_i64().unwrap(), seq_b);

    // Pulling after the newest yields nothing but the same cursor.
    let (_, pull) = owner
        .get(&format!("{}?since_seq={seq_b}", docs_url(&ws)))
        .await;
    assert_eq!(pull["documents"].as_array().unwrap().len(), 0);
    assert_eq!(pull["latest_seq"].as_i64().unwrap(), seq_b);
}

#[tokio::test]
async fn sse_delivers_document_event_and_guards_membership() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws) = bootstrap(&ctx.state).await;
    let cookie = owner.cookie_header().unwrap();

    // A non-member 404s; an anonymous request 401s.
    let (mut charlie, _) = register(&ctx.state, &unique_email(), "Charlie").await;
    let (status, _) = charlie.get(&format!("/api/workspaces/{ws}/events")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let mut anon = Client::new(&ctx.state);
    let (status, _) = anon.get(&format!("/api/workspaces/{ws}/events")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Subscribe, then a PUT publishes a `document` event.
    let mut stream = open_events_stream(&ctx.state, &ws, &cookie).await;
    let doc_id = Uuid::new_v4().to_string();
    let (status, _) = put_doc(
        &mut owner,
        &ws,
        "saved_round",
        &doc_id,
        saved_round_data("live"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    assert!(
        wait_for_event(&mut stream, "document").await,
        "expected a `document` SSE event after the PUT"
    );
}

#[tokio::test]
async fn sse_delivers_revision_pushed_on_commit() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws) = bootstrap(&ctx.state).await;
    let cookie = owner.cookie_header().unwrap();
    let game = "gates";

    let mut stream = open_events_stream(&ctx.state, &ws, &cookie).await;

    // Drive a real math revision commit (check → upload → commit); the owner
    // session's implicit `full` scope satisfies `push:math`.
    let index = br#"{"modes":[]}"#.to_vec();
    let files = json!([{ "path": "index.json", "hash": sha_hex(&index), "size": index.len() }]);
    let (status, check) = owner
        .send(
            Method::POST,
            &format!("/api/workspaces/{ws}/games/{game}/revisions/check"),
            Some(json!({ "files": files })),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{check}");
    let (status, _) = owner
        .send_bytes(
            Method::PUT,
            &format!(
                "/api/workspaces/{ws}/games/{game}/blobs/{}",
                sha_hex(&index)
            ),
            index.clone(),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, commit) = owner
        .send(
            Method::POST,
            &format!("/api/workspaces/{ws}/games/{game}/revisions"),
            Some(json!({ "message": "r1", "files": files })),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{commit}");

    assert!(
        wait_for_event(&mut stream, "revision_pushed").await,
        "expected a `revision_pushed` SSE event after the commit"
    );
}

#[tokio::test]
async fn delete_workspace_owner_only_and_cascades() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws) = bootstrap(&ctx.state).await;
    let ws_id: Uuid = sqlx::query_scalar("SELECT id FROM workspaces WHERE slug = $1")
        .bind(&ws)
        .fetch_one(&ctx.state.pool)
        .await
        .unwrap();

    // Seed a document so we can prove the cascade.
    put_doc(
        &mut owner,
        &ws,
        "profile",
        &Uuid::new_v4().to_string(),
        profile_data("Seed"),
        None,
        None,
    )
    .await;

    // Invite a plain member.
    let (_, invite) = owner
        .post(
            &format!("/api/workspaces/{ws}/invites"),
            json!({ "role": "member" }),
        )
        .await;
    let token = s(&invite["token"]).to_string();
    let (mut bob, _) = register(&ctx.state, &unique_email(), "Bob").await;
    bob.post(&format!("/api/invites/{token}/accept"), json!({}))
        .await;

    // A member cannot delete the workspace.
    let (status, body) = bob
        .send(Method::DELETE, &format!("/api/workspaces/{ws}"), None, None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // A non-member 404s (never learns it exists).
    let (mut charlie, _) = register(&ctx.state, &unique_email(), "Charlie").await;
    let (status, _) = charlie
        .send(Method::DELETE, &format!("/api/workspaces/{ws}"), None, None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The owner can, and it cascades.
    let (status, _) = owner
        .send(Method::DELETE, &format!("/api/workspaces/{ws}"), None, None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = owner.get(&format!("/api/workspaces/{ws}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "workspace should be gone");

    let doc_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM documents WHERE workspace_id = $1")
            .bind(ws_id)
            .fetch_one(&ctx.state.pool)
            .await
            .unwrap();
    assert_eq!(doc_count, 0, "documents should cascade with the workspace");
}

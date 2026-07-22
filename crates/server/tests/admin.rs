//! Instance-admin integration tests: the platform-operator surface (overview
//! stats, workspace plan overrides, user management, share moderation) driven
//! through the HTTP router. Every DB-backed test self-skips when
//! `TEST_DATABASE_URL` is unset, so `cargo test` stays green with no database.
//! The dev database persists between runs, so emails and slugs are UUID-suffixed
//! and instance-wide counts are asserted as monotonic deltas, never exact values.
//!
//! The three tests that mutate the global `users.is_admin` set (or read its
//! instance-wide count for the last-admin guard) serialize on [`FLAG_LOCK`] and
//! reset the flags they touch, so they stay deterministic against the shared DB.

use std::collections::HashMap;
use std::sync::LazyLock;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use server::config::{Config, StorageConfig, StripeConfig};
use server::{AppState, db, http, storage};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tower::ServiceExt;
use uuid::Uuid;

/// Serializes the tests that touch the global `is_admin` set so the instance-wide
/// flagged-admin count stays deterministic under `cargo`'s parallel test running.
static FLAG_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

const PLAY_DOMAIN: &str = "play.test";

// --- math fixture (a minimal, known-good revision, mirrors math_revisions.rs) --

const INDEX_JSON: &[u8] =
    br#"{"modes":[{"name":"base","cost":1,"events":"books_base.jsonl","weights":"lookup_base.csv"}]}"#;
const LOOKUP: &[u8] = b"0,9000,0\n1,900,100\n2,90,5000\n3,10,42000\n";
const BOOKS: &[u8] = b"{\"id\":0,\"events\":[]}\n{\"id\":1,\"events\":[]}\n";

fn rev_files() -> [(&'static str, &'static [u8]); 3] {
    [
        ("index.json", INDEX_JSON),
        ("lookup_base.csv", LOOKUP),
        ("books_base.jsonl", BOOKS),
    ]
}

// --- setup ------------------------------------------------------------------

struct Ctx {
    state: AppState,
    _tmp: tempfile::TempDir,
}

/// Billing-enabled config so plan resolution runs (free/override) instead of
/// short-circuiting to unlimited. The keys are placeholders — no Stripe call is hit.
fn stripe_config() -> StripeConfig {
    StripeConfig {
        secret_key: "sk_test_admin".to_string(),
        webhook_secret: "whsec_test".to_string(),
        price_seat_monthly: "price_seat_m".to_string(),
        price_seat_yearly: "price_seat_y".to_string(),
        price_storage: "price_storage".to_string(),
    }
}

async fn setup(admin_emails: Vec<String>, stripe: Option<StripeConfig>) -> Option<Ctx> {
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
        play_domain: Some(PLAY_DOMAIN.to_string()),
        admin_emails,
    };
    let pool = db::connect_lazy(&database_url).expect("lazy pool");
    let store = storage::build_object_store(&config).expect("fs store");
    let state = AppState::new(config, pool, store);
    db::migrate(&state.pool).await.expect("migrations apply");
    Some(Ctx { state, _tmp: tmp })
}

// --- HTTP client (cookies) --------------------------------------------------

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
    ) -> (StatusCode, Vec<u8>) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(header) = self.cookie_header() {
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
    ) -> (StatusCode, Value) {
        let (ct, raw) = match &body {
            Some(v) => (Some("application/json"), serde_json::to_vec(v).unwrap()),
            None => (None, Vec::new()),
        };
        let (status, bytes) = self.raw(method, uri, ct, raw).await;
        (status, parse_json(&bytes))
    }

    async fn post(&mut self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.send(Method::POST, uri, Some(body)).await
    }

    async fn get(&mut self, uri: &str) -> (StatusCode, Value) {
        self.send(Method::GET, uri, None).await
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

fn i(value: &Value) -> i64 {
    value.as_i64().unwrap()
}

fn unique_email() -> String {
    format!("user-{}@example.com", Uuid::new_v4())
}

fn unique_slug() -> String {
    format!("ws-{}", Uuid::new_v4())
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

/// Push one revision (check → upload missing → commit) as the (session-auth)
/// client. A session carries `full`, which satisfies the `push:math` scope.
async fn push_revision(client: &mut Client, ws: &str, game: &str) {
    let files = rev_files();
    let m = manifest(&files);
    let (status, body) = client
        .send(
            Method::POST,
            &format!("/api/workspaces/{ws}/games/{game}/revisions/check"),
            Some(json!({ "files": m })),
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
                .raw(
                    Method::PUT,
                    &format!("/api/workspaces/{ws}/games/{game}/blobs/{hash}"),
                    Some("application/octet-stream"),
                    bytes.to_vec(),
                )
                .await;
            assert!(
                status == StatusCode::CREATED || status == StatusCode::OK,
                "put {path}: {:?}",
                String::from_utf8_lossy(&up)
            );
        }
    }
    let (status, body) = client
        .send(
            Method::POST,
            &format!("/api/workspaces/{ws}/games/{game}/revisions"),
            Some(json!({ "message": "seed", "files": m })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "commit: {body}");
}

// --- direct SQL helpers -----------------------------------------------------

async fn workspace_id(state: &AppState, slug: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM workspaces WHERE slug = $1")
        .bind(slug)
        .fetch_one(&state.pool)
        .await
        .unwrap()
}

async fn user_id_by_email(state: &AppState, email: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE lower(email) = lower($1)")
        .bind(email)
        .fetch_one(&state.pool)
        .await
        .unwrap()
}

async fn set_is_admin(state: &AppState, user_id: Uuid, is_admin: bool) {
    sqlx::query("UPDATE users SET is_admin = $2 WHERE id = $1")
        .bind(user_id)
        .bind(is_admin)
        .execute(&state.pool)
        .await
        .unwrap();
}

/// Clears every `is_admin` flag instance-wide (only ever run while holding
/// [`FLAG_LOCK`]).
async fn reset_admin_flags(state: &AppState) {
    sqlx::query("UPDATE users SET is_admin = false")
        .execute(&state.pool)
        .await
        .unwrap();
}

async fn insert_game(state: &AppState, ws_id: Uuid, slug: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO games (workspace_id, slug, name) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(ws_id)
    .bind(slug)
    .bind(slug)
    .fetch_one(&state.pool)
    .await
    .unwrap()
}

async fn insert_share(
    state: &AppState,
    ws_id: Uuid,
    game_id: Uuid,
    slug: &str,
    sessions: i64,
    spins: i64,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO share_links (workspace_id, game_id, slug, sessions_count, spins_count) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(ws_id)
    .bind(game_id)
    .bind(slug)
    .bind(sessions)
    .bind(spins)
    .fetch_one(&state.pool)
    .await
    .unwrap()
}

/// Fetch a single workspace row off the admin list (filtered by its unique slug).
async fn admin_ws(client: &mut Client, ws: &str) -> Value {
    let (status, body) = client
        .get(&format!("/api/admin/workspaces?query={ws}"))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["slug"].as_str() == Some(ws))
        .cloned()
        .expect("workspace present on admin list")
}

// ===========================================================================
// gating: the surface is hidden from non-admins
// ===========================================================================

#[tokio::test]
async fn non_admin_member_gets_404_on_every_admin_route() {
    let Some(ctx) = setup(Vec::new(), None).await else {
        return;
    };
    let mut member = register(&ctx.state, &unique_email(), "Member").await;

    // /me and every list route: a hidden 404, never a 403.
    for uri in [
        "/api/admin/me",
        "/api/admin/overview",
        "/api/admin/workspaces",
        "/api/admin/users",
        "/api/admin/shares",
    ] {
        let (status, body) = member.get(uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
    }

    // Mutating routes too.
    let fake = Uuid::new_v4();
    let (status, _) = member
        .send(
            Method::PUT,
            &format!("/api/admin/workspaces/{fake}/override"),
            Some(json!({ "plan": "paid", "seats": 1 })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = member
        .send(
            Method::PUT,
            &format!("/api/admin/users/{fake}/admin"),
            Some(json!({ "is_admin": true })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = member
        .send(
            Method::POST,
            &format!("/api/admin/shares/{fake}/revoke"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn env_email_admin_without_flag_has_access() {
    let email = unique_email();
    let Some(ctx) = setup(vec![email.clone()], None).await else {
        return;
    };
    let mut admin = register(&ctx.state, &email, "Env Admin").await;

    let (status, body) = admin.get("/api/admin/me").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["is_admin"], json!(true));

    // Access is purely via the env allowlist — the DB flag is still false.
    let uid = user_id_by_email(&ctx.state, &email).await;
    let flag: bool = sqlx::query_scalar("SELECT is_admin FROM users WHERE id = $1")
        .bind(uid)
        .fetch_one(&ctx.state.pool)
        .await
        .unwrap();
    assert!(!flag, "env admin should not carry the is_admin flag");

    let (status, _) = admin.get("/api/admin/overview").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn is_admin_flagged_user_has_access() {
    let Some(ctx) = setup(Vec::new(), None).await else {
        return;
    };
    let _guard = FLAG_LOCK.lock().await;

    let email = unique_email();
    let mut user = register(&ctx.state, &email, "Flagged").await;

    // Empty env list + flag false → not an admin yet.
    let (status, _) = user.get("/api/admin/me").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The SQL bootstrap path: flip the flag directly.
    let uid = user_id_by_email(&ctx.state, &email).await;
    set_is_admin(&ctx.state, uid, true).await;
    let (status, body) = user.get("/api/admin/me").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["is_admin"], json!(true));

    // Leave the flagged set as we found it.
    set_is_admin(&ctx.state, uid, false).await;
}

// ===========================================================================
// overview
// ===========================================================================

#[tokio::test]
async fn overview_counts_move_with_new_data() {
    let email = unique_email();
    let Some(ctx) = setup(vec![email.clone()], None).await else {
        return;
    };
    let mut admin = register(&ctx.state, &email, "Admin").await;

    let (status, before) = admin.get("/api/admin/overview").await;
    assert_eq!(status, StatusCode::OK, "{before}");
    // Both series are exactly 30 days of {date, count}.
    assert_eq!(before["signups_30d"].as_array().unwrap().len(), 30);
    assert_eq!(before["pushes_30d"].as_array().unwrap().len(), 30);
    assert!(before["signups_30d"][29]["date"].is_string());
    assert!(before["pushes_30d"][0]["count"].is_number());

    // A workspace + a pushed revision adds a workspace, a game, and a revision.
    let ws = unique_slug();
    let (status, _) = admin
        .post(
            "/api/workspaces",
            json!({ "name": "Overview WS", "slug": &ws }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    push_revision(&mut admin, &ws, "demo").await;

    let (_, after) = admin.get("/api/admin/overview").await;
    // Instance-wide counts only grow (peers run in parallel), so assert deltas.
    assert!(i(&after["workspaces"]) > i(&before["workspaces"]));
    assert!(i(&after["games"]) > i(&before["games"]));
    assert!(i(&after["revisions"]) > i(&before["revisions"]));
    assert!(i(&after["storage_bytes"]) >= i(&before["storage_bytes"]));
}

// ===========================================================================
// workspaces list + plan overrides
// ===========================================================================

#[tokio::test]
async fn workspace_list_shows_storage_and_resolved_plan() {
    let email = unique_email();
    // No Stripe → every plan resolves to "unlimited".
    let Some(ctx) = setup(vec![email.clone()], None).await else {
        return;
    };
    let mut admin = register(&ctx.state, &email, "Admin").await;
    let ws = unique_slug();
    admin
        .post(
            "/api/workspaces",
            json!({ "name": "Store WS", "slug": &ws }),
        )
        .await;
    push_revision(&mut admin, &ws, "demo").await;

    let row = admin_ws(&mut admin, &ws).await;
    assert_eq!(s(&row["plan"]), "unlimited");
    assert_eq!(row["override"], json!(null));
    assert_eq!(row["subscription_status"], json!(null));
    assert_eq!(i(&row["members"]), 1);
    assert_eq!(i(&row["games"]), 1);
    let expected = (INDEX_JSON.len() + LOOKUP.len() + BOOKS.len()) as i64;
    assert_eq!(i(&row["storage_bytes"]), expected);
}

#[tokio::test]
async fn override_grant_flips_resolved_plan() {
    let email = unique_email();
    let Some(ctx) = setup(vec![email.clone()], Some(stripe_config())).await else {
        return;
    };
    let mut admin = register(&ctx.state, &email, "Admin").await;
    let ws = unique_slug();
    admin
        .post("/api/workspaces", json!({ "name": "Comp WS", "slug": &ws }))
        .await;
    let ws_id = workspace_id(&ctx.state, &ws).await;

    // Fresh workspace on a billing-enabled instance → free, no override.
    let row = admin_ws(&mut admin, &ws).await;
    assert_eq!(s(&row["plan"]), "free");
    assert_eq!(row["override"], json!(null));

    // Comp a paid plan (8 seats) → the response echoes the flipped resolution + raw row.
    let (status, granted) = admin
        .send(
            Method::PUT,
            &format!("/api/admin/workspaces/{ws_id}/override"),
            Some(json!({ "plan": "paid", "seats": 8, "note": "comped for launch" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{granted}");
    assert_eq!(s(&granted["plan"]), "paid");
    assert_eq!(granted["seats"], json!(8));
    assert_eq!(s(&granted["override"]["plan"]), "paid");
    assert_eq!(granted["override"]["seats"], json!(8));
    assert_eq!(s(&granted["override"]["note"]), "comped for launch");
    assert_eq!(granted["override"]["expires_at"], json!(null));

    // Reflected on a fresh admin read…
    let reread = admin_ws(&mut admin, &ws).await;
    assert_eq!(s(&reread["plan"]), "paid");
    assert_eq!(reread["seats"], json!(8));
    // …and the override wins over the Free state in the workspace's own billing view.
    let (_, billing) = admin.get(&format!("/api/workspaces/{ws}/billing")).await;
    assert_eq!(s(&billing["plan"]), "paid");
    assert_eq!(billing["seats"], json!(8));
    // 8 seats → 8 members allowed.
    assert_eq!(billing["limits"]["max_members"], json!(8));

    // Null plan clears it → back to free.
    let (status, cleared) = admin
        .send(
            Method::PUT,
            &format!("/api/admin/workspaces/{ws_id}/override"),
            Some(json!({ "plan": null })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert_eq!(s(&cleared["plan"]), "free");
    assert_eq!(cleared["override"], json!(null));

    // An invalid plan value is rejected.
    let (status, err) = admin
        .send(
            Method::PUT,
            &format!("/api/admin/workspaces/{ws_id}/override"),
            Some(json!({ "plan": "enterprise" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(s(&err["error"]["code"]), "invalid_plan");

    // A "paid" comp without seats is rejected.
    let (status, err) = admin
        .send(
            Method::PUT,
            &format!("/api/admin/workspaces/{ws_id}/override"),
            Some(json!({ "plan": "paid" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(s(&err["error"]["code"]), "invalid_seats");

    // A "paid" comp with out-of-range seats is rejected.
    let (status, err) = admin
        .send(
            Method::PUT,
            &format!("/api/admin/workspaces/{ws_id}/override"),
            Some(json!({ "plan": "paid", "seats": 101 })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(s(&err["error"]["code"]), "invalid_seats");

    // An "unlimited" comp needs no seats.
    let (status, granted) = admin
        .send(
            Method::PUT,
            &format!("/api/admin/workspaces/{ws_id}/override"),
            Some(json!({ "plan": "unlimited" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{granted}");
    assert_eq!(s(&granted["plan"]), "unlimited");
    assert_eq!(granted["seats"], json!(null));
    assert_eq!(granted["override"]["seats"], json!(null));
}

#[tokio::test]
async fn expired_override_is_ignored_but_still_listed() {
    let email = unique_email();
    let Some(ctx) = setup(vec![email.clone()], Some(stripe_config())).await else {
        return;
    };
    let mut admin = register(&ctx.state, &email, "Admin").await;
    let ws = unique_slug();
    admin
        .post("/api/workspaces", json!({ "name": "Exp WS", "slug": &ws }))
        .await;
    let ws_id = workspace_id(&ctx.state, &ws).await;

    // Grant with a future expiry → plan flips to paid.
    admin
        .send(
            Method::PUT,
            &format!("/api/admin/workspaces/{ws_id}/override"),
            Some(json!({ "plan": "paid", "seats": 10, "expires_in_days": 30 })),
        )
        .await;
    let granted = admin_ws(&mut admin, &ws).await;
    assert_eq!(s(&granted["plan"]), "paid");
    assert_eq!(granted["seats"], json!(10));

    // Push the expiry into the past → the override is ignored for resolution.
    sqlx::query(
        "UPDATE plan_overrides SET expires_at = now() - INTERVAL '1 day' WHERE workspace_id = $1",
    )
    .bind(ws_id)
    .execute(&ctx.state.pool)
    .await
    .unwrap();

    let row = admin_ws(&mut admin, &ws).await;
    assert_eq!(s(&row["plan"]), "free", "expired override must be ignored");
    // …but the raw row is still surfaced on the list.
    assert_eq!(s(&row["override"]["plan"]), "paid");
    assert_eq!(row["override"]["seats"], json!(10));
    assert!(row["override"]["expires_at"].is_string());

    // The workspace's own billing view agrees (override hook honors expiry).
    let (_, billing) = admin.get(&format!("/api/workspaces/{ws}/billing")).await;
    assert_eq!(s(&billing["plan"]), "free");
}

#[tokio::test]
async fn override_on_unknown_workspace_is_404() {
    let email = unique_email();
    let Some(ctx) = setup(vec![email.clone()], Some(stripe_config())).await else {
        return;
    };
    let mut admin = register(&ctx.state, &email, "Admin").await;
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/api/admin/workspaces/{}/override", Uuid::new_v4()),
            Some(json!({ "plan": "unlimited" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(s(&body["error"]["code"]), "workspace_not_found");
}

// ===========================================================================
// users list + admin toggle + last-admin guard
// ===========================================================================

#[tokio::test]
async fn users_list_and_toggle_admin() {
    let email = unique_email();
    let Some(ctx) = setup(vec![email.clone()], None).await else {
        return;
    };
    let _guard = FLAG_LOCK.lock().await;

    let mut admin = register(&ctx.state, &email, "Admin").await;
    let member_email = unique_email();
    let _member = register(&ctx.state, &member_email, "Member").await;
    let member_id = user_id_by_email(&ctx.state, &member_email).await;

    // The member shows up with the raw flag (false) and a 0 membership count.
    let (status, body) = admin
        .get(&format!("/api/admin/users?query={member_email}"))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = body["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["email"].as_str() == Some(member_email.as_str()))
        .expect("member listed")
        .clone();
    assert_eq!(row["is_admin"], json!(false));
    assert_eq!(i(&row["workspaces"]), 0);

    // Promote the member.
    let (status, res) = admin
        .send(
            Method::PUT,
            &format!("/api/admin/users/{member_id}/admin"),
            Some(json!({ "is_admin": true })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["is_admin"], json!(true));

    // Reflected on a re-read.
    let (_, body) = admin
        .get(&format!("/api/admin/users?query={member_email}"))
        .await;
    let row = body["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["email"].as_str() == Some(member_email.as_str()))
        .unwrap()
        .clone();
    assert_eq!(row["is_admin"], json!(true));

    // Demote again (cleanup).
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/api/admin/users/{member_id}/admin"),
            Some(json!({ "is_admin": false })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Unknown user → 404.
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/api/admin/users/{}/admin", Uuid::new_v4()),
            Some(json!({ "is_admin": true })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn last_admin_guard_blocks_self_demotion() {
    let Some(ctx) = setup(Vec::new(), None).await else {
        return;
    };
    let _guard = FLAG_LOCK.lock().await;
    // Clean slate: no flagged admins anywhere.
    reset_admin_flags(&ctx.state).await;

    // A flagged admin who is the only one and is NOT covered by the env list.
    let email = unique_email();
    let mut admin = register(&ctx.state, &email, "Sole Admin").await;
    let uid = user_id_by_email(&ctx.state, &email).await;
    set_is_admin(&ctx.state, uid, true).await;

    // Removing their own flag as the last admin → 409 last_admin.
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/api/admin/users/{uid}/admin"),
            Some(json!({ "is_admin": false })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(s(&body["error"]["code"]), "last_admin");
    // The refusal left them an admin.
    let (status, _) = admin.get("/api/admin/me").await;
    assert_eq!(status, StatusCode::OK);

    // With a SECOND flagged admin present, self-demotion is allowed.
    let email2 = unique_email();
    let _other = register(&ctx.state, &email2, "Other Admin").await;
    let uid2 = user_id_by_email(&ctx.state, &email2).await;
    set_is_admin(&ctx.state, uid2, true).await;

    let (status, res) = admin
        .send(
            Method::PUT,
            &format!("/api/admin/users/{uid}/admin"),
            Some(json!({ "is_admin": false })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["is_admin"], json!(false));

    // Leave the flagged set clean for peers.
    reset_admin_flags(&ctx.state).await;
}

// ===========================================================================
// shares list + revoke
// ===========================================================================

#[tokio::test]
async fn shares_list_and_revoke() {
    let email = unique_email();
    let Some(ctx) = setup(vec![email.clone()], None).await else {
        return;
    };
    let mut admin = register(&ctx.state, &email, "Admin").await;
    let ws = unique_slug();
    admin
        .post(
            "/api/workspaces",
            json!({ "name": "Share WS", "slug": &ws }),
        )
        .await;
    let ws_id = workspace_id(&ctx.state, &ws).await;
    let game_id = insert_game(&ctx.state, ws_id, "demo").await;
    let share_slug = format!("sh-{}", Uuid::new_v4().simple());
    let share_id = insert_share(&ctx.state, ws_id, game_id, &share_slug, 7, 42).await;

    // Listed with counters, the joined workspace + game slugs, and a play URL.
    let (status, body) = admin
        .get(&format!("/api/admin/shares?query={share_slug}"))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = body["shares"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["slug"].as_str() == Some(share_slug.as_str()))
        .expect("share listed")
        .clone();
    assert_eq!(s(&row["workspace_slug"]), ws);
    assert_eq!(s(&row["game"]), "demo");
    assert_eq!(i(&row["sessions_count"]), 7);
    assert_eq!(i(&row["spins_count"]), 42);
    assert_eq!(row["revoked_at"], json!(null));
    assert_eq!(
        s(&row["url"]),
        format!("https://{share_slug}.{PLAY_DOMAIN}/")
    );

    // Revoke → 200 and revoked_at is set.
    let (status, _) = admin
        .send(
            Method::POST,
            &format!("/api/admin/shares/{share_id}/revoke"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = admin
        .get(&format!("/api/admin/shares?query={share_slug}"))
        .await;
    let row = body["shares"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["slug"].as_str() == Some(share_slug.as_str()))
        .unwrap()
        .clone();
    assert!(row["revoked_at"].is_string(), "revoked_at should be set");

    // Idempotent: revoking an already-revoked link still 200s.
    let (status, _) = admin
        .send(
            Method::POST,
            &format!("/api/admin/shares/{share_id}/revoke"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Unknown share → 404.
    let (status, _) = admin
        .send(
            Method::POST,
            &format!("/api/admin/shares/{}/revoke", Uuid::new_v4()),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

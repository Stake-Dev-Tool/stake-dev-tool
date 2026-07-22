//! M1 integration tests: identity + workspaces, driven entirely through the
//! HTTP API. Every test self-skips when `TEST_DATABASE_URL` is unset, so
//! `cargo test` is green with no database running. The dev database persists
//! between runs, so all emails and slugs are suffixed with a fresh UUID.

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use server::config::{Config, StorageConfig};
use server::{AppState, db, http, storage};
use tower::ServiceExt; // brings `oneshot` onto Router
use uuid::Uuid;

/// Holds the state plus the temp dir backing the (unused-by-auth) object store,
/// so the directory outlives the test.
struct Ctx {
    state: AppState,
    _tmp: tempfile::TempDir,
}

/// Builds real state against `TEST_DATABASE_URL` and applies migrations, or
/// returns `None` to skip.
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
        stripe: None,
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

/// A browser-like client: shares the app state and carries a cookie jar so
/// sessions round-trip across requests. Bearer requests bypass the jar.
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

    async fn send(
        &mut self,
        method: Method,
        uri: &str,
        body: Option<Value>,
        bearer: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = bearer {
            builder = builder.header("authorization", format!("Bearer {token}"));
        } else if let Some(header) = self.cookie_header() {
            builder = builder.header("cookie", header);
        }
        let request = match body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&value).unwrap()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };

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
        let json = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, json)
    }

    async fn post(&mut self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.send(Method::POST, uri, Some(body), None).await
    }

    async fn get(&mut self, uri: &str) -> (StatusCode, Value) {
        self.send(Method::GET, uri, None, None).await
    }
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

/// Registers a password account and returns the logged-in client plus the
/// created user object.
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

#[tokio::test]
async fn register_then_me_roundtrips_the_cookie() {
    let Some(ctx) = setup().await else {
        return;
    };
    let email = unique_email();
    let (mut client, user) = register(&ctx.state, &email, "Alice").await;
    assert_eq!(s(&user["email"]), email);
    assert!(
        client.cookies.contains_key("sdt_session"),
        "register must set the session cookie"
    );

    let (status, body) = client.get("/api/auth/me").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(s(&body["user"]["email"]), email);

    // No cookie → 401.
    let mut anon = Client::new(&ctx.state);
    let (status, _) = anon.get("/api/auth/me").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn providers_reports_password_only_by_default() {
    let Some(ctx) = setup().await else {
        return;
    };
    let mut client = Client::new(&ctx.state);
    let (status, body) = client.get("/api/auth/providers").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["password"], json!(true));
    assert_eq!(body["github"], json!(false));
}

#[tokio::test]
async fn login_wrong_password_is_uniform_401() {
    let Some(ctx) = setup().await else {
        return;
    };
    let email = unique_email();
    register(&ctx.state, &email, "Bob").await;

    let mut anon = Client::new(&ctx.state);
    // Wrong password.
    let (status, body) = anon
        .post(
            "/api/auth/login",
            json!({ "email": &email, "password": "wrong-password" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(s(&body["error"]["code"]), "invalid_credentials");

    // Unknown email → same answer.
    let (status, body) = anon
        .post(
            "/api/auth/login",
            json!({ "email": unique_email(), "password": "whatever0" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(s(&body["error"]["code"]), "invalid_credentials");

    // Correct password → session.
    let (status, _) = anon
        .post(
            "/api/auth/login",
            json!({ "email": &email, "password": "password123" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn duplicate_email_is_409() {
    let Some(ctx) = setup().await else {
        return;
    };
    let email = unique_email();
    register(&ctx.state, &email, "First").await;

    let mut anon = Client::new(&ctx.state);
    let (status, body) = anon
        .post(
            "/api/auth/register",
            json!({ "email": &email, "password": "password123", "display_name": "Second" }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(s(&body["error"]["code"]), "email_taken");
}

#[tokio::test]
async fn login_rate_limit_kicks_in_after_ten_failures() {
    let Some(ctx) = setup().await else {
        return;
    };
    let email = unique_email();
    register(&ctx.state, &email, "Target").await;

    let mut anon = Client::new(&ctx.state);
    for attempt in 1..=10 {
        let (status, _) = anon
            .post(
                "/api/auth/login",
                json!({ "email": &email, "password": "wrong-password" }),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "attempt {attempt} should be 401"
        );
    }
    // 11th attempt is throttled before the password is even checked.
    let (status, body) = anon
        .post(
            "/api/auth/login",
            json!({ "email": &email, "password": "password123" }),
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(s(&body["error"]["code"]), "rate_limited");
}

#[tokio::test]
async fn workspace_create_validation_and_duplicate_slug() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, _) = register(&ctx.state, &unique_email(), "Owner").await;
    let slug = unique_slug();

    let (status, body) = owner
        .post("/api/workspaces", json!({ "name": "My WS", "slug": &slug }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(s(&body["role"]), "owner");
    assert_eq!(s(&body["slug"]), slug);

    // Invalid slug → 422.
    let (status, body) = owner
        .post(
            "/api/workspaces",
            json!({ "name": "Bad", "slug": "Not A Slug" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(s(&body["error"]["code"]), "invalid_slug");

    // Duplicate slug → 409.
    let (status, body) = owner
        .post("/api/workspaces", json!({ "name": "Dup", "slug": &slug }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(s(&body["error"]["code"]), "slug_taken");

    // It shows up in the caller's list and detail.
    let (status, body) = owner.get("/api/workspaces").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| s(&w["slug"]) == slug)
    );

    let (status, body) = owner.get(&format!("/api/workspaces/{slug}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["members"].as_array().unwrap().len(), 1);
}

/// The M1 acceptance test: two accounts share a workspace via an invite link.
#[tokio::test]
async fn two_accounts_share_a_workspace_via_invite() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut alice, alice_user) = register(&ctx.state, &unique_email(), "Alice").await;
    let slug = unique_slug();
    let (status, _) = alice
        .post(
            "/api/workspaces",
            json!({ "name": "Shared", "slug": &slug }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Alice mints a member invite.
    let (status, invite) = alice
        .post(
            &format!("/api/workspaces/{slug}/invites"),
            json!({ "role": "member" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{invite}");
    let token = s(&invite["token"]).to_string();
    assert!(token.starts_with("sdt_inv_"));

    // Bob registers and previews the invite (public, no auth needed here but he
    // is logged in), then accepts it.
    let (mut bob, bob_user) = register(&ctx.state, &unique_email(), "Bob").await;
    let (status, preview) = bob.get(&format!("/api/invites/{token}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preview["valid"], json!(true));
    assert_eq!(s(&preview["role"]), "member");

    let (status, accepted) = bob
        .post(&format!("/api/invites/{token}/accept"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!(s(&accepted["workspace"]["slug"]), slug);
    assert_eq!(s(&accepted["workspace"]["role"]), "member");

    // Alice sees both members with the right roles.
    let (status, detail) = alice.get(&format!("/api/workspaces/{slug}")).await;
    assert_eq!(status, StatusCode::OK);
    let members = detail["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    assert_eq!(member_role(&detail, s(&alice_user["id"])), Some("owner"));
    assert_eq!(member_role(&detail, s(&bob_user["id"])), Some("member"));

    // Bob now lists the workspace as a member.
    let (status, list) = bob.get("/api/workspaces").await;
    assert_eq!(status, StatusCode::OK);
    let mine = list["workspaces"].as_array().unwrap();
    assert!(
        mine.iter()
            .any(|w| s(&w["slug"]) == slug && s(&w["role"]) == "member")
    );
}

fn member_role<'a>(detail: &'a Value, user_id: &str) -> Option<&'a str> {
    detail["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| s(&m["id"]) == user_id)
        .map(|m| s(&m["role"]))
}

#[tokio::test]
async fn invite_max_uses_one_exhausts_after_one_accept() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, _) = register(&ctx.state, &unique_email(), "Owner").await;
    let slug = unique_slug();
    owner
        .post(
            "/api/workspaces",
            json!({ "name": "Limited", "slug": &slug }),
        )
        .await;
    let (_, invite) = owner
        .post(
            &format!("/api/workspaces/{slug}/invites"),
            json!({ "role": "member", "max_uses": 1 }),
        )
        .await;
    let token = s(&invite["token"]).to_string();

    let (mut first, _) = register(&ctx.state, &unique_email(), "First").await;
    let (status, _) = first
        .post(&format!("/api/invites/{token}/accept"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);

    let (mut second, _) = register(&ctx.state, &unique_email(), "Second").await;
    let (status, body) = second
        .post(&format!("/api/invites/{token}/accept"), json!({}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(s(&body["error"]["code"]), "invite_exhausted");
}

#[tokio::test]
async fn expired_invite_is_rejected() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, _) = register(&ctx.state, &unique_email(), "Owner").await;
    let slug = unique_slug();
    owner
        .post(
            "/api/workspaces",
            json!({ "name": "Expiring", "slug": &slug }),
        )
        .await;
    let (_, invite) = owner
        .post(
            &format!("/api/workspaces/{slug}/invites"),
            json!({ "role": "member" }),
        )
        .await;
    let token = s(&invite["token"]).to_string();
    let invite_id = Uuid::parse_str(s(&invite["info"]["id"])).unwrap();

    // Force it into the past.
    sqlx::query("UPDATE invites SET expires_at = now() - INTERVAL '1 day' WHERE id = $1")
        .bind(invite_id)
        .execute(&ctx.state.pool)
        .await
        .unwrap();

    // Preview reports invalid, accept is refused.
    let (mut guest, _) = register(&ctx.state, &unique_email(), "Guest").await;
    let (status, preview) = guest.get(&format!("/api/invites/{token}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preview["valid"], json!(false));

    let (status, body) = guest
        .post(&format!("/api/invites/{token}/accept"), json!({}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(s(&body["error"]["code"]), "invite_expired");
}

#[tokio::test]
async fn only_admins_and_owners_can_create_invites() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, _) = register(&ctx.state, &unique_email(), "Owner").await;
    let slug = unique_slug();
    owner
        .post("/api/workspaces", json!({ "name": "Roles", "slug": &slug }))
        .await;

    // Owner invites Bob as a plain member.
    let (_, invite) = owner
        .post(
            &format!("/api/workspaces/{slug}/invites"),
            json!({ "role": "member" }),
        )
        .await;
    let token = s(&invite["token"]).to_string();
    let (mut bob, bob_user) = register(&ctx.state, &unique_email(), "Bob").await;
    bob.post(&format!("/api/invites/{token}/accept"), json!({}))
        .await;

    // As a member, Bob cannot create invites.
    let (status, body) = bob
        .post(
            &format!("/api/workspaces/{slug}/invites"),
            json!({ "role": "member" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(s(&body["error"]["code"]), "forbidden");

    // Owner promotes Bob to admin.
    let bob_id = s(&bob_user["id"]);
    let (status, _) = owner
        .send(
            Method::PATCH,
            &format!("/api/workspaces/{slug}/members/{bob_id}"),
            Some(json!({ "role": "admin" })),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Now Bob (admin) can create invites.
    let (status, _) = bob
        .post(
            &format!("/api/workspaces/{slug}/invites"),
            json!({ "role": "member" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn pat_authenticates_bearer_but_cannot_mint_tokens() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, _) = register(&ctx.state, &unique_email(), "Owner").await;
    let slug = unique_slug();
    owner
        .post(
            "/api/workspaces",
            json!({ "name": "Tokened", "slug": &slug }),
        )
        .await;

    // Mint a PAT (session-auth).
    let (status, created) = owner
        .post(
            "/api/tokens",
            json!({ "name": "ci", "scopes": ["full", "push:math"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let token = s(&created["token"]).to_string();
    assert!(token.starts_with("sdt_pat_"));

    // The PAT authenticates workspace reads over Bearer.
    let mut bearer = Client::new(&ctx.state);
    let (status, body) = bearer
        .send(Method::GET, "/api/workspaces", None, Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| s(&w["slug"]) == slug)
    );

    // But a PAT must not mint PATs.
    let (status, body) = bearer
        .send(
            Method::POST,
            "/api/tokens",
            Some(json!({ "name": "nope", "scopes": ["full"] })),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(s(&body["error"]["code"]), "session_required");
}

#[tokio::test]
async fn revoked_pat_stops_authenticating() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, _) = register(&ctx.state, &unique_email(), "Owner").await;
    let (status, created) = owner
        .post("/api/tokens", json!({ "name": "temp", "scopes": ["full"] }))
        .await;
    assert_eq!(status, StatusCode::OK);
    let token = s(&created["token"]).to_string();
    let token_id = s(&created["info"]["id"]).to_string();

    // Works before revocation.
    let mut bearer = Client::new(&ctx.state);
    let (status, _) = bearer
        .send(Method::GET, "/api/auth/me", None, Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK);

    // Revoke (session-auth).
    let (status, _) = owner
        .send(
            Method::DELETE,
            &format!("/api/tokens/{token_id}"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Now the Bearer token is rejected.
    let (status, body) = bearer
        .send(Method::GET, "/api/auth/me", None, Some(&token))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(s(&body["error"]["code"]), "invalid_token");
}

#[tokio::test]
async fn device_flow_yields_a_usable_token() {
    let Some(ctx) = setup().await else {
        return;
    };
    let mut device = Client::new(&ctx.state);
    let (status, code) = device.post("/api/auth/device/code", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{code}");
    let device_code = s(&code["device_code"]).to_string();
    let user_code = s(&code["user_code"]).to_string();
    assert_eq!(code["interval"], json!(5));

    // Pending before approval.
    let (status, body) = device
        .post(
            "/api/auth/device/token",
            json!({ "device_code": &device_code }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(s(&body["error"]), "authorization_pending");

    // A signed-in user approves the code.
    let email = unique_email();
    let (mut user, _) = register(&ctx.state, &email, "Device Owner").await;
    let (status, _) = user
        .post(
            "/api/auth/device/approve",
            json!({ "user_code": &user_code, "approve": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // The next poll returns a token.
    let (status, body) = device
        .post(
            "/api/auth/device/token",
            json!({ "device_code": &device_code }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = s(&body["token"]).to_string();
    assert!(token.starts_with("sdt_pat_"));

    // The minted token authenticates as the approving user.
    let mut bearer = Client::new(&ctx.state);
    let (status, me) = bearer
        .send(Method::GET, "/api/auth/me", None, Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(s(&me["user"]["email"]), email);
}

#[tokio::test]
async fn last_owner_cannot_leave_and_promotion_frees_them() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, owner_user) = register(&ctx.state, &unique_email(), "Owner").await;
    let owner_id = s(&owner_user["id"]).to_string();
    let slug = unique_slug();
    owner
        .post("/api/workspaces", json!({ "name": "Solo", "slug": &slug }))
        .await;

    // The sole owner cannot leave...
    let (status, body) = owner
        .send(
            Method::DELETE,
            &format!("/api/workspaces/{slug}/members/{owner_id}"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(s(&body["error"]["code"]), "last_owner");

    // ...nor demote themselves.
    let (status, body) = owner
        .send(
            Method::PATCH,
            &format!("/api/workspaces/{slug}/members/{owner_id}"),
            Some(json!({ "role": "member" })),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(s(&body["error"]["code"]), "last_owner");

    // Bring in a second member and promote them to owner.
    let (_, invite) = owner
        .post(
            &format!("/api/workspaces/{slug}/invites"),
            json!({ "role": "member" }),
        )
        .await;
    let token = s(&invite["token"]).to_string();
    let (mut heir, heir_user) = register(&ctx.state, &unique_email(), "Heir").await;
    heir.post(&format!("/api/invites/{token}/accept"), json!({}))
        .await;
    let heir_id = s(&heir_user["id"]).to_string();
    let (status, _) = owner
        .send(
            Method::PATCH,
            &format!("/api/workspaces/{slug}/members/{heir_id}"),
            Some(json!({ "role": "owner" })),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // With a second owner present, the original owner may now leave.
    let (status, _) = owner
        .send(
            Method::DELETE,
            &format!("/api/workspaces/{slug}/members/{owner_id}"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

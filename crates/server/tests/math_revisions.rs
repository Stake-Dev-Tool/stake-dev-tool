//! M2 integration tests: content-addressed math revisions, driven entirely
//! through the HTTP API. Every test self-skips when `TEST_DATABASE_URL` is
//! unset, so `cargo test` stays green with no database. The dev database
//! persists between runs, so all emails and slugs are suffixed with a UUID.

use std::collections::HashMap;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use server::config::{Config, StorageConfig};
use server::{AppState, db, http, storage};
use sha2::{Digest, Sha256};
use tower::ServiceExt; // brings `oneshot` onto Router
use uuid::Uuid;

// --- fixture math folder (index.json + one lookup CSV + a tiny fake books file) ---

const INDEX_JSON: &[u8] =
    br#"{"modes":[{"name":"base","cost":1,"events":"books_base.jsonl","weights":"lookup_base.csv"}]}"#;
// rev1 base: total weight 10000, sum(w*payout)=960000
//   rtp = 960000/100/10000/1 = 0.96 ; max_win = 42000/100 = 420 ; hit_rate = 0.10
const LOOKUP_V1: &[u8] = b"0,9000,0\n1,900,100\n2,90,5000\n3,10,42000\n";
// rev2 base: total weight 10000, sum(w*payout)=910000 → rtp = 0.91 ; hit_rate = 0.05
const LOOKUP_V2: &[u8] = b"0,9500,0\n1,400,100\n2,90,5000\n3,10,42000\n";
// Books are referenced by index.json but must never be downloaded for stats.
const BOOKS: &[u8] = b"{\"id\":0,\"events\":[]}\n{\"id\":1,\"events\":[]}\n";
const LOOKUP_BROKEN: &[u8] = b"this is not a valid lookup table\n";

struct Ctx {
    state: AppState,
    _tmp: tempfile::TempDir,
}

async fn setup() -> Option<Ctx> {
    setup_with_cap(8_589_934_592).await
}

/// Build real state against `TEST_DATABASE_URL` with a specific blob-size cap.
async fn setup_with_cap(max_blob_bytes: u64) -> Option<Ctx> {
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
        polar: None,
        web_dir: None,
        storage_max_blob_bytes: max_blob_bytes,
        server_math_cache_bytes: 21_474_836_480,
        server_tenant_books_cap_bytes: None,
        play_domain: None,
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

    /// PUT/POST a raw (non-JSON) body; parses the JSON response.
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

/// Build a manifest `files` array from (path, bytes) pairs.
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
        .post("/api/workspaces", json!({ "name": "Math WS", "slug": &ws }))
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

fn check_url(ws: &str, game: &str) -> String {
    format!("/api/workspaces/{ws}/games/{game}/revisions/check")
}
fn blob_url(ws: &str, game: &str, hash: &str) -> String {
    format!("/api/workspaces/{ws}/games/{game}/blobs/{hash}")
}
fn revisions_url(ws: &str, game: &str) -> String {
    format!("/api/workspaces/{ws}/games/{game}/revisions")
}

/// Push a revision: check → upload each missing blob → commit. Returns the
/// commit `(status, body)`.
async fn push(
    client: &mut Client,
    ws: &str,
    game: &str,
    files: &[(&str, &[u8])],
    message: &str,
    parent: Option<i32>,
    token: &str,
) -> (StatusCode, Value) {
    let m = manifest(files);
    let (status, body) = client
        .send(
            Method::POST,
            &check_url(ws, game),
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
                    &blob_url(ws, game, &hash),
                    bytes.to_vec(),
                    Some(token),
                )
                .await;
            assert_eq!(status, StatusCode::CREATED, "PUT {path}: {up}");
            assert_eq!(s(&up["hash"]), hash);
        }
    }
    let mut body = json!({ "message": message, "files": m });
    if let Some(pn) = parent {
        body["parent_number"] = json!(pn);
    }
    client
        .send(
            Method::POST,
            &revisions_url(ws, game),
            Some(body),
            Some(token),
        )
        .await
}

async fn revision_uuid(state: &AppState, ws: &str, game: &str, number: i32) -> Uuid {
    sqlx::query_scalar(
        "SELECT r.id FROM revisions r \
         JOIN games g ON g.id = r.game_id \
         JOIN workspaces w ON w.id = g.workspace_id \
         WHERE w.slug = $1 AND g.slug = $2 AND r.number = $3",
    )
    .bind(ws)
    .bind(game)
    .bind(number)
    .fetch_one(&state.pool)
    .await
    .unwrap()
}

/// Deterministically compute a revision's stats, then poll its detail until the
/// terminal status is observed (neutralizes the endpoint's fire-and-forget task).
async fn compute_and_wait(
    client: &mut Client,
    state: &AppState,
    ws: &str,
    game: &str,
    number: i32,
    token: &str,
    expected: &str,
) -> Value {
    let rev_id = revision_uuid(state, ws, game, number).await;
    server::stats::compute_stats_for_revision(state.pool.clone(), state.store.clone(), rev_id)
        .await;

    let url = format!("{}/{number}", revisions_url(ws, game));
    for _ in 0..100 {
        let (status, body) = client.send(Method::GET, &url, None, Some(token)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if body["stats"]["status"] == json!(expected) {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("stats never reached status {expected}");
}

// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_push_flow_dedup_list_download_and_diff() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    let game = "sweet-bonanza";
    let rev1 = [
        ("index.json", INDEX_JSON),
        ("lookup_base.csv", LOOKUP_V1),
        ("books_base.jsonl", BOOKS),
    ];

    // --- rev 1: everything is missing, uploads all three, commits number 1 ---
    let m = manifest(&rev1);
    let (status, body) = owner
        .send(
            Method::POST,
            &check_url(&ws, game),
            Some(json!({ "files": m })),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["missing"].as_array().unwrap().len(), 3);

    let (status, detail) = push(&mut owner, &ws, game, &rev1, "initial", None, &token).await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
    assert_eq!(detail["number"], json!(1));
    assert_eq!(detail["files"].as_array().unwrap().len(), 3);
    assert_eq!(s(&detail["author_display_name"]), "Owner");

    // --- rev 2: one changed CSV → check returns exactly the changed blob ---
    let rev2 = [
        ("index.json", INDEX_JSON),
        ("lookup_base.csv", LOOKUP_V2),
        ("books_base.jsonl", BOOKS),
    ];
    let (status, body) = owner
        .send(
            Method::POST,
            &check_url(&ws, game),
            Some(json!({ "files": manifest(&rev2) })),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let missing = body["missing"].as_array().unwrap();
    assert_eq!(missing.len(), 1, "dedup: only the changed CSV is missing");
    assert_eq!(s(&missing[0]), sha_hex(LOOKUP_V2));

    let (status, detail) = push(&mut owner, &ws, game, &rev2, "tweak base", Some(1), &token).await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
    assert_eq!(detail["number"], json!(2));

    // --- games list: head_number + revisions_count ---
    let (status, games) = owner
        .send(
            Method::GET,
            &format!("/api/workspaces/{ws}/games"),
            None,
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let g = games["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| s(&g["slug"]) == game)
        .unwrap();
    assert_eq!(g["head_number"], json!(2));
    assert_eq!(g["revisions_count"], json!(2));

    // --- revisions list ordering (newest first) ---
    let (status, list) = owner
        .send(Method::GET, &revisions_url(&ws, game), None, Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK);
    let revs = list["revisions"].as_array().unwrap();
    assert_eq!(revs.len(), 2);
    assert_eq!(revs[0]["number"], json!(2));
    assert_eq!(revs[1]["number"], json!(1));
    assert_eq!(revs[0]["files_count"], json!(3));

    // --- file download roundtrip (bytes identical) ---
    let (status, bytes) = owner
        .get_raw(
            &format!("/api/workspaces/{ws}/games/{game}/revisions/1/files/lookup_base.csv"),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, LOOKUP_V1);

    // --- diff rev1 (before) vs rev2 (after): exactly one changed file ---
    let (status, diff) = owner
        .send(
            Method::GET,
            &format!("/api/workspaces/{ws}/games/{game}/revisions/2/diff/1"),
            None,
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{diff}");
    let files = &diff["files"];
    assert_eq!(files["unchanged"], json!(2));
    assert_eq!(files["added"].as_array().unwrap().len(), 0);
    assert_eq!(files["removed"].as_array().unwrap().len(), 0);
    let changed = files["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(s(&changed[0]["path"]), "lookup_base.csv");
    assert_eq!(s(&changed[0]["before_hash"]), sha_hex(LOOKUP_V1));
    assert_eq!(s(&changed[0]["after_hash"]), sha_hex(LOOKUP_V2));
}

#[tokio::test]
async fn stats_computed_and_surfaced_in_detail_and_diff() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    let game = "gates";
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
    push(&mut owner, &ws, game, &rev1, "r1", None, &token).await;
    push(&mut owner, &ws, game, &rev2, "r2", Some(1), &token).await;

    // rev1 stats: rtp 0.96, max_win 420, entries 4, hit_rate 0.10
    let detail1 = compute_and_wait(&mut owner, &ctx.state, &ws, game, 1, &token, "ok").await;
    let mode = &detail1["stats"]["modes"][0];
    assert_eq!(s(&mode["mode"]), "base");
    assert!((mode["rtp"].as_f64().unwrap() - 0.96).abs() < 1e-9);
    assert!((mode["max_win"].as_f64().unwrap() - 420.0).abs() < 1e-9);
    assert_eq!(mode["entries"], json!(4));
    assert!((mode["hit_rate"].as_f64().unwrap() - 0.10).abs() < 1e-9);
    assert_eq!(mode["cost"].as_f64().unwrap(), 1.0);

    // rev1 analysis (LOOKUP_V1 is the M8 micro-fixture): a single cost-1 mode
    // whose etl_40 (0.87) fails the 2-star cap but clears the 3-star cap, so the
    // revision grades 3-star without being 2-star.
    let analysis = &detail1["stats"]["analysis"];
    assert!(
        analysis.is_object(),
        "analysis should be present: {analysis}"
    );
    assert_eq!(analysis["stars"], json!(3));
    assert_eq!(analysis["two_star_compliant"], json!(false));
    assert_eq!(analysis["three_star_compliant"], json!(true));
    assert_eq!(analysis["cross_mode_rtp_pass"], json!(true));
    assert_eq!(analysis["reference_max_bet_2"], json!(200));
    assert_eq!(analysis["reference_max_bet_3"], json!(1000));

    let ma = &analysis["modes"][0];
    assert_eq!(s(&ma["mode"]), "base");
    assert_eq!(s(&ma["volatility"]), "medium");
    assert!((ma["rtp"].as_f64().unwrap() - 0.96).abs() < 1e-9);
    assert!((ma["std_dev"].as_f64().unwrap() - 198.0684_f64.sqrt()).abs() < 1e-9);
    assert!((ma["cvar"].as_f64().unwrap() - 420.0).abs() < 1e-9);
    assert!((ma["etl_40"].as_f64().unwrap() - 0.87).abs() < 1e-9);
    assert!((ma["max_win_odds"].as_f64().unwrap() - 1000.0).abs() < 1e-9);
    assert_eq!(ma["worst_zero_streak"], json!(66));
    assert_eq!(ma["worst_loss_streak"], json!(688));
    assert_eq!(ma["unique_payouts"], json!(4));

    // The cheapest mode carries the base_cost check (4 checks total).
    let checks = ma["compliance"].as_array().unwrap();
    assert_eq!(checks.len(), 4);
    let base_cost = checks
        .iter()
        .find(|c| s(&c["check"]) == "base_cost")
        .unwrap();
    assert_eq!(base_cost["pass"], json!(true));

    // etl_40 constraint: fails 2-star, passes 3-star.
    let constraints = analysis["constraints"].as_array().unwrap();
    let etl40 = constraints
        .iter()
        .find(|c| s(&c["key"]) == "etl_40")
        .unwrap();
    assert_eq!(etl40["pass2"], json!(false));
    assert_eq!(etl40["pass3"], json!(true));

    // rev2 stats: rtp 0.91 (still inside the RTP band, so its analysis is present).
    let detail2 = compute_and_wait(&mut owner, &ctx.state, &ws, game, 2, &token, "ok").await;
    assert!((detail2["stats"]["modes"][0]["rtp"].as_f64().unwrap() - 0.91).abs() < 1e-9);
    assert!(
        detail2["stats"]["analysis"].is_object(),
        "rev2 analysis should be present"
    );
    assert!(
        (detail2["stats"]["analysis"]["modes"][0]["rtp"]
            .as_f64()
            .unwrap()
            - 0.91)
            .abs()
            < 1e-9
    );

    // diff surfaces before (rev1) / after (rev2) mode stats.
    let (status, diff) = owner
        .send(
            Method::GET,
            &format!("/api/workspaces/{ws}/games/{game}/revisions/2/diff/1"),
            None,
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{diff}");
    let modes = diff["stats"]["modes"].as_array().unwrap();
    assert_eq!(modes.len(), 1);
    assert_eq!(s(&modes[0]["mode"]), "base");
    assert!((modes[0]["before"]["rtp"].as_f64().unwrap() - 0.96).abs() < 1e-9);
    assert!((modes[0]["after"]["rtp"].as_f64().unwrap() - 0.91).abs() < 1e-9);
}

#[tokio::test]
async fn broken_csv_yields_stats_error() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    let game = "broken";
    let rev = [
        ("index.json", INDEX_JSON),
        ("lookup_base.csv", LOOKUP_BROKEN),
        ("books_base.jsonl", BOOKS),
    ];
    push(&mut owner, &ws, game, &rev, "broken", None, &token).await;

    let detail = compute_and_wait(&mut owner, &ctx.state, &ws, game, 1, &token, "error").await;
    assert_eq!(s(&detail["stats"]["status"]), "error");
    assert!(detail["stats"]["error"].is_string());
    assert_eq!(detail["stats"]["modes"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn blob_put_wrong_hash_is_422_and_stores_nothing() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    let game = "hashcheck";
    let body = b"the real content".to_vec();
    let declared = sha_hex(b"a completely different thing");

    let (status, err) = owner
        .send_bytes(
            Method::PUT,
            &blob_url(&ws, game, &declared),
            body,
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(s(&err["error"]["code"]), "hash_mismatch");

    // No blobs row for the declared hash.
    let declared_bytes = hex_to_bytes(&declared);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM blobs b JOIN workspaces w ON w.id = b.workspace_id \
         WHERE w.slug = $1 AND b.hash = $2",
    )
    .bind(&ws)
    .bind(&declared_bytes)
    .fetch_one(&ctx.state.pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn blob_put_is_idempotent() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    let game = "idem";
    let body = b"some bytes".to_vec();
    let hash = sha_hex(&body);

    let (status, _) = owner
        .send_bytes(
            Method::PUT,
            &blob_url(&ws, game, &hash),
            body.clone(),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    // Second upload of the same blob → 200, body ignored.
    let (status, up) = owner
        .send_bytes(Method::PUT, &blob_url(&ws, game, &hash), body, Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK, "{up}");
    assert_eq!(s(&up["hash"]), hash);
}

#[tokio::test]
async fn oversize_blob_is_413() {
    let Some(ctx) = setup_with_cap(16).await else {
        return;
    };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    let game = "big";
    let body = vec![b'x'; 1024]; // well over the 16-byte cap
    let hash = sha_hex(&body);

    let (status, err) = owner
        .send_bytes(Method::PUT, &blob_url(&ws, game, &hash), body, Some(&token))
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{err}");
    assert_eq!(s(&err["error"]["code"]), "payload_too_large");
}

#[tokio::test]
async fn commit_with_missing_blob_is_409_missing_blobs() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    let game = "gap";
    // Manifest is valid, but no blobs were uploaded.
    let files = [("index.json", INDEX_JSON), ("lookup_base.csv", LOOKUP_V1)];
    let (status, body) = owner
        .send(
            Method::POST,
            &revisions_url(&ws, game),
            Some(json!({ "message": "nope", "files": manifest(&files) })),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(s(&body["error"]["code"]), "missing_blobs");
    let missing = body["missing"].as_array().unwrap();
    assert_eq!(missing.len(), 2);
    assert!(missing.iter().any(|h| s(h) == sha_hex(INDEX_JSON)));
}

#[tokio::test]
async fn manifest_validation_rejections() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    let game = "manifests";
    let good = sha_hex(INDEX_JSON);

    let cases: Vec<Value> = vec![
        // .. segment
        json!([
            { "path": "index.json", "hash": good, "size": 1 },
            { "path": "../escape.csv", "hash": sha_hex(b"x"), "size": 1 }
        ]),
        // backslash
        json!([
            { "path": "index.json", "hash": good, "size": 1 },
            { "path": "a\\b.csv", "hash": sha_hex(b"y"), "size": 1 }
        ]),
        // no root index.json
        json!([{ "path": "data.csv", "hash": sha_hex(b"z"), "size": 1 }]),
        // duplicate path
        json!([
            { "path": "index.json", "hash": good, "size": 1 },
            { "path": "dup.csv", "hash": sha_hex(b"a"), "size": 1 },
            { "path": "dup.csv", "hash": sha_hex(b"b"), "size": 1 }
        ]),
    ];
    for files in cases {
        let (status, body) = owner
            .send(
                Method::POST,
                &check_url(&ws, game),
                Some(json!({ "files": files })),
                Some(&token),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(s(&body["error"]["code"]), "invalid_manifest");
    }
}

#[tokio::test]
async fn stale_parent_number_is_409() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    let game = "race";
    let rev1 = [
        ("index.json", INDEX_JSON),
        ("lookup_base.csv", LOOKUP_V1),
        ("books_base.jsonl", BOOKS),
    ];
    push(&mut owner, &ws, game, &rev1, "r1", None, &token).await;

    // Head is now 1; committing with parent_number 0 is stale.
    let rev2 = [
        ("index.json", INDEX_JSON),
        ("lookup_base.csv", LOOKUP_V2),
        ("books_base.jsonl", BOOKS),
    ];
    // Upload the changed blob so the only failure is the stale parent.
    owner
        .send_bytes(
            Method::PUT,
            &blob_url(&ws, game, &sha_hex(LOOKUP_V2)),
            LOOKUP_V2.to_vec(),
            Some(&token),
        )
        .await;
    let (status, body) = owner
        .send(
            Method::POST,
            &revisions_url(&ws, game),
            Some(json!({ "message": "stale", "files": manifest(&rev2), "parent_number": 0 })),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(s(&body["error"]["code"]), "stale_parent");
}

#[tokio::test]
async fn scope_enforcement_and_membership() {
    let Some(ctx) = setup().await else {
        return;
    };
    let (mut owner, ws, token) = bootstrap(&ctx.state).await;
    let game = "scoped";
    let rev1 = [
        ("index.json", INDEX_JSON),
        ("lookup_base.csv", LOOKUP_V1),
        ("books_base.jsonl", BOOKS),
    ];
    push(&mut owner, &ws, game, &rev1, "r1", None, &token).await;

    // A member-owned PAT that lacks push:math (minted directly; the API won't
    // hand out a scopeless token). It authenticates but can't push.
    let readonly = format!("sdt_pat_{}", Uuid::new_v4());
    let owner_id: Uuid = sqlx::query_scalar("SELECT created_by FROM workspaces WHERE slug = $1")
        .bind(&ws)
        .fetch_one(&ctx.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO api_tokens (user_id, name, token_hash, scopes) VALUES ($1, $2, $3, $4)",
    )
    .bind(owner_id)
    .bind("readonly")
    .bind(server::auth::hash_secret(&readonly))
    .bind(vec!["read".to_string()])
    .execute(&ctx.state.pool)
    .await
    .unwrap();

    // 403 on every write with the scopeless PAT.
    let (status, body) = owner
        .send(
            Method::POST,
            &check_url(&ws, game),
            Some(json!({ "files": manifest(&rev1) })),
            Some(&readonly),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(s(&body["error"]["code"]), "insufficient_scope");

    let (status, _) = owner
        .send_bytes(
            Method::PUT,
            &blob_url(&ws, game, &sha_hex(b"new")),
            b"new".to_vec(),
            Some(&readonly),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = owner
        .send(
            Method::POST,
            &revisions_url(&ws, game),
            Some(json!({ "message": "x", "files": manifest(&rev1) })),
            Some(&readonly),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // But a scopeless member PAT CAN still read revisions.
    let (status, list) = owner
        .send(
            Method::GET,
            &revisions_url(&ws, game),
            None,
            Some(&readonly),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["revisions"].as_array().unwrap().len(), 1);

    // A plain member *session* can read too.
    let (mut bob, bob_user) = register(&ctx.state, &unique_email(), "Bob").await;
    let (_, invite) = owner
        .post(
            &format!("/api/workspaces/{ws}/invites"),
            json!({ "role": "member" }),
        )
        .await;
    let invite_token = s(&invite["token"]).to_string();
    bob.post(&format!("/api/invites/{invite_token}/accept"), json!({}))
        .await;
    let _ = bob_user;
    let (status, _) = bob.get(&revisions_url(&ws, game)).await;
    assert_eq!(status, StatusCode::OK);

    // A non-member gets 404 on both reads and writes (never a scope-leaking 403).
    let (mut charlie, _) = register(&ctx.state, &unique_email(), "Charlie").await;
    let (status, _) = charlie.get(&revisions_url(&ws, game)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = charlie
        .send(
            Method::POST,
            &check_url(&ws, game),
            Some(json!({ "files": manifest(&rev1) })),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

fn hex_to_bytes(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

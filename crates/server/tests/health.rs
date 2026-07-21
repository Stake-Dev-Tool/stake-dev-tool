use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use protocol::{HealthResponse, ServiceStatus};
use server::config::{Config, StorageConfig};
use server::{AppState, db, http, storage};
use tower::ServiceExt; // brings `oneshot` onto Router

fn fs_state(database_url: &str, root: PathBuf) -> AppState {
    let config = Config {
        bind_addr: "127.0.0.1:0".to_string(),
        database_url: database_url.to_string(),
        storage: StorageConfig::Fs { root },
        cookie_secure: false,
        public_url: None,
        github: None,
        polar: None,
        web_dir: None,
        storage_max_blob_bytes: 8_589_934_592,
        server_math_cache_bytes: 21_474_836_480,
        server_tenant_books_cap_bytes: None,
        play_domain: None,
    };
    let pool = db::connect_lazy(database_url).expect("lazy pool never connects");
    let store = storage::build_object_store(&config).expect("fs store");
    AppState::new(config, pool, store)
}

async fn call_healthz(state: AppState) -> (StatusCode, HealthResponse) {
    let response = http::build_router(state)
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: HealthResponse = serde_json::from_slice(&bytes).unwrap();
    (status, body)
}

/// The server must answer `/healthz` even with no database reachable. Port 1 is
/// never Postgres, so the connection is refused and the db component reports
/// down while the filesystem store stays healthy.
#[tokio::test]
async fn healthz_is_degraded_without_a_database() {
    let tmp = tempfile::tempdir().unwrap();
    let state = fs_state(
        "postgres://stakedev:stakedev@127.0.0.1:1/stakedev",
        tmp.path().to_path_buf(),
    );

    let (status, body) = call_healthz(state).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body.status, ServiceStatus::Degraded);
    assert!(!body.db.ok, "db should be down: {:?}", body.db);
    assert!(
        body.object_store.ok,
        "fs object store should be healthy: {:?}",
        body.object_store
    );
}

/// Full green path, gated on a real Postgres. Self-skips when
/// `TEST_DATABASE_URL` is unset so `cargo test` passes with no database running.
#[tokio::test]
async fn healthz_is_ok_with_a_real_database() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("TEST_DATABASE_URL unset; skipping real-database health check");
        return;
    };

    let tmp = tempfile::tempdir().unwrap();
    let state = fs_state(&database_url, tmp.path().to_path_buf());
    db::migrate(&state.pool).await.expect("migrations apply");

    let (status, body) = call_healthz(state).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.status, ServiceStatus::Ok);
    assert!(body.db.ok, "db should be up: {:?}", body.db);
    assert!(
        body.object_store.ok,
        "object store should be up: {:?}",
        body.object_store
    );
}

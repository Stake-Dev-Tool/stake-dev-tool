use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use lgs::tenant::{TenantId, TenantRegistry};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn request(router: &Router, method: &str, path: &str, body: Value) -> Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{method} {path}: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn concurrent_settings_updates_do_not_lose_presets() {
    let tmp = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(lgs::settings::SettingsStore::with_path(
        tmp.path().join("settings.json"),
    ));
    let mut tasks = Vec::new();
    for i in 0..32 {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            store
                .add_custom(format!("custom {i}"), 640, 480)
                .await
                .unwrap();
            store.load().await.unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(
        store
            .load()
            .await
            .unwrap()
            .resolutions
            .iter()
            .filter(|r| !r.builtin)
            .count(),
        32
    );
}

#[tokio::test]
async fn tenant_settings_toggle_persists_outside_math_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = TenantRegistry::new();
    let tenant = TenantId::from("commisions/huff-puff/11");
    let rounds = tmp.path().join("tenant/saved-rounds.json");
    let state = registry.get_or_create_disk_with_persistence(
        tenant,
        tmp.path().join("math"),
        &rounds,
        rounds.with_file_name("settings.json"),
    );
    let router = lgs::devtool::router(state);
    let enabled = request(
        &router,
        "POST",
        "/api/devtool/settings/toggle",
        json!({"id":"popout-s", "enabled":true}),
    )
    .await;
    assert_eq!(
        enabled["resolutions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "popout-s")
            .unwrap()["enabled"],
        true
    );
    request(
        &router,
        "POST",
        "/api/devtool/settings/toggle",
        json!({"id":"popout-s", "enabled":false}),
    )
    .await;
    let settings_file = rounds.with_file_name("settings.json");
    assert!(
        settings_file.exists(),
        "settings must use tenant storage, never the server user's home"
    );
    let persisted: Value = serde_json::from_slice(&std::fs::read(settings_file).unwrap()).unwrap();
    assert_eq!(
        persisted["resolutions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "popout-s")
            .unwrap()["enabled"],
        false
    );
    let restored = request(&router, "GET", "/api/devtool/settings", json!(null)).await;
    assert_eq!(restored, persisted);
    // A fresh registry (process restart) reads the same persisted settings.
    let restarted = TenantRegistry::new().get_or_create_disk_with_persistence(
        TenantId::from("commisions/huff-puff/11"),
        tmp.path().join("math"),
        &rounds,
        rounds.with_file_name("settings.json"),
    );
    assert_eq!(
        request(
            &lgs::devtool::router(restarted),
            "GET",
            "/api/devtool/settings",
            json!(null)
        )
        .await,
        persisted
    );
}

#[tokio::test]
async fn all_settings_routes_are_isolated_by_workspace_game_and_revision() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = TenantRegistry::new();
    let mut routers = Vec::new();
    for (ws, game, rev) in [
        ("a", "game", 11),
        ("b", "game", 11),
        ("a", "other", 11),
        ("a", "game", 12),
    ] {
        let state = registry.get_or_create_disk_with_persistence(
            TenantId::from(format!("{ws}/{game}/{rev}")),
            tmp.path().join("math"),
            tmp.path()
                .join(format!("saved-rounds/{ws}/{game}/{rev}.json")),
            tmp.path().join(format!("settings/{ws}/{game}/{rev}.json")),
        );
        routers.push(lgs::devtool::router(state));
    }
    request(
        &routers[0],
        "POST",
        "/api/devtool/settings/toggle",
        json!({"id":"popout-s", "enabled":true}),
    )
    .await;
    let added = request(
        &routers[0],
        "POST",
        "/api/devtool/settings/custom",
        json!({"label":"Test", "width":640, "height":480}),
    )
    .await;
    let id = added["resolutions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["builtin"] == false)
        .unwrap()["id"]
        .as_str()
        .unwrap();
    for router in &routers[1..] {
        let settings = request(router, "GET", "/api/devtool/settings", json!(null)).await;
        assert_eq!(
            settings,
            serde_json::to_value(lgs::settings::Settings::default()).unwrap()
        );
    }
    let deleted = request(
        &routers[0],
        "DELETE",
        &format!("/api/devtool/settings/custom/{id}"),
        json!(null),
    )
    .await;
    assert!(
        deleted["resolutions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["builtin"] == true)
    );
    assert_eq!(
        request(&routers[0], "GET", "/api/devtool/settings", json!(null)).await,
        deleted
    );
}

// A child process keeps default-store OnceLock and desktop environment changes
// out of concurrent tests and never writes to the developer's real settings.
#[test]
fn desktop_settings_compatibility() {
    if std::env::var_os("SDT_SETTINGS_TEST_CHILD").is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "desktop_settings_compatibility", "--nocapture"])
            .env("SDT_SETTINGS_TEST_CHILD", "1")
            .env("XDG_DATA_HOME", tmp.path())
            .env("HOME", tmp.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        use lgs::settings;
        use std::sync::Arc;
        let a = settings::default_store().unwrap();
        assert!(Arc::ptr_eq(&a, &settings::default_store().unwrap()));
        let engine = Arc::new(lgs::math_engine::MathEngine::with_source(
            TenantId::default(),
            Arc::new(lgs::math_engine::DiskMathSource::new(std::env::temp_dir())),
            Arc::new(lgs::math_engine::BooksCache::new()),
        ));
        let state = lgs::state::AppState::from_parts(
            Arc::new(lgs::session::SessionStore::in_memory()),
            engine,
        );
        assert!(Arc::ptr_eq(&a, &state.settings));
        let mut presets = settings::Settings::default().resolutions;
        presets.retain(|r| r.id == "desktop");
        presets[0].enabled = false;
        settings::replace_all(presets).await.unwrap();
        let healed = settings::load().await.unwrap();
        assert_eq!(
            healed.resolutions.len(),
            settings::Settings::default().resolutions.len()
        );
        assert!(
            !healed
                .resolutions
                .iter()
                .find(|r| r.id == "desktop")
                .unwrap()
                .enabled
        );
        settings::toggle("popout-s", true).await.unwrap();
        let added = settings::add_custom("Desktop custom".into(), 640, 480)
            .await
            .unwrap();
        let id = &added.resolutions.iter().find(|r| !r.builtin).unwrap().id;
        settings::delete_custom(id).await.unwrap();
        assert!(settings::delete_custom("desktop").await.is_err());
        assert!(settings::toggle("missing", true).await.is_err());
        assert!(settings::add_custom("bad".into(), 0, 480).await.is_err());
        assert!(settings::add_custom("bad".into(), 4097, 480).await.is_err());
        let path = dirs::data_local_dir()
            .unwrap()
            .join("stake-dev-tool/settings.json");
        assert!(path.exists());
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap(),
            serde_json::to_value(a.load().await.unwrap()).unwrap()
        );
    });
}

use crate::cloud;
use crate::github;
use crate::math_sync;
use crate::preview;
use crate::profiles;
use crate::state::{AppState, LgsRunning};
use crate::teams;

fn resolve_ui_dir() -> Option<PathBuf> {
    // 1) explicit env override
    if let Ok(v) = std::env::var("LGS_UI_DIR") {
        let p = PathBuf::from(v);
        if p.exists() {
            return Some(p);
        }
    }
    // 2) next to the binary (production: bundled as `ui-build/`)
    let exe_candidate = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("ui-build")))
        .filter(|p| p.exists());
    if exe_candidate.is_some() {
        return exe_candidate;
    }
    // 3) dev path: ../../ui/build relative to crate manifest
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dev = manifest.join("..").join("..").join("ui").join("build");
    if dev.exists() {
        return Some(dev.canonicalize().unwrap_or(dev));
    }
    None
}
use lgs::config::{ServerConfig, intern_currency};
use lgs::math_engine::MathEngine;
use lgs::session::{SessionInit, SessionStore};
use lgs::settings as lgs_settings;
use lgs::start_server_with_state;
use lgs::state::AppState as LgsState;
use lgs::tls;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, State};
use uuid::Uuid;

#[derive(Serialize)]
pub struct LgsStatus {
    pub running: bool,
    pub bound_addr: Option<String>,
    pub math_dir: Option<String>,
}

#[derive(Serialize)]
pub struct GameInfo {
    pub slug: String,
    pub path: String,
    pub modes: Vec<String>,
}

#[derive(Serialize)]
pub struct InspectedGame {
    pub slug: String,
    #[serde(rename = "gamePath")]
    pub game_path: String,
    #[serde(rename = "mathDir")]
    pub math_dir: String,
    pub modes: Vec<String>,
}

#[derive(Deserialize)]
pub struct LaunchOptions {
    #[serde(rename = "gameUrl")]
    pub game_url: String,
    #[serde(rename = "gameSlug")]
    pub game_slug: String,
    #[serde(default)]
    pub lang: Option<String>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub social: Option<bool>,
    #[serde(default, rename = "extraParams")]
    pub extra_params: Option<Vec<(String, String)>>,
}

#[tauri::command]
pub async fn lgs_status(state: State<'_, AppState>) -> Result<LgsStatus, String> {
    let guard = state.running.lock();
    Ok(match guard.as_ref() {
        Some(r) => LgsStatus {
            running: true,
            bound_addr: Some(r.bound_addr.to_string()),
            math_dir: Some(r.math_dir.clone()),
        },
        None => LgsStatus {
            running: false,
            bound_addr: None,
            math_dir: None,
        },
    })
}

#[tauri::command]
pub async fn start_lgs(
    port: u16,
    math_dir: String,
    state: State<'_, AppState>,
) -> Result<LgsStatus, String> {
    {
        let guard = state.running.lock();
        if guard.is_some() {
            return Err("LGS already running. Stop it first.".into());
        }
    }

    let ui_dir = resolve_ui_dir();
    let cfg = ServerConfig {
        bind_addr: format!("127.0.0.1:{port}"),
        math_dir: math_dir.clone(),
        ui_dir: ui_dir.clone(),
    };

    let engine = Arc::new(MathEngine::new(cfg));
    let sessions = Arc::new(SessionStore::new());
    let lgs_state = Arc::new(LgsState::from_parts(sessions, engine));

    let handle = start_server_with_state(lgs_state.clone(), format!("127.0.0.1:{port}"), ui_dir)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let bound_addr = handle.bound_addr;

    {
        let mut guard = state.running.lock();
        *guard = Some(LgsRunning {
            bound_addr,
            math_dir: math_dir.clone(),
            state: lgs_state,
            shutdown: handle.shutdown,
            join: handle.join,
        });
    }

    Ok(LgsStatus {
        running: true,
        bound_addr: Some(bound_addr.to_string()),
        math_dir: Some(math_dir),
    })
}

#[tauri::command]
pub async fn stop_lgs(state: State<'_, AppState>) -> Result<LgsStatus, String> {
    let running = {
        let mut guard = state.running.lock();
        guard.take()
    };

    if let Some(r) = running {
        let _ = r.shutdown.send(());
        let _ = r.join.await;
    }

    Ok(LgsStatus {
        running: false,
        bound_addr: None,
        math_dir: None,
    })
}

#[tauri::command]
pub async fn list_games(math_dir: String) -> Result<Vec<GameInfo>, String> {
    let root = PathBuf::from(&math_dir);
    if !root.exists() {
        return Err(format!("path does not exist: {}", root.display()));
    }
    if !root.is_dir() {
        return Err(format!("not a directory: {}", root.display()));
    }

    let mut games = Vec::new();
    let mut entries = tokio::fs::read_dir(&root)
        .await
        .map_err(|e| format!("{e:#}"))?;
    while let Some(entry) = entries.next_entry().await.map_err(|e| format!("{e:#}"))? {
        if !entry
            .file_type()
            .await
            .map_err(|e| format!("{e:#}"))?
            .is_dir()
        {
            continue;
        }
        let game_dir = entry.path();
        let index_path = game_dir.join("index.json");
        if !index_path.exists() {
            continue;
        }
        let slug = match entry.file_name().to_str() {
            Some(s) => s.to_string(),
            None => continue,
        };
        let modes = read_modes(&index_path).await.unwrap_or_default();
        games.push(GameInfo {
            slug,
            path: game_dir.to_string_lossy().into_owned(),
            modes,
        });
    }
    games.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(games)
}

#[tauri::command]
pub async fn inspect_game_folder(path: String) -> Result<InspectedGame, String> {
    let picked = PathBuf::from(&path);
    if !picked.exists() {
        return Err(format!("path does not exist: {}", picked.display()));
    }
    if !picked.is_dir() {
        return Err(format!("not a directory: {}", picked.display()));
    }

    let index_path = picked.join("index.json");
    if !index_path.exists() {
        return Err(format!("no index.json found in {}", picked.display()));
    }

    let slug = picked
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "unable to read folder name".to_string())?
        .to_string();
    let math_dir = picked
        .parent()
        .ok_or_else(|| "game folder has no parent".to_string())?
        .to_string_lossy()
        .into_owned();
    let modes = read_modes(&index_path).await.unwrap_or_default();

    Ok(InspectedGame {
        slug,
        game_path: picked.to_string_lossy().into_owned(),
        math_dir,
        modes,
    })
}

async fn read_modes(index_path: &Path) -> Result<Vec<String>, String> {
    let bytes = tokio::fs::read(index_path)
        .await
        .map_err(|e| format!("{e:#}"))?;
    #[derive(Deserialize)]
    struct ModeRef {
        name: String,
    }
    #[derive(Deserialize)]
    struct IndexFile {
        modes: Vec<ModeRef>,
    }
    let parsed: IndexFile = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    Ok(parsed.modes.into_iter().map(|m| m.name).collect())
}

#[tauri::command]
pub async fn launch_game(
    app: AppHandle,
    options: LaunchOptions,
    state: State<'_, AppState>,
) -> Result<String, String> {
    use tauri_plugin_opener::OpenerExt;

    let bound = {
        let guard = state.running.lock();
        guard
            .as_ref()
            .map(|r| r.bound_addr.to_string())
            .ok_or_else(|| "LGS is not running. Start it first.".to_string())?
    };

    let session_id = Uuid::new_v4().to_string();
    let port = bound.rsplit(':').next().unwrap_or("3001");
    let rgs_url = format!("localhost:{port}/api/rgs/{}", options.game_slug);

    let mut url =
        url::Url::parse(&options.game_url).map_err(|e| format!("invalid gameUrl: {e}"))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("sessionID", &session_id);
        q.append_pair("rgs_url", &rgs_url);
        q.append_pair("lang", options.lang.as_deref().unwrap_or("en"));
        q.append_pair("currency", options.currency.as_deref().unwrap_or("USD"));
        q.append_pair("device", options.device.as_deref().unwrap_or("desktop"));
        q.append_pair(
            "social",
            if options.social.unwrap_or(false) {
                "true"
            } else {
                "false"
            },
        );
        if let Some(extras) = &options.extra_params {
            for (k, v) in extras {
                q.append_pair(k, v);
            }
        }
    }
    let final_url = url.to_string();

    app.opener()
        .open_url(&final_url, None::<&str>)
        .map_err(|e| format!("failed to open browser: {e}"))?;

    Ok(final_url)
}

#[derive(Deserialize)]
pub struct PrepareSession {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "gameSlug")]
    pub game_slug: String,
    #[serde(default)]
    pub balance: Option<u64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
}

#[tauri::command]
pub async fn prepare_session(
    payload: PrepareSession,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let lgs_state = {
        let guard = state.running.lock();
        guard
            .as_ref()
            .map(|r| r.state.clone())
            .ok_or_else(|| "LGS is not running.".to_string())?
    };
    let init = SessionInit {
        game: payload.game_slug,
        language: payload.language,
        balance: payload.balance,
        currency: payload.currency.as_deref().map(intern_currency),
    };
    lgs_state.sessions.prepare(&payload.session_id, init);
    Ok(())
}

#[derive(Serialize)]
pub struct CaStatus {
    pub installed: bool,
    #[serde(rename = "caPath")]
    pub ca_path: String,
}

#[tauri::command]
pub async fn ca_status() -> Result<CaStatus, String> {
    let ca = tls::LocalCa::load_or_create()
        .await
        .map_err(|e| format!("{e:#}"))?;
    let installed = tls::is_ca_installed_user_store();
    Ok(CaStatus {
        installed,
        ca_path: ca.ca_cert_path().to_string_lossy().into_owned(),
    })
}

#[tauri::command]
pub async fn install_ca() -> Result<CaStatus, String> {
    let ca = tls::LocalCa::load_or_create()
        .await
        .map_err(|e| format!("{e:#}"))?;
    tls::install_ca_user_store(&ca.ca_cert_path()).map_err(|e| e.to_string())?;
    Ok(CaStatus {
        installed: tls::is_ca_installed_user_store(),
        ca_path: ca.ca_cert_path().to_string_lossy().into_owned(),
    })
}

#[tauri::command]
pub async fn uninstall_ca() -> Result<CaStatus, String> {
    tls::uninstall_ca_user_store().map_err(|e| e.to_string())?;
    let ca = tls::LocalCa::load_or_create()
        .await
        .map_err(|e| format!("{e:#}"))?;
    Ok(CaStatus {
        installed: tls::is_ca_installed_user_store(),
        ca_path: ca.ca_cert_path().to_string_lossy().into_owned(),
    })
}

// ============================================================
// Settings (resolutions)
// ============================================================

#[tauri::command]
pub async fn get_settings() -> Result<lgs_settings::Settings, String> {
    lgs_settings::load().await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn toggle_resolution(
    id: String,
    enabled: bool,
) -> Result<lgs_settings::Settings, String> {
    lgs_settings::toggle(&id, enabled)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn add_custom_resolution(
    label: String,
    width: u32,
    height: u32,
) -> Result<lgs_settings::Settings, String> {
    lgs_settings::add_custom(label, width, height)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn delete_custom_resolution(id: String) -> Result<lgs_settings::Settings, String> {
    lgs_settings::delete_custom(&id)
        .await
        .map_err(|e| format!("{e:#}"))
}

// ============================================================
// Profiles
// ============================================================

#[tauri::command]
pub async fn list_profiles() -> Result<Vec<profiles::Profile>, String> {
    profiles::list().await.map_err(|e| format!("{e:#}"))
}

#[derive(Deserialize)]
pub struct SaveProfileBody {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(rename = "gamePath")]
    pub game_path: String,
    #[serde(rename = "gameUrl")]
    pub game_url: String,
    #[serde(rename = "gameSlug")]
    pub game_slug: String,
    #[serde(default)]
    pub resolutions: Vec<lgs_settings::ResolutionPreset>,
}

#[tauri::command]
pub async fn save_profile(payload: SaveProfileBody) -> Result<profiles::Profile, String> {
    profiles::upsert(
        payload.id,
        payload.name,
        payload.game_path,
        payload.game_url,
        payload.game_slug,
        payload.resolutions,
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn replace_resolutions(
    resolutions: Vec<lgs_settings::ResolutionPreset>,
) -> Result<lgs_settings::Settings, String> {
    lgs_settings::replace_all(resolutions)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn delete_profile(id: String) -> Result<(), String> {
    profiles::delete(&id).await.map_err(|e| format!("{e:#}"))
}

/// Find a Chromium-based browser executable on the system.
/// Returns the path if found, in priority order: Chrome, Edge, Brave.
fn find_chromium_browser() -> Option<PathBuf> {
    let candidates: &[&str] = if cfg!(windows) {
        &[
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
        ]
    } else if cfg!(target_os = "macos") {
        &[
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        ]
    } else {
        &[
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
            "/usr/bin/microsoft-edge",
        ]
    };

    for c in candidates {
        let p = PathBuf::from(c);
        if p.exists() {
            return Some(p);
        }
    }

    // Fallback: try PATH
    for name in &["chrome", "google-chrome", "chromium", "msedge", "brave"] {
        let Ok(out) = std::process::Command::new("where").arg(name).output() else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let s = String::from_utf8_lossy(&out.stdout);
        if let Some(line) = s.lines().next() {
            let p = PathBuf::from(line.trim());
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

#[derive(Serialize)]
pub struct OpenBrowserResult {
    pub method: String,
    pub url: String,
}

/// Launch the test view in a dedicated Chromium instance with raised WebGL
/// context limit. Falls back to the default browser if no Chromium is found.
#[tauri::command]
pub async fn open_test_browser(url: String) -> Result<OpenBrowserResult, String> {
    if let Some(browser) = find_chromium_browser() {
        let profile_dir = dirs::data_local_dir()
            .ok_or_else(|| "no local data dir".to_string())?
            .join("stake-dev-tool")
            .join("browser-profile");
        std::fs::create_dir_all(&profile_dir).map_err(|e| e.to_string())?;

        let result = std::process::Command::new(&browser)
            .arg(format!("--user-data-dir={}", profile_dir.display()))
            .arg("--max-active-webgl-contexts=64")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-features=Translate,OptimizationHints")
            .arg("--new-window")
            .arg(&url)
            .spawn();

        match result {
            Ok(_) => Ok(OpenBrowserResult {
                method: format!("chromium ({})", browser.display()),
                url,
            }),
            Err(e) => Err(format!("failed to spawn browser: {e}")),
        }
    } else {
        // No Chromium found — UI will catch the error and fall back to
        // its own openUrl call (default system browser).
        Err("no Chromium-based browser found on the system".to_string())
    }
}

// ============================================================
// GitHub auth
// ============================================================

#[tauri::command]
pub async fn github_current_user() -> Result<Option<github::GithubUser>, String> {
    github::auth::current_user()
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn github_start_device_flow() -> Result<github::DeviceCode, String> {
    github::auth::request_device_code()
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn github_poll_device_flow(
    device_code: String,
    current_interval: u64,
) -> Result<github::auth::DeviceFlowPoll, String> {
    github::auth::poll_for_token(&device_code, current_interval)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn github_logout() -> Result<(), String> {
    github::auth::clear_token().map_err(|e| e.to_string())
}

// ============================================================
// Teams → cloud workspaces (M3)
//
// The command NAMES are the UI contract and are preserved; the bodies now
// delegate to `crate::cloud` (workspaces + document sync + M2 math) instead of
// the legacy GitHub-repo system in `crate::teams` (kept only for migration).
// ============================================================

#[tauri::command]
pub async fn teams_list() -> Result<Vec<cloud::workspaces::Workspace>, String> {
    cloud::workspaces::list()
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn teams_active() -> Result<Option<cloud::workspaces::Workspace>, String> {
    cloud::workspaces::active()
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn teams_set_active(team_id: Option<String>) -> Result<(), String> {
    cloud::workspaces::set_active(team_id.as_deref())
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn teams_create(
    name: String,
    slug: Option<String>,
) -> Result<cloud::workspaces::Workspace, String> {
    cloud::workspaces::create(&name, slug.as_deref())
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn github_list_orgs() -> Result<Vec<github::api::OrgInfo>, String> {
    let client = github::api::GithubClient::from_stored_token().map_err(|e| format!("{e:#}"))?;
    client.list_user_orgs().await.map_err(|e| format!("{e:#}"))
}

/// Join a workspace by accepting an invite token (from an invite URL).
#[tauri::command]
pub async fn teams_join(token: String) -> Result<cloud::workspaces::Workspace, String> {
    cloud::workspaces::join(&token)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn teams_leave(team_id: String) -> Result<(), String> {
    cloud::workspaces::leave(&team_id)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn teams_delete(team_id: String) -> Result<(), String> {
    cloud::workspaces::delete(&team_id)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Create an invite for a workspace, returning its shareable URL (copy once).
#[tauri::command]
pub async fn teams_invite(team_id: String, role: Option<String>) -> Result<String, String> {
    let role = match role.as_deref() {
        Some("admin") => protocol::Role::Admin,
        Some("owner") => protocol::Role::Owner,
        _ => protocol::Role::Member,
    };
    cloud::workspaces::invite(&team_id, role)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn teams_sync(team_id: String) -> Result<cloud::sync::SyncReport, String> {
    cloud::sync::sync(&team_id)
        .await
        .map_err(|e| format!("{e:#}"))
}

// ---- Legacy GitHub teams (migration only) ----

/// Lists the legacy GitHub-repo teams still on this device, so the Teams screen
/// can offer a per-team "Migrate to cloud" action.
#[tauri::command]
pub async fn teams_legacy_list() -> Result<Vec<teams::Team>, String> {
    teams::list_local().await.map_err(|e| format!("{e:#}"))
}

/// Migrate a legacy GitHub team into a new cloud workspace (requires both
/// GitHub and cloud sign-in).
#[tauri::command]
pub async fn teams_migrate_to_cloud(
    app: AppHandle,
    team_id: String,
) -> Result<cloud::migrate::MigrateReport, String> {
    cloud::migrate::migrate_to_cloud(&app, &team_id)
        .await
        .map_err(|e| format!("{e:#}"))
}

// ============================================================
// Math file sync (per-team, per-game)
// ============================================================

#[tauri::command]
pub async fn teams_push_math(
    app: AppHandle,
    team_id: String,
    game_slug: String,
    game_path: String,
) -> Result<math_sync::MathSyncReport, String> {
    cloud::sync::push_math(&app, &team_id, &game_slug, game_path)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn teams_pull_math(
    app: AppHandle,
    team_id: String,
    game_slug: String,
    dest_path: String,
) -> Result<math_sync::MathSyncReport, String> {
    cloud::sync::pull_math(&app, &team_id, &game_slug, dest_path)
        .await
        .map_err(|e| format!("{e:#}"))
}

// ============================================================
// Preview publishing (browser-side WASM RGS)
// ============================================================

#[tauri::command]
pub async fn preview_publish(
    app: AppHandle,
    profile_id: String,
    front_path: String,
    math_mode: preview::MathMode,
) -> Result<preview::PublishReport, String> {
    preview::publish_with_progress(&app, &profile_id, front_path, math_mode)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn preview_unpublish(profile_id: String) -> Result<(), String> {
    preview::unpublish(&profile_id)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn preview_build_local(
    profile_id: String,
    front_path: String,
    math_mode: preview::MathMode,
) -> Result<String, String> {
    preview::build_local(&profile_id, front_path, math_mode)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn teams_list_remote_games(team_id: String) -> Result<Vec<String>, String> {
    cloud::sync::list_remote_games(&team_id)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn teams_remove_from_catalog(team_id: String, profile_id: String) -> Result<(), String> {
    cloud::sync::remove_from_catalog(&team_id, &profile_id)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn teams_push_profile(team_id: String, profile_id: String) -> Result<(), String> {
    cloud::sync::push_profile(&team_id, &profile_id)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn teams_all_catalogs() -> Result<Vec<cloud::sync::CatalogEntry>, String> {
    cloud::sync::all_catalogs()
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn teams_list_profiles(
    team_id: String,
) -> Result<Vec<cloud::sync::TeamProfileInfo>, String> {
    cloud::sync::list_profiles(&team_id)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn teams_pull_profile(
    app: AppHandle,
    team_id: String,
    team_profile_id: String,
) -> Result<profiles::Profile, String> {
    cloud::sync::pull_profile(&app, &team_id, &team_profile_id)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn teams_default_math_root(team_id: String) -> Result<String, String> {
    let slug = cloud::workspaces::resolve_slug(&team_id)
        .await
        .map_err(|e| format!("{e:#}"))?;
    cloud::sync::default_math_root(&slug)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| format!("{e:#}"))
}

// ============================================================
// Cloud (V2 platform) — device-flow sign-in, workspaces, live SSE.
//
// The `teams_*` commands above delegate to this cloud surface. These `cloud_*`
// commands cover sign-in, the base-URL config, and the workspace event stream.
// ============================================================

#[tauri::command]
pub async fn cloud_get_base_url() -> Result<String, String> {
    cloud::config::get().map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn cloud_set_base_url(url: String) -> Result<String, String> {
    cloud::config::set(&url).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn cloud_request_device_code() -> Result<protocol::DeviceCodeResponse, String> {
    cloud::auth::request_device_code()
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn cloud_poll_for_token(
    device_code: String,
    current_interval: u64,
) -> Result<cloud::auth::CloudDeviceFlowPoll, String> {
    cloud::auth::poll_for_token(&device_code, current_interval)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn cloud_current_user() -> Result<Option<protocol::User>, String> {
    cloud::auth::current_user()
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn cloud_sign_out() -> Result<(), String> {
    cloud::auth::clear_token().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cloud_list_workspaces() -> Result<protocol::WorkspacesResponse, String> {
    let client = cloud::api::CloudClient::from_stored_token().map_err(|e| e.to_string())?;
    client.list_workspaces().await.map_err(|e| e.to_string())
}

/// Start (or restart) the workspace SSE subscription for `workspace_id`. Live
/// events arrive on the `cloud-workspace-event` Tauri event; any prior stream is
/// aborted first so only the active workspace is subscribed.
#[tauri::command]
pub async fn cloud_subscribe(
    app: AppHandle,
    workspace_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let slug = cloud::workspaces::resolve_slug(&workspace_id)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let previous = {
        let mut guard = state.cloud_sse.lock();
        guard.replace(cloud::sse::spawn(app, slug))
    };
    if let Some(handle) = previous {
        handle.abort();
    }
    Ok(())
}

/// Stop the workspace SSE subscription, if any.
#[tauri::command]
pub async fn cloud_unsubscribe(state: State<'_, AppState>) -> Result<(), String> {
    let previous = state.cloud_sse.lock().take();
    if let Some(handle) = previous {
        handle.abort();
    }
    Ok(())
}

#[tauri::command]
pub async fn build_launch_url(
    options: LaunchOptions,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let bound = {
        let guard = state.running.lock();
        guard
            .as_ref()
            .map(|r| r.bound_addr.to_string())
            .ok_or_else(|| "LGS is not running.".to_string())?
    };

    let session_id = Uuid::new_v4().to_string();
    let port = bound.rsplit(':').next().unwrap_or("3001");
    let rgs_url = format!("localhost:{port}/api/rgs/{}", options.game_slug);
    let mut url =
        url::Url::parse(&options.game_url).map_err(|e| format!("invalid gameUrl: {e}"))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("sessionID", &session_id);
        q.append_pair("rgs_url", &rgs_url);
        q.append_pair("lang", options.lang.as_deref().unwrap_or("en"));
        q.append_pair("currency", options.currency.as_deref().unwrap_or("USD"));
        q.append_pair("device", options.device.as_deref().unwrap_or("desktop"));
        q.append_pair(
            "social",
            if options.social.unwrap_or(false) {
                "true"
            } else {
                "false"
            },
        );
        if let Some(extras) = &options.extra_params {
            for (k, v) in extras {
                q.append_pair(k, v);
            }
        }
    }
    Ok(url.to_string())
}

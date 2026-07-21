mod cloud;
mod commands;
mod file_chunker;
mod github;
mod math_sync;
mod preview;
mod profiles;
mod state;
mod teams;

use crate::state::AppState;
use tracing_subscriber::{EnvFilter, fmt};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=warn")),
        )
        .try_init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::lgs_status,
            commands::start_lgs,
            commands::stop_lgs,
            commands::list_games,
            commands::inspect_game_folder,
            commands::launch_game,
            commands::build_launch_url,
            commands::ca_status,
            commands::install_ca,
            commands::uninstall_ca,
            commands::prepare_session,
            commands::open_test_browser,
            commands::list_profiles,
            commands::save_profile,
            commands::delete_profile,
            commands::get_settings,
            commands::toggle_resolution,
            commands::add_custom_resolution,
            commands::delete_custom_resolution,
            commands::replace_resolutions,
            commands::github_current_user,
            commands::github_start_device_flow,
            commands::github_poll_device_flow,
            commands::github_logout,
            commands::github_list_orgs,
            commands::teams_list,
            commands::teams_active,
            commands::teams_set_active,
            commands::teams_create,
            commands::teams_join,
            commands::teams_leave,
            commands::teams_delete,
            commands::teams_invite,
            commands::teams_discover,
            commands::teams_sync,
            commands::teams_push_math,
            commands::teams_pull_math,
            commands::teams_list_remote_games,
            commands::teams_default_math_root,
            commands::teams_list_profiles,
            commands::teams_pull_profile,
            commands::teams_push_profile,
            commands::teams_all_catalogs,
            commands::teams_remove_from_catalog,
            commands::preview_publish,
            commands::preview_unpublish,
            commands::preview_build_local,
            commands::cloud_get_base_url,
            commands::cloud_set_base_url,
            commands::cloud_request_device_code,
            commands::cloud_poll_for_token,
            commands::cloud_current_user,
            commands::cloud_sign_out,
            commands::cloud_list_workspaces,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

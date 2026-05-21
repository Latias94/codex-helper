mod commands;
mod error;
mod lifecycle;

pub fn run() {
    tauri::Builder::default()
        .manage(lifecycle::DesktopLifecycleState::default())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            lifecycle::setup_tray(app)?;
            lifecycle::setup_main_window_lifecycle(app)?;
            Ok(())
        })
        .on_window_event(lifecycle::handle_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::admin_api::get_admin_read_model,
            commands::app::get_app_metadata,
            commands::app::hide_main_window,
            commands::app::minimize_main_window,
            commands::app::quit_app,
            commands::app::show_main_window,
            commands::app::toggle_main_window_maximized,
            commands::control::apply_provider_runtime_override,
            commands::control::apply_session_overrides,
            commands::control::attach_existing_proxy,
            commands::control::get_desktop_control_state,
            commands::control::probe_station,
            commands::control::refresh_provider_balances,
            commands::control::reload_runtime,
            commands::control::reset_session_overrides,
            commands::control::set_global_route_override,
            commands::control::start_desktop_proxy,
            commands::control::stop_proxy,
            commands::control::switch_codex,
            commands::paths::get_known_paths,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run codex-helper desktop client");
}

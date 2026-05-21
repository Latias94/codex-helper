mod commands;
mod error;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::app::get_app_metadata,
            commands::paths::get_known_paths,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run codex-helper desktop client");
}

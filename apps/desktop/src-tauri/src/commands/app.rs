use serde::Serialize;
use tauri::AppHandle;

use crate::error::CommandError;
use crate::lifecycle;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppMetadata {
    pub name: &'static str,
    pub version: &'static str,
    pub tauri: &'static str,
}

#[tauri::command]
pub fn get_app_metadata() -> AppMetadata {
    AppMetadata {
        name: "codex-helper",
        version: env!("CARGO_PKG_VERSION"),
        tauri: "2",
    }
}

#[tauri::command]
pub fn show_main_window(app: AppHandle) -> Result<(), CommandError> {
    lifecycle::show_main_window(&app)
}

#[tauri::command]
pub fn hide_main_window(app: AppHandle) -> Result<(), CommandError> {
    lifecycle::hide_main_window(&app)
}

#[tauri::command]
pub fn minimize_main_window(app: AppHandle) -> Result<(), CommandError> {
    lifecycle::minimize_main_window(&app)
}

#[tauri::command]
pub fn toggle_main_window_maximized(app: AppHandle) -> Result<(), CommandError> {
    lifecycle::toggle_main_window_maximized(&app)
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    lifecycle::quit_app(&app);
}

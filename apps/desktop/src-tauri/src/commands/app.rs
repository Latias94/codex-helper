use serde::Serialize;

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

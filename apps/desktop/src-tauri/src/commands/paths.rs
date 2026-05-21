use std::path::PathBuf;

use codex_helper_core::config::proxy_home_dir;
use serde::Serialize;

use crate::error::{CommandError, DesktopError};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownPaths {
    pub home: PathBuf,
    pub config: PathBuf,
    pub logs: PathBuf,
    pub cache: PathBuf,
}

#[tauri::command]
pub fn get_known_paths() -> Result<KnownPaths, CommandError> {
    let home = proxy_home_dir();
    if home.as_os_str().is_empty() {
        return Err(DesktopError::Path("empty codex-helper home".to_string()).into());
    }

    Ok(KnownPaths {
        config: home.join("config.toml"),
        logs: home.join("logs"),
        cache: home.join("cache"),
        home,
    })
}

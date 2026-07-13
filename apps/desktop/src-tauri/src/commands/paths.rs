use std::fs;
use std::path::{Path, PathBuf};

use codex_helper_core::config::{config_file_path, proxy_home_dir};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

use crate::error::{CommandError, DesktopError};

const CONFIG_FILE_NAME: &str = "config.toml";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownPaths {
    pub home: PathBuf,
    pub config: PathBuf,
    pub logs: PathBuf,
    pub cache: PathBuf,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KnownPathKind {
    Home,
    Config,
    Logs,
    Cache,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenKnownPathPayload {
    pub kind: KnownPathKind,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportConfigPayload {
    pub destination: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFileActionResult {
    pub ok: bool,
    pub action: &'static str,
    pub message: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub secret_warning: bool,
}

#[tauri::command]
pub fn get_known_paths() -> Result<KnownPaths, CommandError> {
    known_paths()
}

#[tauri::command]
pub fn open_known_path(app: AppHandle, payload: OpenKnownPathPayload) -> Result<(), CommandError> {
    let target = known_path(payload.kind)?;
    ensure_openable_path(payload.kind, &target)?;
    if matches!(payload.kind, KnownPathKind::Config) {
        if target.exists() {
            return app
                .opener()
                .reveal_item_in_dir(&target)
                .map_err(|err| DesktopError::Path(format!("reveal {:?}: {err}", target)).into());
        }
        let parent = target.parent().unwrap_or_else(|| Path::new("."));
        return open_path(&app, parent);
    }
    open_path(&app, &target)
}

fn open_path(app: &AppHandle, target: &Path) -> Result<(), CommandError> {
    app.opener()
        .open_path(target.to_string_lossy().to_string(), None::<String>)
        .map_err(|err| DesktopError::Path(format!("open {:?}: {err}", target)).into())
}

#[tauri::command]
pub fn export_config(payload: ExportConfigPayload) -> Result<ConfigFileActionResult, CommandError> {
    export_config_file(config_path(), payload.destination)
}

fn known_paths() -> Result<KnownPaths, CommandError> {
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

fn known_path(kind: KnownPathKind) -> Result<PathBuf, CommandError> {
    let paths = known_paths()?;
    Ok(match kind {
        KnownPathKind::Home => paths.home,
        KnownPathKind::Config => paths.config,
        KnownPathKind::Logs => paths.logs,
        KnownPathKind::Cache => paths.cache,
    })
}

fn config_path() -> PathBuf {
    let path = config_file_path();
    if path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
        path
    } else {
        proxy_home_dir().join(CONFIG_FILE_NAME)
    }
}

fn ensure_openable_path(kind: KnownPathKind, path: &Path) -> Result<(), CommandError> {
    match kind {
        KnownPathKind::Config => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|err| {
                    DesktopError::Path(format!("create config directory {:?}: {err}", parent))
                })?;
            }
        }
        KnownPathKind::Home | KnownPathKind::Logs | KnownPathKind::Cache => {
            fs::create_dir_all(path)
                .map_err(|err| DesktopError::Path(format!("create directory {:?}: {err}", path)))?;
        }
    }
    Ok(())
}

fn export_config_file(
    source: PathBuf,
    destination: PathBuf,
) -> Result<ConfigFileActionResult, CommandError> {
    validate_export_destination(&destination)?;
    if !source.exists() {
        return Err(
            DesktopError::Config(format!("config file does not exist at {:?}", source)).into(),
        );
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            DesktopError::Config(format!("create export directory {:?}: {err}", parent))
        })?;
    }
    fs::copy(&source, &destination).map_err(|err| {
        DesktopError::Config(format!(
            "export config from {:?} to {:?}: {err}",
            source, destination
        ))
    })?;
    Ok(ConfigFileActionResult {
        ok: true,
        action: "export-config",
        message:
            "已导出当前 codex-helper config.toml；如果文件中包含 inline token，请按密钥文件保管。"
                .to_string(),
        source,
        destination,
        secret_warning: true,
    })
}

fn validate_export_destination(path: &Path) -> Result<(), CommandError> {
    if path.as_os_str().is_empty() {
        return Err(DesktopError::Config("export destination is empty".to_string()).into());
    }
    if path.is_dir() {
        return Err(DesktopError::Config(format!(
            "export destination must be a file path, got directory {:?}",
            path
        ))
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::export_config_file;

    const VALID_CONFIG: &str = r#"
version = 5

[codex.routing]
entry = "relay"

[codex.routing.routes.relay]
strategy = "ordered-failover"
children = ["relay"]

[codex.providers.relay]
base_url = "https://relay.example/v1"
auth_token_env = "RELAY_API_KEY"
"#;

    #[test]
    fn export_config_copies_current_single_config_file_with_secret_warning() {
        let dir = unique_temp_dir("export-config");
        let source = dir.join("config.toml");
        let destination = dir.join("backup").join("config-export.toml");
        fs::write(&source, VALID_CONFIG).expect("write source config");

        let result =
            export_config_file(source.clone(), destination.clone()).expect("export config");

        assert!(result.ok);
        assert!(result.secret_warning);
        assert_eq!(result.source, source);
        assert_eq!(result.destination, destination);
        assert_eq!(
            fs::read_to_string(result.destination).expect("read export"),
            VALID_CONFIG
        );
    }

    fn unique_temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "codex-helper-desktop-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }
}

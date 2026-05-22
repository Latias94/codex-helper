use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use codex_helper_core::config::{
    ProxyConfig, ProxyConfigV2, ProxyConfigV4, config_file_path, proxy_home_dir,
};
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportConfigPayload {
    pub source: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFileActionResult {
    pub ok: bool,
    pub action: &'static str,
    pub message: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
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

#[tauri::command]
pub fn import_config(payload: ImportConfigPayload) -> Result<ConfigFileActionResult, CommandError> {
    import_config_file(payload.source, config_path())
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
        backup: None,
        secret_warning: true,
    })
}

fn import_config_file(
    source: PathBuf,
    destination: PathBuf,
) -> Result<ConfigFileActionResult, CommandError> {
    validate_import_source(&source)?;
    let text = fs::read_to_string(&source)
        .map_err(|err| DesktopError::Config(format!("read import file {:?}: {err}", source)))?;
    validate_config_toml(&text)?;

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            DesktopError::Config(format!("create config directory {:?}: {err}", parent))
        })?;
    }

    let backup = if destination.exists() {
        let backup = backup_path_for(&destination);
        fs::copy(&destination, &backup).map_err(|err| {
            DesktopError::Config(format!(
                "backup current config {:?} to {:?}: {err}",
                destination, backup
            ))
        })?;
        Some(backup)
    } else {
        None
    };

    fs::write(&destination, text).map_err(|err| {
        DesktopError::Config(format!("write imported config to {:?}: {err}", destination))
    })?;

    Ok(ConfigFileActionResult {
        ok: true,
        action: "import-config",
        message: "已导入 config.toml；如本地代理正在运行，请重新加载运行时配置。".to_string(),
        source,
        destination,
        backup,
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

fn validate_import_source(path: &Path) -> Result<(), CommandError> {
    if path.as_os_str().is_empty() {
        return Err(DesktopError::Config("import source is empty".to_string()).into());
    }
    if !path.exists() {
        return Err(
            DesktopError::Config(format!("import source does not exist at {:?}", path)).into(),
        );
    }
    if !path.is_file() {
        return Err(
            DesktopError::Config(format!("import source must be a file, got {:?}", path)).into(),
        );
    }
    if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
        return Err(DesktopError::Config("import source must be a .toml file".to_string()).into());
    }
    Ok(())
}

fn validate_config_toml(text: &str) -> Result<(), CommandError> {
    let value = toml::from_str::<toml::Value>(text).map_err(|err| {
        DesktopError::Config(format!("imported config.toml is invalid TOML: {err}"))
    })?;
    let version = value
        .get("version")
        .and_then(toml::Value::as_integer)
        .and_then(|version| u32::try_from(version).ok());
    let valid = if version == Some(5) {
        toml::from_str::<ProxyConfigV4>(text).map(|_| ())
    } else if version == Some(2) {
        toml::from_str::<ProxyConfigV2>(text).map(|_| ())
    } else {
        toml::from_str::<ProxyConfig>(text).map(|_| ())
    };
    valid.map_err(|err| {
        DesktopError::Config(format!(
            "imported config.toml is not a supported codex-helper config: {err}"
        ))
        .into()
    })
}

fn backup_path_for(path: &Path) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(CONFIG_FILE_NAME);
    path.with_file_name(format!("{file_name}.{timestamp}.bak"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{export_config_file, import_config_file};

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

    #[test]
    fn import_config_validates_toml_and_creates_timestamped_backup() {
        let dir = unique_temp_dir("import-config");
        let source = dir.join("incoming.toml");
        let destination = dir.join("config.toml");
        fs::write(&source, VALID_CONFIG).expect("write import source");
        fs::write(&destination, "version = 5\n").expect("write existing config");

        let result =
            import_config_file(source.clone(), destination.clone()).expect("import config");

        assert!(result.ok);
        assert!(result.secret_warning);
        let backup = result.backup.expect("backup path");
        assert!(backup.exists());
        assert_eq!(
            fs::read_to_string(backup).expect("read backup"),
            "version = 5\n"
        );
        assert_eq!(
            fs::read_to_string(destination).expect("read destination"),
            VALID_CONFIG
        );
    }

    #[test]
    fn import_config_rejects_invalid_toml_without_overwriting_current_config() {
        let dir = unique_temp_dir("import-invalid-config");
        let source = dir.join("incoming.toml");
        let destination = dir.join("config.toml");
        fs::write(&source, "version = 5\n[codex.providers.bad\n").expect("write invalid source");
        fs::write(&destination, VALID_CONFIG).expect("write existing config");

        let error =
            import_config_file(source, destination.clone()).expect_err("invalid import fails");

        assert!(
            error.message.contains("invalid"),
            "unexpected error: {}",
            error.message
        );
        assert_eq!(
            fs::read_to_string(destination).expect("read destination"),
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

    #[allow(dead_code)]
    fn assert_path_exists(path: &Path) {
        assert!(path.exists(), "{path:?} should exist");
    }
}

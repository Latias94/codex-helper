use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use dirs::home_dir;

pub const CLAUDE_ABSENT_BACKUP_SENTINEL: &str = "{\"__codex_helper_backup_absent\":true}";

fn resolve_home_dir(env_var: &str, default_dir_name: &str) -> PathBuf {
    resolve_home_dir_from(env::var_os(env_var), home_dir(), default_dir_name)
}

fn resolve_home_dir_from(
    configured: Option<OsString>,
    fallback_home: Option<PathBuf>,
    default_dir_name: &str,
) -> PathBuf {
    if let Some(dir) = configured
        .filter(|dir| !dir.is_empty() && dir.to_str().is_none_or(|text| !text.trim().is_empty()))
    {
        return PathBuf::from(dir);
    }
    fallback_home
        .unwrap_or_else(|| PathBuf::from("."))
        .join(default_dir_name)
}

pub fn codex_home() -> PathBuf {
    resolve_home_dir("CODEX_HOME", ".codex")
}

pub fn codex_config_path() -> PathBuf {
    codex_home().join("config.toml")
}

pub fn claude_home() -> PathBuf {
    resolve_home_dir("CLAUDE_HOME", ".claude")
}

pub fn claude_settings_path() -> PathBuf {
    let dir = claude_home();
    let settings = dir.join("settings.json");
    if settings.exists() {
        return settings;
    }
    let legacy = dir.join("claude.json");
    if legacy.exists() {
        return legacy;
    }
    settings
}

pub fn claude_settings_backup_path_for(path: &Path) -> PathBuf {
    let mut backup = path.to_path_buf();
    let file_name = backup
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "settings.json".to_string());
    backup.set_file_name(format!("{file_name}.codex-helper-backup"));
    backup
}

pub fn claude_settings_backup_path() -> PathBuf {
    claude_settings_backup_path_for(&claude_settings_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_or_blank_client_home_falls_back_to_the_user_home() {
        let home = PathBuf::from("/test-user-home");

        for configured in [Some(OsString::new()), Some(OsString::from("  \t  ")), None] {
            assert_eq!(
                resolve_home_dir_from(configured, Some(home.clone()), ".codex"),
                home.join(".codex")
            );
        }
    }

    #[test]
    fn non_empty_client_home_is_used_verbatim() {
        let configured = PathBuf::from("relative-client-home");

        assert_eq!(
            resolve_home_dir_from(
                Some(configured.clone().into_os_string()),
                Some(PathBuf::from("/ignored")),
                ".claude",
            ),
            configured
        );
    }

    #[test]
    fn missing_user_home_keeps_the_existing_relative_fallback() {
        assert_eq!(
            resolve_home_dir_from(None, None, ".codex"),
            PathBuf::from(".").join(".codex")
        );
    }
}

use std::env;
use std::path::{Path, PathBuf};

use dirs::home_dir;

pub const CODEX_ABSENT_BACKUP_SENTINEL: &str = "# codex-helper-backup:absent";
pub const CLAUDE_ABSENT_BACKUP_SENTINEL: &str = "{\"__codex_helper_backup_absent\":true}";

fn resolve_home_dir(env_var: &str, default_dir_name: &str) -> PathBuf {
    if let Ok(dir) = env::var(env_var) {
        return PathBuf::from(dir);
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(default_dir_name)
}

pub fn codex_home() -> PathBuf {
    resolve_home_dir("CODEX_HOME", ".codex")
}

pub fn codex_config_path() -> PathBuf {
    codex_home().join("config.toml")
}

pub fn codex_backup_config_path() -> PathBuf {
    codex_home().join("config.toml.codex-helper-backup")
}

pub fn codex_auth_path() -> PathBuf {
    codex_home().join("auth.json")
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

pub fn is_codex_absent_backup_sentinel(text: &str) -> bool {
    text.trim() == CODEX_ABSENT_BACKUP_SENTINEL
}

pub fn is_claude_absent_backup_sentinel(text: &str) -> bool {
    text.trim() == CLAUDE_ABSENT_BACKUP_SENTINEL
}

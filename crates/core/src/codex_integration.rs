use std::fs;
use std::path::{Path, PathBuf};

use crate::client_config::{
    CLAUDE_ABSENT_BACKUP_SENTINEL, claude_settings_backup_path_for as claude_settings_backup_path,
    claude_settings_path,
};
use crate::file_replace::write_text_file;
use anyhow::{Context, Result, anyhow};

fn read_text(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error).with_context(|| format!("read {:?}", path)),
    }
}

fn atomic_write(path: &Path, data: &str) -> Result<()> {
    write_text_file(path, data)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexStartupReadinessSeverity {
    Info,
    Warning,
}

impl CodexStartupReadinessSeverity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexStartupReadinessIssueKind {
    SwitchDisabled,
    SwitchPortMismatch,
    DiagnosticError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexStartupReadinessIssue {
    pub kind: CodexStartupReadinessIssueKind,
    pub severity: CodexStartupReadinessSeverity,
    pub title: String,
    pub detail: String,
    pub action: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexStartupReadiness {
    pub issues: Vec<CodexStartupReadinessIssue>,
}

impl CodexStartupReadiness {
    pub fn has_issues(&self) -> bool {
        !self.issues.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct ClaudeSwitchStatus {
    /// Whether Claude Code currently appears to use the local helper proxy.
    pub enabled: bool,
    /// Current `env.ANTHROPIC_BASE_URL` value, when present.
    pub base_url: Option<String>,
    /// Whether a backup file exists for safe restore.
    pub has_backup: bool,
    /// Resolved settings file path (`settings.json` or legacy `claude.json`).
    pub settings_path: PathBuf,
}

pub fn claude_switch_status() -> Result<ClaudeSwitchStatus> {
    let settings_path = claude_settings_path();
    let backup_path = claude_settings_backup_path(&settings_path);

    if !settings_path.exists() {
        return Ok(ClaudeSwitchStatus {
            enabled: false,
            base_url: None,
            has_backup: backup_path.exists(),
            settings_path,
        });
    }

    let text = read_text(&settings_path)?;
    if text.trim().is_empty() {
        return Ok(ClaudeSwitchStatus {
            enabled: false,
            base_url: None,
            has_backup: backup_path.exists(),
            settings_path,
        });
    }

    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => {
            return Ok(ClaudeSwitchStatus {
                enabled: false,
                base_url: None,
                has_backup: backup_path.exists(),
                settings_path,
            });
        }
    };

    let base_url = value
        .as_object()
        .and_then(|object| object.get("env"))
        .and_then(serde_json::Value::as_object)
        .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);
    let enabled = base_url
        .as_deref()
        .is_some_and(|url| url.contains("127.0.0.1") || url.contains("localhost"));

    Ok(ClaudeSwitchStatus {
        enabled,
        base_url,
        has_backup: backup_path.exists(),
        settings_path,
    })
}

pub fn claude_switch_on(port: u16) -> Result<()> {
    claude_switch_on_base_url(&format!("http://127.0.0.1:{port}"))
}

pub fn claude_switch_on_base_url(base_url: &str) -> Result<()> {
    let base_url = crate::control_plane_client::normalize_base_url(base_url)
        .ok_or_else(|| anyhow!("Claude proxy base URL must start with http:// or https://"))?;
    let settings_path = claude_settings_path();
    let backup_path = claude_settings_backup_path(&settings_path);

    if settings_path.exists() && !backup_path.exists() {
        fs::copy(&settings_path, &backup_path).with_context(|| {
            format!(
                "backup Claude settings {:?} -> {:?}",
                settings_path, backup_path
            )
        })?;
    } else if !settings_path.exists() && !backup_path.exists() {
        if let Some(parent) = backup_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create_dir_all {:?}", parent))?;
        }
        fs::write(&backup_path, CLAUDE_ABSENT_BACKUP_SENTINEL)
            .with_context(|| format!("write {:?}", backup_path))?;
    }

    let text = read_text(&settings_path)?;
    let mut value: serde_json::Value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&text).with_context(|| format!("parse {:?} as JSON", settings_path))?
    };
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("Claude settings root must be an object"))?;
    let env = object
        .entry("env".to_string())
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("Claude settings env must be an object"))?;
    env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        serde_json::Value::String(base_url),
    );

    let new_text = serde_json::to_string_pretty(&value)?;
    write_text_file(&settings_path, &new_text)
        .with_context(|| format!("write {:?}", settings_path))?;
    eprintln!(
        "[EXPERIMENTAL] Updated {:?} to use Claude proxy via codex-helper",
        settings_path
    );
    Ok(())
}

pub fn claude_switch_off() -> Result<()> {
    let settings_path = claude_settings_path();
    let backup_path = claude_settings_backup_path(&settings_path);
    if !backup_path.exists() {
        return Ok(());
    }

    let text = read_text(&backup_path)?;
    if text.trim() == CLAUDE_ABSENT_BACKUP_SENTINEL {
        if settings_path.exists() {
            fs::remove_file(&settings_path)
                .with_context(|| format!("remove {:?} (restore absent)", settings_path))?;
        }
    } else {
        atomic_write(&settings_path, &text)
            .with_context(|| format!("restore {:?} -> {:?}", backup_path, settings_path))?;
        eprintln!(
            "[EXPERIMENTAL] Restored Claude settings from backup {:?}",
            backup_path
        );
    }
    fs::remove_file(&backup_path)
        .with_context(|| format!("remove stale backup {:?}", backup_path))?;
    Ok(())
}

/// Warn before replacing an existing local Claude proxy patch.
pub fn guard_claude_settings_before_switch_on_interactive() -> Result<()> {
    use std::io::{self, Write};

    let status = claude_switch_status()?;
    let Some(base_url) = status.base_url.as_deref().filter(|_| status.enabled) else {
        return Ok(());
    };
    let backup_path = claude_settings_backup_path(&status.settings_path);
    if !status.has_backup {
        eprintln!(
            "Warning: Claude settings {:?} points ANTHROPIC_BASE_URL to a local address ({base_url}), but no backup file {:?} was found; inspect this config manually if this is unexpected.",
            status.settings_path, backup_path
        );
        return Ok(());
    }

    if !atty::is(atty::Stream::Stdin) || !atty::is(atty::Stream::Stdout) {
        eprintln!(
            "Notice: Claude settings {:?} already points to the local proxy ({base_url}), and backup {:?} exists; run `codex-helper switch off --claude` to restore the original config.",
            status.settings_path, backup_path
        );
        return Ok(());
    }

    eprintln!(
        "Claude settings {:?} already points ANTHROPIC_BASE_URL to the local proxy ({base_url}), and backup {:?} exists.\nRestore the original Claude settings now? [Y/n] ",
        status.settings_path, backup_path
    );
    eprint!("> ");
    io::stdout().flush().ok();

    let mut input = String::new();
    if let Err(error) = io::stdin().read_line(&mut input) {
        eprintln!("Failed to read input: {error}");
        return Ok(());
    }
    let answer = input.trim();
    let confirmed =
        answer.is_empty() || answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes");
    if confirmed {
        if let Err(error) = claude_switch_off() {
            eprintln!("Failed to restore Claude settings: {error}");
        } else {
            eprintln!("Restored Claude settings from backup.");
        }
    } else {
        eprintln!("Keeping the current Claude settings unchanged.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    struct ScopedEnv {
        saved: Vec<(String, Option<String>)>,
    }

    impl ScopedEnv {
        fn new() -> Self {
            Self { saved: Vec::new() }
        }

        unsafe fn set_path(&mut self, key: &str, value: &Path) {
            self.saved.push((key.to_string(), std::env::var(key).ok()));
            unsafe { std::env::set_var(key, value) };
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..).rev() {
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }

    #[test]
    fn claude_switch_off_refreshes_the_next_backup_snapshot() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-claude-switch-test-{}",
            uuid::Uuid::new_v4()
        ));
        let claude_home = root.join("claude");
        fs::create_dir_all(&claude_home).expect("create Claude home");
        let settings_path = claude_home.join("settings.json");
        let backup_path = claude_home.join("settings.json.codex-helper-backup");
        let original = r#"{
  "env": {
    "ANTHROPIC_BASE_URL": "https://api.anthropic.com/v1"
  }
}"#;
        let updated = r#"{
  "env": {
    "ANTHROPIC_BASE_URL": "https://proxy.example/v1"
  }
}"#;
        fs::write(&settings_path, original).expect("write original settings");

        let mut env = ScopedEnv::new();
        unsafe { env.set_path("CLAUDE_HOME", &claude_home) };

        claude_switch_on(3211).expect("first switch on");
        claude_switch_off().expect("first switch off");
        assert_eq!(fs::read_to_string(&settings_path).unwrap(), original);
        assert!(!backup_path.exists());

        fs::write(&settings_path, updated).expect("write updated settings");
        claude_switch_on(3211).expect("second switch on");
        assert_eq!(fs::read_to_string(&backup_path).unwrap(), updated);
        claude_switch_off().expect("second switch off");
        assert_eq!(fs::read_to_string(&settings_path).unwrap(), updated);
        assert!(!backup_path.exists());

        let _ = fs::remove_dir_all(root);
    }
}

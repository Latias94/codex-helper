use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::client_config::{
    CLAUDE_ABSENT_BACKUP_SENTINEL, CODEX_ABSENT_BACKUP_SENTINEL,
    claude_settings_backup_path_for as claude_settings_backup_path, claude_settings_path,
    codex_backup_config_path as codex_config_backup_path, codex_config_path,
};
use anyhow::{Context, Result, anyhow};
use toml::Value;

fn read_config_text(path: &Path) -> Result<String> {
    if !path.exists() {
        return Ok(String::new());
    }
    let mut file = fs::File::open(path).with_context(|| format!("open {:?}", path))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)
        .with_context(|| format!("read {:?}", path))?;
    Ok(buf)
}

fn atomic_write(path: &Path, data: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create_dir_all {:?}", parent))?;
    }
    let tmp = path.with_extension("tmp.codex-helper");
    {
        let mut f = fs::File::create(&tmp).with_context(|| format!("create {:?}", tmp))?;
        f.write_all(data.as_bytes())
            .with_context(|| format!("write {:?}", tmp))?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, path).with_context(|| format!("rename {:?} -> {:?}", tmp, path))?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct CodexSwitchStatus {
    /// Whether Codex currently appears to be configured to use the local codex-helper proxy.
    pub enabled: bool,
    /// Current `model_provider` value (if any).
    pub model_provider: Option<String>,
    /// Current `model_providers.codex_proxy.base_url` (if any).
    pub base_url: Option<String>,
    /// Whether a backup file exists for safe restore.
    pub has_backup: bool,
}

pub fn codex_switch_status() -> Result<CodexSwitchStatus> {
    let cfg_path = codex_config_path();
    let backup_path = codex_config_backup_path();

    if !cfg_path.exists() {
        return Ok(CodexSwitchStatus {
            enabled: false,
            model_provider: None,
            base_url: None,
            has_backup: backup_path.exists(),
        });
    }

    let text = read_config_text(&cfg_path)?;
    if text.trim().is_empty() {
        return Ok(CodexSwitchStatus {
            enabled: false,
            model_provider: None,
            base_url: None,
            has_backup: backup_path.exists(),
        });
    }

    let value: Value = match text.parse() {
        Ok(v) => v,
        Err(_) => {
            return Ok(CodexSwitchStatus {
                enabled: false,
                model_provider: None,
                base_url: None,
                has_backup: backup_path.exists(),
            });
        }
    };
    let table = match value.as_table() {
        Some(t) => t,
        None => {
            return Ok(CodexSwitchStatus {
                enabled: false,
                model_provider: None,
                base_url: None,
                has_backup: backup_path.exists(),
            });
        }
    };

    let model_provider = table
        .get("model_provider")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if model_provider.as_deref() != Some("codex_proxy") {
        return Ok(CodexSwitchStatus {
            enabled: false,
            model_provider,
            base_url: None,
            has_backup: backup_path.exists(),
        });
    }

    let empty_map = toml::map::Map::new();
    let providers_table = table
        .get("model_providers")
        .and_then(|v| v.as_table())
        .unwrap_or(&empty_map);
    let empty_provider = toml::map::Map::new();
    let proxy_table = providers_table
        .get("codex_proxy")
        .and_then(|v| v.as_table())
        .unwrap_or(&empty_provider);

    let base_url = proxy_table
        .get("base_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let name = proxy_table
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    let is_local = base_url
        .as_deref()
        .is_some_and(|u| u.contains("127.0.0.1") || u.contains("localhost"));
    let is_helper_name = name == "codex-helper";

    Ok(CodexSwitchStatus {
        enabled: is_local || is_helper_name,
        model_provider,
        base_url,
        has_backup: backup_path.exists(),
    })
}

/// Switch Codex to use the local codex-helper model provider.
pub fn switch_on(port: u16) -> Result<()> {
    let cfg_path = codex_config_path();
    let backup_path = codex_config_backup_path();

    // Backup once if original exists and no backup yet.
    if cfg_path.exists() && !backup_path.exists() {
        fs::copy(&cfg_path, &backup_path)
            .with_context(|| format!("backup {:?} -> {:?}", cfg_path, backup_path))?;
    } else if !cfg_path.exists() && !backup_path.exists() {
        // If Codex has no config.toml yet, create a sentinel backup so we can restore
        // to the "absent" state on switch_off.
        atomic_write(&backup_path, CODEX_ABSENT_BACKUP_SENTINEL)?;
    }

    let text = read_config_text(&cfg_path)?;
    let mut table: toml::Table = if text.trim().is_empty() {
        toml::Table::new()
    } else {
        text.parse::<Value>()?
            .as_table()
            .cloned()
            .ok_or_else(|| anyhow!("config.toml root must be table"))?
    };

    // Ensure [model_providers] table exists.
    let providers = table
        .entry("model_providers")
        .or_insert_with(|| Value::Table(toml::Table::new()));

    let providers_table = providers
        .as_table_mut()
        .ok_or_else(|| anyhow!("model_providers must be a table"))?;

    let base_url = format!("http://127.0.0.1:{}", port);
    let mut proxy_table = providers_table
        .get("codex_proxy")
        .and_then(|v| v.as_table())
        .cloned()
        .unwrap_or_else(toml::Table::new);
    proxy_table.insert("name".into(), Value::String("codex-helper".into()));
    proxy_table.insert("base_url".into(), Value::String(base_url));
    proxy_table.insert("wire_api".into(), Value::String("responses".into()));
    // Avoid double-retry (Codex retries + codex-helper retries) by default.
    proxy_table
        .entry("request_max_retries")
        .or_insert(Value::Integer(0));

    providers_table.insert("codex_proxy".into(), Value::Table(proxy_table));
    table.insert("model_provider".into(), Value::String("codex_proxy".into()));

    let new_text = toml::to_string_pretty(&table)?;
    atomic_write(&cfg_path, &new_text)?;
    Ok(())
}

/// Restore Codex config.toml from backup if present.
pub fn switch_off() -> Result<()> {
    let cfg_path = codex_config_path();
    let backup_path = codex_config_backup_path();
    if backup_path.exists() {
        let text = read_config_text(&backup_path)?;
        if text.trim() == CODEX_ABSENT_BACKUP_SENTINEL {
            if cfg_path.exists() {
                fs::remove_file(&cfg_path)
                    .with_context(|| format!("remove {:?} (restore absent)", cfg_path))?;
            }
        } else {
            atomic_write(&cfg_path, &text)
                .with_context(|| format!("restore {:?} -> {:?}", backup_path, cfg_path))?;
        }
        fs::remove_file(&backup_path)
            .with_context(|| format!("remove stale backup {:?}", backup_path))?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ClaudeSwitchStatus {
    /// Whether Claude Code currently appears to be configured to use the local codex-helper proxy.
    pub enabled: bool,
    /// Current `env.ANTHROPIC_BASE_URL` value (if any).
    pub base_url: Option<String>,
    /// Whether a backup file exists for safe restore.
    pub has_backup: bool,
    /// The resolved settings file path (settings.json or legacy claude.json).
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

    let text = read_settings_text(&settings_path)?;
    if text.trim().is_empty() {
        return Ok(ClaudeSwitchStatus {
            enabled: false,
            base_url: None,
            has_backup: backup_path.exists(),
            settings_path,
        });
    }

    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => {
            return Ok(ClaudeSwitchStatus {
                enabled: false,
                base_url: None,
                has_backup: backup_path.exists(),
                settings_path,
            });
        }
    };

    let env_obj = value
        .as_object()
        .and_then(|o| o.get("env"))
        .and_then(|v| v.as_object());

    let base_url = env_obj
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let enabled = base_url
        .as_deref()
        .is_some_and(|u| u.contains("127.0.0.1") || u.contains("localhost"));

    Ok(ClaudeSwitchStatus {
        enabled,
        base_url,
        has_backup: backup_path.exists(),
        settings_path,
    })
}

/// 闂侀潻璐熼崝宀勫疮閳ь剚绻涢崱蹇婂亾閸愯尙鈧ジ鏌熼獮鍨仼闁糕晛鐭傚鐢割敆閳ь剙锕㈢€涙顩烽柨婵嗘处閸婄偤鏌涢幘宕団枌缂佽鲸绻勯埀?Codex 闂備焦婢樼粔鍫曟偪閸℃稑纾绘慨姗嗗亞椤忚鲸绻涢崱蹇婂亾閼碱剛顔旈梺纭呯堪閸婃牠鍩€椤戭剙瀚峰楣冩煛鐏炶鍔橀柍?
pub fn guard_codex_config_before_switch_on_interactive() -> Result<()> {
    use std::io::{self, Write};

    let cfg_path = codex_config_path();
    let backup_path = codex_config_backup_path();

    if !cfg_path.exists() {
        return Ok(());
    }

    let text = read_config_text(&cfg_path)?;
    if text.trim().is_empty() {
        return Ok(());
    }

    let value: Value = match text.parse() {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };
    let table = match value.as_table() {
        Some(table) => table,
        None => return Ok(()),
    };

    let current_provider = table
        .get("model_provider")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if current_provider != "codex_proxy" {
        return Ok(());
    }

    let empty_map = toml::map::Map::new();
    let providers_table = table
        .get("model_providers")
        .and_then(|value| value.as_table())
        .unwrap_or(&empty_map);
    let empty_provider = toml::map::Map::new();
    let proxy_table = providers_table
        .get("codex_proxy")
        .and_then(|value| value.as_table())
        .unwrap_or(&empty_provider);

    let base_url = proxy_table
        .get("base_url")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let name = proxy_table
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or_default();

    let is_local = base_url.contains("127.0.0.1") || base_url.contains("localhost");
    let is_helper_name = name == "codex-helper";
    if !is_local && !is_helper_name {
        return Ok(());
    }

    if !backup_path.exists() {
        eprintln!(
            "Warning: Codex currently points to the local proxy ({base_url}), but no backup file {:?} was found; please inspect ~/.codex/config.toml manually if this is unexpected.",
            backup_path
        );
        return Ok(());
    }

    let is_tty = atty::is(atty::Stream::Stdin) && atty::is(atty::Stream::Stdout);
    if !is_tty {
        eprintln!(
            "Notice: Codex currently points to local codex-helper ({base_url}) and backup {:?} exists; run `codex-helper switch-off` if you want to restore the original config.",
            backup_path
        );
        return Ok(());
    }

    eprintln!(
        "Codex currently points to local codex-helper ({base_url}), and backup {:?} exists.\nThis usually means the previous run did not switch off cleanly.\nRestore the original Codex config now? [Y/n] ",
        backup_path
    );
    eprint!("> ");
    io::stdout().flush().ok();

    let mut input = String::new();
    if let Err(err) = io::stdin().read_line(&mut input) {
        eprintln!("Failed to read input: {err}");
        return Ok(());
    }
    let answer = input.trim();
    let yes =
        answer.is_empty() || answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes");

    if yes {
        if let Err(err) = switch_off() {
            eprintln!("Failed to restore original Codex config: {err}");
        } else {
            eprintln!("Restored original Codex config from backup.");
        }
    } else {
        eprintln!("Keeping current Codex config unchanged.");
    }

    Ok(())
}

fn read_settings_text(path: &Path) -> Result<String> {
    if !path.exists() {
        return Ok(String::new());
    }
    let mut file = fs::File::open(path).with_context(|| format!("open {:?}", path))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)
        .with_context(|| format!("read {:?}", path))?;
    Ok(buf)
}

pub fn claude_switch_on(port: u16) -> Result<()> {
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
        // If Claude Code has no settings yet, create a sentinel backup so we can restore
        // to the "absent" state on claude_switch_off.
        if let Some(parent) = backup_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create_dir_all {:?}", parent))?;
        }
        fs::write(&backup_path, CLAUDE_ABSENT_BACKUP_SENTINEL)
            .with_context(|| format!("write {:?}", backup_path))?;
    }

    let text = read_settings_text(&settings_path)?;
    let mut value: serde_json::Value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&text).with_context(|| format!("parse {:?} as JSON", settings_path))?
    };

    let obj = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("Claude settings root must be an object"))?;

    let env_val = obj
        .entry("env".to_string())
        .or_insert_with(|| serde_json::json!({}));
    let env_obj = env_val
        .as_object_mut()
        .ok_or_else(|| anyhow!("Claude settings env must be an object"))?;

    let base_url = format!("http://127.0.0.1:{}", port);
    env_obj.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        serde_json::Value::String(base_url),
    );

    let new_text = serde_json::to_string_pretty(&value)?;
    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create_dir_all {:?}", parent))?;
    }
    let tmp = settings_path.with_extension("tmp.codex-helper");
    {
        let mut f = fs::File::create(&tmp).with_context(|| format!("create {:?}", tmp))?;
        f.write_all(new_text.as_bytes())
            .with_context(|| format!("write {:?}", tmp))?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, &settings_path)
        .with_context(|| format!("rename {:?} -> {:?}", tmp, settings_path))?;

    eprintln!(
        "[EXPERIMENTAL] Updated {:?} to use local Claude proxy via codex-helper",
        settings_path
    );
    Ok(())
}

pub fn claude_switch_off() -> Result<()> {
    let settings_path = claude_settings_path();
    let backup_path = claude_settings_backup_path(&settings_path);
    if backup_path.exists() {
        let text = read_settings_text(&backup_path)?;
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
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};

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
            for (key, old) in self.saved.drain(..).rev() {
                unsafe {
                    match old {
                        Some(value) => std::env::set_var(&key, value),
                        None => std::env::remove_var(&key),
                    }
                }
            }
        }
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        match LOCK.get_or_init(|| Mutex::new(())).lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        }
    }

    struct TestEnv {
        _lock: std::sync::MutexGuard<'static, ()>,
        _env: ScopedEnv,
        codex_home: PathBuf,
        claude_home: PathBuf,
    }

    fn setup_temp_env() -> TestEnv {
        let lock = env_lock();
        let root =
            std::env::temp_dir().join(format!("codex-helper-switch-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temp root");

        let codex_home = root.join(".codex");
        let claude_home = root.join(".claude");
        std::fs::create_dir_all(&codex_home).expect("create temp codex home");
        std::fs::create_dir_all(&claude_home).expect("create temp claude home");

        let mut scoped = ScopedEnv::new();
        unsafe {
            scoped.set_path("CODEX_HOME", &codex_home);
            scoped.set_path("CLAUDE_HOME", &claude_home);
            scoped.set_path("HOME", &root);
            scoped.set_path("USERPROFILE", &root);
        }

        TestEnv {
            _lock: lock,
            _env: scoped,
            codex_home,
            claude_home,
        }
    }

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent directories");
        }
        std::fs::write(path, content).expect("write test file");
    }

    fn read_file(path: &Path) -> String {
        std::fs::read_to_string(path).expect("read test file")
    }

    #[test]
    fn codex_switch_off_clears_backup_and_refreshes_next_snapshot() {
        let env = setup_temp_env();
        let cfg_path = env.codex_home.join("config.toml");
        let backup_path = env.codex_home.join("config.toml.codex-helper-backup");

        let original = r#"
model_provider = "openai"

[model_providers.openai]
name = "openai"
base_url = "https://api.openai.com/v1"
"#;
        let updated = r#"
model_provider = "packycode"

[model_providers.packycode]
name = "packycode"
base_url = "https://codex-api.packycode.com/v1"
"#;

        write_file(&cfg_path, original.trim_start());
        switch_on(3211).expect("first switch_on should succeed");
        assert!(
            backup_path.exists(),
            "backup should exist while switched on"
        );

        switch_off().expect("first switch_off should succeed");
        assert_eq!(read_file(&cfg_path), original.trim_start());
        assert!(
            !backup_path.exists(),
            "backup should be removed after restore to avoid stale snapshots"
        );

        write_file(&cfg_path, updated.trim_start());
        switch_on(3211).expect("second switch_on should succeed");
        assert_eq!(read_file(&backup_path), updated.trim_start());

        switch_off().expect("second switch_off should succeed");
        assert_eq!(read_file(&cfg_path), updated.trim_start());
        assert!(
            !backup_path.exists(),
            "backup should be cleaned up after the second restore as well"
        );
    }

    #[test]
    fn claude_switch_off_clears_backup_and_refreshes_next_snapshot() {
        let env = setup_temp_env();
        let settings_path = env.claude_home.join("settings.json");
        let backup_path = env.claude_home.join("settings.json.codex-helper-backup");

        let original = r#"{
  "env": {
    "ANTHROPIC_BASE_URL": "https://api.anthropic.com/v1",
    "ANTHROPIC_API_KEY": "sk-ant-1"
  }
}"#;
        let updated = r#"{
  "env": {
    "ANTHROPIC_BASE_URL": "https://anthropic-proxy.example/v1",
    "ANTHROPIC_API_KEY": "sk-ant-2"
  }
}"#;

        write_file(&settings_path, original);
        claude_switch_on(3211).expect("first claude_switch_on should succeed");
        assert!(
            backup_path.exists(),
            "backup should exist while switched on"
        );

        claude_switch_off().expect("first claude_switch_off should succeed");
        assert_eq!(read_file(&settings_path), original);
        assert!(
            !backup_path.exists(),
            "backup should be removed after Claude restore to avoid stale snapshots"
        );

        write_file(&settings_path, updated);
        claude_switch_on(3211).expect("second claude_switch_on should succeed");
        assert_eq!(read_file(&backup_path), updated);

        claude_switch_off().expect("second claude_switch_off should succeed");
        assert_eq!(read_file(&settings_path), updated);
        assert!(
            !backup_path.exists(),
            "backup should be cleaned up after the second Claude restore as well"
        );
    }
}

/// 闂侀潻璐熼崝宀勫疮閳ь剚绻涢崱蹇婂亾閸愯尙鈧ジ鏌熼獮鍨仼闁糕晛鐭傚鐢割敆閳ь剙锕㈢€涙顩烽柨婵嗘处閸婄偤鏌涢幘宕団枌缂佽鲸绻勯埀?Claude settings 闂佺顑嗛惌顔剧博閺夋垟鏋庨柍銉ㄥ皺缁犱粙鏌熼煬鎻掆偓鏍焵椤戭剙瀚峰楣冩煛鐏炶鍔橀柍?
pub fn guard_claude_settings_before_switch_on_interactive() -> Result<()> {
    use std::io::{self, Write};

    let settings_path = claude_settings_path();
    if !settings_path.exists() {
        return Ok(());
    }
    let backup_path = claude_settings_backup_path(&settings_path);

    let text = read_settings_text(&settings_path)?;
    if text.trim().is_empty() {
        return Ok(());
    }

    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };
    let obj = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(()),
    };
    let env_obj = match obj.get("env").and_then(|value| value.as_object()) {
        Some(env_obj) => env_obj,
        None => return Ok(()),
    };

    let base_url = env_obj
        .get("ANTHROPIC_BASE_URL")
        .and_then(|value| value.as_str())
        .unwrap_or_default();

    let is_local = base_url.contains("127.0.0.1") || base_url.contains("localhost");
    if !is_local {
        return Ok(());
    }

    if !backup_path.exists() {
        eprintln!(
            "Warning: Claude settings {:?} points ANTHROPIC_BASE_URL to a local address ({base_url}), but no backup file {:?} was found; please inspect this config file manually if this is unexpected.",
            settings_path, backup_path
        );
        return Ok(());
    }

    let is_tty = atty::is(atty::Stream::Stdin) && atty::is(atty::Stream::Stdout);
    if !is_tty {
        eprintln!(
            "Notice: Claude settings {:?} already points to the local proxy ({base_url}), and backup {:?} exists; run `codex-helper switch-off --claude` if you want to restore the original config.",
            settings_path, backup_path
        );
        return Ok(());
    }

    eprintln!(
        "Claude settings {:?} already points ANTHROPIC_BASE_URL to the local proxy ({base_url}), and backup {:?} exists.\nThis usually means the previous run did not switch off cleanly.\nRestore the original Claude settings now? [Y/n] ",
        settings_path, backup_path
    );
    eprint!("> ");
    io::stdout().flush().ok();

    let mut input = String::new();
    if let Err(err) = io::stdin().read_line(&mut input) {
        eprintln!("Failed to read input: {err}");
        return Ok(());
    }
    let answer = input.trim();
    let yes =
        answer.is_empty() || answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes");

    if yes {
        if let Err(err) = claude_switch_off() {
            eprintln!("Failed to restore Claude settings: {err}");
        } else {
            eprintln!("Restored Claude settings from backup.");
        }
    } else {
        eprintln!("Keeping current Claude settings unchanged.");
    }

    Ok(())
}

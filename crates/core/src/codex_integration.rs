use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use dirs::home_dir;
use toml::Value;

const ABSENT_BACKUP_SENTINEL: &str = "# codex-helper-backup:absent";

fn codex_home() -> PathBuf {
    if let Ok(dir) = env::var("CODEX_HOME") {
        return PathBuf::from(dir);
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex")
}

fn codex_config_path() -> PathBuf {
    codex_home().join("config.toml")
}

fn codex_config_backup_path() -> PathBuf {
    codex_home().join("config.toml.codex-helper-backup")
}

fn read_config_text(path: &PathBuf) -> Result<String> {
    if !path.exists() {
        return Ok(String::new());
    }
    let mut file = fs::File::open(path).with_context(|| format!("open {:?}", path))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)
        .with_context(|| format!("read {:?}", path))?;
    Ok(buf)
}

fn atomic_write(path: &PathBuf, data: &str) -> Result<()> {
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
        atomic_write(&backup_path, ABSENT_BACKUP_SENTINEL)?;
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
        if text.trim() == ABSENT_BACKUP_SENTINEL {
            if cfg_path.exists() {
                fs::remove_file(&cfg_path)
                    .with_context(|| format!("remove {:?} (restore absent)", cfg_path))?;
            }
        } else {
            fs::copy(&backup_path, &cfg_path)
                .with_context(|| format!("restore {:?} -> {:?}", backup_path, cfg_path))?;
        }
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

/// 在再次切换到本地代理之前，对 Codex 配置做一次守护性检查：
/// - 如发现已存在备份文件，且当前 model_provider 已指向本地代理（127.0.0.1 / codex-helper），
///   则在交互式终端中询问用户是否先恢复原始配置；非交互环境下仅打印告警。
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
        Ok(v) => v,
        Err(_) => return Ok(()),
    };
    let table = match value.as_table() {
        Some(t) => t,
        None => return Ok(()),
    };

    let current_provider = table
        .get("model_provider")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if current_provider != "codex_proxy" {
        return Ok(());
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
        .unwrap_or_default();
    let name = proxy_table
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    // 仅当当前 provider 看起来是“本地 codex-helper 代理”时才触发守护逻辑。
    let is_local = base_url.contains("127.0.0.1") || base_url.contains("localhost");
    let is_helper_name = name == "codex-helper";
    if !is_local && !is_helper_name {
        return Ok(());
    }

    // 如果没有备份文件，无法安全恢复，只打印告警。
    if !backup_path.exists() {
        eprintln!(
            "警告：检测到 Codex 当前 model_provider 指向本地地址 ({base_url})，\
但未找到备份文件 {:?}；如非预期，请手动检查 ~/.codex/config.toml。",
            backup_path
        );
        return Ok(());
    }

    // 非交互环境：打印提示但不阻断。
    let is_tty = atty::is(atty::Stream::Stdin) && atty::is(atty::Stream::Stdout);
    if !is_tty {
        eprintln!(
            "注意：检测到 Codex 当前已指向本地代理 codex-helper ({base_url})，\
且存在备份文件 {:?}；如需恢复原始配置，可运行 `codex-helper switch-off`。",
            backup_path
        );
        return Ok(());
    }

    // 交互模式：询问是否先恢复原始配置。
    eprintln!(
        "检测到 Codex 当前已指向本地代理 codex-helper ({base_url})，且存在备份文件 {:?}。\n\
这通常意味着上一次 codex-helper 未通过 switch-off 恢复配置。\n\
是否现在恢复原始 Codex 配置？ [Y/n] ",
        backup_path
    );
    eprint!("> ");
    io::stdout().flush().ok();

    let mut input = String::new();
    if let Err(err) = io::stdin().read_line(&mut input) {
        eprintln!("读取输入失败：{err}");
        return Ok(());
    }
    let answer = input.trim();
    let yes =
        answer.is_empty() || answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes");

    if yes {
        if let Err(err) = switch_off() {
            eprintln!("恢复 Codex 原始配置失败：{err}");
        } else {
            eprintln!("已根据备份恢复 Codex 原始配置。");
        }
    } else {
        eprintln!("保留当前 Codex 配置不变。");
    }

    Ok(())
}

fn claude_home() -> PathBuf {
    if let Ok(dir) = env::var("CLAUDE_HOME") {
        return PathBuf::from(dir);
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
}

fn claude_settings_path() -> PathBuf {
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

fn claude_settings_backup_path(path: &Path) -> PathBuf {
    let mut backup = path.to_path_buf();
    let file_name = backup
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "settings.json".to_string());
    backup.set_file_name(format!("{file_name}.codex-helper-backup"));
    backup
}

const CLAUDE_ABSENT_BACKUP_SENTINEL: &str = "{\"__codex_helper_backup_absent\":true}";

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

/// 将 Claude Code 的 settings.json 指向本地 codex-helper 代理（实验性）。
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

/// 从备份恢复 Claude settings.json（若存在）。
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
            fs::copy(&backup_path, &settings_path)
                .with_context(|| format!("restore {:?} -> {:?}", backup_path, settings_path))?;
            eprintln!(
                "[EXPERIMENTAL] Restored Claude settings from backup {:?}",
                backup_path
            );
        }
    }
    Ok(())
}

/// Claude settings Guard：在修改 settings.json 之前，如发现当前已指向本地代理且存在备份，
/// 则在交互模式下询问是否先恢复；非交互环境仅打印提示。
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
        Ok(v) => v,
        Err(_) => return Ok(()),
    };
    let obj = match value.as_object() {
        Some(o) => o,
        None => return Ok(()),
    };
    let env_obj = match obj.get("env").and_then(|v| v.as_object()) {
        Some(e) => e,
        None => return Ok(()),
    };

    let base_url = env_obj
        .get("ANTHROPIC_BASE_URL")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    let is_local = base_url.contains("127.0.0.1") || base_url.contains("localhost");
    if !is_local {
        return Ok(());
    }

    if !backup_path.exists() {
        eprintln!(
            "警告：检测到 Claude settings {:?} 的 ANTHROPIC_BASE_URL 指向本地地址 ({base_url})，\
但未找到备份文件 {:?}；如非预期，请手动检查该文件。",
            settings_path, backup_path
        );
        return Ok(());
    }

    let is_tty = atty::is(atty::Stream::Stdin) && atty::is(atty::Stream::Stdout);
    if !is_tty {
        eprintln!(
            "注意：检测到 Claude settings {:?} 已指向本地代理 ({base_url})，且存在备份 {:?}；\
如需恢复原始配置，可运行 `codex-helper switch-off --claude`。",
            settings_path, backup_path
        );
        return Ok(());
    }

    eprintln!(
        "检测到 Claude settings {:?} 的 ANTHROPIC_BASE_URL 已指向本地代理 ({base_url})，且存在备份文件 {:?}。\n\
这通常意味着上一次 codex-helper 未通过 switch-off --claude 恢复配置。\n\
是否现在恢复原始 Claude settings？ [Y/n] ",
        settings_path, backup_path
    );
    eprint!("> ");
    io::stdout().flush().ok();

    let mut input = String::new();
    if let Err(err) = io::stdin().read_line(&mut input) {
        eprintln!("读取输入失败：{err}");
        return Ok(());
    }
    let answer = input.trim();
    let yes =
        answer.is_empty() || answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes");

    if yes {
        if let Err(err) = claude_switch_off() {
            eprintln!("恢复 Claude settings 失败：{err}");
        } else {
            eprintln!("已根据备份恢复 Claude settings。");
        }
    } else {
        eprintln!("保留当前 Claude settings 不变。");
    }

    Ok(())
}

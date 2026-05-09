use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::client_config::{
    CLAUDE_ABSENT_BACKUP_SENTINEL, claude_settings_backup_path_for as claude_settings_backup_path,
    claude_settings_path, codex_config_path, codex_switch_state_path,
};
use crate::file_replace::write_text_file;
use anyhow::{Context, Result, anyhow};
use toml::Value;
use toml_edit::{
    Document as EditableTomlDocument, Item as EditableTomlItem, Table as EditableTomlTable,
    Value as EditableTomlValue, value as editable_toml_value,
};

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
    write_text_file(path, data)
}

fn set_toml_value_preserving_decor(item: &mut EditableTomlItem, mut value: EditableTomlValue) {
    if let Some(current) = item.as_value_mut() {
        let decor = current.decor().clone();
        *value.decor_mut() = decor;
        *current = value;
    } else {
        *item = EditableTomlItem::Value(value);
    }
}

fn set_toml_string(table: &mut EditableTomlTable, key: &str, value: impl Into<String>) {
    let item = table.entry(key).or_insert(EditableTomlItem::None);
    set_toml_value_preserving_decor(item, EditableTomlValue::from(value.into()));
}

fn toml_string(table: &EditableTomlTable, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(EditableTomlItem::as_value)
        .and_then(EditableTomlValue::as_str)
        .map(ToOwned::to_owned)
}

fn local_helper_proxy_item(item: Option<&EditableTomlItem>) -> bool {
    let Some(table) = item.and_then(EditableTomlItem::as_table) else {
        return false;
    };
    let name_is_helper = toml_string(table, "name").as_deref() == Some("codex-helper");
    let base_url_is_local = toml_string(table, "base_url")
        .as_deref()
        .is_some_and(|url| url.contains("127.0.0.1") || url.contains("localhost"));
    name_is_helper || base_url_is_local
}

fn codex_text_points_to_local_helper(text: &str) -> Result<bool> {
    if text.trim().is_empty() {
        return Ok(false);
    }
    let doc = text.parse::<EditableTomlDocument>()?;
    let root = doc.as_table();
    if toml_string(root, "model_provider").as_deref() != Some("codex_proxy") {
        return Ok(false);
    }
    Ok(root
        .get("model_providers")
        .and_then(EditableTomlItem::as_table)
        .and_then(|table| table.get("codex_proxy"))
        .is_some_and(|item| local_helper_proxy_item(Some(item))))
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct CodexSwitchState {
    version: u32,
    original_config_absent: bool,
    original_model_provider: Option<String>,
    original_codex_proxy: Option<Value>,
    had_model_providers: bool,
}

impl CodexSwitchState {
    fn from_codex_config_text(text: &str, original_config_absent: bool) -> Result<Self> {
        let doc = if text.trim().is_empty() {
            EditableTomlDocument::new()
        } else {
            text.parse::<EditableTomlDocument>()?
        };
        let root = doc.as_table();
        let providers_table = root
            .get("model_providers")
            .and_then(EditableTomlItem::as_table);

        Ok(Self {
            version: 1,
            original_config_absent,
            original_model_provider: toml_string(root, "model_provider"),
            original_codex_proxy: original_codex_proxy_value(text)?,
            had_model_providers: providers_table.is_some(),
        })
    }
}

fn original_codex_proxy_value(text: &str) -> Result<Option<Value>> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let value = text.parse::<Value>()?;
    Ok(value
        .as_table()
        .and_then(|root| root.get("model_providers"))
        .and_then(Value::as_table)
        .and_then(|providers| providers.get("codex_proxy"))
        .cloned())
}

fn editable_item_from_toml_value(value: &Value) -> Result<EditableTomlItem> {
    match value {
        Value::Table(table) => {
            let body = toml::to_string(table)?;
            let doc = format!("[codex_proxy]\n{body}").parse::<EditableTomlDocument>()?;
            doc.as_table()
                .get("codex_proxy")
                .cloned()
                .ok_or_else(|| anyhow!("failed to parse stored codex_proxy state"))
        }
        _ => Err(anyhow!("stored codex_proxy state must be a TOML table")),
    }
}

fn read_codex_switch_state() -> Result<Option<CodexSwitchState>> {
    let path = codex_switch_state_path();
    if !path.exists() {
        return Ok(None);
    }
    let text = read_config_text(&path)?;
    let state = serde_json::from_str::<CodexSwitchState>(&text)
        .with_context(|| format!("parse {:?}", path))?;
    Ok(Some(state))
}

fn write_codex_switch_state_if_absent(state: &CodexSwitchState) -> Result<()> {
    let path = codex_switch_state_path();
    if path.exists() {
        return Ok(());
    }
    let text = serde_json::to_string_pretty(state)?;
    atomic_write(&path, &text)
}

pub fn codex_switch_state_exists() -> bool {
    codex_switch_state_path().exists()
}

enum CodexSwitchOffEdit {
    Write(String),
    RemoveFile,
}

fn switch_off_codex_toml(
    current_text: &str,
    original: &CodexSwitchState,
) -> Result<CodexSwitchOffEdit> {
    let mut doc = if current_text.trim().is_empty() {
        EditableTomlDocument::new()
    } else {
        current_text.parse::<EditableTomlDocument>()?
    };
    let root = doc.as_table_mut();

    let current_model_provider = toml_string(root, "model_provider");
    let proxy_is_helper = root
        .get("model_providers")
        .and_then(EditableTomlItem::as_table)
        .and_then(|table| table.get("codex_proxy"))
        .map(|item| local_helper_proxy_item(Some(item)))
        .unwrap_or(current_model_provider.as_deref() == Some("codex_proxy"));

    if current_model_provider.as_deref() == Some("codex_proxy") && proxy_is_helper {
        if let Some(provider) = original.original_model_provider.as_deref() {
            set_toml_string(root, "model_provider", provider);
        } else {
            root.remove("model_provider");
        }
    }

    let mut remove_model_providers = false;
    if let Some(providers_table) = root
        .get_mut("model_providers")
        .and_then(EditableTomlItem::as_table_mut)
    {
        let proxy_is_helper = local_helper_proxy_item(providers_table.get("codex_proxy"));
        if proxy_is_helper {
            if let Some(original_proxy) = original.original_codex_proxy.as_ref() {
                providers_table.insert(
                    "codex_proxy",
                    editable_item_from_toml_value(original_proxy)?,
                );
            } else {
                providers_table.remove("codex_proxy");
            }
        }
        remove_model_providers = !original.had_model_providers && providers_table.is_empty();
    }
    if remove_model_providers {
        root.remove("model_providers");
    }

    if original.original_config_absent && root.is_empty() {
        Ok(CodexSwitchOffEdit::RemoveFile)
    } else {
        Ok(CodexSwitchOffEdit::Write(doc.to_string()))
    }
}

fn codex_config_text_with_switch_state(
    current_text: &str,
    state: &CodexSwitchState,
) -> Result<String> {
    let mut doc = if current_text.trim().is_empty() {
        EditableTomlDocument::new()
    } else {
        current_text.parse::<EditableTomlDocument>()?
    };
    let root = doc.as_table_mut();
    let current_model_provider = toml_string(root, "model_provider");
    let proxy_is_helper = root
        .get("model_providers")
        .and_then(EditableTomlItem::as_table)
        .and_then(|table| table.get("codex_proxy"))
        .map(|item| local_helper_proxy_item(Some(item)))
        .unwrap_or(current_model_provider.as_deref() == Some("codex_proxy"));

    if current_model_provider.as_deref() != Some("codex_proxy") || !proxy_is_helper {
        return Ok(current_text.to_string());
    }

    if let Some(provider) = state.original_model_provider.as_deref() {
        set_toml_string(root, "model_provider", provider);
    } else {
        root.remove("model_provider");
    }

    let mut remove_model_providers = false;
    if let Some(providers_table) = root
        .get_mut("model_providers")
        .and_then(EditableTomlItem::as_table_mut)
    {
        if let Some(original_proxy) = state.original_codex_proxy.as_ref() {
            providers_table.insert(
                "codex_proxy",
                editable_item_from_toml_value(original_proxy)?,
            );
        } else {
            providers_table.remove("codex_proxy");
        }
        remove_model_providers = !state.had_model_providers && providers_table.is_empty();
    }
    if remove_model_providers {
        root.remove("model_providers");
    }

    Ok(doc.to_string())
}

pub fn codex_config_text_for_import() -> Result<Option<String>> {
    let cfg_path = codex_config_path();
    if !cfg_path.exists() {
        return Ok(None);
    }
    let current_text = read_config_text(&cfg_path)?;
    let Some(state) = read_codex_switch_state()? else {
        return Ok(Some(current_text));
    };
    codex_config_text_with_switch_state(&current_text, &state).map(Some)
}

fn switch_on_codex_toml(text: &str, port: u16) -> Result<String> {
    let mut doc = if text.trim().is_empty() {
        EditableTomlDocument::new()
    } else {
        text.parse::<EditableTomlDocument>()?
    };
    let root = doc.as_table_mut();

    if !root.contains_key("model_providers") {
        root.insert(
            "model_providers",
            EditableTomlItem::Table(EditableTomlTable::new()),
        );
    }
    let providers_table = root
        .get_mut("model_providers")
        .and_then(EditableTomlItem::as_table_mut)
        .ok_or_else(|| anyhow!("model_providers must be a table"))?;

    if !providers_table.contains_key("codex_proxy") {
        providers_table.insert(
            "codex_proxy",
            EditableTomlItem::Table(EditableTomlTable::new()),
        );
    }
    let proxy_table = providers_table
        .get_mut("codex_proxy")
        .and_then(EditableTomlItem::as_table_mut)
        .ok_or_else(|| anyhow!("model_providers.codex_proxy must be a table"))?;

    set_toml_string(proxy_table, "name", "codex-helper");
    set_toml_string(proxy_table, "base_url", format!("http://127.0.0.1:{port}"));
    set_toml_string(proxy_table, "wire_api", "responses");
    if !proxy_table.contains_key("request_max_retries") {
        proxy_table.insert("request_max_retries", editable_toml_value(0));
    }

    set_toml_string(root, "model_provider", "codex_proxy");
    Ok(doc.to_string())
}

#[derive(Debug, Clone)]
pub struct CodexSwitchStatus {
    /// Whether Codex currently appears to be configured to use the local codex-helper proxy.
    pub enabled: bool,
    /// Current `model_provider` value (if any).
    pub model_provider: Option<String>,
    /// Current `model_providers.codex_proxy.base_url` (if any).
    pub base_url: Option<String>,
    /// Whether original switch metadata exists for disabling the local proxy patch.
    pub has_switch_state: bool,
}

pub fn codex_switch_status() -> Result<CodexSwitchStatus> {
    let cfg_path = codex_config_path();
    let state_path = codex_switch_state_path();

    if !cfg_path.exists() {
        return Ok(CodexSwitchStatus {
            enabled: false,
            model_provider: None,
            base_url: None,
            has_switch_state: state_path.exists(),
        });
    }

    let text = read_config_text(&cfg_path)?;
    if text.trim().is_empty() {
        return Ok(CodexSwitchStatus {
            enabled: false,
            model_provider: None,
            base_url: None,
            has_switch_state: state_path.exists(),
        });
    }

    let value: Value = match text.parse() {
        Ok(v) => v,
        Err(_) => {
            return Ok(CodexSwitchStatus {
                enabled: false,
                model_provider: None,
                base_url: None,
                has_switch_state: state_path.exists(),
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
                has_switch_state: state_path.exists(),
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
            has_switch_state: state_path.exists(),
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
        has_switch_state: state_path.exists(),
    })
}

/// Switch Codex to use the local codex-helper model provider.
pub fn switch_on(port: u16) -> Result<()> {
    let cfg_path = codex_config_path();
    let state_path = codex_switch_state_path();
    let text = read_config_text(&cfg_path)?;
    if !state_path.exists() && codex_text_points_to_local_helper(&text)? {
        return Err(anyhow!(
            "Codex already points to the local codex-helper proxy, but no switch state was found at {:?}; refusing to treat the local proxy as the original provider. Please inspect ~/.codex/config.toml manually or run `codex-helper switch off` only if a switch state exists.",
            state_path
        ));
    }
    let state = CodexSwitchState::from_codex_config_text(&text, !cfg_path.exists())?;
    write_codex_switch_state_if_absent(&state)?;
    let new_text = switch_on_codex_toml(&text, port)?;
    atomic_write(&cfg_path, &new_text)?;
    Ok(())
}

/// Undo the local Codex proxy patch while preserving config edits made during the run.
pub fn switch_off() -> Result<()> {
    let cfg_path = codex_config_path();
    let state_path = codex_switch_state_path();
    if state_path.exists() {
        if !cfg_path.exists() {
            fs::remove_file(&state_path)
                .with_context(|| format!("remove stale switch state {:?}", state_path))?;
            return Ok(());
        }
        let state = read_codex_switch_state()?.ok_or_else(|| {
            anyhow!(
                "missing Codex switch state at {:?}",
                codex_switch_state_path()
            )
        })?;
        let current_text = read_config_text(&cfg_path)?;
        match switch_off_codex_toml(&current_text, &state)? {
            CodexSwitchOffEdit::RemoveFile => {
                if cfg_path.exists() {
                    fs::remove_file(&cfg_path)
                        .with_context(|| format!("remove {:?} (restore absent)", cfg_path))?;
                }
            }
            CodexSwitchOffEdit::Write(text) => {
                atomic_write(&cfg_path, &text)
                    .with_context(|| format!("patch {:?} to disable local proxy", cfg_path))?;
            }
        }
        fs::remove_file(&state_path)
            .with_context(|| format!("remove stale switch state {:?}", state_path))?;
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

/// Warn before replacing an existing local Codex proxy patch.
pub fn guard_codex_config_before_switch_on_interactive() -> Result<()> {
    use std::io::{self, Write};

    let cfg_path = codex_config_path();
    let state_path = codex_switch_state_path();

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

    if !state_path.exists() {
        eprintln!(
            "Warning: Codex currently points to the local proxy ({base_url}), but no codex-helper switch state {:?} was found; please inspect ~/.codex/config.toml manually if this is unexpected.",
            state_path
        );
        return Ok(());
    }

    let is_tty = atty::is(atty::Stream::Stdin) && atty::is(atty::Stream::Stdout);
    if !is_tty {
        eprintln!(
            "Notice: Codex currently points to local codex-helper ({base_url}) and switch state {:?} exists; run `codex-helper switch off` to disable the local proxy patch while preserving other config edits.",
            state_path
        );
        return Ok(());
    }

    eprintln!(
        "Codex currently points to local codex-helper ({base_url}), and switch state {:?} exists.\nThis usually means the previous run did not switch off cleanly.\nDisable the local proxy patch now while preserving other config edits? [Y/n] ",
        state_path
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
            eprintln!("Failed to disable local Codex proxy patch: {err}");
        } else {
            eprintln!("Disabled local Codex proxy patch.");
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
    write_text_file(&settings_path, &new_text)
        .with_context(|| format!("write {:?}", settings_path))?;

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
#[allow(clippy::items_after_test_module)]
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
    fn codex_switch_on_preserves_unrelated_toml_comments_and_fields() {
        let env = setup_temp_env();
        let cfg_path = env.codex_home.join("config.toml");

        let original = r#"# top comment
model_provider = "openai"

[model_providers.openai]
# keep this comment
name = "OpenAI"
base_url = "https://api.openai.com/v1"
request_max_retries = 3

[projects."D:\\Work"]
trust_level = "trusted"
"#;

        write_file(&cfg_path, original);
        switch_on(3211).expect("switch_on should preserve editable TOML structure");

        let updated = read_file(&cfg_path);
        assert!(updated.contains("# top comment"));
        assert!(updated.contains("# keep this comment"));
        assert!(updated.contains("[model_providers.openai]"));
        assert!(updated.contains("[projects."));
        assert!(updated.contains("model_provider = \"codex_proxy\""));
        assert!(updated.contains("[model_providers.codex_proxy]"));
        assert!(updated.contains("base_url = \"http://127.0.0.1:3211\""));
    }

    #[test]
    fn codex_switch_on_keeps_existing_proxy_retry_setting() {
        let text = r#"
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "custom"
base_url = "http://127.0.0.1:1111"
request_max_retries = 5
"#;

        let updated = switch_on_codex_toml(text, 3333)
            .expect("switch_on should update the local proxy provider in place");

        assert!(updated.contains("request_max_retries = 5"));
        assert!(updated.contains("base_url = \"http://127.0.0.1:3333\""));
        assert!(updated.contains("name = \"codex-helper\""));
    }

    #[test]
    fn codex_switch_on_refuses_local_proxy_without_switch_state() {
        let env = setup_temp_env();
        let cfg_path = env.codex_home.join("config.toml");
        let state_path = env.codex_home.join("codex-helper-switch-state.json");

        write_file(
            &cfg_path,
            r#"
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "codex-helper"
base_url = "http://127.0.0.1:3211"
"#
            .trim_start(),
        );

        let err = switch_on(3211).expect_err("switch_on should not snapshot a local proxy");
        assert!(err.to_string().contains("no switch state was found"));
        assert!(
            !state_path.exists(),
            "switch_on must not create state from an already-patched local proxy"
        );
    }

    #[test]
    fn codex_config_text_for_import_hides_proxy_created_from_absent_config() {
        let env = setup_temp_env();
        let cfg_path = env.codex_home.join("config.toml");

        switch_on(3211).expect("switch_on should create config");
        assert!(cfg_path.exists());

        let import_text = codex_config_text_for_import()
            .expect("read import view")
            .expect("config exists");
        assert!(
            import_text.trim().is_empty(),
            "import view should not expose helper proxy as a real upstream"
        );
    }

    #[test]
    fn codex_switch_off_clears_switch_state_and_refreshes_next_snapshot() {
        let env = setup_temp_env();
        let cfg_path = env.codex_home.join("config.toml");
        let state_path = env.codex_home.join("codex-helper-switch-state.json");

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
            state_path.exists(),
            "switch state should exist while patched"
        );
        let state_text = read_file(&state_path);
        assert!(state_text.contains("\"original_model_provider\": \"openai\""));
        assert!(
            !state_text.contains("api.openai.com"),
            "switch state should not store the full Codex config"
        );

        switch_off().expect("first switch_off should succeed");
        assert_eq!(read_file(&cfg_path), original.trim_start());
        assert!(
            !state_path.exists(),
            "switch state should be removed after patch-off to avoid stale snapshots"
        );

        write_file(&cfg_path, updated.trim_start());
        switch_on(3211).expect("second switch_on should succeed");
        let state_text = read_file(&state_path);
        assert!(state_text.contains("\"original_model_provider\": \"packycode\""));

        switch_off().expect("second switch_off should succeed");
        assert_eq!(read_file(&cfg_path), updated.trim_start());
        assert!(
            !state_path.exists(),
            "switch state should be cleaned up after the second patch-off as well"
        );
    }

    #[test]
    fn codex_switch_off_preserves_codex_runtime_config_edits() {
        let env = setup_temp_env();
        let cfg_path = env.codex_home.join("config.toml");
        let state_path = env.codex_home.join("codex-helper-switch-state.json");

        let original = r#"
model_provider = "openai"

[model_providers.openai]
name = "openai"
base_url = "https://api.openai.com/v1"
"#;

        write_file(&cfg_path, original.trim_start());
        switch_on(3211).expect("switch_on should succeed");

        let mut during_run = read_file(&cfg_path);
        during_run.push_str(
            r#"
[projects."D:\\Projects\\rust\\codex-helper"]
trust_level = "trusted"
"#,
        );
        write_file(&cfg_path, &during_run);

        switch_off().expect("switch_off should patch rather than restore whole file");

        let updated = read_file(&cfg_path);
        assert!(updated.contains("model_provider = \"openai\""));
        assert!(updated.contains("[model_providers.openai]"));
        assert!(!updated.contains("[model_providers.codex_proxy]"));
        assert!(updated.contains("[projects."));
        assert!(updated.contains("trust_level = \"trusted\""));
        assert!(
            !state_path.exists(),
            "switch state should be removed after successful patch-off"
        );
    }

    #[test]
    fn codex_switch_off_keeps_user_provider_change_made_during_run() {
        let env = setup_temp_env();
        let cfg_path = env.codex_home.join("config.toml");

        let original = r#"
model_provider = "openai"

[model_providers.openai]
name = "openai"
base_url = "https://api.openai.com/v1"
"#;
        let user_changed = r#"
model_provider = "packycode"

[model_providers.openai]
name = "openai"
base_url = "https://api.openai.com/v1"

[model_providers.codex_proxy]
name = "codex-helper"
base_url = "http://127.0.0.1:3211"
wire_api = "responses"
request_max_retries = 0

[model_providers.packycode]
name = "packycode"
base_url = "https://codex-api.packycode.com/v1"
"#;

        write_file(&cfg_path, original.trim_start());
        switch_on(3211).expect("switch_on should succeed");
        write_file(&cfg_path, user_changed.trim_start());

        switch_off().expect("switch_off should not undo user's model_provider change");

        let updated = read_file(&cfg_path);
        assert!(updated.contains("model_provider = \"packycode\""));
        assert!(updated.contains("[model_providers.packycode]"));
        assert!(!updated.contains("[model_providers.codex_proxy]"));
    }

    #[test]
    fn codex_switch_off_preserves_new_config_when_original_was_absent() {
        let env = setup_temp_env();
        let cfg_path = env.codex_home.join("config.toml");

        switch_on(3211).expect("switch_on should create config");
        let mut during_run = read_file(&cfg_path);
        during_run.push_str(
            r#"
[projects."D:\\Projects\\rust\\codex-helper"]
trust_level = "trusted"
"#,
        );
        write_file(&cfg_path, &during_run);

        switch_off().expect("switch_off should remove only local proxy fields");

        let updated = read_file(&cfg_path);
        assert!(!updated.contains("model_provider = \"codex_proxy\""));
        assert!(!updated.contains("[model_providers.codex_proxy]"));
        assert!(updated.contains("[projects."));
        assert!(updated.contains("trust_level = \"trusted\""));
    }

    #[test]
    fn codex_switch_off_removes_empty_config_created_by_switch_on() {
        let env = setup_temp_env();
        let cfg_path = env.codex_home.join("config.toml");

        switch_on(3211).expect("switch_on should create config");
        assert!(cfg_path.exists());

        switch_off().expect("switch_off should restore absent config state");

        assert!(
            !cfg_path.exists(),
            "config created only for the local proxy should be removed"
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

/// Warn before replacing an existing local Claude proxy patch.
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
            "Notice: Claude settings {:?} already points to the local proxy ({base_url}), and backup {:?} exists; run `codex-helper switch off --claude` if you want to restore the original config.",
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

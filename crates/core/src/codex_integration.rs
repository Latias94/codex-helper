use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::client_config::{
    CLAUDE_ABSENT_BACKUP_SENTINEL, claude_settings_backup_path_for as claude_settings_backup_path,
    claude_settings_path, codex_auth_path, codex_config_path, codex_switch_state_path,
};
use crate::config::{ProxyConfig, ProxyConfigV2, ProxyConfigV4};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    patch_mode: Option<CodexPatchMode>,
    original_config_absent: bool,
    original_model_provider: Option<String>,
    original_codex_proxy: Option<Value>,
    had_model_providers: bool,
    #[serde(default)]
    original_auth_json_absent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    original_auth_json: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    patched_auth_json: Option<String>,
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
            version: 2,
            patch_mode: None,
            original_config_absent,
            original_model_provider: toml_string(root, "model_provider"),
            original_codex_proxy: original_codex_proxy_value(text)?,
            had_model_providers: providers_table.is_some(),
            original_auth_json_absent: false,
            original_auth_json: None,
            patched_auth_json: None,
        })
    }

    fn set_auth_patch(&mut self, patch: &CodexAuthPatch) {
        self.original_auth_json_absent = patch.original_absent;
        self.original_auth_json = patch.original_text.clone();
        self.patched_auth_json = Some(patch.patched_text.clone());
    }

    fn clear_auth_patch(&mut self) {
        self.original_auth_json_absent = false;
        self.original_auth_json = None;
        self.patched_auth_json = None;
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

fn write_codex_switch_state(state: &CodexSwitchState) -> Result<()> {
    let path = codex_switch_state_path();
    let text = serde_json::to_string_pretty(state)?;
    atomic_write(&path, &text)
}

pub fn codex_switch_state_exists() -> bool {
    codex_switch_state_path().exists()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CodexPatchMode {
    /// Keep the historical codex-helper patch behavior.
    #[default]
    Default,
    /// Keep Codex/ChatGPT account auth for app/mobile features while model traffic goes through
    /// codex-helper.
    ChatGptBridge,
    /// Use a minimal ChatGPT-looking auth facade to expose Codex hosted image generation while
    /// request credentials still come from codex-helper routing/upstream configuration.
    ImagegenBridge,
}

impl CodexPatchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::ChatGptBridge => "chatgpt-bridge",
            Self::ImagegenBridge => "imagegen-bridge",
        }
    }

    pub fn is_default(self) -> bool {
        matches!(self, Self::Default)
    }

    pub fn strips_codex_client_auth(self) -> bool {
        matches!(self, Self::ChatGptBridge | Self::ImagegenBridge)
    }
}

impl std::fmt::Display for CodexPatchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

enum CodexSwitchOffEdit {
    Write(String),
    RemoveFile,
}

struct CodexAuthPatch {
    original_absent: bool,
    original_text: Option<String>,
    patched_text: String,
}

enum CodexAuthEdit {
    None,
    Write(String),
    RemoveFile,
}

fn auth_json_matches_helper_patch(current_text: Option<&str>, patched_auth_json: &str) -> bool {
    let Some(current_text) = current_text else {
        return false;
    };
    if current_text == patched_auth_json {
        return true;
    }

    let Ok(current_value) = serde_json::from_str::<serde_json::Value>(current_text) else {
        return false;
    };
    let Ok(patched_value) = serde_json::from_str::<serde_json::Value>(patched_auth_json) else {
        return false;
    };
    current_value == patched_value
}

fn apply_codex_auth_edit(edit: CodexAuthEdit) -> Result<()> {
    match edit {
        CodexAuthEdit::None => Ok(()),
        CodexAuthEdit::Write(text) => atomic_write(&codex_auth_path(), &text)
            .with_context(|| format!("patch {:?}", codex_auth_path())),
        CodexAuthEdit::RemoveFile => {
            let path = codex_auth_path();
            if path.exists() {
                fs::remove_file(&path).with_context(|| format!("remove {:?}", path))?;
            }
            Ok(())
        }
    }
}

fn auth_restore_edit_from_state(state: &mut CodexSwitchState) -> Result<CodexAuthEdit> {
    let Some(patched_auth_json) = state.patched_auth_json.as_deref() else {
        return Ok(CodexAuthEdit::None);
    };

    let auth_path = codex_auth_path();
    let current_text = if auth_path.exists() {
        Some(read_config_text(&auth_path)?)
    } else {
        None
    };

    let edit = if auth_json_matches_helper_patch(current_text.as_deref(), patched_auth_json) {
        if state.original_auth_json_absent {
            CodexAuthEdit::RemoveFile
        } else if let Some(original) = state.original_auth_json.clone() {
            CodexAuthEdit::Write(original)
        } else {
            CodexAuthEdit::None
        }
    } else {
        CodexAuthEdit::None
    };

    state.clear_auth_patch();
    Ok(edit)
}

fn auth_baseline_for_patch(state: &CodexSwitchState) -> Result<(bool, Option<String>)> {
    let auth_path = codex_auth_path();
    let current_text = if auth_path.exists() {
        Some(read_config_text(&auth_path)?)
    } else {
        None
    };

    if let Some(patched_auth_json) = state.patched_auth_json.as_deref()
        && auth_json_matches_helper_patch(current_text.as_deref(), patched_auth_json)
    {
        return Ok((
            state.original_auth_json_absent,
            state.original_auth_json.clone(),
        ));
    }

    Ok((current_text.is_none(), current_text))
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

fn switch_on_codex_toml_with_mode(text: &str, port: u16, mode: CodexPatchMode) -> Result<String> {
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
    match mode {
        CodexPatchMode::Default | CodexPatchMode::ImagegenBridge => {
            proxy_table.remove("requires_openai_auth");
            proxy_table.remove("supports_websockets");
        }
        CodexPatchMode::ChatGptBridge => {
            proxy_table.insert("requires_openai_auth", editable_toml_value(true));
            proxy_table.insert("supports_websockets", editable_toml_value(false));
        }
    }

    set_toml_string(root, "model_provider", "codex_proxy");
    Ok(doc.to_string())
}

fn json_string_at_path<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str().filter(|text| !text.trim().is_empty())
}

fn decode_jwt_payload(jwt: &str) -> Result<serde_json::Value> {
    use base64::Engine as _;

    let mut parts = jwt.split('.');
    let (_header, payload, _signature) = match (parts.next(), parts.next(), parts.next()) {
        (Some(header), Some(payload), Some(signature))
            if !header.is_empty() && !payload.is_empty() && !signature.is_empty() =>
        {
            (header, payload, signature)
        }
        _ => return Err(anyhow!("invalid JWT format")),
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload))
        .context("decode JWT payload")?;
    serde_json::from_slice(&bytes).context("parse JWT payload JSON")
}

fn chatgpt_bridge_auth_requirements_missing(
    value: &serde_json::Value,
) -> Result<Vec<&'static str>> {
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("Codex auth.json root must be a JSON object"))?;
    let tokens = obj.get("tokens").and_then(serde_json::Value::as_object);
    let mut missing = Vec::new();

    let id_token = match tokens.and_then(|tokens| tokens.get("id_token")) {
        Some(value) => value.as_str().filter(|text| !text.trim().is_empty()),
        None => None,
    };
    let id_token_payload = match id_token {
        Some(id_token) => Some(decode_jwt_payload(id_token).context("decode tokens.id_token")?),
        None => {
            missing.push("tokens.id_token");
            None
        }
    };

    if tokens
        .and_then(|tokens| tokens.get("access_token"))
        .and_then(serde_json::Value::as_str)
        .is_none_or(|text| text.trim().is_empty())
    {
        missing.push("tokens.access_token");
    }
    if tokens
        .and_then(|tokens| tokens.get("refresh_token"))
        .and_then(serde_json::Value::as_str)
        .is_none_or(|text| text.trim().is_empty())
    {
        missing.push("tokens.refresh_token");
    }
    if obj
        .get("last_refresh")
        .is_none_or(serde_json::Value::is_null)
    {
        missing.push("last_refresh");
    }

    if let Some(payload) = id_token_payload.as_ref() {
        let has_email = json_string_at_path(payload, &["email"])
            .or_else(|| json_string_at_path(payload, &["https://api.openai.com/profile", "email"]))
            .is_some();
        if !has_email {
            missing.push("tokens.id_token.email");
        }

        let has_account_id = tokens
            .and_then(|tokens| tokens.get("account_id"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|text| !text.trim().is_empty())
            || json_string_at_path(
                payload,
                &["https://api.openai.com/auth", "chatgpt_account_id"],
            )
            .is_some();
        if !has_account_id {
            missing.push("tokens.account_id or tokens.id_token.chatgpt_account_id");
        }
    }

    Ok(missing)
}

fn ensure_chatgpt_bridge_auth_ready(value: &serde_json::Value) -> Result<()> {
    let missing = chatgpt_bridge_auth_requirements_missing(value)?;
    if missing.is_empty() {
        return Ok(());
    }

    Err(anyhow!(
        "Codex auth.json does not contain a complete ChatGPT login state required for chatgpt-bridge (missing: {}). Open Codex and sign in with ChatGPT first, then run `codex-helper switch on --mode chatgpt-bridge` again.",
        missing.join(", ")
    ))
}

fn chatgpt_bridge_auth_json_value(mut value: serde_json::Value) -> Result<serde_json::Value> {
    ensure_chatgpt_bridge_auth_ready(&value)?;
    let obj = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("Codex auth.json root must be a JSON object"))?;
    obj.insert(
        "auth_mode".to_string(),
        serde_json::Value::String("chatgpt".to_string()),
    );
    obj.insert("OPENAI_API_KEY".to_string(), serde_json::Value::Null);
    Ok(value)
}

fn chatgpt_bridge_auth_json_text(text: &str) -> Result<String> {
    let mut value: serde_json::Value =
        serde_json::from_str(text).context("parse Codex auth.json as JSON")?;
    value = chatgpt_bridge_auth_json_value(value)?;
    Ok(serde_json::to_string_pretty(&value)?)
}

fn imagegen_bridge_auth_json_text() -> Result<String> {
    Ok(serde_json::to_string_pretty(&serde_json::json!({}))?)
}

fn prepare_chatgpt_bridge_auth_patch_from_baseline(
    original_absent: bool,
    original_text: Option<String>,
) -> Result<CodexAuthPatch> {
    let auth_path = codex_auth_path();
    let Some(original_text) = original_text else {
        return Err(anyhow!(
            "Codex auth.json not found at {:?}; run `codex login` first, then enable chatgpt-bridge mode.",
            auth_path
        ));
    };
    if original_absent {
        return Err(anyhow!(
            "Codex auth.json not found at {:?}; run `codex login` first, then enable chatgpt-bridge mode.",
            auth_path
        ));
    }
    let patched_text = chatgpt_bridge_auth_json_text(&original_text)?;
    Ok(CodexAuthPatch {
        original_absent: false,
        original_text: Some(original_text),
        patched_text,
    })
}

fn prepare_imagegen_bridge_auth_patch_from_baseline(
    original_absent: bool,
    original_text: Option<String>,
) -> Result<CodexAuthPatch> {
    Ok(CodexAuthPatch {
        original_absent,
        original_text,
        patched_text: imagegen_bridge_auth_json_text()?,
    })
}

fn auth_env_is_set(env_name: &str) -> bool {
    let env_name = env_name.trim();
    !env_name.is_empty() && std::env::var(env_name).is_ok_and(|value| !value.trim().is_empty())
}

fn upstream_has_resolved_auth(upstream: &crate::config::UpstreamConfig) -> bool {
    upstream
        .auth
        .auth_token
        .as_deref()
        .is_some_and(|token| !token.trim().is_empty())
        || upstream
            .auth
            .api_key
            .as_deref()
            .is_some_and(|key| !key.trim().is_empty())
        || upstream
            .auth
            .auth_token_env
            .as_deref()
            .is_some_and(auth_env_is_set)
        || upstream
            .auth
            .api_key_env
            .as_deref()
            .is_some_and(auth_env_is_set)
}

fn upstream_auth_env_names(upstream: &crate::config::UpstreamConfig) -> impl Iterator<Item = &str> {
    upstream
        .auth
        .auth_token_env
        .as_deref()
        .into_iter()
        .chain(upstream.auth.api_key_env.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn config_toml_schema_version_or_shape(text: &str) -> Option<u32> {
    let value = toml::from_str::<toml::Value>(text).ok()?;
    if let Some(version) = value
        .get("version")
        .and_then(|value| value.as_integer())
        .map(|value| value as u32)
    {
        return Some(version);
    }

    let has_v4_routing = ["codex", "claude"].iter().any(|service| {
        value
            .get(*service)
            .and_then(|service| service.get("routing"))
            .and_then(|routing| routing.get("entry").or_else(|| routing.get("routes")))
            .is_some()
    });
    if has_v4_routing {
        return Some(4);
    }

    let has_legacy_routing = ["codex", "claude"].iter().any(|service| {
        value
            .get(*service)
            .and_then(|service| service.get("routing"))
            .is_some()
    });
    if has_legacy_routing { Some(3) } else { None }
}

fn load_runtime_config_for_imagegen_bridge_check() -> Result<ProxyConfig> {
    let path = crate::config::config_file_path();
    if !path.exists() {
        return Ok(ProxyConfig::default());
    }

    let text = read_config_text(&path).with_context(|| format!("read {:?}", path))?;
    if path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
        let version = config_toml_schema_version_or_shape(&text);
        if version.is_some_and(crate::config::is_supported_route_graph_config_version) {
            let cfg = toml::from_str::<ProxyConfigV4>(&text)
                .with_context(|| format!("parse {:?} as route graph config", path))?;
            return crate::config::compile_v4_to_runtime(&cfg);
        }
        if version == Some(3) {
            let cfg = toml::from_str::<crate::config::legacy::ProxyConfigV3Legacy>(&text)
                .with_context(|| format!("parse {:?} as legacy route config", path))?;
            let migrated = crate::config::legacy::migrate_v3_legacy_to_v4(&cfg)?;
            return crate::config::compile_v4_to_runtime(&migrated.config);
        }
        if version == Some(2) {
            let cfg = toml::from_str::<ProxyConfigV2>(&text)
                .with_context(|| format!("parse {:?} as v2 config", path))?;
            return crate::config::compile_v2_to_runtime(&cfg);
        }
        return toml::from_str::<ProxyConfig>(&text)
            .with_context(|| format!("parse {:?} as runtime config", path));
    }

    serde_json::from_str::<ProxyConfig>(&text).with_context(|| format!("parse {:?}", path))
}

fn ensure_imagegen_bridge_runtime_ready() -> Result<()> {
    let cfg = load_runtime_config_for_imagegen_bridge_check()
        .context("load codex-helper config before enabling imagegen-bridge")?;
    let mut routable_upstreams = 0usize;
    let mut authed_upstreams = 0usize;
    let mut missing_env = BTreeSet::new();
    for station in cfg
        .codex
        .stations()
        .values()
        .filter(|station| station.enabled)
    {
        for upstream in &station.upstreams {
            routable_upstreams += 1;
            if upstream_has_resolved_auth(upstream) {
                authed_upstreams += 1;
            } else {
                for env_name in upstream_auth_env_names(upstream) {
                    missing_env.insert(env_name.to_string());
                }
            }
        }
    }

    if routable_upstreams == 0 {
        anyhow::bail!(
            "imagegen-bridge requires at least one enabled Codex upstream in codex-helper config; run `codex-helper config init` or add a [codex.providers.*] entry first"
        );
    }
    if authed_upstreams == 0 {
        if missing_env.is_empty() {
            anyhow::bail!(
                "imagegen-bridge strips Codex client auth, but no enabled Codex upstream has auth_token/auth_token_env/api_key/api_key_env configured; configure an upstream credential before enabling it"
            );
        }
        anyhow::bail!(
            "imagegen-bridge strips Codex client auth, but no enabled Codex upstream credential is available in this process; set one of these env vars first: {}",
            missing_env.into_iter().collect::<Vec<_>>().join(", ")
        );
    }

    Ok(())
}

pub fn patch_codex_auth_for_chatgpt_bridge() -> Result<()> {
    let auth_path = codex_auth_path();
    if !auth_path.exists() {
        return Err(anyhow!(
            "Codex auth.json not found at {:?}; run `codex login` first, then enable chatgpt-bridge mode.",
            auth_path
        ));
    }
    let text = read_config_text(&auth_path)?;
    let new_text = chatgpt_bridge_auth_json_text(&text)?;
    atomic_write(&auth_path, &new_text)?;
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
    /// Current codex-helper Codex patch mode inferred from `model_providers.codex_proxy`.
    pub patch_mode: Option<CodexPatchMode>,
    /// Current `model_providers.codex_proxy.requires_openai_auth` value (if any).
    pub requires_openai_auth: Option<bool>,
    /// Current `model_providers.codex_proxy.supports_websockets` value (if any).
    pub supports_websockets: Option<bool>,
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
            patch_mode: None,
            requires_openai_auth: None,
            supports_websockets: None,
            has_switch_state: state_path.exists(),
        });
    }

    let text = read_config_text(&cfg_path)?;
    if text.trim().is_empty() {
        return Ok(CodexSwitchStatus {
            enabled: false,
            model_provider: None,
            base_url: None,
            patch_mode: None,
            requires_openai_auth: None,
            supports_websockets: None,
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
                patch_mode: None,
                requires_openai_auth: None,
                supports_websockets: None,
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
                patch_mode: None,
                requires_openai_auth: None,
                supports_websockets: None,
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
            patch_mode: None,
            requires_openai_auth: None,
            supports_websockets: None,
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
    let requires_openai_auth = proxy_table
        .get("requires_openai_auth")
        .and_then(|v| v.as_bool());
    let supports_websockets = proxy_table
        .get("supports_websockets")
        .and_then(|v| v.as_bool());

    let is_local = base_url
        .as_deref()
        .is_some_and(|u| u.contains("127.0.0.1") || u.contains("localhost"));
    let is_helper_name = name == "codex-helper";
    let enabled = is_local || is_helper_name;

    Ok(CodexSwitchStatus {
        enabled,
        model_provider,
        base_url,
        patch_mode: enabled.then_some(
            read_codex_switch_state()?
                .and_then(|state| state.patch_mode)
                .unwrap_or(if requires_openai_auth == Some(true) {
                    CodexPatchMode::ChatGptBridge
                } else {
                    CodexPatchMode::Default
                }),
        ),
        requires_openai_auth,
        supports_websockets,
        has_switch_state: state_path.exists(),
    })
}

/// Switch Codex to use the local codex-helper model provider.
pub fn switch_on(port: u16) -> Result<()> {
    switch_on_with_mode(port, CodexPatchMode::Default)
}

/// Switch Codex to use the local codex-helper model provider with an explicit client patch mode.
pub fn switch_on_with_mode(port: u16, mode: CodexPatchMode) -> Result<()> {
    if mode == CodexPatchMode::ImagegenBridge {
        ensure_imagegen_bridge_runtime_ready()?;
    }

    let cfg_path = codex_config_path();
    let state_path = codex_switch_state_path();
    let text = read_config_text(&cfg_path)?;
    if !state_path.exists() && codex_text_points_to_local_helper(&text)? {
        return Err(anyhow!(
            "Codex already points to the local codex-helper proxy, but no switch state was found at {:?}; refusing to treat the local proxy as the original provider. Please inspect ~/.codex/config.toml manually or run `codex-helper switch off` only if a switch state exists.",
            state_path
        ));
    }
    let mut state = if state_path.exists() {
        read_codex_switch_state()?.ok_or_else(|| {
            anyhow!(
                "missing Codex switch state at {:?}",
                codex_switch_state_path()
            )
        })?
    } else {
        CodexSwitchState::from_codex_config_text(&text, !cfg_path.exists())?
    };
    state.patch_mode = Some(mode);

    let auth_edit = match mode {
        CodexPatchMode::Default => auth_restore_edit_from_state(&mut state)?,
        CodexPatchMode::ChatGptBridge | CodexPatchMode::ImagegenBridge => {
            let current_auth = if codex_auth_path().exists() {
                Some(read_config_text(&codex_auth_path())?)
            } else {
                None
            };
            let (original_absent, original_text) = auth_baseline_for_patch(&state)?;
            let patch = match mode {
                CodexPatchMode::ChatGptBridge => {
                    prepare_chatgpt_bridge_auth_patch_from_baseline(original_absent, original_text)?
                }
                CodexPatchMode::ImagegenBridge => prepare_imagegen_bridge_auth_patch_from_baseline(
                    original_absent,
                    original_text,
                )?,
                CodexPatchMode::Default => unreachable!("handled above"),
            };
            let auth_edit = if auth_json_matches_helper_patch(
                current_auth.as_deref(),
                patch.patched_text.as_str(),
            ) {
                CodexAuthEdit::None
            } else {
                CodexAuthEdit::Write(patch.patched_text.clone())
            };
            state.set_auth_patch(&patch);
            auth_edit
        }
    };
    let new_text = switch_on_codex_toml_with_mode(&text, port, mode)?;
    match mode {
        CodexPatchMode::Default => {
            atomic_write(&cfg_path, &new_text)?;
            apply_codex_auth_edit(auth_edit)?;
            write_codex_switch_state(&state)?;
        }
        CodexPatchMode::ChatGptBridge | CodexPatchMode::ImagegenBridge => {
            write_codex_switch_state(&state)?;
            atomic_write(&cfg_path, &new_text)?;
            apply_codex_auth_edit(auth_edit)?;
        }
    }
    Ok(())
}

/// Undo the local Codex proxy patch while preserving config edits made during the run.
pub fn switch_off() -> Result<()> {
    let cfg_path = codex_config_path();
    let state_path = codex_switch_state_path();
    if state_path.exists() {
        let mut state = read_codex_switch_state()?.ok_or_else(|| {
            anyhow!(
                "missing Codex switch state at {:?}",
                codex_switch_state_path()
            )
        })?;
        let auth_edit = auth_restore_edit_from_state(&mut state)?;
        if !cfg_path.exists() {
            apply_codex_auth_edit(auth_edit)?;
            fs::remove_file(&state_path)
                .with_context(|| format!("remove stale switch state {:?}", state_path))?;
            return Ok(());
        }
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
        apply_codex_auth_edit(auth_edit)?;
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
    use base64::Engine as _;
    use serde_json::json;
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

        unsafe fn set_str(&mut self, key: &str, value: &str) {
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
        let proxy_home = root.join(".codex-helper");
        std::fs::create_dir_all(&codex_home).expect("create temp codex home");
        std::fs::create_dir_all(&claude_home).expect("create temp claude home");
        std::fs::create_dir_all(&proxy_home).expect("create temp proxy home");

        let mut scoped = ScopedEnv::new();
        unsafe {
            scoped.set_path("CODEX_HOME", &codex_home);
            scoped.set_path("CLAUDE_HOME", &claude_home);
            scoped.set_path("CODEX_HELPER_HOME", &proxy_home);
            scoped.set_path("HOME", &root);
            scoped.set_path("USERPROFILE", &root);
            scoped.set_str("CODEX_HELPER_IMAGEGEN_TEST_KEY", "sk-relay-test");
            scoped.set_str("CODEX_HELPER_MISSING_IMAGEGEN_KEY", "");
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

    fn write_helper_codex_config(env: &TestEnv, content: &str) {
        let proxy_home = env.codex_home.parent().unwrap().join(".codex-helper");
        write_file(&proxy_home.join("config.toml"), content.trim_start());
    }

    fn write_helper_codex_config_with_env_auth(env: &TestEnv) {
        write_helper_codex_config(
            env,
            r#"
version = 5

[codex.providers.relay]
base_url = "https://relay.example/v1"
auth_token_env = "CODEX_HELPER_IMAGEGEN_TEST_KEY"

[codex.routing]
entry = "main"

[codex.routing.routes.main]
strategy = "ordered-failover"
children = ["relay"]
"#,
        );
    }

    fn fake_chatgpt_jwt(email: &str, account_id: &str, plan_type: &str) -> String {
        let header = json!({
            "alg": "none",
            "typ": "JWT",
        });
        let payload = json!({
            "email": email,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
                "chatgpt_plan_type": plan_type,
            },
        });
        let encode = |value: serde_json::Value| {
            let bytes = serde_json::to_vec(&value).expect("serialize JWT part");
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
        };
        format!("{}.{}.sig", encode(header), encode(payload))
    }

    fn chatgpt_auth_json(email: &str, account_id: &str, plan_type: &str) -> String {
        let id_token = fake_chatgpt_jwt(email, account_id, plan_type);
        serde_json::to_string_pretty(&json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": "sk-platform-onboarding",
            "tokens": {
                "id_token": id_token,
                "access_token": "chatgpt-access-token",
                "refresh_token": "chatgpt-refresh-token",
                "account_id": account_id,
            },
            "last_refresh": "2026-05-17T00:00:00Z",
        }))
        .expect("serialize chatgpt auth fixture")
    }

    fn chatgpt_auth_json_without_plan(email: &str, account_id: &str) -> String {
        let header = json!({
            "alg": "none",
            "typ": "JWT",
        });
        let payload = json!({
            "email": email,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
            },
        });
        let encode = |value: serde_json::Value| {
            let bytes = serde_json::to_vec(&value).expect("serialize JWT part");
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
        };
        let id_token = format!("{}.{}.sig", encode(header), encode(payload));
        serde_json::to_string_pretty(&json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": id_token,
                "access_token": "chatgpt-access-token",
                "refresh_token": "chatgpt-refresh-token",
                "account_id": account_id,
            },
            "last_refresh": "2026-05-17T00:00:00Z",
        }))
        .expect("serialize chatgpt auth fixture")
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

        let updated = switch_on_codex_toml_with_mode(text, 3333, CodexPatchMode::Default)
            .expect("switch_on should update the local proxy provider in place");

        assert!(updated.contains("request_max_retries = 5"));
        assert!(updated.contains("base_url = \"http://127.0.0.1:3333\""));
        assert!(updated.contains("name = \"codex-helper\""));
    }

    #[test]
    fn codex_switch_on_chatgpt_bridge_sets_openai_auth_flags() {
        let updated = switch_on_codex_toml_with_mode("", 3333, CodexPatchMode::ChatGptBridge)
            .expect("switch_on should write chatgpt bridge fields");

        assert!(updated.contains("model_provider = \"codex_proxy\""));
        assert!(updated.contains("base_url = \"http://127.0.0.1:3333\""));
        assert!(updated.contains("requires_openai_auth = true"));
        assert!(updated.contains("supports_websockets = false"));
    }

    #[test]
    fn codex_switch_on_imagegen_bridge_uses_default_proxy_flags() {
        let updated = switch_on_codex_toml_with_mode("", 3333, CodexPatchMode::ImagegenBridge)
            .expect("switch_on should write imagegen bridge fields");

        assert!(updated.contains("model_provider = \"codex_proxy\""));
        assert!(updated.contains("base_url = \"http://127.0.0.1:3333\""));
        assert!(!updated.contains("requires_openai_auth"));
        assert!(!updated.contains("supports_websockets"));
    }

    #[test]
    fn codex_switch_on_default_removes_bridge_only_flags() {
        let text = r#"
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "codex-helper"
base_url = "http://127.0.0.1:1111"
wire_api = "responses"
requires_openai_auth = true
supports_websockets = false
"#;

        let updated = switch_on_codex_toml_with_mode(text, 3333, CodexPatchMode::Default)
            .expect("switch_on should switch local proxy back to default mode");

        assert!(updated.contains("base_url = \"http://127.0.0.1:3333\""));
        assert!(!updated.contains("requires_openai_auth"));
        assert!(!updated.contains("supports_websockets"));
    }

    #[test]
    fn chatgpt_bridge_auth_patch_preserves_other_auth_json_fields() {
        let mut input: serde_json::Value =
            serde_json::from_str(&chatgpt_auth_json("user@example.com", "account-1", "plus"))
                .expect("valid fixture");
        input["last_refresh"] = json!(123);
        input["unrelated"] = json!("keep");

        let updated = chatgpt_bridge_auth_json_text(&serde_json::to_string_pretty(&input).unwrap())
            .expect("auth json patch should preserve unrelated fields");
        let value: serde_json::Value = serde_json::from_str(&updated).expect("valid json");
        let object = value.as_object().expect("root object");

        assert_eq!(
            object.get("auth_mode").and_then(|value| value.as_str()),
            Some("chatgpt")
        );
        assert!(
            object
                .get("OPENAI_API_KEY")
                .is_some_and(|value| value.is_null())
        );
        assert_eq!(
            object
                .get("tokens")
                .and_then(|value| value.get("access_token"))
                .and_then(|value| value.as_str()),
            Some("chatgpt-access-token")
        );
        assert_eq!(
            object
                .get("tokens")
                .and_then(|value| value.get("account_id"))
                .and_then(|value| value.as_str()),
            Some("account-1")
        );
        assert_eq!(
            object.get("last_refresh").and_then(|value| value.as_i64()),
            Some(123)
        );
        assert_eq!(
            object.get("unrelated").and_then(|value| value.as_str()),
            Some("keep")
        );
    }

    #[test]
    fn chatgpt_bridge_auth_patch_rejects_incomplete_login_state() {
        let err = chatgpt_bridge_auth_json_text(r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null}"#)
            .expect_err("empty ChatGPT auth state should be rejected");

        let message = err.to_string();
        assert!(message.contains("complete ChatGPT login state"));
        assert!(message.contains("tokens.id_token"));
        assert!(message.contains("tokens.access_token"));
        assert!(message.contains("last_refresh"));
    }

    #[test]
    fn chatgpt_bridge_auth_patch_rejects_api_key_only_auth() {
        let err = chatgpt_bridge_auth_json_text(
            r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-old","account_id":"acct_1"}"#,
        )
        .expect_err("API key auth should not be converted into fake ChatGPT auth");

        assert!(
            err.to_string()
                .contains("Open Codex and sign in with ChatGPT first")
        );
    }

    #[test]
    fn chatgpt_bridge_auth_patch_accepts_chatgpt_auth_without_plan_claim() {
        let input = chatgpt_auth_json_without_plan("user@example.com", "acct_1");

        let updated = chatgpt_bridge_auth_json_text(&input)
            .expect("Codex maps missing ChatGPT plan claims to unknown");
        let value: serde_json::Value = serde_json::from_str(&updated).expect("valid json");

        assert_eq!(
            value.get("auth_mode").and_then(|value| value.as_str()),
            Some("chatgpt")
        );
        assert!(
            value
                .get("OPENAI_API_KEY")
                .is_some_and(|value| value.is_null())
        );
    }

    #[test]
    fn imagegen_bridge_auth_patch_writes_empty_chatgpt_facade() {
        let updated = imagegen_bridge_auth_json_text().expect("serialize facade auth");
        let value: serde_json::Value = serde_json::from_str(&updated).expect("valid json");

        assert!(
            value.as_object().is_some_and(serde_json::Map::is_empty),
            "imagegen bridge must rely on Codex's default ChatGPT mode fallback, not an explicit auth_mode field"
        );
    }

    #[test]
    fn helper_auth_patch_match_uses_json_semantics() {
        assert!(auth_json_matches_helper_patch(
            Some(r#"{"auth_mode":"chatgpt"}"#),
            "{\n  \"auth_mode\": \"chatgpt\"\n}"
        ));
        assert!(!auth_json_matches_helper_patch(
            Some(r#"{"auth_mode":"apikey"}"#),
            "{\n  \"auth_mode\": \"chatgpt\"\n}"
        ));
    }

    #[test]
    fn imagegen_bridge_ready_check_rejects_missing_codex_upstreams() {
        let _env = setup_temp_env();

        let err = ensure_imagegen_bridge_runtime_ready()
            .expect_err("imagegen bridge should require codex-helper upstreams");

        assert!(
            err.to_string()
                .contains("requires at least one enabled Codex upstream")
        );
    }

    #[test]
    fn imagegen_bridge_ready_check_rejects_unresolved_upstream_env() {
        let env = setup_temp_env();
        write_helper_codex_config(
            &env,
            r#"
version = 5

[codex.providers.relay]
base_url = "https://relay.example/v1"
auth_token_env = "CODEX_HELPER_MISSING_IMAGEGEN_KEY"

[codex.routing]
entry = "main"

[codex.routing.routes.main]
strategy = "ordered-failover"
children = ["relay"]
"#,
        );

        let err = ensure_imagegen_bridge_runtime_ready()
            .expect_err("imagegen bridge should require resolved upstream auth");

        let message = err.to_string();
        assert!(message.contains("no enabled Codex upstream credential is available"));
        assert!(message.contains("CODEX_HELPER_MISSING_IMAGEGEN_KEY"));
    }

    #[test]
    fn codex_switch_on_chatgpt_bridge_patches_auth_json() {
        let env = setup_temp_env();
        let cfg_path = env.codex_home.join("config.toml");
        let auth_path = env.codex_home.join("auth.json");

        write_file(
            &cfg_path,
            r#"
model_provider = "openai"

[model_providers.openai]
name = "openai"
base_url = "https://api.openai.com/v1"
"#
            .trim_start(),
        );
        write_file(
            &auth_path,
            &chatgpt_auth_json("user@example.com", "acct_1", "plus"),
        );

        switch_on_with_mode(3211, CodexPatchMode::ChatGptBridge)
            .expect("switch_on bridge should patch config and auth");

        let updated_cfg = read_file(&cfg_path);
        assert!(updated_cfg.contains("requires_openai_auth = true"));
        assert!(updated_cfg.contains("supports_websockets = false"));

        let updated_auth: serde_json::Value =
            serde_json::from_str(&read_file(&auth_path)).expect("valid auth json");
        assert_eq!(
            updated_auth
                .get("auth_mode")
                .and_then(|value| value.as_str()),
            Some("chatgpt")
        );
        assert!(
            updated_auth
                .get("OPENAI_API_KEY")
                .is_some_and(|value| value.is_null())
        );
        assert_eq!(
            updated_auth
                .get("tokens")
                .and_then(|value| value.get("account_id"))
                .and_then(|value| value.as_str()),
            Some("acct_1")
        );
    }

    #[test]
    fn codex_switch_on_imagegen_bridge_patches_auth_json_and_records_mode() {
        let env = setup_temp_env();
        write_helper_codex_config_with_env_auth(&env);
        let cfg_path = env.codex_home.join("config.toml");
        let auth_path = env.codex_home.join("auth.json");
        let state_path = env.codex_home.join("codex-helper-switch-state.json");
        let original_auth = chatgpt_auth_json("user@example.com", "acct_1", "plus");

        write_file(
            &cfg_path,
            r#"
model_provider = "openai"

[model_providers.openai]
name = "openai"
base_url = "https://api.openai.com/v1"
"#
            .trim_start(),
        );
        write_file(&auth_path, &original_auth);

        switch_on_with_mode(3211, CodexPatchMode::ImagegenBridge)
            .expect("switch_on imagegen bridge should patch config and auth");

        let updated_cfg = read_file(&cfg_path);
        assert!(updated_cfg.contains("model_provider = \"codex_proxy\""));
        assert!(!updated_cfg.contains("requires_openai_auth"));

        let updated_auth: serde_json::Value =
            serde_json::from_str(&read_file(&auth_path)).expect("valid auth json");
        assert!(
            updated_auth
                .as_object()
                .is_some_and(serde_json::Map::is_empty)
        );

        let state_text = read_file(&state_path);
        let state: serde_json::Value = serde_json::from_str(&state_text).expect("valid state");
        assert_eq!(
            state.get("patch_mode").and_then(|value| value.as_str()),
            Some("imagegen-bridge")
        );
        assert_eq!(
            state
                .get("original_auth_json")
                .and_then(|value| value.as_str()),
            Some(original_auth.as_str())
        );
        assert_eq!(
            state
                .get("patched_auth_json")
                .and_then(|value| value.as_str()),
            Some("{}")
        );
    }

    #[test]
    fn codex_switch_default_restores_imagegen_bridge_auth_json() {
        let env = setup_temp_env();
        write_helper_codex_config_with_env_auth(&env);
        let cfg_path = env.codex_home.join("config.toml");
        let auth_path = env.codex_home.join("auth.json");
        let original_auth = chatgpt_auth_json("user@example.com", "acct_1", "plus");

        write_file(
            &cfg_path,
            r#"
model_provider = "openai"

[model_providers.openai]
name = "openai"
base_url = "https://api.openai.com/v1"
"#
            .trim_start(),
        );
        write_file(&auth_path, &original_auth);

        switch_on_with_mode(3211, CodexPatchMode::ImagegenBridge)
            .expect("switch_on imagegen bridge should patch auth");
        assert_ne!(read_file(&auth_path), original_auth);

        switch_on_with_mode(3211, CodexPatchMode::Default)
            .expect("switching to default should restore auth");

        assert_eq!(read_file(&auth_path), original_auth);
        let status = codex_switch_status().expect("status should load");
        assert_eq!(status.patch_mode, Some(CodexPatchMode::Default));
    }

    #[test]
    fn codex_switch_off_restores_imagegen_bridge_auth_json() {
        let env = setup_temp_env();
        write_helper_codex_config_with_env_auth(&env);
        let cfg_path = env.codex_home.join("config.toml");
        let auth_path = env.codex_home.join("auth.json");
        let original_auth = chatgpt_auth_json("user@example.com", "acct_1", "plus");

        write_file(
            &cfg_path,
            r#"
model_provider = "openai"

[model_providers.openai]
name = "openai"
base_url = "https://api.openai.com/v1"
"#
            .trim_start(),
        );
        write_file(&auth_path, &original_auth);

        switch_on_with_mode(3211, CodexPatchMode::ImagegenBridge)
            .expect("switch_on imagegen bridge should patch auth");
        switch_off().expect("switch_off should restore auth");

        assert_eq!(read_file(&auth_path), original_auth);
        assert!(read_file(&cfg_path).contains("model_provider = \"openai\""));
    }

    #[test]
    fn codex_switch_off_does_not_restore_auth_if_user_changed_it() {
        let env = setup_temp_env();
        write_helper_codex_config_with_env_auth(&env);
        let cfg_path = env.codex_home.join("config.toml");
        let auth_path = env.codex_home.join("auth.json");
        let user_auth = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-user"}"#;

        write_file(
            &cfg_path,
            r#"
model_provider = "openai"

[model_providers.openai]
name = "openai"
base_url = "https://api.openai.com/v1"
"#
            .trim_start(),
        );
        write_file(
            &auth_path,
            &chatgpt_auth_json("user@example.com", "acct_1", "plus"),
        );

        switch_on_with_mode(3211, CodexPatchMode::ImagegenBridge)
            .expect("switch_on imagegen bridge should patch auth");
        write_file(&auth_path, user_auth);
        switch_off().expect("switch_off should leave user auth alone");

        assert_eq!(read_file(&auth_path), user_auth);
    }

    #[test]
    fn codex_switch_imagegen_to_chatgpt_bridge_uses_original_auth_json() {
        let env = setup_temp_env();
        write_helper_codex_config_with_env_auth(&env);
        let cfg_path = env.codex_home.join("config.toml");
        let auth_path = env.codex_home.join("auth.json");
        let original_auth = chatgpt_auth_json("user@example.com", "acct_1", "plus");

        write_file(
            &cfg_path,
            r#"
model_provider = "openai"

[model_providers.openai]
name = "openai"
base_url = "https://api.openai.com/v1"
"#
            .trim_start(),
        );
        write_file(&auth_path, &original_auth);

        switch_on_with_mode(3211, CodexPatchMode::ImagegenBridge)
            .expect("switch_on imagegen bridge should patch auth");
        switch_on_with_mode(3211, CodexPatchMode::ChatGptBridge)
            .expect("switching to chatgpt bridge should use original auth snapshot");

        let updated_auth: serde_json::Value =
            serde_json::from_str(&read_file(&auth_path)).expect("valid auth json");
        assert_eq!(
            updated_auth
                .get("tokens")
                .and_then(|value| value.get("account_id"))
                .and_then(|value| value.as_str()),
            Some("acct_1")
        );
        assert!(
            updated_auth
                .get("OPENAI_API_KEY")
                .is_some_and(|value| value.is_null())
        );
    }

    #[test]
    fn codex_switch_on_chatgpt_bridge_does_not_rewrite_already_patched_auth_json() {
        let env = setup_temp_env();
        let cfg_path = env.codex_home.join("config.toml");
        let auth_path = env.codex_home.join("auth.json");

        write_file(
            &cfg_path,
            r#"
model_provider = "openai"

[model_providers.openai]
name = "openai"
base_url = "https://api.openai.com/v1"
"#
            .trim_start(),
        );
        write_file(
            &auth_path,
            &chatgpt_bridge_auth_json_text(&chatgpt_auth_json(
                "user@example.com",
                "acct_1",
                "plus",
            ))
            .expect("pre-patch auth fixture"),
        );
        let before = std::fs::metadata(&auth_path)
            .expect("auth metadata")
            .modified()
            .expect("auth modified time");

        switch_on_with_mode(3211, CodexPatchMode::ChatGptBridge)
            .expect("switch_on bridge should patch config without rewriting already-patched auth");

        let after = std::fs::metadata(&auth_path)
            .expect("auth metadata")
            .modified()
            .expect("auth modified time");
        assert_eq!(before, after);
    }

    #[test]
    fn codex_switch_on_chatgpt_bridge_refuses_incomplete_auth_without_writing_config() {
        let env = setup_temp_env();
        let cfg_path = env.codex_home.join("config.toml");
        let auth_path = env.codex_home.join("auth.json");
        let state_path = env.codex_home.join("codex-helper-switch-state.json");
        let original_config = r#"
model_provider = "openai"

[model_providers.openai]
name = "openai"
base_url = "https://api.openai.com/v1"
"#
        .trim_start();
        let original_auth = r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null}"#;

        write_file(&cfg_path, original_config);
        write_file(&auth_path, original_auth);

        let err = switch_on_with_mode(3211, CodexPatchMode::ChatGptBridge)
            .expect_err("incomplete ChatGPT auth should be rejected before writing config");

        assert!(err.to_string().contains("complete ChatGPT login state"));
        assert_eq!(read_file(&cfg_path), original_config);
        assert_eq!(read_file(&auth_path), original_auth);
        assert!(
            !state_path.exists(),
            "failed bridge switch must not create switch state"
        );
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

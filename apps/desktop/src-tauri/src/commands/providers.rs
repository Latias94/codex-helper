use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use codex_helper_core::config::{ProxyConfigV4, config_file_path, proxy_home_dir};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use toml::Value;
use toml::map::Map;

use crate::error::{CommandError, DesktopError};

const CONFIG_FILE_NAME: &str = "config.toml";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCommonEditPayload {
    #[serde(default = "default_service")]
    pub service: String,
    pub provider_name: String,
    pub alias: Option<String>,
    pub base_url: String,
    #[serde(default, deserialize_with = "deserialize_optional_string_patch")]
    pub continuity_domain: OptionalStringPatch,
    pub enabled: bool,
    pub auth_token_env: Option<String>,
    pub api_key_env: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum OptionalStringPatch {
    #[default]
    Preserve,
    Set(Option<String>),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfigEditResult {
    pub ok: bool,
    pub action: &'static str,
    pub message: String,
    pub service: String,
    pub provider_name: String,
    pub config: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
    pub reload_required: bool,
    pub advanced_fields_preserved: bool,
}

#[tauri::command]
pub fn save_common_provider(
    payload: ProviderCommonEditPayload,
) -> Result<ProviderConfigEditResult, CommandError> {
    edit_provider_config_file(config_path(), payload)
}

fn edit_provider_config_file(
    path: PathBuf,
    payload: ProviderCommonEditPayload,
) -> Result<ProviderConfigEditResult, CommandError> {
    validate_payload(&payload)?;
    if !path.exists() {
        return Err(
            DesktopError::Config(format!("config file does not exist at {:?}", path)).into(),
        );
    }

    let original = fs::read_to_string(&path)
        .map_err(|err| DesktopError::Config(format!("read config {:?}: {err}", path)))?;
    let updated = apply_common_provider_edit_to_text(&original, &payload)?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            DesktopError::Config(format!("create config directory {:?}: {err}", parent))
        })?;
    }

    let backup = backup_path_for(&path);
    fs::copy(&path, &backup).map_err(|err| {
        DesktopError::Config(format!(
            "backup current config {:?} to {:?}: {err}",
            path, backup
        ))
    })?;
    fs::write(&path, updated)
        .map_err(|err| DesktopError::Config(format!("write config {:?}: {err}", path)))?;

    Ok(ProviderConfigEditResult {
        ok: true,
        action: "edit-provider",
        message: format!(
            "已更新 provider {} 的常用字段；高级字段已保留。如代理正在运行，请重新加载运行时配置。",
            payload.provider_name.trim()
        ),
        service: payload.service.trim().to_string(),
        provider_name: payload.provider_name.trim().to_string(),
        config: path,
        backup: Some(backup),
        reload_required: true,
        advanced_fields_preserved: true,
    })
}

fn apply_common_provider_edit_to_text(
    text: &str,
    payload: &ProviderCommonEditPayload,
) -> Result<String, CommandError> {
    validate_payload(payload)?;
    let mut value = toml::from_str::<Value>(text)
        .map_err(|err| DesktopError::Config(format!("config.toml is invalid TOML: {err}")))?;
    ensure_v5_config(&value)?;

    let service = payload.service.trim();
    let provider_name = payload.provider_name.trim();
    let root = value
        .as_table_mut()
        .ok_or_else(|| DesktopError::Config("config.toml root must be a TOML table".to_string()))?;
    let provider_table = provider_table_mut(root, service, provider_name)?;
    patch_single_endpoint_provider(provider_table, payload)?;

    let updated = toml::to_string_pretty(&value)
        .map_err(|err| DesktopError::Config(format!("serialize updated config.toml: {err}")))?;
    toml::from_str::<ProxyConfigV4>(&updated).map_err(|err| {
        DesktopError::Config(format!(
            "updated config.toml is not a valid codex-helper v5 config: {err}"
        ))
    })?;
    Ok(updated)
}

fn patch_single_endpoint_provider(
    provider_table: &mut Map<String, Value>,
    payload: &ProviderCommonEditPayload,
) -> Result<(), CommandError> {
    let base_url = payload.base_url.trim();
    let provider_has_base_url = provider_base_url_state(provider_table)?;
    let endpoint_names = endpoint_names(provider_table)?;

    match (provider_has_base_url, endpoint_names.as_slice()) {
        (true, []) => {
            provider_table.insert("base_url".to_string(), Value::String(base_url.to_string()));
            patch_optional_string(
                provider_table,
                "continuity_domain",
                &payload.continuity_domain,
            );
        }
        (true, _) => {
            return Err(DesktopError::Config(
                "provider 同时包含 base_url 和 endpoints，属于高级结构；请使用 raw TOML 编辑"
                    .to_string(),
            )
            .into());
        }
        (false, [endpoint_name]) => {
            let endpoints = provider_table
                .get_mut("endpoints")
                .and_then(Value::as_table_mut)
                .ok_or_else(|| {
                    DesktopError::Config("provider endpoints must be a table".to_string())
                })?;
            let endpoint = endpoints
                .get_mut(endpoint_name)
                .and_then(Value::as_table_mut)
                .ok_or_else(|| {
                    DesktopError::Config(format!("endpoint {endpoint_name} must be a table"))
                })?;
            endpoint.insert("base_url".to_string(), Value::String(base_url.to_string()));
            patch_optional_string(endpoint, "continuity_domain", &payload.continuity_domain);
        }
        (false, []) => {
            return Err(DesktopError::Config(
                "provider 没有可安全编辑的单 endpoint base_url；请使用 raw TOML 编辑".to_string(),
            )
            .into());
        }
        (false, _) => {
            return Err(DesktopError::Config(
                "多 endpoint provider 暂不提供常用表单；高级路由和多端点请使用 raw TOML 编辑"
                    .to_string(),
            )
            .into());
        }
    }

    set_optional_string(provider_table, "alias", payload.alias.as_deref());
    provider_table.insert("enabled".to_string(), Value::Boolean(payload.enabled));
    if let Some(auth_token_env) = payload.auth_token_env.as_deref() {
        set_optional_string(provider_table, "auth_token_env", Some(auth_token_env));
    }
    if let Some(api_key_env) = payload.api_key_env.as_deref() {
        set_optional_string(provider_table, "api_key_env", Some(api_key_env));
    }

    Ok(())
}

fn validate_payload(payload: &ProviderCommonEditPayload) -> Result<(), CommandError> {
    match payload.service.trim() {
        "codex" | "claude" => {}
        other => {
            return Err(DesktopError::Config(format!(
                "unsupported service {other:?}; expected codex or claude"
            ))
            .into());
        }
    }
    if payload.provider_name.trim().is_empty() {
        return Err(DesktopError::Config("providerName is required".to_string()).into());
    }
    let parsed = Url::parse(payload.base_url.trim()).map_err(|err| {
        DesktopError::Config(format!("baseUrl must be an absolute HTTP(S) URL: {err}"))
    })?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(
            DesktopError::Config("baseUrl scheme must be http or https".to_string()).into(),
        );
    }
    Ok(())
}

fn ensure_v5_config(value: &Value) -> Result<(), CommandError> {
    let version = value
        .get("version")
        .and_then(Value::as_integer)
        .and_then(|version| u32::try_from(version).ok());
    if version != Some(5) {
        return Err(DesktopError::Config(
            "provider 常用表单只支持当前 v5 route graph config；旧配置请先迁移或使用 raw TOML"
                .to_string(),
        )
        .into());
    }
    Ok(())
}

fn provider_table_mut<'a>(
    root: &'a mut Map<String, Value>,
    service: &str,
    provider_name: &str,
) -> Result<&'a mut Map<String, Value>, CommandError> {
    root.get_mut(service)
        .and_then(Value::as_table_mut)
        .and_then(|service_table| service_table.get_mut("providers"))
        .and_then(Value::as_table_mut)
        .and_then(|providers| providers.get_mut(provider_name))
        .and_then(Value::as_table_mut)
        .ok_or_else(|| {
            DesktopError::Config(format!(
                "provider {provider_name:?} was not found under [{service}.providers]"
            ))
            .into()
        })
}

fn provider_base_url_state(provider_table: &Map<String, Value>) -> Result<bool, CommandError> {
    match provider_table.get("base_url") {
        None => Ok(false),
        Some(Value::String(_)) => Ok(true),
        Some(_) => {
            Err(DesktopError::Config("provider base_url must be a string".to_string()).into())
        }
    }
}

fn endpoint_names(provider_table: &Map<String, Value>) -> Result<Vec<String>, CommandError> {
    match provider_table.get("endpoints") {
        None => Ok(Vec::new()),
        Some(Value::Table(table)) => Ok(table.keys().cloned().collect()),
        Some(_) => {
            Err(DesktopError::Config("provider endpoints must be a table".to_string()).into())
        }
    }
}

fn set_optional_string(table: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => {
            table.insert(key.to_string(), Value::String(value.to_string()));
        }
        None => {
            table.remove(key);
        }
    }
}

fn patch_optional_string(table: &mut Map<String, Value>, key: &str, patch: &OptionalStringPatch) {
    match patch {
        OptionalStringPatch::Preserve => {}
        OptionalStringPatch::Set(value) => set_optional_string(table, key, value.as_deref()),
    }
}

fn deserialize_optional_string_patch<'de, D>(
    deserializer: D,
) -> Result<OptionalStringPatch, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = JsonValue::deserialize(deserializer)?;
    match value {
        JsonValue::Null => Ok(OptionalStringPatch::Set(None)),
        JsonValue::String(value) => Ok(OptionalStringPatch::Set(Some(value))),
        other => Err(serde::de::Error::custom(format!(
            "expected string or null for optional string patch, got {other}"
        ))),
    }
}

fn config_path() -> PathBuf {
    let path = config_file_path();
    if path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
        path
    } else {
        proxy_home_dir().join(CONFIG_FILE_NAME)
    }
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

fn default_service() -> String {
    "codex".to_string()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use toml::Value;

    use super::{
        OptionalStringPatch, ProviderCommonEditPayload, apply_common_provider_edit_to_text,
        edit_provider_config_file,
    };

    const SINGLE_PROVIDER_CONFIG: &str = r#"
version = 5

[codex.routing]
entry = "relay"

[codex.routing.routes.relay]
strategy = "ordered-failover"
children = ["relay"]

[codex.providers.relay]
alias = "Old Relay"
base_url = "https://old.example/v1"
auth_token_env = "OLD_TOKEN"
enabled = true
supported_models = { "gpt-5.4" = true }

[codex.providers.relay.tags]
region = "us"

[codex.providers.relay.limits]
max_concurrent_requests = 3
"#;

    const SINGLE_PROVIDER_WITH_CONTINUITY_CONFIG: &str = r#"
version = 5

[codex.routing]
entry = "relay"

[codex.routing.routes.relay]
strategy = "ordered-failover"
children = ["relay"]

[codex.providers.relay]
alias = "Old Relay"
base_url = "https://old.example/v1"
continuity_domain = "relay-cluster-existing"
auth_token_env = "OLD_TOKEN"
enabled = true
"#;

    const MULTI_ENDPOINT_CONFIG: &str = r#"
version = 5

[codex.routing]
entry = "relay"

[codex.routing.routes.relay]
strategy = "ordered-failover"
children = ["relay"]

[codex.providers.relay]
enabled = true

[codex.providers.relay.endpoints.primary]
base_url = "https://primary.example/v1"
priority = 0

[codex.providers.relay.endpoints.backup]
base_url = "https://backup.example/v1"
priority = 10
"#;

    const SINGLE_ENDPOINT_TABLE_CONFIG: &str = r#"
version = 5

[codex.routing]
entry = "relay"

[codex.routing.routes.relay]
strategy = "ordered-failover"
children = ["relay"]

[codex.providers.relay]
enabled = true

[codex.providers.relay.endpoints.primary]
base_url = "https://old-endpoint.example/v1"
priority = 0

[codex.providers.relay.endpoints.primary.tags]
region = "eu"
"#;

    #[test]
    fn common_edit_updates_single_provider_and_preserves_advanced_fields() {
        let updated =
            apply_common_provider_edit_to_text(SINGLE_PROVIDER_CONFIG, &payload()).expect("edit");
        let value = toml::from_str::<Value>(&updated).expect("valid toml");
        let provider = value
            .get("codex")
            .and_then(Value::as_table)
            .and_then(|service| service.get("providers"))
            .and_then(Value::as_table)
            .and_then(|providers| providers.get("relay"))
            .and_then(Value::as_table)
            .expect("provider table");

        assert_eq!(
            provider.get("alias").and_then(Value::as_str),
            Some("New Relay")
        );
        assert_eq!(
            provider.get("base_url").and_then(Value::as_str),
            Some("https://new.example/v1")
        );
        assert_eq!(
            provider.get("continuity_domain").and_then(Value::as_str),
            Some("relay-cluster-a")
        );
        assert_eq!(
            provider.get("enabled").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            provider.get("auth_token_env").and_then(Value::as_str),
            Some("NEW_TOKEN")
        );
        assert!(provider.get("api_key_env").is_none());
        assert_eq!(
            provider
                .get("supported_models")
                .and_then(Value::as_table)
                .and_then(|models| models.get("gpt-5.4"))
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            provider
                .get("tags")
                .and_then(Value::as_table)
                .and_then(|tags| tags.get("region"))
                .and_then(Value::as_str),
            Some("us")
        );
        assert_eq!(
            provider
                .get("limits")
                .and_then(Value::as_table)
                .and_then(|limits| limits.get("max_concurrent_requests"))
                .and_then(Value::as_integer),
            Some(3)
        );
    }

    #[test]
    fn common_edit_rejects_multi_endpoint_provider_without_overwriting_file() {
        let dir = unique_temp_dir("provider-common-edit");
        let path = dir.join("config.toml");
        fs::write(&path, MULTI_ENDPOINT_CONFIG).expect("write config");

        let error = edit_provider_config_file(path.clone(), payload()).expect_err("rejects");

        assert!(
            error.message.contains("多 endpoint"),
            "unexpected error: {}",
            error.message
        );
        assert_eq!(
            fs::read_to_string(path).expect("read unchanged config"),
            MULTI_ENDPOINT_CONFIG
        );
    }

    #[test]
    fn common_edit_updates_single_endpoint_table_and_preserves_endpoint_fields() {
        let updated = apply_common_provider_edit_to_text(SINGLE_ENDPOINT_TABLE_CONFIG, &payload())
            .expect("edit");
        let value = toml::from_str::<Value>(&updated).expect("valid toml");
        let endpoint = value
            .get("codex")
            .and_then(Value::as_table)
            .and_then(|service| service.get("providers"))
            .and_then(Value::as_table)
            .and_then(|providers| providers.get("relay"))
            .and_then(Value::as_table)
            .and_then(|provider| provider.get("endpoints"))
            .and_then(Value::as_table)
            .and_then(|endpoints| endpoints.get("primary"))
            .and_then(Value::as_table)
            .expect("endpoint table");

        assert_eq!(
            endpoint.get("base_url").and_then(Value::as_str),
            Some("https://new.example/v1")
        );
        assert_eq!(
            endpoint.get("continuity_domain").and_then(Value::as_str),
            Some("relay-cluster-a")
        );
        assert_eq!(
            endpoint.get("priority").and_then(Value::as_integer),
            Some(0)
        );
        assert_eq!(
            endpoint
                .get("tags")
                .and_then(Value::as_table)
                .and_then(|tags| tags.get("region"))
                .and_then(Value::as_str),
            Some("eu")
        );
    }

    #[test]
    fn common_edit_preserves_continuity_domain_when_payload_field_is_missing() {
        let mut payload = payload();
        payload.continuity_domain = OptionalStringPatch::Preserve;

        let updated =
            apply_common_provider_edit_to_text(SINGLE_PROVIDER_WITH_CONTINUITY_CONFIG, &payload)
                .expect("edit");
        let value = toml::from_str::<Value>(&updated).expect("valid toml");
        let provider = value
            .get("codex")
            .and_then(Value::as_table)
            .and_then(|service| service.get("providers"))
            .and_then(Value::as_table)
            .and_then(|providers| providers.get("relay"))
            .and_then(Value::as_table)
            .expect("provider table");

        assert_eq!(
            provider.get("continuity_domain").and_then(Value::as_str),
            Some("relay-cluster-existing")
        );
        assert_eq!(
            provider.get("base_url").and_then(Value::as_str),
            Some("https://new.example/v1")
        );
    }

    #[test]
    fn common_edit_clears_continuity_domain_when_payload_field_is_blank() {
        let mut payload = payload();
        payload.continuity_domain = OptionalStringPatch::Set(Some(String::new()));

        let updated =
            apply_common_provider_edit_to_text(SINGLE_PROVIDER_WITH_CONTINUITY_CONFIG, &payload)
                .expect("edit");
        let value = toml::from_str::<Value>(&updated).expect("valid toml");
        let provider = value
            .get("codex")
            .and_then(Value::as_table)
            .and_then(|service| service.get("providers"))
            .and_then(Value::as_table)
            .and_then(|providers| providers.get("relay"))
            .and_then(Value::as_table)
            .expect("provider table");

        assert!(provider.get("continuity_domain").is_none());
    }

    #[test]
    fn common_edit_payload_deserializes_continuity_domain_as_three_state_patch() {
        let missing = serde_json::from_value::<ProviderCommonEditPayload>(serde_json::json!({
            "providerName": "relay",
            "baseUrl": "https://new.example/v1",
            "enabled": true
        }))
        .expect("missing field payload");
        assert_eq!(missing.continuity_domain, OptionalStringPatch::Preserve);

        let blank = serde_json::from_value::<ProviderCommonEditPayload>(serde_json::json!({
            "providerName": "relay",
            "baseUrl": "https://new.example/v1",
            "continuityDomain": "",
            "enabled": true
        }))
        .expect("blank field payload");
        assert_eq!(
            blank.continuity_domain,
            OptionalStringPatch::Set(Some(String::new()))
        );

        let null = serde_json::from_value::<ProviderCommonEditPayload>(serde_json::json!({
            "providerName": "relay",
            "baseUrl": "https://new.example/v1",
            "continuityDomain": null,
            "enabled": true
        }))
        .expect("null field payload");
        assert_eq!(null.continuity_domain, OptionalStringPatch::Set(None));
    }

    #[test]
    fn common_edit_rejects_non_http_base_url() {
        let mut payload = payload();
        payload.base_url = "file:///tmp/provider".to_string();

        let error =
            apply_common_provider_edit_to_text(SINGLE_PROVIDER_CONFIG, &payload).expect_err("url");

        assert!(
            error.message.contains("http or https"),
            "unexpected error: {}",
            error.message
        );
    }

    fn payload() -> ProviderCommonEditPayload {
        ProviderCommonEditPayload {
            service: "codex".to_string(),
            provider_name: "relay".to_string(),
            alias: Some("New Relay".to_string()),
            base_url: "https://new.example/v1".to_string(),
            continuity_domain: OptionalStringPatch::Set(Some("relay-cluster-a".to_string())),
            enabled: false,
            auth_token_env: Some("NEW_TOKEN".to_string()),
            api_key_env: Some(String::new()),
        }
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

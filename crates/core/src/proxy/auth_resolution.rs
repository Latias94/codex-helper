use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime};

use crate::config::UpstreamAuth;

use super::AUTH_FILE_CACHE_MIN_CHECK_INTERVAL;

#[derive(Default)]
struct JsonFileCache {
    last_check_at: Option<Instant>,
    last_path: Option<PathBuf>,
    last_mtime: Option<SystemTime>,
    value: Option<serde_json::Value>,
}

fn cached_json_file_value(
    cache: &'static OnceLock<Mutex<JsonFileCache>>,
    path: PathBuf,
) -> Option<serde_json::Value> {
    let cache = cache.get_or_init(|| Mutex::new(JsonFileCache::default()));
    let now = Instant::now();
    let mut state = match cache.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let path_changed = state.last_path.as_ref() != Some(&path);
    let should_check = path_changed
        || state
            .last_check_at
            .map(|last| now.saturating_duration_since(last) >= AUTH_FILE_CACHE_MIN_CHECK_INTERVAL)
            .unwrap_or(true);
    if !should_check {
        return state.value.clone();
    }

    let mtime = std::fs::metadata(&path)
        .ok()
        .and_then(|metadata| metadata.modified().ok());
    if !path_changed && mtime == state.last_mtime {
        state.last_check_at = Some(now);
        return state.value.clone();
    }

    let value = read_json_file(&path);
    state.last_check_at = Some(now);
    state.last_path = Some(path);
    state.last_mtime = mtime;
    state.value = value.clone();
    value
}

fn read_json_file(path: &Path) -> Option<serde_json::Value> {
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    if text.trim().is_empty() {
        return None;
    }
    serde_json::from_str(&text).ok()
}

pub(super) fn codex_auth_json_value(key: &str) -> Option<String> {
    static CACHE: OnceLock<Mutex<JsonFileCache>> = OnceLock::new();
    let value = cached_json_file_value(&CACHE, crate::config::codex_auth_path());
    let object = value.as_ref()?.as_object()?;
    object
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_owned)
}

pub(super) fn claude_settings_env_value(key: &str) -> Option<String> {
    static CACHE: OnceLock<Mutex<JsonFileCache>> = OnceLock::new();
    let value = cached_json_file_value(&CACHE, crate::config::claude_settings_path());
    let object = value.as_ref()?.as_object()?;
    let env_object = object.get("env")?.as_object()?;
    env_object
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_owned)
}

pub(super) fn resolve_auth_token_with_source(
    service_name: &str,
    auth: &UpstreamAuth,
    client_has_auth: bool,
) -> (Option<String>, String) {
    if let Some(token) = auth.auth_token.as_deref()
        && !token.trim().is_empty()
    {
        return (Some(token.to_string()), "inline".to_string());
    }

    if let Some(env_name) = auth.auth_token_env.as_deref()
        && !env_name.trim().is_empty()
    {
        if let Ok(value) = std::env::var(env_name)
            && !value.trim().is_empty()
        {
            return (Some(value), format!("env:{env_name}"));
        }

        let file_value = match service_name {
            "codex" => codex_auth_json_value(env_name),
            "claude" => claude_settings_env_value(env_name),
            _ => None,
        };
        if let Some(value) = file_value
            && !value.trim().is_empty()
        {
            let source = match service_name {
                "codex" => format!("codex_auth_json:{env_name}"),
                "claude" => format!("claude_settings_env:{env_name}"),
                _ => format!("file:{env_name}"),
            };
            return (Some(value), source);
        }

        if client_has_auth {
            return (None, format!("client_passthrough (missing_env:{env_name})"));
        }
        return (None, format!("missing_env:{env_name}"));
    }

    if client_has_auth {
        (None, "client_passthrough".to_string())
    } else {
        (None, "none".to_string())
    }
}

pub(super) fn resolve_api_key_with_source(
    service_name: &str,
    auth: &UpstreamAuth,
    client_has_x_api_key: bool,
) -> (Option<String>, String) {
    if let Some(key) = auth.api_key.as_deref()
        && !key.trim().is_empty()
    {
        return (Some(key.to_string()), "inline".to_string());
    }

    if let Some(env_name) = auth.api_key_env.as_deref()
        && !env_name.trim().is_empty()
    {
        if let Ok(value) = std::env::var(env_name)
            && !value.trim().is_empty()
        {
            return (Some(value), format!("env:{env_name}"));
        }

        let file_value = match service_name {
            "codex" => codex_auth_json_value(env_name),
            "claude" => claude_settings_env_value(env_name),
            _ => None,
        };
        if let Some(value) = file_value
            && !value.trim().is_empty()
        {
            let source = match service_name {
                "codex" => format!("codex_auth_json:{env_name}"),
                "claude" => format!("claude_settings_env:{env_name}"),
                _ => format!("file:{env_name}"),
            };
            return (Some(value), source);
        }

        if client_has_x_api_key {
            return (None, format!("client_passthrough (missing_env:{env_name})"));
        }
        return (None, format!("missing_env:{env_name}"));
    }

    if client_has_x_api_key {
        (None, "client_passthrough".to_string())
    } else {
        (None, "none".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_api_key_with_source, resolve_auth_token_with_source};
    use crate::config::UpstreamAuth;

    #[test]
    fn auth_resolution_prefers_inline_secret() {
        let auth = UpstreamAuth {
            auth_token: Some("token-1".to_string()),
            auth_token_env: Some("UNUSED_AUTH_ENV_FOR_TEST".to_string()),
            api_key: Some("key-1".to_string()),
            api_key_env: Some("UNUSED_API_ENV_FOR_TEST".to_string()),
        };

        assert_eq!(
            resolve_auth_token_with_source("codex", &auth, true),
            (Some("token-1".to_string()), "inline".to_string())
        );
        assert_eq!(
            resolve_api_key_with_source("codex", &auth, true),
            (Some("key-1".to_string()), "inline".to_string())
        );
    }

    #[test]
    fn auth_resolution_falls_back_to_client_passthrough_when_env_missing() {
        let auth = UpstreamAuth {
            auth_token: None,
            auth_token_env: Some("CODEX_HELPER_TEST_MISSING_AUTH_ENV_09A1".to_string()),
            api_key: None,
            api_key_env: Some("CODEX_HELPER_TEST_MISSING_API_ENV_09A1".to_string()),
        };

        assert_eq!(
            resolve_auth_token_with_source("other", &auth, true),
            (
                None,
                "client_passthrough (missing_env:CODEX_HELPER_TEST_MISSING_AUTH_ENV_09A1)"
                    .to_string(),
            )
        );
        assert_eq!(
            resolve_api_key_with_source("other", &auth, true),
            (
                None,
                "client_passthrough (missing_env:CODEX_HELPER_TEST_MISSING_API_ENV_09A1)"
                    .to_string(),
            )
        );
    }

    #[test]
    fn auth_resolution_reports_none_without_client_headers_or_config() {
        let auth = UpstreamAuth::default();

        assert_eq!(
            resolve_auth_token_with_source("codex", &auth, false),
            (None, "none".to_string())
        );
        assert_eq!(
            resolve_api_key_with_source("codex", &auth, false),
            (None, "none".to_string())
        );
    }
}

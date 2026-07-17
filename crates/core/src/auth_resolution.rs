use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::http::HeaderValue;
use thiserror::Error;

use crate::config::UpstreamAuth;
use crate::provider_catalog::ProviderAdapter;

#[cfg(test)]
const AUTH_FILE_CACHE_MIN_CHECK_INTERVAL: Duration = Duration::from_millis(20);
#[cfg(not(test))]
const AUTH_FILE_CACHE_MIN_CHECK_INTERVAL: Duration = Duration::from_millis(800);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CredentialSource {
    Inline,
    Environment { name: String },
    CodexAuthJson { field: String },
    ClaudeSettingsEnv { field: String },
}

impl CredentialSource {
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Inline => "inline configuration".to_string(),
            Self::Environment { name } => format!("environment reference `{name}`"),
            Self::CodexAuthJson { field } => format!("Codex auth.json field `{field}`"),
            Self::ClaudeSettingsEnv { field } => {
                format!("Claude settings env field `{field}`")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CredentialKind {
    Bearer,
    ApiKey,
}

impl CredentialKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Bearer => "Bearer token",
            Self::ApiKey => "X-API-Key",
        }
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub(crate) enum UpstreamAuthResolutionError {
    #[error("configured {kind} reference `{name}` is unavailable")]
    MissingReference { kind: &'static str, name: String },
    #[error("configured {kind} from {location} is not a valid HTTP header value")]
    InvalidValue {
        kind: &'static str,
        location: String,
    },
    #[error(
        "remote third-party Codex upstream requires helper credentials or explicit allow_anonymous = true"
    )]
    AnonymousNotAllowed,
}

impl UpstreamAuthResolutionError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::MissingReference { .. } => "missing_auth",
            Self::InvalidValue { .. } => "invalid_auth",
            Self::AnonymousNotAllowed => "missing_auth",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum CredentialResolution {
    Unconfigured,
    Resolved {
        value: String,
        source: CredentialSource,
    },
    MissingReference {
        name: String,
    },
    InvalidValue {
        source: CredentialSource,
    },
}

impl CredentialResolution {
    pub(crate) fn value(&self) -> Option<&str> {
        match self {
            Self::Resolved { value, .. } => Some(value),
            Self::Unconfigured | Self::MissingReference { .. } | Self::InvalidValue { .. } => None,
        }
    }

    pub(crate) fn into_value(self) -> Option<String> {
        match self {
            Self::Resolved { value, .. } => Some(value),
            Self::Unconfigured | Self::MissingReference { .. } | Self::InvalidValue { .. } => None,
        }
    }

    pub(crate) fn is_unavailable(&self) -> bool {
        matches!(
            self,
            Self::MissingReference { .. } | Self::InvalidValue { .. }
        )
    }

    fn unavailable_error(&self, kind: CredentialKind) -> Option<UpstreamAuthResolutionError> {
        match self {
            Self::MissingReference { name } => {
                Some(UpstreamAuthResolutionError::MissingReference {
                    kind: kind.label(),
                    name: name.clone(),
                })
            }
            Self::InvalidValue { source } => Some(UpstreamAuthResolutionError::InvalidValue {
                kind: kind.label(),
                location: source.label(),
            }),
            Self::Unconfigured | Self::Resolved { .. } => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ResolvedUpstreamAuth {
    pub(crate) auth_token: CredentialResolution,
    pub(crate) api_key: CredentialResolution,
}

impl ResolvedUpstreamAuth {
    pub(crate) fn has_unavailable_credential(&self) -> bool {
        self.auth_token.is_unavailable() || self.api_key.is_unavailable()
    }

    pub(crate) fn ensure_available(&self) -> Result<(), UpstreamAuthResolutionError> {
        if let Some(error) = self.auth_token.unavailable_error(CredentialKind::Bearer) {
            return Err(error);
        }
        if let Some(error) = self.api_key.unavailable_error(CredentialKind::ApiKey) {
            return Err(error);
        }
        Ok(())
    }
}

#[derive(Default)]
struct JsonFileCache {
    last_check_at: Option<Instant>,
    last_path: Option<PathBuf>,
    value: Option<serde_json::Value>,
}

fn cached_json_file_value(
    cache: &OnceLock<Mutex<JsonFileCache>>,
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

    let value = read_json_file(&path);
    state.last_check_at = Some(now);
    state.last_path = Some(path);
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

fn codex_auth_json_value(key: &str) -> Option<String> {
    static CACHE: OnceLock<Mutex<JsonFileCache>> = OnceLock::new();
    let path = crate::config::codex_home().join("auth.json");
    let value = cached_json_file_value(&CACHE, path);
    value
        .as_ref()?
        .as_object()?
        .get(key)?
        .as_str()
        .map(str::to_owned)
}

pub(crate) fn claude_settings_env_value(key: &str) -> Option<String> {
    static CACHE: OnceLock<Mutex<JsonFileCache>> = OnceLock::new();
    let value = cached_json_file_value(&CACHE, crate::config::claude_settings_path());
    value
        .as_ref()?
        .as_object()?
        .get("env")?
        .as_object()?
        .get(key)?
        .as_str()
        .map(str::to_owned)
}

fn service_file_value(service_name: &str, key: &str) -> Option<(String, CredentialSource)> {
    let value = match service_name {
        "codex" => codex_auth_json_value(key).map(|value| {
            (
                value,
                CredentialSource::CodexAuthJson {
                    field: key.to_string(),
                },
            )
        }),
        "claude" => claude_settings_env_value(key).map(|value| {
            (
                value,
                CredentialSource::ClaudeSettingsEnv {
                    field: key.to_string(),
                },
            )
        }),
        _ => None,
    }?;
    (!value.0.trim().is_empty()).then_some(value)
}

fn resolve_credential(
    service_name: &str,
    inline: Option<&str>,
    env_name: Option<&str>,
    file_lookup: &impl Fn(&str, &str) -> Option<(String, CredentialSource)>,
) -> CredentialResolution {
    if let Some(value) = inline.filter(|value| !value.trim().is_empty()) {
        return CredentialResolution::Resolved {
            value: value.to_string(),
            source: CredentialSource::Inline,
        };
    }

    let Some(env_name) = env_name.map(str::trim).filter(|name| !name.is_empty()) else {
        return CredentialResolution::Unconfigured;
    };
    if let Ok(value) = std::env::var(env_name)
        && !value.trim().is_empty()
    {
        return CredentialResolution::Resolved {
            value,
            source: CredentialSource::Environment {
                name: env_name.to_string(),
            },
        };
    }
    if let Some((value, source)) = file_lookup(service_name, env_name)
        && !value.trim().is_empty()
    {
        return CredentialResolution::Resolved { value, source };
    }

    CredentialResolution::MissingReference {
        name: env_name.to_string(),
    }
}

fn resolve_upstream_auth_with_file_lookup(
    service_name: &str,
    auth: &UpstreamAuth,
    file_lookup: &impl Fn(&str, &str) -> Option<(String, CredentialSource)>,
) -> ResolvedUpstreamAuth {
    ResolvedUpstreamAuth {
        auth_token: validate_header_value(
            resolve_credential(
                service_name,
                auth.auth_token.as_deref(),
                auth.auth_token_env.as_deref(),
                file_lookup,
            ),
            true,
        ),
        api_key: validate_header_value(
            resolve_credential(
                service_name,
                auth.api_key.as_deref(),
                auth.api_key_env.as_deref(),
                file_lookup,
            ),
            false,
        ),
    }
}

fn validate_header_value(resolution: CredentialResolution, bearer: bool) -> CredentialResolution {
    let CredentialResolution::Resolved { value, source } = resolution else {
        return resolution;
    };
    let valid = if bearer {
        HeaderValue::from_str(&format!("Bearer {value}")).is_ok()
    } else {
        HeaderValue::from_str(&value).is_ok()
    };
    if valid {
        CredentialResolution::Resolved { value, source }
    } else {
        CredentialResolution::InvalidValue { source }
    }
}

pub(crate) fn resolve_upstream_auth(
    service_name: &str,
    auth: &UpstreamAuth,
) -> ResolvedUpstreamAuth {
    resolve_upstream_auth_with_file_lookup(service_name, auth, &service_file_value)
}

pub(crate) fn upstream_auth_contract_is_configured(auth: &UpstreamAuth) -> bool {
    [
        auth.auth_token.as_deref(),
        auth.auth_token_env.as_deref(),
        auth.api_key.as_deref(),
        auth.api_key_env.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| !value.trim().is_empty())
}

pub(crate) fn trusted_codex_passthrough_origin(target_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(target_url) else {
        return false;
    };
    url.scheme() == "https" && ProviderAdapter::for_endpoint(&url) == ProviderAdapter::OpenAiCodex
}

fn target_is_loopback(target_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(target_url) else {
        return false;
    };
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    })
}

pub(crate) fn unconfigured_upstream_auth_requires_opt_in(
    service_name: &str,
    auth: &UpstreamAuth,
    target_url: &str,
) -> bool {
    service_name == "codex"
        && !upstream_auth_contract_is_configured(auth)
        && auth.allow_anonymous != Some(true)
        && !trusted_codex_passthrough_origin(target_url)
        && !target_is_loopback(target_url)
}

pub(crate) fn resolve_upstream_auth_for_target(
    service_name: &str,
    auth: &UpstreamAuth,
    target_url: &str,
) -> Result<ResolvedUpstreamAuth, UpstreamAuthResolutionError> {
    let resolved = resolve_upstream_auth(service_name, auth);
    resolved.ensure_available()?;
    if unconfigured_upstream_auth_requires_opt_in(service_name, auth, target_url) {
        return Err(UpstreamAuthResolutionError::AnonymousNotAllowed);
    }
    Ok(resolved)
}

pub(crate) fn resolve_named_credential(service_name: &str, env_name: &str) -> CredentialResolution {
    validate_header_value(
        resolve_credential(service_name, None, Some(env_name), &service_file_value),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_file_cache_reloads_same_path_after_check_interval() {
        let directory = tempfile::tempdir().expect("create temporary auth directory");
        let path = directory.path().join("auth.json");
        let cache = OnceLock::new();
        std::fs::write(&path, r#"{"TOKEN":"aaaa"}"#).expect("write initial auth file");

        let initial = cached_json_file_value(&cache, path.clone()).expect("read initial auth file");
        assert_eq!(initial["TOKEN"].as_str(), Some("aaaa"));

        std::fs::write(&path, r#"{"TOKEN":"bbbb"}"#).expect("replace auth file");
        std::thread::sleep(AUTH_FILE_CACHE_MIN_CHECK_INTERVAL + Duration::from_millis(10));

        let updated = cached_json_file_value(&cache, path).expect("reload replaced auth file");
        assert_eq!(updated["TOKEN"].as_str(), Some("bbbb"));
    }

    #[test]
    fn auth_resolution_prefers_inline_secrets() {
        let auth = UpstreamAuth {
            auth_token: Some("token-1".to_string()),
            auth_token_env: Some("UNUSED_AUTH_ENV_FOR_TEST".to_string()),
            api_key: Some("key-1".to_string()),
            api_key_env: Some("UNUSED_API_ENV_FOR_TEST".to_string()),
            allow_anonymous: None,
        };

        let resolved = resolve_upstream_auth_with_file_lookup("codex", &auth, &|_, _| None);

        assert_eq!(resolved.auth_token.value(), Some("token-1"));
        assert_eq!(resolved.api_key.value(), Some("key-1"));
        assert!(!resolved.has_unavailable_credential());
    }

    #[test]
    fn auth_resolution_falls_back_to_explicit_codex_auth_json_field() {
        let auth = UpstreamAuth {
            auth_token_env: Some("RELAY_API_KEY".to_string()),
            ..UpstreamAuth::default()
        };

        let resolved = resolve_upstream_auth_with_file_lookup("codex", &auth, &|service, key| {
            (service == "codex" && key == "RELAY_API_KEY").then(|| {
                (
                    "file-token".to_string(),
                    CredentialSource::CodexAuthJson {
                        field: key.to_string(),
                    },
                )
            })
        });

        assert_eq!(resolved.auth_token.value(), Some("file-token"));
        assert!(matches!(
            resolved.auth_token,
            CredentialResolution::Resolved {
                source: CredentialSource::CodexAuthJson { ref field },
                ..
            } if field == "RELAY_API_KEY"
        ));
        assert!(!resolved.has_unavailable_credential());
    }

    #[test]
    fn auth_resolution_marks_unresolved_explicit_references_missing() {
        let auth = UpstreamAuth {
            auth_token_env: Some("CODEX_HELPER_TEST_MISSING_AUTH_ENV_09A1".to_string()),
            api_key_env: Some("CODEX_HELPER_TEST_MISSING_API_ENV_09A1".to_string()),
            ..UpstreamAuth::default()
        };

        let resolved = resolve_upstream_auth_with_file_lookup("codex", &auth, &|_, _| None);

        assert!(resolved.auth_token.is_unavailable());
        assert!(resolved.api_key.is_unavailable());
        assert!(resolved.has_unavailable_credential());
        assert!(matches!(
            resolved.ensure_available(),
            Err(UpstreamAuthResolutionError::MissingReference {
                kind: "Bearer token",
                ref name,
            }) if name == "CODEX_HELPER_TEST_MISSING_AUTH_ENV_09A1"
        ));
    }

    #[test]
    fn auth_resolution_does_not_consult_files_without_an_explicit_reference() {
        let resolved =
            resolve_upstream_auth_with_file_lookup("codex", &UpstreamAuth::default(), &|_, _| {
                panic!("unconfigured credentials must not read auth files")
            });

        assert!(matches!(
            resolved.auth_token,
            CredentialResolution::Unconfigured
        ));
        assert!(matches!(
            resolved.api_key,
            CredentialResolution::Unconfigured
        ));
    }

    #[test]
    fn auth_resolution_rejects_invalid_header_values_without_retaining_them() {
        let auth = UpstreamAuth {
            auth_token: Some("secret\nvalue".to_string()),
            ..UpstreamAuth::default()
        };

        let resolved = resolve_upstream_auth_with_file_lookup("codex", &auth, &|_, _| None);

        assert!(matches!(
            resolved.auth_token,
            CredentialResolution::InvalidValue {
                source: CredentialSource::Inline
            }
        ));
        assert!(resolved.has_unavailable_credential());
        assert!(matches!(
            resolved.ensure_available(),
            Err(UpstreamAuthResolutionError::InvalidValue {
                kind: "Bearer token",
                ref location,
            }) if location == "inline configuration"
        ));
    }

    #[test]
    fn remote_third_party_codex_target_rejects_unconfigured_auth_by_default() {
        let result = resolve_upstream_auth_for_target(
            "codex",
            &UpstreamAuth::default(),
            "https://relay.example/v1/responses",
        );

        assert!(matches!(
            result,
            Err(UpstreamAuthResolutionError::AnonymousNotAllowed)
        ));
    }

    #[test]
    fn remote_third_party_codex_target_allows_explicit_anonymous_opt_in() {
        let auth = UpstreamAuth {
            allow_anonymous: Some(true),
            ..UpstreamAuth::default()
        };

        let resolved =
            resolve_upstream_auth_for_target("codex", &auth, "https://relay.example/v1/responses")
                .expect("explicit anonymous opt-in");

        assert!(matches!(
            resolved.auth_token,
            CredentialResolution::Unconfigured
        ));
        assert!(matches!(
            resolved.api_key,
            CredentialResolution::Unconfigured
        ));
    }

    #[test]
    fn loopback_codex_target_allows_unconfigured_auth() {
        resolve_upstream_auth_for_target(
            "codex",
            &UpstreamAuth::default(),
            "http://127.0.0.1:3211/v1/responses",
        )
        .expect("loopback target does not require an anonymous opt-in");
    }

    #[test]
    fn explicit_anonymous_opt_in_does_not_mask_a_missing_reference() {
        let missing_reference = format!(
            "CODEX_HELPER_TEST_MISSING_AUTH_WITH_ANON_{}",
            uuid::Uuid::new_v4().simple()
        );
        let auth = UpstreamAuth {
            auth_token_env: Some(missing_reference.clone()),
            allow_anonymous: Some(true),
            ..UpstreamAuth::default()
        };

        let result =
            resolve_upstream_auth_for_target("codex", &auth, "https://relay.example/v1/responses");

        assert!(matches!(
            result,
            Err(UpstreamAuthResolutionError::MissingReference { name, .. })
                if name == missing_reference
        ));
    }
}

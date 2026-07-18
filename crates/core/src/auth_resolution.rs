use std::io::Read as _;
use std::path::Path;

use thiserror::Error;
use zeroize::{Zeroize as _, Zeroizing};

use crate::credentials::CredentialReadinessCode;
use crate::provider_catalog::ProviderAdapter;

const CLIENT_CREDENTIAL_FILE_MAX_BYTES: u64 = 1024 * 1024;

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

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub(crate) enum UpstreamAuthResolutionError {
    #[error("configured {kind} reference `{name}` is unavailable")]
    MissingReference { kind: &'static str, name: String },
    #[error("configured {kind} credential source `{source_kind}` is not available in this runtime")]
    UnsupportedReference {
        kind: &'static str,
        source_kind: &'static str,
    },
    #[error("configured {kind} credential source `{source_kind}` is {reason}")]
    RuntimeCredentialUnavailable {
        kind: &'static str,
        source_kind: &'static str,
        reason: &'static str,
        reference: String,
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
            Self::UnsupportedReference { .. } => "missing_auth",
            Self::RuntimeCredentialUnavailable { reason, .. } if *reason == "invalid" => {
                "invalid_auth"
            }
            Self::RuntimeCredentialUnavailable { .. } => "missing_auth",
            Self::AnonymousNotAllowed => "missing_auth",
        }
    }

    pub(crate) fn source_kind(&self) -> &'static str {
        match self {
            Self::MissingReference { .. } => "environment",
            Self::UnsupportedReference { source_kind, .. }
            | Self::RuntimeCredentialUnavailable { source_kind, .. } => source_kind,
            Self::AnonymousNotAllowed => "configuration",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum CredentialResolution {
    Unconfigured,
    Resolved {
        value: Zeroizing<String>,
        source: CredentialSource,
    },
    MissingReference {
        name: String,
    },
    InvalidValue {
        source: CredentialSource,
    },
}

struct SensitiveJsonValue(serde_json::Value);

impl Drop for SensitiveJsonValue {
    fn drop(&mut self) {
        zeroize_json_value(&mut self.0);
    }
}

fn zeroize_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(value) => value.zeroize(),
        serde_json::Value::Array(values) => {
            for value in values {
                zeroize_json_value(value);
            }
        }
        serde_json::Value::Object(values) => {
            for (mut key, mut value) in std::mem::take(values) {
                key.zeroize();
                zeroize_json_value(&mut value);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn read_json_file(path: &Path) -> Option<SensitiveJsonValue> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(CLIENT_CREDENTIAL_FILE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.is_empty() || bytes.len() as u64 > CLIENT_CREDENTIAL_FILE_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes.as_slice())
        .ok()
        .map(SensitiveJsonValue)
}

fn codex_auth_json_value(key: &str) -> Option<String> {
    let path = crate::config::codex_home().join("auth.json");
    let value = read_json_file(&path)?;
    value.0.as_object()?.get(key)?.as_str().map(str::to_owned)
}

pub(crate) fn claude_settings_env_value(key: &str) -> Option<String> {
    let value = read_json_file(&crate::config::claude_settings_path())?;
    value
        .0
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
            value: Zeroizing::new(value.to_string()),
            source: CredentialSource::Inline,
        };
    }

    let Some(env_name) = env_name.map(str::trim).filter(|name| !name.is_empty()) else {
        return CredentialResolution::Unconfigured;
    };
    if !is_valid_environment_variable_name(env_name) {
        return CredentialResolution::InvalidValue {
            source: CredentialSource::Environment {
                name: env_name.to_string(),
            },
        };
    }
    if let Ok(value) = std::env::var(env_name)
        && !value.trim().is_empty()
    {
        return CredentialResolution::Resolved {
            value: Zeroizing::new(value),
            source: CredentialSource::Environment {
                name: env_name.to_string(),
            },
        };
    }
    if let Some((value, source)) = file_lookup(service_name, env_name)
        && !value.trim().is_empty()
    {
        return CredentialResolution::Resolved {
            value: Zeroizing::new(value),
            source,
        };
    }

    CredentialResolution::MissingReference {
        name: env_name.to_string(),
    }
}

pub(crate) fn is_valid_environment_variable_name(name: &str) -> bool {
    !name.is_empty() && !name.bytes().any(|byte| byte == b'=' || byte == 0)
}

pub(crate) fn resolve_service_credential_for_runtime(
    service_name: &str,
    inline: Option<&str>,
    env_name: Option<&str>,
) -> CredentialResolution {
    resolve_credential(service_name, inline, env_name, &service_file_value)
}

pub(crate) fn resolve_environment_credential_for_runtime(env_name: &str) -> CredentialResolution {
    resolve_credential("environment", None, Some(env_name), &|_, _| None)
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

pub(crate) fn unconfigured_upstream_auth_contract_requires_opt_in(
    service_name: &str,
    configured_contract: bool,
    allow_anonymous: bool,
    target_url: &str,
) -> bool {
    service_name == "codex"
        && !configured_contract
        && !allow_anonymous
        && !trusted_codex_passthrough_origin(target_url)
        && !target_is_loopback(target_url)
}

pub(crate) fn target_credential_readiness(
    service_name: &str,
    configured_contract: bool,
    allow_anonymous: bool,
    target_url: &str,
    captured: CredentialReadinessCode,
) -> CredentialReadinessCode {
    if unconfigured_upstream_auth_contract_requires_opt_in(
        service_name,
        configured_contract,
        allow_anonymous,
        target_url,
    ) {
        CredentialReadinessCode::Missing
    } else {
        captured
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_json_file_reader_reads_valid_files_without_retaining_a_cache() {
        let directory = tempfile::tempdir().expect("create temporary auth directory");
        let path = directory.path().join("auth.json");
        std::fs::write(&path, r#"{"TOKEN":"aaaa"}"#).expect("write initial auth file");

        let initial = read_json_file(&path).expect("read initial auth file");
        assert_eq!(initial.0["TOKEN"].as_str(), Some("aaaa"));
        drop(initial);

        std::fs::write(&path, r#"{"TOKEN":"bbbb"}"#).expect("replace auth file");

        let updated = read_json_file(&path).expect("read replaced auth file");
        assert_eq!(updated.0["TOKEN"].as_str(), Some("bbbb"));
    }

    #[test]
    fn bounded_json_file_reader_rejects_empty_and_oversized_files() {
        let directory = tempfile::tempdir().expect("create temporary auth directory");
        let path = directory.path().join("auth.json");
        std::fs::write(&path, []).expect("write empty auth file");
        assert!(read_json_file(&path).is_none());

        std::fs::write(
            &path,
            vec![b'a'; CLIENT_CREDENTIAL_FILE_MAX_BYTES as usize + 1],
        )
        .expect("write oversized auth file");
        assert!(read_json_file(&path).is_none());
    }

    #[test]
    fn environment_only_resolution_never_uses_a_client_file_fallback() {
        let name = format!(
            "CODEX_HELPER_TEST_ENVIRONMENT_ONLY_{}",
            uuid::Uuid::new_v4().simple()
        );
        let environment_only = resolve_environment_credential_for_runtime(&name);
        assert!(matches!(
            environment_only,
            CredentialResolution::MissingReference { name: ref missing } if missing == &name
        ));

        let service_credential = resolve_credential("codex", None, Some(&name), &|_, field| {
            (field == name).then(|| {
                (
                    "client-file-value".to_string(),
                    CredentialSource::CodexAuthJson {
                        field: field.to_string(),
                    },
                )
            })
        });
        assert!(matches!(
            service_credential,
            CredentialResolution::Resolved { ref value, .. }
                if value.as_str() == "client-file-value"
        ));
    }

    #[test]
    fn invalid_environment_variable_names_are_rejected_without_accessing_the_environment() {
        for name in ["BAD=NAME", "BAD\0NAME"] {
            let resolution = resolve_environment_credential_for_runtime(name);
            assert!(matches!(
                resolution,
                CredentialResolution::InvalidValue {
                    source: CredentialSource::Environment { name: ref rejected },
                } if rejected == name
            ));
        }
    }

    #[test]
    fn service_resolution_prefers_inline_secrets() {
        let resolved = resolve_credential(
            "codex",
            Some("token-1"),
            Some("UNUSED_AUTH_ENV_FOR_TEST"),
            &|_, _| panic!("inline credentials must not read client files"),
        );

        assert!(matches!(
            resolved,
            CredentialResolution::Resolved {
                ref value,
                source: CredentialSource::Inline,
            } if value.as_str() == "token-1"
        ));
    }

    #[test]
    fn unconfigured_resolution_does_not_consult_client_files() {
        let resolved = resolve_credential("codex", None, None, &|_, _| {
            panic!("unconfigured credentials must not read client files")
        });

        assert!(matches!(resolved, CredentialResolution::Unconfigured));
    }

    #[test]
    fn remote_third_party_codex_target_rejects_unconfigured_auth_by_default() {
        assert_eq!(
            target_credential_readiness(
                "codex",
                false,
                false,
                "https://relay.example/v1/responses",
                CredentialReadinessCode::Ready,
            ),
            CredentialReadinessCode::Missing
        );
    }

    #[test]
    fn explicit_anonymous_opt_in_allows_unconfigured_remote_target() {
        assert_eq!(
            target_credential_readiness(
                "codex",
                false,
                true,
                "https://relay.example/v1/responses",
                CredentialReadinessCode::Ready,
            ),
            CredentialReadinessCode::Ready
        );
    }

    #[test]
    fn loopback_codex_target_allows_unconfigured_auth() {
        assert_eq!(
            target_credential_readiness(
                "codex",
                false,
                false,
                "http://127.0.0.1:3211/v1/responses",
                CredentialReadinessCode::Ready,
            ),
            CredentialReadinessCode::Ready
        );
    }

    #[test]
    fn anonymous_opt_in_does_not_mask_configured_credential_failures() {
        for captured in [
            CredentialReadinessCode::Missing,
            CredentialReadinessCode::Unsupported,
        ] {
            assert_eq!(
                target_credential_readiness(
                    "codex",
                    true,
                    true,
                    "https://relay.example/v1/responses",
                    captured,
                ),
                captured
            );
        }
    }

    #[test]
    fn auth_resolution_log_fields_do_not_expose_logical_references() {
        const REFERENCE_CANARY: &str = "CODEX_HELPER_REFERENCE_CANARY_7D4E";
        let error = UpstreamAuthResolutionError::MissingReference {
            kind: "Bearer token",
            name: REFERENCE_CANARY.to_string(),
        };
        let rendered = format!("code={} source={}", error.code(), error.source_kind());

        assert_eq!(rendered, "code=missing_auth source=environment");
        assert!(!rendered.contains(REFERENCE_CANARY));
    }
}

// These fields are intentionally read only by Serde: the structs are a migration-time schema.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value as JsonValue;

use super::{
    FleetRegistryConfig, NotifyConfig, RetryProfileName, RetryStrategy, ServiceKind,
    ServiceStatusConfig, UpstreamConfig,
};

/// Validate JSON written by the last native `config.json` loader before nulls are removed for TOML.
pub(super) fn validate_json_migration_source(value: &JsonValue, source_name: &str) -> Result<()> {
    if has_provider_shaped_service(value) {
        reject_provider_shaped_json_nulls(value, "", source_name)?;
        return Ok(());
    }

    serde_json::from_value::<LegacyJsonConfig>(value.clone())
        .map(|_| ())
        .with_context(|| format!("validate {source_name} against the historical JSON schema"))
}

fn has_provider_shaped_service(value: &JsonValue) -> bool {
    ["codex", "claude"].iter().any(|service_name| {
        value
            .get(*service_name)
            .and_then(JsonValue::as_object)
            .is_some_and(|service| {
                [
                    "providers",
                    "groups",
                    "routing",
                    "active_group",
                    "active_station",
                ]
                .iter()
                .any(|field| service.contains_key(*field))
            })
    })
}

fn reject_provider_shaped_json_nulls(
    value: &JsonValue,
    path: &str,
    source_name: &str,
) -> Result<()> {
    match value {
        JsonValue::Null => {
            let path = if path.is_empty() { "<root>" } else { path };
            anyhow::bail!(
                "{source_name} contains null at `{path}` in a provider-shaped JSON configuration; only historical station-shaped JSON has a published nullable-field contract"
            );
        }
        JsonValue::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                reject_provider_shaped_json_nulls(value, &format!("{path}[{index}]"), source_name)?;
            }
        }
        JsonValue::Object(values) => {
            for (key, value) in values {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                reject_provider_shaped_json_nulls(value, &child_path, source_name)?;
            }
        }
        JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {}
    }
    Ok(())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LegacyJsonConfig {
    version: Option<JsonValue>,
    codex: LegacyJsonService,
    claude: LegacyJsonService,
    retry: LegacyJsonRetryConfig,
    notify: NotifyConfig,
    default_service: Option<ServiceKind>,
    relay_targets: BTreeMap<String, LegacyJsonRelayTarget>,
    fleet: FleetRegistryConfig,
    ui: LegacyJsonUiConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LegacyJsonService {
    active: Option<String>,
    default_profile: Option<String>,
    profiles: BTreeMap<String, LegacyJsonProfile>,
    #[serde(rename = "stations", alias = "configs")]
    stations: HashMap<String, LegacyJsonStation>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LegacyJsonProfile {
    extends: Option<String>,
    station: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    service_tier: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LegacyJsonStation {
    name: String,
    alias: Option<String>,
    enabled: bool,
    level: u8,
    upstreams: Vec<UpstreamConfig>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LegacyJsonRetryLayer {
    max_attempts: Option<u32>,
    backoff_ms: Option<u64>,
    backoff_max_ms: Option<u64>,
    jitter_ms: Option<u64>,
    on_status: Option<String>,
    on_class: Option<Vec<String>>,
    strategy: Option<RetryStrategy>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LegacyJsonReasoningGuard {
    enabled: Option<bool>,
    reasoning_equals: Option<Vec<i64>>,
    paths: Option<Vec<String>>,
    action: Option<super::ReasoningGuardAction>,
    stream_mode: Option<super::ReasoningGuardStreamMode>,
    max_guard_retries: Option<u32>,
    log_matches: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LegacyJsonRetryConfig {
    profile: Option<RetryProfileName>,
    max_attempts: Option<u32>,
    backoff_ms: Option<u64>,
    backoff_max_ms: Option<u64>,
    jitter_ms: Option<u64>,
    on_status: Option<String>,
    on_class: Option<Vec<String>>,
    strategy: Option<RetryStrategy>,
    upstream: Option<LegacyJsonRetryLayer>,
    provider: Option<LegacyJsonRetryLayer>,
    reasoning_guard: Option<LegacyJsonReasoningGuard>,
    allow_cross_station_before_first_output: Option<bool>,
    never_on_status: Option<String>,
    never_on_class: Option<Vec<String>>,
    cloudflare_challenge_cooldown_secs: Option<u64>,
    cloudflare_timeout_cooldown_secs: Option<u64>,
    transport_cooldown_secs: Option<u64>,
    cooldown_backoff_factor: Option<u64>,
    cooldown_backoff_max_secs: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LegacyJsonRelayTarget {
    service: Option<ServiceKind>,
    proxy_url: String,
    admin_url: Option<String>,
    admin_token_env: Option<String>,
    client_preset: Option<JsonValue>,
    responses_websocket: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LegacyJsonUsageForecast {
    enabled: bool,
    rate_window_minutes: u64,
    min_priced_requests: u64,
    reset_time: String,
    reset_utc_offset: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LegacyJsonUiConfig {
    language: Option<String>,
    usage_forecast: LegacyJsonUsageForecast,
    service_status: ServiceStatusConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_json_accepts_published_nullable_fields() {
        let value = serde_json::json!({
            "version": null,
            "default_service": null,
            "codex": {
                "active": null,
                "default_profile": null,
                "profiles": {
                    "deep": {
                        "extends": null,
                        "station": null,
                        "model": null,
                        "reasoning_effort": null,
                        "service_tier": null
                    }
                },
                "stations": {
                    "primary": {
                        "name": "primary",
                        "alias": null,
                        "enabled": true,
                        "level": 1,
                        "upstreams": [{
                            "base_url": "https://relay.example/v1",
                            "auth": {
                                "auth_token": null,
                                "auth_token_env": null,
                                "api_key": null,
                                "api_key_env": null
                            }
                        }]
                    }
                }
            },
            "retry": {
                "max_attempts": null,
                "upstream": null,
                "provider": null,
                "reasoning_guard": null
            }
        });

        validate_json_migration_source(&value, "config.json")
            .expect("published Option fields should accept JSON null");
    }

    #[test]
    fn historical_json_rejects_non_nullable_nulls() {
        let cases = [
            serde_json::json!({"codex": null}),
            serde_json::json!({
                "codex": {"stations": {"primary": {
                    "upstreams": [{"base_url": "https://relay.example/v1", "auth": null}]
                }}}
            }),
            serde_json::json!({
                "codex": {"stations": {"primary": {
                    "upstreams": [null]
                }}}
            }),
            serde_json::json!({
                "codex": {"stations": {"primary": {
                    "upstreams": [{
                        "base_url": "https://relay.example/v1",
                        "tags": {"region": null}
                    }]
                }}}
            }),
        ];

        for value in cases {
            validate_json_migration_source(&value, "config.json")
                .expect_err("historical non-Option null must fail validation");
        }
    }

    #[test]
    fn provider_shaped_json_rejects_every_null() {
        let value = serde_json::json!({
            "version": 5,
            "codex": {
                "providers": {
                    "relay": {"base_url": "https://relay.example/v1", "alias": null}
                }
            }
        });

        let error = validate_json_migration_source(&value, "config.json")
            .expect_err("provider-shaped JSON null must fail closed");
        assert!(error.to_string().contains("codex.providers.relay.alias"));
    }
}

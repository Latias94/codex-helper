use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

use crate::auth_resolution::{
    CodexAuthMetadata, CodexAuthModeMetadata, is_valid_environment_variable_name,
    trusted_codex_passthrough_origin,
};
use crate::codex_switch::{
    project_original_codex_config, project_original_codex_config_for_onboarding,
};
use crate::config::{
    HelperConfig, ProviderConfig, RouteGraphConfig, ServiceRouteConfig, UpstreamAuth,
    load_config_with_source, mutate_helper_config,
};
use crate::routing_ir::compile_route_handshake_plan;

const DEFAULT_CODEX_PROVIDER_ID: &str = "openai";
const HELPER_CODEX_PROVIDER_ID: &str = "codex_proxy";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexOnboardingOutcome {
    ExistingConfiguration,
    Imported {
        provider_id: String,
        config_path: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexOnboardingFeasibility {
    ExistingConfiguration,
    Importable {
        provider_id: String,
        credential_reference: Option<String>,
    },
    Blocked {
        reason: String,
    },
}

#[derive(Debug, Deserialize, Default)]
struct CodexConfigProjection {
    model_provider: Option<String>,
    openai_base_url: Option<String>,
    #[serde(default)]
    model_providers: BTreeMap<String, CodexProviderProjection>,
}

#[derive(Debug, Deserialize, Default)]
struct CodexProviderProjection {
    name: Option<String>,
    base_url: Option<String>,
    env_key: Option<String>,
    requires_openai_auth: Option<bool>,
}

#[derive(Debug)]
struct PlannedCodexRoute {
    provider_id: String,
    provider: ProviderConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitDecision {
    Existing,
    Imported,
}

pub async fn ensure_default_codex_route(local_proxy_port: u16) -> Result<CodexOnboardingOutcome> {
    let loaded = load_config_with_source().await?;
    if has_existing_codex_configuration(&loaded.source.codex)? {
        return Ok(CodexOnboardingOutcome::ExistingConfiguration);
    }

    let planned = project_original_codex_config_for_onboarding(|config, auth| {
        plan_codex_route(config, &auth, local_proxy_port)
    })
    .context("inspect the original Codex client configuration")??;
    let provider_id = planned.provider_id.clone();
    let (config_path, decision) = mutate_helper_config(move |config| {
        if has_existing_codex_configuration(&config.codex)? {
            return Ok(CommitDecision::Existing);
        }
        install_planned_route(config, planned)?;
        Ok(CommitDecision::Imported)
    })
    .await
    .context("persist the imported Codex provider route")?;

    if decision == CommitDecision::Existing {
        return Ok(CodexOnboardingOutcome::ExistingConfiguration);
    }

    let persisted = load_config_with_source().await?;
    let handshake = compile_route_handshake_plan("codex", &persisted.source.codex)
        .context("validate the imported Codex provider route")?;
    ensure!(
        handshake
            .candidates
            .iter()
            .any(|candidate| candidate.provider_id == provider_id),
        "the imported Codex provider route is not selectable"
    );
    Ok(CodexOnboardingOutcome::Imported {
        provider_id,
        config_path,
    })
}

pub fn inspect_codex_onboarding_feasibility(
    config: &HelperConfig,
    local_proxy_port: u16,
) -> CodexOnboardingFeasibility {
    match has_existing_codex_configuration(&config.codex) {
        Ok(true) => return CodexOnboardingFeasibility::ExistingConfiguration,
        Ok(false) => {}
        Err(error) => {
            return CodexOnboardingFeasibility::Blocked {
                reason: error.to_string(),
            };
        }
    }

    match project_original_codex_config(|config, auth| {
        plan_codex_route(config, &auth, local_proxy_port).map(|planned| {
            let credential_reference = planned.provider.auth.auth_token_env.clone();
            (planned.provider_id, credential_reference)
        })
    }) {
        Ok(Ok((provider_id, credential_reference))) => CodexOnboardingFeasibility::Importable {
            provider_id,
            credential_reference,
        },
        Ok(Err(error)) => CodexOnboardingFeasibility::Blocked {
            reason: error.to_string(),
        },
        Err(error) => CodexOnboardingFeasibility::Blocked {
            reason: error.to_string(),
        },
    }
}

fn install_planned_route(config: &mut HelperConfig, planned: PlannedCodexRoute) -> Result<()> {
    let mut candidate = config.clone();
    let imported_provider_id = planned.provider_id.clone();
    candidate
        .codex
        .providers
        .insert(planned.provider_id.clone(), planned.provider);
    candidate.codex.routing = Some(RouteGraphConfig::ordered_failover(vec![
        planned.provider_id,
    ]));
    let handshake = compile_route_handshake_plan("codex", &candidate.codex)
        .context("validate the candidate Codex provider route before saving")?;
    ensure!(
        handshake
            .candidates
            .iter()
            .any(|route| route.provider_id == imported_provider_id),
        "the candidate Codex provider route is not selectable"
    );
    *config = candidate;
    Ok(())
}

fn has_existing_codex_configuration(service: &ServiceRouteConfig) -> Result<bool> {
    if !service.providers.is_empty() {
        return Ok(true);
    }
    Ok(!compile_route_handshake_plan("codex", service)?
        .candidates
        .is_empty())
}

fn plan_codex_route(
    config_text: Option<&str>,
    auth: &CodexAuthMetadata,
    local_proxy_port: u16,
) -> Result<PlannedCodexRoute> {
    let config = match config_text.filter(|text| !text.trim().is_empty()) {
        Some(text) => toml::from_str::<CodexConfigProjection>(text).map_err(|_| {
            anyhow::anyhow!(
                "Codex config.toml cannot be read for automatic provider import; fix it or configure ~/.codex-helper/config.toml explicitly"
            )
        })?,
        None => CodexConfigProjection::default(),
    };
    let provider_id = config
        .model_provider
        .as_deref()
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .unwrap_or(DEFAULT_CODEX_PROVIDER_ID)
        .to_string();
    ensure!(
        provider_id != HELPER_CODEX_PROVIDER_ID,
        "Codex selects codex_proxy without a recoverable helper switch journal; run `codex-helper switch status` before automatic onboarding"
    );

    let source = config.model_providers.get(provider_id.as_str());
    let requires_openai_auth = source
        .and_then(|provider| provider.requires_openai_auth)
        .unwrap_or(provider_id == DEFAULT_CODEX_PROVIDER_ID);
    let base_url = resolve_base_url(&config, source, &provider_id, auth)?;
    let base_url = validate_import_base_url(base_url.as_str(), local_proxy_port)?;
    let mut upstream_auth = UpstreamAuth::default();
    if requires_openai_auth {
        ensure!(
            trusted_codex_passthrough_origin(base_url.as_str()),
            "Codex provider `{provider_id}` requests OpenAI client authentication for a non-official origin; configure helper-owned credentials explicitly"
        );
    } else {
        let env_name = source
            .and_then(|provider| provider.env_key.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| auth.unique_api_key_field().map(str::to_string))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Codex provider `{provider_id}` has no unambiguous environment credential reference; configure its helper provider explicitly"
                )
            })?;
        ensure!(
            is_valid_environment_variable_name(env_name.as_str()),
            "Codex provider `{provider_id}` uses an invalid environment variable name"
        );
        upstream_auth.auth_token_env = Some(env_name);
    }

    let alias = source
        .and_then(|provider| provider.name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != provider_id.as_str())
        .map(str::to_string);
    let tags = BTreeMap::from([
        ("provider_id".to_string(), provider_id.clone()),
        (
            "requires_openai_auth".to_string(),
            requires_openai_auth.to_string(),
        ),
        ("source".to_string(), "codex-config".to_string()),
    ]);
    Ok(PlannedCodexRoute {
        provider_id,
        provider: ProviderConfig {
            alias,
            base_url: Some(base_url),
            auth: upstream_auth,
            tags,
            ..ProviderConfig::default()
        },
    })
}

fn resolve_base_url(
    config: &CodexConfigProjection,
    source: Option<&CodexProviderProjection>,
    provider_id: &str,
    auth: &CodexAuthMetadata,
) -> Result<String> {
    if let Some(base_url) = source
        .and_then(|provider| provider.base_url.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(base_url.to_string());
    }
    if provider_id == DEFAULT_CODEX_PROVIDER_ID {
        if let Some(base_url) = config
            .openai_base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(base_url.to_string());
        }
        return match auth.mode {
            Some(CodexAuthModeMetadata::ApiKey) => Ok("https://api.openai.com/v1".to_string()),
            Some(CodexAuthModeMetadata::ChatGpt) => {
                Ok("https://chatgpt.com/backend-api/codex".to_string())
            }
            None if auth.unique_api_key_field() == Some("OPENAI_API_KEY") => {
                Ok("https://api.openai.com/v1".to_string())
            }
            None => bail!(
                "the built-in Codex provider has no discoverable authentication mode; sign in with Codex or configure ~/.codex-helper/config.toml explicitly"
            ),
        };
    }
    bail!(
        "Codex provider `{provider_id}` has no base_url; configure its helper provider explicitly"
    )
}

fn validate_import_base_url(base_url: &str, local_proxy_port: u16) -> Result<String> {
    let url = reqwest::Url::parse(base_url)
        .map_err(|_| anyhow::anyhow!("the active Codex provider has an invalid base_url"))?;
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "the active Codex provider base_url must use http or https"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "the active Codex provider base_url must not contain credentials"
    );
    ensure!(
        url.query().is_none() && url.fragment().is_none(),
        "the active Codex provider base_url must not contain a query or fragment"
    );
    let is_loopback = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    ensure!(
        !(is_loopback && url.port_or_known_default() == Some(local_proxy_port)),
        "the active Codex provider points back to this codex-helper listener"
    );
    Ok(url.as_str().trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(mode: Option<CodexAuthModeMetadata>, fields: &[&str]) -> CodexAuthMetadata {
        CodexAuthMetadata {
            mode,
            non_empty_api_key_fields: fields.iter().map(|field| (*field).to_string()).collect(),
        }
    }

    #[test]
    fn imports_only_the_active_custom_provider_and_references_its_environment() {
        let plan = plan_codex_route(
            Some(
                r#"model_provider = "relay"

[model_providers.relay]
name = "Relay"
base_url = "https://relay.example/v1/"
env_key = "RELAY_API_KEY"
requires_openai_auth = false

[model_providers.ignored]
base_url = "https://ignored.example/v1"
env_key = "IGNORED_API_KEY"
"#,
            ),
            &metadata(None, &[]),
            3211,
        )
        .expect("plan active custom provider");

        assert_eq!(plan.provider_id, "relay");
        assert_eq!(plan.provider.alias.as_deref(), Some("Relay"));
        assert_eq!(
            plan.provider.base_url.as_deref(),
            Some("https://relay.example/v1")
        );
        assert_eq!(
            plan.provider.auth.auth_token_env.as_deref(),
            Some("RELAY_API_KEY")
        );
        assert!(plan.provider.auth.auth_token.is_none());
        assert!(!format!("{plan:?}").contains("IGNORED_API_KEY"));
    }

    #[test]
    fn maps_builtin_auth_modes_to_their_official_origins() {
        for (mode, expected) in [
            (CodexAuthModeMetadata::ApiKey, "https://api.openai.com/v1"),
            (
                CodexAuthModeMetadata::ChatGpt,
                "https://chatgpt.com/backend-api/codex",
            ),
        ] {
            let plan = plan_codex_route(None, &metadata(Some(mode), &[]), 3211)
                .expect("plan built-in provider");
            assert_eq!(plan.provider_id, DEFAULT_CODEX_PROVIDER_ID);
            assert_eq!(plan.provider.base_url.as_deref(), Some(expected));
            assert!(plan.provider.auth.auth_token_env.is_none());
        }
    }

    #[test]
    fn any_existing_user_provider_blocks_automatic_import() {
        let mut service = ServiceRouteConfig::default();
        service.providers.insert(
            "disabled-user-provider".to_string(),
            ProviderConfig {
                enabled: false,
                ..ProviderConfig::default()
            },
        );

        assert!(
            has_existing_codex_configuration(&service)
                .expect("inspect existing provider configuration")
        );
    }

    #[test]
    fn invalid_candidate_never_partially_mutates_the_helper_config() {
        let mut config = HelperConfig::default();
        let original = toml::to_string(&config).expect("serialize original helper config");
        install_planned_route(
            &mut config,
            PlannedCodexRoute {
                provider_id: "missing-base-url".to_string(),
                provider: ProviderConfig::default(),
            },
        )
        .expect_err("candidate without an endpoint must fail");

        assert_eq!(
            toml::to_string(&config).expect("serialize helper config after failure"),
            original
        );
    }

    #[test]
    fn blocks_openai_auth_passthrough_to_a_third_party_origin() {
        let error = plan_codex_route(
            Some(
                r#"model_provider = "relay"
[model_providers.relay]
base_url = "https://relay.example/v1"
requires_openai_auth = true
"#,
            ),
            &metadata(Some(CodexAuthModeMetadata::ChatGpt), &[]),
            3211,
        )
        .expect_err("third-party passthrough must be blocked");
        assert!(error.to_string().contains("non-official origin"));
    }

    #[test]
    fn ambiguous_auth_fields_and_self_loops_fail_closed() {
        let ambiguous = plan_codex_route(
            Some(
                r#"model_provider = "relay"
[model_providers.relay]
base_url = "https://relay.example/v1"
requires_openai_auth = false
"#,
            ),
            &metadata(None, &["FIRST_API_KEY", "SECOND_API_KEY"]),
            3211,
        )
        .expect_err("ambiguous auth fields must be rejected");
        assert!(ambiguous.to_string().contains("unambiguous"));

        let self_loop = plan_codex_route(
            Some(
                r#"model_provider = "relay"
[model_providers.relay]
base_url = "http://127.0.0.1:3211/v1"
env_key = "RELAY_API_KEY"
requires_openai_auth = false
"#,
            ),
            &metadata(None, &[]),
            3211,
        )
        .expect_err("self-loop must be rejected");
        assert!(self_loop.to_string().contains("points back"));
    }
}

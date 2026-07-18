use super::config_doc::{
    ensure_routing, ensure_routing_order_contains, load_helper_config, ordered_provider_names,
    parse_cli_string_map, parse_cli_tags, print_provider_list, select_service_route_config,
    select_service_route_config_mut,
};
use crate::cli_types::ProviderCommand;
use crate::config::{
    CURRENT_CONFIG_VERSION, CredentialRef, ProviderConfig, ProviderEndpointConfig,
    ServiceRouteConfig, UpstreamAuth, storage::save_helper_config,
};
use crate::{CliError, CliResult};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
struct ProviderCatalogPayload {
    schema_version: u32,
    service: String,
    providers: Vec<ProviderView>,
}

#[derive(Debug, Serialize)]
struct ProviderShowPayload {
    schema_version: u32,
    service: String,
    provider: ProviderView,
}

#[derive(Debug, Serialize, Clone)]
struct ProviderEndpointView {
    name: String,
    base_url: String,
    enabled: bool,
    priority: u32,
    tags: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Clone)]
struct ProviderView {
    name: String,
    alias: Option<String>,
    enabled: bool,
    routing_index: Option<usize>,
    routing_target: bool,
    auth_token_env: Option<String>,
    auth_token_ref: Option<CredentialRef>,
    api_key_env: Option<String>,
    api_key_ref: Option<CredentialRef>,
    has_inline_auth_token: bool,
    has_inline_api_key: bool,
    allow_anonymous: bool,
    tags: BTreeMap<String, String>,
    supported_models: Vec<String>,
    model_mapping: BTreeMap<String, String>,
    endpoints: Vec<ProviderEndpointView>,
}

pub async fn handle_provider_cmd(cmd: ProviderCommand) -> CliResult<()> {
    match cmd {
        ProviderCommand::List {
            codex,
            claude,
            json,
        } => {
            let (cfg, service, label) = load_helper_config(codex, claude, "provider")
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            let (view, _) = select_service_route_config(&cfg, service);

            if json {
                let payload = ProviderCatalogPayload {
                    schema_version: CURRENT_CONFIG_VERSION,
                    service: service.to_string(),
                    providers: build_provider_views(view),
                };
                let text = serde_json::to_string_pretty(&payload)
                    .map_err(|e| CliError::Configuration(e.to_string()))?;
                println!("{text}");
            } else {
                print_provider_list(label, view);
            }
        }
        ProviderCommand::Show {
            name,
            codex,
            claude,
            json,
        } => {
            let (cfg, service, label) = load_helper_config(codex, claude, "provider")
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            let (view, _) = select_service_route_config(&cfg, service);
            let provider = build_provider_view(view, name.as_str()).ok_or_else(|| {
                CliError::Configuration(format!("provider '{}' not found in source config", name))
            })?;

            if json {
                let payload = ProviderShowPayload {
                    schema_version: CURRENT_CONFIG_VERSION,
                    service: service.to_string(),
                    provider,
                };
                let text = serde_json::to_string_pretty(&payload)
                    .map_err(|e| CliError::Configuration(e.to_string()))?;
                println!("{text}");
            } else {
                print_provider_detail(label, &provider);
            }
        }
        ProviderCommand::Add {
            name,
            base_url,
            auth_token,
            auth_token_env,
            api_key,
            api_key_env,
            allow_anonymous,
            alias,
            tags,
            supported_models,
            model_mapping,
            disabled,
            replace,
            codex,
            claude,
        } => {
            let parsed_tags =
                parse_cli_tags(&tags).map_err(|e| CliError::Configuration(e.to_string()))?;
            let parsed_supported_models = parse_cli_supported_models(&supported_models)
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            let parsed_model_mapping = parse_cli_string_map(&model_mapping, "model-map")
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            let (mut cfg, service, label) = load_helper_config(codex, claude, "provider")
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            {
                let (view, _) = select_service_route_config_mut(&mut cfg, service);
                if view.providers.contains_key(name.as_str()) && !replace {
                    return Err(CliError::Configuration(format!(
                        "provider '{}' already exists; pass --replace to overwrite it",
                        name
                    )));
                }
                view.providers.insert(
                    name.clone(),
                    ProviderConfig {
                        alias,
                        enabled: !disabled,
                        base_url: Some(base_url),
                        inline_auth: UpstreamAuth {
                            auth_token: auth_token.map(Into::into),
                            auth_token_env,
                            auth_token_ref: None,
                            api_key: api_key.map(Into::into),
                            api_key_env,
                            api_key_ref: None,
                            allow_anonymous: allow_anonymous.then_some(true),
                        },
                        tags: parsed_tags,
                        supported_models: parsed_supported_models,
                        model_mapping: parsed_model_mapping,
                        ..ProviderConfig::default()
                    },
                );
                ensure_routing_order_contains(view, name.as_str());
                if disabled {
                    clear_manual_target_for_provider(view, name.as_str());
                }
            }

            save_helper_config(&cfg)
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            println!("Added {label} provider '{}'", name);
        }
        ProviderCommand::Enable {
            name,
            codex,
            claude,
        } => {
            let (mut cfg, service, label) = load_helper_config(codex, claude, "provider")
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            {
                let (view, _) = select_service_route_config_mut(&mut cfg, service);
                let Some(provider) = view.providers.get_mut(name.as_str()) else {
                    return Err(CliError::Configuration(format!(
                        "provider '{}' not found in source config",
                        name
                    )));
                };
                provider.enabled = true;
                ensure_routing_order_contains(view, name.as_str());
            }

            save_helper_config(&cfg)
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            println!("Enabled {label} provider '{}'", name);
        }
        ProviderCommand::Disable {
            name,
            codex,
            claude,
        } => {
            let (mut cfg, service, label) = load_helper_config(codex, claude, "provider")
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            let mut cleared_target = false;
            {
                let (view, _) = select_service_route_config_mut(&mut cfg, service);
                let Some(provider) = view.providers.get_mut(name.as_str()) else {
                    return Err(CliError::Configuration(format!(
                        "provider '{}' not found in source config",
                        name
                    )));
                };
                provider.enabled = false;

                if clear_manual_target_for_provider(view, name.as_str()) {
                    cleared_target = true;
                }
            }

            save_helper_config(&cfg)
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            if cleared_target {
                println!(
                    "Disabled {label} provider '{}' and cleared manual routing target",
                    name
                );
            } else {
                println!("Disabled {label} provider '{}'", name);
            }
        }
    }

    Ok(())
}

fn build_provider_views(view: &ServiceRouteConfig) -> Vec<ProviderView> {
    ordered_provider_names(view)
        .into_iter()
        .filter_map(|name| build_provider_view(view, name.as_str()))
        .collect()
}

fn build_provider_view(view: &ServiceRouteConfig, name: &str) -> Option<ProviderView> {
    let provider = view.providers.get(name)?;
    let effective_auth = provider.effective_auth();
    let route_order = crate::config::resolved_provider_order("provider-cli", view)
        .unwrap_or_else(|_| ordered_provider_names(view));
    let routing_index = route_order
        .iter()
        .position(|candidate| candidate == name)
        .map(|idx| idx + 1);
    let routing_target = crate::config::effective_routing(view)
        .entry_node()
        .and_then(|node| {
            matches!(node.strategy, crate::config::RouteStrategy::ManualSticky)
                .then(|| node.target.as_deref())
                .flatten()
        })
        .is_some_and(|target| target == name);

    Some(ProviderView {
        name: name.to_string(),
        alias: provider.alias.clone(),
        enabled: provider.enabled,
        routing_index,
        routing_target,
        auth_token_env: effective_auth.auth_token_env.clone(),
        auth_token_ref: effective_auth.auth_token_ref.clone(),
        api_key_env: effective_auth.api_key_env.clone(),
        api_key_ref: effective_auth.api_key_ref.clone(),
        has_inline_auth_token: effective_auth.auth_token.is_some(),
        has_inline_api_key: effective_auth.api_key.is_some(),
        allow_anonymous: effective_auth.allow_anonymous == Some(true),
        tags: provider.tags.clone(),
        supported_models: provider
            .supported_models
            .iter()
            .filter(|(_model, supported)| **supported)
            .map(|(model, _supported)| model.clone())
            .collect(),
        model_mapping: provider.model_mapping.clone(),
        endpoints: provider_endpoints(provider),
    })
}

fn clear_manual_target_for_provider(view: &mut ServiceRouteConfig, provider_name: &str) -> bool {
    ensure_routing(view).clear_manual_target_for(provider_name)
}

fn provider_endpoints(provider: &ProviderConfig) -> Vec<ProviderEndpointView> {
    let mut endpoints = Vec::new();
    if let Some(base_url) = provider
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        endpoints.push(ProviderEndpointView {
            name: "default".to_string(),
            base_url: base_url.to_string(),
            enabled: provider.enabled,
            priority: 0,
            tags: BTreeMap::new(),
        });
    }
    endpoints.extend(
        provider
            .endpoints
            .iter()
            .map(|(name, endpoint)| endpoint_view_from_config(name.as_str(), endpoint)),
    );
    endpoints
}

fn endpoint_view_from_config(
    name: &str,
    endpoint: &ProviderEndpointConfig,
) -> ProviderEndpointView {
    ProviderEndpointView {
        name: name.to_string(),
        base_url: endpoint.base_url.clone(),
        enabled: endpoint.enabled,
        priority: endpoint.priority,
        tags: endpoint.tags.clone(),
    }
}

fn print_provider_detail(label: &str, provider: &ProviderView) {
    println!("Schema version: v{CURRENT_CONFIG_VERSION}");
    println!("Service: {label}");
    println!("Provider: {}", provider.name);
    if let Some(alias) = provider.alias.as_deref() {
        println!("Alias: {alias}");
    }
    println!("Enabled: {}", provider.enabled);
    println!(
        "Routing: target={} index={}",
        provider.routing_target,
        provider
            .routing_index
            .map(|idx| idx.to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!("Auth: {}", provider_auth_summary(provider));
    println!("Tags: {}", format_tags(&provider.tags));
    println!(
        "Supported models: {}",
        format_models(&provider.supported_models)
    );
    println!(
        "Model mapping: {}",
        format_string_map(&provider.model_mapping)
    );
    println!("Endpoints:");
    if provider.endpoints.is_empty() {
        println!("  <none>");
    } else {
        for endpoint in &provider.endpoints {
            println!(
                "  [{}] {} enabled={} priority={} tags={}",
                endpoint.name,
                endpoint.base_url,
                endpoint.enabled,
                endpoint.priority,
                format_tags(&endpoint.tags)
            );
        }
    }
}

fn parse_cli_supported_models(raw_models: &[String]) -> anyhow::Result<BTreeMap<String, bool>> {
    let mut models = BTreeMap::new();
    for raw in raw_models {
        let model = raw.trim();
        if model.is_empty() {
            anyhow::bail!("supported-model must not be empty");
        }
        if models.insert(model.to_string(), true).is_some() {
            anyhow::bail!("duplicate supported-model '{}'", model);
        }
    }
    Ok(models)
}

fn provider_auth_summary(provider: &ProviderView) -> String {
    let mut parts = Vec::new();
    if let Some(reference) = provider.auth_token_ref.as_ref() {
        parts.push(format!("bearer_ref={}", credential_ref_summary(reference)));
    }
    if let Some(env) = provider.auth_token_env.as_deref() {
        parts.push(format!("bearer_env={env}"));
    }
    if let Some(reference) = provider.api_key_ref.as_ref() {
        parts.push(format!("api_key_ref={}", credential_ref_summary(reference)));
    }
    if let Some(env) = provider.api_key_env.as_deref() {
        parts.push(format!("api_key_env={env}"));
    }
    if provider.has_inline_auth_token {
        parts.push("bearer_inline=<redacted>".to_string());
    }
    if provider.has_inline_api_key {
        parts.push("api_key_inline=<redacted>".to_string());
    }
    if provider.allow_anonymous {
        parts.push("anonymous=explicit".to_string());
    }
    if parts.is_empty() {
        "<none>".to_string()
    } else {
        parts.join(" ")
    }
}

fn credential_ref_summary(reference: &CredentialRef) -> String {
    match reference {
        CredentialRef::Native { name } => format!("native:{name}"),
        CredentialRef::SecretFile { path } => format!("secret_file:{path}"),
    }
}

fn format_models(models: &[String]) -> String {
    if models.is_empty() {
        "-".to_string()
    } else {
        models.join(",")
    }
}

fn format_tags(tags: &BTreeMap<String, String>) -> String {
    if tags.is_empty() {
        return "-".to_string();
    }
    format_string_map(tags)
}

fn format_string_map(map: &BTreeMap<String, String>) -> String {
    if map.is_empty() {
        return "-".to_string();
    }
    map.iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cli_supported_models_rejects_empty_and_duplicate_entries() {
        let models = parse_cli_supported_models(&["gpt-5".to_string(), "gpt-5.5".to_string()])
            .expect("valid supported models");
        assert_eq!(models.get("gpt-5").copied(), Some(true));
        assert_eq!(models.get("gpt-5.5").copied(), Some(true));

        assert!(parse_cli_supported_models(&[" ".to_string()]).is_err());
        assert!(parse_cli_supported_models(&["gpt-5".to_string(), "gpt-5".to_string()]).is_err());
    }

    #[test]
    fn provider_view_projects_configured_credential_references() {
        let mut view = ServiceRouteConfig::default();
        view.providers.insert(
            "relay".to_string(),
            ProviderConfig {
                base_url: Some("https://relay.example/v1".to_string()),
                auth: UpstreamAuth {
                    auth_token_ref: Some(crate::config::CredentialRef::Native {
                        name: "relay.primary".to_string(),
                    }),
                    ..UpstreamAuth::default()
                },
                ..ProviderConfig::default()
            },
        );

        let provider = build_provider_view(&view, "relay").expect("provider view");
        let serialized = serde_json::to_value(&provider).expect("serialize provider view");

        assert_eq!(serialized["auth_token_ref"]["source"], "native");
        assert_eq!(serialized["auth_token_ref"]["name"], "relay.primary");
        assert_eq!(
            provider_auth_summary(&provider),
            "bearer_ref=native:relay.primary"
        );
    }
}

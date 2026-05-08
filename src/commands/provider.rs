use super::config_doc::{
    ensure_v3_routing, ensure_v3_routing_order_contains, load_v3_config, ordered_v3_provider_names,
    parse_cli_tags, print_v3_provider_list, select_v3_service_view, select_v3_service_view_mut,
};
use crate::cli_types::ProviderCommand;
use crate::config::{
    ProviderConfigV3, ProviderEndpointV3, RoutingExhaustedActionV3, RoutingPolicyV3, ServiceViewV3,
    UpstreamAuth, storage::save_config_v3,
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
    api_key_env: Option<String>,
    has_inline_auth_token: bool,
    has_inline_api_key: bool,
    tags: BTreeMap<String, String>,
    endpoints: Vec<ProviderEndpointView>,
}

pub async fn handle_provider_cmd(cmd: ProviderCommand) -> CliResult<()> {
    match cmd {
        ProviderCommand::List {
            codex,
            claude,
            json,
        } => {
            let (cfg, service, label) = load_v3_config(codex, claude, "provider")
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let (view, _) = select_v3_service_view(&cfg, service);

            if json {
                let payload = ProviderCatalogPayload {
                    schema_version: 3,
                    service: service.to_string(),
                    providers: build_provider_views(view),
                };
                let text = serde_json::to_string_pretty(&payload)
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                println!("{text}");
            } else {
                print_v3_provider_list(label, view);
            }
        }
        ProviderCommand::Show {
            name,
            codex,
            claude,
            json,
        } => {
            let (cfg, service, label) = load_v3_config(codex, claude, "provider")
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let (view, _) = select_v3_service_view(&cfg, service);
            let provider = build_provider_view(view, name.as_str()).ok_or_else(|| {
                CliError::ProxyConfig(format!("provider '{}' not found in v3 config", name))
            })?;

            if json {
                let payload = ProviderShowPayload {
                    schema_version: 3,
                    service: service.to_string(),
                    provider,
                };
                let text = serde_json::to_string_pretty(&payload)
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
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
            alias,
            tags,
            disabled,
            replace,
            codex,
            claude,
        } => {
            let parsed_tags =
                parse_cli_tags(&tags).map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let (mut cfg, service, label) = load_v3_config(codex, claude, "provider")
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            {
                let (view, _) = select_v3_service_view_mut(&mut cfg, service);
                if view.providers.contains_key(name.as_str()) && !replace {
                    return Err(CliError::ProxyConfig(format!(
                        "provider '{}' already exists; pass --replace to overwrite it",
                        name
                    )));
                }
                view.providers.insert(
                    name.clone(),
                    ProviderConfigV3 {
                        alias,
                        enabled: !disabled,
                        base_url: Some(base_url),
                        inline_auth: UpstreamAuth {
                            auth_token,
                            auth_token_env,
                            api_key,
                            api_key_env,
                        },
                        tags: parsed_tags,
                        ..ProviderConfigV3::default()
                    },
                );
                ensure_v3_routing_order_contains(view, name.as_str());
                if disabled {
                    let routing = ensure_v3_routing(view);
                    if routing.target.as_deref() == Some(name.as_str()) {
                        routing.policy = RoutingPolicyV3::OrderedFailover;
                        routing.target = None;
                        routing.prefer_tags.clear();
                        routing.on_exhausted = RoutingExhaustedActionV3::Continue;
                    }
                }
            }

            save_config_v3(&cfg)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            println!("Added {label} provider '{}'", name);
        }
        ProviderCommand::Enable {
            name,
            codex,
            claude,
        } => {
            let (mut cfg, service, label) = load_v3_config(codex, claude, "provider")
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            {
                let (view, _) = select_v3_service_view_mut(&mut cfg, service);
                let Some(provider) = view.providers.get_mut(name.as_str()) else {
                    return Err(CliError::ProxyConfig(format!(
                        "provider '{}' not found in v3 config",
                        name
                    )));
                };
                provider.enabled = true;
                ensure_v3_routing_order_contains(view, name.as_str());
            }

            save_config_v3(&cfg)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            println!("Enabled {label} provider '{}'", name);
        }
        ProviderCommand::Disable {
            name,
            codex,
            claude,
        } => {
            let (mut cfg, service, label) = load_v3_config(codex, claude, "provider")
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let mut cleared_target = false;
            {
                let (view, _) = select_v3_service_view_mut(&mut cfg, service);
                let Some(provider) = view.providers.get_mut(name.as_str()) else {
                    return Err(CliError::ProxyConfig(format!(
                        "provider '{}' not found in v3 config",
                        name
                    )));
                };
                provider.enabled = false;

                let routing = ensure_v3_routing(view);
                if routing.target.as_deref() == Some(name.as_str()) {
                    routing.policy = RoutingPolicyV3::OrderedFailover;
                    routing.target = None;
                    routing.prefer_tags.clear();
                    routing.on_exhausted = RoutingExhaustedActionV3::Continue;
                    cleared_target = true;
                }
            }

            save_config_v3(&cfg)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
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

fn build_provider_views(view: &ServiceViewV3) -> Vec<ProviderView> {
    ordered_v3_provider_names(view)
        .into_iter()
        .filter_map(|name| build_provider_view(view, name.as_str()))
        .collect()
}

fn build_provider_view(view: &ServiceViewV3, name: &str) -> Option<ProviderView> {
    let provider = view.providers.get(name)?;
    let routing = view.routing.as_ref();
    let routing_index = routing
        .and_then(|routing| routing.order.iter().position(|candidate| candidate == name))
        .map(|idx| idx + 1);
    let routing_target = routing
        .and_then(|routing| {
            matches!(routing.policy, RoutingPolicyV3::ManualSticky)
                .then(|| routing.target.as_deref())
                .flatten()
        })
        .is_some_and(|target| target == name);

    Some(ProviderView {
        name: name.to_string(),
        alias: provider.alias.clone(),
        enabled: provider.enabled,
        routing_index,
        routing_target,
        auth_token_env: provider
            .inline_auth
            .auth_token_env
            .clone()
            .or_else(|| provider.auth.auth_token_env.clone()),
        api_key_env: provider
            .inline_auth
            .api_key_env
            .clone()
            .or_else(|| provider.auth.api_key_env.clone()),
        has_inline_auth_token: provider.inline_auth.auth_token.is_some()
            || provider.auth.auth_token.is_some(),
        has_inline_api_key: provider.inline_auth.api_key.is_some()
            || provider.auth.api_key.is_some(),
        tags: provider.tags.clone(),
        endpoints: provider_endpoints(provider),
    })
}

fn provider_endpoints(provider: &ProviderConfigV3) -> Vec<ProviderEndpointView> {
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

fn endpoint_view_from_config(name: &str, endpoint: &ProviderEndpointV3) -> ProviderEndpointView {
    ProviderEndpointView {
        name: name.to_string(),
        base_url: endpoint.base_url.clone(),
        enabled: endpoint.enabled,
        priority: endpoint.priority,
        tags: endpoint.tags.clone(),
    }
}

fn print_provider_detail(label: &str, provider: &ProviderView) {
    println!("Schema version: v3");
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

fn provider_auth_summary(provider: &ProviderView) -> String {
    let mut parts = Vec::new();
    if let Some(env) = provider.auth_token_env.as_deref() {
        parts.push(format!("bearer_env={env}"));
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
    if parts.is_empty() {
        "<none>".to_string()
    } else {
        parts.join(" ")
    }
}

fn format_tags(tags: &BTreeMap<String, String>) -> String {
    if tags.is_empty() {
        return "-".to_string();
    }
    tags.iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",")
}

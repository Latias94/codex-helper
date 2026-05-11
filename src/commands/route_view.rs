use super::config_doc::{
    ConfigDocument, load_config_document, ordered_v4_provider_names, print_v4_provider_list,
    resolve_service, routing_exhausted_label, routing_policy_label, select_service_manager,
    select_v4_service_view,
};
use crate::config::{
    ServiceConfigManager, ServiceRoutingExplanation, ServiceViewV4, explain_service_routing,
    storage::config_file_path,
};
use crate::{CliError, CliResult, RoutingCommand};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
struct ConfigExplainProvider {
    name: String,
    alias: Option<String>,
    enabled: bool,
    level: u8,
    upstreams: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ConfigExplainPayload {
    schema_version: u32,
    service: String,
    active_provider: Option<String>,
    routing: ServiceRoutingExplanation,
    provider: Option<ConfigExplainProvider>,
}

#[derive(Debug, Serialize, Clone)]
struct ConfigExplainV4ProviderEndpoint {
    name: String,
    base_url: String,
    enabled: bool,
}

#[derive(Debug, Serialize, Clone)]
struct ConfigExplainV4Provider {
    name: String,
    alias: Option<String>,
    enabled: bool,
    routing_index: Option<usize>,
    target: bool,
    tags: BTreeMap<String, String>,
    endpoints: Vec<ConfigExplainV4ProviderEndpoint>,
}

#[derive(Debug, Serialize)]
struct ConfigExplainV4Routing {
    entry: String,
    policy: &'static str,
    target: Option<String>,
    order: Vec<String>,
    expanded_order: Vec<String>,
    prefer_tags: Vec<BTreeMap<String, String>>,
    on_exhausted: &'static str,
}

#[derive(Debug, Serialize)]
struct ConfigExplainV4Payload {
    schema_version: u32,
    service: String,
    routing: ConfigExplainV4Routing,
    providers: Vec<ConfigExplainV4Provider>,
    provider: Option<ConfigExplainV4Provider>,
}

fn explain_v4_routing(view: &ServiceViewV4) -> ConfigExplainV4Routing {
    let routing = crate::config::effective_v4_routing(view);
    let entry_node = routing.entry_node();
    ConfigExplainV4Routing {
        entry: routing.entry.clone(),
        policy: routing_policy_label(
            entry_node
                .map(|node| node.strategy)
                .unwrap_or(crate::config::RoutingPolicyV4::OrderedFailover),
        ),
        target: entry_node.and_then(|node| node.target.clone()),
        order: entry_node
            .map(|node| node.children.clone())
            .unwrap_or_default(),
        expanded_order: crate::config::resolved_v4_provider_order("route-view", view)
            .unwrap_or_else(|_| view.providers.keys().cloned().collect()),
        prefer_tags: entry_node
            .map(|node| node.prefer_tags.clone())
            .unwrap_or_default(),
        on_exhausted: routing_exhausted_label(
            entry_node
                .map(|node| node.on_exhausted)
                .unwrap_or(crate::config::RoutingExhaustedActionV4::Continue),
        ),
    }
}

fn explain_v4_provider(
    view: &ServiceViewV4,
    provider_name: &str,
) -> Option<ConfigExplainV4Provider> {
    let provider = view.providers.get(provider_name)?;
    let route_order = crate::config::resolved_v4_provider_order("route-view", view)
        .unwrap_or_else(|_| ordered_v4_provider_names(view));
    let routing_index = route_order
        .iter()
        .position(|candidate| candidate == provider_name)
        .map(|idx| idx + 1);
    let target = crate::config::effective_v4_routing(view)
        .entry_node()
        .and_then(|node| node.target.as_deref())
        .is_some_and(|target| target == provider_name);

    let mut endpoints = Vec::new();
    if let Some(base_url) = provider
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        endpoints.push(ConfigExplainV4ProviderEndpoint {
            name: "default".to_string(),
            base_url: base_url.to_string(),
            enabled: provider.enabled,
        });
    }
    endpoints.extend(provider.endpoints.iter().map(|(endpoint_name, endpoint)| {
        ConfigExplainV4ProviderEndpoint {
            name: endpoint_name.clone(),
            base_url: endpoint.base_url.clone(),
            enabled: endpoint.enabled,
        }
    }));

    Some(ConfigExplainV4Provider {
        name: provider_name.to_string(),
        alias: provider.alias.clone(),
        enabled: provider.enabled,
        routing_index,
        target,
        tags: provider.tags.clone(),
        endpoints,
    })
}

fn explain_v4_providers(view: &ServiceViewV4) -> Vec<ConfigExplainV4Provider> {
    ordered_v4_provider_names(view)
        .into_iter()
        .filter_map(|provider_name| explain_v4_provider(view, provider_name.as_str()))
        .collect()
}

fn print_v4_explain_text(
    label: &str,
    view: &ServiceViewV4,
    provider_name: Option<&str>,
) -> anyhow::Result<()> {
    let routing = explain_v4_routing(view);
    println!("Schema version: v4");
    println!("Service: {label}");
    println!("Routing entry: {}", routing.entry);
    println!("Routing policy: {}", routing.policy);
    println!(
        "Routing target: {}",
        routing.target.as_deref().unwrap_or("<none>")
    );
    let order = if routing.order.is_empty() {
        "<provider key order>".to_string()
    } else {
        routing.order.join(", ")
    };
    println!("Routing order: [{order}]");
    if !routing.expanded_order.is_empty() && routing.expanded_order != routing.order {
        println!("Expanded order: [{}]", routing.expanded_order.join(", "));
    }
    println!("On exhausted: {}", routing.on_exhausted);

    let providers = explain_v4_providers(view);
    if providers.is_empty() {
        println!("Providers: <empty>");
    } else {
        println!("Providers:");
        for provider in &providers {
            let marker = if provider.target { "*" } else { " " };
            let enabled = if provider.enabled { "on" } else { "off" };
            let index = provider
                .routing_index
                .map(|idx| idx.to_string())
                .unwrap_or_else(|| "-".to_string());
            println!(
                "  {} {} {} (order={}, endpoints={}, tags={})",
                marker,
                enabled,
                provider.name,
                index,
                provider.endpoints.len(),
                if provider.tags.is_empty() {
                    "-".to_string()
                } else {
                    provider
                        .tags
                        .iter()
                        .map(|(key, value)| format!("{key}={value}"))
                        .collect::<Vec<_>>()
                        .join(",")
                }
            );
        }
    }

    if let Some(provider_name) = provider_name {
        let provider = explain_v4_provider(view, provider_name)
            .ok_or_else(|| anyhow::anyhow!("provider '{}' not found", provider_name))?;
        println!("Provider '{}': enabled={}", provider.name, provider.enabled);
        if provider.endpoints.is_empty() {
            println!("  <no endpoints>");
        } else {
            for endpoint in provider.endpoints {
                println!(
                    "  [{}] {} enabled={}",
                    endpoint.name, endpoint.base_url, endpoint.enabled
                );
            }
        }
    }

    Ok(())
}

fn build_group_explain(
    mgr: &ServiceConfigManager,
    provider_name: Option<&str>,
) -> anyhow::Result<Option<ConfigExplainProvider>> {
    let Some(provider_name) = provider_name else {
        return Ok(None);
    };

    let svc = mgr
        .configs
        .get(provider_name)
        .ok_or_else(|| anyhow::anyhow!("provider '{}' not found", provider_name))?;
    Ok(Some(ConfigExplainProvider {
        name: provider_name.to_string(),
        alias: svc.alias.clone(),
        enabled: svc.enabled,
        level: svc.level.clamp(1, 10),
        upstreams: svc.upstreams.iter().map(|up| up.base_url.clone()).collect(),
    }))
}

fn print_explain_text(
    label: &str,
    schema_version: u32,
    routing: &ServiceRoutingExplanation,
    provider: Option<&ConfigExplainProvider>,
) {
    println!("Schema version: v{}", schema_version);
    println!("Service: {}", label);
    println!(
        "Active provider: {}",
        routing.active_station.as_deref().unwrap_or("<none>")
    );
    println!("Routing mode: {}", routing.mode);

    if routing.eligible_stations.is_empty() {
        println!("Candidate order: <empty>");
    } else {
        println!("Candidate order:");
        for (idx, candidate) in routing.eligible_stations.iter().enumerate() {
            let active = if candidate.active { " preferred" } else { "" };
            if let Some(alias) = candidate.alias.as_deref() {
                println!(
                    "  {}. {}{} (alias={}, priority={}, enabled={}, upstreams={})",
                    idx + 1,
                    candidate.name,
                    active,
                    alias,
                    candidate.level,
                    candidate.enabled,
                    candidate.upstreams
                );
            } else {
                println!(
                    "  {}. {}{} (priority={}, enabled={}, upstreams={})",
                    idx + 1,
                    candidate.name,
                    active,
                    candidate.level,
                    candidate.enabled,
                    candidate.upstreams
                );
            }
        }
    }

    if let Some(fallback) = &routing.fallback_station {
        println!(
            "Fallback: {} (priority={}, enabled={}, upstreams={})",
            fallback.name, fallback.level, fallback.enabled, fallback.upstreams
        );
    }

    if let Some(provider) = provider {
        println!(
            "Provider '{}': priority={} enabled={} upstreams={}",
            provider.name,
            provider.level,
            provider.enabled,
            provider.upstreams.len()
        );
        if provider.upstreams.is_empty() {
            println!("  <no upstreams>");
        } else {
            for (idx, upstream) in provider.upstreams.iter().enumerate() {
                println!("  [{}] {}", idx, upstream);
            }
        }
    }
}

pub async fn handle_route_view_cmd(cmd: RoutingCommand) -> CliResult<()> {
    match cmd {
        RoutingCommand::List { codex, claude } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let document = load_config_document()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            if let ConfigDocument::V4(cfg) = &document {
                let (view, label) = select_v4_service_view(cfg, service);
                print_v4_provider_list(label, view);
                return Ok(());
            }

            let runtime = document
                .runtime()
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let (mgr, label) = if service == "claude" {
                (&runtime.claude, "Claude")
            } else {
                (&runtime.codex, "Codex")
            };
            let cfg_path = config_file_path();

            if mgr.configs.is_empty() {
                println!("No {} providers in {:?}", label, cfg_path);
            } else {
                let active = mgr.active.clone();
                println!("{} providers (from {:?}):", label, cfg_path);
                let mut items = mgr
                    .configs
                    .iter()
                    .map(|(name, svc)| (name.as_str(), svc))
                    .collect::<Vec<_>>();
                items.sort_by(|(a_name, a), (b_name, b)| {
                    let a_level = a.level.clamp(1, 10);
                    let b_level = b.level.clamp(1, 10);
                    a_level.cmp(&b_level).then_with(|| a_name.cmp(b_name))
                });

                for (name, service_cfg) in items {
                    let marker = if active.as_deref() == Some(name) {
                        "*"
                    } else {
                        " "
                    };
                    let enabled = if service_cfg.enabled { "on" } else { "off" };
                    let level = service_cfg.level.clamp(1, 10);
                    if let Some(alias) = &service_cfg.alias {
                        println!(
                            "  {} L{} {} {} [{}] ({} upstreams)",
                            marker,
                            level,
                            enabled,
                            name,
                            alias,
                            service_cfg.upstreams.len()
                        );
                    } else {
                        println!(
                            "  {} L{} {} {} ({} upstreams)",
                            marker,
                            level,
                            enabled,
                            name,
                            service_cfg.upstreams.len()
                        );
                    }
                }
            }
        }

        RoutingCommand::Explain {
            codex,
            claude,
            json,
            provider,
        } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let document = load_config_document()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            if let ConfigDocument::V4(cfg) = &document {
                let (view, label) = select_v4_service_view(cfg, service);
                if json {
                    let provider = if let Some(provider_name) = provider.as_deref() {
                        Some(explain_v4_provider(view, provider_name).ok_or_else(|| {
                            CliError::ProxyConfig(format!("provider '{}' not found", provider_name))
                        })?)
                    } else {
                        None
                    };
                    let payload = ConfigExplainV4Payload {
                        schema_version: document.schema_version(),
                        service: service.to_string(),
                        routing: explain_v4_routing(view),
                        providers: explain_v4_providers(view),
                        provider,
                    };
                    let text = serde_json::to_string_pretty(&payload)
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                    println!("{text}");
                } else {
                    print_v4_explain_text(label, view, provider.as_deref())
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                }
                return Ok(());
            }

            let runtime = document
                .runtime()
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let (mgr, label) = select_service_manager(&runtime, service);
            let routing = explain_service_routing(mgr);
            let group_detail = build_group_explain(mgr, provider.as_deref())
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            if json {
                let payload = ConfigExplainPayload {
                    schema_version: document.schema_version(),
                    service: service.to_string(),
                    active_provider: mgr.active.clone(),
                    routing,
                    provider: group_detail,
                };
                let text = serde_json::to_string_pretty(&payload)
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                println!("{text}");
            } else {
                print_explain_text(
                    label,
                    document.schema_version(),
                    &routing,
                    group_detail.as_ref(),
                );
            }
        }
        _ => unreachable!("route view handles only routing list/explain"),
    }

    Ok(())
}

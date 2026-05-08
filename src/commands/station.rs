use super::config_doc::{
    ConfigDocument, load_config_document, ordered_v3_provider_names, print_v3_provider_list,
    resolve_service, routing_exhausted_label, routing_policy_label, select_service_manager,
    select_v3_service_view,
};
use crate::config::{
    ServiceConfigManager, ServiceRoutingExplanation, ServiceViewV3, explain_service_routing,
    storage::config_file_path,
};
use crate::{CliError, CliResult, StationCommand};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
struct ConfigExplainStation {
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
    active_station: Option<String>,
    routing: ServiceRoutingExplanation,
    station: Option<ConfigExplainStation>,
}

#[derive(Debug, Serialize, Clone)]
struct ConfigExplainV3ProviderEndpoint {
    name: String,
    base_url: String,
    enabled: bool,
}

#[derive(Debug, Serialize, Clone)]
struct ConfigExplainV3Provider {
    name: String,
    alias: Option<String>,
    enabled: bool,
    routing_index: Option<usize>,
    target: bool,
    tags: BTreeMap<String, String>,
    endpoints: Vec<ConfigExplainV3ProviderEndpoint>,
}

#[derive(Debug, Serialize)]
struct ConfigExplainV3Routing {
    policy: &'static str,
    target: Option<String>,
    order: Vec<String>,
    prefer_tags: Vec<BTreeMap<String, String>>,
    on_exhausted: &'static str,
}

#[derive(Debug, Serialize)]
struct ConfigExplainV3Payload {
    schema_version: u32,
    service: String,
    routing: ConfigExplainV3Routing,
    providers: Vec<ConfigExplainV3Provider>,
    provider: Option<ConfigExplainV3Provider>,
}

fn explain_v3_routing(view: &ServiceViewV3) -> ConfigExplainV3Routing {
    let routing = view.routing.clone().unwrap_or_default();
    ConfigExplainV3Routing {
        policy: routing_policy_label(routing.policy),
        target: routing.target,
        order: routing.order,
        prefer_tags: routing.prefer_tags,
        on_exhausted: routing_exhausted_label(routing.on_exhausted),
    }
}

fn explain_v3_provider(
    view: &ServiceViewV3,
    provider_name: &str,
) -> Option<ConfigExplainV3Provider> {
    let provider = view.providers.get(provider_name)?;
    let routing = view.routing.as_ref();
    let routing_index = routing
        .and_then(|routing| {
            routing
                .order
                .iter()
                .position(|candidate| candidate == provider_name)
        })
        .map(|idx| idx + 1);
    let target = routing
        .and_then(|routing| routing.target.as_deref())
        .is_some_and(|target| target == provider_name);

    let mut endpoints = Vec::new();
    if let Some(base_url) = provider
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        endpoints.push(ConfigExplainV3ProviderEndpoint {
            name: "default".to_string(),
            base_url: base_url.to_string(),
            enabled: provider.enabled,
        });
    }
    endpoints.extend(provider.endpoints.iter().map(|(endpoint_name, endpoint)| {
        ConfigExplainV3ProviderEndpoint {
            name: endpoint_name.clone(),
            base_url: endpoint.base_url.clone(),
            enabled: endpoint.enabled,
        }
    }));

    Some(ConfigExplainV3Provider {
        name: provider_name.to_string(),
        alias: provider.alias.clone(),
        enabled: provider.enabled,
        routing_index,
        target,
        tags: provider.tags.clone(),
        endpoints,
    })
}

fn explain_v3_providers(view: &ServiceViewV3) -> Vec<ConfigExplainV3Provider> {
    ordered_v3_provider_names(view)
        .into_iter()
        .filter_map(|provider_name| explain_v3_provider(view, provider_name.as_str()))
        .collect()
}

fn print_v3_explain_text(
    label: &str,
    view: &ServiceViewV3,
    provider_name: Option<&str>,
) -> anyhow::Result<()> {
    let routing = explain_v3_routing(view);
    println!("Schema version: v3");
    println!("Service: {label}");
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
    println!("On exhausted: {}", routing.on_exhausted);

    let providers = explain_v3_providers(view);
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
        let provider = explain_v3_provider(view, provider_name)
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
    station_name: Option<&str>,
) -> anyhow::Result<Option<ConfigExplainStation>> {
    let Some(station_name) = station_name else {
        return Ok(None);
    };

    let svc = mgr
        .configs
        .get(station_name)
        .ok_or_else(|| anyhow::anyhow!("station '{}' not found", station_name))?;
    Ok(Some(ConfigExplainStation {
        name: station_name.to_string(),
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
    station: Option<&ConfigExplainStation>,
) {
    println!("Schema version: v{}", schema_version);
    println!("Service: {}", label);
    println!(
        "Active station: {}",
        routing.active_station.as_deref().unwrap_or("<none>")
    );
    println!("Routing mode: {}", routing.mode);

    if routing.eligible_stations.is_empty() {
        println!("Candidate order: <empty>");
    } else {
        println!("Candidate order:");
        for (idx, candidate) in routing.eligible_stations.iter().enumerate() {
            let active = if candidate.active { " active" } else { "" };
            if let Some(alias) = candidate.alias.as_deref() {
                println!(
                    "  {}. {}{} (alias={}, level={}, enabled={}, upstreams={})",
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
                    "  {}. {}{} (level={}, enabled={}, upstreams={})",
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
            "Fallback: {} (level={}, enabled={}, upstreams={})",
            fallback.name, fallback.level, fallback.enabled, fallback.upstreams
        );
    }

    if let Some(station) = station {
        println!(
            "Station '{}': level={} enabled={} upstreams={}",
            station.name,
            station.level,
            station.enabled,
            station.upstreams.len()
        );
        if station.upstreams.is_empty() {
            println!("  <no upstreams>");
        } else {
            for (idx, upstream) in station.upstreams.iter().enumerate() {
                println!("  [{}] {}", idx, upstream);
            }
        }
    }
}

pub async fn handle_station_cmd(cmd: StationCommand) -> CliResult<()> {
    match cmd {
        StationCommand::List { codex, claude } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let document = load_config_document()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            if let ConfigDocument::V3(cfg) = &document {
                let (view, label) = select_v3_service_view(cfg, service);
                print_v3_provider_list(label, view);
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
                println!("No {} stations in {:?}", label, cfg_path);
            } else {
                let active = mgr.active.clone();
                println!("{} stations (from {:?}):", label, cfg_path);
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

        StationCommand::Explain {
            codex,
            claude,
            json,
            station,
        } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let document = load_config_document()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            if let ConfigDocument::V3(cfg) = &document {
                let (view, label) = select_v3_service_view(cfg, service);
                if json {
                    let provider = if let Some(provider_name) = station.as_deref() {
                        Some(explain_v3_provider(view, provider_name).ok_or_else(|| {
                            CliError::ProxyConfig(format!("provider '{}' not found", provider_name))
                        })?)
                    } else {
                        None
                    };
                    let payload = ConfigExplainV3Payload {
                        schema_version: document.schema_version(),
                        service: service.to_string(),
                        routing: explain_v3_routing(view),
                        providers: explain_v3_providers(view),
                        provider,
                    };
                    let text = serde_json::to_string_pretty(&payload)
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                    println!("{text}");
                } else {
                    print_v3_explain_text(label, view, station.as_deref())
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                }
                return Ok(());
            }

            let runtime = document
                .runtime()
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let (mgr, label) = select_service_manager(&runtime, service);
            let routing = explain_service_routing(mgr);
            let group_detail = build_group_explain(mgr, station.as_deref())
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            if json {
                let payload = ConfigExplainPayload {
                    schema_version: document.schema_version(),
                    service: service.to_string(),
                    active_station: mgr.active.clone(),
                    routing,
                    station: group_detail,
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
    }

    Ok(())
}

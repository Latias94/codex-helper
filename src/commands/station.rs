use super::config_doc::{
    ConfigDocument, ensure_v3_routing, ensure_v3_routing_order_contains, load_config_document,
    ordered_v3_provider_names, parse_cli_tags, print_v3_provider_list, resolve_service,
    routing_exhausted_label, routing_policy_label, select_service_manager, select_v3_service_view,
    select_v3_service_view_mut,
};
use crate::cli_types::ConfigSchemaTarget;
use crate::config::{
    ProviderConfigV3, RetryConfig, RetryProfileName, RoutingPolicyV3, ServiceConfig,
    ServiceConfigManager, ServiceRoutingExplanation, ServiceViewV3, UpstreamAuth, UpstreamConfig,
    bootstrap::{
        import_codex_config_from_codex_cli, overwrite_codex_config_from_codex_cli_in_place,
    },
    compact_v2_config, explain_service_routing,
    storage::{
        config_file_path, init_config_toml, load_config, save_config, save_config_v2,
        save_config_v3,
    },
};
use crate::{CliError, CliResult, RetryProfile, StationCommand};
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

fn print_migration_warnings(warnings: &[String]) {
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
}

pub async fn handle_station_cmd(cmd: StationCommand) -> CliResult<()> {
    match cmd {
        StationCommand::Init { force, no_import } => {
            let path = init_config_toml(force, !no_import)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            println!("Wrote TOML config template to {:?}", path);
        }
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
        StationCommand::Add {
            name,
            base_url,
            auth_token,
            auth_token_env,
            api_key,
            api_key_env,
            alias,
            tags,
            level,
            disabled,
            codex,
            claude,
        } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let parsed_tags =
                parse_cli_tags(&tags).map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let document = load_config_document()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            if let ConfigDocument::V3(mut cfg) = document {
                let (view, label) = select_v3_service_view_mut(&mut cfg, service);
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
                save_config_v3(&cfg)
                    .await
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                if level != 1 {
                    eprintln!(
                        "warning: --level is ignored for v3 routing configs; edit routing.order to control priority."
                    );
                }
                println!("Added {label} provider '{}' to v3 routing config", name);
                return Ok(());
            }

            let mut cfg = load_config()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            let upstream = UpstreamConfig {
                base_url,
                auth: UpstreamAuth {
                    auth_token,
                    auth_token_env,
                    api_key,
                    api_key_env,
                },
                tags: parsed_tags.into_iter().collect(),
                supported_models: Default::default(),
                model_mapping: Default::default(),
            };
            let service_cfg = ServiceConfig {
                name: name.clone(),
                alias,
                enabled: !disabled,
                level: level.clamp(1, 10),
                upstreams: vec![upstream],
            };

            if service == "claude" {
                cfg.claude.configs.insert(name.clone(), service_cfg);
                if cfg.claude.active.is_none() {
                    cfg.claude.active = Some(name.clone());
                }
                save_config(&cfg)
                    .await
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                println!("Added Claude station '{}'", name);
            } else {
                cfg.codex.configs.insert(name.clone(), service_cfg);
                if cfg.codex.active.is_none() {
                    cfg.codex.active = Some(name.clone());
                }
                save_config(&cfg)
                    .await
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                println!("Added Codex station '{}'", name);
            }
        }
        StationCommand::SetActive {
            name,
            codex,
            claude,
        } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let document = load_config_document()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            if let ConfigDocument::V3(mut cfg) = document {
                let (view, label) = select_v3_service_view_mut(&mut cfg, service);
                let Some(provider) = view.providers.get_mut(name.as_str()) else {
                    println!("{label} provider '{}' not found", name);
                    return Ok(());
                };
                provider.enabled = true;
                ensure_v3_routing_order_contains(view, name.as_str());
                let routing = ensure_v3_routing(view);
                routing.policy = RoutingPolicyV3::ManualSticky;
                routing.target = Some(name.clone());
                save_config_v3(&cfg)
                    .await
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                println!("{label} routing target pinned to provider '{}'", name);
                return Ok(());
            }

            let mut cfg = load_config()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            if service == "claude" {
                if !cfg.claude.configs.contains_key(&name) {
                    println!("Claude station '{}' not found", name);
                } else {
                    cfg.claude.active = Some(name.clone());
                    save_config(&cfg)
                        .await
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                    println!("Active Claude station set to '{}'", name);
                }
            } else if !cfg.codex.configs.contains_key(&name) {
                println!("Codex station '{}' not found", name);
            } else {
                cfg.codex.active = Some(name.clone());
                save_config(&cfg)
                    .await
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                println!("Active Codex station set to '{}'", name);
            }
        }
        StationCommand::SetLevel {
            name,
            level,
            codex,
            claude,
        } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            if !(1..=10).contains(&level) {
                return Err(CliError::ProxyConfig(
                    "level must be in range 1..=10".to_string(),
                ));
            }
            let document = load_config_document()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            if matches!(document, ConfigDocument::V3(_)) {
                return Err(CliError::ProxyConfig(
                    "v3 routing configs do not have station levels; edit routing.order or use station set-active to pin a provider.".to_string(),
                ));
            }

            let mut cfg = load_config()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let mgr = if service == "claude" {
                &mut cfg.claude
            } else {
                &mut cfg.codex
            };

            let Some(svc) = mgr.configs.get_mut(&name) else {
                println!(
                    "{} station '{}' not found",
                    if service == "claude" {
                        "Claude"
                    } else {
                        "Codex"
                    },
                    name
                );
                return Ok(());
            };
            svc.level = level;
            save_config(&cfg)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            println!(
                "Set {} station '{}' level to {}",
                if service == "claude" {
                    "Claude"
                } else {
                    "Codex"
                },
                name,
                level
            );
        }
        StationCommand::Enable {
            name,
            codex,
            claude,
        } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let document = load_config_document()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            if let ConfigDocument::V3(mut cfg) = document {
                let (view, label) = select_v3_service_view_mut(&mut cfg, service);
                let Some(provider) = view.providers.get_mut(name.as_str()) else {
                    println!("{label} provider '{}' not found", name);
                    return Ok(());
                };
                provider.enabled = true;
                ensure_v3_routing_order_contains(view, name.as_str());
                save_config_v3(&cfg)
                    .await
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                println!("Enabled {label} provider '{}'", name);
                return Ok(());
            }

            let mut cfg = load_config()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let mgr = if service == "claude" {
                &mut cfg.claude
            } else {
                &mut cfg.codex
            };

            let Some(svc) = mgr.configs.get_mut(&name) else {
                println!(
                    "{} station '{}' not found",
                    if service == "claude" {
                        "Claude"
                    } else {
                        "Codex"
                    },
                    name
                );
                return Ok(());
            };
            svc.enabled = true;
            save_config(&cfg)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            println!(
                "Enabled {} station '{}'",
                if service == "claude" {
                    "Claude"
                } else {
                    "Codex"
                },
                name
            );
        }
        StationCommand::Disable {
            name,
            codex,
            claude,
        } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let document = load_config_document()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            if let ConfigDocument::V3(mut cfg) = document {
                let (view, label) = select_v3_service_view_mut(&mut cfg, service);
                let Some(provider) = view.providers.get_mut(name.as_str()) else {
                    println!("{label} provider '{}' not found", name);
                    return Ok(());
                };
                provider.enabled = false;

                let routing = ensure_v3_routing(view);
                let was_target = routing.target.as_deref() == Some(name.as_str());
                if was_target {
                    routing.policy = RoutingPolicyV3::OrderedFailover;
                    routing.target = None;
                }
                save_config_v3(&cfg)
                    .await
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

                if was_target {
                    println!(
                        "Disabled {label} provider '{}' and cleared manual routing target",
                        name
                    );
                } else {
                    println!("Disabled {label} provider '{}'", name);
                }
                return Ok(());
            }

            let mut cfg = load_config()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let is_active = {
                let mgr = if service == "claude" {
                    &mut cfg.claude
                } else {
                    &mut cfg.codex
                };

                let Some(svc) = mgr.configs.get_mut(&name) else {
                    println!(
                        "{} station '{}' not found",
                        if service == "claude" {
                            "Claude"
                        } else {
                            "Codex"
                        },
                        name
                    );
                    return Ok(());
                };
                svc.enabled = false;
                mgr.active.as_deref() == Some(name.as_str())
            };

            save_config(&cfg)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            if is_active {
                println!(
                    "Disabled {} station '{}' (note: active station is still eligible for routing)",
                    if service == "claude" {
                        "Claude"
                    } else {
                        "Codex"
                    },
                    name
                );
            } else {
                println!(
                    "Disabled {} station '{}'",
                    if service == "claude" {
                        "Claude"
                    } else {
                        "Codex"
                    },
                    name
                );
            }
        }
        StationCommand::SetRetryProfile { profile } => {
            let mut cfg = load_config()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            let profile_name = match profile {
                RetryProfile::Balanced => RetryProfileName::Balanced,
                RetryProfile::SameUpstream => RetryProfileName::SameUpstream,
                RetryProfile::AggressiveFailover => RetryProfileName::AggressiveFailover,
                RetryProfile::CostPrimary => RetryProfileName::CostPrimary,
            };

            // Apply profile and clear explicit per-field overrides to keep config minimal.
            cfg.retry = RetryConfig {
                profile: Some(profile_name),
                ..RetryConfig::default()
            };

            save_config(&cfg)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            println!("Set retry profile to '{:?}'", profile);
            let resolved = cfg.retry.resolve();
            println!(
                "retry: upstream(strategy={:?} max_attempts={} backoff={}..{} jitter={}) route(strategy={:?} max_attempts={}) guardrails(never_on_status='{}' never_on_class={:?}) cooldown(cf_chal={}s cf_to={}s transport={}s) cooldown_backoff(factor={} max={}s)",
                resolved.upstream.strategy,
                resolved.upstream.max_attempts,
                resolved.upstream.backoff_ms,
                resolved.upstream.backoff_max_ms,
                resolved.upstream.jitter_ms,
                resolved.route.strategy,
                resolved.route.max_attempts,
                resolved.never_on_status,
                resolved.never_on_class,
                resolved.cloudflare_challenge_cooldown_secs,
                resolved.cloudflare_timeout_cooldown_secs,
                resolved.transport_cooldown_secs,
                resolved.cooldown_backoff_factor,
                resolved.cooldown_backoff_max_secs,
            );
        }
        StationCommand::ImportFromCodex { force } => {
            let cfg = import_codex_config_from_codex_cli(force)
                .await
                .map_err(|e| CliError::CodexConfig(e.to_string()))?;
            if cfg.codex.configs.is_empty() {
                println!(
                    "No Codex stations were imported from ~/.codex; please ensure ~/.codex/config.toml and ~/.codex/auth.json are valid."
                );
            } else {
                let names: Vec<_> = cfg.codex.configs.keys().cloned().collect();
                println!(
                    "Imported Codex stations from ~/.codex (force = {}): {:?}",
                    force, names
                );
            }
        }
        StationCommand::OverwriteFromCodex { dry_run, yes } => {
            if !dry_run && !yes {
                return Err(CliError::ProxyConfig(
                    "该操作会覆盖并重建 Codex 站点配置（active/enabled/level 会重置），请使用 --yes 确认，或先用 --dry-run 预览".to_string(),
                ));
            }
            let cfg = load_config()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            let mut working = if dry_run { cfg.clone() } else { cfg };
            overwrite_codex_config_from_codex_cli_in_place(&mut working)
                .map_err(|e| CliError::CodexConfig(e.to_string()))?;

            if dry_run {
                println!("Dry-run: no files written.");
            } else {
                save_config(&working)
                    .await
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            }

            let names: Vec<_> = working.codex.configs.keys().cloned().collect();
            println!(
                "Overwrote Codex stations from ~/.codex (dry_run = {}): {:?}",
                dry_run, names
            );
        }
        StationCommand::Migrate {
            to,
            dry_run,
            write,
            compact,
            yes,
        } => {
            if write && !yes {
                return Err(CliError::ProxyConfig(
                    "This will overwrite ~/.codex-helper/config.toml; use --yes to confirm."
                        .to_string(),
                ));
            }

            let preview = dry_run || !write;
            match to {
                ConfigSchemaTarget::V2 => {
                    let document = load_config_document()
                        .await
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                    let v2_view = document
                        .v2_view()
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                    let migrated = if compact {
                        compact_v2_config(&v2_view)
                            .map_err(|e| CliError::ProxyConfig(e.to_string()))?
                    } else {
                        v2_view
                    };

                    if preview {
                        let text = toml::to_string_pretty(&migrated)
                            .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                        println!("{text}");
                    } else {
                        let path = save_config_v2(&migrated)
                            .await
                            .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                        println!("Migrated config written to {:?}", path);
                    }
                }
                ConfigSchemaTarget::V3 => {
                    let document = load_config_document()
                        .await
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                    let report = document
                        .v3_migration_report()
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                    print_migration_warnings(&report.warnings);

                    if preview {
                        let text = toml::to_string_pretty(&report.config)
                            .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                        println!("{text}");
                    } else {
                        let path = save_config_v3(&report.config)
                            .await
                            .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                        println!("Migrated config written to {:?}", path);
                    }
                }
            }
        }
    }

    Ok(())
}

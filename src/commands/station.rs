use crate::cli_types::ConfigSchemaTarget;
use crate::config::{
    ConfigV3MigrationReport, ProviderConfigV3, ProxyConfig, ProxyConfigV2, ProxyConfigV3,
    RetryConfig, RetryProfileName, RoutingConfigV3, RoutingExhaustedActionV3, RoutingPolicyV3,
    ServiceConfig, ServiceConfigManager, ServiceKind, ServiceRoutingExplanation, ServiceViewV3,
    UpstreamAuth, UpstreamConfig,
    bootstrap::{
        import_codex_config_from_codex_cli, overwrite_codex_config_from_codex_cli_in_place,
    },
    compact_v2_config, compile_v2_to_runtime, compile_v3_to_runtime, explain_service_routing,
    migrate_legacy_to_v2, migrate_legacy_to_v3_with_report, migrate_v2_to_v3_with_report,
    storage::{
        config_file_path, init_config_toml, load_config, save_config, save_config_v2,
        save_config_v3,
    },
};
use crate::{CliError, CliResult, RetryProfile, StationCommand};
use serde::Serialize;
use std::collections::BTreeMap;
use tokio::fs;

async fn resolve_service(codex: bool, claude: bool) -> anyhow::Result<&'static str> {
    if codex && claude {
        anyhow::bail!("Please specify at most one of --codex / --claude");
    }
    if codex {
        return Ok("codex");
    }
    if claude {
        return Ok("claude");
    }

    // 未显式指定时，根据配置中的 default_service 决定默认服务（缺省为 Codex）。
    match load_config().await {
        Ok(cfg) => match cfg.default_service {
            Some(ServiceKind::Claude) => Ok("claude"),
            _ => Ok("codex"),
        },
        Err(_) => Ok("codex"),
    }
}

#[derive(Debug, Clone)]
enum ConfigDocument {
    Legacy(ProxyConfig),
    V2(ProxyConfigV2),
    V3(ProxyConfigV3),
}

impl ConfigDocument {
    fn schema_version(&self) -> u32 {
        match self {
            Self::Legacy(cfg) => cfg.version.unwrap_or(1),
            Self::V2(cfg) => cfg.version,
            Self::V3(cfg) => cfg.version,
        }
    }

    fn runtime(&self) -> anyhow::Result<ProxyConfig> {
        match self {
            Self::Legacy(cfg) => Ok(cfg.clone()),
            Self::V2(cfg) => compile_v2_to_runtime(cfg),
            Self::V3(cfg) => compile_v3_to_runtime(cfg),
        }
    }

    fn v2_view(&self) -> anyhow::Result<ProxyConfigV2> {
        match self {
            Self::Legacy(cfg) => Ok(migrate_legacy_to_v2(cfg)),
            Self::V2(cfg) => Ok(cfg.clone()),
            Self::V3(cfg) => crate::config::compile_v3_to_v2(cfg),
        }
    }

    fn v3_migration_report(&self) -> anyhow::Result<ConfigV3MigrationReport> {
        match self {
            Self::Legacy(cfg) => migrate_legacy_to_v3_with_report(cfg),
            Self::V2(cfg) => migrate_v2_to_v3_with_report(cfg),
            Self::V3(cfg) => Ok(ConfigV3MigrationReport {
                config: cfg.clone(),
                warnings: Vec::new(),
            }),
        }
    }
}

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

async fn load_config_document() -> anyhow::Result<ConfigDocument> {
    let path = config_file_path();
    if !path.exists() {
        return Ok(ConfigDocument::Legacy(load_config().await?));
    }

    let is_toml = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"));
    if !is_toml {
        return Ok(ConfigDocument::Legacy(load_config().await?));
    }

    let text = fs::read_to_string(&path).await?;
    let value = toml::from_str::<toml::Value>(&text).ok();
    let version = value
        .as_ref()
        .and_then(|value| value.get("version").and_then(|v| v.as_integer()))
        .map(|value| value as u32)
        .or_else(|| {
            let has_routing = ["codex", "claude"].iter().any(|service| {
                value
                    .as_ref()
                    .and_then(|value| value.get(*service))
                    .and_then(|service| service.get("routing"))
                    .is_some()
            });
            if has_routing { Some(3) } else { None }
        });

    if version == Some(3) {
        let cfg = toml::from_str::<ProxyConfigV3>(&text)?;
        compile_v3_to_runtime(&cfg)?;
        Ok(ConfigDocument::V3(cfg))
    } else if version == Some(2) {
        let cfg = toml::from_str::<ProxyConfigV2>(&text)?;
        compile_v2_to_runtime(&cfg)?;
        Ok(ConfigDocument::V2(cfg))
    } else {
        Ok(ConfigDocument::Legacy(load_config().await?))
    }
}

fn select_service_manager<'a>(
    cfg: &'a ProxyConfig,
    service: &str,
) -> (&'a ServiceConfigManager, &'static str) {
    if service == "claude" {
        (&cfg.claude, "Claude")
    } else {
        (&cfg.codex, "Codex")
    }
}

fn select_v3_service_view_mut<'a>(
    cfg: &'a mut ProxyConfigV3,
    service: &str,
) -> (&'a mut ServiceViewV3, &'static str) {
    if service == "claude" {
        (&mut cfg.claude, "Claude")
    } else {
        (&mut cfg.codex, "Codex")
    }
}

fn select_v3_service_view<'a>(
    cfg: &'a ProxyConfigV3,
    service: &str,
) -> (&'a ServiceViewV3, &'static str) {
    if service == "claude" {
        (&cfg.claude, "Claude")
    } else {
        (&cfg.codex, "Codex")
    }
}

fn ensure_v3_routing(view: &mut ServiceViewV3) -> &mut RoutingConfigV3 {
    view.routing.get_or_insert_with(RoutingConfigV3::default)
}

fn ensure_v3_routing_order_contains(view: &mut ServiceViewV3, provider_name: &str) {
    let routing = ensure_v3_routing(view);
    if !routing
        .order
        .iter()
        .any(|candidate| candidate == provider_name)
    {
        routing.order.push(provider_name.to_string());
    }
}

fn routing_policy_label(policy: RoutingPolicyV3) -> &'static str {
    match policy {
        RoutingPolicyV3::ManualSticky => "manual-sticky",
        RoutingPolicyV3::OrderedFailover => "ordered-failover",
        RoutingPolicyV3::TagPreferred => "tag-preferred",
    }
}

fn routing_exhausted_label(action: RoutingExhaustedActionV3) -> &'static str {
    match action {
        RoutingExhaustedActionV3::Continue => "continue",
        RoutingExhaustedActionV3::Stop => "stop",
    }
}

fn v3_provider_endpoint_count(provider: &ProviderConfigV3) -> usize {
    let inline = provider
        .base_url
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty()) as usize;
    inline + provider.endpoints.len()
}

fn push_v3_provider_name_once(names: &mut Vec<String>, view: &ServiceViewV3, name: &str) {
    if view.providers.contains_key(name) && !names.iter().any(|existing| existing == name) {
        names.push(name.to_string());
    }
}

fn ordered_v3_provider_names(view: &ServiceViewV3) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(routing) = view.routing.as_ref() {
        if let Some(target) = routing.target.as_deref() {
            push_v3_provider_name_once(&mut names, view, target);
        }
        for provider_name in &routing.order {
            push_v3_provider_name_once(&mut names, view, provider_name);
        }
    }
    for provider_name in view.providers.keys() {
        push_v3_provider_name_once(&mut names, view, provider_name);
    }
    names
}

fn print_v3_provider_list(label: &str, view: &ServiceViewV3) {
    let provider_names = ordered_v3_provider_names(view);
    if view.providers.is_empty() {
        println!("No {label} providers in v3 config.");
        return;
    }

    if let Some(routing) = view.routing.as_ref() {
        let target = routing.target.as_deref().unwrap_or("<none>");
        let order = if routing.order.is_empty() {
            "<provider key order>".to_string()
        } else {
            routing.order.join(", ")
        };
        println!(
            "{label} providers (v3): policy={} target={} order=[{}] on_exhausted={}",
            routing_policy_label(routing.policy),
            target,
            order,
            routing_exhausted_label(routing.on_exhausted)
        );
    } else {
        println!("{label} providers (v3): routing=<implicit ordered-failover>");
    }

    let target = view
        .routing
        .as_ref()
        .and_then(|routing| routing.target.as_deref());
    let first_ordered = view
        .routing
        .as_ref()
        .and_then(|routing| routing.order.first().map(String::as_str));

    for provider_name in provider_names {
        let Some(provider) = view.providers.get(provider_name.as_str()) else {
            continue;
        };
        let marker = if target == Some(provider_name.as_str())
            || (target.is_none() && first_ordered == Some(provider_name.as_str()))
        {
            "*"
        } else {
            " "
        };
        let enabled = if provider.enabled { "on" } else { "off" };
        let endpoints = v3_provider_endpoint_count(provider);
        let tags = if provider.tags.is_empty() {
            "-".to_string()
        } else {
            provider
                .tags
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(",")
        };
        if let Some(alias) = provider.alias.as_deref() {
            println!(
                "  {} {} {} [{}] ({} endpoints, tags={})",
                marker, enabled, provider_name, alias, endpoints, tags
            );
        } else {
            println!(
                "  {} {} {} ({} endpoints, tags={})",
                marker, enabled, provider_name, endpoints, tags
            );
        }
    }
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
            level,
            disabled,
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
                tags: Default::default(),
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

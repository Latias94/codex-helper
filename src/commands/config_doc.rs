use crate::config::{
    ConfigV4MigrationReport, ProviderConfigV4, ProxyConfig, ProxyConfigV2, ProxyConfigV4,
    RoutingConfigV4, RoutingExhaustedActionV4, RoutingPolicyV4, ServiceConfigManager, ServiceKind,
    ServiceViewV4, compile_v2_to_runtime, compile_v4_to_runtime, migrate_legacy_to_v4_with_report,
    migrate_v2_to_v4_with_report,
    storage::{config_file_path, load_config},
};
use std::collections::BTreeMap;
use tokio::fs;

#[derive(Debug, Clone)]
pub(super) enum ConfigDocument {
    Legacy(Box<ProxyConfig>),
    V2(Box<ProxyConfigV2>),
    V4(Box<ProxyConfigV4>),
}

impl ConfigDocument {
    pub(super) fn schema_version(&self) -> u32 {
        match self {
            Self::Legacy(cfg) => cfg.version.unwrap_or(1),
            Self::V2(cfg) => cfg.version,
            Self::V4(cfg) => cfg.version,
        }
    }

    pub(super) fn runtime(&self) -> anyhow::Result<ProxyConfig> {
        match self {
            Self::Legacy(cfg) => Ok((**cfg).clone()),
            Self::V2(cfg) => compile_v2_to_runtime(cfg),
            Self::V4(cfg) => compile_v4_to_runtime(cfg),
        }
    }

    pub(super) fn v4_migration_report(&self) -> anyhow::Result<ConfigV4MigrationReport> {
        match self {
            Self::Legacy(cfg) => migrate_legacy_to_v4_with_report(cfg),
            Self::V2(cfg) => migrate_v2_to_v4_with_report(cfg),
            Self::V4(cfg) => Ok(ConfigV4MigrationReport {
                config: (**cfg).clone(),
                warnings: Vec::new(),
            }),
        }
    }
}

pub(super) async fn resolve_service(codex: bool, claude: bool) -> anyhow::Result<&'static str> {
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

pub(super) async fn load_config_document() -> anyhow::Result<ConfigDocument> {
    let path = config_file_path();
    if !path.exists() {
        return Ok(ConfigDocument::Legacy(Box::new(load_config().await?)));
    }

    let is_toml = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"));
    if !is_toml {
        return Ok(ConfigDocument::Legacy(Box::new(load_config().await?)));
    }

    let text = fs::read_to_string(&path).await?;
    let value = toml::from_str::<toml::Value>(&text).ok();
    let version = value
        .as_ref()
        .and_then(|value| value.get("version").and_then(|v| v.as_integer()))
        .map(|value| value as u32)
        .or_else(|| {
            let has_v4_routing = ["codex", "claude"].iter().any(|service| {
                value
                    .as_ref()
                    .and_then(|value| value.get(*service))
                    .and_then(|service| service.get("routing"))
                    .and_then(|routing| routing.get("entry").or_else(|| routing.get("routes")))
                    .is_some()
            });
            if has_v4_routing {
                Some(4)
            } else {
                let has_legacy_routing = ["codex", "claude"].iter().any(|service| {
                    value
                        .as_ref()
                        .and_then(|value| value.get(*service))
                        .and_then(|service| service.get("routing"))
                        .is_some()
                });
                if has_legacy_routing { Some(3) } else { None }
            }
        });

    if version == Some(4) {
        let mut cfg = toml::from_str::<ProxyConfigV4>(&text)?;
        cfg.sync_routing_compat_from_graph();
        compile_v4_to_runtime(&cfg)?;
        Ok(ConfigDocument::V4(Box::new(cfg)))
    } else if version == Some(3) {
        let legacy = toml::from_str::<crate::config::legacy::ProxyConfigV3Legacy>(&text)?;
        let migrated = crate::config::legacy::migrate_v3_legacy_to_v4(&legacy)?;
        let mut cfg = migrated.config;
        cfg.sync_routing_compat_from_graph();
        compile_v4_to_runtime(&cfg)?;
        Ok(ConfigDocument::V4(Box::new(cfg)))
    } else if version == Some(2) {
        let cfg = toml::from_str::<ProxyConfigV2>(&text)?;
        compile_v2_to_runtime(&cfg)?;
        Ok(ConfigDocument::V2(Box::new(cfg)))
    } else {
        Ok(ConfigDocument::Legacy(Box::new(load_config().await?)))
    }
}

pub(super) async fn load_v4_config(
    codex: bool,
    claude: bool,
    command_group: &str,
) -> anyhow::Result<(ProxyConfigV4, &'static str, &'static str)> {
    let service = resolve_service(codex, claude).await?;
    let document = load_config_document().await?;
    let ConfigDocument::V4(cfg) = document else {
        anyhow::bail!(
            "{} commands require a version = 4 route graph config; run `codex-helper config migrate --write --yes` first",
            command_group
        );
    };
    let label = if service == "claude" {
        "Claude"
    } else {
        "Codex"
    };
    Ok((*cfg, service, label))
}

pub(super) fn select_service_manager<'a>(
    cfg: &'a ProxyConfig,
    service: &str,
) -> (&'a ServiceConfigManager, &'static str) {
    if service == "claude" {
        (&cfg.claude, "Claude")
    } else {
        (&cfg.codex, "Codex")
    }
}

pub(super) fn select_v4_service_view_mut<'a>(
    cfg: &'a mut ProxyConfigV4,
    service: &str,
) -> (&'a mut ServiceViewV4, &'static str) {
    if service == "claude" {
        (&mut cfg.claude, "Claude")
    } else {
        (&mut cfg.codex, "Codex")
    }
}

pub(super) fn select_v4_service_view<'a>(
    cfg: &'a ProxyConfigV4,
    service: &str,
) -> (&'a ServiceViewV4, &'static str) {
    if service == "claude" {
        (&cfg.claude, "Claude")
    } else {
        (&cfg.codex, "Codex")
    }
}

pub(super) fn ensure_v4_routing(view: &mut ServiceViewV4) -> &mut RoutingConfigV4 {
    view.routing.get_or_insert_with(RoutingConfigV4::default)
}

pub(super) fn ensure_v4_routing_order_contains(view: &mut ServiceViewV4, provider_name: &str) {
    let routing = ensure_v4_routing(view);
    let entry_name = routing.entry.clone();
    if !routing.routes.contains_key(entry_name.as_str()) {
        routing
            .routes
            .insert(entry_name.clone(), crate::config::RoutingNodeV4::default());
    }
    let node = routing
        .routes
        .get_mut(entry_name.as_str())
        .expect("entry route");
    if !node
        .children
        .iter()
        .any(|candidate| candidate == provider_name)
    {
        node.children.push(provider_name.to_string());
    }
    routing.sync_compat_from_graph();
}

pub(super) fn parse_cli_tags(raw_tags: &[String]) -> anyhow::Result<BTreeMap<String, String>> {
    let mut tags = BTreeMap::new();
    for raw in raw_tags {
        let Some((key, value)) = raw.split_once('=') else {
            anyhow::bail!("tag '{}' must use KEY=VALUE form", raw);
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() {
            anyhow::bail!("tag '{}' has an empty key", raw);
        }
        if value.is_empty() {
            anyhow::bail!("tag '{}' has an empty value", raw);
        }
        if tags.insert(key.to_string(), value.to_string()).is_some() {
            anyhow::bail!("duplicate tag key '{}'", key);
        }
    }
    Ok(tags)
}

pub(super) fn routing_policy_label(policy: RoutingPolicyV4) -> &'static str {
    match policy {
        RoutingPolicyV4::ManualSticky => "manual-sticky",
        RoutingPolicyV4::OrderedFailover => "ordered-failover",
        RoutingPolicyV4::TagPreferred => "tag-preferred",
    }
}

pub(super) fn routing_exhausted_label(action: RoutingExhaustedActionV4) -> &'static str {
    match action {
        RoutingExhaustedActionV4::Continue => "continue",
        RoutingExhaustedActionV4::Stop => "stop",
    }
}

pub(super) fn v4_provider_endpoint_count(provider: &ProviderConfigV4) -> usize {
    let inline = provider
        .base_url
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty()) as usize;
    inline + provider.endpoints.len()
}

fn push_v4_provider_name_once(names: &mut Vec<String>, view: &ServiceViewV4, name: &str) {
    if view.providers.contains_key(name) && !names.iter().any(|existing| existing == name) {
        names.push(name.to_string());
    }
}

pub(super) fn ordered_v4_provider_names(view: &ServiceViewV4) -> Vec<String> {
    let mut names =
        crate::config::resolved_v4_provider_order("config_doc", view).unwrap_or_default();
    for provider_name in view.providers.keys() {
        push_v4_provider_name_once(&mut names, view, provider_name);
    }
    names
}

pub(super) fn print_v4_provider_list(label: &str, view: &ServiceViewV4) {
    let provider_names = ordered_v4_provider_names(view);
    if view.providers.is_empty() {
        println!("No {label} providers in v4 route graph config.");
        return;
    }

    if view.routing.is_some() {
        let routing = crate::config::effective_v4_routing(view);
        let entry = routing.entry_node();
        let target = entry
            .and_then(|node| node.target.as_deref())
            .unwrap_or("<none>");
        let order = if entry.is_none_or(|node| node.children.is_empty()) {
            "<provider key order>".to_string()
        } else {
            entry
                .map(|node| node.children.join(", "))
                .unwrap_or_default()
        };
        println!(
            "{label} providers (v4): entry={} policy={} target={} order=[{}] on_exhausted={}",
            routing.entry,
            routing_policy_label(
                entry
                    .map(|node| node.strategy)
                    .unwrap_or(RoutingPolicyV4::OrderedFailover)
            ),
            target,
            order,
            routing_exhausted_label(
                entry
                    .map(|node| node.on_exhausted)
                    .unwrap_or(RoutingExhaustedActionV4::Continue)
            )
        );
    } else {
        println!("{label} providers (v4): routing=<implicit ordered-failover>");
    }

    let effective = crate::config::effective_v4_routing(view);
    let target = effective.entry_node().and_then(|node| {
        matches!(node.strategy, RoutingPolicyV4::ManualSticky)
            .then(|| node.target.as_deref())
            .flatten()
    });
    let first_ordered = crate::config::resolved_v4_provider_order("config_doc", view)
        .ok()
        .and_then(|order| order.first().cloned());

    for provider_name in provider_names {
        let Some(provider) = view.providers.get(provider_name.as_str()) else {
            continue;
        };
        let marker = if target == Some(provider_name.as_str())
            || (target.is_none() && first_ordered.as_deref() == Some(provider_name.as_str()))
        {
            "*"
        } else {
            " "
        };
        let enabled = if provider.enabled { "on" } else { "off" };
        let endpoints = v4_provider_endpoint_count(provider);
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

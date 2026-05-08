use crate::config::{
    ConfigV3MigrationReport, ProviderConfigV3, ProxyConfig, ProxyConfigV2, ProxyConfigV3,
    RoutingConfigV3, RoutingExhaustedActionV3, RoutingPolicyV3, ServiceConfigManager, ServiceKind,
    ServiceViewV3, compile_v2_to_runtime, compile_v3_to_runtime, migrate_legacy_to_v2,
    migrate_legacy_to_v3_with_report, migrate_v2_to_v3_with_report,
    storage::{config_file_path, load_config},
};
use std::collections::BTreeMap;
use tokio::fs;

#[derive(Debug, Clone)]
pub(super) enum ConfigDocument {
    Legacy(ProxyConfig),
    V2(ProxyConfigV2),
    V3(ProxyConfigV3),
}

impl ConfigDocument {
    pub(super) fn schema_version(&self) -> u32 {
        match self {
            Self::Legacy(cfg) => cfg.version.unwrap_or(1),
            Self::V2(cfg) => cfg.version,
            Self::V3(cfg) => cfg.version,
        }
    }

    pub(super) fn runtime(&self) -> anyhow::Result<ProxyConfig> {
        match self {
            Self::Legacy(cfg) => Ok(cfg.clone()),
            Self::V2(cfg) => compile_v2_to_runtime(cfg),
            Self::V3(cfg) => compile_v3_to_runtime(cfg),
        }
    }

    pub(super) fn v2_view(&self) -> anyhow::Result<ProxyConfigV2> {
        match self {
            Self::Legacy(cfg) => Ok(migrate_legacy_to_v2(cfg)),
            Self::V2(cfg) => Ok(cfg.clone()),
            Self::V3(cfg) => crate::config::compile_v3_to_v2(cfg),
        }
    }

    pub(super) fn v3_migration_report(&self) -> anyhow::Result<ConfigV3MigrationReport> {
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

pub(super) async fn load_v3_config(
    codex: bool,
    claude: bool,
    command_group: &str,
) -> anyhow::Result<(ProxyConfigV3, &'static str, &'static str)> {
    let service = resolve_service(codex, claude).await?;
    let document = load_config_document().await?;
    let ConfigDocument::V3(cfg) = document else {
        anyhow::bail!(
            "{} commands require a version = 3 config; run `codex-helper station migrate --to v3 --write --yes` first",
            command_group
        );
    };
    let label = if service == "claude" {
        "Claude"
    } else {
        "Codex"
    };
    Ok((cfg, service, label))
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

pub(super) fn select_v3_service_view_mut<'a>(
    cfg: &'a mut ProxyConfigV3,
    service: &str,
) -> (&'a mut ServiceViewV3, &'static str) {
    if service == "claude" {
        (&mut cfg.claude, "Claude")
    } else {
        (&mut cfg.codex, "Codex")
    }
}

pub(super) fn select_v3_service_view<'a>(
    cfg: &'a ProxyConfigV3,
    service: &str,
) -> (&'a ServiceViewV3, &'static str) {
    if service == "claude" {
        (&cfg.claude, "Claude")
    } else {
        (&cfg.codex, "Codex")
    }
}

pub(super) fn ensure_v3_routing(view: &mut ServiceViewV3) -> &mut RoutingConfigV3 {
    view.routing.get_or_insert_with(RoutingConfigV3::default)
}

pub(super) fn ensure_v3_routing_order_contains(view: &mut ServiceViewV3, provider_name: &str) {
    let routing = ensure_v3_routing(view);
    if !routing
        .order
        .iter()
        .any(|candidate| candidate == provider_name)
    {
        routing.order.push(provider_name.to_string());
    }
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

pub(super) fn routing_policy_label(policy: RoutingPolicyV3) -> &'static str {
    match policy {
        RoutingPolicyV3::ManualSticky => "manual-sticky",
        RoutingPolicyV3::OrderedFailover => "ordered-failover",
        RoutingPolicyV3::TagPreferred => "tag-preferred",
    }
}

pub(super) fn routing_exhausted_label(action: RoutingExhaustedActionV3) -> &'static str {
    match action {
        RoutingExhaustedActionV3::Continue => "continue",
        RoutingExhaustedActionV3::Stop => "stop",
    }
}

pub(super) fn v3_provider_endpoint_count(provider: &ProviderConfigV3) -> usize {
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

pub(super) fn ordered_v3_provider_names(view: &ServiceViewV3) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(routing) = view.routing.as_ref() {
        if matches!(routing.policy, RoutingPolicyV3::ManualSticky)
            && let Some(target) = routing.target.as_deref()
        {
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

pub(super) fn print_v3_provider_list(label: &str, view: &ServiceViewV3) {
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

    let target = view.routing.as_ref().and_then(|routing| {
        matches!(routing.policy, RoutingPolicyV3::ManualSticky)
            .then(|| routing.target.as_deref())
            .flatten()
    });
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

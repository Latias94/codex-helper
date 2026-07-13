use crate::config::{
    CURRENT_CONFIG_VERSION, HelperConfig, ProviderConfig, RouteExhaustedAction, RouteGraphConfig,
    RouteStrategy, ServiceKind, ServiceRouteConfig,
    storage::{config_file_path, load_config, load_config_with_source},
};
use std::collections::BTreeMap;

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

pub(super) async fn load_config_document() -> anyhow::Result<HelperConfig> {
    let path = config_file_path();
    anyhow::ensure!(
        path.exists(),
        "canonical version = {} config is missing at {:?}",
        CURRENT_CONFIG_VERSION,
        path
    );
    Ok(load_config_with_source().await?.source)
}

pub(super) async fn load_helper_config(
    codex: bool,
    claude: bool,
    command_group: &str,
) -> anyhow::Result<(HelperConfig, &'static str, &'static str)> {
    let service = resolve_service(codex, claude).await?;
    let cfg = load_config_document().await.map_err(|error| {
        anyhow::anyhow!(
            "{command_group} commands require the canonical version = {CURRENT_CONFIG_VERSION} route graph config: {error}"
        )
    })?;
    let label = if service == "claude" {
        "Claude"
    } else {
        "Codex"
    };
    Ok((cfg, service, label))
}

pub(super) fn select_service_route_config_mut<'a>(
    cfg: &'a mut HelperConfig,
    service: &str,
) -> (&'a mut ServiceRouteConfig, &'static str) {
    if service == "claude" {
        (&mut cfg.claude, "Claude")
    } else {
        (&mut cfg.codex, "Codex")
    }
}

pub(super) fn select_service_route_config<'a>(
    cfg: &'a HelperConfig,
    service: &str,
) -> (&'a ServiceRouteConfig, &'static str) {
    if service == "claude" {
        (&cfg.claude, "Claude")
    } else {
        (&cfg.codex, "Codex")
    }
}

pub(super) fn ensure_routing(view: &mut ServiceRouteConfig) -> &mut RouteGraphConfig {
    view.ensure_routing_mut()
}

pub(super) fn ensure_routing_order_contains(view: &mut ServiceRouteConfig, provider_name: &str) {
    view.ensure_routing_mut()
        .ensure_entry_order_contains(provider_name);
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

pub(super) fn parse_cli_string_map(
    raw_entries: &[String],
    label: &str,
) -> anyhow::Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    for raw in raw_entries {
        let Some((key, value)) = raw.split_once('=') else {
            anyhow::bail!("{} '{}' must use KEY=VALUE form", label, raw);
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() {
            anyhow::bail!("{} '{}' has an empty key", label, raw);
        }
        if value.is_empty() {
            anyhow::bail!("{} '{}' has an empty value", label, raw);
        }
        if map.insert(key.to_string(), value.to_string()).is_some() {
            anyhow::bail!("duplicate {} key '{}'", label, key);
        }
    }
    Ok(map)
}

pub(super) fn routing_policy_label(policy: RouteStrategy) -> &'static str {
    match policy {
        RouteStrategy::ManualSticky => "manual-sticky",
        RouteStrategy::OrderedFailover => "ordered-failover",
        RouteStrategy::TagPreferred => "tag-preferred",
        RouteStrategy::Conditional => "conditional",
    }
}

pub(super) fn routing_exhausted_label(action: RouteExhaustedAction) -> &'static str {
    match action {
        RouteExhaustedAction::Continue => "continue",
        RouteExhaustedAction::Stop => "stop",
    }
}

pub(super) fn provider_endpoint_count(provider: &ProviderConfig) -> usize {
    let inline = provider
        .base_url
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty()) as usize;
    inline + provider.endpoints.len()
}

fn push_provider_name_once(names: &mut Vec<String>, view: &ServiceRouteConfig, name: &str) {
    if view.providers.contains_key(name) && !names.iter().any(|existing| existing == name) {
        names.push(name.to_string());
    }
}

pub(super) fn ordered_provider_names(view: &ServiceRouteConfig) -> Vec<String> {
    let mut names = crate::config::resolved_provider_order("config_doc", view).unwrap_or_default();
    for provider_name in view.providers.keys() {
        push_provider_name_once(&mut names, view, provider_name);
    }
    names
}

pub(super) fn print_provider_list(label: &str, view: &ServiceRouteConfig) {
    let provider_names = ordered_provider_names(view);
    if view.providers.is_empty() {
        println!("No {label} providers in v{CURRENT_CONFIG_VERSION} route graph config.");
        return;
    }

    if view.routing.is_some() {
        let routing = crate::config::effective_routing(view);
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
            "{label} providers (v{CURRENT_CONFIG_VERSION}): entry={} policy={} target={} order=[{}] on_exhausted={}",
            routing.entry,
            routing_policy_label(
                entry
                    .map(|node| node.strategy)
                    .unwrap_or(RouteStrategy::OrderedFailover)
            ),
            target,
            order,
            routing_exhausted_label(
                entry
                    .map(|node| node.on_exhausted)
                    .unwrap_or(RouteExhaustedAction::Continue)
            )
        );
    } else {
        println!(
            "{label} providers (v{CURRENT_CONFIG_VERSION}): routing=<implicit ordered-failover>"
        );
    }

    let effective = crate::config::effective_routing(view);
    let target = effective.entry_node().and_then(|node| {
        matches!(node.strategy, RouteStrategy::ManualSticky)
            .then(|| node.target.as_deref())
            .flatten()
    });
    let first_ordered = crate::config::resolved_provider_order("config_doc", view)
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
        let endpoints = provider_endpoint_count(provider);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cli_string_map_rejects_invalid_entries() {
        let map = parse_cli_string_map(
            &[
                "gpt-5.5=openai/gpt-5.5".to_string(),
                "gpt-* = openai/gpt-*".to_string(),
            ],
            "model-map",
        )
        .expect("valid map");
        assert_eq!(
            map.get("gpt-5.5").map(String::as_str),
            Some("openai/gpt-5.5")
        );
        assert_eq!(map.get("gpt-*").map(String::as_str), Some("openai/gpt-*"));

        assert!(parse_cli_string_map(&["missing-separator".to_string()], "model-map").is_err());
        assert!(parse_cli_string_map(&["=target".to_string()], "model-map").is_err());
        assert!(parse_cli_string_map(&["source=".to_string()], "model-map").is_err());
        assert!(
            parse_cli_string_map(&["a=b".to_string(), "a=c".to_string()], "model-map").is_err()
        );
    }
}

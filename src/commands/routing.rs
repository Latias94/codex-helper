use super::config_doc::{
    ensure_v4_routing, load_v4_config, ordered_v4_provider_names, parse_cli_tags,
    routing_exhausted_label, routing_policy_label, select_v4_service_view,
    select_v4_service_view_mut,
};
use super::route_view;
use crate::cli_types::{RoutingCommand, RoutingPolicy};
use crate::config::{
    PersistedRoutingProviderRef, PersistedRoutingSpec, RoutingExhaustedActionV4, RoutingPolicyV4,
    ServiceViewV4, storage::save_config_v4,
};
use crate::{CliError, CliResult};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Serialize)]
struct RoutingShowPayload {
    schema_version: u32,
    service: String,
    routing: PersistedRoutingSpec,
}

pub async fn handle_routing_cmd(cmd: RoutingCommand) -> CliResult<()> {
    match cmd {
        RoutingCommand::Show {
            codex,
            claude,
            json,
        } => {
            let (cfg, service, label) = load_v4_config(codex, claude, "routing")
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let (view, _) = select_v4_service_view(&cfg, service);
            let routing = persisted_routing_spec_from_view(view);
            if json {
                let payload = RoutingShowPayload {
                    schema_version: 4,
                    service: service.to_string(),
                    routing,
                };
                let text = serde_json::to_string_pretty(&payload)
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                println!("{text}");
            } else {
                print_routing_text(label, view);
            }
        }
        view_cmd @ (RoutingCommand::List { .. } | RoutingCommand::Explain { .. }) => {
            route_view::handle_route_view_cmd(view_cmd).await?;
        }
        RoutingCommand::Set {
            policy,
            target,
            clear_target,
            order,
            prefer_tags,
            clear_prefer_tags,
            on_exhausted,
            codex,
            claude,
        } => {
            let (mut cfg, service, label) = load_v4_config(codex, claude, "routing")
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            {
                let (view, _) = select_v4_service_view_mut(&mut cfg, service);
                if let Some(policy) = policy {
                    if matches!(policy, RoutingPolicy::ManualSticky) && !prefer_tags.is_empty() {
                        return Err(CliError::ProxyConfig(
                            "manual-sticky routing cannot combine with prefer-tags".to_string(),
                        ));
                    }
                    if !matches!(policy, RoutingPolicy::ManualSticky) && target.is_some() {
                        return Err(CliError::ProxyConfig(
                            "routing target only makes sense with manual-sticky policy".to_string(),
                        ));
                    }
                } else if target.is_some() && !prefer_tags.is_empty() {
                    return Err(CliError::ProxyConfig(
                        "routing target and prefer-tags should not be set together without an explicit policy".to_string(),
                    ));
                }
                if clear_target && matches!(policy, Some(RoutingPolicy::ManualSticky)) {
                    return Err(CliError::ProxyConfig(
                        "manual-sticky routing requires a target; do not combine it with --clear-target".to_string(),
                    ));
                }

                let mut changed = false;
                let current_routing = crate::config::effective_v4_routing(view);
                let current_entry = current_routing.entry_node();
                let mut next_policy = current_entry
                    .map(|node| node.strategy)
                    .unwrap_or(RoutingPolicyV4::OrderedFailover);
                let mut next_target = current_entry.and_then(|node| node.target.clone());
                let mut next_order = current_entry
                    .map(|node| node.children.clone())
                    .unwrap_or_default();
                let mut next_prefer_tags = current_entry
                    .map(|node| node.prefer_tags.clone())
                    .unwrap_or_default();
                let mut next_on_exhausted = current_entry
                    .map(|node| node.on_exhausted)
                    .unwrap_or(RoutingExhaustedActionV4::Continue);

                if let Some(policy) = policy {
                    next_policy = policy.into();
                    changed = true;
                }
                if let Some(value) = target {
                    next_policy = RoutingPolicyV4::ManualSticky;
                    next_target = Some(value);
                    changed = true;
                }
                if clear_target {
                    next_target = None;
                    if matches!(next_policy, RoutingPolicyV4::ManualSticky) {
                        next_policy = RoutingPolicyV4::OrderedFailover;
                    }
                    changed = true;
                }
                if !order.is_empty() {
                    next_order = normalize_complete_order(view, order, next_target.as_deref())
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                    changed = true;
                }
                if !prefer_tags.is_empty() {
                    next_prefer_tags = vec![
                        parse_cli_tags(&prefer_tags)
                            .map_err(|e| CliError::ProxyConfig(e.to_string()))?,
                    ];
                    next_policy = RoutingPolicyV4::TagPreferred;
                    changed = true;
                }
                if clear_prefer_tags {
                    next_prefer_tags.clear();
                    changed = true;
                }
                if let Some(action) = on_exhausted {
                    next_on_exhausted = action.into();
                    changed = true;
                }
                if !changed {
                    return Err(CliError::ProxyConfig(
                        "routing set requires at least one field change".to_string(),
                    ));
                }

                if !matches!(next_policy, RoutingPolicyV4::ManualSticky) {
                    next_target = None;
                }
                if !matches!(next_policy, RoutingPolicyV4::TagPreferred) {
                    next_prefer_tags.clear();
                }
                if !matches!(next_policy, RoutingPolicyV4::TagPreferred) && on_exhausted.is_none() {
                    next_on_exhausted = RoutingExhaustedActionV4::Continue;
                }
                next_order = normalize_complete_order(view, next_order, next_target.as_deref())
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

                validate_routing_fields(
                    view,
                    next_policy,
                    next_target.as_deref(),
                    &next_order,
                    &next_prefer_tags,
                )
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

                set_v4_entry_routing(
                    view,
                    next_policy,
                    next_target,
                    next_order,
                    next_prefer_tags,
                    next_on_exhausted,
                );
            }

            save_config_v4(&cfg)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            println!("{label} routing updated");
        }
        RoutingCommand::Pin {
            provider,
            codex,
            claude,
        } => {
            let (mut cfg, service, label) = load_v4_config(codex, claude, "routing")
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            {
                let (view, _) = select_v4_service_view_mut(&mut cfg, service);
                ensure_provider_exists(view, provider.as_str())?;

                let order =
                    normalize_complete_order(view, vec![provider.clone()], Some(provider.as_str()))
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                set_v4_entry_routing(
                    view,
                    RoutingPolicyV4::ManualSticky,
                    Some(provider.clone()),
                    order,
                    Vec::new(),
                    RoutingExhaustedActionV4::Continue,
                );
            }

            save_config_v4(&cfg)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            println!("{label} routing pinned to provider '{}'", provider);
        }
        RoutingCommand::Order {
            providers,
            codex,
            claude,
        } => {
            let (mut cfg, service, label) = load_v4_config(codex, claude, "routing")
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            {
                let (view, _) = select_v4_service_view_mut(&mut cfg, service);
                let order = normalize_complete_order(view, providers, None)
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                set_v4_entry_routing(
                    view,
                    RoutingPolicyV4::OrderedFailover,
                    None,
                    order,
                    Vec::new(),
                    RoutingExhaustedActionV4::Continue,
                );
            }

            save_config_v4(&cfg)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            println!("{label} routing order updated");
        }
        RoutingCommand::PreferTag {
            tags,
            order,
            on_exhausted,
            codex,
            claude,
        } => {
            let (mut cfg, service, label) = load_v4_config(codex, claude, "routing")
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            {
                let (view, _) = select_v4_service_view_mut(&mut cfg, service);
                let prefer_tag =
                    parse_cli_tags(&tags).map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                let order = if order.is_empty() {
                    normalize_complete_order(view, Vec::new(), None)
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?
                } else {
                    normalize_complete_order(view, order, None)
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?
                };
                let next_on_exhausted = on_exhausted
                    .map(Into::into)
                    .unwrap_or(RoutingExhaustedActionV4::Continue);
                set_v4_entry_routing(
                    view,
                    RoutingPolicyV4::TagPreferred,
                    None,
                    order,
                    vec![prefer_tag],
                    next_on_exhausted,
                );
            }

            save_config_v4(&cfg)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            println!("{label} tag-preferred routing updated");
        }
        RoutingCommand::ClearTarget { codex, claude } => {
            let (mut cfg, service, label) = load_v4_config(codex, claude, "routing")
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            {
                let (view, _) = select_v4_service_view_mut(&mut cfg, service);
                let current_routing = crate::config::effective_v4_routing(view);
                let Some(current_entry) = current_routing.entry_node() else {
                    return Err(CliError::ProxyConfig(
                        "routing clear-target requires an existing v4 routing block".to_string(),
                    ));
                };
                let next_order =
                    normalize_complete_order(view, current_entry.children.clone(), None)
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                let routing = view
                    .routing
                    .as_mut()
                    .expect("routing existence was checked above");
                let entry_name = routing.entry.clone();
                let node = routing.routes.entry(entry_name).or_default();
                node.strategy = RoutingPolicyV4::OrderedFailover;
                node.target = None;
                node.children = next_order;
                node.prefer_tags.clear();
                node.on_exhausted = RoutingExhaustedActionV4::Continue;
                routing.sync_compat_from_graph();
            }

            save_config_v4(&cfg)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            println!("{label} routing target cleared");
        }
    }

    Ok(())
}

fn persisted_routing_spec_from_view(view: &ServiceViewV4) -> PersistedRoutingSpec {
    let routing = crate::config::effective_v4_routing(view);
    let entry_node = routing.entry_node();
    let expanded_order = ordered_v4_provider_names(view);
    let providers = ordered_v4_provider_names(view)
        .into_iter()
        .filter_map(|name| {
            view.providers
                .get(name.as_str())
                .map(|provider| PersistedRoutingProviderRef {
                    name,
                    alias: provider.alias.clone(),
                    enabled: provider.enabled,
                    tags: provider.tags.clone(),
                })
        })
        .collect();
    PersistedRoutingSpec {
        entry: routing.entry.clone(),
        routes: routing.routes.clone(),
        policy: routing.policy,
        order: entry_node
            .map(|node| node.children.clone())
            .unwrap_or_default(),
        target: entry_node.and_then(|node| node.target.clone()),
        prefer_tags: entry_node
            .map(|node| node.prefer_tags.clone())
            .unwrap_or_default(),
        on_exhausted: entry_node
            .map(|node| node.on_exhausted)
            .unwrap_or(RoutingExhaustedActionV4::Continue),
        entry_strategy: entry_node
            .map(|node| node.strategy)
            .unwrap_or(RoutingPolicyV4::OrderedFailover),
        expanded_order,
        entry_target: entry_node.and_then(|node| node.target.clone()),
        providers,
    }
}

fn print_routing_text(label: &str, view: &ServiceViewV4) {
    let routing = persisted_routing_spec_from_view(view);
    let policy = routing.policy;
    let order = routing.order;
    let target = routing.target;
    let prefer_tags = routing.prefer_tags;
    let on_exhausted = routing.on_exhausted;
    let providers = routing.providers;
    println!("Schema version: v4");
    println!("Service: {label}");
    println!("Routing policy: {}", routing_policy_label(policy));
    println!("Routing target: {}", target.as_deref().unwrap_or("<none>"));
    let order = if order.is_empty() {
        "<provider key order>".to_string()
    } else {
        order.join(", ")
    };
    println!("Routing order: [{order}]");
    println!(
        "Prefer tags: {}",
        format_prefer_tags(prefer_tags.as_slice())
    );
    println!("On exhausted: {}", routing_exhausted_label(on_exhausted));
    println!("Providers:");
    for provider in providers {
        let marker = if target.as_deref() == Some(provider.name.as_str()) {
            "*"
        } else if matches_prefer_tags(&provider.tags, prefer_tags.as_slice()) {
            "+"
        } else {
            " "
        };
        let enabled = if provider.enabled { "on" } else { "off" };
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
                "  {} {} {} [{}] tags={}",
                marker, enabled, provider.name, alias, tags
            );
        } else {
            println!("  {} {} {} tags={}", marker, enabled, provider.name, tags);
        }
    }
}

fn format_prefer_tags(filters: &[BTreeMap<String, String>]) -> String {
    if filters.is_empty() {
        return "<none>".to_string();
    }
    filters
        .iter()
        .map(|filter| {
            let body = filter
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn matches_prefer_tags(
    provider_tags: &BTreeMap<String, String>,
    filters: &[BTreeMap<String, String>],
) -> bool {
    filters.iter().any(|filter| {
        !filter.is_empty()
            && filter
                .iter()
                .all(|(key, value)| provider_tags.get(key) == Some(value))
    })
}

fn ensure_provider_exists(view: &ServiceViewV4, provider: &str) -> CliResult<()> {
    if view.providers.contains_key(provider) {
        Ok(())
    } else {
        Err(CliError::ProxyConfig(format!(
            "provider '{}' not found in v4 routing config",
            provider
        )))
    }
}

fn normalize_complete_order(
    view: &ServiceViewV4,
    raw_order: Vec<String>,
    target: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    let mut seen = BTreeSet::new();
    let mut order = Vec::new();

    let mut push_name = |name: &str| -> anyhow::Result<()> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("routing order provider name must not be empty");
        }
        if !view.providers.contains_key(name) {
            anyhow::bail!("routing references missing provider '{}'", name);
        }
        if seen.insert(name.to_string()) {
            order.push(name.to_string());
        }
        Ok(())
    };

    if let Some(target) = target {
        push_name(target)?;
    }
    for name in raw_order {
        push_name(name.as_str())?;
    }
    for name in ordered_v4_provider_names(view) {
        push_name(name.as_str())?;
    }

    Ok(order)
}

fn validate_routing_fields(
    view: &ServiceViewV4,
    policy: RoutingPolicyV4,
    target: Option<&str>,
    order: &[String],
    prefer_tags: &[BTreeMap<String, String>],
) -> anyhow::Result<()> {
    if matches!(policy, RoutingPolicyV4::ManualSticky) && target.is_none() {
        anyhow::bail!("manual-sticky routing requires a target provider");
    }
    if !matches!(policy, RoutingPolicyV4::ManualSticky) && target.is_some() {
        anyhow::bail!("routing target only makes sense with manual-sticky policy");
    }
    if matches!(policy, RoutingPolicyV4::TagPreferred) && prefer_tags.is_empty() {
        anyhow::bail!("tag-preferred routing requires at least one prefer-tag filter");
    }
    for provider_name in order {
        if !view.providers.contains_key(provider_name) {
            anyhow::bail!("routing references missing provider '{}'", provider_name);
        }
    }
    if let Some(target) = target
        && !view.providers.contains_key(target)
    {
        anyhow::bail!("routing target references missing provider '{}'", target);
    }
    if let Some(target) = target {
        let Some(provider) = view.providers.get(target) else {
            anyhow::bail!("routing target references missing provider '{}'", target);
        };
        if !provider.enabled {
            anyhow::bail!(
                "routing target provider '{}' is disabled; enable it before pinning",
                target
            );
        }
    }
    Ok(())
}

fn set_v4_entry_routing(
    view: &mut ServiceViewV4,
    policy: RoutingPolicyV4,
    target: Option<String>,
    order: Vec<String>,
    prefer_tags: Vec<BTreeMap<String, String>>,
    on_exhausted: RoutingExhaustedActionV4,
) {
    let routing = ensure_v4_routing(view);
    let entry_name = routing.entry.clone();
    let node = routing.routes.entry(entry_name).or_default();
    node.strategy = policy;
    node.children = order;
    node.target = target;
    node.prefer_tags = prefer_tags;
    node.on_exhausted = on_exhausted;
    if !matches!(node.strategy, RoutingPolicyV4::ManualSticky) {
        node.target = None;
    }
    if !matches!(node.strategy, RoutingPolicyV4::TagPreferred) {
        node.prefer_tags.clear();
    }
    routing.sync_compat_from_graph();
}

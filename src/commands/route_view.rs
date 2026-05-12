use super::config_doc::{
    ConfigDocument, load_config_document, ordered_v4_provider_names, print_v4_provider_list,
    resolve_service, routing_exhausted_label, routing_policy_label, select_service_manager,
    select_v4_service_view,
};
use crate::config::{
    ServiceConfig, ServiceConfigManager, ServiceRoutingExplanation, ServiceViewV4,
    explain_service_routing, storage::config_file_path,
};
use crate::routing_explain::{
    RoutingExplainCondition, RoutingExplainConditionalBranch, RoutingExplainResponse,
    RoutingExplainRouteRef, RoutingExplainRouteRefKind, RoutingExplainSkipReason,
    build_routing_explain_response_with_request, parse_routing_explain_headers,
};
use crate::routing_ir::{
    RoutePlanRuntimeState, RouteRequestContext, compile_legacy_route_plan_template,
    compile_v4_route_plan_template_with_request,
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

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn build_route_request_context(
    model: Option<String>,
    service_tier: Option<String>,
    reasoning_effort: Option<String>,
    path: Option<String>,
    method: Option<String>,
    headers: Vec<String>,
) -> anyhow::Result<RouteRequestContext> {
    Ok(RouteRequestContext {
        model: clean_optional(model),
        service_tier: clean_optional(service_tier),
        reasoning_effort: clean_optional(reasoning_effort),
        path: clean_optional(path),
        method: clean_optional(method),
        headers: parse_routing_explain_headers(&headers).map_err(anyhow::Error::msg)?,
    })
}

fn build_v4_runtime_explain(
    service_name: &str,
    view: &ServiceViewV4,
    request: RouteRequestContext,
) -> anyhow::Result<RoutingExplainResponse> {
    let template = compile_v4_route_plan_template_with_request(service_name, view, &request)?;
    Ok(build_routing_explain_response_with_request(
        service_name,
        None,
        request,
        None,
        &template,
        &RoutePlanRuntimeState::default(),
    ))
}

fn build_legacy_runtime_explain(
    service_name: &str,
    mgr: &ServiceConfigManager,
    routing: &ServiceRoutingExplanation,
    request: RouteRequestContext,
) -> RoutingExplainResponse {
    let services = legacy_runtime_services_for_explain(mgr, routing);
    let template = compile_legacy_route_plan_template(service_name, services);
    build_routing_explain_response_with_request(
        service_name,
        None,
        request,
        None,
        &template,
        &RoutePlanRuntimeState::default(),
    )
}

fn legacy_runtime_services_for_explain<'a>(
    mgr: &'a ServiceConfigManager,
    routing: &ServiceRoutingExplanation,
) -> Vec<&'a ServiceConfig> {
    let names = if routing.eligible_stations.is_empty() {
        routing
            .fallback_station
            .as_ref()
            .map(|candidate| vec![candidate.name.as_str()])
            .unwrap_or_default()
    } else {
        routing
            .eligible_stations
            .iter()
            .map(|candidate| candidate.name.as_str())
            .collect::<Vec<_>>()
    };

    names
        .into_iter()
        .filter_map(|name| mgr.station(name))
        .collect()
}

fn print_runtime_explain_text(explain: &RoutingExplainResponse) {
    if let Some(model) = explain.request_model.as_deref() {
        println!("Request model: {model}");
    }
    if let Some(service_tier) = explain.request_context.service_tier.as_deref() {
        println!("Request service tier: {service_tier}");
    }
    if let Some(reasoning_effort) = explain.request_context.reasoning_effort.as_deref() {
        println!("Request reasoning effort: {reasoning_effort}");
    }
    if let Some(path) = explain.request_context.path.as_deref() {
        println!("Request path: {path}");
    }
    if let Some(method) = explain.request_context.method.as_deref() {
        println!("Request method: {method}");
    }
    if !explain.request_context.headers.is_empty() {
        println!(
            "Request headers: {}",
            explain.request_context.headers.join(", ")
        );
    }
    if let Some(selected) = &explain.selected_route {
        println!(
            "Selected route: {} endpoint={} path=[{}] compat_station={} upstream#{}",
            selected.provider_id,
            selected.endpoint_id,
            selected.route_path.join(" > "),
            selected.compatibility.station_name,
            selected.compatibility.upstream_index
        );
    } else {
        println!("Selected route: <none>");
    }

    if explain.candidates.is_empty() {
        println!("Runtime candidates: <empty>");
        return;
    }

    println!("Runtime candidates:");
    for (idx, candidate) in explain.candidates.iter().enumerate() {
        let marker = if candidate.selected { "*" } else { " " };
        let skips = format_skip_reasons(&candidate.skip_reasons);
        println!(
            "  {} {}. {} endpoint={} path=[{}] skip={} compat_station={} upstream#{}",
            marker,
            idx + 1,
            candidate.provider_id,
            candidate.endpoint_id,
            candidate.route_path.join(" > "),
            skips,
            candidate.compatibility.station_name,
            candidate.compatibility.upstream_index
        );
    }

    if !explain.conditional_routes.is_empty() {
        println!("Conditional routes:");
        for route in &explain.conditional_routes {
            let target = route
                .selected_target
                .as_ref()
                .map(format_route_ref)
                .unwrap_or_else(|| "<none>".to_string());
            println!(
                "  {} matched={} branch={} target={} condition=[{}]",
                route.route_name,
                route.matched,
                format_conditional_branch(route.selected_branch),
                target,
                format_condition(&route.condition)
            );
        }
    }
}

fn format_skip_reasons(reasons: &[RoutingExplainSkipReason]) -> String {
    if reasons.is_empty() {
        return "-".to_string();
    }

    reasons
        .iter()
        .map(RoutingExplainSkipReason::code)
        .collect::<Vec<_>>()
        .join(",")
}

fn format_conditional_branch(branch: RoutingExplainConditionalBranch) -> &'static str {
    match branch {
        RoutingExplainConditionalBranch::Then => "then",
        RoutingExplainConditionalBranch::Default => "default",
    }
}

fn format_route_ref(route_ref: &RoutingExplainRouteRef) -> String {
    let kind = match route_ref.kind {
        RoutingExplainRouteRefKind::Route => "route",
        RoutingExplainRouteRefKind::Provider => "provider",
    };
    format!("{kind}:{}", route_ref.name)
}

fn format_condition(condition: &RoutingExplainCondition) -> String {
    let mut parts = Vec::new();
    if let Some(model) = condition.model.as_deref() {
        parts.push(format!("model={model}"));
    }
    if let Some(service_tier) = condition.service_tier.as_deref() {
        parts.push(format!("service_tier={service_tier}"));
    }
    if let Some(reasoning_effort) = condition.reasoning_effort.as_deref() {
        parts.push(format!("reasoning_effort={reasoning_effort}"));
    }
    if let Some(path) = condition.path.as_deref() {
        parts.push(format!("path={path}"));
    }
    if let Some(method) = condition.method.as_deref() {
        parts.push(format!("method={method}"));
    }
    if !condition.headers.is_empty() {
        parts.push(format!("headers={}", condition.headers.join(",")));
    }

    if parts.is_empty() {
        "<empty>".to_string()
    } else {
        parts.join(",")
    }
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
            model,
            service_tier,
            reasoning_effort,
            path,
            method,
            headers,
        } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let document = load_config_document()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let request = build_route_request_context(
                model,
                service_tier,
                reasoning_effort,
                path,
                method,
                headers,
            )
            .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            if let ConfigDocument::V4(cfg) = &document {
                let (view, label) = select_v4_service_view(cfg, service);
                let runtime_explain = build_v4_runtime_explain(service, view, request.clone())
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                if json {
                    if let Some(provider_name) = provider.as_deref()
                        && explain_v4_provider(view, provider_name).is_none()
                    {
                        return Err(CliError::ProxyConfig(format!(
                            "provider '{}' not found",
                            provider_name
                        )));
                    }
                    let text = serde_json::to_string_pretty(&runtime_explain)
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                    println!("{text}");
                } else {
                    print_v4_explain_text(label, view, provider.as_deref())
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                    print_runtime_explain_text(&runtime_explain);
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
            let runtime_explain = build_legacy_runtime_explain(service, mgr, &routing, request);

            if json {
                if let Some(provider_name) = provider.as_deref()
                    && group_detail.is_none()
                {
                    return Err(CliError::ProxyConfig(format!(
                        "provider '{}' not found",
                        provider_name
                    )));
                }
                let text = serde_json::to_string_pretty(&runtime_explain)
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                println!("{text}");
            } else {
                print_explain_text(
                    label,
                    document.schema_version(),
                    &routing,
                    group_detail.as_ref(),
                );
                print_runtime_explain_text(&runtime_explain);
            }
        }
        _ => unreachable!("route view handles only routing list/explain"),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProviderConfigV4, RoutingConfigV4, UpstreamAuth};

    fn provider(base_url: &str, supported_models: &[&str]) -> ProviderConfigV4 {
        ProviderConfigV4 {
            base_url: Some(base_url.to_string()),
            inline_auth: UpstreamAuth::default(),
            supported_models: supported_models
                .iter()
                .map(|model| ((*model).to_string(), true))
                .collect(),
            ..ProviderConfigV4::default()
        }
    }

    #[test]
    fn v4_runtime_explain_json_contract_reports_selected_route_and_model_skips() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "old".to_string(),
                    provider("https://old.example/v1", &["gpt-4.1"]),
                ),
                (
                    "new".to_string(),
                    provider("https://new.example/v1", &["gpt-5"]),
                ),
            ]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "old".to_string(),
                "new".to_string(),
            ])),
            ..ServiceViewV4::default()
        };

        let explain = build_v4_runtime_explain(
            "codex",
            &view,
            RouteRequestContext {
                model: Some("gpt-5".to_string()),
                ..RouteRequestContext::default()
            },
        )
        .expect("runtime explain");
        let value = serde_json::to_value(&explain).expect("serialize runtime explain");

        assert_eq!(value["api_version"].as_u64(), Some(1));
        assert_eq!(value["service_name"].as_str(), Some("codex"));
        assert_eq!(value["request_model"].as_str(), Some("gpt-5"));
        assert!(value["runtime_loaded_at_ms"].is_null());
        assert_eq!(value["selected_route"]["provider_id"].as_str(), Some("new"));
        assert_eq!(
            value["selected_route"]["route_path"]
                .as_array()
                .map(|items| items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .collect::<Vec<_>>()),
            Some(vec!["main", "new"])
        );
        assert_eq!(
            value["candidates"][0]["skip_reasons"][0]["code"].as_str(),
            Some("unsupported_model")
        );
        assert_eq!(
            value["candidates"][0]["skip_reasons"][0]["requested_model"].as_str(),
            Some("gpt-5")
        );
    }
}

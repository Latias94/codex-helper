use super::config_doc::{
    load_config_document, ordered_provider_names, print_provider_list, resolve_service,
    routing_exhausted_label, routing_policy_label, select_service_route_config,
};
use crate::config::{CURRENT_CONFIG_VERSION, ServiceRouteConfig};
use crate::routing_explain::parse_routing_explain_headers;
use crate::routing_ir::{RouteRequestContext, compile_route_plan_template_with_request};
use crate::{CliError, CliResult, RoutingCommand};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Serialize, Clone)]
struct ConfigExplainProviderEndpoint {
    name: String,
    base_url: String,
    enabled: bool,
}

#[derive(Debug, Serialize, Clone)]
struct ConfigExplainProvider {
    name: String,
    alias: Option<String>,
    enabled: bool,
    routing_index: Option<usize>,
    target: bool,
    tags: BTreeMap<String, String>,
    endpoints: Vec<ConfigExplainProviderEndpoint>,
}

#[derive(Debug, Serialize, Clone)]
struct ConfigExplainRouting {
    entry: String,
    policy: &'static str,
    target: Option<String>,
    order: Vec<String>,
    expanded_order: Vec<String>,
    prefer_tags: Vec<BTreeMap<String, String>>,
    on_exhausted: &'static str,
}

#[derive(Debug, Serialize)]
struct ConfigOnlyRouteExplain {
    api_version: u32,
    source: &'static str,
    runtime_state_queried: bool,
    schema_version: u32,
    service_name: String,
    routing: ConfigExplainRouting,
    providers: Vec<ConfigExplainProvider>,
    request: ConfigOnlyRequestContext,
    route_graph_key: String,
    first_config_eligible_candidate: Option<ConfigOnlyRouteCandidate>,
    candidates: Vec<ConfigOnlyRouteCandidate>,
}

#[derive(Debug, Serialize)]
struct ConfigOnlyRequestContext {
    model: Option<String>,
    service_tier: Option<String>,
    reasoning_effort: Option<String>,
    path: Option<String>,
    method: Option<String>,
    header_names: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
struct ConfigOnlyRouteCandidate {
    provider_id: String,
    provider_alias: Option<String>,
    endpoint_id: String,
    provider_endpoint_key: String,
    route_path: Vec<String>,
    preference_group: u32,
    upstream_base_url: String,
    model_supported: bool,
    effective_model: Option<String>,
    config_skip_reasons: Vec<&'static str>,
}

fn explain_routing(view: &ServiceRouteConfig) -> ConfigExplainRouting {
    let routing = crate::config::effective_routing(view);
    let entry_node = routing.entry_node();
    ConfigExplainRouting {
        entry: routing.entry.clone(),
        policy: routing_policy_label(
            entry_node
                .map(|node| node.strategy)
                .unwrap_or(crate::config::RouteStrategy::OrderedFailover),
        ),
        target: entry_node.and_then(|node| node.target.clone()),
        order: entry_node
            .map(|node| node.children.clone())
            .unwrap_or_default(),
        expanded_order: crate::config::resolved_provider_order("route-view", view)
            .unwrap_or_else(|_| view.providers.keys().cloned().collect()),
        prefer_tags: entry_node
            .map(|node| node.prefer_tags.clone())
            .unwrap_or_default(),
        on_exhausted: routing_exhausted_label(
            entry_node
                .map(|node| node.on_exhausted)
                .unwrap_or(crate::config::RouteExhaustedAction::Continue),
        ),
    }
}

fn explain_provider(
    view: &ServiceRouteConfig,
    provider_name: &str,
) -> Option<ConfigExplainProvider> {
    let provider = view.providers.get(provider_name)?;
    let route_order = crate::config::resolved_provider_order("route-view", view)
        .unwrap_or_else(|_| ordered_provider_names(view));
    let routing_index = route_order
        .iter()
        .position(|candidate| candidate == provider_name)
        .map(|idx| idx + 1);
    let target = crate::config::effective_routing(view)
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
        endpoints.push(ConfigExplainProviderEndpoint {
            name: "default".to_string(),
            base_url: base_url.to_string(),
            enabled: provider.enabled,
        });
    }
    endpoints.extend(provider.endpoints.iter().map(|(endpoint_name, endpoint)| {
        ConfigExplainProviderEndpoint {
            name: endpoint_name.clone(),
            base_url: endpoint.base_url.clone(),
            enabled: endpoint.enabled,
        }
    }));

    Some(ConfigExplainProvider {
        name: provider_name.to_string(),
        alias: provider.alias.clone(),
        enabled: provider.enabled,
        routing_index,
        target,
        tags: provider.tags.clone(),
        endpoints,
    })
}

fn explain_providers(view: &ServiceRouteConfig) -> Vec<ConfigExplainProvider> {
    ordered_provider_names(view)
        .into_iter()
        .filter_map(|provider_name| explain_provider(view, provider_name.as_str()))
        .collect()
}

fn print_explain_text(
    label: &str,
    view: &ServiceRouteConfig,
    provider_name: Option<&str>,
) -> anyhow::Result<()> {
    let routing = explain_routing(view);
    println!("Schema version: v{CURRENT_CONFIG_VERSION}");
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

    let providers = explain_providers(view);
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
        let provider = explain_provider(view, provider_name)
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

fn build_config_only_explain(
    service_name: &str,
    view: &ServiceRouteConfig,
    provider_name: Option<&str>,
    request: RouteRequestContext,
) -> anyhow::Result<ConfigOnlyRouteExplain> {
    let providers = match provider_name {
        Some(provider_name) => vec![
            explain_provider(view, provider_name)
                .ok_or_else(|| anyhow::anyhow!("provider '{}' not found", provider_name))?,
        ],
        None => explain_providers(view),
    };
    let template = compile_route_plan_template_with_request(service_name, view, &request)?;
    let requested_model = request.model.as_deref();
    let candidates = template
        .candidates
        .iter()
        .map(|candidate| {
            let model_supported = requested_model
                .map(|model| candidate.is_model_supported(model))
                .unwrap_or(true);
            ConfigOnlyRouteCandidate {
                provider_id: candidate.provider_id.clone(),
                provider_alias: candidate.provider_alias.clone(),
                endpoint_id: candidate.endpoint_id.clone(),
                provider_endpoint_key: template
                    .candidate_provider_endpoint_key(candidate)
                    .stable_key(),
                route_path: candidate.route_path.clone(),
                preference_group: candidate.preference_group,
                upstream_base_url: candidate.base_url.clone(),
                model_supported,
                effective_model: requested_model.map(|model| candidate.effective_model(model)),
                config_skip_reasons: if model_supported {
                    Vec::new()
                } else {
                    vec!["unsupported_model"]
                },
            }
        })
        .collect::<Vec<_>>();
    let first_config_eligible_candidate = candidates
        .iter()
        .find(|candidate| candidate.model_supported)
        .cloned();
    let request = ConfigOnlyRequestContext {
        model: request.model,
        service_tier: request.service_tier,
        reasoning_effort: request.reasoning_effort,
        path: request.path,
        method: request.method,
        header_names: request.headers.into_keys().collect(),
    };

    Ok(ConfigOnlyRouteExplain {
        api_version: 1,
        source: "config_only",
        runtime_state_queried: false,
        schema_version: CURRENT_CONFIG_VERSION,
        service_name: service_name.to_string(),
        routing: explain_routing(view),
        providers,
        request,
        route_graph_key: template.route_graph_key(),
        first_config_eligible_candidate,
        candidates,
    })
}

fn config_only_explain_text_lines(explain: &ConfigOnlyRouteExplain) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push("Preview source: config_only".to_string());
    lines.push("Runtime state queried: no".to_string());
    if let Some(model) = explain.request.model.as_deref() {
        lines.push(format!("Request model: {model}"));
    }
    if let Some(service_tier) = explain.request.service_tier.as_deref() {
        lines.push(format!("Request service tier: {service_tier}"));
    }
    if let Some(reasoning_effort) = explain.request.reasoning_effort.as_deref() {
        lines.push(format!("Request reasoning effort: {reasoning_effort}"));
    }
    if let Some(path) = explain.request.path.as_deref() {
        lines.push(format!("Request path: {path}"));
    }
    if let Some(method) = explain.request.method.as_deref() {
        lines.push(format!("Request method: {method}"));
    }
    if !explain.request.header_names.is_empty() {
        lines.push(format!(
            "Request header names: {}",
            explain.request.header_names.join(", ")
        ));
    }
    if let Some(candidate) = &explain.first_config_eligible_candidate {
        lines.push(format!(
            "First config-eligible candidate: endpoint={} group={} provider={} path=[{}]",
            candidate.provider_endpoint_key,
            candidate.preference_group,
            candidate.provider_id,
            candidate.route_path.join(" > ")
        ));
    } else {
        lines.push("First config-eligible candidate: <none>".to_string());
    }

    if explain.candidates.is_empty() {
        lines.push("Config candidates: <empty>".to_string());
        return lines;
    }

    lines.push("Config candidates:".to_string());
    for (idx, candidate) in explain.candidates.iter().enumerate() {
        let marker = if explain
            .first_config_eligible_candidate
            .as_ref()
            .is_some_and(|first| first.provider_endpoint_key == candidate.provider_endpoint_key)
        {
            "*"
        } else {
            " "
        };
        let skips = if candidate.config_skip_reasons.is_empty() {
            "-".to_string()
        } else {
            candidate.config_skip_reasons.join(",")
        };
        lines.push(format!(
            "  {} {}. endpoint={} group={} provider={} path=[{}] skip={}",
            marker,
            idx + 1,
            candidate.provider_endpoint_key,
            candidate.preference_group,
            candidate.provider_id,
            candidate.route_path.join(" > "),
            skips
        ));
    }
    lines
}

fn print_config_only_explain_text(explain: &ConfigOnlyRouteExplain) {
    for line in config_only_explain_text_lines(explain) {
        println!("{line}");
    }
}

pub async fn handle_route_view_cmd(cmd: RoutingCommand) -> CliResult<()> {
    match cmd {
        RoutingCommand::List { codex, claude } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            let document = load_config_document()
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;

            let (view, label) = select_service_route_config(&document, service);
            print_provider_list(label, view);
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
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            let document = load_config_document()
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            let request = build_route_request_context(
                model,
                service_tier,
                reasoning_effort,
                path,
                method,
                headers,
            )
            .map_err(|e| CliError::Configuration(e.to_string()))?;

            let (view, label) = select_service_route_config(&document, service);
            let config_explain =
                build_config_only_explain(service, view, provider.as_deref(), request)
                    .map_err(|e| CliError::Configuration(e.to_string()))?;
            if json {
                let text = serde_json::to_string_pretty(&config_explain)
                    .map_err(|e| CliError::Configuration(e.to_string()))?;
                println!("{text}");
            } else {
                print_explain_text(label, view, provider.as_deref())
                    .map_err(|e| CliError::Configuration(e.to_string()))?;
                print_config_only_explain_text(&config_explain);
            }
        }
        _ => unreachable!("route view handles only routing list/explain"),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProviderConfig, RouteGraphConfig, UpstreamAuth};

    fn provider(base_url: &str, supported_models: &[&str]) -> ProviderConfig {
        ProviderConfig {
            base_url: Some(base_url.to_string()),
            inline_auth: UpstreamAuth::default(),
            supported_models: supported_models
                .iter()
                .map(|model| ((*model).to_string(), true))
                .collect(),
            ..ProviderConfig::default()
        }
    }

    #[test]
    fn config_only_explain_reports_candidate_order_without_runtime_state() {
        let view = ServiceRouteConfig {
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
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "old".to_string(),
                "new".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };

        let explain = build_config_only_explain(
            "codex",
            &view,
            None,
            RouteRequestContext {
                model: Some("gpt-5".to_string()),
                ..RouteRequestContext::default()
            },
        )
        .expect("config-only explain");
        let value = serde_json::to_value(&explain).expect("serialize config-only explain");

        assert_eq!(value["api_version"].as_u64(), Some(1));
        assert_eq!(value["source"].as_str(), Some("config_only"));
        assert_eq!(value["runtime_state_queried"].as_bool(), Some(false));
        assert_eq!(value["service_name"].as_str(), Some("codex"));
        assert_eq!(value["request"]["model"].as_str(), Some("gpt-5"));
        assert_eq!(
            value["first_config_eligible_candidate"]["provider_id"].as_str(),
            Some("new")
        );
        assert_eq!(
            value["first_config_eligible_candidate"]["route_path"]
                .as_array()
                .map(|items| items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .collect::<Vec<_>>()),
            Some(vec!["main", "new"])
        );
        assert_eq!(
            value["candidates"][0]["config_skip_reasons"][0].as_str(),
            Some("unsupported_model")
        );
        assert!(value.get("runtime_loaded_at_ms").is_none());
        assert!(value["candidates"][0].get("availability").is_none());
        assert!(value["candidates"][0].get("capacity").is_none());
    }

    #[test]
    fn config_only_explain_text_names_the_non_runtime_source() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "input".to_string(),
                provider("https://input.example/v1", &["gpt-5"]),
            )]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "input".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let explain =
            build_config_only_explain("codex", &view, None, RouteRequestContext::default())
                .expect("config-only explain");

        let lines = config_only_explain_text_lines(&explain);

        assert_eq!(lines[0], "Preview source: config_only");
        assert_eq!(lines[1], "Runtime state queried: no");
        assert_eq!(
            lines[2],
            "First config-eligible candidate: endpoint=codex/input/default group=0 provider=input path=[main > input]"
        );
        assert_eq!(lines[3], "Config candidates:");
        assert!(lines[4].starts_with(
            "  * 1. endpoint=codex/input/default group=0 provider=input path=[main > input]"
        ));
    }
}

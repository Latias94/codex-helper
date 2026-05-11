use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context, Result};

use crate::config::{
    ProviderConfigV4, RoutingConfigV4, RoutingExhaustedActionV4, RoutingNodeV4, RoutingPolicyV4,
    ServiceViewV4, UpstreamAuth, UpstreamConfig, effective_v4_routing,
};
use crate::lb::SelectedUpstream;

const V4_COMPATIBILITY_STATION_NAME: &str = "routing";

#[derive(Debug, Clone)]
pub struct RoutePlanTemplate {
    pub service_name: String,
    pub entry: String,
    pub nodes: BTreeMap<String, RouteNodePlan>,
    pub expanded_provider_order: Vec<String>,
    pub candidates: Vec<RouteCandidate>,
    pub compatibility_station_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RoutePlan {
    pub service_name: String,
    pub entry: String,
    pub candidates: Vec<RouteCandidate>,
    pub decision_trace: RouteDecisionTrace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteNodePlan {
    pub name: String,
    pub strategy: RoutingPolicyV4,
    pub children: Vec<RouteRef>,
    pub target: Option<RouteRef>,
    pub prefer_tags: Vec<BTreeMap<String, String>>,
    pub on_exhausted: RoutingExhaustedActionV4,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteRef {
    Route(String),
    Provider(String),
}

#[derive(Debug, Clone)]
pub struct RouteCandidate {
    pub provider_id: String,
    pub provider_alias: Option<String>,
    pub endpoint_id: String,
    pub base_url: String,
    pub auth: UpstreamAuth,
    pub tags: BTreeMap<String, String>,
    pub supported_models: BTreeMap<String, bool>,
    pub model_mapping: BTreeMap<String, String>,
    pub route_path: Vec<String>,
    pub stable_index: usize,
    pub compatibility_station_name: Option<String>,
    pub compatibility_upstream_index: Option<usize>,
}

impl RouteCandidate {
    pub fn to_upstream_config(&self) -> UpstreamConfig {
        UpstreamConfig {
            base_url: self.base_url.clone(),
            auth: self.auth.clone(),
            tags: btree_string_map_to_hash_map(&self.tags),
            supported_models: btree_bool_map_to_hash_map(&self.supported_models),
            model_mapping: btree_string_map_to_hash_map(&self.model_mapping),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SelectedRouteCandidate<'a> {
    pub candidate: &'a RouteCandidate,
    pub selected_upstream: SelectedUpstream,
}

pub struct RoutePlanExecutor<'a> {
    template: &'a RoutePlanTemplate,
}

impl<'a> RoutePlanExecutor<'a> {
    pub fn new(template: &'a RoutePlanTemplate) -> Self {
        Self { template }
    }

    pub fn iter_candidates(&self) -> impl Iterator<Item = &RouteCandidate> + '_ {
        self.template.candidates.iter()
    }

    pub fn iter_selected_upstreams(&self) -> impl Iterator<Item = SelectedRouteCandidate<'_>> + '_ {
        self.template
            .candidates
            .iter()
            .map(|candidate| SelectedRouteCandidate {
                candidate,
                selected_upstream: self.selected_upstream_for_candidate(candidate),
            })
    }

    pub fn selected_upstream_for_candidate(&self, candidate: &RouteCandidate) -> SelectedUpstream {
        SelectedUpstream {
            station_name: candidate
                .compatibility_station_name
                .clone()
                .or_else(|| self.template.compatibility_station_name.clone())
                .unwrap_or_else(|| self.template.service_name.clone()),
            index: candidate
                .compatibility_upstream_index
                .unwrap_or(candidate.stable_index),
            upstream: candidate.to_upstream_config(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteDecisionTrace {
    pub events: Vec<RouteDecisionEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecisionEvent {
    pub route_path: Vec<String>,
    pub provider_id: Option<String>,
    pub endpoint_id: Option<String>,
    pub decision: RouteDecision,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteDecision {
    Candidate,
    Selected,
    Skipped,
}

#[derive(Debug, Clone)]
struct RouteLeaf {
    provider_id: String,
    route_path: Vec<String>,
}

#[derive(Debug, Clone)]
struct EndpointParts {
    endpoint_id: String,
    base_url: String,
    enabled: bool,
    priority: u32,
    tags: BTreeMap<String, String>,
    supported_models: BTreeMap<String, bool>,
    model_mapping: BTreeMap<String, String>,
}

pub fn compile_v4_route_plan_template(
    service_name: &str,
    view: &ServiceViewV4,
) -> Result<RoutePlanTemplate> {
    let routing = effective_v4_routing(view);
    validate_route_provider_name_conflicts(service_name, view, &routing)?;

    let nodes = normalize_route_nodes(service_name, view, &routing)?;
    let leaves = expand_v4_route_leaves(service_name, view, &routing)?;
    ensure_unique_provider_leaves(service_name, &leaves)?;

    let expanded_provider_order = leaves
        .iter()
        .map(|leaf| leaf.provider_id.clone())
        .collect::<Vec<_>>();
    let candidates = route_candidates_from_leaves(service_name, view, &leaves)?;

    Ok(RoutePlanTemplate {
        service_name: service_name.to_string(),
        entry: routing.entry,
        nodes,
        expanded_provider_order,
        compatibility_station_name: (!leaves.is_empty())
            .then(|| V4_COMPATIBILITY_STATION_NAME.to_string()),
        candidates,
    })
}

fn validate_route_provider_name_conflicts(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
) -> Result<()> {
    for route_name in routing.routes.keys() {
        if view.providers.contains_key(route_name.as_str()) {
            anyhow::bail!(
                "[{service_name}] route node '{route_name}' conflicts with a provider of the same name"
            );
        }
    }
    Ok(())
}

fn normalize_route_nodes(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
) -> Result<BTreeMap<String, RouteNodePlan>> {
    let mut out = BTreeMap::new();
    for (route_name, node) in &routing.routes {
        let children = node
            .children
            .iter()
            .map(|child| normalize_route_ref(service_name, view, routing, child))
            .collect::<Result<Vec<_>>>()?;
        let target = node
            .target
            .as_deref()
            .map(|target| normalize_route_ref(service_name, view, routing, target))
            .transpose()?;
        out.insert(
            route_name.clone(),
            RouteNodePlan {
                name: route_name.clone(),
                strategy: node.strategy,
                children,
                target,
                prefer_tags: node.prefer_tags.clone(),
                on_exhausted: node.on_exhausted,
                metadata: node.metadata.clone(),
            },
        );
    }
    Ok(out)
}

fn normalize_route_ref(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    name: &str,
) -> Result<RouteRef> {
    if view.providers.contains_key(name) {
        return Ok(RouteRef::Provider(name.to_string()));
    }
    if routing.routes.contains_key(name) {
        return Ok(RouteRef::Route(name.to_string()));
    }
    anyhow::bail!("[{service_name}] routing references missing route or provider '{name}'");
}

fn expand_v4_route_leaves(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
) -> Result<Vec<RouteLeaf>> {
    if view.providers.is_empty() && routing.routes.is_empty() {
        return Ok(Vec::new());
    }
    if routing.routes.is_empty() {
        return Ok(view
            .providers
            .keys()
            .map(|provider_id| RouteLeaf {
                provider_id: provider_id.clone(),
                route_path: vec![provider_id.clone()],
            })
            .collect());
    }

    let mut stack = Vec::new();
    expand_route_node(
        service_name,
        view,
        routing,
        routing.entry.as_str(),
        &[],
        &mut stack,
    )
}

fn expand_route_ref(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    child_name: &str,
    parent_path: &[String],
    stack: &mut Vec<String>,
) -> Result<Vec<RouteLeaf>> {
    if view.providers.contains_key(child_name) {
        let mut route_path = parent_path.to_vec();
        route_path.push(child_name.to_string());
        return Ok(vec![RouteLeaf {
            provider_id: child_name.to_string(),
            route_path,
        }]);
    }

    expand_route_node(service_name, view, routing, child_name, parent_path, stack)
}

fn expand_route_node(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    route_name: &str,
    parent_path: &[String],
    stack: &mut Vec<String>,
) -> Result<Vec<RouteLeaf>> {
    if stack.iter().any(|name| name == route_name) {
        let mut cycle = stack.clone();
        cycle.push(route_name.to_string());
        anyhow::bail!(
            "[{service_name}] routing graph has a cycle: {}",
            cycle.join(" -> ")
        );
    }

    let Some(node) = routing.routes.get(route_name) else {
        anyhow::bail!(
            "[{service_name}] routing entry references missing route node '{route_name}'"
        );
    };

    stack.push(route_name.to_string());
    let mut node_path = parent_path.to_vec();
    node_path.push(route_name.to_string());
    let result = match node.strategy {
        RoutingPolicyV4::OrderedFailover => expand_ordered_route_children(
            service_name,
            view,
            routing,
            route_name,
            node,
            &node_path,
            stack,
        ),
        RoutingPolicyV4::ManualSticky => expand_manual_sticky_route(
            service_name,
            view,
            routing,
            route_name,
            node,
            &node_path,
            stack,
        ),
        RoutingPolicyV4::TagPreferred => expand_tag_preferred_route(
            service_name,
            view,
            routing,
            route_name,
            node,
            &node_path,
            stack,
        ),
    };
    stack.pop();
    result
}

fn expand_ordered_route_children(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    route_name: &str,
    node: &RoutingNodeV4,
    node_path: &[String],
    stack: &mut Vec<String>,
) -> Result<Vec<RouteLeaf>> {
    if node.children.is_empty() {
        anyhow::bail!(
            "[{service_name}] ordered-failover route '{route_name}' requires at least one child"
        );
    }

    let mut leaves = Vec::new();
    for child_name in &node.children {
        leaves.extend(expand_route_ref(
            service_name,
            view,
            routing,
            child_name.as_str(),
            node_path,
            stack,
        )?);
    }
    Ok(leaves)
}

fn expand_manual_sticky_route(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    route_name: &str,
    node: &RoutingNodeV4,
    node_path: &[String],
    stack: &mut Vec<String>,
) -> Result<Vec<RouteLeaf>> {
    let target = node
        .target
        .as_deref()
        .or_else(|| node.children.first().map(String::as_str))
        .with_context(|| {
            format!("[{service_name}] manual-sticky route '{route_name}' requires target")
        })?;
    if let Some(provider) = view.providers.get(target)
        && !provider.enabled
    {
        anyhow::bail!(
            "[{service_name}] manual-sticky route '{route_name}' targets disabled provider '{target}'"
        );
    }

    expand_route_ref(service_name, view, routing, target, node_path, stack)
}

fn expand_tag_preferred_route(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    route_name: &str,
    node: &RoutingNodeV4,
    node_path: &[String],
    stack: &mut Vec<String>,
) -> Result<Vec<RouteLeaf>> {
    if node.children.is_empty() {
        anyhow::bail!(
            "[{service_name}] tag-preferred route '{route_name}' requires at least one child"
        );
    }
    if node.prefer_tags.is_empty() {
        anyhow::bail!("[{service_name}] tag-preferred route '{route_name}' requires prefer_tags");
    }

    let mut preferred = Vec::new();
    let mut fallback = Vec::new();
    for child_name in &node.children {
        let child_leaves = expand_route_ref(
            service_name,
            view,
            routing,
            child_name.as_str(),
            node_path,
            stack,
        )?;
        if child_route_matches_any_filter(view, &child_leaves, &node.prefer_tags) {
            preferred.extend(child_leaves);
        } else {
            fallback.extend(child_leaves);
        }
    }

    if matches!(node.on_exhausted, RoutingExhaustedActionV4::Stop) {
        if preferred.is_empty() {
            anyhow::bail!(
                "[{service_name}] tag-preferred route '{route_name}' with on_exhausted = 'stop' matched no providers"
            );
        }
        return Ok(preferred);
    }

    preferred.extend(fallback);
    Ok(preferred)
}

fn child_route_matches_any_filter(
    view: &ServiceViewV4,
    leaves: &[RouteLeaf],
    filters: &[BTreeMap<String, String>],
) -> bool {
    leaves.iter().any(|leaf| {
        view.providers
            .get(leaf.provider_id.as_str())
            .is_some_and(|provider| provider_matches_any_filter(&provider.tags, filters))
    })
}

fn provider_matches_any_filter(
    tags: &BTreeMap<String, String>,
    filters: &[BTreeMap<String, String>],
) -> bool {
    filters.iter().any(|filter| {
        !filter.is_empty()
            && filter
                .iter()
                .all(|(key, value)| tags.get(key) == Some(value))
    })
}

fn ensure_unique_provider_leaves(service_name: &str, leaves: &[RouteLeaf]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for leaf in leaves {
        if !seen.insert(leaf.provider_id.as_str()) {
            anyhow::bail!(
                "[{service_name}] routing graph expands provider '{}' more than once; duplicate leaves are ambiguous",
                leaf.provider_id
            );
        }
    }
    Ok(())
}

fn route_candidates_from_leaves(
    service_name: &str,
    view: &ServiceViewV4,
    leaves: &[RouteLeaf],
) -> Result<Vec<RouteCandidate>> {
    let mut candidates = Vec::new();
    for leaf in leaves {
        let Some(provider) = view.providers.get(leaf.provider_id.as_str()) else {
            anyhow::bail!(
                "[{service_name}] routing references missing provider '{}'",
                leaf.provider_id
            );
        };
        if !provider.enabled {
            continue;
        }

        let auth = merge_auth(&provider.auth, &provider.inline_auth);
        for endpoint in
            ordered_provider_endpoints(service_name, leaf.provider_id.as_str(), provider)?
        {
            if !endpoint.enabled {
                continue;
            }
            let stable_index = candidates.len();
            candidates.push(RouteCandidate {
                provider_id: leaf.provider_id.clone(),
                provider_alias: provider.alias.clone(),
                endpoint_id: endpoint.endpoint_id,
                base_url: endpoint.base_url,
                auth: auth.clone(),
                tags: merge_string_maps_with_provider_id(
                    leaf.provider_id.as_str(),
                    &provider.tags,
                    &endpoint.tags,
                ),
                supported_models: merge_bool_maps(
                    &provider.supported_models,
                    &endpoint.supported_models,
                ),
                model_mapping: merge_string_maps(&provider.model_mapping, &endpoint.model_mapping),
                route_path: leaf.route_path.clone(),
                stable_index,
                compatibility_station_name: Some(V4_COMPATIBILITY_STATION_NAME.to_string()),
                compatibility_upstream_index: Some(stable_index),
            });
        }
    }
    Ok(candidates)
}

fn ordered_provider_endpoints(
    service_name: &str,
    provider_name: &str,
    provider: &ProviderConfigV4,
) -> Result<Vec<EndpointParts>> {
    let mut endpoints = Vec::new();
    if let Some(base_url) = provider
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if provider.endpoints.contains_key("default") {
            anyhow::bail!(
                "[{service_name}] provider '{provider_name}' cannot define both base_url and endpoints.default"
            );
        }
        endpoints.push(EndpointParts {
            endpoint_id: "default".to_string(),
            base_url: base_url.to_string(),
            enabled: true,
            priority: 0,
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
        });
    }

    for (endpoint_id, endpoint) in &provider.endpoints {
        if endpoint.base_url.trim().is_empty() {
            anyhow::bail!(
                "[{service_name}] provider '{provider_name}' endpoint '{endpoint_id}' has an empty base_url"
            );
        }
        endpoints.push(EndpointParts {
            endpoint_id: endpoint_id.clone(),
            base_url: endpoint.base_url.trim().to_string(),
            enabled: endpoint.enabled,
            priority: endpoint.priority,
            tags: endpoint.tags.clone(),
            supported_models: endpoint.supported_models.clone(),
            model_mapping: endpoint.model_mapping.clone(),
        });
    }

    if endpoints.is_empty() {
        anyhow::bail!("[{service_name}] provider '{provider_name}' has no base_url or endpoints");
    }

    endpoints.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.endpoint_id.cmp(&right.endpoint_id))
            .then_with(|| left.base_url.cmp(&right.base_url))
    });
    Ok(endpoints)
}

fn merge_auth(block: &UpstreamAuth, inline: &UpstreamAuth) -> UpstreamAuth {
    UpstreamAuth {
        auth_token: inline
            .auth_token
            .clone()
            .or_else(|| block.auth_token.clone()),
        auth_token_env: inline
            .auth_token_env
            .clone()
            .or_else(|| block.auth_token_env.clone()),
        api_key: inline.api_key.clone().or_else(|| block.api_key.clone()),
        api_key_env: inline
            .api_key_env
            .clone()
            .or_else(|| block.api_key_env.clone()),
    }
}

fn merge_string_maps(
    provider_values: &BTreeMap<String, String>,
    endpoint_values: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut merged = provider_values.clone();
    for (key, value) in endpoint_values {
        merged.insert(key.clone(), value.clone());
    }
    merged
}

fn merge_string_maps_with_provider_id(
    provider_id: &str,
    provider_values: &BTreeMap<String, String>,
    endpoint_values: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut provider_values = provider_values.clone();
    provider_values.insert("provider_id".to_string(), provider_id.to_string());
    merge_string_maps(&provider_values, endpoint_values)
}

fn merge_bool_maps(
    provider_values: &BTreeMap<String, bool>,
    endpoint_values: &BTreeMap<String, bool>,
) -> BTreeMap<String, bool> {
    let mut merged = provider_values.clone();
    for (key, value) in endpoint_values {
        merged.insert(key.clone(), *value);
    }
    merged
}

fn btree_string_map_to_hash_map(values: &BTreeMap<String, String>) -> HashMap<String, String> {
    values
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn btree_bool_map_to_hash_map(values: &BTreeMap<String, bool>) -> HashMap<String, bool> {
    values
        .iter()
        .map(|(key, value)| (key.clone(), *value))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ProviderEndpointV4, ProxyConfigV4, RoutingConfigV4, RoutingExhaustedActionV4,
        RoutingNodeV4, RoutingPolicyV4, compile_v4_to_runtime, resolved_v4_provider_order,
    };
    use crate::lb::{LbState, LoadBalancer, SelectedUpstream};
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};

    fn provider(base_url: &str) -> ProviderConfigV4 {
        ProviderConfigV4 {
            base_url: Some(base_url.to_string()),
            ..ProviderConfigV4::default()
        }
    }

    fn tagged_provider(base_url: &str, key: &str, value: &str) -> ProviderConfigV4 {
        ProviderConfigV4 {
            base_url: Some(base_url.to_string()),
            tags: BTreeMap::from([(key.to_string(), value.to_string())]),
            ..ProviderConfigV4::default()
        }
    }

    fn provider_ids(template: &RoutePlanTemplate) -> Vec<String> {
        template
            .candidates
            .iter()
            .map(|candidate| candidate.provider_id.clone())
            .collect()
    }

    fn assert_provider_order_parity(view: &ServiceViewV4, template: &RoutePlanTemplate) {
        let resolved = resolved_v4_provider_order("routing-ir-test", view).expect("resolved order");
        assert_eq!(template.expanded_provider_order, resolved);
        assert_eq!(provider_ids(template), resolved);
    }

    #[derive(Debug, PartialEq, Eq)]
    struct UpstreamSignature {
        station_name: String,
        index: usize,
        base_url: String,
        tags: BTreeMap<String, String>,
        supported_models: BTreeMap<String, bool>,
        model_mapping: BTreeMap<String, String>,
    }

    fn hash_string_map_to_btree(values: &HashMap<String, String>) -> BTreeMap<String, String> {
        values
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    fn hash_bool_map_to_btree(values: &HashMap<String, bool>) -> BTreeMap<String, bool> {
        values
            .iter()
            .map(|(key, value)| (key.clone(), *value))
            .collect()
    }

    fn upstream_signature(selected: &SelectedUpstream) -> UpstreamSignature {
        UpstreamSignature {
            station_name: selected.station_name.clone(),
            index: selected.index,
            base_url: selected.upstream.base_url.clone(),
            tags: hash_string_map_to_btree(&selected.upstream.tags),
            supported_models: hash_bool_map_to_btree(&selected.upstream.supported_models),
            model_mapping: hash_string_map_to_btree(&selected.upstream.model_mapping),
        }
    }

    fn executor_selected_upstream_signatures(
        template: &RoutePlanTemplate,
    ) -> Vec<UpstreamSignature> {
        RoutePlanExecutor::new(template)
            .iter_selected_upstreams()
            .map(|selected| upstream_signature(&selected.selected_upstream))
            .collect()
    }

    fn legacy_load_balancer_selected_upstream_signatures(
        view: ServiceViewV4,
    ) -> Vec<UpstreamSignature> {
        let runtime = compile_v4_to_runtime(&ProxyConfigV4 {
            codex: view,
            ..ProxyConfigV4::default()
        })
        .expect("compile v4 runtime");
        let service = runtime
            .codex
            .station("routing")
            .expect("routing station")
            .clone();
        let upstream_count = service.upstreams.len();
        let lb = LoadBalancer::new(
            Arc::new(service),
            Arc::new(Mutex::new(HashMap::<String, LbState>::new())),
        );
        let mut avoid = HashSet::new();
        let mut selected = Vec::new();
        while selected.len() < upstream_count {
            let next = lb
                .select_upstream_avoiding_strict(&avoid)
                .expect("legacy selected upstream");
            avoid.insert(next.index);
            selected.push(upstream_signature(&next));
        }
        selected
    }

    fn assert_executor_matches_legacy_load_balancer(view: ServiceViewV4) {
        let template = compile_v4_route_plan_template("codex", &view).expect("route template");
        assert_eq!(
            executor_selected_upstream_signatures(&template),
            legacy_load_balancer_selected_upstream_signatures(view)
        );
    }

    #[test]
    fn routing_ir_one_provider_matches_resolved_order() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([(
                "input".to_string(),
                provider("https://input.example/v1"),
            )]),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template("codex", &view).expect("route template");

        assert_provider_order_parity(&view, &template);
        assert_eq!(template.entry, "main");
        assert_eq!(template.candidates[0].endpoint_id, "default");
        assert_eq!(template.candidates[0].base_url, "https://input.example/v1");
        assert_eq!(template.candidates[0].route_path, vec!["main", "input"]);
        assert_eq!(
            template.candidates[0]
                .tags
                .get("provider_id")
                .map(String::as_str),
            Some("input")
        );
    }

    #[test]
    fn routing_ir_ordered_failover_matches_resolved_order() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "primary".to_string(),
                    provider("https://primary.example/v1"),
                ),
                ("backup".to_string(), provider("https://backup.example/v1")),
            ]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "backup".to_string(),
                "primary".to_string(),
            ])),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template("codex", &view).expect("route template");

        assert_provider_order_parity(&view, &template);
        assert_eq!(provider_ids(&template), vec!["backup", "primary"]);
    }

    #[test]
    fn routing_ir_nested_route_graph_preserves_candidate_order_and_path() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "input".to_string(),
                    tagged_provider("https://input.example/v1", "billing", "monthly"),
                ),
                (
                    "input1".to_string(),
                    tagged_provider("https://input1.example/v1", "billing", "monthly"),
                ),
                (
                    "paygo".to_string(),
                    tagged_provider("https://paygo.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(RoutingConfigV4 {
                entry: "monthly_first".to_string(),
                routes: BTreeMap::from([
                    (
                        "monthly_pool".to_string(),
                        RoutingNodeV4 {
                            strategy: RoutingPolicyV4::OrderedFailover,
                            children: vec!["input".to_string(), "input1".to_string()],
                            ..RoutingNodeV4::default()
                        },
                    ),
                    (
                        "monthly_first".to_string(),
                        RoutingNodeV4 {
                            strategy: RoutingPolicyV4::OrderedFailover,
                            children: vec!["monthly_pool".to_string(), "paygo".to_string()],
                            ..RoutingNodeV4::default()
                        },
                    ),
                ]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template("codex", &view).expect("route template");

        assert_provider_order_parity(&view, &template);
        assert_eq!(provider_ids(&template), vec!["input", "input1", "paygo"]);
        assert_eq!(
            template.candidates[1].route_path,
            vec!["monthly_first", "monthly_pool", "input1"]
        );
        assert_eq!(
            template.candidates[2].route_path,
            vec!["monthly_first", "paygo"]
        );
    }

    #[test]
    fn routing_ir_manual_sticky_matches_resolved_order() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "primary".to_string(),
                    provider("https://primary.example/v1"),
                ),
                ("backup".to_string(), provider("https://backup.example/v1")),
            ]),
            routing: Some(RoutingConfigV4::manual_sticky(
                "backup".to_string(),
                vec!["backup".to_string(), "primary".to_string()],
            )),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template("codex", &view).expect("route template");

        assert_provider_order_parity(&view, &template);
        assert_eq!(provider_ids(&template), vec!["backup"]);
        assert_eq!(template.candidates[0].route_path, vec!["main", "backup"]);
    }

    #[test]
    fn routing_ir_tag_preferred_continue_matches_resolved_order() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    tagged_provider("https://monthly.example/v1", "billing", "monthly"),
                ),
                (
                    "paygo".to_string(),
                    tagged_provider("https://paygo.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(RoutingConfigV4::tag_preferred(
                vec!["paygo".to_string(), "monthly".to_string()],
                vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                RoutingExhaustedActionV4::Continue,
            )),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template("codex", &view).expect("route template");

        assert_provider_order_parity(&view, &template);
        assert_eq!(provider_ids(&template), vec!["monthly", "paygo"]);
    }

    #[test]
    fn routing_ir_tag_preferred_stop_matches_resolved_order() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    tagged_provider("https://monthly.example/v1", "billing", "monthly"),
                ),
                (
                    "paygo".to_string(),
                    tagged_provider("https://paygo.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(RoutingConfigV4::tag_preferred(
                vec!["paygo".to_string(), "monthly".to_string()],
                vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                RoutingExhaustedActionV4::Stop,
            )),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template("codex", &view).expect("route template");

        assert_provider_order_parity(&view, &template);
        assert_eq!(provider_ids(&template), vec!["monthly"]);
    }

    #[test]
    fn routing_ir_candidate_expands_provider_endpoints_in_runtime_order() {
        let mut endpoints = BTreeMap::new();
        endpoints.insert(
            "slow".to_string(),
            ProviderEndpointV4 {
                base_url: "https://slow.example/v1".to_string(),
                enabled: true,
                priority: 10,
                tags: BTreeMap::from([("region".to_string(), "us".to_string())]),
                supported_models: BTreeMap::from([("gpt-4.1".to_string(), true)]),
                model_mapping: BTreeMap::new(),
            },
        );
        endpoints.insert(
            "fast".to_string(),
            ProviderEndpointV4 {
                base_url: "https://fast.example/v1".to_string(),
                enabled: true,
                priority: 0,
                tags: BTreeMap::from([("region".to_string(), "hk".to_string())]),
                supported_models: BTreeMap::new(),
                model_mapping: BTreeMap::from([(
                    "gpt-5".to_string(),
                    "provider-gpt-5".to_string(),
                )]),
            },
        );
        let view = ServiceViewV4 {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfigV4 {
                    tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                    supported_models: BTreeMap::from([("gpt-5".to_string(), true)]),
                    endpoints,
                    ..ProviderConfigV4::default()
                },
            )]),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template("codex", &view).expect("route template");

        assert_eq!(provider_ids(&template), vec!["input", "input"]);
        assert_eq!(template.candidates[0].endpoint_id, "fast");
        assert_eq!(template.candidates[1].endpoint_id, "slow");
        assert_eq!(
            template.candidates[0]
                .tags
                .get("billing")
                .map(String::as_str),
            Some("monthly")
        );
        assert_eq!(
            template.candidates[0]
                .tags
                .get("region")
                .map(String::as_str),
            Some("hk")
        );
        assert_eq!(
            template.candidates[0]
                .model_mapping
                .get("gpt-5")
                .map(String::as_str),
            Some("provider-gpt-5")
        );
        assert_eq!(
            template.candidates[1].supported_models.get("gpt-5"),
            Some(&true)
        );
        assert_eq!(
            template.candidates[1].supported_models.get("gpt-4.1"),
            Some(&true)
        );
    }

    #[test]
    fn route_plan_executor_matches_legacy_load_balancer_for_nested_route() {
        assert_executor_matches_legacy_load_balancer(ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "input".to_string(),
                    tagged_provider("https://input.example/v1", "billing", "monthly"),
                ),
                (
                    "input1".to_string(),
                    tagged_provider("https://input1.example/v1", "billing", "monthly"),
                ),
                (
                    "paygo".to_string(),
                    tagged_provider("https://paygo.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(RoutingConfigV4 {
                entry: "monthly_first".to_string(),
                routes: BTreeMap::from([
                    (
                        "monthly_pool".to_string(),
                        RoutingNodeV4 {
                            strategy: RoutingPolicyV4::OrderedFailover,
                            children: vec!["input".to_string(), "input1".to_string()],
                            ..RoutingNodeV4::default()
                        },
                    ),
                    (
                        "monthly_first".to_string(),
                        RoutingNodeV4 {
                            strategy: RoutingPolicyV4::OrderedFailover,
                            children: vec!["monthly_pool".to_string(), "paygo".to_string()],
                            ..RoutingNodeV4::default()
                        },
                    ),
                ]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        });
    }

    #[test]
    fn route_plan_executor_matches_legacy_load_balancer_for_tag_preferred() {
        assert_executor_matches_legacy_load_balancer(ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    tagged_provider("https://monthly.example/v1", "billing", "monthly"),
                ),
                (
                    "paygo".to_string(),
                    tagged_provider("https://paygo.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(RoutingConfigV4::tag_preferred(
                vec!["paygo".to_string(), "monthly".to_string()],
                vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                RoutingExhaustedActionV4::Continue,
            )),
            ..ServiceViewV4::default()
        });
    }

    #[test]
    fn route_plan_executor_matches_legacy_load_balancer_for_multi_endpoint_provider() {
        let mut endpoints = BTreeMap::new();
        endpoints.insert(
            "slow".to_string(),
            ProviderEndpointV4 {
                base_url: "https://slow.example/v1".to_string(),
                enabled: true,
                priority: 10,
                tags: BTreeMap::from([("region".to_string(), "us".to_string())]),
                supported_models: BTreeMap::from([("gpt-4.1".to_string(), true)]),
                model_mapping: BTreeMap::new(),
            },
        );
        endpoints.insert(
            "fast".to_string(),
            ProviderEndpointV4 {
                base_url: "https://fast.example/v1".to_string(),
                enabled: true,
                priority: 0,
                tags: BTreeMap::from([("region".to_string(), "hk".to_string())]),
                supported_models: BTreeMap::new(),
                model_mapping: BTreeMap::from([(
                    "gpt-5".to_string(),
                    "provider-gpt-5".to_string(),
                )]),
            },
        );

        assert_executor_matches_legacy_load_balancer(ServiceViewV4 {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfigV4 {
                    tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                    supported_models: BTreeMap::from([("gpt-5".to_string(), true)]),
                    endpoints,
                    ..ProviderConfigV4::default()
                },
            )]),
            ..ServiceViewV4::default()
        });
    }
}

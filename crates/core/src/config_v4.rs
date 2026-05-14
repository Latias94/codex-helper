use super::*;
use crate::routing_ir::{RouteCandidate, compile_v4_route_plan_template_for_compat_runtime};
use std::collections::BTreeSet;

const ROUTING_STATION_NAME: &str = "routing";

#[derive(Debug, Clone)]
pub struct ConfigV4MigrationReport {
    pub config: ProxyConfigV4,
    pub warnings: Vec<String>,
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

fn remove_import_metadata_tags(tags: &mut BTreeMap<String, String>) {
    tags.remove("provider_id");
    tags.remove("requires_openai_auth");
    if tags
        .get("source")
        .is_some_and(|value| value == "codex-config")
    {
        tags.remove("source");
    }
}

fn compact_service_view_v4_for_write(view: &mut ServiceViewV4) {
    for provider in view.providers.values_mut() {
        remove_import_metadata_tags(&mut provider.tags);
        for endpoint in provider.endpoints.values_mut() {
            remove_import_metadata_tags(&mut endpoint.tags);
        }
    }
    if let Some(routing) = view.routing.as_mut() {
        if routing.routes.is_empty() {
            routing.sync_graph_from_compat();
        }
        routing.sync_compat_from_graph();
    }
}

pub fn collect_route_graph_affinity_migration_warnings(
    service_name: &str,
    view: &ServiceViewV4,
    warnings: &mut Vec<String>,
) {
    let Some(routing) = view.routing.as_ref() else {
        return;
    };

    if routing.affinity_policy == RoutingAffinityPolicyV5::PreferredGroup
        && route_graph_has_fallback_choices(routing)
    {
        warnings.push(format!(
            "[{service_name}] route graph affinity now defaults to preferred-group; if you relied on old fallback-sticky behavior, set affinity_policy = \"fallback-sticky\" explicitly."
        ));
    }
}

fn route_graph_has_fallback_choices(routing: &RoutingConfigV4) -> bool {
    routing.order.len() > 1
        || routing.chain.len() > 1
        || routing.routes.values().any(|node| {
            node.children.len() > 1 || (node.target.is_some() && !node.children.is_empty())
        })
}

pub fn compact_v4_config_for_write(cfg: &mut ProxyConfigV4) {
    compact_service_view_v4_for_write(&mut cfg.codex);
    compact_service_view_v4_for_write(&mut cfg.claude);
}

fn provider_v4_to_v2(
    service_name: &str,
    provider_name: &str,
    provider: &ProviderConfigV4,
) -> Result<ProviderConfigV2> {
    let mut endpoints = BTreeMap::new();
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
        endpoints.insert(
            "default".to_string(),
            ProviderEndpointV2 {
                base_url: base_url.to_string(),
                enabled: true,
                priority: default_provider_endpoint_priority(),
                tags: BTreeMap::from([("endpoint_id".to_string(), "default".to_string())]),
                supported_models: BTreeMap::new(),
                model_mapping: BTreeMap::new(),
            },
        );
    }

    for (endpoint_name, endpoint) in &provider.endpoints {
        if endpoint.base_url.trim().is_empty() {
            anyhow::bail!(
                "[{service_name}] provider '{provider_name}' endpoint '{endpoint_name}' has an empty base_url"
            );
        }
        endpoints.insert(
            endpoint_name.clone(),
            ProviderEndpointV2 {
                base_url: endpoint.base_url.trim().to_string(),
                enabled: endpoint.enabled,
                priority: endpoint.priority,
                tags: {
                    let mut tags = endpoint.tags.clone();
                    tags.insert("endpoint_id".to_string(), endpoint_name.clone());
                    tags
                },
                supported_models: endpoint.supported_models.clone(),
                model_mapping: endpoint.model_mapping.clone(),
            },
        );
    }

    if endpoints.is_empty() {
        anyhow::bail!("[{service_name}] provider '{provider_name}' has no base_url or endpoints");
    }

    let mut tags = provider.tags.clone();
    tags.insert("provider_id".to_string(), provider_name.to_string());

    Ok(ProviderConfigV2 {
        alias: provider.alias.clone(),
        enabled: provider.enabled,
        auth: merge_auth(&provider.auth, &provider.inline_auth),
        tags,
        supported_models: provider.supported_models.clone(),
        model_mapping: provider.model_mapping.clone(),
        endpoints,
    })
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

fn validate_runtime_provider_v4_shape(
    service_name: &str,
    provider_name: &str,
    provider: &ProviderConfigV4,
) -> Result<()> {
    let mut has_endpoint = false;
    if let Some(_base_url) = provider
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
        has_endpoint = true;
    }

    for (endpoint_name, endpoint) in &provider.endpoints {
        if endpoint.base_url.trim().is_empty() {
            anyhow::bail!(
                "[{service_name}] provider '{provider_name}' endpoint '{endpoint_name}' has an empty base_url"
            );
        }
        has_endpoint = true;
    }

    if !has_endpoint {
        anyhow::bail!("[{service_name}] provider '{provider_name}' has no base_url or endpoints");
    }

    Ok(())
}

fn validate_service_view_v4_runtime_shape(service_name: &str, view: &ServiceViewV4) -> Result<()> {
    for (provider_name, provider) in &view.providers {
        validate_runtime_provider_v4_shape(service_name, provider_name, provider)?;
    }
    Ok(())
}

fn route_candidate_to_compat_upstream(candidate: &RouteCandidate) -> UpstreamConfig {
    let mut tags = btree_string_map_to_hash_map(&candidate.tags);
    tags.insert("endpoint_id".to_string(), candidate.endpoint_id.clone());

    UpstreamConfig {
        base_url: candidate.base_url.clone(),
        auth: candidate.auth.clone(),
        tags,
        supported_models: btree_bool_map_to_hash_map(&candidate.supported_models),
        model_mapping: btree_string_map_to_hash_map(&candidate.model_mapping),
    }
}

fn provider_order_from_routing(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
) -> Result<Vec<String>> {
    if view.providers.is_empty() && routing.routes.is_empty() {
        return Ok(Vec::new());
    }

    if routing.routes.is_empty() {
        return Ok(view.providers.keys().cloned().collect());
    }

    for route_name in routing.routes.keys() {
        if view.providers.contains_key(route_name.as_str()) {
            anyhow::bail!(
                "[{service_name}] route node '{route_name}' conflicts with a provider of the same name"
            );
        }
    }

    let mut stack = Vec::new();
    let order = expand_route_node(
        service_name,
        view,
        routing,
        routing.entry.as_str(),
        &mut stack,
    )?;
    ensure_unique_route_order(service_name, &order)?;
    Ok(order)
}

fn ensure_unique_route_order(service_name: &str, order: &[String]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for provider_name in order {
        if !seen.insert(provider_name.as_str()) {
            anyhow::bail!(
                "[{service_name}] routing graph expands provider '{provider_name}' more than once; duplicate leaves are ambiguous"
            );
        }
    }
    Ok(())
}

fn expand_route_ref(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    child_name: &str,
    stack: &mut Vec<String>,
) -> Result<Vec<String>> {
    if view.providers.contains_key(child_name) {
        return Ok(vec![child_name.to_string()]);
    }

    expand_route_node(service_name, view, routing, child_name, stack)
}

fn expand_route_node(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    route_name: &str,
    stack: &mut Vec<String>,
) -> Result<Vec<String>> {
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
    let result = match node.strategy {
        RoutingPolicyV4::OrderedFailover => expand_ordered_route_children(
            service_name,
            view,
            routing,
            route_name,
            &node.children,
            stack,
        ),
        RoutingPolicyV4::ManualSticky => {
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
            expand_route_ref(service_name, view, routing, target, stack)
        }
        RoutingPolicyV4::TagPreferred => {
            expand_tag_preferred_route(service_name, view, routing, route_name, node, stack)
        }
        RoutingPolicyV4::Conditional => {
            expand_conditional_route_compat(service_name, view, routing, route_name, node, stack)
        }
    };
    stack.pop();
    result
}

fn expand_ordered_route_children(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    route_name: &str,
    children: &[String],
    stack: &mut Vec<String>,
) -> Result<Vec<String>> {
    if children.is_empty() {
        anyhow::bail!(
            "[{service_name}] ordered-failover route '{route_name}' requires at least one child"
        );
    }

    let mut order = Vec::new();
    for child_name in children {
        order.extend(expand_route_ref(
            service_name,
            view,
            routing,
            child_name.as_str(),
            stack,
        )?);
    }
    Ok(order)
}

fn child_route_matches_any_filter(
    view: &ServiceViewV4,
    provider_names: &[String],
    filters: &[BTreeMap<String, String>],
) -> bool {
    provider_names.iter().any(|provider_name| {
        view.providers
            .get(provider_name.as_str())
            .is_some_and(|provider| provider_matches_any_filter(&provider.tags, filters))
    })
}

fn expand_tag_preferred_route(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    route_name: &str,
    node: &RoutingNodeV4,
    stack: &mut Vec<String>,
) -> Result<Vec<String>> {
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
        let child_order =
            expand_route_ref(service_name, view, routing, child_name.as_str(), stack)?;
        if child_route_matches_any_filter(view, &child_order, &node.prefer_tags) {
            preferred.extend(child_order);
        } else {
            fallback.extend(child_order);
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

fn expand_conditional_route_compat(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    route_name: &str,
    node: &RoutingNodeV4,
    stack: &mut Vec<String>,
) -> Result<Vec<String>> {
    let condition = node.when.as_ref().with_context(|| {
        format!("[{service_name}] conditional route '{route_name}' requires when")
    })?;
    if condition.is_empty() {
        anyhow::bail!(
            "[{service_name}] conditional route '{route_name}' requires at least one condition field"
        );
    }

    let then = node
        .then
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .with_context(|| {
            format!("[{service_name}] conditional route '{route_name}' requires then")
        })?;
    let default_route = node
        .default_route
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .with_context(|| {
            format!("[{service_name}] conditional route '{route_name}' requires default")
        })?;

    let mut order = Vec::new();
    order.extend(expand_route_ref(service_name, view, routing, then, stack)?);
    order.extend(expand_route_ref(
        service_name,
        view,
        routing,
        default_route,
        stack,
    )?);
    dedupe_preserving_order(&mut order);
    Ok(order)
}

fn dedupe_preserving_order(values: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    values.retain(|value| seen.insert(value.clone()));
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

fn default_routing_for_view(view: &ServiceViewV4) -> RoutingConfigV4 {
    if view.providers.is_empty() {
        RoutingConfigV4::default()
    } else {
        RoutingConfigV4::ordered_failover(view.providers.keys().cloned().collect())
    }
}

pub fn effective_v4_routing(view: &ServiceViewV4) -> RoutingConfigV4 {
    let mut routing = view
        .routing
        .clone()
        .unwrap_or_else(|| default_routing_for_view(view));
    if routing.routes.is_empty() {
        routing.sync_graph_from_compat();
    }
    routing.sync_compat_from_graph();
    routing
}

pub fn resolved_v4_provider_order(service_name: &str, view: &ServiceViewV4) -> Result<Vec<String>> {
    let routing = effective_v4_routing(view);
    provider_order_from_routing(service_name, view, &routing)
}

fn compile_service_view_v4(service_name: &str, view: &ServiceViewV4) -> Result<ServiceViewV2> {
    let providers = view
        .providers
        .iter()
        .map(|(provider_name, provider)| {
            provider_v4_to_v2(service_name, provider_name, provider)
                .map(|provider| (provider_name.clone(), provider))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;

    let route_order = resolved_v4_provider_order(service_name, view)?;
    let groups = if route_order.is_empty() {
        BTreeMap::new()
    } else {
        BTreeMap::from([(
            ROUTING_STATION_NAME.to_string(),
            GroupConfigV2 {
                alias: Some("active routing".to_string()),
                enabled: true,
                level: default_service_config_level(),
                members: route_order
                    .into_iter()
                    .map(|provider| GroupMemberRefV2 {
                        provider,
                        endpoint_names: Vec::new(),
                        preferred: false,
                    })
                    .collect(),
            },
        )])
    };

    Ok(ServiceViewV2 {
        active_group: if groups.is_empty() {
            None
        } else {
            Some(ROUTING_STATION_NAME.to_string())
        },
        default_profile: view.default_profile.clone(),
        profiles: view.profiles.clone(),
        providers,
        groups,
    })
}

fn compile_service_view_v4_runtime(
    service_name: &str,
    view: &ServiceViewV4,
) -> Result<ServiceConfigManager> {
    validate_service_view_v4_runtime_shape(service_name, view)?;
    let template = compile_v4_route_plan_template_for_compat_runtime(service_name, view)?;
    let mut configs = HashMap::new();
    if !template.expanded_provider_order.is_empty() {
        configs.insert(
            ROUTING_STATION_NAME.to_string(),
            ServiceConfig {
                name: ROUTING_STATION_NAME.to_string(),
                alias: Some("active routing".to_string()),
                enabled: true,
                level: default_service_config_level(),
                upstreams: template
                    .candidates
                    .iter()
                    .map(route_candidate_to_compat_upstream)
                    .collect(),
            },
        );
    }

    let mgr = ServiceConfigManager {
        active: if configs.is_empty() {
            None
        } else {
            Some(ROUTING_STATION_NAME.to_string())
        },
        default_profile: view.default_profile.clone(),
        profiles: view.profiles.clone(),
        configs,
    };
    validate_service_profiles(service_name, &mgr)?;
    Ok(mgr)
}

pub fn compile_v4_to_v2(v4: &ProxyConfigV4) -> Result<ProxyConfigV2> {
    if !is_supported_route_graph_config_version(v4.version) {
        anyhow::bail!("unsupported route graph config version: {}", v4.version);
    }

    Ok(ProxyConfigV2 {
        version: 2,
        codex: compile_service_view_v4("codex", &v4.codex)?,
        claude: compile_service_view_v4("claude", &v4.claude)?,
        retry: v4.retry.clone(),
        notify: v4.notify.clone(),
        default_service: v4.default_service,
        ui: v4.ui.clone(),
    })
}

pub fn compile_v4_to_runtime(v4: &ProxyConfigV4) -> Result<ProxyConfig> {
    if !is_supported_route_graph_config_version(v4.version) {
        anyhow::bail!("unsupported route graph config version: {}", v4.version);
    }

    Ok(ProxyConfig {
        version: Some(v4.version),
        codex: compile_service_view_v4_runtime("codex", &v4.codex)?,
        claude: compile_service_view_v4_runtime("claude", &v4.claude)?,
        retry: v4.retry.clone(),
        notify: v4.notify.clone(),
        default_service: v4.default_service,
        ui: v4.ui.clone(),
    })
}

fn endpoint_v2_to_v4(endpoint: &ProviderEndpointV2) -> ProviderEndpointV4 {
    ProviderEndpointV4 {
        base_url: endpoint.base_url.clone(),
        enabled: endpoint.enabled,
        priority: endpoint.priority,
        tags: endpoint.tags.clone(),
        supported_models: endpoint.supported_models.clone(),
        model_mapping: endpoint.model_mapping.clone(),
    }
}

fn endpoint_can_be_inlined(endpoint_name: &str, endpoint: &ProviderEndpointV2) -> bool {
    endpoint_name == "default"
        && endpoint.enabled
        && endpoint.priority == default_provider_endpoint_priority()
        && endpoint.tags.is_empty()
        && endpoint.supported_models.is_empty()
        && endpoint.model_mapping.is_empty()
}

fn provider_v2_to_v4(provider: &ProviderConfigV2) -> ProviderConfigV4 {
    let mut out = ProviderConfigV4 {
        alias: provider.alias.clone(),
        enabled: provider.enabled,
        base_url: None,
        auth: UpstreamAuth::default(),
        inline_auth: provider.auth.clone(),
        tags: provider.tags.clone(),
        supported_models: provider.supported_models.clone(),
        model_mapping: provider.model_mapping.clone(),
        endpoints: BTreeMap::new(),
    };

    if provider.endpoints.len() == 1
        && let Some((endpoint_name, endpoint)) = provider.endpoints.iter().next()
        && endpoint_can_be_inlined(endpoint_name, endpoint)
    {
        out.base_url = Some(endpoint.base_url.clone());
        return out;
    }

    out.endpoints = provider
        .endpoints
        .iter()
        .map(|(name, endpoint)| (name.clone(), endpoint_v2_to_v4(endpoint)))
        .collect();
    out
}

fn member_order_for_group(group: &GroupConfigV2) -> Vec<String> {
    let mut members = group.members.iter().enumerate().collect::<Vec<_>>();
    members.sort_by_key(|(idx, member)| (!member.preferred, *idx));
    members
        .into_iter()
        .map(|(_, member)| member.provider.clone())
        .collect()
}

fn ordered_selected_v2_group_names(view: &ServiceViewV2) -> Vec<String> {
    let active = view.active_group.as_deref();
    let mut groups = view.groups.iter().collect::<Vec<_>>();
    groups.retain(|(name, group)| group.enabled || active == Some(name.as_str()));

    if groups.is_empty() {
        if let Some(active_name) = active
            && view.groups.contains_key(active_name)
        {
            return vec![active_name.to_string()];
        }

        return view.groups.keys().min().cloned().into_iter().collect();
    }

    groups.sort_by(|(left_name, left), (right_name, right)| {
        left.level
            .cmp(&right.level)
            .then_with(|| {
                let left_is_active = active == Some(left_name.as_str());
                let right_is_active = active == Some(right_name.as_str());
                right_is_active.cmp(&left_is_active)
            })
            .then_with(|| left_name.cmp(right_name))
    });
    groups
        .into_iter()
        .map(|(group_name, _)| group_name.clone())
        .collect()
}

fn routing_order_from_v2_groups(view: &ServiceViewV2) -> Vec<String> {
    if view.groups.is_empty() {
        return view.providers.keys().cloned().collect();
    }

    let mut seen = BTreeMap::<String, ()>::new();
    let mut order = Vec::new();
    for group_name in ordered_selected_v2_group_names(view) {
        let Some(group) = view.groups.get(group_name.as_str()) else {
            continue;
        };
        for provider in member_order_for_group(group) {
            if seen.insert(provider.clone(), ()).is_none() {
                order.push(provider);
            }
        }
    }
    order
}

fn enabled_endpoint_name_set(provider: &ProviderConfigV2) -> BTreeSet<String> {
    provider
        .endpoints
        .iter()
        .filter(|(_, endpoint)| endpoint.enabled)
        .map(|(endpoint_name, _)| endpoint_name.clone())
        .collect()
}

fn selected_enabled_endpoint_name_set(
    provider: &ProviderConfigV2,
    member: &GroupMemberRefV2,
) -> BTreeSet<String> {
    if member.endpoint_names.is_empty() {
        return enabled_endpoint_name_set(provider);
    }

    member
        .endpoint_names
        .iter()
        .filter(|endpoint_name| {
            provider
                .endpoints
                .get(endpoint_name.as_str())
                .is_some_and(|endpoint| endpoint.enabled)
        })
        .cloned()
        .collect()
}

fn endpoint_scope_is_full_provider(provider: &ProviderConfigV2, member: &GroupMemberRefV2) -> bool {
    selected_enabled_endpoint_name_set(provider, member) == enabled_endpoint_name_set(provider)
}

fn collect_service_v2_to_v4_warnings(
    service_name: &str,
    view: &ServiceViewV2,
    warnings: &mut Vec<String>,
) {
    if !view.groups.is_empty() {
        let selected_group_names = ordered_selected_v2_group_names(view);
        if view.groups.len() > 1 {
            let selected = if selected_group_names.is_empty() {
                "<none>".to_string()
            } else {
                selected_group_names.join(", ")
            };
            warnings.push(format!(
                "[{service_name}] v2 has {} stations/groups; v4 migration flattens the effective route into a single route graph entry (selected groups: {selected}). Station aliases, levels, and enabled flags are not preserved as station metadata.",
                view.groups.len()
            ));
        }

        let active = view.active_group.as_deref();
        let omitted_disabled = view
            .groups
            .iter()
            .filter(|(group_name, group)| !group.enabled && active != Some(group_name.as_str()))
            .map(|(group_name, _)| group_name.clone())
            .collect::<Vec<_>>();
        if !omitted_disabled.is_empty() {
            warnings.push(format!(
                "[{service_name}] disabled inactive v2 stations/groups are omitted from the route graph: {}.",
                omitted_disabled.join(", ")
            ));
        }

        let included_disabled_active = selected_group_names
            .iter()
            .filter(|group_name| {
                view.groups
                    .get(group_name.as_str())
                    .is_some_and(|group| !group.enabled && active == Some(group_name.as_str()))
            })
            .cloned()
            .collect::<Vec<_>>();
        if !included_disabled_active.is_empty() {
            warnings.push(format!(
                "[{service_name}] disabled active v2 stations/groups remain routeable in the route graph to match current runtime fallback behavior: {}.",
                included_disabled_active.join(", ")
            ));
        }

        let mut provider_occurrences = BTreeMap::<String, usize>::new();
        for group_name in &selected_group_names {
            let Some(group) = view.groups.get(group_name.as_str()) else {
                continue;
            };
            for member in &group.members {
                *provider_occurrences
                    .entry(member.provider.clone())
                    .or_insert(0) += 1;

                let Some(provider) = view.providers.get(member.provider.as_str()) else {
                    continue;
                };
                if !endpoint_scope_is_full_provider(provider, member) {
                    let selected = selected_enabled_endpoint_name_set(provider, member)
                        .into_iter()
                        .collect::<Vec<_>>()
                        .join(", ");
                    let available = enabled_endpoint_name_set(provider)
                        .into_iter()
                        .collect::<Vec<_>>()
                        .join(", ");
                    warnings.push(format!(
                        "[{service_name}] v2 group '{group_name}' scopes provider '{}' to endpoint(s) [{}], but route graph leaves are provider-level; provider '{}' keeps all enabled endpoint(s) [{}].",
                        member.provider, selected, member.provider, available
                    ));
                }
            }
        }

        let repeated = provider_occurrences
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(provider, count)| format!("{provider} x{count}"))
            .collect::<Vec<_>>();
        if !repeated.is_empty() {
            warnings.push(format!(
                "[{service_name}] providers referenced multiple times in selected v2 groups are de-duplicated in the route graph: {}.",
                repeated.join(", ")
            ));
        }
    }

    let migrated_view = migrate_service_v2_to_v4(view);
    collect_route_graph_affinity_migration_warnings(service_name, &migrated_view, warnings);

    let cleared_profiles = view
        .profiles
        .iter()
        .filter(|(_, profile)| {
            profile
                .station
                .as_deref()
                .map(str::trim)
                .is_some_and(|station| !station.is_empty())
        })
        .map(|(profile_name, profile)| {
            format!(
                "{} -> {}",
                profile_name,
                profile.station.as_deref().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    if !cleared_profiles.is_empty() {
        warnings.push(format!(
            "[{service_name}] profile station bindings are cleared because route graph routing owns active provider selection: {}.",
            cleared_profiles.join(", ")
        ));
    }
}

fn migrate_service_v2_to_v4(view: &ServiceViewV2) -> ServiceViewV4 {
    let mut profiles = view.profiles.clone();
    for profile in profiles.values_mut() {
        if profile
            .station
            .as_deref()
            .map(str::trim)
            .is_some_and(|station| !station.is_empty())
        {
            profile.station = None;
        }
    }

    let providers = view
        .providers
        .iter()
        .map(|(name, provider)| (name.clone(), provider_v2_to_v4(provider)))
        .collect::<BTreeMap<_, _>>();

    let order = routing_order_from_v2_groups(view);
    let routing = if providers.is_empty() {
        None
    } else {
        Some(RoutingConfigV4::ordered_failover(order))
    };

    ServiceViewV4 {
        default_profile: view.default_profile.clone(),
        profiles,
        providers,
        routing,
    }
}

pub fn migrate_v2_to_v4(v2: &ProxyConfigV2) -> Result<ProxyConfigV4> {
    Ok(migrate_v2_to_v4_with_report(v2)?.config)
}

pub fn migrate_v2_to_v4_with_report(v2: &ProxyConfigV2) -> Result<ConfigV4MigrationReport> {
    let compact = compact_v2_config(v2)?;
    let mut warnings = Vec::new();
    collect_service_v2_to_v4_warnings("codex", &compact.codex, &mut warnings);
    collect_service_v2_to_v4_warnings("claude", &compact.claude, &mut warnings);

    let config = ProxyConfigV4 {
        version: CURRENT_ROUTE_GRAPH_CONFIG_VERSION,
        codex: migrate_service_v2_to_v4(&compact.codex),
        claude: migrate_service_v2_to_v4(&compact.claude),
        retry: compact.retry,
        notify: compact.notify,
        default_service: compact.default_service,
        ui: compact.ui,
    };

    Ok(ConfigV4MigrationReport { config, warnings })
}

pub fn migrate_legacy_to_v4(old: &ProxyConfig) -> Result<ProxyConfigV4> {
    Ok(migrate_legacy_to_v4_with_report(old)?.config)
}

pub fn migrate_legacy_to_v4_with_report(old: &ProxyConfig) -> Result<ConfigV4MigrationReport> {
    migrate_v2_to_v4_with_report(&migrate_legacy_to_v2(old))
}

pub mod legacy {
    use super::*;

    fn default_legacy_proxy_config_version() -> u32 {
        3
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ProxyConfigV3Legacy {
        #[serde(default = "default_legacy_proxy_config_version")]
        pub version: u32,
        #[serde(default)]
        pub codex: ServiceViewV3Legacy,
        #[serde(default)]
        pub claude: ServiceViewV3Legacy,
        #[serde(default)]
        pub retry: RetryConfig,
        #[serde(default)]
        pub notify: NotifyConfig,
        #[serde(default)]
        pub default_service: Option<ServiceKind>,
        #[serde(default)]
        pub ui: UiConfig,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, Default)]
    pub struct ServiceViewV3Legacy {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub default_profile: Option<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        pub profiles: BTreeMap<String, ServiceControlProfile>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        pub providers: BTreeMap<String, ProviderConfigV4>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub routing: Option<RoutingConfigV3Legacy>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct RoutingConfigV3Legacy {
        #[serde(default = "default_legacy_routing_policy")]
        pub policy: RoutingPolicyV3Legacy,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub order: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub target: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub prefer_tags: Vec<BTreeMap<String, String>>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub chain: Vec<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        pub pools: BTreeMap<String, RoutingPoolV4>,
        #[serde(default = "default_legacy_on_exhausted")]
        pub on_exhausted: RoutingExhaustedActionV3Legacy,
    }

    impl Default for RoutingConfigV3Legacy {
        fn default() -> Self {
            Self {
                policy: default_legacy_routing_policy(),
                order: Vec::new(),
                target: None,
                prefer_tags: Vec::new(),
                chain: Vec::new(),
                pools: BTreeMap::new(),
                on_exhausted: default_legacy_on_exhausted(),
            }
        }
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case")]
    pub enum RoutingPolicyV3Legacy {
        ManualSticky,
        OrderedFailover,
        TagPreferred,
        PoolFallback,
    }

    fn default_legacy_routing_policy() -> RoutingPolicyV3Legacy {
        RoutingPolicyV3Legacy::OrderedFailover
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case")]
    pub enum RoutingExhaustedActionV3Legacy {
        Continue,
        Stop,
    }

    fn default_legacy_on_exhausted() -> RoutingExhaustedActionV3Legacy {
        RoutingExhaustedActionV3Legacy::Continue
    }

    fn safe_route_name(
        candidate: &str,
        used: &mut BTreeSet<String>,
        fallback_suffix: &str,
    ) -> String {
        let base = if candidate.trim().is_empty() {
            "route".to_string()
        } else {
            candidate.trim().to_string()
        };
        let mut name = base.clone();
        if used.insert(name.clone()) {
            return name;
        }
        name = format!("{base}_{fallback_suffix}");
        let mut idx = 2usize;
        while !used.insert(name.clone()) {
            name = format!("{base}_{fallback_suffix}_{idx}");
            idx += 1;
        }
        name
    }

    fn route_from_legacy_routing(
        service_name: &str,
        view: &ServiceViewV3Legacy,
        routing: &RoutingConfigV3Legacy,
        warnings: &mut Vec<String>,
    ) -> Result<RoutingConfigV4> {
        let default_children = || view.providers.keys().cloned().collect::<Vec<_>>();
        let mut routes = BTreeMap::new();
        let entry = "main".to_string();

        let mut root = RoutingNodeV4 {
            on_exhausted: match routing.on_exhausted {
                RoutingExhaustedActionV3Legacy::Continue => RoutingExhaustedActionV4::Continue,
                RoutingExhaustedActionV3Legacy::Stop => RoutingExhaustedActionV4::Stop,
            },
            ..RoutingNodeV4::default()
        };

        match routing.policy {
            RoutingPolicyV3Legacy::ManualSticky => {
                root.strategy = RoutingPolicyV4::ManualSticky;
                root.target = routing
                    .target
                    .clone()
                    .or_else(|| routing.order.first().cloned())
                    .or_else(|| view.providers.keys().next().cloned());
                root.children = if routing.order.is_empty() {
                    root.target
                        .as_ref()
                        .map(|target| vec![target.clone()])
                        .unwrap_or_else(default_children)
                } else {
                    routing.order.clone()
                };
            }
            RoutingPolicyV3Legacy::OrderedFailover => {
                root.strategy = RoutingPolicyV4::OrderedFailover;
                root.children = if routing.order.is_empty() {
                    default_children()
                } else {
                    routing.order.clone()
                };
            }
            RoutingPolicyV3Legacy::TagPreferred => {
                root.strategy = RoutingPolicyV4::TagPreferred;
                root.children = if routing.order.is_empty() {
                    default_children()
                } else {
                    routing.order.clone()
                };
                root.prefer_tags = routing.prefer_tags.clone();
            }
            RoutingPolicyV3Legacy::PoolFallback => {
                root.strategy = RoutingPolicyV4::OrderedFailover;
                let chain = if routing.chain.is_empty() {
                    routing.pools.keys().cloned().collect::<Vec<_>>()
                } else {
                    routing.chain.clone()
                };
                if chain.is_empty() {
                    anyhow::bail!(
                        "[{service_name}] legacy pool-fallback routing requires at least one pool"
                    );
                }
                let mut used = view.providers.keys().cloned().collect::<BTreeSet<_>>();
                used.insert(entry.clone());
                let mut root_children = Vec::new();
                for (idx, pool_name) in chain.iter().enumerate() {
                    let Some(pool) = routing.pools.get(pool_name.as_str()) else {
                        anyhow::bail!(
                            "[{service_name}] legacy routing references missing pool '{pool_name}'"
                        );
                    };
                    if pool.providers.is_empty() {
                        anyhow::bail!(
                            "[{service_name}] legacy pool '{pool_name}' must define at least one provider"
                        );
                    }
                    let route_name = safe_route_name(
                        pool_name,
                        &mut used,
                        if idx == 0 { "pool" } else { "branch" },
                    );
                    if route_name != *pool_name {
                        warnings.push(format!(
                            "[{service_name}] legacy pool '{pool_name}' is renamed to route node '{route_name}' in v4"
                        ));
                    }
                    routes.insert(
                        route_name.clone(),
                        RoutingNodeV4 {
                            strategy: RoutingPolicyV4::OrderedFailover,
                            children: pool.providers.clone(),
                            target: None,
                            prefer_tags: Vec::new(),
                            on_exhausted: RoutingExhaustedActionV4::Continue,
                            metadata: BTreeMap::new(),
                            when: None,
                            then: None,
                            default_route: None,
                        },
                    );
                    root_children.push(route_name);
                }
                if matches!(routing.on_exhausted, RoutingExhaustedActionV3Legacy::Stop) {
                    root_children.truncate(1);
                }
                root.children = root_children;
            }
        }

        if root.children.is_empty() {
            root.children = default_children();
        }
        routes.insert(entry.clone(), root);
        let mut routing = RoutingConfigV4 {
            entry,
            routes,
            ..RoutingConfigV4::default()
        };
        routing.sync_compat_from_graph();
        Ok(routing)
    }

    fn migrate_service_view(
        service_name: &str,
        view: &ServiceViewV3Legacy,
        warnings: &mut Vec<String>,
    ) -> Result<ServiceViewV4> {
        let mut profiles = view.profiles.clone();
        for profile in profiles.values_mut() {
            if profile
                .station
                .as_deref()
                .map(str::trim)
                .is_some_and(|station| !station.is_empty())
            {
                profile.station = None;
            }
        }

        let routing = if let Some(routing) = view.routing.as_ref() {
            Some(route_from_legacy_routing(
                service_name,
                view,
                routing,
                warnings,
            )?)
        } else if view.providers.is_empty() {
            None
        } else {
            Some(RoutingConfigV4::ordered_failover(
                view.providers.keys().cloned().collect(),
            ))
        };

        Ok(ServiceViewV4 {
            default_profile: view.default_profile.clone(),
            profiles,
            providers: view.providers.clone(),
            routing,
        })
    }

    pub fn migrate_v3_legacy_to_v4(
        legacy: &ProxyConfigV3Legacy,
    ) -> Result<ConfigV4MigrationReport> {
        let mut warnings = Vec::new();
        let mut config = ProxyConfigV4 {
            version: CURRENT_ROUTE_GRAPH_CONFIG_VERSION,
            codex: migrate_service_view("codex", &legacy.codex, &mut warnings)?,
            claude: migrate_service_view("claude", &legacy.claude, &mut warnings)?,
            retry: legacy.retry.clone(),
            notify: legacy.notify.clone(),
            default_service: legacy.default_service,
            ui: legacy.ui.clone(),
        };
        if let Some(routing) = config.codex.routing.as_mut() {
            routing.sync_compat_from_graph();
        }
        if let Some(routing) = config.claude.routing.as_mut() {
            routing.sync_compat_from_graph();
        }
        Ok(ConfigV4MigrationReport { config, warnings })
    }
}

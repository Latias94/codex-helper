use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;

use super::ProxyService;
use super::api_responses::{ProfilesResponse, make_profiles_response};
use super::control_plane_service::{
    PersistedProxySettingsDocument, load_persisted_proxy_settings_document,
    load_persisted_proxy_settings_v2, runtime_service_manager_mut,
    save_persisted_proxy_settings_document_and_reload, save_persisted_proxy_settings_v2_and_reload,
    save_runtime_profile_settings_and_reload, save_runtime_proxy_settings_and_reload,
    service_view_v2, service_view_v2_mut, service_view_v4, service_view_v4_mut,
};

fn default_persisted_station_enabled() -> bool {
    true
}

fn default_persisted_station_level() -> u8 {
    1
}

fn default_persisted_routing_on_exhausted() -> crate::config::RoutingExhaustedActionV4 {
    crate::config::RoutingExhaustedActionV4::Continue
}

#[derive(serde::Deserialize)]
pub(super) struct PersistedStationUpdateRequest {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    level: Option<u8>,
}

#[derive(serde::Deserialize)]
pub(super) struct PersistedStationSpecUpsertRequest {
    #[serde(default)]
    alias: Option<String>,
    #[serde(default = "default_persisted_station_enabled")]
    enabled: bool,
    #[serde(default = "default_persisted_station_level")]
    level: u8,
    #[serde(default)]
    members: Vec<crate::config::GroupMemberRefV2>,
}

#[derive(serde::Deserialize)]
pub(super) struct PersistedProviderEndpointSpecUpsertRequest {
    name: String,
    base_url: String,
    #[serde(default = "default_persisted_station_enabled")]
    enabled: bool,
    #[serde(default)]
    priority: u32,
    #[serde(default)]
    tags: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(serde::Deserialize)]
pub(super) struct PersistedProviderSpecUpsertRequest {
    #[serde(default)]
    alias: Option<String>,
    #[serde(default = "default_persisted_station_enabled")]
    enabled: bool,
    #[serde(default)]
    auth_token_env: Option<String>,
    #[serde(default)]
    api_key_env: Option<String>,
    #[serde(default)]
    tags: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    endpoints: Vec<PersistedProviderEndpointSpecUpsertRequest>,
}

struct SanitizedPersistedProviderSpec {
    spec: crate::config::PersistedProviderSpec,
    tags_provided: bool,
    endpoint_tags_provided: std::collections::BTreeMap<String, bool>,
}

#[derive(serde::Deserialize)]
pub(super) struct PersistedProfileUpsertRequest {
    #[serde(default)]
    extends: Option<String>,
    #[serde(default)]
    station: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    reasoning_effort: Option<String>,
    #[serde(default)]
    service_tier: Option<String>,
}

#[derive(serde::Deserialize)]
pub(super) struct PersistedStationActiveRequest {
    #[serde(default)]
    station_name: Option<String>,
}

impl PersistedStationActiveRequest {
    fn station_name(&self) -> Option<String> {
        self.station_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
    }
}

#[derive(serde::Deserialize)]
pub(super) struct PersistedDefaultProfileRequest {
    profile_name: Option<String>,
}

#[derive(serde::Deserialize)]
pub(super) struct PersistedRoutingUpsertRequest {
    #[serde(default)]
    entry: Option<String>,
    #[serde(default)]
    routes: Option<std::collections::BTreeMap<String, crate::config::RoutingNodeV4>>,
    #[serde(default)]
    policy: Option<crate::config::RoutingPolicyV4>,
    #[serde(default)]
    order: Vec<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    prefer_tags: Option<Vec<std::collections::BTreeMap<String, String>>>,
    #[serde(default)]
    chain: Vec<String>,
    #[serde(default)]
    pools: std::collections::BTreeMap<String, crate::config::RoutingPoolV4>,
    #[serde(default = "default_persisted_routing_on_exhausted")]
    on_exhausted: crate::config::RoutingExhaustedActionV4,
}

fn sanitize_profile_name(profile_name: &str) -> Result<String, (StatusCode, String)> {
    let profile_name = profile_name.trim();
    if profile_name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "profile name is required".to_string(),
        ));
    }
    Ok(profile_name.to_string())
}

fn sanitize_station_name(station_name: &str) -> Result<String, (StatusCode, String)> {
    let station_name = station_name.trim();
    if station_name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "station name is required".to_string(),
        ));
    }
    Ok(station_name.to_string())
}

fn sanitize_provider_name(provider_name: &str) -> Result<String, (StatusCode, String)> {
    let provider_name = provider_name.trim();
    if provider_name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "provider name is required".to_string(),
        ));
    }
    Ok(provider_name.to_string())
}

fn sanitize_profile_request(
    payload: PersistedProfileUpsertRequest,
) -> crate::config::ServiceControlProfile {
    fn normalize(value: Option<String>) -> Option<String> {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }

    crate::config::ServiceControlProfile {
        extends: normalize(payload.extends),
        station: normalize(payload.station),
        model: normalize(payload.model),
        reasoning_effort: normalize(payload.reasoning_effort),
        service_tier: normalize(payload.service_tier),
    }
}

fn profile_request_has_station_binding(payload: &PersistedProfileUpsertRequest) -> bool {
    payload
        .station
        .as_deref()
        .is_some_and(|station| !station.trim().is_empty())
}

fn normalize_optional_config_string(value: Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn sanitize_tag_map(
    tags: std::collections::BTreeMap<String, String>,
    context: &str,
) -> Result<std::collections::BTreeMap<String, String>, (StatusCode, String)> {
    let mut out = std::collections::BTreeMap::new();
    for (key, value) in tags {
        let key = key.trim();
        if key.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("{context} tag key is required"),
            ));
        }
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        out.insert(key.to_string(), value.to_string());
    }
    Ok(out)
}

fn sanitize_station_spec_request(
    payload: PersistedStationSpecUpsertRequest,
) -> Result<crate::config::PersistedStationSpec, (StatusCode, String)> {
    let mut members = Vec::new();
    for member in payload.members {
        let provider = member.provider.trim();
        if provider.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "station member provider is required".to_string(),
            ));
        }

        let mut endpoint_names = member
            .endpoint_names
            .into_iter()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>();
        endpoint_names.dedup();

        members.push(crate::config::GroupMemberRefV2 {
            provider: provider.to_string(),
            endpoint_names,
            preferred: member.preferred,
        });
    }

    Ok(crate::config::PersistedStationSpec {
        name: String::new(),
        alias: normalize_optional_config_string(payload.alias),
        enabled: payload.enabled,
        level: payload.level.clamp(1, 10),
        members,
    })
}

fn sanitize_provider_spec_request(
    payload: PersistedProviderSpecUpsertRequest,
) -> Result<SanitizedPersistedProviderSpec, (StatusCode, String)> {
    let mut endpoints = Vec::new();
    let mut endpoint_tags_provided = std::collections::BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for endpoint in payload.endpoints {
        let endpoint_name = endpoint.name.trim();
        if endpoint_name.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "provider endpoint name is required".to_string(),
            ));
        }
        let base_url = endpoint.base_url.trim();
        if base_url.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("provider endpoint '{}' base_url is required", endpoint_name),
            ));
        }
        if !seen.insert(endpoint_name.to_string()) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("duplicate provider endpoint '{}'", endpoint_name),
            ));
        }
        let tags_provided = endpoint.tags.is_some();
        let tags = endpoint
            .tags
            .map(|tags| sanitize_tag_map(tags, &format!("provider endpoint '{}'", endpoint_name)))
            .transpose()?
            .unwrap_or_default();

        endpoint_tags_provided.insert(endpoint_name.to_string(), tags_provided);
        endpoints.push(crate::config::PersistedProviderEndpointSpec {
            name: endpoint_name.to_string(),
            base_url: base_url.to_string(),
            enabled: endpoint.enabled,
            priority: endpoint.priority,
            tags,
        });
    }

    let tags_provided = payload.tags.is_some();
    let tags = payload
        .tags
        .map(|tags| sanitize_tag_map(tags, "provider"))
        .transpose()?
        .unwrap_or_default();

    Ok(SanitizedPersistedProviderSpec {
        spec: crate::config::PersistedProviderSpec {
            name: String::new(),
            alias: normalize_optional_config_string(payload.alias),
            enabled: payload.enabled,
            auth_token_env: normalize_optional_config_string(payload.auth_token_env),
            api_key_env: normalize_optional_config_string(payload.api_key_env),
            tags,
            endpoints,
        },
        tags_provided,
        endpoint_tags_provided,
    })
}

fn merge_persisted_provider_spec(
    existing: Option<&crate::config::ProviderConfigV2>,
    provider: &SanitizedPersistedProviderSpec,
) -> crate::config::ProviderConfigV2 {
    let spec = &provider.spec;
    let mut auth = existing
        .map(|provider| provider.auth.clone())
        .unwrap_or_default();
    auth.auth_token_env = spec.auth_token_env.clone();
    auth.api_key_env = spec.api_key_env.clone();

    crate::config::ProviderConfigV2 {
        alias: spec.alias.clone(),
        enabled: spec.enabled,
        auth,
        tags: if provider.tags_provided {
            spec.tags.clone()
        } else {
            existing
                .map(|provider| provider.tags.clone())
                .unwrap_or_default()
        },
        supported_models: existing
            .map(|provider| provider.supported_models.clone())
            .unwrap_or_default(),
        model_mapping: existing
            .map(|provider| provider.model_mapping.clone())
            .unwrap_or_default(),
        endpoints: spec
            .endpoints
            .iter()
            .map(|endpoint| {
                let existing_endpoint =
                    existing.and_then(|provider| provider.endpoints.get(endpoint.name.as_str()));
                (
                    endpoint.name.clone(),
                    crate::config::ProviderEndpointV2 {
                        base_url: endpoint.base_url.clone(),
                        enabled: endpoint.enabled,
                        priority: endpoint.priority,
                        tags: if provider
                            .endpoint_tags_provided
                            .get(endpoint.name.as_str())
                            .copied()
                            .unwrap_or(false)
                        {
                            endpoint.tags.clone()
                        } else {
                            existing_endpoint
                                .map(|endpoint| endpoint.tags.clone())
                                .unwrap_or_default()
                        },
                        supported_models: existing_endpoint
                            .map(|endpoint| endpoint.supported_models.clone())
                            .unwrap_or_default(),
                        model_mapping: existing_endpoint
                            .map(|endpoint| endpoint.model_mapping.clone())
                            .unwrap_or_default(),
                    },
                )
            })
            .collect(),
    }
}

fn persisted_provider_spec_from_v4(
    name: &str,
    provider: &crate::config::ProviderConfigV4,
) -> crate::config::PersistedProviderSpec {
    let mut endpoints = Vec::new();
    if let Some(base_url) = provider
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        endpoints.push(crate::config::PersistedProviderEndpointSpec {
            name: "default".to_string(),
            base_url: base_url.to_string(),
            enabled: provider.enabled,
            priority: 0,
            tags: std::collections::BTreeMap::new(),
        });
    }
    endpoints.extend(provider.endpoints.iter().map(|(endpoint_name, endpoint)| {
        crate::config::PersistedProviderEndpointSpec {
            name: endpoint_name.clone(),
            base_url: endpoint.base_url.clone(),
            enabled: endpoint.enabled,
            priority: endpoint.priority,
            tags: endpoint.tags.clone(),
        }
    }));

    crate::config::PersistedProviderSpec {
        name: name.to_string(),
        alias: provider.alias.clone(),
        enabled: provider.enabled,
        auth_token_env: provider.inline_auth.auth_token_env.clone(),
        api_key_env: provider.inline_auth.api_key_env.clone(),
        tags: provider.tags.clone(),
        endpoints,
    }
}

fn persisted_routing_spec_from_v4(
    view: &crate::config::ServiceViewV4,
) -> crate::config::PersistedRoutingSpec {
    let routing = crate::config::effective_v4_routing(view);
    let order = crate::config::resolved_v4_provider_order("persisted-routing", view)
        .unwrap_or_else(|_| view.providers.keys().cloned().collect::<Vec<_>>());
    let entry_node = routing.entry_node();
    crate::config::PersistedRoutingSpec {
        entry: routing.entry.clone(),
        routes: routing.routes.clone(),
        policy: entry_node
            .map(|node| node.strategy)
            .unwrap_or(crate::config::RoutingPolicyV4::OrderedFailover),
        order: order.clone(),
        target: entry_node.and_then(|node| node.target.clone()),
        prefer_tags: entry_node
            .map(|node| node.prefer_tags.clone())
            .unwrap_or_default(),
        on_exhausted: entry_node
            .map(|node| node.on_exhausted)
            .unwrap_or(crate::config::RoutingExhaustedActionV4::Continue),
        entry_strategy: entry_node
            .map(|node| node.strategy)
            .unwrap_or(crate::config::RoutingPolicyV4::OrderedFailover),
        expanded_order: order,
        entry_target: entry_node.and_then(|node| node.target.clone()),
        providers: view
            .providers
            .iter()
            .map(
                |(name, provider)| crate::config::PersistedRoutingProviderRef {
                    name: name.clone(),
                    alias: provider.alias.clone(),
                    enabled: provider.enabled,
                    tags: provider.tags.clone(),
                },
            )
            .collect(),
    }
}

fn sanitize_routing_spec_request(
    view: &crate::config::ServiceViewV4,
    payload: PersistedRoutingUpsertRequest,
) -> Result<crate::config::RoutingConfigV4, (StatusCode, String)> {
    if !payload.chain.is_empty() || !payload.pools.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "pool routing is no longer part of the v4 authoring model; use nested routes instead"
                .to_string(),
        ));
    }

    let has_graph_update = payload.entry.is_some() || payload.routes.is_some();
    let has_entry_update = payload.policy.is_some()
        || !payload.order.is_empty()
        || payload.target.is_some()
        || payload.prefer_tags.is_some()
        || payload.on_exhausted != crate::config::RoutingExhaustedActionV4::Continue;

    let mut routing = crate::config::effective_v4_routing(view);
    if let Some(entry) = payload.entry {
        let entry = entry.trim();
        if entry.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "routing entry is required".to_string(),
            ));
        }
        routing.entry = entry.to_string();
    }
    if let Some(routes) = payload.routes {
        routing.routes = sanitize_route_nodes(routes)?;
    }

    if has_entry_update || !has_graph_update {
        let entry_name = routing.entry.clone();
        if !routing.routes.contains_key(entry_name.as_str()) {
            routing.routes.insert(
                entry_name.clone(),
                crate::config::RoutingNodeV4 {
                    strategy: crate::config::RoutingPolicyV4::OrderedFailover,
                    children: Vec::new(),
                    target: None,
                    prefer_tags: Vec::new(),
                    on_exhausted: crate::config::RoutingExhaustedActionV4::Continue,
                    metadata: std::collections::BTreeMap::new(),
                },
            );
        }
        let node = routing
            .routes
            .get_mut(entry_name.as_str())
            .expect("entry route inserted above");

        if let Some(policy) = payload.policy {
            node.strategy = policy;
        }

        let order = sanitize_route_children(payload.order)?;
        if !order.is_empty() {
            node.children = order;
        }

        if let Some(raw_target) = payload.target {
            let target = raw_target.trim();
            if target.is_empty() {
                node.target = None;
            } else {
                node.strategy = crate::config::RoutingPolicyV4::ManualSticky;
                node.target = Some(target.to_string());
                if node.children.is_empty() {
                    node.children = vec![target.to_string()];
                }
            }
        }

        if let Some(prefer_tags) = payload.prefer_tags {
            node.prefer_tags = sanitize_routing_prefer_tags(prefer_tags)?;
            if !node.prefer_tags.is_empty() {
                node.strategy = crate::config::RoutingPolicyV4::TagPreferred;
            }
        }

        node.on_exhausted = payload.on_exhausted;
        if !matches!(node.strategy, crate::config::RoutingPolicyV4::ManualSticky) {
            node.target = None;
        }
        if !matches!(node.strategy, crate::config::RoutingPolicyV4::TagPreferred) {
            node.prefer_tags.clear();
        }
        if node.children.is_empty() {
            node.children = view.providers.keys().cloned().collect();
        }
    }

    routing.sync_compat_from_graph();
    Ok(routing)
}

fn sanitize_route_nodes(
    routes: std::collections::BTreeMap<String, crate::config::RoutingNodeV4>,
) -> Result<std::collections::BTreeMap<String, crate::config::RoutingNodeV4>, (StatusCode, String)>
{
    let mut out = std::collections::BTreeMap::new();
    for (name, mut node) in routes {
        let route_name = name.trim();
        if route_name.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "routing route name is required".to_string(),
            ));
        }
        node.children = sanitize_route_children(node.children)?;
        if let Some(target) = node.target.take() {
            let target = target.trim();
            if !target.is_empty() {
                node.target = Some(target.to_string());
            }
        }
        node.prefer_tags = sanitize_routing_prefer_tags(node.prefer_tags)?;
        if !matches!(node.strategy, crate::config::RoutingPolicyV4::ManualSticky) {
            node.target = None;
        }
        if !matches!(node.strategy, crate::config::RoutingPolicyV4::TagPreferred) {
            node.prefer_tags.clear();
        }
        if out.insert(route_name.to_string(), node).is_some() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("duplicate routing route '{}'", route_name),
            ));
        }
    }
    Ok(out)
}

fn sanitize_route_children(children: Vec<String>) -> Result<Vec<String>, (StatusCode, String)> {
    let mut order = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for child_name in children {
        let child_name = child_name.trim();
        if child_name.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "routing child name is required".to_string(),
            ));
        }
        if !seen.insert(child_name.to_string()) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("duplicate routing child '{}'", child_name),
            ));
        }
        order.push(child_name.to_string());
    }
    Ok(order)
}

fn sanitize_routing_prefer_tags(
    prefer_tags: Vec<std::collections::BTreeMap<String, String>>,
) -> Result<Vec<std::collections::BTreeMap<String, String>>, (StatusCode, String)> {
    let mut out = Vec::new();
    for filter in prefer_tags {
        let normalized = filter
            .into_iter()
            .filter_map(|(key, value)| {
                let key = key.trim();
                let value = value.trim();
                (!key.is_empty() && !value.is_empty()).then(|| (key.to_string(), value.to_string()))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        if normalized.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "routing prefer_tags entries must contain at least one key/value pair".to_string(),
            ));
        }
        out.push(normalized);
    }
    Ok(out)
}

fn validate_v4_routing_spec_for_view(
    service_name: &str,
    view: &crate::config::ServiceViewV4,
    routing: &crate::config::RoutingConfigV4,
) -> Result<(), (StatusCode, String)> {
    let mut next_view = view.clone();
    next_view.routing = Some(routing.clone());
    crate::config::resolved_v4_provider_order(service_name, &next_view)
        .map(|_| ())
        .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))
}

fn v4_default_endpoint_can_be_inlined(existing: Option<&crate::config::ProviderConfigV4>) -> bool {
    existing
        .and_then(|provider| provider.endpoints.get("default"))
        .map(|endpoint| {
            endpoint.tags.is_empty()
                && endpoint.supported_models.is_empty()
                && endpoint.model_mapping.is_empty()
        })
        .unwrap_or(true)
}

fn merge_persisted_provider_spec_v4(
    existing: Option<&crate::config::ProviderConfigV4>,
    provider: &SanitizedPersistedProviderSpec,
) -> crate::config::ProviderConfigV4 {
    let spec = &provider.spec;
    let mut out = crate::config::ProviderConfigV4 {
        alias: spec.alias.clone(),
        enabled: spec.enabled,
        base_url: None,
        auth: existing
            .map(|provider| provider.auth.clone())
            .unwrap_or_default(),
        inline_auth: crate::config::UpstreamAuth {
            auth_token: existing.and_then(|provider| provider.inline_auth.auth_token.clone()),
            auth_token_env: spec.auth_token_env.clone(),
            api_key: existing.and_then(|provider| provider.inline_auth.api_key.clone()),
            api_key_env: spec.api_key_env.clone(),
        },
        tags: if provider.tags_provided {
            spec.tags.clone()
        } else {
            existing
                .map(|provider| provider.tags.clone())
                .unwrap_or_default()
        },
        supported_models: existing
            .map(|provider| provider.supported_models.clone())
            .unwrap_or_default(),
        model_mapping: existing
            .map(|provider| provider.model_mapping.clone())
            .unwrap_or_default(),
        endpoints: std::collections::BTreeMap::new(),
    };

    if spec.endpoints.len() == 1
        && spec.endpoints[0].name == "default"
        && spec.endpoints[0].priority == 0
        && spec.endpoints[0].tags.is_empty()
        && v4_default_endpoint_can_be_inlined(existing)
    {
        out.base_url = Some(spec.endpoints[0].base_url.clone());
    } else {
        out.endpoints = spec
            .endpoints
            .iter()
            .map(|endpoint| {
                let existing_endpoint =
                    existing.and_then(|provider| provider.endpoints.get(endpoint.name.as_str()));
                (
                    endpoint.name.clone(),
                    crate::config::ProviderEndpointV4 {
                        base_url: endpoint.base_url.clone(),
                        enabled: endpoint.enabled,
                        priority: endpoint.priority,
                        tags: if provider
                            .endpoint_tags_provided
                            .get(endpoint.name.as_str())
                            .copied()
                            .unwrap_or(false)
                        {
                            endpoint.tags.clone()
                        } else {
                            existing_endpoint
                                .map(|endpoint| endpoint.tags.clone())
                                .unwrap_or_default()
                        },
                        supported_models: existing_endpoint
                            .map(|endpoint| endpoint.supported_models.clone())
                            .unwrap_or_default(),
                        model_mapping: existing_endpoint
                            .map(|endpoint| endpoint.model_mapping.clone())
                            .unwrap_or_default(),
                    },
                )
            })
            .collect();
    }

    out
}

fn runtime_service_manager_for_document<'a>(
    runtime: &'a crate::config::ProxyConfig,
    service_name: &str,
) -> &'a crate::config::ServiceConfigManager {
    if service_name == "claude" {
        &runtime.claude
    } else {
        &runtime.codex
    }
}

fn ensure_v4_entry_route_mut(
    view: &mut crate::config::ServiceViewV4,
) -> &mut crate::config::RoutingNodeV4 {
    let routing = view
        .routing
        .get_or_insert_with(|| crate::config::RoutingConfigV4::default());
    if !routing.routes.contains_key(routing.entry.as_str()) {
        routing.routes.insert(
            routing.entry.clone(),
            crate::config::RoutingNodeV4::default(),
        );
    }
    routing
        .routes
        .get_mut(routing.entry.as_str())
        .expect("entry route inserted above")
}

fn append_new_provider_to_explicit_v4_order(
    view: &mut crate::config::ServiceViewV4,
    provider_name: &str,
) {
    let routing = ensure_v4_entry_route_mut(view);
    if routing.children.iter().any(|name| name == provider_name) {
        return;
    }
    routing.children.push(provider_name.to_string());
}

fn validate_station_members_for_view(
    service_name: &str,
    station_name: &str,
    view: &crate::config::ServiceViewV2,
    members: &[crate::config::GroupMemberRefV2],
) -> Result<(), (StatusCode, String)> {
    for member in members {
        let provider = view
            .providers
            .get(member.provider.as_str())
            .ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    format!(
                        "[{service_name}] station '{}' references missing provider '{}'",
                        station_name, member.provider
                    ),
                )
            })?;

        for endpoint_name in &member.endpoint_names {
            if !provider.endpoints.contains_key(endpoint_name.as_str()) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!(
                        "[{service_name}] station '{}' references missing endpoint '{}.{}'",
                        station_name, member.provider, endpoint_name
                    ),
                ));
            }
        }
    }
    Ok(())
}

pub(super) async fn list_persisted_station_specs(
    proxy: ProxyService,
) -> Result<Json<crate::config::PersistedStationsCatalog>, (StatusCode, String)> {
    match load_persisted_proxy_settings_document().await? {
        PersistedProxySettingsDocument::V2(cfg) => {
            Ok(Json(crate::config::build_persisted_station_catalog(
                service_view_v2(&cfg, proxy.service_name),
            )))
        }
        PersistedProxySettingsDocument::V4(_) => Err((
            StatusCode::BAD_REQUEST,
            "v4 route graph configs do not expose station specs; use the routing and provider specs APIs"
                .to_string(),
        )),
    }
}

pub(super) async fn list_persisted_provider_specs(
    proxy: ProxyService,
) -> Result<Json<crate::config::PersistedProvidersCatalog>, (StatusCode, String)> {
    match load_persisted_proxy_settings_document().await? {
        PersistedProxySettingsDocument::V2(cfg) => {
            Ok(Json(crate::config::build_persisted_provider_catalog(
                service_view_v2(&cfg, proxy.service_name),
            )))
        }
        PersistedProxySettingsDocument::V4(cfg) => {
            Ok(Json(crate::config::PersistedProvidersCatalog {
                providers: service_view_v4(&cfg, proxy.service_name)
                    .providers
                    .iter()
                    .map(|(name, provider)| persisted_provider_spec_from_v4(name, provider))
                    .collect(),
            }))
        }
    }
}

pub(super) async fn list_persisted_routing_spec(
    proxy: ProxyService,
) -> Result<Json<crate::config::PersistedRoutingSpec>, (StatusCode, String)> {
    match load_persisted_proxy_settings_document().await? {
        PersistedProxySettingsDocument::V4(cfg) => Ok(Json(persisted_routing_spec_from_v4(
            service_view_v4(&cfg, proxy.service_name),
        ))),
        PersistedProxySettingsDocument::V2(_) => Err((
            StatusCode::BAD_REQUEST,
            "routing API requires a version = 4 route graph config".to_string(),
        )),
    }
}

pub(super) async fn upsert_persisted_routing_spec(
    proxy: ProxyService,
    Json(payload): Json<PersistedRoutingUpsertRequest>,
) -> Result<Json<crate::config::PersistedRoutingSpec>, (StatusCode, String)> {
    let mut document = match load_persisted_proxy_settings_document().await? {
        PersistedProxySettingsDocument::V4(document) => document,
        PersistedProxySettingsDocument::V2(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                "routing API requires a version = 4 route graph config".to_string(),
            ));
        }
    };

    let view = service_view_v4_mut(&mut document, proxy.service_name);
    let routing = sanitize_routing_spec_request(view, payload)?;
    validate_v4_routing_spec_for_view(proxy.service_name, view, &routing)?;
    view.routing = Some(routing);
    crate::config::compile_v4_to_runtime(&document)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    save_persisted_proxy_settings_document_and_reload(
        &proxy,
        PersistedProxySettingsDocument::V4(document),
    )
    .await?;

    if let PersistedProxySettingsDocument::V4(cfg) =
        load_persisted_proxy_settings_document().await?
    {
        return Ok(Json(persisted_routing_spec_from_v4(service_view_v4(
            &cfg,
            proxy.service_name,
        ))));
    }

    unreachable!("saved routing document should reload as v4");
}

pub(super) async fn upsert_persisted_profile(
    proxy: ProxyService,
    Path(profile_name): Path<String>,
    Json(payload): Json<PersistedProfileUpsertRequest>,
) -> Result<Json<ProfilesResponse>, (StatusCode, String)> {
    let profile_name = sanitize_profile_name(profile_name.as_str())?;
    let has_station_binding = profile_request_has_station_binding(&payload);

    if let PersistedProxySettingsDocument::V4(mut document) =
        load_persisted_proxy_settings_document().await?
    {
        if has_station_binding {
            return Err((
                StatusCode::BAD_REQUEST,
                "v4 route graph profiles do not support station bindings; edit routing instead"
                    .to_string(),
            ));
        }
        let profile = sanitize_profile_request(payload);
        let view = service_view_v4_mut(&mut document, proxy.service_name);
        view.profiles.insert(profile_name.clone(), profile);
        let runtime = crate::config::compile_v4_to_runtime(&document)
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        let mgr = runtime_service_manager_for_document(&runtime, proxy.service_name);
        let resolved = crate::config::resolve_service_profile(mgr, profile_name.as_str())
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        crate::config::validate_profile_station_compatibility(
            proxy.service_name,
            mgr,
            profile_name.as_str(),
            &resolved,
        )
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        save_persisted_proxy_settings_document_and_reload(
            &proxy,
            PersistedProxySettingsDocument::V4(document),
        )
        .await?;
        return Ok(Json(make_profiles_response(&proxy).await));
    }

    let profile = sanitize_profile_request(payload);
    let cfg_snapshot = proxy.config.snapshot().await;
    let mut cfg = cfg_snapshot.as_ref().clone();
    let mgr = runtime_service_manager_mut(&mut cfg, proxy.service_name);

    mgr.profiles.insert(profile_name.clone(), profile);
    let resolved = crate::config::resolve_service_profile(mgr, profile_name.as_str())
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    crate::config::validate_profile_station_compatibility(
        proxy.service_name,
        mgr,
        profile_name.as_str(),
        &resolved,
    )
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(
        save_runtime_profile_settings_and_reload(&proxy, cfg).await?,
    ))
}

pub(super) async fn delete_persisted_profile(
    proxy: ProxyService,
    Path(profile_name): Path<String>,
) -> Result<Json<ProfilesResponse>, (StatusCode, String)> {
    let profile_name = sanitize_profile_name(profile_name.as_str())?;

    if let PersistedProxySettingsDocument::V4(mut document) =
        load_persisted_proxy_settings_document().await?
    {
        let view = service_view_v4_mut(&mut document, proxy.service_name);
        let referencing_profiles = view
            .profiles
            .iter()
            .filter_map(|(name, profile)| {
                (profile.extends.as_deref() == Some(profile_name.as_str())).then_some(name.clone())
            })
            .collect::<Vec<_>>();
        if !referencing_profiles.is_empty() {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "profile '{}' is extended by profiles: {}",
                    profile_name,
                    referencing_profiles.join(", ")
                ),
            ));
        }
        if view.profiles.remove(profile_name.as_str()).is_none() {
            return Err((
                StatusCode::NOT_FOUND,
                format!("profile '{}' not found", profile_name),
            ));
        }
        if view.default_profile.as_deref() == Some(profile_name.as_str()) {
            view.default_profile = None;
        }
        save_persisted_proxy_settings_document_and_reload(
            &proxy,
            PersistedProxySettingsDocument::V4(document),
        )
        .await?;
        if proxy
            .state
            .get_runtime_default_profile_override(proxy.service_name)
            .await
            .as_deref()
            == Some(profile_name.as_str())
        {
            proxy
                .state
                .clear_runtime_default_profile_override(proxy.service_name)
                .await;
        }
        return Ok(Json(make_profiles_response(&proxy).await));
    }

    let cfg_snapshot = proxy.config.snapshot().await;
    let mut cfg = cfg_snapshot.as_ref().clone();
    let mgr = runtime_service_manager_mut(&mut cfg, proxy.service_name);

    let referencing_profiles = mgr
        .profiles
        .iter()
        .filter_map(|(name, profile)| {
            (profile.extends.as_deref() == Some(profile_name.as_str())).then_some(name.clone())
        })
        .collect::<Vec<_>>();
    if !referencing_profiles.is_empty() {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "profile '{}' is extended by profiles: {}",
                profile_name,
                referencing_profiles.join(", ")
            ),
        ));
    }

    if mgr.profiles.remove(profile_name.as_str()).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("profile '{}' not found", profile_name),
        ));
    }
    if mgr.default_profile.as_deref() == Some(profile_name.as_str()) {
        mgr.default_profile = None;
    }

    save_runtime_profile_settings_and_reload(&proxy, cfg).await?;
    if proxy
        .state
        .get_runtime_default_profile_override(proxy.service_name)
        .await
        .as_deref()
        == Some(profile_name.as_str())
    {
        proxy
            .state
            .clear_runtime_default_profile_override(proxy.service_name)
            .await;
    }

    Ok(Json(make_profiles_response(&proxy).await))
}

pub(super) async fn update_persisted_station(
    proxy: ProxyService,
    Path(station_name): Path<String>,
    Json(payload): Json<PersistedStationUpdateRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let station_name = sanitize_station_name(station_name.as_str())?;
    if payload.enabled.is_none() && payload.level.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "at least one persisted station field must be provided".to_string(),
        ));
    }

    if matches!(
        load_persisted_proxy_settings_document().await?,
        PersistedProxySettingsDocument::V4(_)
    ) {
        return Err((
            StatusCode::BAD_REQUEST,
            "v4 route graph configs do not support station settings writes; edit providers and routing instead"
                .to_string(),
        ));
    }

    let cfg_snapshot = proxy.config.snapshot().await;
    let mut cfg = cfg_snapshot.as_ref().clone();
    let mgr = runtime_service_manager_mut(&mut cfg, proxy.service_name);
    let Some(station) = mgr.station_mut(station_name.as_str()) else {
        return Err((
            StatusCode::NOT_FOUND,
            format!("station '{}' not found", station_name),
        ));
    };
    if let Some(enabled) = payload.enabled {
        station.enabled = enabled;
    }
    if let Some(level) = payload.level {
        station.level = level.clamp(1, 10);
    }

    save_runtime_proxy_settings_and_reload(&proxy, cfg).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn set_persisted_active_station(
    proxy: ProxyService,
    Json(payload): Json<PersistedStationActiveRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let station_name = payload.station_name();

    if matches!(
        load_persisted_proxy_settings_document().await?,
        PersistedProxySettingsDocument::V4(_)
    ) {
        return Err((
            StatusCode::BAD_REQUEST,
            "v4 route graph configs do not support station active writes; edit routing instead"
                .to_string(),
        ));
    }

    let cfg_snapshot = proxy.config.snapshot().await;
    let mut cfg = cfg_snapshot.as_ref().clone();
    let mgr = runtime_service_manager_mut(&mut cfg, proxy.service_name);
    if let Some(station_name) = station_name.as_deref()
        && !mgr.contains_station(station_name)
    {
        return Err((
            StatusCode::NOT_FOUND,
            format!("station '{}' not found", station_name),
        ));
    }
    mgr.active = station_name;

    save_runtime_proxy_settings_and_reload(&proxy, cfg).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn upsert_persisted_station_spec(
    proxy: ProxyService,
    Path(station_name): Path<String>,
    Json(payload): Json<PersistedStationSpecUpsertRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let station_name = sanitize_station_name(station_name.as_str())?;
    let mut station = sanitize_station_spec_request(payload)?;
    station.name = station_name.clone();

    if matches!(
        load_persisted_proxy_settings_document().await?,
        PersistedProxySettingsDocument::V4(_)
    ) {
        return Err((
            StatusCode::BAD_REQUEST,
            "v4 route graph configs do not support station spec editing; edit providers and routing instead"
                .to_string(),
        ));
    }

    let mut cfg = load_persisted_proxy_settings_v2().await?;
    let view = service_view_v2_mut(&mut cfg, proxy.service_name);
    validate_station_members_for_view(
        proxy.service_name,
        station_name.as_str(),
        view,
        &station.members,
    )?;
    view.groups.insert(
        station_name.clone(),
        crate::config::GroupConfigV2 {
            alias: station.alias.clone(),
            enabled: station.enabled,
            level: station.level.clamp(1, 10),
            members: station.members.clone(),
        },
    );

    crate::config::compile_v2_to_runtime(&cfg)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    save_persisted_proxy_settings_v2_and_reload(&proxy, cfg).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn delete_persisted_station_spec(
    proxy: ProxyService,
    Path(station_name): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let station_name = sanitize_station_name(station_name.as_str())?;
    if matches!(
        load_persisted_proxy_settings_document().await?,
        PersistedProxySettingsDocument::V4(_)
    ) {
        return Err((
            StatusCode::BAD_REQUEST,
            "v4 route graph configs do not support station spec editing; edit providers and routing instead"
                .to_string(),
        ));
    }
    let mut cfg = load_persisted_proxy_settings_v2().await?;
    let view = service_view_v2_mut(&mut cfg, proxy.service_name);

    let referencing_profiles = view
        .profiles
        .iter()
        .filter_map(|(profile_name, profile)| {
            (profile.station.as_deref() == Some(station_name.as_str()))
                .then_some(profile_name.clone())
        })
        .collect::<Vec<_>>();
    if !referencing_profiles.is_empty() {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "station '{}' is referenced by profiles: {}",
                station_name,
                referencing_profiles.join(", ")
            ),
        ));
    }

    if view.groups.remove(station_name.as_str()).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("station '{}' not found", station_name),
        ));
    }
    if view.active_group.as_deref() == Some(station_name.as_str()) {
        view.active_group = None;
    }

    crate::config::compile_v2_to_runtime(&cfg)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    save_persisted_proxy_settings_v2_and_reload(&proxy, cfg).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn upsert_persisted_provider_spec(
    proxy: ProxyService,
    Path(provider_name): Path<String>,
    Json(payload): Json<PersistedProviderSpecUpsertRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let provider_name = sanitize_provider_name(provider_name.as_str())?;
    let mut provider = sanitize_provider_spec_request(payload)?;
    provider.spec.name = provider_name.clone();

    if let PersistedProxySettingsDocument::V4(mut document) =
        load_persisted_proxy_settings_document().await?
    {
        let view = service_view_v4_mut(&mut document, proxy.service_name);
        let existing_provider = view.providers.get(provider_name.as_str()).cloned();
        let is_new_provider = existing_provider.is_none();
        view.providers.insert(
            provider_name.clone(),
            merge_persisted_provider_spec_v4(existing_provider.as_ref(), &provider),
        );
        if is_new_provider {
            append_new_provider_to_explicit_v4_order(view, provider_name.as_str());
        }
        if !provider.spec.enabled {
            let entry = ensure_v4_entry_route_mut(view);
            if entry.target.as_deref() == Some(provider_name.as_str()) {
                entry.strategy = crate::config::RoutingPolicyV4::OrderedFailover;
                entry.target = None;
            }
        }
        crate::config::compile_v4_to_runtime(&document)
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        save_persisted_proxy_settings_document_and_reload(
            &proxy,
            PersistedProxySettingsDocument::V4(document),
        )
        .await?;
        return Ok(StatusCode::NO_CONTENT);
    }

    let mut cfg = load_persisted_proxy_settings_v2().await?;
    let view = service_view_v2_mut(&mut cfg, proxy.service_name);
    let existing_provider = view.providers.get(provider_name.as_str()).cloned();
    view.providers.insert(
        provider_name,
        merge_persisted_provider_spec(existing_provider.as_ref(), &provider),
    );

    crate::config::compile_v2_to_runtime(&cfg)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    save_persisted_proxy_settings_v2_and_reload(&proxy, cfg).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn delete_persisted_provider_spec(
    proxy: ProxyService,
    Path(provider_name): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let provider_name = sanitize_provider_name(provider_name.as_str())?;

    if let PersistedProxySettingsDocument::V4(mut document) =
        load_persisted_proxy_settings_document().await?
    {
        let view = service_view_v4_mut(&mut document, proxy.service_name);
        let Some(_) = view.providers.remove(provider_name.as_str()) else {
            return Err((
                StatusCode::NOT_FOUND,
                format!("provider '{}' not found", provider_name),
            ));
        };
        if let Some(routing) = view.routing.as_mut() {
            for node in routing.routes.values_mut() {
                node.children.retain(|name| name != &provider_name);
                if node.target.as_deref() == Some(provider_name.as_str()) {
                    node.target = None;
                    if matches!(node.strategy, crate::config::RoutingPolicyV4::ManualSticky) {
                        node.strategy = crate::config::RoutingPolicyV4::OrderedFailover;
                    }
                }
            }
        }
        save_persisted_proxy_settings_document_and_reload(
            &proxy,
            PersistedProxySettingsDocument::V4(document),
        )
        .await?;
        return Ok(StatusCode::NO_CONTENT);
    }

    let mut cfg = load_persisted_proxy_settings_v2().await?;
    let view = service_view_v2_mut(&mut cfg, proxy.service_name);

    let referencing_stations = view
        .groups
        .iter()
        .filter_map(|(station_name, station)| {
            station
                .members
                .iter()
                .any(|member| member.provider == provider_name)
                .then_some(station_name.clone())
        })
        .collect::<Vec<_>>();
    if !referencing_stations.is_empty() {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "provider '{}' is referenced by stations: {}",
                provider_name,
                referencing_stations.join(", ")
            ),
        ));
    }

    if view.providers.remove(provider_name.as_str()).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("provider '{}' not found", provider_name),
        ));
    }

    crate::config::compile_v2_to_runtime(&cfg)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    save_persisted_proxy_settings_v2_and_reload(&proxy, cfg).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn set_persisted_default_profile(
    proxy: ProxyService,
    Json(payload): Json<PersistedDefaultProfileRequest>,
) -> Result<Json<ProfilesResponse>, (StatusCode, String)> {
    let profile_name = payload
        .profile_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    if let PersistedProxySettingsDocument::V4(mut document) =
        load_persisted_proxy_settings_document().await?
    {
        if let Some(profile_name) = profile_name.as_deref() {
            let view = service_view_v4(&document, proxy.service_name);
            if !view.profiles.contains_key(profile_name) {
                return Err((
                    StatusCode::NOT_FOUND,
                    format!("profile '{}' not found", profile_name),
                ));
            }
            let runtime = crate::config::compile_v4_to_runtime(&document)
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            let mgr = runtime_service_manager_for_document(&runtime, proxy.service_name);
            let resolved = crate::config::resolve_service_profile(mgr, profile_name)
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            crate::config::validate_profile_station_compatibility(
                proxy.service_name,
                mgr,
                profile_name,
                &resolved,
            )
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        }
        let view = service_view_v4_mut(&mut document, proxy.service_name);
        view.default_profile = profile_name;
        save_persisted_proxy_settings_document_and_reload(
            &proxy,
            PersistedProxySettingsDocument::V4(document),
        )
        .await?;
        return Ok(Json(make_profiles_response(&proxy).await));
    }

    let cfg_snapshot = proxy.config.snapshot().await;
    let mut cfg = cfg_snapshot.as_ref().clone();
    let mgr = runtime_service_manager_mut(&mut cfg, proxy.service_name);

    if let Some(profile_name) = profile_name.as_deref() {
        if mgr.profile(profile_name).is_none() {
            return Err((
                StatusCode::NOT_FOUND,
                format!("profile '{}' not found", profile_name),
            ));
        }
        let resolved = crate::config::resolve_service_profile(mgr, profile_name)
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        crate::config::validate_profile_station_compatibility(
            proxy.service_name,
            mgr,
            profile_name,
            &resolved,
        )
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    }
    mgr.default_profile = profile_name;

    Ok(Json(
        save_runtime_profile_settings_and_reload(&proxy, cfg).await?,
    ))
}

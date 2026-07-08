use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use axum::Json;
use axum::extract::Query;
use axum::http::StatusCode;

use crate::config::{
    CURRENT_ROUTE_GRAPH_CONFIG_VERSION, ProviderConcurrencyLimits, ProxyConfig, ProxyConfigV2,
    ProxyConfigV4, ServiceConfig, ServiceConfigManager, ServiceViewV2, ServiceViewV4, UpstreamAuth,
    UpstreamConfig,
};
use crate::dashboard_core::{ProviderCapacity, ProviderOption, build_provider_options_from_view};
use crate::logging::{log_retry_trace, now_ms};
use crate::routing_ir::RouteCandidateConcurrency;
use crate::runtime_identity::ProviderEndpointKey;
use crate::state::RuntimeConfigState;
use crate::usage_providers::{
    UsageProviderRefreshOptions, UsageProviderRefreshSummary, refresh_balances_for_service,
};

use super::ProxyService;
use super::control_plane_service::{
    PersistedProxySettingsDocument, load_persisted_proxy_settings_document,
    load_persisted_proxy_settings_v2, service_view_v2, service_view_v4,
};

#[derive(serde::Deserialize)]
pub(super) struct ProviderRuntimeMetaRequest {
    provider_name: String,
    #[serde(default)]
    endpoint_name: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    clear_enabled: bool,
    #[serde(default)]
    runtime_state: Option<RuntimeConfigState>,
    #[serde(default)]
    clear_runtime_state: bool,
}

#[derive(serde::Deserialize, Default)]
pub(super) struct ProviderBalanceRefreshQuery {
    #[serde(default)]
    station_name: Option<String>,
    #[serde(default)]
    provider_id: Option<String>,
    #[serde(default)]
    force: bool,
}

fn normalize_provider_name(value: &str) -> Result<String, (StatusCode, String)> {
    let value = value.trim();
    if value.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "provider_name is required".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn normalize_optional_endpoint_name(value: Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn normalize_optional_filter(value: Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn provider_endpoint_override_key(
    service_name: &str,
    provider_name: &str,
    endpoint_name: &str,
) -> ProviderEndpointKey {
    ProviderEndpointKey::new(service_name, provider_name, endpoint_name)
}

fn canonical_provider_endpoint_override_key(
    service_name: &str,
    provider_name: &str,
    endpoint_name: &str,
    endpoint: &crate::config::ProviderEndpointV2,
) -> ProviderEndpointKey {
    let provider_id = endpoint
        .tags
        .get("provider_id")
        .map(String::as_str)
        .unwrap_or(provider_name);
    let endpoint_id = endpoint
        .tags
        .get("endpoint_id")
        .map(String::as_str)
        .unwrap_or(endpoint_name);
    provider_endpoint_override_key(service_name, provider_id, endpoint_id)
}

fn endpoint_base_url_is_unique(
    provider: &crate::config::ProviderConfigV2,
    endpoint_name: &str,
) -> bool {
    let Some(endpoint) = provider.endpoints.get(endpoint_name) else {
        return false;
    };
    provider
        .endpoints
        .values()
        .filter(|candidate| candidate.base_url == endpoint.base_url)
        .count()
        == 1
}

fn merge_refresh_summary(
    summary: &mut UsageProviderRefreshSummary,
    extra: UsageProviderRefreshSummary,
) {
    summary.providers_configured += extra.providers_configured;
    summary.providers_matched += extra.providers_matched;
    summary.upstreams_matched += extra.upstreams_matched;
    summary.attempted += extra.attempted;
    summary.refreshed += extra.refreshed;
    summary.failed += extra.failed;
    summary.missing_token += extra.missing_token;
    summary.auto_attempted += extra.auto_attempted;
    summary.auto_refreshed += extra.auto_refreshed;
    summary.auto_failed += extra.auto_failed;
    summary.deduplicated += extra.deduplicated;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProviderBalanceRefreshKey {
    service_name: String,
    station_name: Option<String>,
    provider_id: Option<String>,
    force: bool,
}

static IN_FLIGHT_PROVIDER_BALANCE_REFRESHES: OnceLock<Mutex<HashSet<ProviderBalanceRefreshKey>>> =
    OnceLock::new();

struct ProviderBalanceRefreshInFlight {
    key: ProviderBalanceRefreshKey,
}

impl Drop for ProviderBalanceRefreshInFlight {
    fn drop(&mut self) {
        if let Some(in_flight) = IN_FLIGHT_PROVIDER_BALANCE_REFRESHES.get()
            && let Ok(mut guard) = in_flight.lock()
        {
            guard.remove(&self.key);
        }
    }
}

fn try_mark_provider_balance_refresh_in_flight(
    key: ProviderBalanceRefreshKey,
) -> Option<ProviderBalanceRefreshInFlight> {
    let in_flight = IN_FLIGHT_PROVIDER_BALANCE_REFRESHES.get_or_init(|| Mutex::new(HashSet::new()));
    let mut guard = in_flight.lock().ok()?;
    if !guard.insert(key.clone()) {
        return None;
    }
    Some(ProviderBalanceRefreshInFlight { key })
}

fn merge_auth_v4(block: &UpstreamAuth, inline: &UpstreamAuth) -> UpstreamAuth {
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
    base: &BTreeMap<String, String>,
    overlay: &BTreeMap<String, String>,
) -> HashMap<String, String> {
    let mut out = base
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<HashMap<_, _>>();
    for (key, value) in overlay {
        out.insert(key.clone(), value.clone());
    }
    out
}

fn merge_bool_maps(
    base: &BTreeMap<String, bool>,
    overlay: &BTreeMap<String, bool>,
) -> HashMap<String, bool> {
    let mut out = base
        .iter()
        .map(|(key, value)| (key.clone(), *value))
        .collect::<HashMap<_, _>>();
    for (key, value) in overlay {
        out.insert(key.clone(), *value);
    }
    out
}

fn provider_tags_for_balance(
    provider_name: &str,
    endpoint_name: &str,
    provider_tags: &BTreeMap<String, String>,
    endpoint_tags: &BTreeMap<String, String>,
) -> HashMap<String, String> {
    let mut tags = merge_string_maps(provider_tags, endpoint_tags);
    tags.insert("provider_id".to_string(), provider_name.to_string());
    tags.insert("endpoint_id".to_string(), endpoint_name.to_string());
    tags
}

fn insert_optional_tag(tags: &mut HashMap<String, String>, key: &str, value: Option<&String>) {
    if let Some(value) = value
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        tags.insert(key.to_string(), value.to_string());
    }
}

fn service_manager_from_v4_provider_catalog(view: &ServiceViewV4) -> ServiceConfigManager {
    let mut configs = HashMap::new();
    for (provider_name, provider) in &view.providers {
        if !provider.enabled {
            continue;
        }

        let auth = merge_auth_v4(&provider.auth, &provider.inline_auth);
        let mut upstreams = Vec::new();
        if let Some(base_url) = provider
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let mut tags = provider_tags_for_balance(
                provider_name,
                "default",
                &provider.tags,
                &BTreeMap::new(),
            );
            insert_optional_tag(
                &mut tags,
                "continuity_domain",
                provider.continuity_domain.as_ref(),
            );
            insert_optional_tag(
                &mut tags,
                "provider_continuity_domain",
                provider.continuity_domain.as_ref(),
            );
            upstreams.push(UpstreamConfig {
                base_url: base_url.to_string(),
                auth: auth.clone(),
                tags,
                supported_models: provider
                    .supported_models
                    .iter()
                    .map(|(key, value)| (key.clone(), *value))
                    .collect(),
                model_mapping: provider
                    .model_mapping
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            });
        }

        let mut endpoints = provider.endpoints.iter().collect::<Vec<_>>();
        endpoints.sort_by(|(left_name, left), (right_name, right)| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| left_name.cmp(right_name))
                .then_with(|| left.base_url.cmp(&right.base_url))
        });
        for (endpoint_name, endpoint) in endpoints {
            if !endpoint.enabled {
                continue;
            }
            let mut tags = provider_tags_for_balance(
                provider_name,
                endpoint_name,
                &provider.tags,
                &endpoint.tags,
            );
            insert_optional_tag(
                &mut tags,
                "continuity_domain",
                endpoint.continuity_domain.as_ref(),
            );
            insert_optional_tag(
                &mut tags,
                "provider_continuity_domain",
                provider.continuity_domain.as_ref(),
            );
            upstreams.push(UpstreamConfig {
                base_url: endpoint.base_url.clone(),
                auth: auth.clone(),
                tags,
                supported_models: merge_bool_maps(
                    &provider.supported_models,
                    &endpoint.supported_models,
                ),
                model_mapping: merge_string_maps(&provider.model_mapping, &endpoint.model_mapping),
            });
        }

        if !upstreams.is_empty() {
            configs.insert(
                provider_name.clone(),
                ServiceConfig {
                    name: provider_name.clone(),
                    alias: provider.alias.clone(),
                    enabled: provider.enabled,
                    level: 1,
                    upstreams,
                },
            );
        }
    }

    ServiceConfigManager {
        active: view.routing.as_ref().and_then(|routing| {
            routing
                .entry_node()
                .and_then(|node| node.target.as_deref())
                .filter(|target| configs.contains_key(*target))
                .map(ToOwned::to_owned)
                .or_else(|| {
                    crate::config::resolved_v4_provider_order("providers_api", view)
                        .ok()
                        .and_then(|order| order.into_iter().find(|name| configs.contains_key(name)))
                })
        }),
        default_profile: view.default_profile.clone(),
        profiles: view.profiles.clone(),
        configs,
    }
}

fn service_manager_from_v2_provider_catalog(view: &ServiceViewV2) -> ServiceConfigManager {
    let mut configs = HashMap::new();
    for (provider_name, provider) in &view.providers {
        if !provider.enabled {
            continue;
        }

        let mut endpoints = provider.endpoints.iter().collect::<Vec<_>>();
        endpoints.sort_by(|(left_name, left), (right_name, right)| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| left_name.cmp(right_name))
                .then_with(|| left.base_url.cmp(&right.base_url))
        });

        let upstreams = endpoints
            .into_iter()
            .filter(|(_, endpoint)| endpoint.enabled)
            .map(|(endpoint_name, endpoint)| UpstreamConfig {
                base_url: endpoint.base_url.clone(),
                auth: provider.auth.clone(),
                tags: provider_tags_for_balance(
                    provider_name,
                    endpoint_name,
                    &provider.tags,
                    &endpoint.tags,
                ),
                supported_models: merge_bool_maps(
                    &provider.supported_models,
                    &endpoint.supported_models,
                ),
                model_mapping: merge_string_maps(&provider.model_mapping, &endpoint.model_mapping),
            })
            .collect::<Vec<_>>();

        if !upstreams.is_empty() {
            configs.insert(
                provider_name.clone(),
                ServiceConfig {
                    name: provider_name.clone(),
                    alias: provider.alias.clone(),
                    enabled: provider.enabled,
                    level: 1,
                    upstreams,
                },
            );
        }
    }

    ServiceConfigManager {
        active: None,
        default_profile: view.default_profile.clone(),
        profiles: view.profiles.clone(),
        configs,
    }
}

fn provider_catalog_runtime_from_v4(cfg: &ProxyConfigV4, service_name: &str) -> ProxyConfig {
    let mut runtime = ProxyConfig {
        version: Some(CURRENT_ROUTE_GRAPH_CONFIG_VERSION),
        retry: cfg.retry.clone(),
        notify: cfg.notify.clone(),
        default_service: cfg.default_service,
        relay_targets: cfg.relay_targets.clone(),
        ui: cfg.ui.clone(),
        ..ProxyConfig::default()
    };
    let mgr = service_manager_from_v4_provider_catalog(service_view_v4(cfg, service_name));
    match service_name {
        "claude" => runtime.claude = mgr,
        _ => runtime.codex = mgr,
    }
    runtime
}

fn provider_catalog_runtime_from_v2(cfg: &ProxyConfigV2, service_name: &str) -> ProxyConfig {
    let mut runtime = ProxyConfig {
        version: Some(2),
        retry: cfg.retry.clone(),
        notify: cfg.notify.clone(),
        default_service: cfg.default_service,
        relay_targets: cfg.relay_targets.clone(),
        ui: cfg.ui.clone(),
        ..ProxyConfig::default()
    };
    let mgr = service_manager_from_v2_provider_catalog(service_view_v2(cfg, service_name));
    match service_name {
        "claude" => runtime.claude = mgr,
        _ => runtime.codex = mgr,
    }
    runtime
}

async fn load_provider_catalog_runtime(
    service_name: &str,
) -> Result<Option<ProxyConfig>, (StatusCode, String)> {
    let document = load_persisted_proxy_settings_document().await?;
    let cfg = match document {
        PersistedProxySettingsDocument::V4(cfg) => {
            provider_catalog_runtime_from_v4(&cfg, service_name)
        }
        PersistedProxySettingsDocument::V2(cfg) => {
            provider_catalog_runtime_from_v2(&cfg, service_name)
        }
    };

    let mgr = match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    if mgr.has_stations() {
        Ok(Some(cfg))
    } else {
        Ok(None)
    }
}

fn resolve_target_base_urls(
    view: &crate::config::ServiceViewV2,
    provider_name: &str,
    endpoint_name: Option<&str>,
) -> Result<Vec<String>, (StatusCode, String)> {
    let provider = view.providers.get(provider_name).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("provider '{}' not found", provider_name),
        )
    })?;

    let mut urls = BTreeSet::new();
    if let Some(endpoint_name) = endpoint_name {
        let endpoint = provider.endpoints.get(endpoint_name).ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!(
                    "provider endpoint '{}.{}' not found",
                    provider_name, endpoint_name
                ),
            )
        })?;
        urls.insert(endpoint.base_url.clone());
    } else {
        for endpoint in provider.endpoints.values() {
            urls.insert(endpoint.base_url.clone());
        }
    }

    if urls.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("provider '{}' has no endpoints", provider_name),
        ));
    }
    Ok(urls.into_iter().collect())
}

fn capacity_limit_group(limits: &ProviderConcurrencyLimits) -> Option<String> {
    limits
        .limit_group
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn endpoint_effective_concurrency(
    provider_limits: &ProviderConcurrencyLimits,
    endpoint_limits: &ProviderConcurrencyLimits,
) -> RouteCandidateConcurrency {
    RouteCandidateConcurrency {
        max_concurrent_requests: endpoint_limits
            .max_concurrent_requests
            .or(provider_limits.max_concurrent_requests),
        limit_group: endpoint_limits
            .limit_group
            .as_ref()
            .or(provider_limits.limit_group.as_ref())
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
    }
}

fn provider_capacity_from_limits(limits: &ProviderConcurrencyLimits) -> ProviderCapacity {
    ProviderCapacity {
        configured_max_concurrent_requests: limits.max_concurrent_requests,
        configured_limit_group: capacity_limit_group(limits),
        effective_max_concurrent_requests: limits.max_concurrent_requests,
        effective_limit_group: capacity_limit_group(limits),
        active: None,
        limit: limits.max_concurrent_requests,
        limit_key: None,
        saturated: false,
        inherited_from_provider: None,
    }
}

fn endpoint_capacity_from_limits(
    proxy: &ProxyService,
    provider_name: &str,
    endpoint_name: &str,
    provider_limits: &ProviderConcurrencyLimits,
    endpoint_limits: &ProviderConcurrencyLimits,
) -> ProviderCapacity {
    let effective = endpoint_effective_concurrency(provider_limits, endpoint_limits);
    let provider_endpoint =
        ProviderEndpointKey::new(proxy.service_name, provider_name, endpoint_name);
    let limit_key = effective.limit_key(proxy.service_name, &provider_endpoint);
    let (active, limit, saturated) = match (effective.max_concurrent_requests, limit_key.as_deref())
    {
        (Some(limit), Some(key)) if limit > 0 => {
            let snapshot = proxy.concurrency_limiter.snapshot(key, limit);
            (
                Some(snapshot.active),
                Some(snapshot.limit),
                snapshot.saturated,
            )
        }
        (Some(limit), _) => (None, Some(limit), false),
        (None, _) => (None, None, false),
    };
    let endpoint_group = capacity_limit_group(endpoint_limits);
    let provider_group = capacity_limit_group(provider_limits);
    let inherited = (endpoint_limits.max_concurrent_requests.is_none()
        && provider_limits.max_concurrent_requests.is_some())
        || (endpoint_group.is_none() && provider_group.is_some());

    ProviderCapacity {
        configured_max_concurrent_requests: endpoint_limits.max_concurrent_requests,
        configured_limit_group: endpoint_group,
        effective_max_concurrent_requests: effective.max_concurrent_requests,
        effective_limit_group: effective.limit_group,
        active,
        limit,
        limit_key,
        saturated,
        inherited_from_provider: effective
            .max_concurrent_requests
            .is_some()
            .then_some(inherited),
    }
}

fn copy_endpoint_capacity_to_provider(
    provider_capacity: &mut ProviderCapacity,
    endpoint_capacity: &ProviderCapacity,
) {
    provider_capacity.active = endpoint_capacity.active;
    provider_capacity.limit = endpoint_capacity.limit;
    provider_capacity.limit_key = endpoint_capacity.limit_key.clone();
    provider_capacity.saturated = endpoint_capacity.saturated;
    provider_capacity.effective_max_concurrent_requests = provider_capacity
        .effective_max_concurrent_requests
        .or(endpoint_capacity.effective_max_concurrent_requests);
    provider_capacity.effective_limit_group = provider_capacity
        .effective_limit_group
        .clone()
        .or_else(|| endpoint_capacity.effective_limit_group.clone());
}

fn apply_shared_provider_group_snapshot(proxy: &ProxyService, provider: &mut ProviderOption) {
    let (Some(limit), Some(group), Some(first_endpoint)) = (
        provider.capacity.effective_max_concurrent_requests,
        provider.capacity.effective_limit_group.as_deref(),
        provider.endpoints.first(),
    ) else {
        return;
    };
    let concurrency = RouteCandidateConcurrency {
        max_concurrent_requests: Some(limit),
        limit_group: Some(group.to_string()),
    };
    let endpoint_key = ProviderEndpointKey::new(
        proxy.service_name,
        provider.name.as_str(),
        first_endpoint.name.as_str(),
    );
    let Some(limit_key) = concurrency.limit_key(proxy.service_name, &endpoint_key) else {
        return;
    };
    let snapshot = proxy
        .concurrency_limiter
        .snapshot(limit_key.as_str(), limit);
    provider.capacity.active = Some(snapshot.active);
    provider.capacity.limit = Some(snapshot.limit);
    provider.capacity.limit_key = Some(limit_key);
    provider.capacity.saturated = snapshot.saturated;
}

fn provider_endpoints_share_capacity(provider: &ProviderOption) -> bool {
    let Some(group) = provider.capacity.effective_limit_group.as_deref() else {
        return false;
    };
    let Some(limit) = provider.capacity.effective_max_concurrent_requests else {
        return false;
    };
    !provider.endpoints.is_empty()
        && provider.endpoints.iter().all(|endpoint| {
            endpoint.capacity.effective_limit_group.as_deref() == Some(group)
                && endpoint.capacity.effective_max_concurrent_requests == Some(limit)
        })
}

fn apply_provider_capacity_surface(
    proxy: &ProxyService,
    view: &ServiceViewV4,
    providers: &mut [ProviderOption],
) {
    for provider in providers {
        let Some(provider_cfg) = view.providers.get(provider.name.as_str()) else {
            continue;
        };
        provider.capacity = provider_capacity_from_limits(&provider_cfg.limits);

        for endpoint in &mut provider.endpoints {
            let default_endpoint_limits = ProviderConcurrencyLimits::default();
            let endpoint_limits = provider_cfg
                .endpoints
                .get(endpoint.name.as_str())
                .map(|endpoint_cfg| &endpoint_cfg.limits)
                .unwrap_or(&default_endpoint_limits);
            endpoint.capacity = endpoint_capacity_from_limits(
                proxy,
                provider.name.as_str(),
                endpoint.name.as_str(),
                &provider_cfg.limits,
                endpoint_limits,
            );
        }

        if provider.endpoints.len() == 1 {
            if let Some(endpoint) = provider.endpoints.first() {
                copy_endpoint_capacity_to_provider(&mut provider.capacity, &endpoint.capacity);
            }
        } else if provider_endpoints_share_capacity(provider) {
            apply_shared_provider_group_snapshot(proxy, provider);
        }
    }
}

async fn apply_provider_policy_action_surface(
    proxy: &ProxyService,
    providers: &mut [ProviderOption],
) {
    let projections = proxy
        .state
        .active_policy_action_projections(proxy.service_name, now_ms())
        .await;
    if projections.is_empty() {
        return;
    }
    let mut projections_by_endpoint: HashMap<String, Vec<_>> = HashMap::new();
    for projection in projections {
        projections_by_endpoint
            .entry(projection.provider_endpoint_key.stable_key())
            .or_default()
            .push(projection);
    }

    for provider in providers {
        for endpoint in &mut provider.endpoints {
            endpoint.policy_actions = projections_by_endpoint
                .get(&endpoint.provider_endpoint_key)
                .cloned()
                .unwrap_or_default();
        }
    }
}

pub(super) async fn build_provider_options_for_proxy(
    proxy: &ProxyService,
) -> Result<Vec<ProviderOption>, (StatusCode, String)> {
    let cfg = load_persisted_proxy_settings_v2().await?;
    let upstream_overrides = proxy
        .state
        .get_upstream_meta_overrides(proxy.service_name)
        .await;
    let mut providers = build_provider_options_from_view(
        proxy.service_name,
        service_view_v2(&cfg, proxy.service_name),
        &upstream_overrides,
    );
    if let Some(v4) = proxy.config.v4_snapshot().await {
        apply_provider_capacity_surface(
            proxy,
            service_view_v4(v4.as_ref(), proxy.service_name),
            &mut providers,
        );
    }
    apply_provider_policy_action_surface(proxy, &mut providers).await;
    Ok(providers)
}

pub(super) async fn list_providers(
    proxy: ProxyService,
) -> Result<Json<Vec<ProviderOption>>, (StatusCode, String)> {
    build_provider_options_for_proxy(&proxy).await.map(Json)
}

pub(super) async fn apply_provider_runtime_meta(
    proxy: ProxyService,
    Json(payload): Json<ProviderRuntimeMetaRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if payload.enabled.is_none()
        && payload.runtime_state.is_none()
        && !payload.clear_enabled
        && !payload.clear_runtime_state
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "at least one provider runtime action must be provided".to_string(),
        ));
    }

    let provider_name = normalize_provider_name(payload.provider_name.as_str())?;
    let endpoint_name = normalize_optional_endpoint_name(payload.endpoint_name);
    let cfg = load_persisted_proxy_settings_v2().await?;
    let view = service_view_v2(&cfg, proxy.service_name);
    let base_urls =
        resolve_target_base_urls(view, provider_name.as_str(), endpoint_name.as_deref())?;
    let runtime_state = payload.runtime_state;
    let applied_base_urls = base_urls.clone();

    let now = now_ms();
    if let Some(endpoint_name) = endpoint_name.as_deref() {
        let provider = view.providers.get(provider_name.as_str()).ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("provider '{}' not found", provider_name),
            )
        })?;
        let Some(endpoint) = provider.endpoints.get(endpoint_name) else {
            return Err((
                StatusCode::NOT_FOUND,
                format!(
                    "provider endpoint '{}.{}' not found",
                    provider_name, endpoint_name
                ),
            ));
        };
        let override_key = canonical_provider_endpoint_override_key(
            proxy.service_name,
            provider_name.as_str(),
            endpoint_name,
            endpoint,
        );
        if payload.clear_enabled {
            proxy
                .state
                .clear_provider_endpoint_enabled_override(proxy.service_name, &override_key)
                .await;
        } else if let Some(enabled) = payload.enabled {
            proxy
                .state
                .set_provider_endpoint_enabled_override(
                    proxy.service_name,
                    override_key.clone(),
                    enabled,
                    now,
                )
                .await;
        }
        if endpoint_base_url_is_unique(provider, endpoint_name) {
            if payload.clear_enabled {
                proxy
                    .state
                    .clear_upstream_enabled_override(proxy.service_name, endpoint.base_url.as_str())
                    .await;
            } else if let Some(enabled) = payload.enabled {
                proxy
                    .state
                    .set_upstream_enabled_override(
                        proxy.service_name,
                        endpoint.base_url.clone(),
                        enabled,
                        now,
                    )
                    .await;
            }
        }

        if payload.clear_runtime_state {
            proxy
                .state
                .clear_provider_endpoint_runtime_state_override(proxy.service_name, &override_key)
                .await;
        } else if let Some(runtime_state) = payload.runtime_state {
            proxy
                .state
                .set_provider_endpoint_runtime_state_override(
                    proxy.service_name,
                    override_key,
                    runtime_state,
                    now,
                )
                .await;
        }
        if endpoint_base_url_is_unique(provider, endpoint_name) {
            if payload.clear_runtime_state {
                proxy
                    .state
                    .clear_upstream_runtime_state_override(
                        proxy.service_name,
                        endpoint.base_url.as_str(),
                    )
                    .await;
            } else if let Some(runtime_state) = payload.runtime_state {
                proxy
                    .state
                    .set_upstream_runtime_state_override(
                        proxy.service_name,
                        endpoint.base_url.clone(),
                        runtime_state,
                        now,
                    )
                    .await;
            }
        }
    } else {
        for base_url in base_urls {
            if payload.clear_enabled {
                proxy
                    .state
                    .clear_upstream_enabled_override(proxy.service_name, base_url.as_str())
                    .await;
            } else if let Some(enabled) = payload.enabled {
                proxy
                    .state
                    .set_upstream_enabled_override(
                        proxy.service_name,
                        base_url.clone(),
                        enabled,
                        now,
                    )
                    .await;
            }

            if payload.clear_runtime_state {
                proxy
                    .state
                    .clear_upstream_runtime_state_override(proxy.service_name, base_url.as_str())
                    .await;
            } else if let Some(runtime_state) = payload.runtime_state {
                proxy
                    .state
                    .set_upstream_runtime_state_override(
                        proxy.service_name,
                        base_url.clone(),
                        runtime_state,
                        now,
                    )
                    .await;
            }
        }
    }

    log_retry_trace(serde_json::json!({
        "event": "provider_runtime_override",
        "service": proxy.service_name,
        "provider_name": provider_name,
        "endpoint_name": endpoint_name,
        "base_urls": applied_base_urls,
        "enabled": payload.enabled,
        "clear_enabled": payload.clear_enabled,
        "runtime_state": runtime_state.map(|state| format!("{state:?}").to_ascii_lowercase()),
        "clear_runtime_state": payload.clear_runtime_state,
    }));

    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn refresh_provider_balances(
    proxy: ProxyService,
    Query(query): Query<ProviderBalanceRefreshQuery>,
) -> Result<Json<super::ProviderBalanceRefreshResponse>, (StatusCode, String)> {
    let station_name = normalize_optional_filter(query.station_name);
    let provider_id = normalize_optional_filter(query.provider_id);
    let response = proxy
        .refresh_provider_balances(station_name.as_deref(), provider_id.as_deref(), query.force)
        .await
        .map_err(super::ProxyControlError::into_http_error)?;
    Ok(Json(response))
}

pub(super) async fn refresh_provider_balances_for_proxy(
    proxy: &ProxyService,
    station_name_filter: Option<&str>,
    provider_id_filter: Option<&str>,
    force: bool,
) -> Result<UsageProviderRefreshSummary, (StatusCode, String)> {
    let refresh_key = ProviderBalanceRefreshKey {
        service_name: proxy.service_name.to_string(),
        station_name: station_name_filter.map(ToOwned::to_owned),
        provider_id: provider_id_filter.map(ToOwned::to_owned),
        force,
    };
    let Some(_in_flight) = try_mark_provider_balance_refresh_in_flight(refresh_key) else {
        tracing::debug!(
            "provider balance refresh deduplicated: service={}, station={:?}, provider_id={:?}, force={}",
            proxy.service_name,
            station_name_filter,
            provider_id_filter,
            force
        );
        return Ok(UsageProviderRefreshSummary {
            deduplicated: 1,
            ..UsageProviderRefreshSummary::default()
        });
    };

    let cfg = proxy.config.snapshot().await;
    let mut refresh = refresh_balances_for_service(
        &proxy.client,
        cfg,
        proxy.lb_states.clone(),
        proxy.state.clone(),
        proxy.service_name,
        UsageProviderRefreshOptions {
            station_name_filter,
            provider_id_filter,
            force,
        },
    )
    .await;

    if let Some(provider_catalog_cfg) = load_provider_catalog_runtime(proxy.service_name).await? {
        let display_lb_states = Arc::new(Mutex::new(HashMap::new()));
        let provider_summary = refresh_balances_for_service(
            &proxy.client,
            Arc::new(provider_catalog_cfg),
            display_lb_states,
            proxy.state.clone(),
            proxy.service_name,
            UsageProviderRefreshOptions {
                station_name_filter,
                provider_id_filter,
                force,
            },
        )
        .await;
        merge_refresh_summary(&mut refresh, provider_summary);
    }

    Ok(refresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_balance_refresh_in_flight_guard_deduplicates_exact_key() {
        let key = ProviderBalanceRefreshKey {
            service_name: "codex-test-dedupe".to_string(),
            station_name: Some("monthly".to_string()),
            provider_id: Some("provider-a".to_string()),
            force: false,
        };

        let guard = try_mark_provider_balance_refresh_in_flight(key.clone())
            .expect("first refresh should enter");
        assert!(try_mark_provider_balance_refresh_in_flight(key.clone()).is_none());

        drop(guard);
        assert!(try_mark_provider_balance_refresh_in_flight(key).is_some());
    }

    #[test]
    fn provider_balance_refresh_in_flight_guard_keeps_force_separate() {
        let normal = ProviderBalanceRefreshKey {
            service_name: "codex-test-force-dedupe".to_string(),
            station_name: Some("monthly".to_string()),
            provider_id: Some("provider-a".to_string()),
            force: false,
        };
        let forced = ProviderBalanceRefreshKey {
            force: true,
            ..normal.clone()
        };

        let normal_guard = try_mark_provider_balance_refresh_in_flight(normal)
            .expect("normal refresh should enter");
        let forced_guard =
            try_mark_provider_balance_refresh_in_flight(forced).expect("forced refresh can enter");

        drop(normal_guard);
        drop(forced_guard);
    }
}

use std::collections::HashMap;
use std::sync::Arc;

use axum::http::StatusCode;

use crate::config::{ProviderConcurrencyLimits, ServiceRouteConfig};
use crate::dashboard_core::{
    ProviderCapacity, ProviderOption, build_provider_options_from_route_runtime,
};
use crate::logging::now_ms;
use crate::routing_ir::{CapturedRouteCandidate, RouteCandidateConcurrency};
use crate::runtime_identity::ProviderEndpointKey;
use crate::runtime_store::ProviderPolicySnapshot;
use crate::usage_providers::{
    UsageProviderRefreshOptions, UsageProviderRefreshSummary, UsageProviderRuntimeCapture,
    refresh_balances_for_service,
};

use super::ProxyService;
use super::concurrency_limits::ConcurrencyLimit;
use super::control_plane_service::service_route_config;
use super::route_target_selection::{
    apply_auth_resolution_to_runtime, apply_concurrency_snapshots_to_runtime,
};
use super::runtime_config::RuntimeSnapshot;

fn format_provider_balance_refresh_error(error: &anyhow::Error) -> String {
    format!("failed to refresh provider balances: {error:#}")
}

pub(super) fn enqueue_provider_balance_probe(
    client: reqwest::Client,
    state: Arc<crate::state::ProxyState>,
    target: CapturedRouteCandidate,
) {
    crate::usage_providers::enqueue_poll_for_captured_route_candidate(client, state, target);
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
    runtime_revision: u64,
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
            let observed_limit = ConcurrencyLimit::new(limit, runtime_revision)
                .expect("positive provider concurrency limit");
            let snapshot = proxy.concurrency_limiter.snapshot(key, observed_limit);
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

fn apply_shared_provider_group_snapshot(
    proxy: &ProxyService,
    runtime_revision: u64,
    provider: &mut ProviderOption,
) {
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
    let observed_limit = ConcurrencyLimit::new(limit, runtime_revision)
        .expect("positive provider concurrency limit");
    let snapshot = proxy
        .concurrency_limiter
        .snapshot(limit_key.as_str(), observed_limit);
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
    runtime_revision: u64,
    view: &ServiceRouteConfig,
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
                runtime_revision,
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
            apply_shared_provider_group_snapshot(proxy, runtime_revision, provider);
        }
    }
}

fn apply_provider_policy_action_surface(
    proxy: &ProxyService,
    provider_policy: &ProviderPolicySnapshot,
    providers: &mut [ProviderOption],
) {
    let projections = proxy.state.policy_action_projections_for_snapshot(
        proxy.service_name,
        now_ms(),
        provider_policy,
    );
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

pub(super) async fn build_provider_options_for_runtime_snapshot(
    proxy: &ProxyService,
    runtime_snapshot: &RuntimeSnapshot,
) -> Result<Vec<ProviderOption>, (StatusCode, String)> {
    let graph = runtime_snapshot
        .route_graph(proxy.service_name)
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!(
                    "captured runtime snapshot has no route graph for service '{}'",
                    proxy.service_name
                ),
            )
        })?;
    let template = graph.handshake_plan();
    let provider_policy = runtime_snapshot.provider_policy();
    let runtime_identities = template.candidate_identities().map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("captured runtime credential binding is invalid: {error}"),
        )
    })?;
    let mut runtime = proxy
        .state
        .route_plan_runtime_state_with_provider_policy(
            proxy.service_name,
            provider_policy.as_ref(),
            runtime_snapshot.revision(),
            runtime_identities.as_slice(),
        )
        .await;
    apply_auth_resolution_to_runtime(proxy.service_name, &template, &mut runtime).map_err(
        |error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("captured runtime credential binding is invalid: {error}"),
            )
        },
    )?;
    apply_concurrency_snapshots_to_runtime(
        proxy,
        &template,
        runtime_snapshot.revision(),
        &mut runtime,
    );
    let source = runtime_snapshot.config();
    let view = service_route_config(source.as_ref(), proxy.service_name);
    let mut providers =
        build_provider_options_from_route_runtime(proxy.service_name, view, &template, &runtime);
    apply_provider_capacity_surface(proxy, runtime_snapshot.revision(), view, &mut providers);
    apply_provider_policy_action_surface(proxy, provider_policy.as_ref(), &mut providers);
    Ok(providers)
}

pub(super) async fn refresh_provider_balances_for_proxy(
    proxy: &ProxyService,
    route_provider_id_filter: Option<&str>,
    provider_id_filter: Option<&str>,
    force: bool,
) -> Result<UsageProviderRefreshSummary, (StatusCode, String)> {
    let runtime_snapshot = proxy.config.capture().await;
    let refresh = refresh_balances_for_service(
        &proxy.client,
        UsageProviderRuntimeCapture::new(
            runtime_snapshot.config(),
            runtime_snapshot.credential_generation(),
        ),
        proxy.state.clone(),
        proxy.service_name,
        UsageProviderRefreshOptions {
            route_provider_id_filter,
            provider_id_filter,
            force,
        },
    )
    .await
    .map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format_provider_balance_refresh_error(&error),
        )
    })?;

    let provider_policy = proxy.state.capture_provider_policy_snapshot().await;
    proxy
        .config
        .publish_provider_policy(provider_policy)
        .await
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to publish committed provider policy: {error}"),
            )
        })?;

    Ok(refresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_balance_refresh_error_preserves_cause_chain() {
        let error = anyhow::anyhow!("unknown field `headers` at line 4")
            .context("failed to parse usage provider configuration");

        let detail = format_provider_balance_refresh_error(&error);

        assert!(detail.contains("failed to parse usage provider configuration"));
        assert!(detail.contains("unknown field `headers` at line 4"));
    }
}

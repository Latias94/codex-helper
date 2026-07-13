use std::collections::BTreeMap;

use crate::config::{ServiceControlProfile, ServiceRouteConfig};
use crate::routing_ir::{RoutePlanRuntimeState, RoutePlanTemplate};
use crate::runtime_identity::ProviderEndpointKey;

use super::types::{ControlProfileOption, ProviderEndpointOption, ProviderOption};

pub fn build_profile_options_from_route_view(
    view: &ServiceRouteConfig,
    default_name: Option<&str>,
) -> Vec<ControlProfileOption> {
    build_profile_options_from_catalog(&view.profiles, default_name)
}

fn build_profile_options_from_catalog(
    profiles: &BTreeMap<String, ServiceControlProfile>,
    default_name: Option<&str>,
) -> Vec<ControlProfileOption> {
    let mut profiles = profiles
        .iter()
        .map(|(name, profile)| ControlProfileOption {
            name: name.clone(),
            extends: profile.extends.clone(),
            model: profile.model.clone(),
            reasoning_effort: profile.reasoning_effort.clone(),
            service_tier: profile.service_tier.clone(),
            fast_mode: profile.service_tier.as_deref() == Some("priority"),
            is_default: default_name == Some(name.as_str()),
        })
        .collect::<Vec<_>>();
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    profiles
}

pub fn build_provider_options_from_route_runtime(
    service_name: &str,
    view: &ServiceRouteConfig,
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
) -> Vec<ProviderOption> {
    let mut providers = view
        .providers
        .iter()
        .map(|(provider_name, provider)| {
            let mut endpoints = Vec::new();
            if let Some(base_url) = provider
                .base_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                endpoints.push(build_route_provider_endpoint_option(
                    service_name,
                    provider_name,
                    "default",
                    base_url,
                    provider.enabled,
                    0,
                    provider.continuity_domain.as_deref(),
                    provider.continuity_domain.as_deref(),
                ));
            }
            endpoints.extend(provider.endpoints.iter().map(|(endpoint_name, endpoint)| {
                build_route_provider_endpoint_option(
                    service_name,
                    provider_name,
                    endpoint_name,
                    endpoint.base_url.as_str(),
                    provider.enabled && endpoint.enabled,
                    endpoint.priority,
                    endpoint.continuity_domain.as_deref(),
                    endpoint
                        .continuity_domain
                        .as_deref()
                        .or(provider.continuity_domain.as_deref()),
                )
            }));
            endpoints.sort_by(|a, b| {
                a.priority
                    .cmp(&b.priority)
                    .then_with(|| a.name.cmp(&b.name))
                    .then_with(|| a.base_url.cmp(&b.base_url))
            });

            ProviderOption {
                name: provider_name.clone(),
                alias: provider.alias.clone(),
                configured_enabled: provider.enabled,
                effective_enabled: provider.enabled
                    && endpoints.iter().any(|endpoint| endpoint.effective_enabled),
                routable_endpoints: endpoints
                    .iter()
                    .filter(|endpoint| endpoint.routable)
                    .count(),
                endpoints,
                capacity: Default::default(),
            }
        })
        .collect::<Vec<_>>();

    for provider in &mut providers {
        provider.effective_enabled = false;
        provider.routable_endpoints = 0;
        for endpoint in &mut provider.endpoints {
            endpoint.effective_enabled = false;
            endpoint.routable = false;
        }
    }

    let mut provider_order = BTreeMap::new();
    let mut endpoint_order = BTreeMap::<String, BTreeMap<String, usize>>::new();
    for candidate in &template.candidates {
        let next_provider_index = provider_order.len();
        provider_order
            .entry(candidate.provider_id.clone())
            .or_insert(next_provider_index);
        let endpoints = endpoint_order
            .entry(candidate.provider_id.clone())
            .or_default();
        let next_endpoint_index = endpoints.len();
        endpoints
            .entry(candidate.endpoint_id.clone())
            .or_insert(next_endpoint_index);

        let Some(provider) = providers
            .iter_mut()
            .find(|provider| provider.name == candidate.provider_id)
        else {
            continue;
        };
        let Some(endpoint) = provider
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.name == candidate.endpoint_id)
        else {
            continue;
        };
        let candidate_runtime = runtime.candidate_runtime_snapshot(template, candidate);
        endpoint.effective_enabled =
            endpoint.configured_enabled && !candidate_runtime.runtime_disabled;
        endpoint.routable = endpoint.configured_enabled && candidate_runtime.runtime_available;
    }

    for provider in &mut providers {
        if let Some(order) = endpoint_order.get(provider.name.as_str()) {
            provider.endpoints.sort_by(|left, right| {
                let left_order = order.get(left.name.as_str()).copied().unwrap_or(usize::MAX);
                let right_order = order
                    .get(right.name.as_str())
                    .copied()
                    .unwrap_or(usize::MAX);
                left_order
                    .cmp(&right_order)
                    .then_with(|| left.priority.cmp(&right.priority))
                    .then_with(|| left.name.cmp(&right.name))
                    .then_with(|| left.base_url.cmp(&right.base_url))
            });
        }
        provider.effective_enabled = provider
            .endpoints
            .iter()
            .any(|endpoint| endpoint.effective_enabled);
        provider.routable_endpoints = provider
            .endpoints
            .iter()
            .filter(|endpoint| endpoint.routable)
            .count();
    }
    providers.sort_by(|left, right| {
        let left_order = provider_order
            .get(left.name.as_str())
            .copied()
            .unwrap_or(usize::MAX);
        let right_order = provider_order
            .get(right.name.as_str())
            .copied()
            .unwrap_or(usize::MAX);
        left_order
            .cmp(&right_order)
            .then_with(|| left.name.cmp(&right.name))
    });
    providers
}

#[allow(clippy::too_many_arguments)]
fn build_route_provider_endpoint_option(
    service_name: &str,
    provider_name: &str,
    endpoint_name: &str,
    base_url: &str,
    configured_enabled: bool,
    priority: u32,
    continuity_domain: Option<&str>,
    effective_continuity_domain: Option<&str>,
) -> ProviderEndpointOption {
    let provider_endpoint_key =
        ProviderEndpointKey::new(service_name, provider_name, endpoint_name);

    ProviderEndpointOption {
        provider_name: provider_name.to_string(),
        name: endpoint_name.to_string(),
        provider_endpoint_key: provider_endpoint_key.stable_key(),
        base_url: base_url.to_string(),
        continuity_domain: normalize_optional_text(continuity_domain),
        effective_continuity_domain: normalize_optional_text(effective_continuity_domain),
        priority,
        configured_enabled,
        effective_enabled: configured_enabled,
        routable: configured_enabled,
        runtime_enabled_override: None,
        runtime_state: Default::default(),
        runtime_state_override: None,
        capacity: Default::default(),
        policy_actions: Vec::new(),
    }
}

fn normalize_optional_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

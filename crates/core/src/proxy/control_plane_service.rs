use std::collections::BTreeSet;

use crate::config::{HelperConfig, ServiceRouteConfig};

use super::ProxyService;

pub(super) fn service_route_config<'a>(
    config: &'a HelperConfig,
    service_name: &str,
) -> &'a ServiceRouteConfig {
    match service_name {
        "claude" => &config.claude,
        _ => &config.codex,
    }
}

pub(super) async fn prune_runtime_observability_after_reload(proxy: &ProxyService) {
    let snapshot = proxy.config.capture().await;
    let Some(graph) = snapshot.route_graph(proxy.service_name) else {
        return;
    };
    let active_provider_endpoints = graph
        .candidates()
        .iter()
        .map(|candidate| {
            crate::runtime_identity::ProviderEndpointKey::new(
                proxy.service_name,
                candidate.provider_id.clone(),
                candidate.endpoint_id.clone(),
            )
        })
        .collect();
    let active_limit_keys = graph
        .candidates()
        .iter()
        .filter(|candidate| {
            candidate
                .concurrency
                .max_concurrent_requests
                .is_some_and(|limit| limit > 0)
        })
        .filter_map(|candidate| {
            let provider_endpoint = crate::runtime_identity::ProviderEndpointKey::new(
                proxy.service_name,
                candidate.provider_id.clone(),
                candidate.endpoint_id.clone(),
            );
            candidate
                .concurrency
                .limit_key(proxy.service_name, &provider_endpoint)
        })
        .collect::<BTreeSet<_>>();
    proxy
        .state
        .prune_provider_endpoint_runtime_for_service(proxy.service_name, &active_provider_endpoints)
        .await;
    proxy.concurrency_limiter.prune_inactive(&active_limit_keys);
}

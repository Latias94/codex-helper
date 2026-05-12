use crate::lb::SelectedUpstream;
use crate::logging::{RouteAttemptLog, now_ms};
use crate::routing_ir::{RoutePlanRuntimeState, RoutePlanTemplate};
use crate::state::SessionRouteAffinityTarget;

use super::ProxyService;
use super::route_metadata::selected_route_metadata;

pub(super) async fn apply_session_route_affinity_to_runtime(
    proxy: &ProxyService,
    session_id: Option<&str>,
    template: &RoutePlanTemplate,
    route_graph_key: &str,
    runtime: &mut RoutePlanRuntimeState,
) {
    // V4 route graphs must not inherit the compatibility load balancer's global
    // last_good_index. Session stickiness is isolated by session_id below.
    runtime.clear_last_good_indices();

    let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    let Some(affinity) = proxy.state.get_session_route_affinity(session_id).await else {
        return;
    };
    if affinity.route_graph_key != route_graph_key {
        return;
    }
    let (Some(provider_id), Some(endpoint_id)) = (
        affinity.provider_id.as_deref(),
        affinity.endpoint_id.as_deref(),
    ) else {
        return;
    };
    let Some((station_name, upstream_index)) = template.compatibility_index_for_provider_endpoint(
        provider_id,
        endpoint_id,
        affinity.upstream_base_url.as_str(),
    ) else {
        return;
    };

    runtime.set_station_last_good_index(station_name, Some(upstream_index));
}

pub(super) async fn record_session_route_affinity_success(
    proxy: &ProxyService,
    session_id: Option<&str>,
    route_graph_key: Option<&str>,
    selected: &SelectedUpstream,
    route_attempts: &[RouteAttemptLog],
    route_attempt_index: usize,
) {
    let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    let Some(route_graph_key) = route_graph_key else {
        return;
    };
    let metadata = selected_route_metadata(selected);
    let target = SessionRouteAffinityTarget {
        route_graph_key: route_graph_key.to_string(),
        station_name: selected.station_name.clone(),
        upstream_index: selected.index,
        provider_id: metadata.provider_id,
        endpoint_id: metadata.endpoint_id,
        upstream_base_url: selected.upstream.base_url.clone(),
        route_path: metadata.route_path,
    };
    let reason_hint = route_affinity_change_reason(route_attempts, route_attempt_index);
    proxy
        .state
        .record_session_route_affinity_success(session_id, target, reason_hint, now_ms())
        .await;
}

fn route_affinity_change_reason(
    route_attempts: &[RouteAttemptLog],
    route_attempt_index: usize,
) -> Option<String> {
    route_attempts
        .iter()
        .take(route_attempt_index)
        .rev()
        .find(|attempt| {
            !attempt.skipped && !matches!(attempt.decision.as_str(), "selected" | "completed")
        })
        .map(|attempt| {
            if let Some(status_code) = attempt.status_code {
                return format!("failover_after_status_{status_code}");
            }
            if let Some(error_class) = attempt.error_class.as_deref() {
                return format!("failover_after_{error_class}");
            }
            if let Some(reason) = attempt.reason.as_deref() {
                return format!("failover_after_{reason}");
            }
            format!("failover_after_{}", attempt.decision)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_affinity_reason_uses_previous_failed_status() {
        let attempts = vec![
            RouteAttemptLog {
                decision: "failed_status".to_string(),
                status_code: Some(502),
                ..RouteAttemptLog::default()
            },
            RouteAttemptLog {
                decision: "completed".to_string(),
                status_code: Some(200),
                ..RouteAttemptLog::default()
            },
        ];

        assert_eq!(
            route_affinity_change_reason(&attempts, 1).as_deref(),
            Some("failover_after_status_502")
        );
    }
}

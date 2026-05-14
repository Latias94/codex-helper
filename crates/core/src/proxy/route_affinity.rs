use crate::logging::{RouteAttemptLog, now_ms};
use crate::routing_ir::{RoutePlanRuntimeState, RoutePlanTemplate};
use crate::state::SessionRouteAffinityTarget;

use super::ProxyService;
use super::attempt_target::AttemptTarget;

pub(super) async fn apply_session_route_affinity_to_runtime(
    proxy: &ProxyService,
    session_id: Option<&str>,
    template: &RoutePlanTemplate,
    route_graph_key: &str,
    runtime: &mut RoutePlanRuntimeState,
) {
    // V4 route graphs must not inherit any previously cached affinity.
    // Session stickiness is isolated by session_id below.
    runtime.clear_affinity_provider_endpoint();

    let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    let Some(affinity) = proxy.state.get_session_route_affinity(session_id).await else {
        return;
    };
    if affinity.route_graph_key != route_graph_key {
        return;
    }
    if !template.contains_provider_endpoint(
        &affinity.provider_endpoint,
        affinity.upstream_base_url.as_str(),
    ) {
        return;
    }

    runtime.set_affinity_provider_endpoint_with_observed_at(
        Some(affinity.provider_endpoint),
        Some(affinity.last_selected_at_ms),
        Some(affinity.last_changed_at_ms),
    );
}

pub(super) async fn record_session_route_affinity_success(
    proxy: &ProxyService,
    session_id: Option<&str>,
    route_graph_key: Option<&str>,
    target: &AttemptTarget,
    route_attempts: &[RouteAttemptLog],
    route_attempt_index: usize,
) {
    let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    let Some(route_graph_key) = route_graph_key else {
        return;
    };
    let Some(provider_endpoint) = target.provider_endpoint_ref().cloned() else {
        return;
    };
    let target = SessionRouteAffinityTarget {
        route_graph_key: route_graph_key.to_string(),
        provider_endpoint,
        upstream_base_url: target.upstream().base_url.clone(),
        route_path: target.route_path(),
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

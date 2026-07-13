use crate::logging::{RouteAttemptLog, now_ms};
use crate::routing_ir::{RoutePlanRuntimeState, RoutePlanTemplate};
use crate::state::{ProxyState, SessionIdentitySource, SessionRouteAffinityTarget};

use super::ProxyService;
use crate::routing_ir::CapturedRouteCandidate;

pub(super) struct SessionRouteAffinitySuccess {
    session_id: String,
    target: SessionRouteAffinityTarget,
    reason_hint: Option<String>,
}

impl SessionRouteAffinitySuccess {
    pub(super) async fn publish(self, state: &ProxyState) {
        if let Err(error) = state
            .record_session_route_affinity_success(
                self.session_id.as_str(),
                self.target,
                self.reason_hint,
                now_ms(),
            )
            .await
        {
            tracing::warn!(
                session_id = self.session_id,
                error = %error,
                "failed to publish durable session route affinity"
            );
        }
    }
}

pub(super) async fn apply_session_route_affinity_to_runtime(
    proxy: &ProxyService,
    session_id: Option<&str>,
    template: &RoutePlanTemplate,
    route_graph_key: &str,
    runtime: &mut RoutePlanRuntimeState,
) {
    // A fresh route runtime must not inherit any previously cached affinity.
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

pub(super) async fn apply_session_route_affinity_for_template(
    proxy: &ProxyService,
    session_id: Option<&str>,
    template: &RoutePlanTemplate,
    runtime: &mut RoutePlanRuntimeState,
) {
    let route_graph_key = template.route_graph_key();
    apply_session_route_affinity_to_runtime(
        proxy,
        session_id,
        template,
        route_graph_key.as_str(),
        runtime,
    )
    .await;
}

pub(super) async fn record_session_route_affinity_success(
    proxy: &ProxyService,
    session_id: Option<&str>,
    session_identity_source: Option<SessionIdentitySource>,
    route_graph_key: Option<&str>,
    target: &CapturedRouteCandidate,
    route_attempts: &[RouteAttemptLog],
    route_attempt_index: usize,
) {
    let Some(success) = prepare_session_route_affinity_success(
        session_id,
        session_identity_source,
        route_graph_key,
        target,
        route_attempts,
        route_attempt_index,
    ) else {
        return;
    };
    success.publish(proxy.state.as_ref()).await;
}

pub(super) fn prepare_session_route_affinity_success(
    session_id: Option<&str>,
    session_identity_source: Option<SessionIdentitySource>,
    route_graph_key: Option<&str>,
    target: &CapturedRouteCandidate,
    route_attempts: &[RouteAttemptLog],
    route_attempt_index: usize,
) -> Option<SessionRouteAffinitySuccess> {
    let session_id = session_id
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let route_graph_key = route_graph_key?;
    let target = SessionRouteAffinityTarget {
        route_graph_key: route_graph_key.to_string(),
        session_identity_source,
        provider_endpoint: target.provider_endpoint().clone(),
        upstream_base_url: target.base_url().to_owned(),
        route_path: target.route_path().to_vec(),
    };
    let reason_hint = route_affinity_change_reason(route_attempts, route_attempt_index);
    Some(SessionRouteAffinitySuccess {
        session_id: session_id.to_string(),
        target,
        reason_hint,
    })
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
            format!("failover_after_{}", attempt.stable_code())
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

    #[test]
    fn route_affinity_reason_uses_previous_error_class_without_status() {
        let attempts = vec![
            RouteAttemptLog {
                decision: "failed_transport".to_string(),
                error_class: Some("upstream_transport_error".to_string()),
                ..RouteAttemptLog::default()
            },
            RouteAttemptLog {
                decision: "completed".to_string(),
                ..RouteAttemptLog::default()
            },
        ];

        assert_eq!(
            route_affinity_change_reason(&attempts, 1).as_deref(),
            Some("failover_after_upstream_transport_error")
        );
    }

    #[test]
    fn route_affinity_reason_falls_back_to_stable_attempt_code() {
        let attempts = vec![
            RouteAttemptLog {
                decision: "failed_target_build".to_string(),
                ..RouteAttemptLog::default()
            },
            RouteAttemptLog {
                decision: "completed".to_string(),
                ..RouteAttemptLog::default()
            },
        ];

        assert_eq!(
            route_affinity_change_reason(&attempts, 1).as_deref(),
            Some("failover_after_failed_target_build")
        );
    }
}

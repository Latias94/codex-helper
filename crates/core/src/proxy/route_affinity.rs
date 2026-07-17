use crate::logging::{RouteAttemptLog, now_ms};
use crate::routing_ir::{RoutePlanRuntimeState, RoutePlanTemplate};
use crate::state::{
    SessionIdentitySource, SessionRouteAffinity, SessionRouteAffinitySuccess,
    SessionRouteAffinityTarget, SessionRouteControlGuard, SessionRouteReservationAccess,
};

use super::ProxyService;
use crate::routing_ir::CapturedRouteCandidate;

pub(super) enum SessionRouteReservationDecision {
    None,
    Available(SessionRouteAffinity),
    Busy,
    Failed,
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
    apply_matching_session_affinity(template, route_graph_key, runtime, affinity);
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

pub(super) async fn claim_session_route_reservation(
    proxy: &ProxyService,
    request_id: u64,
    session_id: Option<&str>,
    session_identity_source: Option<SessionIdentitySource>,
    route_graph_key: Option<&str>,
    target: &CapturedRouteCandidate,
) -> SessionRouteReservationDecision {
    let Some((session_id, target)) =
        session_route_affinity_target(session_id, session_identity_source, route_graph_key, target)
    else {
        return SessionRouteReservationDecision::None;
    };
    match proxy
        .state
        .claim_session_route_reservation(session_id.as_str(), target, request_id, now_ms())
        .await
    {
        Ok(SessionRouteReservationAccess::Available(affinity)) => {
            SessionRouteReservationDecision::Available(affinity)
        }
        Ok(SessionRouteReservationAccess::Busy { owner_request_id }) => {
            tracing::debug!(
                session_id,
                request_id,
                owner_request_id,
                "session route reservation is owned by another active request"
            );
            SessionRouteReservationDecision::Busy
        }
        Ok(SessionRouteReservationAccess::None) => SessionRouteReservationDecision::None,
        Err(error) => {
            tracing::warn!(
                session_id,
                error = %error,
                "failed to claim in-memory session route reservation"
            );
            SessionRouteReservationDecision::Failed
        }
    }
}

pub(super) async fn lock_session_route_reservation_selection(
    proxy: &ProxyService,
    session_id: Option<&str>,
) -> Option<SessionRouteControlGuard> {
    let session_id = session_id
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(proxy.state.lock_session_route_control(session_id).await)
}

pub(super) async fn apply_session_route_reservation_to_runtime(
    proxy: &ProxyService,
    request_id: u64,
    session_id: Option<&str>,
    template: &RoutePlanTemplate,
    route_graph_key: Option<&str>,
    runtime: &mut RoutePlanRuntimeState,
) -> SessionRouteReservationDecision {
    let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return SessionRouteReservationDecision::None;
    };
    let Some(route_graph_key) = route_graph_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return SessionRouteReservationDecision::None;
    };
    match proxy
        .state
        .get_session_route_reservation(session_id, route_graph_key, request_id, now_ms())
        .await
    {
        Ok(SessionRouteReservationAccess::Available(affinity)) => {
            if apply_matching_session_affinity(template, route_graph_key, runtime, affinity.clone())
            {
                SessionRouteReservationDecision::Available(affinity)
            } else {
                SessionRouteReservationDecision::None
            }
        }
        Ok(SessionRouteReservationAccess::Busy { owner_request_id }) => {
            tracing::debug!(
                session_id,
                request_id,
                owner_request_id,
                "session route reservation is owned by another active request"
            );
            SessionRouteReservationDecision::Busy
        }
        Ok(SessionRouteReservationAccess::None) => SessionRouteReservationDecision::None,
        Err(error) => {
            tracing::warn!(
                session_id,
                error = %error,
                "failed to read in-memory session route reservation"
            );
            SessionRouteReservationDecision::Failed
        }
    }
}

fn apply_matching_session_affinity(
    template: &RoutePlanTemplate,
    route_graph_key: &str,
    runtime: &mut RoutePlanRuntimeState,
    affinity: SessionRouteAffinity,
) -> bool {
    if affinity.route_graph_key != route_graph_key
        || !template.contains_provider_endpoint(
            &affinity.provider_endpoint,
            affinity.upstream_base_url.as_str(),
        )
    {
        return false;
    }
    runtime.set_affinity_provider_endpoint_with_observed_at(
        Some(affinity.provider_endpoint),
        Some(affinity.last_selected_at_ms),
        Some(affinity.last_changed_at_ms),
    );
    true
}

pub(super) fn prepare_session_route_affinity_success(
    request_id: u64,
    session_id: Option<&str>,
    session_identity_source: Option<SessionIdentitySource>,
    route_graph_key: Option<&str>,
    target: &CapturedRouteCandidate,
    route_attempts: &[RouteAttemptLog],
    route_attempt_index: usize,
) -> Option<SessionRouteAffinitySuccess> {
    let (session_id, target) = session_route_affinity_target(
        session_id,
        session_identity_source,
        route_graph_key,
        target,
    )?;
    let reason_hint = route_affinity_change_reason(route_attempts, route_attempt_index);
    Some(SessionRouteAffinitySuccess {
        request_id,
        session_id,
        target,
        reason_hint,
    })
}

fn session_route_affinity_target(
    session_id: Option<&str>,
    session_identity_source: Option<SessionIdentitySource>,
    route_graph_key: Option<&str>,
    target: &CapturedRouteCandidate,
) -> Option<(String, SessionRouteAffinityTarget)> {
    let session_id = session_id
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let route_graph_key = route_graph_key?;
    Some((
        session_id.to_string(),
        SessionRouteAffinityTarget {
            route_graph_key: route_graph_key.to_string(),
            session_identity_source,
            provider_endpoint: target.provider_endpoint().clone(),
            upstream_base_url: target.base_url().to_owned(),
            route_path: target.route_path().to_vec(),
        },
    ))
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

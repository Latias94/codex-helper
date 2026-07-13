use std::collections::HashSet;

use axum::http::StatusCode;

use crate::endpoint_health::CooldownBackoff;

use super::ProxyService;
use super::attempt_health::penalize_attempt_target;
use crate::routing_ir::CapturedRouteCandidate;

pub(super) struct TerminalUpstreamFailureParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) target: &'a CapturedRouteCandidate,
    pub(super) penalize_endpoint: bool,
    pub(super) cooldown_secs: u64,
    pub(super) cooldown_backoff: CooldownBackoff,
    pub(super) error_message: String,
    pub(super) avoid_set: &'a mut HashSet<usize>,
    pub(super) avoided_total: &'a mut usize,
    pub(super) last_err: &'a mut Option<(StatusCode, String)>,
}

pub(super) async fn apply_terminal_upstream_failure(params: TerminalUpstreamFailureParams<'_>) {
    let TerminalUpstreamFailureParams {
        proxy,
        target,
        penalize_endpoint,
        cooldown_secs,
        cooldown_backoff,
        error_message,
        avoid_set,
        avoided_total,
        last_err,
    } = params;

    if penalize_endpoint {
        penalize_attempt_target(
            proxy.state.as_ref(),
            proxy.service_name,
            target,
            cooldown_secs,
            cooldown_backoff,
        )
        .await;
    }

    if avoid_set.insert(target.attempt_avoid_index()) {
        *avoided_total = avoided_total.saturating_add(1);
    }
    *last_err = Some((StatusCode::BAD_GATEWAY, error_message));
}

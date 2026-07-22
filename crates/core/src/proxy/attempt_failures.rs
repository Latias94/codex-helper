use std::collections::HashSet;

use axum::http::StatusCode;

use crate::endpoint_health::{CooldownBackoff, RuntimeHealthDomain};

use super::ProxyService;
use super::attempt_health::{settle_half_open_probe_neutral, settle_or_penalize_attempt_target};
use crate::routing_ir::CapturedRouteCandidate;
use crate::state::DispatchedRuntimeHealthHalfOpenProbe;

pub(super) struct TerminalUpstreamFailureParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) target: &'a CapturedRouteCandidate,
    pub(super) health_domain: Option<RuntimeHealthDomain>,
    pub(super) cooldown_secs: u64,
    pub(super) cooldown_backoff: CooldownBackoff,
    pub(super) error_message: String,
    pub(super) half_open_probe: Option<DispatchedRuntimeHealthHalfOpenProbe>,
    pub(super) avoid_set: &'a mut HashSet<usize>,
    pub(super) avoided_total: &'a mut usize,
    pub(super) last_err: &'a mut Option<(StatusCode, String)>,
}

pub(super) async fn apply_terminal_upstream_failure(params: TerminalUpstreamFailureParams<'_>) {
    let TerminalUpstreamFailureParams {
        proxy,
        target,
        health_domain,
        cooldown_secs,
        cooldown_backoff,
        error_message,
        half_open_probe,
        avoid_set,
        avoided_total,
        last_err,
    } = params;

    if let Some(health_domain) = health_domain {
        settle_or_penalize_attempt_target(
            proxy.state.as_ref(),
            proxy.service_name,
            target,
            health_domain,
            cooldown_secs,
            cooldown_backoff,
            half_open_probe,
        )
        .await;
    } else {
        settle_half_open_probe_neutral(proxy.state.as_ref(), half_open_probe).await;
    }

    if avoid_set.insert(target.attempt_avoid_index()) {
        *avoided_total = avoided_total.saturating_add(1);
    }
    *last_err = Some((StatusCode::BAD_GATEWAY, error_message));
}

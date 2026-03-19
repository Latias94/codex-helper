use std::collections::HashSet;

use axum::http::StatusCode;

use crate::lb::{CooldownBackoff, LoadBalancer, SelectedUpstream};

use super::ProxyService;
use super::passive_health::record_passive_upstream_failure;

pub(super) struct TerminalUpstreamFailureParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) lb: Option<&'a LoadBalancer>,
    pub(super) selected: &'a SelectedUpstream,
    pub(super) error_class: &'a str,
    pub(super) penalize_reason: Option<&'a str>,
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
        lb,
        selected,
        error_class,
        penalize_reason,
        cooldown_secs,
        cooldown_backoff,
        error_message,
        avoid_set,
        avoided_total,
        last_err,
    } = params;

    if let (Some(lb), Some(reason)) = (lb, penalize_reason) {
        lb.penalize_with_backoff(selected.index, cooldown_secs, reason, cooldown_backoff);
    }

    record_passive_upstream_failure(
        proxy.state.as_ref(),
        proxy.service_name,
        &selected.station_name,
        &selected.upstream.base_url,
        Some(StatusCode::BAD_GATEWAY.as_u16()),
        Some(error_class),
        Some(error_message.clone()),
    )
    .await;

    if avoid_set.insert(selected.index) {
        *avoided_total = avoided_total.saturating_add(1);
    }
    *last_err = Some((StatusCode::BAD_GATEWAY, error_message));
}

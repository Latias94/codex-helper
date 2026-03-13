use std::collections::HashSet;

use axum::http::StatusCode;

use crate::lb::{CooldownBackoff, LoadBalancer, SelectedUpstream};

use super::{ProxyService, record_passive_upstream_failure};

pub(super) async fn apply_terminal_upstream_failure(
    proxy: &ProxyService,
    lb: Option<&LoadBalancer>,
    selected: &SelectedUpstream,
    error_class: &str,
    penalize_reason: Option<&str>,
    cooldown_secs: u64,
    cooldown_backoff: CooldownBackoff,
    error_message: String,
    avoid_set: &mut HashSet<usize>,
    avoided_total: &mut usize,
    last_err: &mut Option<(StatusCode, String)>,
) {
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

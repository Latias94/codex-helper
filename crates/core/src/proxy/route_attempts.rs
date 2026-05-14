use std::collections::HashSet;

use crate::logging::RouteAttemptLog;

use super::attempt_target::AttemptTarget;

pub(super) struct StartRouteAttemptParams<'a> {
    pub(super) target: &'a AttemptTarget,
    pub(super) provider_id: Option<&'a str>,
    pub(super) provider_attempt: u32,
    pub(super) upstream_attempt: u32,
    pub(super) provider_max_attempts: u32,
    pub(super) upstream_max_attempts: u32,
    pub(super) model_note: &'a str,
    pub(super) avoid_set: &'a HashSet<usize>,
    pub(super) avoided_total: usize,
    pub(super) total_upstreams: usize,
}

pub(super) struct UnsupportedModelSkipParams<'a> {
    pub(super) target: &'a AttemptTarget,
    pub(super) requested_model: &'a str,
    pub(super) provider_attempt: u32,
    pub(super) provider_max_attempts: u32,
    pub(super) avoid_set: &'a HashSet<usize>,
    pub(super) avoided_total: usize,
    pub(super) total_upstreams: usize,
}

pub(super) struct StatusRouteAttemptParams<'a> {
    pub(super) target: &'a AttemptTarget,
    pub(super) route_attempt_index: usize,
    pub(super) status_code: u16,
    pub(super) error_class: Option<&'a str>,
    pub(super) model_note: &'a str,
    pub(super) upstream_headers_ms: u64,
    pub(super) duration_ms: u64,
    pub(super) cooldown_secs: Option<u64>,
    pub(super) cooldown_reason: Option<&'a str>,
}

pub(super) struct ErrorRouteAttemptParams<'a> {
    pub(super) target: &'a AttemptTarget,
    pub(super) route_attempt_index: usize,
    pub(super) kind: RouteAttemptErrorKind,
    pub(super) reason: &'a str,
    pub(super) model_note: &'a str,
    pub(super) duration_ms: Option<u64>,
    pub(super) cooldown_secs: Option<u64>,
    pub(super) cooldown_reason: Option<&'a str>,
}

#[derive(Clone, Copy)]
pub(super) enum RouteAttemptErrorKind {
    TargetBuild,
    Transport,
    BodyRead,
    BodyTooLarge,
}

impl RouteAttemptErrorKind {
    fn chain_key(self) -> &'static str {
        match self {
            RouteAttemptErrorKind::TargetBuild => "target_build_error",
            RouteAttemptErrorKind::Transport => "transport_error",
            RouteAttemptErrorKind::BodyRead => "body_read_error",
            RouteAttemptErrorKind::BodyTooLarge => "body_too_large",
        }
    }

    fn decision(self) -> &'static str {
        match self {
            RouteAttemptErrorKind::TargetBuild => "failed_target_build",
            RouteAttemptErrorKind::Transport => "failed_transport",
            RouteAttemptErrorKind::BodyRead => "failed_body_read",
            RouteAttemptErrorKind::BodyTooLarge => "failed_body_too_large",
        }
    }

    fn error_class(self) -> &'static str {
        match self {
            RouteAttemptErrorKind::TargetBuild => "target_build_error",
            RouteAttemptErrorKind::Transport => "upstream_transport_error",
            RouteAttemptErrorKind::BodyRead => "upstream_body_read_error",
            RouteAttemptErrorKind::BodyTooLarge => "upstream_response_body_too_large",
        }
    }
}

pub(super) fn start_selected_route_attempt(
    route_attempts: &mut Vec<RouteAttemptLog>,
    params: StartRouteAttemptParams<'_>,
) -> usize {
    let raw = format!(
        "{} selected model={}",
        params.target.route_attempt_identity(),
        params.model_note
    );
    let attempt_index = route_attempts.len() as u32;
    route_attempts.push(RouteAttemptLog {
        attempt_index,
        provider_id: non_dash(params.provider_id)
            .map(ToOwned::to_owned)
            .or_else(|| params.target.provider_id().map(ToOwned::to_owned)),
        endpoint_id: params.target.endpoint_id(),
        provider_endpoint_key: params.target.provider_endpoint_key(),
        preference_group: params.target.preference_group(),
        route_path: params.target.route_path(),
        provider_attempt: Some(params.provider_attempt + 1),
        upstream_attempt: Some(params.upstream_attempt + 1),
        provider_max_attempts: Some(params.provider_max_attempts),
        upstream_max_attempts: Some(params.upstream_max_attempts),
        station_name: params
            .target
            .compatibility_station_name()
            .map(ToOwned::to_owned),
        upstream_base_url: Some(params.target.upstream().base_url.clone()),
        upstream_index: params.target.compatibility_upstream_index(),
        avoid_for_station: legacy_avoid_for_station(params.target, params.avoid_set),
        avoided_candidate_indices: avoided_candidate_indices(params.target, params.avoid_set),
        avoided_total: Some(params.avoided_total),
        total_upstreams: Some(params.total_upstreams),
        decision: "selected".to_string(),
        model: normalize_model(params.model_note),
        raw,
        ..Default::default()
    });
    route_attempts.len() - 1
}

pub(super) fn record_unsupported_model_skip(
    upstream_chain: &mut Vec<String>,
    route_attempts: &mut Vec<RouteAttemptLog>,
    params: UnsupportedModelSkipParams<'_>,
) {
    let raw = format!(
        "{} skipped_unsupported_model={}",
        params.target.route_attempt_identity(),
        params.requested_model
    );
    upstream_chain.push(raw.clone());
    route_attempts.push(RouteAttemptLog {
        attempt_index: route_attempts.len() as u32,
        provider_id: params.target.provider_id().map(ToOwned::to_owned),
        endpoint_id: params.target.endpoint_id(),
        provider_endpoint_key: params.target.provider_endpoint_key(),
        preference_group: params.target.preference_group(),
        route_path: params.target.route_path(),
        provider_attempt: Some(params.provider_attempt + 1),
        provider_max_attempts: Some(params.provider_max_attempts),
        station_name: params
            .target
            .compatibility_station_name()
            .map(ToOwned::to_owned),
        upstream_base_url: Some(params.target.upstream().base_url.clone()),
        upstream_index: params.target.compatibility_upstream_index(),
        avoid_for_station: legacy_avoid_for_station(params.target, params.avoid_set),
        avoided_candidate_indices: avoided_candidate_indices(params.target, params.avoid_set),
        avoided_total: Some(params.avoided_total),
        total_upstreams: Some(params.total_upstreams),
        decision: "skipped_capability_mismatch".to_string(),
        reason: Some("unsupported_model".to_string()),
        model: normalize_model(params.requested_model),
        skipped: true,
        raw,
        ..Default::default()
    });
}

pub(super) fn record_status_route_attempt(
    upstream_chain: &mut Vec<String>,
    route_attempts: &mut Vec<RouteAttemptLog>,
    params: StatusRouteAttemptParams<'_>,
) {
    let class_for_chain = params.error_class.unwrap_or("-");
    let raw = format!(
        "{} status={} class={} model={}",
        params.target.route_attempt_identity(),
        params.status_code,
        class_for_chain,
        params.model_note
    );
    upstream_chain.push(raw.clone());

    if let Some(attempt) = route_attempts.get_mut(params.route_attempt_index) {
        if attempt.provider_id.is_none() {
            attempt.provider_id = params.target.provider_id().map(ToOwned::to_owned);
        }
        if attempt.endpoint_id.is_none() {
            attempt.endpoint_id = params.target.endpoint_id();
        }
        if attempt.provider_endpoint_key.is_none() {
            attempt.provider_endpoint_key = params.target.provider_endpoint_key();
        }
        if attempt.preference_group.is_none() {
            attempt.preference_group = params.target.preference_group();
        }
        if attempt.route_path.is_empty() {
            attempt.route_path = params.target.route_path();
        }
        attempt.decision = if (200..300).contains(&params.status_code) {
            "completed".to_string()
        } else {
            "failed_status".to_string()
        };
        attempt.status_code = Some(params.status_code);
        attempt.error_class = params.error_class.map(ToOwned::to_owned);
        attempt.model = normalize_model(params.model_note);
        attempt.upstream_headers_ms = Some(params.upstream_headers_ms);
        attempt.duration_ms = Some(params.duration_ms);
        attempt.cooldown_secs = normalize_cooldown(params.cooldown_secs);
        attempt.cooldown_reason = normalize_cooldown(params.cooldown_secs)
            .and_then(|_| params.cooldown_reason.map(ToOwned::to_owned));
        attempt.raw = raw;
        return;
    }

    route_attempts.push(RouteAttemptLog {
        attempt_index: route_attempts.len() as u32,
        provider_id: params.target.provider_id().map(ToOwned::to_owned),
        endpoint_id: params.target.endpoint_id(),
        provider_endpoint_key: params.target.provider_endpoint_key(),
        preference_group: params.target.preference_group(),
        route_path: params.target.route_path(),
        station_name: params
            .target
            .compatibility_station_name()
            .map(ToOwned::to_owned),
        upstream_base_url: Some(params.target.upstream().base_url.clone()),
        upstream_index: params.target.compatibility_upstream_index(),
        decision: if (200..300).contains(&params.status_code) {
            "completed".to_string()
        } else {
            "failed_status".to_string()
        },
        status_code: Some(params.status_code),
        error_class: params.error_class.map(ToOwned::to_owned),
        model: normalize_model(params.model_note),
        upstream_headers_ms: Some(params.upstream_headers_ms),
        duration_ms: Some(params.duration_ms),
        cooldown_secs: normalize_cooldown(params.cooldown_secs),
        cooldown_reason: normalize_cooldown(params.cooldown_secs)
            .and_then(|_| params.cooldown_reason.map(ToOwned::to_owned)),
        raw,
        ..Default::default()
    });
}

pub(super) fn record_error_route_attempt(
    upstream_chain: &mut Vec<String>,
    route_attempts: &mut Vec<RouteAttemptLog>,
    params: ErrorRouteAttemptParams<'_>,
) {
    let raw = format!(
        "{} {}={} model={}",
        params.target.route_attempt_identity(),
        params.kind.chain_key(),
        params.reason,
        params.model_note
    );
    upstream_chain.push(raw.clone());

    if let Some(attempt) = route_attempts.get_mut(params.route_attempt_index) {
        if attempt.provider_id.is_none() {
            attempt.provider_id = params.target.provider_id().map(ToOwned::to_owned);
        }
        if attempt.endpoint_id.is_none() {
            attempt.endpoint_id = params.target.endpoint_id();
        }
        if attempt.provider_endpoint_key.is_none() {
            attempt.provider_endpoint_key = params.target.provider_endpoint_key();
        }
        if attempt.preference_group.is_none() {
            attempt.preference_group = params.target.preference_group();
        }
        if attempt.route_path.is_empty() {
            attempt.route_path = params.target.route_path();
        }
        attempt.decision = params.kind.decision().to_string();
        attempt.reason = Some(params.reason.to_string());
        attempt.error_class = Some(params.kind.error_class().to_string());
        attempt.model = normalize_model(params.model_note);
        attempt.duration_ms = params.duration_ms;
        attempt.cooldown_secs = normalize_cooldown(params.cooldown_secs);
        attempt.cooldown_reason = normalize_cooldown(params.cooldown_secs)
            .and_then(|_| params.cooldown_reason.map(ToOwned::to_owned));
        attempt.raw = raw;
        return;
    }

    route_attempts.push(RouteAttemptLog {
        attempt_index: route_attempts.len() as u32,
        provider_id: params.target.provider_id().map(ToOwned::to_owned),
        endpoint_id: params.target.endpoint_id(),
        provider_endpoint_key: params.target.provider_endpoint_key(),
        preference_group: params.target.preference_group(),
        route_path: params.target.route_path(),
        station_name: params
            .target
            .compatibility_station_name()
            .map(ToOwned::to_owned),
        upstream_base_url: Some(params.target.upstream().base_url.clone()),
        upstream_index: params.target.compatibility_upstream_index(),
        decision: params.kind.decision().to_string(),
        reason: Some(params.reason.to_string()),
        error_class: Some(params.kind.error_class().to_string()),
        model: normalize_model(params.model_note),
        duration_ms: params.duration_ms,
        cooldown_secs: normalize_cooldown(params.cooldown_secs),
        cooldown_reason: normalize_cooldown(params.cooldown_secs)
            .and_then(|_| params.cooldown_reason.map(ToOwned::to_owned)),
        raw,
        ..Default::default()
    });
}

fn sorted_avoid_set(avoid_set: &HashSet<usize>) -> Vec<usize> {
    let mut values = avoid_set.iter().copied().collect::<Vec<_>>();
    values.sort_unstable();
    values
}

fn legacy_avoid_for_station(target: &AttemptTarget, avoid_set: &HashSet<usize>) -> Vec<usize> {
    if target.uses_provider_endpoint_attempt_index() {
        Vec::new()
    } else {
        sorted_avoid_set(avoid_set)
    }
}

fn avoided_candidate_indices(target: &AttemptTarget, avoid_set: &HashSet<usize>) -> Vec<usize> {
    if target.uses_provider_endpoint_attempt_index() {
        sorted_avoid_set(avoid_set)
    } else {
        Vec::new()
    }
}

fn non_dash(value: Option<&str>) -> Option<&str> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "-")
}

fn normalize_model(value: &str) -> Option<String> {
    non_dash(Some(value)).map(ToOwned::to_owned)
}

fn normalize_cooldown(value: Option<u64>) -> Option<u64> {
    value.filter(|secs| *secs > 0)
}

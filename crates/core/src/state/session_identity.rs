use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config::ServiceConfigManager;
use crate::pricing::CostBreakdown;
use crate::sessions;
use crate::usage::UsageMetrics;

fn bool_is_false(value: &bool) -> bool {
    !*value
}

fn service_tier_is_fast(value: Option<&str>) -> bool {
    value
        .map(str::trim)
        .is_some_and(|tier| tier.eq_ignore_ascii_case("priority"))
}

fn generation_ms_from_duration(duration_ms: u64, ttfb_ms: Option<u64>) -> Option<u64> {
    if duration_ms == 0 {
        return None;
    }
    let ttfb_ms = ttfb_ms.unwrap_or(0);
    if ttfb_ms > 0 && ttfb_ms < duration_ms {
        Some(duration_ms.saturating_sub(ttfb_ms))
    } else {
        Some(duration_ms)
    }
}

fn default_attempt_count() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestObservability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens_per_second: Option<f64>,
    #[serde(default = "default_attempt_count")]
    pub attempt_count: u32,
    #[serde(default)]
    pub route_attempt_count: usize,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub retried: bool,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub cross_station_failover: bool,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub same_station_retry: bool,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub fast_mode: bool,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub streaming: bool,
}

impl Default for RequestObservability {
    fn default() -> Self {
        Self {
            trace_id: None,
            duration_ms: None,
            ttfb_ms: None,
            generation_ms: None,
            output_tokens_per_second: None,
            attempt_count: 1,
            route_attempt_count: 0,
            retried: false,
            cross_station_failover: false,
            same_station_retry: false,
            fast_mode: false,
            streaming: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveRequest {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_addr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub station_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_decision: Option<RouteDecisionProvenance>,
    pub service: String,
    pub method: String,
    pub path: String,
    pub started_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FinishedRequest {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_addr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub station_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_decision: Option<RouteDecisionProvenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageMetrics>,
    #[serde(default, skip_serializing_if = "CostBreakdown::is_unknown")]
    pub cost: CostBreakdown,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<crate::logging::RetryInfo>,
    #[serde(default)]
    pub observability: RequestObservability,
    pub service: String,
    pub method: String,
    pub path: String,
    pub status_code: u16,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub streaming: bool,
    pub ended_at_ms: u64,
}

impl FinishedRequest {
    pub fn observability_view(&self) -> RequestObservability {
        RequestObservability::from_finished_request(self)
    }

    pub fn refresh_observability(&mut self) {
        self.observability = self.observability_view();
    }

    pub fn generation_ms(&self) -> Option<u64> {
        self.observability_view().generation_ms
    }

    pub fn output_tokens_per_second(&self) -> Option<f64> {
        self.observability_view().output_tokens_per_second
    }

    pub fn attempt_count(&self) -> u32 {
        self.observability_view().attempt_count
    }

    pub fn is_fast_mode(&self) -> bool {
        self.observability_view().fast_mode
    }

    pub fn crossed_station_boundary(&self) -> bool {
        self.observability_view().cross_station_failover
    }
}

impl RequestObservability {
    pub fn from_finished_request(request: &FinishedRequest) -> Self {
        let retry = request.retry.as_ref();
        let attempt_count = retry.map(|retry| retry.attempts.max(1)).unwrap_or(1);
        let route_attempts = retry
            .map(|retry| retry.route_attempts_or_derived())
            .unwrap_or_default();
        let route_attempt_count = route_attempts.len();
        let final_station = request
            .station_name
            .as_deref()
            .map(str::trim)
            .filter(|station| !station.is_empty());
        let has_station_context = final_station.is_some()
            && route_attempts
                .iter()
                .any(|attempt| attempt.station_name.as_deref().is_some());
        let cross_station_failover = final_station.is_some_and(|final_station| {
            route_attempts
                .iter()
                .filter_map(|attempt| attempt.station_name.as_deref())
                .any(|station| station != final_station)
        });
        let retried = attempt_count > 1;
        let same_station_retry = retried && has_station_context && !cross_station_failover;
        let generation_ms = generation_ms_from_duration(request.duration_ms, request.ttfb_ms);
        let output_tokens_per_second = request.usage.as_ref().and_then(|usage| {
            if usage.output_tokens == 0 {
                return None;
            }
            let generation_ms = generation_ms?;
            if generation_ms == 0 {
                return None;
            }
            let rate = (usage.output_tokens as f64) / (generation_ms as f64 / 1000.0);
            rate.is_finite().then_some(rate).filter(|rate| *rate > 0.0)
        });
        let decided_fast = request
            .route_decision
            .as_ref()
            .and_then(|decision| decision.effective_service_tier.as_ref())
            .is_some_and(|value| service_tier_is_fast(Some(value.value.as_str())));

        Self {
            trace_id: request
                .trace_id
                .clone()
                .or_else(|| request.observability.trace_id.clone()),
            duration_ms: Some(request.duration_ms),
            ttfb_ms: request.ttfb_ms.filter(|value| *value > 0),
            generation_ms,
            output_tokens_per_second,
            attempt_count,
            route_attempt_count,
            retried,
            cross_station_failover,
            same_station_retry,
            fast_mode: decided_fast || service_tier_is_fast(request.service_tier.as_deref()),
            streaming: request.streaming || request.observability.streaming,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FinishRequestParams {
    pub id: u64,
    pub status_code: u16,
    pub duration_ms: u64,
    pub ended_at_ms: u64,
    pub observed_service_tier: Option<String>,
    pub usage: Option<UsageMetrics>,
    pub retry: Option<crate::logging::RetryInfo>,
    pub ttfb_ms: Option<u64>,
    pub streaming: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionStats {
    pub turns_total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_client_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_client_addr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_service_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_station_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_route_decision: Option<RouteDecisionProvenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_usage: Option<UsageMetrics>,
    pub total_usage: UsageMetrics,
    pub turns_with_usage: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_ended_at_ms: Option<u64>,
    pub last_seen_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionContinuityMode {
    #[default]
    DefaultProfile,
    ManualProfile,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionObservationScope {
    #[default]
    ObservedOnly,
    HostLocalEnriched,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionBinding {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub station_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    pub continuity_mode: SessionContinuityMode,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub last_seen_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RouteValueSource {
    RequestPayload,
    SessionOverride,
    GlobalOverride,
    ProfileDefault,
    StationMapping,
    RuntimeFallback,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedRouteValue {
    pub value: String,
    pub source: RouteValueSource,
}

impl ResolvedRouteValue {
    pub(crate) fn new(value: impl Into<String>, source: RouteValueSource) -> Self {
        Self {
            value: value.into(),
            source,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RouteDecisionProvenance {
    pub decided_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding_profile_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding_continuity_mode: Option<SessionContinuityMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_model: Option<ResolvedRouteValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_reasoning_effort: Option<ResolvedRouteValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_service_tier: Option<ResolvedRouteValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_station: Option<ResolvedRouteValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_upstream_base_url: Option<ResolvedRouteValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route_path: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionRouteAffinity {
    pub route_graph_key: String,
    pub station_name: String,
    pub upstream_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    pub upstream_base_url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route_path: Vec<String>,
    pub last_selected_at_ms: u64,
    pub last_changed_at_ms: u64,
    pub change_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRouteAffinityTarget {
    pub route_graph_key: String,
    pub station_name: String,
    pub upstream_index: usize,
    pub provider_id: Option<String>,
    pub endpoint_id: Option<String>,
    pub upstream_base_url: String,
    pub route_path: Vec<String>,
}

impl SessionRouteAffinityTarget {
    pub(crate) fn same_target(&self, affinity: &SessionRouteAffinity) -> bool {
        self.route_graph_key == affinity.route_graph_key
            && self.station_name == affinity.station_name
            && self.upstream_index == affinity.upstream_index
            && self.provider_id == affinity.provider_id
            && self.endpoint_id == affinity.endpoint_id
            && self.upstream_base_url == affinity.upstream_base_url
            && self.route_path == affinity.route_path
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionIdentityCard {
    pub session_id: Option<String>,
    #[serde(default)]
    pub observation_scope: SessionObservationScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_local_transcript_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_client_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_client_addr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub active_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_started_at_ms_min: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_ended_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_service_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_station_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_upstream_base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_usage: Option<UsageMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_usage: Option<UsageMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns_with_usage: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding_profile_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding_continuity_mode: Option<SessionContinuityMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_route_decision: Option<RouteDecisionProvenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_affinity: Option<SessionRouteAffinity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_model: Option<ResolvedRouteValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_reasoning_effort: Option<ResolvedRouteValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_service_tier: Option<ResolvedRouteValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_station: Option<ResolvedRouteValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_upstream_base_url: Option<ResolvedRouteValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub override_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub override_station_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub override_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub override_service_tier: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionManualOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub station_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
}

impl SessionManualOverrides {
    pub fn is_empty(&self) -> bool {
        self.reasoning_effort.is_none()
            && self.station_name.is_none()
            && self.model.is_none()
            && self.service_tier.is_none()
    }
}

#[derive(Debug, Clone)]
pub(super) struct SessionEffortOverride {
    pub(super) effort: String,
    #[allow(dead_code)]
    pub(super) updated_at_ms: u64,
    pub(super) last_seen_ms: u64,
}

#[derive(Debug, Clone)]
pub(super) struct SessionStationOverride {
    pub(super) station_name: String,
    #[allow(dead_code)]
    pub(super) updated_at_ms: u64,
    pub(super) last_seen_ms: u64,
}

#[derive(Debug, Clone)]
pub(super) struct SessionModelOverride {
    pub(super) model: String,
    #[allow(dead_code)]
    pub(super) updated_at_ms: u64,
    pub(super) last_seen_ms: u64,
}

#[derive(Debug, Clone)]
pub(super) struct SessionServiceTierOverride {
    pub(super) service_tier: String,
    #[allow(dead_code)]
    pub(super) updated_at_ms: u64,
    pub(super) last_seen_ms: u64,
}

#[derive(Debug, Clone)]
pub(super) struct SessionBindingEntry {
    pub(super) binding: SessionBinding,
}

#[derive(Debug, Clone)]
pub(super) struct SessionCwdCacheEntry {
    pub(super) cwd: Option<String>,
    pub(super) last_seen_ms: u64,
}

fn empty_session_identity_card(session_id: Option<String>) -> SessionIdentityCard {
    SessionIdentityCard {
        session_id,
        observation_scope: SessionObservationScope::ObservedOnly,
        host_local_transcript_path: None,
        last_client_name: None,
        last_client_addr: None,
        cwd: None,
        active_count: 0,
        active_started_at_ms_min: None,
        last_status: None,
        last_duration_ms: None,
        last_ended_at_ms: None,
        last_model: None,
        last_reasoning_effort: None,
        last_service_tier: None,
        last_provider_id: None,
        last_station_name: None,
        last_upstream_base_url: None,
        last_usage: None,
        total_usage: None,
        turns_total: None,
        turns_with_usage: None,
        binding_profile_name: None,
        binding_continuity_mode: None,
        last_route_decision: None,
        route_affinity: None,
        effective_model: None,
        effective_reasoning_effort: None,
        effective_service_tier: None,
        effective_station: None,
        effective_upstream_base_url: None,
        override_effort: None,
        override_station_name: None,
        override_model: None,
        override_service_tier: None,
    }
}

fn non_empty_trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn resolve_effective_observed_value(
    override_value: Option<&str>,
    observed_value: Option<&str>,
    binding_value: Option<&str>,
) -> Option<ResolvedRouteValue> {
    if let Some(value) = non_empty_trimmed(override_value) {
        return Some(ResolvedRouteValue::new(
            value,
            RouteValueSource::SessionOverride,
        ));
    }
    let binding_value = non_empty_trimmed(binding_value);
    if let Some(binding) = binding_value {
        return Some(ResolvedRouteValue::new(
            binding,
            RouteValueSource::ProfileDefault,
        ));
    }
    non_empty_trimmed(observed_value)
        .map(|observed| ResolvedRouteValue::new(observed, RouteValueSource::RequestPayload))
}

fn classify_session_observation_scope(card: &SessionIdentityCard) -> SessionObservationScope {
    if card.cwd.is_some() {
        SessionObservationScope::HostLocalEnriched
    } else {
        SessionObservationScope::ObservedOnly
    }
}

fn resolve_effective_station_value(
    card: &SessionIdentityCard,
    global_station_override: Option<&str>,
    binding_station_name: Option<&str>,
) -> Option<ResolvedRouteValue> {
    if let Some(value) = non_empty_trimmed(card.override_station_name.as_deref()) {
        return Some(ResolvedRouteValue::new(
            value,
            RouteValueSource::SessionOverride,
        ));
    }
    if let Some(value) = non_empty_trimmed(global_station_override) {
        return Some(ResolvedRouteValue::new(
            value,
            RouteValueSource::GlobalOverride,
        ));
    }
    let binding = non_empty_trimmed(binding_station_name);
    if let Some(binding) = binding {
        return Some(ResolvedRouteValue::new(
            binding,
            RouteValueSource::ProfileDefault,
        ));
    }
    non_empty_trimmed(card.last_station_name.as_deref())
        .map(|observed| ResolvedRouteValue::new(observed, RouteValueSource::RuntimeFallback))
}

fn apply_basic_effective_route(
    card: &mut SessionIdentityCard,
    global_station_override: Option<&str>,
    binding: Option<&SessionBinding>,
) {
    card.effective_model = resolve_effective_observed_value(
        card.override_model.as_deref(),
        card.last_model.as_deref(),
        binding.and_then(|binding| binding.model.as_deref()),
    );
    card.effective_reasoning_effort = resolve_effective_observed_value(
        card.override_effort.as_deref(),
        card.last_reasoning_effort.as_deref(),
        binding.and_then(|binding| binding.reasoning_effort.as_deref()),
    );
    card.effective_service_tier = resolve_effective_observed_value(
        card.override_service_tier.as_deref(),
        card.last_service_tier.as_deref(),
        binding.and_then(|binding| binding.service_tier.as_deref()),
    );
    card.binding_profile_name = binding.and_then(|binding| binding.profile_name.clone());
    card.binding_continuity_mode = binding.map(|binding| binding.continuity_mode);
    card.effective_station = resolve_effective_station_value(
        card,
        global_station_override,
        binding.and_then(|binding| binding.station_name.as_deref()),
    );
    card.effective_upstream_base_url = match (
        card.effective_station.as_ref(),
        non_empty_trimmed(card.last_station_name.as_deref()),
        non_empty_trimmed(card.last_upstream_base_url.as_deref()),
    ) {
        (Some(station), Some(last_station), Some(upstream)) if station.value == last_station => {
            Some(ResolvedRouteValue::new(
                upstream,
                RouteValueSource::RuntimeFallback,
            ))
        }
        _ => None,
    };
}

pub fn enrich_session_identity_cards_with_runtime(
    cards: &mut [SessionIdentityCard],
    mgr: &ServiceConfigManager,
) {
    for card in cards {
        if card.effective_station.is_none()
            && let Some(active) = mgr.active_station()
        {
            card.effective_station = Some(ResolvedRouteValue::new(
                active.name.clone(),
                RouteValueSource::RuntimeFallback,
            ));
        }

        let effective_station_name = card
            .effective_station
            .as_ref()
            .map(|value| value.value.as_str());
        if card.effective_upstream_base_url.is_none()
            && let Some(station_name) = effective_station_name
            && let Some(station) = mgr.station(station_name)
            && station.upstreams.len() == 1
        {
            card.effective_upstream_base_url = Some(ResolvedRouteValue::new(
                station.upstreams[0].base_url.clone(),
                RouteValueSource::RuntimeFallback,
            ));
        }

        let Some(model) = card
            .effective_model
            .as_ref()
            .map(|value| value.value.clone())
        else {
            continue;
        };
        let Some(station_name) = effective_station_name else {
            continue;
        };
        let Some(last_station_name) = card.last_station_name.as_deref() else {
            continue;
        };
        if last_station_name != station_name {
            continue;
        }
        let Some(last_upstream_base_url) = card.last_upstream_base_url.as_deref() else {
            continue;
        };
        let Some(station) = mgr.station(station_name) else {
            continue;
        };
        let Some(upstream) = station
            .upstreams
            .iter()
            .find(|upstream| upstream.base_url == last_upstream_base_url)
        else {
            continue;
        };

        let mapped = crate::model_routing::effective_model(&upstream.model_mapping, model.as_str());
        if mapped != model {
            card.effective_model = Some(ResolvedRouteValue::new(
                mapped,
                RouteValueSource::StationMapping,
            ));
        }
    }
}

fn session_identity_sort_key(card: &SessionIdentityCard) -> u64 {
    card.last_ended_at_ms
        .unwrap_or(0)
        .max(card.active_started_at_ms_min.unwrap_or(0))
}

fn route_decision_at_ms(route_decision: Option<&RouteDecisionProvenance>) -> u64 {
    route_decision
        .map(|decision| decision.decided_at_ms)
        .unwrap_or(0)
}

fn update_card_route_decision(
    card: &mut SessionIdentityCard,
    route_decision: Option<&RouteDecisionProvenance>,
) {
    let Some(route_decision) = route_decision.cloned() else {
        return;
    };
    let current_at = route_decision_at_ms(card.last_route_decision.as_ref());
    if current_at <= route_decision.decided_at_ms {
        card.last_route_decision = Some(route_decision);
    }
}

pub struct SessionIdentityCardBuildInputs<'a> {
    pub active: &'a [ActiveRequest],
    pub recent: &'a [FinishedRequest],
    pub overrides: &'a HashMap<String, String>,
    pub station_overrides: &'a HashMap<String, String>,
    pub model_overrides: &'a HashMap<String, String>,
    pub service_tier_overrides: &'a HashMap<String, String>,
    pub bindings: &'a HashMap<String, SessionBinding>,
    pub route_affinities: &'a HashMap<String, SessionRouteAffinity>,
    pub global_station_override: Option<&'a str>,
    pub stats: &'a HashMap<String, SessionStats>,
}

pub fn build_session_identity_cards_from_parts(
    inputs: SessionIdentityCardBuildInputs<'_>,
) -> Vec<SessionIdentityCard> {
    let SessionIdentityCardBuildInputs {
        active,
        recent,
        overrides,
        station_overrides,
        model_overrides,
        service_tier_overrides,
        bindings,
        route_affinities,
        global_station_override,
        stats,
    } = inputs;

    use std::collections::HashMap as StdHashMap;

    let mut map: StdHashMap<Option<String>, SessionIdentityCard> = StdHashMap::new();

    for req in active {
        let key = req.session_id.clone();
        let entry = map
            .entry(key.clone())
            .or_insert_with(|| empty_session_identity_card(key));

        entry.active_count = entry.active_count.saturating_add(1);
        entry.active_started_at_ms_min = Some(
            entry
                .active_started_at_ms_min
                .unwrap_or(req.started_at_ms)
                .min(req.started_at_ms),
        );
        if entry.cwd.is_none() {
            entry.cwd = req.cwd.clone();
        }
        if entry.last_client_name.is_none() {
            entry.last_client_name = req.client_name.clone();
        }
        if entry.last_client_addr.is_none() {
            entry.last_client_addr = req.client_addr.clone();
        }
        if let Some(effort) = req.reasoning_effort.as_ref() {
            entry.last_reasoning_effort = Some(effort.clone());
        }
        if let Some(service_tier) = req.service_tier.as_ref() {
            entry.last_service_tier = Some(service_tier.clone());
        }
        if entry.last_model.is_none() {
            entry.last_model = req.model.clone();
        }
        if entry.last_provider_id.is_none() {
            entry.last_provider_id = req.provider_id.clone();
        }
        if entry.last_station_name.is_none() {
            entry.last_station_name = req.station_name.clone();
        }
        if entry.last_upstream_base_url.is_none() {
            entry.last_upstream_base_url = req.upstream_base_url.clone();
        }
        update_card_route_decision(entry, req.route_decision.as_ref());
    }

    for r in recent {
        let key = r.session_id.clone();
        let entry = map
            .entry(key.clone())
            .or_insert_with(|| empty_session_identity_card(key));

        let should_update = entry
            .last_ended_at_ms
            .is_none_or(|prev| r.ended_at_ms >= prev);
        if should_update {
            entry.last_status = Some(r.status_code);
            entry.last_duration_ms = Some(r.duration_ms);
            entry.last_ended_at_ms = Some(r.ended_at_ms);
            entry.last_client_name = r.client_name.clone().or(entry.last_client_name.clone());
            entry.last_client_addr = r.client_addr.clone().or(entry.last_client_addr.clone());
            entry.last_model = r.model.clone().or(entry.last_model.clone());
            entry.last_reasoning_effort = r
                .reasoning_effort
                .clone()
                .or(entry.last_reasoning_effort.clone());
            entry.last_service_tier = r.service_tier.clone().or(entry.last_service_tier.clone());
            entry.last_provider_id = r.provider_id.clone().or(entry.last_provider_id.clone());
            entry.last_station_name = r.station_name.clone().or(entry.last_station_name.clone());
            entry.last_upstream_base_url = r
                .upstream_base_url
                .clone()
                .or(entry.last_upstream_base_url.clone());
            entry.last_usage = r.usage.clone().or(entry.last_usage.clone());
        }
        if entry.cwd.is_none() {
            entry.cwd = r.cwd.clone();
        }
        update_card_route_decision(entry, r.route_decision.as_ref());
    }

    for (sid, st) in stats {
        let key = Some(sid.clone());
        let entry = map
            .entry(key.clone())
            .or_insert_with(|| empty_session_identity_card(key));

        if entry.turns_total.is_none() {
            entry.turns_total = Some(st.turns_total);
        }
        if entry.last_client_name.is_none() {
            entry.last_client_name = st.last_client_name.clone();
        }
        if entry.last_client_addr.is_none() {
            entry.last_client_addr = st.last_client_addr.clone();
        }
        if entry.last_status.is_none() {
            entry.last_status = st.last_status;
        }
        if entry.last_duration_ms.is_none() {
            entry.last_duration_ms = st.last_duration_ms;
        }
        if entry.last_ended_at_ms.is_none() {
            entry.last_ended_at_ms = st.last_ended_at_ms;
        }
        if entry.last_model.is_none() {
            entry.last_model = st.last_model.clone();
        }
        if entry.last_reasoning_effort.is_none() {
            entry.last_reasoning_effort = st.last_reasoning_effort.clone();
        }
        if entry.last_service_tier.is_none() {
            entry.last_service_tier = st.last_service_tier.clone();
        }
        if entry.last_provider_id.is_none() {
            entry.last_provider_id = st.last_provider_id.clone();
        }
        if entry.last_station_name.is_none() {
            entry.last_station_name = st.last_station_name.clone();
        }
        if entry.last_usage.is_none() {
            entry.last_usage = st.last_usage.clone();
        }
        if entry.total_usage.is_none() {
            entry.total_usage = Some(st.total_usage.clone());
        }
        if entry.turns_with_usage.is_none() {
            entry.turns_with_usage = Some(st.turns_with_usage);
        }
        update_card_route_decision(entry, st.last_route_decision.as_ref());
    }

    for (sid, eff) in overrides {
        let key = Some(sid.clone());
        let entry = map
            .entry(key.clone())
            .or_insert_with(|| empty_session_identity_card(key));
        entry.override_effort = Some(eff.clone());
    }

    for (sid, station_name) in station_overrides {
        let key = Some(sid.clone());
        let entry = map
            .entry(key.clone())
            .or_insert_with(|| empty_session_identity_card(key));
        entry.override_station_name = Some(station_name.clone());
    }

    for (sid, model) in model_overrides {
        let key = Some(sid.clone());
        let entry = map
            .entry(key.clone())
            .or_insert_with(|| empty_session_identity_card(key));
        entry.override_model = Some(model.clone());
    }

    for (sid, service_tier) in service_tier_overrides {
        let key = Some(sid.clone());
        let entry = map
            .entry(key.clone())
            .or_insert_with(|| empty_session_identity_card(key));
        entry.override_service_tier = Some(service_tier.clone());
    }

    for (sid, affinity) in route_affinities {
        let key = Some(sid.clone());
        let entry = map
            .entry(key.clone())
            .or_insert_with(|| empty_session_identity_card(key));
        entry.route_affinity = Some(affinity.clone());
    }

    let mut cards = map.into_values().collect::<Vec<_>>();
    for card in &mut cards {
        let binding = card
            .session_id
            .as_deref()
            .and_then(|session_id| bindings.get(session_id));
        apply_basic_effective_route(card, global_station_override, binding);
        card.observation_scope = classify_session_observation_scope(card);
    }
    cards.sort_by_key(|card| std::cmp::Reverse(session_identity_sort_key(card)));
    cards
}

pub async fn enrich_session_identity_cards_with_host_transcripts(
    cards: &mut [SessionIdentityCard],
) {
    let session_ids = cards
        .iter()
        .filter_map(|card| card.session_id.as_deref())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if session_ids.is_empty() {
        return;
    }

    let found = match sessions::find_codex_session_files_by_ids(&session_ids).await {
        Ok(found) => found,
        Err(_) => return,
    };

    for card in cards {
        card.host_local_transcript_path = card
            .session_id
            .as_deref()
            .and_then(|sid| found.get(sid))
            .map(|path| path.to_string_lossy().to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::logging::RetryInfo;
    use crate::pricing::CostBreakdown;
    use crate::usage::UsageMetrics;

    fn sample_finished_request() -> FinishedRequest {
        FinishedRequest {
            id: 7,
            trace_id: Some("codex-7".to_string()),
            session_id: Some("sid".to_string()),
            client_name: None,
            client_addr: None,
            cwd: None,
            model: Some("gpt-5".to_string()),
            reasoning_effort: None,
            service_tier: Some("default".to_string()),
            station_name: Some("primary".to_string()),
            provider_id: Some("primary-provider".to_string()),
            upstream_base_url: Some("https://primary.example/v1".to_string()),
            route_decision: Some(RouteDecisionProvenance {
                effective_service_tier: Some(ResolvedRouteValue {
                    value: "priority".to_string(),
                    source: RouteValueSource::ProfileDefault,
                }),
                ..Default::default()
            }),
            usage: Some(UsageMetrics {
                output_tokens: 200,
                total_tokens: 200,
                ..UsageMetrics::default()
            }),
            cost: CostBreakdown::default(),
            retry: Some(RetryInfo {
                attempts: 2,
                upstream_chain: vec![
                    "backup:https://backup.example/v1 (idx=0) transport_error=timeout".to_string(),
                    "primary:https://primary.example/v1 (idx=0) status=200 class=-".to_string(),
                ],
                route_attempts: Vec::new(),
            }),
            observability: RequestObservability::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 1_500,
            ttfb_ms: Some(500),
            streaming: true,
            ended_at_ms: 2_000,
        }
    }

    #[test]
    fn finished_request_observability_derives_canonical_request_facts() {
        let request = sample_finished_request();

        let observability = request.observability_view();

        assert_eq!(observability.trace_id.as_deref(), Some("codex-7"));
        assert_eq!(observability.duration_ms, Some(1_500));
        assert_eq!(observability.ttfb_ms, Some(500));
        assert_eq!(observability.generation_ms, Some(1_000));
        assert_eq!(observability.attempt_count, 2);
        assert_eq!(observability.route_attempt_count, 2);
        assert!(observability.retried);
        assert!(observability.cross_station_failover);
        assert!(observability.fast_mode);
        assert!(observability.streaming);
        assert_eq!(observability.output_tokens_per_second, Some(200.0));
    }

    #[test]
    fn finished_request_serializes_materialized_observability_for_operator_api() {
        let mut request = sample_finished_request();
        request.refresh_observability();

        let value = serde_json::to_value(&request).expect("finished request json");

        assert_eq!(value["observability"]["trace_id"].as_str(), Some("codex-7"));
        assert_eq!(
            value["observability"]["generation_ms"].as_u64(),
            Some(1_000)
        );
        assert_eq!(
            value["observability"]["output_tokens_per_second"].as_f64(),
            Some(200.0)
        );
        assert_eq!(value["observability"]["attempt_count"].as_u64(), Some(2));
        assert_eq!(value["observability"]["streaming"].as_bool(), Some(true));
    }

    #[test]
    fn finished_request_legacy_payload_still_derives_observability() {
        let request: FinishedRequest = serde_json::from_value(serde_json::json!({
            "id": 8,
            "trace_id": "codex-8",
            "usage": {
                "output_tokens": 120,
                "total_tokens": 120
            },
            "cost": {},
            "service": "codex",
            "method": "POST",
            "path": "/v1/responses",
            "status_code": 200,
            "duration_ms": 1_500,
            "ttfb_ms": 300,
            "ended_at_ms": 2_500
        }))
        .expect("legacy finished request");

        assert_eq!(request.observability.attempt_count, 1);
        assert!(!request.streaming);

        let observability = request.observability_view();
        assert_eq!(observability.generation_ms, Some(1_200));
        assert_eq!(observability.output_tokens_per_second, Some(100.0));
        assert_eq!(observability.attempt_count, 1);
        assert!(!observability.fast_mode);
    }
}

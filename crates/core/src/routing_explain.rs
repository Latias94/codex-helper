use std::collections::BTreeMap;

use crate::lb::SelectedUpstream;
use crate::routing_ir::{
    RouteCandidate, RoutePlanAttemptState, RoutePlanExecutor, RoutePlanRuntimeState,
    RoutePlanSkipReason, RoutePlanTemplate,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RoutingExplainResponse {
    pub api_version: u32,
    pub service_name: String,
    pub runtime_loaded_at_ms: Option<u64>,
    pub request_model: Option<String>,
    pub session_id: Option<String>,
    pub selected_route: Option<RoutingExplainCandidate>,
    pub candidates: Vec<RoutingExplainCandidate>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RoutingExplainCandidate {
    pub provider_id: String,
    pub provider_alias: Option<String>,
    pub endpoint_id: String,
    pub route_path: Vec<String>,
    pub station_name: String,
    pub upstream_index: usize,
    pub upstream_base_url: String,
    pub selected: bool,
    pub skip_reasons: Vec<RoutingExplainSkipReason>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum RoutingExplainSkipReason {
    UnsupportedModel { requested_model: String },
    RuntimeDisabled,
    Cooldown,
    BreakerOpen { failure_count: u32 },
    UsageExhausted,
    MissingAuth,
}

pub fn build_routing_explain_response(
    service_name: impl Into<String>,
    runtime_loaded_at_ms: Option<u64>,
    request_model: Option<String>,
    session_id: Option<String>,
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
) -> RoutingExplainResponse {
    let executor = RoutePlanExecutor::new(template);
    let mut state = RoutePlanAttemptState::default();
    let selection = executor.select_supported_candidate_with_runtime_state(
        &mut state,
        runtime,
        request_model.as_deref(),
    );
    let selected_key = selection
        .selected
        .as_ref()
        .map(|selected| candidate_key(&selected.selected_upstream));
    let skip_reasons_by_candidate = executor
        .explain_candidate_skip_reasons_with_runtime_state(runtime, request_model.as_deref())
        .into_iter()
        .map(|explanation| {
            (
                candidate_key(&explanation.selected_upstream),
                explanation
                    .reasons
                    .iter()
                    .map(RoutingExplainSkipReason::from)
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let candidates = executor
        .iter_selected_upstreams()
        .map(|selected| {
            let key = candidate_key(&selected.selected_upstream);
            routing_explain_candidate(
                selected.candidate,
                &selected.selected_upstream,
                selected_key.as_ref() == Some(&key),
                skip_reasons_by_candidate
                    .get(&key)
                    .cloned()
                    .unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    let selected_route = candidates
        .iter()
        .find(|candidate| candidate.selected)
        .cloned();

    RoutingExplainResponse {
        api_version: 1,
        service_name: service_name.into(),
        runtime_loaded_at_ms,
        request_model,
        session_id,
        selected_route,
        candidates,
    }
}

impl From<&RoutePlanSkipReason> for RoutingExplainSkipReason {
    fn from(reason: &RoutePlanSkipReason) -> Self {
        match reason {
            RoutePlanSkipReason::UnsupportedModel { requested_model } => {
                RoutingExplainSkipReason::UnsupportedModel {
                    requested_model: requested_model.clone(),
                }
            }
            RoutePlanSkipReason::RuntimeDisabled => RoutingExplainSkipReason::RuntimeDisabled,
            RoutePlanSkipReason::Cooldown => RoutingExplainSkipReason::Cooldown,
            RoutePlanSkipReason::BreakerOpen { failure_count } => {
                RoutingExplainSkipReason::BreakerOpen {
                    failure_count: *failure_count,
                }
            }
            RoutePlanSkipReason::UsageExhausted => RoutingExplainSkipReason::UsageExhausted,
            RoutePlanSkipReason::MissingAuth => RoutingExplainSkipReason::MissingAuth,
        }
    }
}

impl RoutingExplainSkipReason {
    pub fn code(&self) -> &'static str {
        match self {
            RoutingExplainSkipReason::UnsupportedModel { .. } => "unsupported_model",
            RoutingExplainSkipReason::RuntimeDisabled => "runtime_disabled",
            RoutingExplainSkipReason::Cooldown => "cooldown",
            RoutingExplainSkipReason::BreakerOpen { .. } => "breaker_open",
            RoutingExplainSkipReason::UsageExhausted => "usage_exhausted",
            RoutingExplainSkipReason::MissingAuth => "missing_auth",
        }
    }
}

fn candidate_key(selected: &SelectedUpstream) -> (String, usize) {
    (selected.station_name.clone(), selected.index)
}

fn routing_explain_candidate(
    candidate: &RouteCandidate,
    selected_upstream: &SelectedUpstream,
    selected: bool,
    skip_reasons: Vec<RoutingExplainSkipReason>,
) -> RoutingExplainCandidate {
    RoutingExplainCandidate {
        provider_id: candidate.provider_id.clone(),
        provider_alias: candidate.provider_alias.clone(),
        endpoint_id: candidate.endpoint_id.clone(),
        route_path: candidate.route_path.clone(),
        station_name: selected_upstream.station_name.clone(),
        upstream_index: selected_upstream.index,
        upstream_base_url: selected_upstream.upstream.base_url.clone(),
        selected,
        skip_reasons,
    }
}

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context, Result};

use crate::config::{
    ProviderConfigV4, RoutingConditionV4, RoutingConfigV4, RoutingExhaustedActionV4, RoutingNodeV4,
    RoutingPolicyV4, ServiceConfig, ServiceViewV4, UpstreamAuth, UpstreamConfig,
    effective_v4_routing,
};
use crate::lb::{FAILURE_THRESHOLD, SelectedUpstream};
use crate::model_routing;
use crate::runtime_identity::{LegacyUpstreamKey, ProviderEndpointKey, RuntimeUpstreamIdentity};

const V4_COMPATIBILITY_STATION_NAME: &str = "routing";

#[derive(Debug, Clone)]
pub struct RoutePlanTemplate {
    pub service_name: String,
    pub entry: String,
    pub nodes: BTreeMap<String, RouteNodePlan>,
    pub expanded_provider_order: Vec<String>,
    pub candidates: Vec<RouteCandidate>,
    pub compatibility_station_name: Option<String>,
}

impl RoutePlanTemplate {
    pub fn candidate_identity(&self, candidate: &RouteCandidate) -> RuntimeUpstreamIdentity {
        RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new(
                self.service_name.clone(),
                candidate.provider_id.clone(),
                candidate.endpoint_id.clone(),
            ),
            LegacyUpstreamKey::new(
                self.service_name.clone(),
                candidate_compatibility_station_name(self, candidate),
                candidate_compatibility_upstream_index(candidate),
            ),
            candidate.base_url.clone(),
        )
    }

    pub fn candidate_identities(&self) -> Vec<RuntimeUpstreamIdentity> {
        self.candidates
            .iter()
            .map(|candidate| self.candidate_identity(candidate))
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct RoutePlan {
    pub service_name: String,
    pub entry: String,
    pub candidates: Vec<RouteCandidate>,
    pub decision_trace: RouteDecisionTrace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteNodePlan {
    pub name: String,
    pub strategy: RoutingPolicyV4,
    pub children: Vec<RouteRef>,
    pub target: Option<RouteRef>,
    pub prefer_tags: Vec<BTreeMap<String, String>>,
    pub on_exhausted: RoutingExhaustedActionV4,
    pub metadata: BTreeMap<String, String>,
    pub when: Option<RoutingConditionV4>,
    pub then: Option<RouteRef>,
    pub default_route: Option<RouteRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteRef {
    Route(String),
    Provider(String),
}

#[derive(Debug, Clone)]
pub struct RouteCandidate {
    pub provider_id: String,
    pub provider_alias: Option<String>,
    pub endpoint_id: String,
    pub base_url: String,
    pub auth: UpstreamAuth,
    pub tags: BTreeMap<String, String>,
    pub supported_models: BTreeMap<String, bool>,
    pub model_mapping: BTreeMap<String, String>,
    pub route_path: Vec<String>,
    pub stable_index: usize,
    pub compatibility_station_name: Option<String>,
    pub compatibility_upstream_index: Option<usize>,
}

impl RouteCandidate {
    pub fn to_upstream_config(&self) -> UpstreamConfig {
        UpstreamConfig {
            base_url: self.base_url.clone(),
            auth: self.auth.clone(),
            tags: btree_string_map_to_hash_map(&self.tags),
            supported_models: btree_bool_map_to_hash_map(&self.supported_models),
            model_mapping: btree_string_map_to_hash_map(&self.model_mapping),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SelectedRouteCandidate<'a> {
    pub candidate: &'a RouteCandidate,
    pub selected_upstream: SelectedUpstream,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutePlanAttemptState {
    avoid_by_station: BTreeMap<String, BTreeSet<usize>>,
    avoided_total: usize,
}

impl RoutePlanAttemptState {
    pub fn avoid_upstream_index(&mut self, upstream_index: usize) -> bool {
        self.avoid_upstream(V4_COMPATIBILITY_STATION_NAME, upstream_index)
    }

    pub fn avoid_upstream(&mut self, station_name: &str, upstream_index: usize) -> bool {
        if self
            .avoid_by_station
            .entry(station_name.to_string())
            .or_default()
            .insert(upstream_index)
        {
            self.avoided_total = self.avoided_total.saturating_add(1);
            return true;
        }
        false
    }

    pub fn avoid_selected(&mut self, selected: &SelectedRouteCandidate<'_>) -> bool {
        self.avoid_selected_upstream(&selected.selected_upstream)
    }

    pub fn avoid_selected_upstream(&mut self, selected: &SelectedUpstream) -> bool {
        self.avoid_upstream(selected.station_name.as_str(), selected.index)
    }

    pub fn avoids_upstream_index(&self, upstream_index: usize) -> bool {
        self.avoids_upstream(V4_COMPATIBILITY_STATION_NAME, upstream_index)
    }

    pub fn avoids_upstream(&self, station_name: &str, upstream_index: usize) -> bool {
        self.avoid_by_station
            .get(station_name)
            .is_some_and(|indices| indices.contains(&upstream_index))
    }

    pub fn avoid_for_station(&self) -> Vec<usize> {
        self.avoid_for_station_name(V4_COMPATIBILITY_STATION_NAME)
    }

    pub fn avoid_for_station_name(&self, station_name: &str) -> Vec<usize> {
        self.avoid_by_station
            .get(station_name)
            .map(|indices| indices.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn avoided_total(&self) -> usize {
        self.avoided_total
    }

    pub fn station_exhausted(&self, upstream_total: usize) -> bool {
        self.station_exhausted_for(V4_COMPATIBILITY_STATION_NAME, upstream_total)
    }

    pub fn station_exhausted_for(&self, station_name: &str, upstream_total: usize) -> bool {
        upstream_total > 0
            && self
                .avoid_by_station
                .get(station_name)
                .map(|indices| indices.iter().filter(|&&idx| idx < upstream_total).count())
                .unwrap_or_default()
                >= upstream_total
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutePlanRuntimeState {
    stations: BTreeMap<String, RoutePlanStationRuntimeState>,
}

impl RoutePlanRuntimeState {
    pub fn set_station(
        &mut self,
        station_name: impl Into<String>,
        state: RoutePlanStationRuntimeState,
    ) {
        self.stations.insert(station_name.into(), state);
    }

    pub fn station(&self, station_name: &str) -> Option<&RoutePlanStationRuntimeState> {
        self.stations.get(station_name)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutePlanStationRuntimeState {
    pub last_good_index: Option<usize>,
    pub upstreams: BTreeMap<usize, RoutePlanUpstreamRuntimeState>,
}

impl RoutePlanStationRuntimeState {
    pub fn set_upstream(&mut self, index: usize, state: RoutePlanUpstreamRuntimeState) {
        self.upstreams.insert(index, state);
    }

    fn upstream(&self, index: usize) -> RoutePlanUpstreamRuntimeState {
        self.upstreams.get(&index).copied().unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoutePlanUpstreamRuntimeState {
    pub runtime_disabled: bool,
    pub failure_count: u32,
    pub cooldown_active: bool,
    pub usage_exhausted: bool,
    pub missing_auth: bool,
}

impl RoutePlanUpstreamRuntimeState {
    fn breaker_open(self) -> bool {
        self.cooldown_active || self.failure_count >= FAILURE_THRESHOLD
    }

    fn hard_unavailable(self) -> bool {
        self.runtime_disabled || self.missing_auth || self.breaker_open()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutePlanSkipReason {
    UnsupportedModel { requested_model: String },
    RuntimeDisabled,
    Cooldown,
    BreakerOpen { failure_count: u32 },
    UsageExhausted,
    MissingAuth,
}

impl RoutePlanSkipReason {
    pub fn code(&self) -> &'static str {
        match self {
            RoutePlanSkipReason::UnsupportedModel { .. } => "unsupported_model",
            RoutePlanSkipReason::RuntimeDisabled => "runtime_disabled",
            RoutePlanSkipReason::Cooldown => "cooldown",
            RoutePlanSkipReason::BreakerOpen { .. } => "breaker_open",
            RoutePlanSkipReason::UsageExhausted => "usage_exhausted",
            RoutePlanSkipReason::MissingAuth => "missing_auth",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SkippedRouteCandidate<'a> {
    pub candidate: &'a RouteCandidate,
    pub selected_upstream: SelectedUpstream,
    pub reason: RoutePlanSkipReason,
    pub avoid_for_station: Vec<usize>,
    pub avoided_total: usize,
    pub total_upstreams: usize,
}

#[derive(Debug, Clone)]
pub struct RouteCandidateSkipExplanation<'a> {
    pub candidate: &'a RouteCandidate,
    pub selected_upstream: SelectedUpstream,
    pub reasons: Vec<RoutePlanSkipReason>,
}

#[derive(Debug, Clone)]
pub struct RoutePlanAttemptSelection<'a> {
    pub selected: Option<SelectedRouteCandidate<'a>>,
    pub skipped: Vec<SkippedRouteCandidate<'a>>,
    pub avoid_for_station: Vec<usize>,
    pub avoided_total: usize,
    pub total_upstreams: usize,
}

pub struct RoutePlanExecutor<'a> {
    template: &'a RoutePlanTemplate,
}

impl<'a> RoutePlanExecutor<'a> {
    pub fn new(template: &'a RoutePlanTemplate) -> Self {
        Self { template }
    }

    pub fn iter_candidates(&self) -> impl Iterator<Item = &RouteCandidate> + '_ {
        self.template.candidates.iter()
    }

    pub fn iter_selected_upstreams(&self) -> impl Iterator<Item = SelectedRouteCandidate<'_>> + '_ {
        self.template
            .candidates
            .iter()
            .map(|candidate| SelectedRouteCandidate {
                candidate,
                selected_upstream: self.selected_upstream_for_candidate(candidate),
            })
    }

    pub fn explain_candidate_skip_reasons_with_runtime_state(
        &self,
        runtime: &RoutePlanRuntimeState,
        request_model: Option<&str>,
    ) -> Vec<RouteCandidateSkipExplanation<'_>> {
        self.template
            .candidates
            .iter()
            .filter_map(|candidate| {
                let selected_upstream = self.selected_upstream_for_candidate(candidate);
                let runtime_state = runtime
                    .station(selected_upstream.station_name.as_str())
                    .map(|station| station.upstream(selected_upstream.index))
                    .unwrap_or_default();
                let reasons = self.candidate_skip_reasons(candidate, runtime_state, request_model);
                (!reasons.is_empty()).then_some(RouteCandidateSkipExplanation {
                    candidate,
                    selected_upstream,
                    reasons,
                })
            })
            .collect()
    }

    pub fn select_supported_candidate(
        &self,
        state: &mut RoutePlanAttemptState,
        request_model: Option<&str>,
    ) -> RoutePlanAttemptSelection<'_> {
        self.select_supported_candidate_with_runtime_state(
            state,
            &RoutePlanRuntimeState::default(),
            request_model,
        )
    }

    pub fn select_supported_candidate_with_runtime_state(
        &self,
        state: &mut RoutePlanAttemptState,
        runtime: &RoutePlanRuntimeState,
        request_model: Option<&str>,
    ) -> RoutePlanAttemptSelection<'_> {
        let total_upstreams = self.template.candidates.len();
        let mut skipped = Vec::new();

        loop {
            if self.candidates_exhausted(state) {
                return RoutePlanAttemptSelection {
                    selected: None,
                    skipped,
                    avoid_for_station: state.avoid_for_station(),
                    avoided_total: state.avoided_total(),
                    total_upstreams,
                };
            }

            let Some(candidate) = self.next_unavoided_candidate(state, runtime) else {
                return RoutePlanAttemptSelection {
                    selected: None,
                    skipped,
                    avoid_for_station: state.avoid_for_station(),
                    avoided_total: state.avoided_total(),
                    total_upstreams,
                };
            };
            let selected_upstream = self.selected_upstream_for_candidate(candidate);

            if let Some(requested_model) = request_model
                && !candidate_supports_model(candidate, requested_model)
            {
                state.avoid_selected_upstream(&selected_upstream);
                let avoid_for_station =
                    state.avoid_for_station_name(selected_upstream.station_name.as_str());
                skipped.push(SkippedRouteCandidate {
                    candidate,
                    selected_upstream,
                    reason: RoutePlanSkipReason::UnsupportedModel {
                        requested_model: requested_model.to_string(),
                    },
                    avoid_for_station,
                    avoided_total: state.avoided_total(),
                    total_upstreams,
                });
                continue;
            }

            let avoid_for_station =
                state.avoid_for_station_name(selected_upstream.station_name.as_str());
            return RoutePlanAttemptSelection {
                selected: Some(SelectedRouteCandidate {
                    candidate,
                    selected_upstream,
                }),
                skipped,
                avoid_for_station,
                avoided_total: state.avoided_total(),
                total_upstreams,
            };
        }
    }

    pub fn select_supported_station_candidate_with_runtime_state(
        &self,
        state: &mut RoutePlanAttemptState,
        runtime: &RoutePlanRuntimeState,
        station_name: &str,
        request_model: Option<&str>,
    ) -> RoutePlanAttemptSelection<'_> {
        let total_upstreams = self.template.candidates.len();
        let mut skipped = Vec::new();

        loop {
            if state.station_exhausted_for(station_name, self.station_candidate_count(station_name))
            {
                return RoutePlanAttemptSelection {
                    selected: None,
                    skipped,
                    avoid_for_station: state.avoid_for_station_name(station_name),
                    avoided_total: state.avoided_total(),
                    total_upstreams,
                };
            }

            let Some(candidate) =
                self.next_unavoided_station_candidate(state, runtime, station_name)
            else {
                return RoutePlanAttemptSelection {
                    selected: None,
                    skipped,
                    avoid_for_station: state.avoid_for_station_name(station_name),
                    avoided_total: state.avoided_total(),
                    total_upstreams,
                };
            };
            let selected_upstream = self.selected_upstream_for_candidate(candidate);

            if let Some(requested_model) = request_model
                && !candidate_supports_model(candidate, requested_model)
            {
                state.avoid_selected_upstream(&selected_upstream);
                let avoid_for_station =
                    state.avoid_for_station_name(selected_upstream.station_name.as_str());
                skipped.push(SkippedRouteCandidate {
                    candidate,
                    selected_upstream,
                    reason: RoutePlanSkipReason::UnsupportedModel {
                        requested_model: requested_model.to_string(),
                    },
                    avoid_for_station,
                    avoided_total: state.avoided_total(),
                    total_upstreams,
                });
                continue;
            }

            let avoid_for_station =
                state.avoid_for_station_name(selected_upstream.station_name.as_str());
            return RoutePlanAttemptSelection {
                selected: Some(SelectedRouteCandidate {
                    candidate,
                    selected_upstream,
                }),
                skipped,
                avoid_for_station,
                avoided_total: state.avoided_total(),
                total_upstreams,
            };
        }
    }

    pub fn selected_upstream_for_candidate(&self, candidate: &RouteCandidate) -> SelectedUpstream {
        SelectedUpstream {
            station_name: candidate_compatibility_station_name(self.template, candidate),
            index: candidate_compatibility_upstream_index(candidate),
            upstream: candidate.to_upstream_config(),
        }
    }

    fn next_unavoided_candidate(
        &self,
        state: &RoutePlanAttemptState,
        runtime: &RoutePlanRuntimeState,
    ) -> Option<&'a RouteCandidate> {
        let mut seen_stations = BTreeSet::new();
        for station_name in self.template.candidates.iter().filter_map(|candidate| {
            let station_name = candidate_compatibility_station_name(self.template, candidate);
            seen_stations
                .insert(station_name.clone())
                .then_some(station_name)
        }) {
            if state.station_exhausted_for(
                station_name.as_str(),
                self.station_candidate_count(station_name.as_str()),
            ) {
                continue;
            }
            if let Some(candidate) =
                self.next_unavoided_station_candidate(state, runtime, station_name.as_str())
            {
                return Some(candidate);
            }
        }

        None
    }

    fn candidates_exhausted(&self, state: &RoutePlanAttemptState) -> bool {
        !self.template.candidates.is_empty()
            && self.template.candidates.iter().all(|candidate| {
                let station_name = candidate_compatibility_station_name(self.template, candidate);
                state.avoids_upstream(
                    station_name.as_str(),
                    candidate_compatibility_upstream_index(candidate),
                )
            })
    }

    fn candidate_skip_reasons(
        &self,
        candidate: &RouteCandidate,
        runtime_state: RoutePlanUpstreamRuntimeState,
        request_model: Option<&str>,
    ) -> Vec<RoutePlanSkipReason> {
        let mut reasons = Vec::new();
        if let Some(requested_model) = request_model
            && !candidate_supports_model(candidate, requested_model)
        {
            reasons.push(RoutePlanSkipReason::UnsupportedModel {
                requested_model: requested_model.to_string(),
            });
        }
        if runtime_state.runtime_disabled {
            reasons.push(RoutePlanSkipReason::RuntimeDisabled);
        }
        if runtime_state.cooldown_active {
            reasons.push(RoutePlanSkipReason::Cooldown);
        } else if runtime_state.failure_count >= FAILURE_THRESHOLD {
            reasons.push(RoutePlanSkipReason::BreakerOpen {
                failure_count: runtime_state.failure_count,
            });
        }
        if runtime_state.usage_exhausted {
            reasons.push(RoutePlanSkipReason::UsageExhausted);
        }
        if runtime_state.missing_auth {
            reasons.push(RoutePlanSkipReason::MissingAuth);
        }
        reasons
    }

    fn station_candidate_count(&self, station_name: &str) -> usize {
        self.template
            .candidates
            .iter()
            .filter(|candidate| {
                candidate_compatibility_station_name(self.template, candidate) == station_name
            })
            .count()
    }

    fn next_unavoided_station_candidate(
        &self,
        state: &RoutePlanAttemptState,
        runtime: &RoutePlanRuntimeState,
        station_name: &str,
    ) -> Option<&'a RouteCandidate> {
        let station_runtime = runtime.station(station_name);
        let station_candidates = self
            .template
            .candidates
            .iter()
            .filter(|candidate| {
                candidate_compatibility_station_name(self.template, candidate) == station_name
            })
            .filter(|candidate| {
                !state.avoids_upstream(
                    station_name,
                    candidate_compatibility_upstream_index(candidate),
                )
            })
            .collect::<Vec<_>>();

        if let Some(last_good_index) = station_runtime.and_then(|station| station.last_good_index)
            && let Some(candidate) = station_candidates.iter().copied().find(|candidate| {
                candidate_compatibility_upstream_index(candidate) == last_good_index
                    && station_runtime
                        .map(|station| station.upstream(last_good_index))
                        .is_none_or(|upstream| {
                            !upstream.hard_unavailable() && !upstream.usage_exhausted
                        })
            })
        {
            return Some(candidate);
        }

        if let Some(candidate) = station_candidates.iter().copied().find(|candidate| {
            let index = candidate_compatibility_upstream_index(candidate);
            station_runtime
                .map(|station| station.upstream(index))
                .is_none_or(|upstream| !upstream.hard_unavailable() && !upstream.usage_exhausted)
        }) {
            return Some(candidate);
        }

        station_candidates.iter().copied().find(|candidate| {
            let index = candidate_compatibility_upstream_index(candidate);
            station_runtime
                .map(|station| station.upstream(index))
                .is_none_or(|upstream| !upstream.hard_unavailable())
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteDecisionTrace {
    pub events: Vec<RouteDecisionEvent>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteRequestContext {
    pub model: Option<String>,
    pub service_tier: Option<String>,
    pub reasoning_effort: Option<String>,
    pub path: Option<String>,
    pub method: Option<String>,
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecisionEvent {
    pub route_path: Vec<String>,
    pub provider_id: Option<String>,
    pub endpoint_id: Option<String>,
    pub decision: RouteDecision,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteDecision {
    Candidate,
    Selected,
    Skipped,
}

#[derive(Debug, Clone)]
struct RouteLeaf {
    provider_id: String,
    route_path: Vec<String>,
}

#[derive(Debug, Clone)]
struct EndpointParts {
    endpoint_id: String,
    base_url: String,
    enabled: bool,
    priority: u32,
    tags: BTreeMap<String, String>,
    supported_models: BTreeMap<String, bool>,
    model_mapping: BTreeMap<String, String>,
}

pub fn compile_v4_route_plan_template(
    service_name: &str,
    view: &ServiceViewV4,
) -> Result<RoutePlanTemplate> {
    compile_v4_route_plan_template_with_request(service_name, view, &RouteRequestContext::default())
}

pub fn compile_v4_route_plan_template_with_request(
    service_name: &str,
    view: &ServiceViewV4,
    request: &RouteRequestContext,
) -> Result<RoutePlanTemplate> {
    let routing = effective_v4_routing(view);
    validate_route_provider_name_conflicts(service_name, view, &routing)?;

    let nodes = normalize_route_nodes(service_name, view, &routing)?;
    let leaves = expand_v4_route_leaves(service_name, view, &routing, request)?;
    ensure_unique_provider_leaves(service_name, &leaves)?;

    let expanded_provider_order = leaves
        .iter()
        .map(|leaf| leaf.provider_id.clone())
        .collect::<Vec<_>>();
    let candidates = route_candidates_from_leaves(service_name, view, &leaves)?;

    Ok(RoutePlanTemplate {
        service_name: service_name.to_string(),
        entry: routing.entry,
        nodes,
        expanded_provider_order,
        compatibility_station_name: (!leaves.is_empty())
            .then(|| V4_COMPATIBILITY_STATION_NAME.to_string()),
        candidates,
    })
}

pub fn compile_legacy_route_plan_template<'a>(
    service_name: &str,
    services: impl IntoIterator<Item = &'a ServiceConfig>,
) -> RoutePlanTemplate {
    let entry = "legacy".to_string();
    let mut candidates = Vec::new();
    let mut station_names = BTreeSet::new();

    for service in services {
        station_names.insert(service.name.clone());
        for (upstream_index, upstream) in service.upstreams.iter().enumerate() {
            let provider_id = upstream
                .tags
                .get("provider_id")
                .cloned()
                .unwrap_or_else(|| format!("{}#{upstream_index}", service.name));
            let endpoint_id = upstream
                .tags
                .get("endpoint_id")
                .cloned()
                .unwrap_or_else(|| upstream_index.to_string());
            let stable_index = candidates.len();
            let route_path = vec![entry.clone(), service.name.clone(), provider_id.clone()];
            candidates.push(RouteCandidate {
                provider_id,
                provider_alias: service.alias.clone(),
                endpoint_id,
                base_url: upstream.base_url.clone(),
                auth: upstream.auth.clone(),
                tags: hash_string_map_to_btree(&upstream.tags),
                supported_models: hash_bool_map_to_btree(&upstream.supported_models),
                model_mapping: hash_string_map_to_btree(&upstream.model_mapping),
                route_path,
                stable_index,
                compatibility_station_name: Some(service.name.clone()),
                compatibility_upstream_index: Some(upstream_index),
            });
        }
    }

    RoutePlanTemplate {
        service_name: service_name.to_string(),
        entry,
        nodes: BTreeMap::new(),
        expanded_provider_order: candidates
            .iter()
            .map(|candidate| candidate.provider_id.clone())
            .collect(),
        candidates,
        compatibility_station_name: if station_names.len() == 1 {
            station_names.into_iter().next()
        } else {
            None
        },
    }
}

fn validate_route_provider_name_conflicts(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
) -> Result<()> {
    for route_name in routing.routes.keys() {
        if view.providers.contains_key(route_name.as_str()) {
            anyhow::bail!(
                "[{service_name}] route node '{route_name}' conflicts with a provider of the same name"
            );
        }
    }
    Ok(())
}

fn normalize_route_nodes(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
) -> Result<BTreeMap<String, RouteNodePlan>> {
    let mut out = BTreeMap::new();
    for (route_name, node) in &routing.routes {
        validate_route_node_shape(service_name, view, routing, route_name, node)?;
        let children = node
            .children
            .iter()
            .map(|child| normalize_route_ref(service_name, view, routing, child))
            .collect::<Result<Vec<_>>>()?;
        let target = node
            .target
            .as_deref()
            .map(|target| normalize_route_ref(service_name, view, routing, target))
            .transpose()?;
        let then = node
            .then
            .as_deref()
            .map(|target| normalize_route_ref(service_name, view, routing, target))
            .transpose()?;
        let default_route = node
            .default_route
            .as_deref()
            .map(|target| normalize_route_ref(service_name, view, routing, target))
            .transpose()?;
        out.insert(
            route_name.clone(),
            RouteNodePlan {
                name: route_name.clone(),
                strategy: node.strategy,
                children,
                target,
                prefer_tags: node.prefer_tags.clone(),
                on_exhausted: node.on_exhausted,
                metadata: node.metadata.clone(),
                when: node.when.clone(),
                then,
                default_route,
            },
        );
    }
    Ok(out)
}

fn validate_route_node_shape(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    route_name: &str,
    node: &RoutingNodeV4,
) -> Result<()> {
    match node.strategy {
        RoutingPolicyV4::Conditional => {
            let Some(condition) = node.when.as_ref() else {
                anyhow::bail!("[{service_name}] conditional route '{route_name}' requires when");
            };
            if condition.is_empty() {
                anyhow::bail!(
                    "[{service_name}] conditional route '{route_name}' requires at least one condition field"
                );
            }
            let then = node
                .then
                .as_deref()
                .filter(|value| !value.trim().is_empty());
            let default_route = node
                .default_route
                .as_deref()
                .filter(|value| !value.trim().is_empty());
            let Some(then) = then else {
                anyhow::bail!("[{service_name}] conditional route '{route_name}' requires then");
            };
            let Some(default_route) = default_route else {
                anyhow::bail!("[{service_name}] conditional route '{route_name}' requires default");
            };
            normalize_route_ref(service_name, view, routing, then)?;
            normalize_route_ref(service_name, view, routing, default_route)?;
        }
        _ => {
            if node.when.is_some() || node.then.is_some() || node.default_route.is_some() {
                anyhow::bail!(
                    "[{service_name}] route '{route_name}' uses conditional fields but strategy is not conditional"
                );
            }
        }
    }

    Ok(())
}

fn normalize_route_ref(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    name: &str,
) -> Result<RouteRef> {
    if view.providers.contains_key(name) {
        return Ok(RouteRef::Provider(name.to_string()));
    }
    if routing.routes.contains_key(name) {
        return Ok(RouteRef::Route(name.to_string()));
    }
    anyhow::bail!("[{service_name}] routing references missing route or provider '{name}'");
}

fn expand_v4_route_leaves(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    request: &RouteRequestContext,
) -> Result<Vec<RouteLeaf>> {
    if view.providers.is_empty() && routing.routes.is_empty() {
        return Ok(Vec::new());
    }
    if routing.routes.is_empty() {
        return Ok(view
            .providers
            .keys()
            .map(|provider_id| RouteLeaf {
                provider_id: provider_id.clone(),
                route_path: vec![provider_id.clone()],
            })
            .collect());
    }

    let mut stack = Vec::new();
    expand_route_node(
        service_name,
        view,
        routing,
        routing.entry.as_str(),
        &[],
        request,
        &mut stack,
    )
}

fn expand_route_ref(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    child_name: &str,
    parent_path: &[String],
    request: &RouteRequestContext,
    stack: &mut Vec<String>,
) -> Result<Vec<RouteLeaf>> {
    if view.providers.contains_key(child_name) {
        let mut route_path = parent_path.to_vec();
        route_path.push(child_name.to_string());
        return Ok(vec![RouteLeaf {
            provider_id: child_name.to_string(),
            route_path,
        }]);
    }

    expand_route_node(
        service_name,
        view,
        routing,
        child_name,
        parent_path,
        request,
        stack,
    )
}

fn expand_route_node(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    route_name: &str,
    parent_path: &[String],
    request: &RouteRequestContext,
    stack: &mut Vec<String>,
) -> Result<Vec<RouteLeaf>> {
    if stack.iter().any(|name| name == route_name) {
        let mut cycle = stack.clone();
        cycle.push(route_name.to_string());
        anyhow::bail!(
            "[{service_name}] routing graph has a cycle: {}",
            cycle.join(" -> ")
        );
    }

    let Some(node) = routing.routes.get(route_name) else {
        anyhow::bail!(
            "[{service_name}] routing entry references missing route node '{route_name}'"
        );
    };

    stack.push(route_name.to_string());
    let mut node_path = parent_path.to_vec();
    node_path.push(route_name.to_string());
    let result = match node.strategy {
        RoutingPolicyV4::OrderedFailover => expand_ordered_route_children(
            service_name,
            view,
            routing,
            route_name,
            node,
            &node_path,
            request,
            stack,
        ),
        RoutingPolicyV4::ManualSticky => expand_manual_sticky_route(
            service_name,
            view,
            routing,
            route_name,
            node,
            &node_path,
            request,
            stack,
        ),
        RoutingPolicyV4::TagPreferred => expand_tag_preferred_route(
            service_name,
            view,
            routing,
            route_name,
            node,
            &node_path,
            request,
            stack,
        ),
        RoutingPolicyV4::Conditional => expand_conditional_route(
            service_name,
            view,
            routing,
            route_name,
            node,
            &node_path,
            request,
            stack,
        ),
    };
    stack.pop();
    result
}

fn expand_ordered_route_children(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    route_name: &str,
    node: &RoutingNodeV4,
    node_path: &[String],
    request: &RouteRequestContext,
    stack: &mut Vec<String>,
) -> Result<Vec<RouteLeaf>> {
    if node.children.is_empty() {
        anyhow::bail!(
            "[{service_name}] ordered-failover route '{route_name}' requires at least one child"
        );
    }

    let mut leaves = Vec::new();
    for child_name in &node.children {
        leaves.extend(expand_route_ref(
            service_name,
            view,
            routing,
            child_name.as_str(),
            node_path,
            request,
            stack,
        )?);
    }
    Ok(leaves)
}

fn expand_manual_sticky_route(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    route_name: &str,
    node: &RoutingNodeV4,
    node_path: &[String],
    request: &RouteRequestContext,
    stack: &mut Vec<String>,
) -> Result<Vec<RouteLeaf>> {
    let target = node
        .target
        .as_deref()
        .or_else(|| node.children.first().map(String::as_str))
        .with_context(|| {
            format!("[{service_name}] manual-sticky route '{route_name}' requires target")
        })?;
    if let Some(provider) = view.providers.get(target)
        && !provider.enabled
    {
        anyhow::bail!(
            "[{service_name}] manual-sticky route '{route_name}' targets disabled provider '{target}'"
        );
    }

    expand_route_ref(
        service_name,
        view,
        routing,
        target,
        node_path,
        request,
        stack,
    )
}

fn expand_tag_preferred_route(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    route_name: &str,
    node: &RoutingNodeV4,
    node_path: &[String],
    request: &RouteRequestContext,
    stack: &mut Vec<String>,
) -> Result<Vec<RouteLeaf>> {
    if node.children.is_empty() {
        anyhow::bail!(
            "[{service_name}] tag-preferred route '{route_name}' requires at least one child"
        );
    }
    if node.prefer_tags.is_empty() {
        anyhow::bail!("[{service_name}] tag-preferred route '{route_name}' requires prefer_tags");
    }

    let mut preferred = Vec::new();
    let mut fallback = Vec::new();
    for child_name in &node.children {
        let child_leaves = expand_route_ref(
            service_name,
            view,
            routing,
            child_name.as_str(),
            node_path,
            request,
            stack,
        )?;
        if child_route_matches_any_filter(view, &child_leaves, &node.prefer_tags) {
            preferred.extend(child_leaves);
        } else {
            fallback.extend(child_leaves);
        }
    }

    if matches!(node.on_exhausted, RoutingExhaustedActionV4::Stop) {
        if preferred.is_empty() {
            anyhow::bail!(
                "[{service_name}] tag-preferred route '{route_name}' with on_exhausted = 'stop' matched no providers"
            );
        }
        return Ok(preferred);
    }

    preferred.extend(fallback);
    Ok(preferred)
}

fn expand_conditional_route(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    route_name: &str,
    node: &RoutingNodeV4,
    node_path: &[String],
    request: &RouteRequestContext,
    stack: &mut Vec<String>,
) -> Result<Vec<RouteLeaf>> {
    let condition = node.when.as_ref().with_context(|| {
        format!("[{service_name}] conditional route '{route_name}' requires when")
    })?;
    if condition.is_empty() {
        anyhow::bail!(
            "[{service_name}] conditional route '{route_name}' requires at least one condition field"
        );
    }

    let then = node.then.as_deref().with_context(|| {
        format!("[{service_name}] conditional route '{route_name}' requires then")
    })?;
    let default_route = node.default_route.as_deref().with_context(|| {
        format!("[{service_name}] conditional route '{route_name}' requires default")
    })?;
    let selected = if request_matches_condition(request, condition) {
        then
    } else {
        default_route
    };

    expand_route_ref(
        service_name,
        view,
        routing,
        selected,
        node_path,
        request,
        stack,
    )
}

fn request_matches_condition(
    request: &RouteRequestContext,
    condition: &RoutingConditionV4,
) -> bool {
    optional_field_matches(request.model.as_deref(), condition.model.as_deref(), false)
        && optional_field_matches(
            request.service_tier.as_deref(),
            condition.service_tier.as_deref(),
            true,
        )
        && optional_field_matches(
            request.reasoning_effort.as_deref(),
            condition.reasoning_effort.as_deref(),
            true,
        )
        && optional_field_matches(request.path.as_deref(), condition.path.as_deref(), false)
        && optional_field_matches(request.method.as_deref(), condition.method.as_deref(), true)
        && condition.headers.iter().all(|(key, expected)| {
            request
                .headers
                .iter()
                .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
                .is_some_and(|(_, actual)| actual == expected)
        })
}

fn optional_field_matches(
    actual: Option<&str>,
    expected: Option<&str>,
    ignore_ascii_case: bool,
) -> bool {
    let Some(expected) = expected.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    let Some(actual) = actual.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    if ignore_ascii_case {
        actual.eq_ignore_ascii_case(expected)
    } else {
        actual == expected
    }
}

fn child_route_matches_any_filter(
    view: &ServiceViewV4,
    leaves: &[RouteLeaf],
    filters: &[BTreeMap<String, String>],
) -> bool {
    leaves.iter().any(|leaf| {
        view.providers
            .get(leaf.provider_id.as_str())
            .is_some_and(|provider| provider_matches_any_filter(&provider.tags, filters))
    })
}

fn provider_matches_any_filter(
    tags: &BTreeMap<String, String>,
    filters: &[BTreeMap<String, String>],
) -> bool {
    filters.iter().any(|filter| {
        !filter.is_empty()
            && filter
                .iter()
                .all(|(key, value)| tags.get(key) == Some(value))
    })
}

fn ensure_unique_provider_leaves(service_name: &str, leaves: &[RouteLeaf]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for leaf in leaves {
        if !seen.insert(leaf.provider_id.as_str()) {
            anyhow::bail!(
                "[{service_name}] routing graph expands provider '{}' more than once; duplicate leaves are ambiguous",
                leaf.provider_id
            );
        }
    }
    Ok(())
}

fn route_candidates_from_leaves(
    service_name: &str,
    view: &ServiceViewV4,
    leaves: &[RouteLeaf],
) -> Result<Vec<RouteCandidate>> {
    let mut candidates = Vec::new();
    for leaf in leaves {
        let Some(provider) = view.providers.get(leaf.provider_id.as_str()) else {
            anyhow::bail!(
                "[{service_name}] routing references missing provider '{}'",
                leaf.provider_id
            );
        };
        if !provider.enabled {
            continue;
        }

        let auth = merge_auth(&provider.auth, &provider.inline_auth);
        for endpoint in
            ordered_provider_endpoints(service_name, leaf.provider_id.as_str(), provider)?
        {
            if !endpoint.enabled {
                continue;
            }
            let stable_index = candidates.len();
            candidates.push(RouteCandidate {
                provider_id: leaf.provider_id.clone(),
                provider_alias: provider.alias.clone(),
                endpoint_id: endpoint.endpoint_id,
                base_url: endpoint.base_url,
                auth: auth.clone(),
                tags: merge_string_maps_with_provider_id(
                    leaf.provider_id.as_str(),
                    &provider.tags,
                    &endpoint.tags,
                ),
                supported_models: merge_bool_maps(
                    &provider.supported_models,
                    &endpoint.supported_models,
                ),
                model_mapping: merge_string_maps(&provider.model_mapping, &endpoint.model_mapping),
                route_path: leaf.route_path.clone(),
                stable_index,
                compatibility_station_name: Some(V4_COMPATIBILITY_STATION_NAME.to_string()),
                compatibility_upstream_index: Some(stable_index),
            });
        }
    }
    Ok(candidates)
}

fn ordered_provider_endpoints(
    service_name: &str,
    provider_name: &str,
    provider: &ProviderConfigV4,
) -> Result<Vec<EndpointParts>> {
    let mut endpoints = Vec::new();
    if let Some(base_url) = provider
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if provider.endpoints.contains_key("default") {
            anyhow::bail!(
                "[{service_name}] provider '{provider_name}' cannot define both base_url and endpoints.default"
            );
        }
        endpoints.push(EndpointParts {
            endpoint_id: "default".to_string(),
            base_url: base_url.to_string(),
            enabled: true,
            priority: 0,
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
        });
    }

    for (endpoint_id, endpoint) in &provider.endpoints {
        if endpoint.base_url.trim().is_empty() {
            anyhow::bail!(
                "[{service_name}] provider '{provider_name}' endpoint '{endpoint_id}' has an empty base_url"
            );
        }
        endpoints.push(EndpointParts {
            endpoint_id: endpoint_id.clone(),
            base_url: endpoint.base_url.trim().to_string(),
            enabled: endpoint.enabled,
            priority: endpoint.priority,
            tags: endpoint.tags.clone(),
            supported_models: endpoint.supported_models.clone(),
            model_mapping: endpoint.model_mapping.clone(),
        });
    }

    if endpoints.is_empty() {
        anyhow::bail!("[{service_name}] provider '{provider_name}' has no base_url or endpoints");
    }

    endpoints.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.endpoint_id.cmp(&right.endpoint_id))
            .then_with(|| left.base_url.cmp(&right.base_url))
    });
    Ok(endpoints)
}

fn merge_auth(block: &UpstreamAuth, inline: &UpstreamAuth) -> UpstreamAuth {
    UpstreamAuth {
        auth_token: inline
            .auth_token
            .clone()
            .or_else(|| block.auth_token.clone()),
        auth_token_env: inline
            .auth_token_env
            .clone()
            .or_else(|| block.auth_token_env.clone()),
        api_key: inline.api_key.clone().or_else(|| block.api_key.clone()),
        api_key_env: inline
            .api_key_env
            .clone()
            .or_else(|| block.api_key_env.clone()),
    }
}

fn merge_string_maps(
    provider_values: &BTreeMap<String, String>,
    endpoint_values: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut merged = provider_values.clone();
    for (key, value) in endpoint_values {
        merged.insert(key.clone(), value.clone());
    }
    merged
}

fn merge_string_maps_with_provider_id(
    provider_id: &str,
    provider_values: &BTreeMap<String, String>,
    endpoint_values: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut provider_values = provider_values.clone();
    provider_values.insert("provider_id".to_string(), provider_id.to_string());
    merge_string_maps(&provider_values, endpoint_values)
}

fn merge_bool_maps(
    provider_values: &BTreeMap<String, bool>,
    endpoint_values: &BTreeMap<String, bool>,
) -> BTreeMap<String, bool> {
    let mut merged = provider_values.clone();
    for (key, value) in endpoint_values {
        merged.insert(key.clone(), *value);
    }
    merged
}

fn candidate_compatibility_upstream_index(candidate: &RouteCandidate) -> usize {
    candidate
        .compatibility_upstream_index
        .unwrap_or(candidate.stable_index)
}

fn candidate_compatibility_station_name(
    template: &RoutePlanTemplate,
    candidate: &RouteCandidate,
) -> String {
    candidate
        .compatibility_station_name
        .clone()
        .or_else(|| template.compatibility_station_name.clone())
        .unwrap_or_else(|| template.service_name.clone())
}

fn candidate_supports_model(candidate: &RouteCandidate, requested_model: &str) -> bool {
    model_routing::is_model_supported(
        &btree_bool_map_to_hash_map(&candidate.supported_models),
        &btree_string_map_to_hash_map(&candidate.model_mapping),
        requested_model,
    )
}

fn hash_string_map_to_btree(values: &HashMap<String, String>) -> BTreeMap<String, String> {
    values
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn hash_bool_map_to_btree(values: &HashMap<String, bool>) -> BTreeMap<String, bool> {
    values
        .iter()
        .map(|(key, value)| (key.clone(), *value))
        .collect()
}

fn btree_string_map_to_hash_map(values: &BTreeMap<String, String>) -> HashMap<String, String> {
    values
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn btree_bool_map_to_hash_map(values: &BTreeMap<String, bool>) -> HashMap<String, bool> {
    values
        .iter()
        .map(|(key, value)| (key.clone(), *value))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ProviderEndpointV4, ProxyConfigV4, RoutingConditionV4, RoutingConfigV4,
        RoutingExhaustedActionV4, RoutingNodeV4, RoutingPolicyV4, compile_v4_to_runtime,
        resolved_v4_provider_order,
    };
    use crate::lb::{LbState, LoadBalancer, SelectedUpstream};
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};

    fn provider(base_url: &str) -> ProviderConfigV4 {
        ProviderConfigV4 {
            base_url: Some(base_url.to_string()),
            ..ProviderConfigV4::default()
        }
    }

    fn tagged_provider(base_url: &str, key: &str, value: &str) -> ProviderConfigV4 {
        ProviderConfigV4 {
            base_url: Some(base_url.to_string()),
            tags: BTreeMap::from([(key.to_string(), value.to_string())]),
            ..ProviderConfigV4::default()
        }
    }

    fn provider_ids(template: &RoutePlanTemplate) -> Vec<String> {
        template
            .candidates
            .iter()
            .map(|candidate| candidate.provider_id.clone())
            .collect()
    }

    fn provider_endpoint_keys(template: &RoutePlanTemplate) -> Vec<String> {
        template
            .candidate_identities()
            .into_iter()
            .map(|identity| identity.provider_endpoint.stable_key())
            .collect()
    }

    fn legacy_upstream_keys(template: &RoutePlanTemplate) -> Vec<String> {
        template
            .candidate_identities()
            .into_iter()
            .map(|identity| identity.legacy.stable_key())
            .collect()
    }

    fn assert_provider_order_parity(view: &ServiceViewV4, template: &RoutePlanTemplate) {
        let resolved = resolved_v4_provider_order("routing-ir-test", view).expect("resolved order");
        assert_eq!(template.expanded_provider_order, resolved);
        assert_eq!(provider_ids(template), resolved);
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct UpstreamSignature {
        station_name: String,
        index: usize,
        base_url: String,
        tags: BTreeMap<String, String>,
        supported_models: BTreeMap<String, bool>,
        model_mapping: BTreeMap<String, String>,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct AttemptOrderEvent {
        decision: &'static str,
        upstream: UpstreamSignature,
        avoid_for_station: Vec<usize>,
        avoided_total: usize,
        total_upstreams: usize,
        reason: Option<&'static str>,
    }

    fn hash_string_map_to_btree(values: &HashMap<String, String>) -> BTreeMap<String, String> {
        values
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    fn hash_bool_map_to_btree(values: &HashMap<String, bool>) -> BTreeMap<String, bool> {
        values
            .iter()
            .map(|(key, value)| (key.clone(), *value))
            .collect()
    }

    fn upstream_signature(selected: &SelectedUpstream) -> UpstreamSignature {
        UpstreamSignature {
            station_name: selected.station_name.clone(),
            index: selected.index,
            base_url: selected.upstream.base_url.clone(),
            tags: hash_string_map_to_btree(&selected.upstream.tags),
            supported_models: hash_bool_map_to_btree(&selected.upstream.supported_models),
            model_mapping: hash_string_map_to_btree(&selected.upstream.model_mapping),
        }
    }

    fn provider_ids_from_attempt_events(events: &[AttemptOrderEvent]) -> Vec<String> {
        events
            .iter()
            .map(|event| {
                event
                    .upstream
                    .tags
                    .get("provider_id")
                    .expect("provider_id tag")
                    .clone()
            })
            .collect()
    }

    fn skip_reason(reason: &RoutePlanSkipReason) -> &'static str {
        reason.code()
    }

    fn sorted_hash_set(values: &HashSet<usize>) -> Vec<usize> {
        let mut sorted = values.iter().copied().collect::<Vec<_>>();
        sorted.sort_unstable();
        sorted
    }

    fn station_exhausted(upstream_total: usize, avoid: &HashSet<usize>) -> bool {
        upstream_total > 0
            && avoid.iter().filter(|&&idx| idx < upstream_total).count() >= upstream_total
    }

    fn executor_selected_upstream_signatures(
        template: &RoutePlanTemplate,
    ) -> Vec<UpstreamSignature> {
        RoutePlanExecutor::new(template)
            .iter_selected_upstreams()
            .map(|selected| upstream_signature(&selected.selected_upstream))
            .collect()
    }

    fn legacy_routing_load_balancer(view: ServiceViewV4) -> LoadBalancer {
        let runtime = compile_v4_to_runtime(&ProxyConfigV4 {
            codex: view,
            ..ProxyConfigV4::default()
        })
        .expect("compile v4 runtime");
        let service = runtime
            .codex
            .station("routing")
            .expect("routing station")
            .clone();
        LoadBalancer::new(
            Arc::new(service),
            Arc::new(Mutex::new(HashMap::<String, LbState>::new())),
        )
    }

    fn legacy_load_balancer_selected_upstream_signatures(
        view: ServiceViewV4,
    ) -> Vec<UpstreamSignature> {
        let lb = legacy_routing_load_balancer(view);
        let upstream_count = lb.service.upstreams.len();
        let mut avoid = HashSet::new();
        let mut selected = Vec::new();
        while selected.len() < upstream_count {
            let next = lb
                .select_upstream_avoiding_strict(&avoid)
                .expect("legacy selected upstream");
            avoid.insert(next.index);
            selected.push(upstream_signature(&next));
        }
        selected
    }

    fn legacy_shadow_attempt_order_signatures(
        view: ServiceViewV4,
        request_model: Option<&str>,
    ) -> Vec<AttemptOrderEvent> {
        let lb = legacy_routing_load_balancer(view);
        let total_upstreams = lb.service.upstreams.len();
        let mut avoid = HashSet::new();
        let mut avoided_total = 0usize;
        let mut events = Vec::new();

        while !station_exhausted(total_upstreams, &avoid) {
            let Some(selected) = lb.select_upstream_avoiding_strict(&avoid) else {
                break;
            };

            if let Some(requested_model) = request_model {
                let supported = model_routing::is_model_supported(
                    &selected.upstream.supported_models,
                    &selected.upstream.model_mapping,
                    requested_model,
                );
                if !supported {
                    if avoid.insert(selected.index) {
                        avoided_total = avoided_total.saturating_add(1);
                    }
                    events.push(AttemptOrderEvent {
                        decision: "skipped_capability_mismatch",
                        upstream: upstream_signature(&selected),
                        avoid_for_station: sorted_hash_set(&avoid),
                        avoided_total,
                        total_upstreams,
                        reason: Some("unsupported_model"),
                    });
                    continue;
                }
            }

            events.push(AttemptOrderEvent {
                decision: "selected",
                upstream: upstream_signature(&selected),
                avoid_for_station: sorted_hash_set(&avoid),
                avoided_total,
                total_upstreams,
                reason: None,
            });

            if avoid.insert(selected.index) {
                avoided_total = avoided_total.saturating_add(1);
            }
        }

        events
    }

    fn executor_shadow_attempt_order_signatures(
        view: &ServiceViewV4,
        request_model: Option<&str>,
    ) -> Vec<AttemptOrderEvent> {
        let template = compile_v4_route_plan_template("codex", view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut state = RoutePlanAttemptState::default();
        let mut events = Vec::new();

        loop {
            let selection = executor.select_supported_candidate(&mut state, request_model);
            events.extend(
                selection
                    .skipped
                    .into_iter()
                    .map(|skipped| AttemptOrderEvent {
                        decision: "skipped_capability_mismatch",
                        upstream: upstream_signature(&skipped.selected_upstream),
                        avoid_for_station: skipped.avoid_for_station,
                        avoided_total: skipped.avoided_total,
                        total_upstreams: skipped.total_upstreams,
                        reason: Some(skip_reason(&skipped.reason)),
                    }),
            );

            let Some(selected) = selection.selected else {
                break;
            };
            events.push(AttemptOrderEvent {
                decision: "selected",
                upstream: upstream_signature(&selected.selected_upstream),
                avoid_for_station: selection.avoid_for_station,
                avoided_total: selection.avoided_total,
                total_upstreams: selection.total_upstreams,
                reason: None,
            });
            state.avoid_selected(&selected);
        }

        events
    }

    fn assert_executor_matches_legacy_load_balancer(view: ServiceViewV4) {
        let template = compile_v4_route_plan_template("codex", &view).expect("route template");
        assert_eq!(
            executor_selected_upstream_signatures(&template),
            legacy_load_balancer_selected_upstream_signatures(view)
        );
    }

    #[test]
    fn routing_ir_one_provider_matches_resolved_order() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([(
                "input".to_string(),
                provider("https://input.example/v1"),
            )]),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template("codex", &view).expect("route template");

        assert_provider_order_parity(&view, &template);
        assert_eq!(template.entry, "main");
        assert_eq!(template.candidates[0].endpoint_id, "default");
        assert_eq!(template.candidates[0].base_url, "https://input.example/v1");
        assert_eq!(template.candidates[0].route_path, vec!["main", "input"]);
        assert_eq!(
            template.candidates[0]
                .tags
                .get("provider_id")
                .map(String::as_str),
            Some("input")
        );
    }

    #[test]
    fn routing_ir_v4_candidate_identity_retains_provider_endpoint_and_legacy_key() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([(
                "input".to_string(),
                provider("https://input.example/v1"),
            )]),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template("codex", &view).expect("route template");
        let identities = template.candidate_identities();

        assert_eq!(identities.len(), 1);
        assert_eq!(
            identities[0].provider_endpoint.stable_key(),
            "codex/input/default"
        );
        assert_eq!(identities[0].legacy.stable_key(), "codex/routing/0");
        assert_eq!(identities[0].base_url, "https://input.example/v1");
    }

    #[test]
    fn routing_ir_ordered_failover_matches_resolved_order() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "primary".to_string(),
                    provider("https://primary.example/v1"),
                ),
                ("backup".to_string(), provider("https://backup.example/v1")),
            ]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "backup".to_string(),
                "primary".to_string(),
            ])),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template("codex", &view).expect("route template");

        assert_provider_order_parity(&view, &template);
        assert_eq!(provider_ids(&template), vec!["backup", "primary"]);
    }

    #[test]
    fn routing_ir_nested_route_graph_preserves_candidate_order_and_path() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "input".to_string(),
                    tagged_provider("https://input.example/v1", "billing", "monthly"),
                ),
                (
                    "input1".to_string(),
                    tagged_provider("https://input1.example/v1", "billing", "monthly"),
                ),
                (
                    "paygo".to_string(),
                    tagged_provider("https://paygo.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(RoutingConfigV4 {
                entry: "monthly_first".to_string(),
                routes: BTreeMap::from([
                    (
                        "monthly_pool".to_string(),
                        RoutingNodeV4 {
                            strategy: RoutingPolicyV4::OrderedFailover,
                            children: vec!["input".to_string(), "input1".to_string()],
                            ..RoutingNodeV4::default()
                        },
                    ),
                    (
                        "monthly_first".to_string(),
                        RoutingNodeV4 {
                            strategy: RoutingPolicyV4::OrderedFailover,
                            children: vec!["monthly_pool".to_string(), "paygo".to_string()],
                            ..RoutingNodeV4::default()
                        },
                    ),
                ]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template("codex", &view).expect("route template");

        assert_provider_order_parity(&view, &template);
        assert_eq!(provider_ids(&template), vec!["input", "input1", "paygo"]);
        assert_eq!(
            template.candidates[1].route_path,
            vec!["monthly_first", "monthly_pool", "input1"]
        );
        assert_eq!(
            template.candidates[2].route_path,
            vec!["monthly_first", "paygo"]
        );
    }

    #[test]
    fn routing_ir_manual_sticky_matches_resolved_order() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "primary".to_string(),
                    provider("https://primary.example/v1"),
                ),
                ("backup".to_string(), provider("https://backup.example/v1")),
            ]),
            routing: Some(RoutingConfigV4::manual_sticky(
                "backup".to_string(),
                vec!["backup".to_string(), "primary".to_string()],
            )),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template("codex", &view).expect("route template");

        assert_provider_order_parity(&view, &template);
        assert_eq!(provider_ids(&template), vec!["backup"]);
        assert_eq!(template.candidates[0].route_path, vec!["main", "backup"]);
    }

    #[test]
    fn routing_ir_tag_preferred_continue_matches_resolved_order() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    tagged_provider("https://monthly.example/v1", "billing", "monthly"),
                ),
                (
                    "paygo".to_string(),
                    tagged_provider("https://paygo.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(RoutingConfigV4::tag_preferred(
                vec!["paygo".to_string(), "monthly".to_string()],
                vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                RoutingExhaustedActionV4::Continue,
            )),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template("codex", &view).expect("route template");

        assert_provider_order_parity(&view, &template);
        assert_eq!(provider_ids(&template), vec!["monthly", "paygo"]);
    }

    #[test]
    fn routing_ir_tag_preferred_stop_matches_resolved_order() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    tagged_provider("https://monthly.example/v1", "billing", "monthly"),
                ),
                (
                    "paygo".to_string(),
                    tagged_provider("https://paygo.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(RoutingConfigV4::tag_preferred(
                vec!["paygo".to_string(), "monthly".to_string()],
                vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                RoutingExhaustedActionV4::Stop,
            )),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template("codex", &view).expect("route template");

        assert_provider_order_parity(&view, &template);
        assert_eq!(provider_ids(&template), vec!["monthly"]);
    }

    #[test]
    fn routing_ir_conditional_route_selects_then_branch_for_matching_request() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                ("small".to_string(), provider("https://small.example/v1")),
                ("large".to_string(), provider("https://large.example/v1")),
            ]),
            routing: Some(RoutingConfigV4 {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RoutingNodeV4 {
                        strategy: RoutingPolicyV4::Conditional,
                        when: Some(RoutingConditionV4 {
                            model: Some("gpt-5".to_string()),
                            ..RoutingConditionV4::default()
                        }),
                        then: Some("large".to_string()),
                        default_route: Some("small".to_string()),
                        ..RoutingNodeV4::default()
                    },
                )]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template_with_request(
            "codex",
            &view,
            &RouteRequestContext {
                model: Some("gpt-5".to_string()),
                ..RouteRequestContext::default()
            },
        )
        .expect("conditional route template");

        assert_eq!(provider_ids(&template), vec!["large"]);
        assert_eq!(template.candidates[0].route_path, vec!["root", "large"]);
    }

    #[test]
    fn routing_ir_conditional_route_uses_default_branch_for_no_match() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                ("small".to_string(), provider("https://small.example/v1")),
                ("large".to_string(), provider("https://large.example/v1")),
            ]),
            routing: Some(RoutingConfigV4 {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RoutingNodeV4 {
                        strategy: RoutingPolicyV4::Conditional,
                        when: Some(RoutingConditionV4 {
                            service_tier: Some("priority".to_string()),
                            ..RoutingConditionV4::default()
                        }),
                        then: Some("large".to_string()),
                        default_route: Some("small".to_string()),
                        ..RoutingNodeV4::default()
                    },
                )]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template_with_request(
            "codex",
            &view,
            &RouteRequestContext {
                service_tier: Some("default".to_string()),
                ..RouteRequestContext::default()
            },
        )
        .expect("conditional route template");

        assert_eq!(provider_ids(&template), vec!["small"]);
        assert_eq!(template.candidates[0].route_path, vec!["root", "small"]);
    }

    #[test]
    fn routing_ir_conditional_route_composes_with_ordered_fallback_branch() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "large-primary".to_string(),
                    provider("https://large-primary.example/v1"),
                ),
                (
                    "large-backup".to_string(),
                    provider("https://large-backup.example/v1"),
                ),
                ("small".to_string(), provider("https://small.example/v1")),
            ]),
            routing: Some(RoutingConfigV4 {
                entry: "root".to_string(),
                routes: BTreeMap::from([
                    (
                        "root".to_string(),
                        RoutingNodeV4 {
                            strategy: RoutingPolicyV4::Conditional,
                            when: Some(RoutingConditionV4 {
                                model: Some("gpt-5".to_string()),
                                ..RoutingConditionV4::default()
                            }),
                            then: Some("large_pool".to_string()),
                            default_route: Some("small".to_string()),
                            ..RoutingNodeV4::default()
                        },
                    ),
                    (
                        "large_pool".to_string(),
                        RoutingNodeV4 {
                            strategy: RoutingPolicyV4::OrderedFailover,
                            children: vec!["large-primary".to_string(), "large-backup".to_string()],
                            ..RoutingNodeV4::default()
                        },
                    ),
                ]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template_with_request(
            "codex",
            &view,
            &RouteRequestContext {
                model: Some("gpt-5".to_string()),
                ..RouteRequestContext::default()
            },
        )
        .expect("conditional route template");

        assert_eq!(
            provider_ids(&template),
            vec!["large-primary", "large-backup"]
        );
        assert_eq!(
            template.candidates[1].route_path,
            vec!["root", "large_pool", "large-backup"]
        );
    }

    #[test]
    fn routing_ir_conditional_route_rejects_missing_default() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                ("small".to_string(), provider("https://small.example/v1")),
                ("large".to_string(), provider("https://large.example/v1")),
            ]),
            routing: Some(RoutingConfigV4 {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RoutingNodeV4 {
                        strategy: RoutingPolicyV4::Conditional,
                        when: Some(RoutingConditionV4 {
                            model: Some("gpt-5".to_string()),
                            ..RoutingConditionV4::default()
                        }),
                        then: Some("large".to_string()),
                        ..RoutingNodeV4::default()
                    },
                )]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        };

        let err = compile_v4_route_plan_template_with_request(
            "codex",
            &view,
            &RouteRequestContext {
                model: Some("gpt-5".to_string()),
                ..RouteRequestContext::default()
            },
        )
        .expect_err("missing default should fail");

        assert!(err.to_string().contains("requires default"));
    }

    #[test]
    fn routing_ir_conditional_route_rejects_empty_condition() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                ("small".to_string(), provider("https://small.example/v1")),
                ("large".to_string(), provider("https://large.example/v1")),
            ]),
            routing: Some(RoutingConfigV4 {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RoutingNodeV4 {
                        strategy: RoutingPolicyV4::Conditional,
                        when: Some(RoutingConditionV4::default()),
                        then: Some("large".to_string()),
                        default_route: Some("small".to_string()),
                        ..RoutingNodeV4::default()
                    },
                )]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        };

        let err = compile_v4_route_plan_template_with_request(
            "codex",
            &view,
            &RouteRequestContext::default(),
        )
        .expect_err("empty condition should fail");

        assert!(
            err.to_string()
                .contains("requires at least one condition field")
        );
    }

    #[test]
    fn routing_ir_conditional_route_is_not_flattened_to_legacy_runtime_path() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                ("small".to_string(), provider("https://small.example/v1")),
                ("large".to_string(), provider("https://large.example/v1")),
            ]),
            routing: Some(RoutingConfigV4 {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RoutingNodeV4 {
                        strategy: RoutingPolicyV4::Conditional,
                        when: Some(RoutingConditionV4 {
                            model: Some("gpt-5".to_string()),
                            ..RoutingConditionV4::default()
                        }),
                        then: Some("large".to_string()),
                        default_route: Some("small".to_string()),
                        ..RoutingNodeV4::default()
                    },
                )]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        };
        let cfg = ProxyConfigV4 {
            version: 4,
            codex: view,
            ..ProxyConfigV4::default()
        };

        let err = compile_v4_to_runtime(&cfg).expect_err("conditional should not flatten");

        assert!(
            err.to_string()
                .contains("requires request-aware route execution")
        );
    }

    #[test]
    fn routing_ir_candidate_expands_provider_endpoints_in_runtime_order() {
        let mut endpoints = BTreeMap::new();
        endpoints.insert(
            "slow".to_string(),
            ProviderEndpointV4 {
                base_url: "https://slow.example/v1".to_string(),
                enabled: true,
                priority: 10,
                tags: BTreeMap::from([("region".to_string(), "us".to_string())]),
                supported_models: BTreeMap::from([("gpt-4.1".to_string(), true)]),
                model_mapping: BTreeMap::new(),
            },
        );
        endpoints.insert(
            "fast".to_string(),
            ProviderEndpointV4 {
                base_url: "https://fast.example/v1".to_string(),
                enabled: true,
                priority: 0,
                tags: BTreeMap::from([("region".to_string(), "hk".to_string())]),
                supported_models: BTreeMap::new(),
                model_mapping: BTreeMap::from([(
                    "gpt-5".to_string(),
                    "provider-gpt-5".to_string(),
                )]),
            },
        );
        let view = ServiceViewV4 {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfigV4 {
                    tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                    supported_models: BTreeMap::from([("gpt-5".to_string(), true)]),
                    endpoints,
                    ..ProviderConfigV4::default()
                },
            )]),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template("codex", &view).expect("route template");

        assert_eq!(provider_ids(&template), vec!["input", "input"]);
        assert_eq!(template.candidates[0].endpoint_id, "fast");
        assert_eq!(template.candidates[1].endpoint_id, "slow");
        assert_eq!(
            provider_endpoint_keys(&template),
            vec!["codex/input/fast", "codex/input/slow"]
        );
        assert_eq!(
            legacy_upstream_keys(&template),
            vec!["codex/routing/0", "codex/routing/1"]
        );
        assert_eq!(
            template.candidates[0]
                .tags
                .get("billing")
                .map(String::as_str),
            Some("monthly")
        );
        assert_eq!(
            template.candidates[0]
                .tags
                .get("region")
                .map(String::as_str),
            Some("hk")
        );
        assert_eq!(
            template.candidates[0]
                .model_mapping
                .get("gpt-5")
                .map(String::as_str),
            Some("provider-gpt-5")
        );
        assert_eq!(
            template.candidates[1].supported_models.get("gpt-5"),
            Some(&true)
        );
        assert_eq!(
            template.candidates[1].supported_models.get("gpt-4.1"),
            Some(&true)
        );
    }

    #[test]
    fn routing_ir_legacy_template_identity_uses_tagged_provider_and_station_index() {
        let service = ServiceConfig {
            name: "legacy-station".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: vec![UpstreamConfig {
                base_url: "https://legacy.example/v1".to_string(),
                auth: UpstreamAuth::default(),
                tags: HashMap::from([("provider_id".to_string(), "tagged".to_string())]),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        };

        let template = compile_legacy_route_plan_template("codex", [&service]);
        let identities = template.candidate_identities();

        assert_eq!(identities.len(), 1);
        assert_eq!(
            identities[0].provider_endpoint.stable_key(),
            "codex/tagged/0"
        );
        assert_eq!(identities[0].legacy.stable_key(), "codex/legacy-station/0");
        assert_eq!(identities[0].base_url, "https://legacy.example/v1");
    }

    #[test]
    fn route_plan_executor_matches_legacy_load_balancer_for_nested_route() {
        assert_executor_matches_legacy_load_balancer(ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "input".to_string(),
                    tagged_provider("https://input.example/v1", "billing", "monthly"),
                ),
                (
                    "input1".to_string(),
                    tagged_provider("https://input1.example/v1", "billing", "monthly"),
                ),
                (
                    "paygo".to_string(),
                    tagged_provider("https://paygo.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(RoutingConfigV4 {
                entry: "monthly_first".to_string(),
                routes: BTreeMap::from([
                    (
                        "monthly_pool".to_string(),
                        RoutingNodeV4 {
                            strategy: RoutingPolicyV4::OrderedFailover,
                            children: vec!["input".to_string(), "input1".to_string()],
                            ..RoutingNodeV4::default()
                        },
                    ),
                    (
                        "monthly_first".to_string(),
                        RoutingNodeV4 {
                            strategy: RoutingPolicyV4::OrderedFailover,
                            children: vec!["monthly_pool".to_string(), "paygo".to_string()],
                            ..RoutingNodeV4::default()
                        },
                    ),
                ]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        });
    }

    #[test]
    fn route_plan_executor_matches_legacy_load_balancer_for_tag_preferred() {
        assert_executor_matches_legacy_load_balancer(ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    tagged_provider("https://monthly.example/v1", "billing", "monthly"),
                ),
                (
                    "paygo".to_string(),
                    tagged_provider("https://paygo.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(RoutingConfigV4::tag_preferred(
                vec!["paygo".to_string(), "monthly".to_string()],
                vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                RoutingExhaustedActionV4::Continue,
            )),
            ..ServiceViewV4::default()
        });
    }

    #[test]
    fn route_plan_executor_matches_legacy_load_balancer_for_multi_endpoint_provider() {
        let mut endpoints = BTreeMap::new();
        endpoints.insert(
            "slow".to_string(),
            ProviderEndpointV4 {
                base_url: "https://slow.example/v1".to_string(),
                enabled: true,
                priority: 10,
                tags: BTreeMap::from([("region".to_string(), "us".to_string())]),
                supported_models: BTreeMap::from([("gpt-4.1".to_string(), true)]),
                model_mapping: BTreeMap::new(),
            },
        );
        endpoints.insert(
            "fast".to_string(),
            ProviderEndpointV4 {
                base_url: "https://fast.example/v1".to_string(),
                enabled: true,
                priority: 0,
                tags: BTreeMap::from([("region".to_string(), "hk".to_string())]),
                supported_models: BTreeMap::new(),
                model_mapping: BTreeMap::from([(
                    "gpt-5".to_string(),
                    "provider-gpt-5".to_string(),
                )]),
            },
        );

        assert_executor_matches_legacy_load_balancer(ServiceViewV4 {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfigV4 {
                    tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                    supported_models: BTreeMap::from([("gpt-5".to_string(), true)]),
                    endpoints,
                    ..ProviderConfigV4::default()
                },
            )]),
            ..ServiceViewV4::default()
        });
    }

    #[test]
    fn route_plan_executor_shadow_attempt_order_matches_legacy_failover_avoidance() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "primary".to_string(),
                    provider("https://primary.example/v1"),
                ),
                ("backup".to_string(), provider("https://backup.example/v1")),
                ("paygo".to_string(), provider("https://paygo.example/v1")),
            ]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "backup".to_string(),
                "primary".to_string(),
                "paygo".to_string(),
            ])),
            ..ServiceViewV4::default()
        };

        let executor_events = executor_shadow_attempt_order_signatures(&view, None);
        let legacy_events = legacy_shadow_attempt_order_signatures(view, None);

        assert_eq!(executor_events, legacy_events);
        assert_eq!(
            provider_ids_from_attempt_events(&executor_events),
            vec!["backup", "primary", "paygo"]
        );
        assert_eq!(executor_events[0].avoid_for_station, Vec::<usize>::new());
        assert_eq!(executor_events[1].avoid_for_station, vec![0]);
        assert_eq!(executor_events[2].avoid_for_station, vec![0, 1]);
    }

    #[test]
    fn route_plan_executor_shadow_attempt_order_matches_legacy_unsupported_model_skip() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "legacy".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://legacy.example/v1".to_string()),
                        supported_models: BTreeMap::from([("gpt-4.1".to_string(), true)]),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "mapped".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://mapped.example/v1".to_string()),
                        model_mapping: BTreeMap::from([(
                            "gpt-5".to_string(),
                            "provider-gpt-5".to_string(),
                        )]),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "fallback".to_string(),
                    provider("https://fallback.example/v1"),
                ),
            ]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "legacy".to_string(),
                "mapped".to_string(),
                "fallback".to_string(),
            ])),
            ..ServiceViewV4::default()
        };

        let executor_events = executor_shadow_attempt_order_signatures(&view, Some("gpt-5"));
        let legacy_events = legacy_shadow_attempt_order_signatures(view, Some("gpt-5"));

        assert_eq!(executor_events, legacy_events);
        assert_eq!(
            executor_events
                .iter()
                .map(|event| event.decision)
                .collect::<Vec<_>>(),
            vec!["skipped_capability_mismatch", "selected", "selected"]
        );
        assert_eq!(
            provider_ids_from_attempt_events(&executor_events),
            vec!["legacy", "mapped", "fallback"]
        );
        assert_eq!(executor_events[0].reason, Some("unsupported_model"));
        assert_eq!(executor_events[0].avoid_for_station, vec![0]);
        assert_eq!(executor_events[1].avoid_for_station, vec![0]);
        assert_eq!(executor_events[2].avoid_for_station, vec![0, 1]);
    }

    #[test]
    fn route_plan_executor_shadow_attempt_order_matches_legacy_all_unsupported_exhaustion() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "old".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://old.example/v1".to_string()),
                        supported_models: BTreeMap::from([("gpt-4.1".to_string(), true)]),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "older".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://older.example/v1".to_string()),
                        supported_models: BTreeMap::from([("gpt-4o".to_string(), true)]),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "old".to_string(),
                "older".to_string(),
            ])),
            ..ServiceViewV4::default()
        };

        let executor_events = executor_shadow_attempt_order_signatures(&view, Some("gpt-5"));
        let legacy_events = legacy_shadow_attempt_order_signatures(view.clone(), Some("gpt-5"));

        assert_eq!(executor_events, legacy_events);
        assert_eq!(
            executor_events
                .iter()
                .map(|event| event.decision)
                .collect::<Vec<_>>(),
            vec!["skipped_capability_mismatch", "skipped_capability_mismatch"]
        );

        let template = compile_v4_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut state = RoutePlanAttemptState::default();
        let selection = executor.select_supported_candidate(&mut state, Some("gpt-5"));

        assert!(selection.selected.is_none());
        assert_eq!(selection.skipped.len(), 2);
        assert_eq!(selection.avoid_for_station, vec![0, 1]);
        assert!(state.station_exhausted(selection.total_upstreams));
    }

    #[test]
    fn route_plan_executor_explains_structured_skip_reasons() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "unsupported".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://unsupported.example/v1".to_string()),
                        supported_models: BTreeMap::from([("gpt-4.1".to_string(), true)]),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "disabled".to_string(),
                    provider("https://disabled.example/v1"),
                ),
                (
                    "cooldown".to_string(),
                    provider("https://cooldown.example/v1"),
                ),
                (
                    "breaker".to_string(),
                    provider("https://breaker.example/v1"),
                ),
                ("usage".to_string(), provider("https://usage.example/v1")),
                (
                    "missing-auth".to_string(),
                    provider("https://missing-auth.example/v1"),
                ),
                (
                    "healthy".to_string(),
                    provider("https://healthy.example/v1"),
                ),
            ]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "unsupported".to_string(),
                "disabled".to_string(),
                "cooldown".to_string(),
                "breaker".to_string(),
                "usage".to_string(),
                "missing-auth".to_string(),
                "healthy".to_string(),
            ])),
            ..ServiceViewV4::default()
        };
        let template = compile_v4_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        let mut station = RoutePlanStationRuntimeState::default();
        station.set_upstream(
            1,
            RoutePlanUpstreamRuntimeState {
                runtime_disabled: true,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        station.set_upstream(
            2,
            RoutePlanUpstreamRuntimeState {
                cooldown_active: true,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        station.set_upstream(
            3,
            RoutePlanUpstreamRuntimeState {
                failure_count: FAILURE_THRESHOLD,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        station.set_upstream(
            4,
            RoutePlanUpstreamRuntimeState {
                usage_exhausted: true,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        station.set_upstream(
            5,
            RoutePlanUpstreamRuntimeState {
                missing_auth: true,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        runtime.set_station("routing", station);

        let explanations =
            executor.explain_candidate_skip_reasons_with_runtime_state(&runtime, Some("gpt-5"));
        let reasons_by_provider = explanations
            .iter()
            .map(|explanation| {
                (
                    explanation.candidate.provider_id.as_str(),
                    explanation
                        .reasons
                        .iter()
                        .map(RoutePlanSkipReason::code)
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            reasons_by_provider.get("unsupported").cloned(),
            Some(vec!["unsupported_model"])
        );
        assert_eq!(
            reasons_by_provider.get("disabled").cloned(),
            Some(vec!["runtime_disabled"])
        );
        assert_eq!(
            reasons_by_provider.get("cooldown").cloned(),
            Some(vec!["cooldown"])
        );
        assert_eq!(
            reasons_by_provider.get("breaker").cloned(),
            Some(vec!["breaker_open"])
        );
        assert_eq!(
            reasons_by_provider.get("usage").cloned(),
            Some(vec!["usage_exhausted"])
        );
        assert_eq!(
            reasons_by_provider.get("missing-auth").cloned(),
            Some(vec!["missing_auth"])
        );
        assert!(!reasons_by_provider.contains_key("healthy"));

        let mut state = RoutePlanAttemptState::default();
        let selection = executor.select_supported_candidate_with_runtime_state(
            &mut state,
            &runtime,
            Some("gpt-5"),
        );
        let selected = selection.selected.expect("healthy candidate selected");
        assert_eq!(selected.candidate.provider_id, "healthy");
    }

    #[test]
    fn route_plan_executor_shadow_keeps_same_candidate_until_caller_marks_avoidance() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                ("first".to_string(), provider("https://first.example/v1")),
                ("second".to_string(), provider("https://second.example/v1")),
            ]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "first".to_string(),
                "second".to_string(),
            ])),
            ..ServiceViewV4::default()
        };
        let template = compile_v4_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut state = RoutePlanAttemptState::default();

        let first = executor.select_supported_candidate(&mut state, None);
        let first_selected = first.selected.as_ref().expect("first selected");
        assert_eq!(first_selected.selected_upstream.index, 0);

        let same = executor.select_supported_candidate(&mut state, None);
        let same_selected = same.selected.as_ref().expect("same selected");
        assert_eq!(same_selected.selected_upstream.index, 0);

        assert!(state.avoid_selected(same_selected));
        let next = executor.select_supported_candidate(&mut state, None);
        let next_selected = next.selected.as_ref().expect("next selected");

        assert_eq!(next_selected.selected_upstream.index, 1);
        assert_eq!(next.avoid_for_station, vec![0]);
    }
}

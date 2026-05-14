use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};

use anyhow::{Context, Result};

use crate::config::{
    ProviderConfigV4, RoutingAffinityPolicyV5, RoutingConditionV4, RoutingConfigV4,
    RoutingExhaustedActionV4, RoutingNodeV4, RoutingPolicyV4, ServiceConfig, ServiceViewV4,
    UpstreamAuth, UpstreamConfig, effective_v4_routing,
};
use crate::lb::{FAILURE_THRESHOLD, SelectedUpstream};
use crate::model_routing;
use crate::runtime_identity::{LegacyUpstreamKey, ProviderEndpointKey, RuntimeUpstreamIdentity};

const V4_COMPATIBILITY_STATION_NAME: &str = "routing";

#[derive(Debug, Clone)]
pub struct RoutePlanTemplate {
    pub service_name: String,
    pub entry: String,
    pub affinity_policy: RoutingAffinityPolicyV5,
    pub fallback_ttl_ms: Option<u64>,
    pub reprobe_preferred_after_ms: Option<u64>,
    pub nodes: BTreeMap<String, RouteNodePlan>,
    pub expanded_provider_order: Vec<String>,
    pub candidates: Vec<RouteCandidate>,
    pub compatibility_station_name: Option<String>,
}

impl RoutePlanTemplate {
    pub fn route_graph_key(&self) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.service_name.hash(&mut hasher);
        self.entry.hash(&mut hasher);
        self.affinity_policy.hash(&mut hasher);
        self.fallback_ttl_ms.hash(&mut hasher);
        self.reprobe_preferred_after_ms.hash(&mut hasher);
        self.compatibility_station_name.hash(&mut hasher);
        self.expanded_provider_order.hash(&mut hasher);
        hash_route_nodes(&self.nodes, &mut hasher);
        for candidate in &self.candidates {
            hash_route_candidate(candidate, &mut hasher);
        }
        format!("v4:{:016x}", hasher.finish())
    }

    pub fn contains_provider_endpoint(&self, key: &ProviderEndpointKey, base_url: &str) -> bool {
        key.service_name == self.service_name
            && self.candidates.iter().any(|candidate| {
                candidate.provider_id == key.provider_id
                    && candidate.endpoint_id == key.endpoint_id
                    && candidate.base_url == base_url
            })
    }

    pub fn candidate_provider_endpoint_key(
        &self,
        candidate: &RouteCandidate,
    ) -> ProviderEndpointKey {
        candidate_provider_endpoint_key(self, candidate)
    }

    pub fn candidate_identity(&self, candidate: &RouteCandidate) -> RuntimeUpstreamIdentity {
        RuntimeUpstreamIdentity::new(
            candidate_provider_endpoint_key(self, candidate),
            self.candidate_compatibility_key(candidate),
            candidate.base_url.clone(),
        )
    }

    pub fn candidate_identities(&self) -> Vec<RuntimeUpstreamIdentity> {
        self.candidates
            .iter()
            .map(|candidate| self.candidate_identity(candidate))
            .collect()
    }

    pub fn candidate_compatibility_key(
        &self,
        candidate: &RouteCandidate,
    ) -> Option<LegacyUpstreamKey> {
        candidate
            .compatibility_station_name
            .as_ref()
            .or(self.compatibility_station_name.as_ref())
            .and_then(|station_name| {
                candidate
                    .compatibility_upstream_index
                    .map(|upstream_index| {
                        LegacyUpstreamKey::new(
                            self.service_name.clone(),
                            station_name.clone(),
                            upstream_index,
                        )
                    })
            })
    }
}

fn candidate_provider_endpoint_key(
    template: &RoutePlanTemplate,
    candidate: &RouteCandidate,
) -> ProviderEndpointKey {
    ProviderEndpointKey::new(
        template.service_name.clone(),
        candidate.provider_id.clone(),
        candidate.endpoint_id.clone(),
    )
}

// Keep the affinity key aligned with selection semantics, not just leaf identity.
// Any change that can alter route selection or the selected upstream's routing metadata
// must change this fingerprint so session stickiness does not bleed across config edits.
fn hash_route_nodes<H: Hasher>(nodes: &BTreeMap<String, RouteNodePlan>, hasher: &mut H) {
    for (name, node) in nodes {
        name.hash(hasher);
        hash_route_node(node, hasher);
    }
}

fn hash_route_node<H: Hasher>(node: &RouteNodePlan, hasher: &mut H) {
    node.strategy.hash(hasher);
    node.children.hash(hasher);
    node.target.hash(hasher);
    node.prefer_tags.hash(hasher);
    node.on_exhausted.hash(hasher);
    node.when.hash(hasher);
    node.then.hash(hasher);
    node.default_route.hash(hasher);
}

fn hash_route_candidate<H: Hasher>(candidate: &RouteCandidate, hasher: &mut H) {
    candidate.provider_id.hash(hasher);
    candidate.endpoint_id.hash(hasher);
    candidate.base_url.hash(hasher);
    candidate.tags.hash(hasher);
    candidate.supported_models.hash(hasher);
    candidate.model_mapping.hash(hasher);
    candidate.route_path.hash(hasher);
    candidate.preference_group.hash(hasher);
    candidate.stable_index.hash(hasher);
    candidate.compatibility_station_name.hash(hasher);
    candidate.compatibility_upstream_index.hash(hasher);
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RouteRef {
    Route(String),
    Provider(String),
    ProviderEndpoint {
        provider_id: String,
        endpoint_id: String,
    },
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
    pub preference_group: u32,
    pub stable_index: usize,
    pub compatibility_station_name: Option<String>,
    pub compatibility_upstream_index: Option<usize>,
}

impl RouteCandidate {
    pub fn to_upstream_config(&self) -> UpstreamConfig {
        let mut tags = self.tags.clone();
        tags.insert("endpoint_id".to_string(), self.endpoint_id.clone());
        if let Ok(route_path) = serde_json::to_string(&self.route_path) {
            tags.insert("route_path".to_string(), route_path);
        }
        tags.insert(
            "preference_group".to_string(),
            self.preference_group.to_string(),
        );
        UpstreamConfig {
            base_url: self.base_url.clone(),
            auth: self.auth.clone(),
            tags: btree_string_map_to_hash_map(&tags),
            supported_models: btree_bool_map_to_hash_map(&self.supported_models),
            model_mapping: btree_string_map_to_hash_map(&self.model_mapping),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SelectedRouteCandidate<'a> {
    pub candidate: &'a RouteCandidate,
    pub provider_endpoint: ProviderEndpointKey,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutePlanAttemptState {
    avoided_provider_endpoints: BTreeSet<ProviderEndpointKey>,
    avoid_by_station: BTreeMap<String, BTreeSet<usize>>,
    avoided_total: usize,
}

impl RoutePlanAttemptState {
    pub fn avoid_provider_endpoint(&mut self, key: ProviderEndpointKey) -> bool {
        if self.avoided_provider_endpoints.insert(key) {
            self.avoided_total = self.avoided_total.saturating_add(1);
            return true;
        }
        false
    }

    pub fn avoids_provider_endpoint(&self, key: &ProviderEndpointKey) -> bool {
        self.avoided_provider_endpoints.contains(key)
    }

    pub fn avoid_candidate(
        &mut self,
        template: &RoutePlanTemplate,
        candidate: &RouteCandidate,
    ) -> bool {
        self.avoid_provider_endpoint(candidate_provider_endpoint_key(template, candidate))
    }

    pub fn avoids_candidate(
        &self,
        template: &RoutePlanTemplate,
        candidate: &RouteCandidate,
    ) -> bool {
        self.avoids_provider_endpoint(&candidate_provider_endpoint_key(template, candidate))
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
        self.avoid_provider_endpoint(selected.provider_endpoint.clone())
    }

    pub fn avoid_selected_upstream(&mut self, selected: &SelectedUpstream) -> bool {
        self.avoid_upstream(selected.station_name.as_str(), selected.index)
    }

    pub fn avoids_upstream(&self, station_name: &str, upstream_index: usize) -> bool {
        self.avoid_by_station
            .get(station_name)
            .is_some_and(|indices| indices.contains(&upstream_index))
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

    pub fn station_exhausted_for(&self, station_name: &str, upstream_total: usize) -> bool {
        upstream_total > 0
            && self
                .avoid_by_station
                .get(station_name)
                .map(|indices| indices.iter().filter(|&&idx| idx < upstream_total).count())
                .unwrap_or_default()
                >= upstream_total
    }

    pub fn route_candidates_exhausted(&self, template: &RoutePlanTemplate) -> bool {
        !template.candidates.is_empty()
            && template
                .candidates
                .iter()
                .all(|candidate| self.avoids_candidate(template, candidate))
    }

    pub fn route_avoid_candidate_indices(&self, template: &RoutePlanTemplate) -> Vec<usize> {
        template
            .candidates
            .iter()
            .filter(|candidate| self.avoids_candidate(template, candidate))
            .map(|candidate| candidate.stable_index)
            .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutePlanRuntimeState {
    provider_endpoints: BTreeMap<ProviderEndpointKey, RoutePlanUpstreamRuntimeState>,
    affinity_provider_endpoint: Option<ProviderEndpointKey>,
    affinity_last_selected_at_ms: Option<u64>,
    affinity_last_changed_at_ms: Option<u64>,
}

impl RoutePlanRuntimeState {
    pub fn set_provider_endpoint(
        &mut self,
        key: ProviderEndpointKey,
        state: RoutePlanUpstreamRuntimeState,
    ) {
        self.provider_endpoints.insert(key, state);
    }

    pub fn provider_endpoint(&self, key: &ProviderEndpointKey) -> RoutePlanUpstreamRuntimeState {
        self.provider_endpoints
            .get(key)
            .copied()
            .unwrap_or_default()
    }

    pub fn set_affinity_provider_endpoint(&mut self, key: Option<ProviderEndpointKey>) {
        self.affinity_provider_endpoint = key;
        self.affinity_last_selected_at_ms = None;
        self.affinity_last_changed_at_ms = None;
    }

    pub fn set_affinity_provider_endpoint_with_observed_at(
        &mut self,
        key: Option<ProviderEndpointKey>,
        last_selected_at_ms: Option<u64>,
        last_changed_at_ms: Option<u64>,
    ) {
        self.affinity_provider_endpoint = key;
        self.affinity_last_selected_at_ms = last_selected_at_ms;
        self.affinity_last_changed_at_ms = last_changed_at_ms;
    }

    pub fn affinity_provider_endpoint(&self) -> Option<&ProviderEndpointKey> {
        self.affinity_provider_endpoint.as_ref()
    }

    pub fn affinity_last_selected_at_ms(&self) -> Option<u64> {
        self.affinity_last_selected_at_ms
    }

    pub fn affinity_last_changed_at_ms(&self) -> Option<u64> {
        self.affinity_last_changed_at_ms
    }

    pub fn clear_affinity_provider_endpoint(&mut self) {
        self.affinity_provider_endpoint = None;
        self.affinity_last_selected_at_ms = None;
        self.affinity_last_changed_at_ms = None;
    }

    fn runtime_state_for_candidate(
        &self,
        template: &RoutePlanTemplate,
        candidate: &RouteCandidate,
    ) -> RoutePlanUpstreamRuntimeState {
        self.provider_endpoint(&candidate_provider_endpoint_key(template, candidate))
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
    pub provider_endpoint: ProviderEndpointKey,
    pub reason: RoutePlanSkipReason,
    pub avoided_candidate_indices: Vec<usize>,
    pub avoided_total: usize,
    pub total_upstreams: usize,
}

#[derive(Debug, Clone)]
pub struct RouteCandidateSkipExplanation<'a> {
    pub candidate: &'a RouteCandidate,
    pub provider_endpoint: ProviderEndpointKey,
    pub reasons: Vec<RoutePlanSkipReason>,
}

#[derive(Debug, Clone)]
pub struct SelectedStationRouteCandidate<'a> {
    pub candidate: &'a RouteCandidate,
    pub selected_upstream: SelectedUpstream,
}

#[derive(Debug, Clone)]
pub struct SkippedStationRouteCandidate<'a> {
    pub candidate: &'a RouteCandidate,
    pub selected_upstream: SelectedUpstream,
    pub reason: RoutePlanSkipReason,
    pub avoid_for_station: Vec<usize>,
    pub avoided_total: usize,
    pub total_upstreams: usize,
}

#[derive(Debug, Clone)]
pub struct RoutePlanAttemptSelection<'a> {
    pub selected: Option<SelectedRouteCandidate<'a>>,
    pub skipped: Vec<SkippedRouteCandidate<'a>>,
    pub avoided_candidate_indices: Vec<usize>,
    pub avoided_total: usize,
    pub total_upstreams: usize,
}

#[derive(Debug, Clone)]
pub struct RoutePlanStationAttemptSelection<'a> {
    pub selected: Option<SelectedStationRouteCandidate<'a>>,
    pub skipped: Vec<SkippedStationRouteCandidate<'a>>,
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

    pub fn template(&self) -> &'a RoutePlanTemplate {
        self.template
    }

    pub fn iter_selected_upstreams(
        &self,
    ) -> impl Iterator<Item = SelectedStationRouteCandidate<'_>> + '_ {
        self.template
            .candidates
            .iter()
            .map(|candidate| self.selected_station_route_candidate_for_candidate(candidate))
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
                let provider_endpoint = candidate_provider_endpoint_key(self.template, candidate);
                let runtime_state = runtime.runtime_state_for_candidate(self.template, candidate);
                let reasons = self.candidate_skip_reasons(candidate, runtime_state, request_model);
                (!reasons.is_empty()).then_some(RouteCandidateSkipExplanation {
                    candidate,
                    provider_endpoint,
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
                    avoided_candidate_indices: state.route_avoid_candidate_indices(self.template),
                    avoided_total: state.avoided_total(),
                    total_upstreams,
                };
            }

            let Some(candidate) = self.next_unavoided_candidate(state, runtime) else {
                return RoutePlanAttemptSelection {
                    selected: None,
                    skipped,
                    avoided_candidate_indices: state.route_avoid_candidate_indices(self.template),
                    avoided_total: state.avoided_total(),
                    total_upstreams,
                };
            };
            if let Some(requested_model) = request_model
                && !candidate_supports_model(candidate, requested_model)
            {
                state.avoid_candidate(self.template, candidate);
                let avoided_candidate_indices = state.route_avoid_candidate_indices(self.template);
                skipped.push(SkippedRouteCandidate {
                    candidate,
                    provider_endpoint: candidate_provider_endpoint_key(self.template, candidate),
                    reason: RoutePlanSkipReason::UnsupportedModel {
                        requested_model: requested_model.to_string(),
                    },
                    avoided_candidate_indices,
                    avoided_total: state.avoided_total(),
                    total_upstreams,
                });
                continue;
            }

            let avoided_candidate_indices = state.route_avoid_candidate_indices(self.template);
            return RoutePlanAttemptSelection {
                selected: Some(self.selected_route_candidate_for_candidate(candidate)),
                skipped,
                avoided_candidate_indices,
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
    ) -> RoutePlanStationAttemptSelection<'_> {
        let total_upstreams = self.template.candidates.len();
        let mut skipped = Vec::new();

        loop {
            if state.station_exhausted_for(station_name, self.station_candidate_count(station_name))
            {
                return RoutePlanStationAttemptSelection {
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
                return RoutePlanStationAttemptSelection {
                    selected: None,
                    skipped,
                    avoid_for_station: state.avoid_for_station_name(station_name),
                    avoided_total: state.avoided_total(),
                    total_upstreams,
                };
            };
            let selected_upstream = self.legacy_selected_upstream_for_candidate(candidate);

            if let Some(requested_model) = request_model
                && !candidate_supports_model(candidate, requested_model)
            {
                state.avoid_selected_upstream(&selected_upstream);
                let avoid_for_station =
                    state.avoid_for_station_name(selected_upstream.station_name.as_str());
                skipped.push(SkippedStationRouteCandidate {
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
            return RoutePlanStationAttemptSelection {
                selected: Some(self.selected_station_route_candidate_for_candidate(candidate)),
                skipped,
                avoid_for_station,
                avoided_total: state.avoided_total(),
                total_upstreams,
            };
        }
    }

    pub fn legacy_selected_upstream_for_candidate(
        &self,
        candidate: &RouteCandidate,
    ) -> SelectedUpstream {
        let mut upstream = candidate.to_upstream_config();
        upstream.tags.insert(
            "provider_endpoint_key".to_string(),
            candidate_provider_endpoint_key(self.template, candidate).stable_key(),
        );
        SelectedUpstream {
            station_name: candidate_compatibility_station_name(self.template, candidate),
            index: candidate_compatibility_upstream_index(candidate),
            upstream,
        }
    }

    fn selected_route_candidate_for_candidate(
        &self,
        candidate: &'a RouteCandidate,
    ) -> SelectedRouteCandidate<'a> {
        SelectedRouteCandidate {
            candidate,
            provider_endpoint: candidate_provider_endpoint_key(self.template, candidate),
        }
    }

    fn selected_station_route_candidate_for_candidate(
        &self,
        candidate: &'a RouteCandidate,
    ) -> SelectedStationRouteCandidate<'a> {
        SelectedStationRouteCandidate {
            candidate,
            selected_upstream: self.legacy_selected_upstream_for_candidate(candidate),
        }
    }

    fn next_unavoided_candidate(
        &self,
        state: &RoutePlanAttemptState,
        runtime: &RoutePlanRuntimeState,
    ) -> Option<&'a RouteCandidate> {
        let route_candidates = self
            .template
            .candidates
            .iter()
            .filter(|candidate| !state.avoids_candidate(self.template, candidate))
            .collect::<Vec<_>>();

        if let Some(candidate) =
            best_candidate_by_affinity_policy(self.template, runtime, &route_candidates, true)
        {
            return Some(candidate);
        }

        best_candidate_by_affinity_policy(self.template, runtime, &route_candidates, false)
    }

    fn candidates_exhausted(&self, state: &RoutePlanAttemptState) -> bool {
        state.route_candidates_exhausted(self.template)
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

        if let Some(candidate) =
            best_candidate_by_affinity_policy(self.template, runtime, &station_candidates, true)
        {
            return Some(candidate);
        }

        best_candidate_by_affinity_policy(self.template, runtime, &station_candidates, false)
    }
}

fn best_candidate_by_affinity_policy<'a>(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    station_candidates: &[&'a RouteCandidate],
    require_usage_available: bool,
) -> Option<&'a RouteCandidate> {
    match template.affinity_policy {
        RoutingAffinityPolicyV5::Off => first_candidate_in_best_preference_group(
            template,
            runtime,
            station_candidates,
            require_usage_available,
        ),
        RoutingAffinityPolicyV5::PreferredGroup => best_candidate_in_preference_group(
            template,
            runtime,
            station_candidates,
            require_usage_available,
        ),
        RoutingAffinityPolicyV5::FallbackSticky => affinity_candidate(
            template,
            runtime,
            station_candidates,
            require_usage_available,
        )
        .filter(|candidate| {
            fallback_affinity_within_configured_window(template, runtime, station_candidates)
                || first_candidate_in_best_preference_group(
                    template,
                    runtime,
                    station_candidates,
                    require_usage_available,
                )
                .is_none_or(|best| best.preference_group >= candidate.preference_group)
        })
        .or_else(|| {
            first_candidate_in_best_preference_group(
                template,
                runtime,
                station_candidates,
                require_usage_available,
            )
        }),
        RoutingAffinityPolicyV5::Hard => {
            if runtime.affinity_provider_endpoint().is_some() {
                affinity_candidate(
                    template,
                    runtime,
                    station_candidates,
                    require_usage_available,
                )
            } else {
                first_candidate_in_best_preference_group(
                    template,
                    runtime,
                    station_candidates,
                    require_usage_available,
                )
            }
        }
    }
}

fn first_candidate_in_best_preference_group<'a>(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    station_candidates: &[&'a RouteCandidate],
    require_usage_available: bool,
) -> Option<&'a RouteCandidate> {
    let best_group = station_candidates
        .iter()
        .copied()
        .filter(|candidate| {
            candidate_available_in_runtime(template, runtime, candidate, require_usage_available)
        })
        .map(|candidate| candidate.preference_group)
        .min()?;

    station_candidates.iter().copied().find(|candidate| {
        candidate.preference_group == best_group
            && candidate_available_in_runtime(template, runtime, candidate, require_usage_available)
    })
}

fn best_candidate_in_preference_group<'a>(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    station_candidates: &[&'a RouteCandidate],
    require_usage_available: bool,
) -> Option<&'a RouteCandidate> {
    let best_group = station_candidates
        .iter()
        .copied()
        .filter(|candidate| {
            candidate_available_in_runtime(template, runtime, candidate, require_usage_available)
        })
        .map(|candidate| candidate.preference_group)
        .min()?;

    if let Some(candidate) = affinity_candidate_in_group(
        template,
        runtime,
        station_candidates,
        best_group,
        require_usage_available,
    ) {
        return Some(candidate);
    }

    station_candidates.iter().copied().find(|candidate| {
        candidate.preference_group == best_group
            && candidate_available_in_runtime(template, runtime, candidate, require_usage_available)
    })
}

fn affinity_candidate<'a>(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    station_candidates: &[&'a RouteCandidate],
    require_usage_available: bool,
) -> Option<&'a RouteCandidate> {
    let affinity_key = runtime.affinity_provider_endpoint()?;
    station_candidates.iter().copied().find(|candidate| {
        candidate_provider_endpoint_key(template, candidate) == *affinity_key
            && candidate_available_in_runtime(template, runtime, candidate, require_usage_available)
    })
}

fn affinity_candidate_in_group<'a>(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    station_candidates: &[&'a RouteCandidate],
    preference_group: u32,
    require_usage_available: bool,
) -> Option<&'a RouteCandidate> {
    let affinity_key = runtime.affinity_provider_endpoint()?;
    station_candidates.iter().copied().find(|candidate| {
        candidate.preference_group == preference_group
            && candidate_provider_endpoint_key(template, candidate) == *affinity_key
            && candidate_available_in_runtime(template, runtime, candidate, require_usage_available)
    })
}

fn fallback_affinity_within_configured_window(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    station_candidates: &[&RouteCandidate],
) -> bool {
    let Some(affinity_key) = runtime.affinity_provider_endpoint() else {
        return true;
    };
    let Some(affinity_candidate) = station_candidates
        .iter()
        .copied()
        .find(|candidate| candidate_provider_endpoint_key(template, candidate) == *affinity_key)
    else {
        return true;
    };
    let Some(best_group) = station_candidates
        .iter()
        .copied()
        .filter(|candidate| candidate_available_in_runtime(template, runtime, candidate, false))
        .map(|candidate| candidate.preference_group)
        .min()
    else {
        return true;
    };
    if affinity_candidate.preference_group <= best_group {
        return true;
    }

    fallback_affinity_age_within_window(
        template.fallback_ttl_ms,
        runtime.affinity_last_changed_at_ms(),
    ) && fallback_affinity_age_within_window(
        template.reprobe_preferred_after_ms,
        runtime.affinity_last_changed_at_ms(),
    )
}

fn fallback_affinity_age_within_window(
    window_ms: Option<u64>,
    observed_at_ms: Option<u64>,
) -> bool {
    let Some(window_ms) = window_ms else {
        return true;
    };
    if window_ms == 0 {
        return false;
    }
    let Some(observed_at_ms) = observed_at_ms else {
        return false;
    };
    crate::logging::now_ms().saturating_sub(observed_at_ms) < window_ms
}

fn candidate_available_in_runtime(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    candidate: &RouteCandidate,
    require_usage_available: bool,
) -> bool {
    let upstream = runtime.runtime_state_for_candidate(template, candidate);
    !upstream.hard_unavailable() && (!require_usage_available || !upstream.usage_exhausted)
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
    endpoint_id: Option<String>,
    route_path: Vec<String>,
    preference_group: u32,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConditionalExpansion {
    MatchRequest,
    AllBranchesForCompatibility,
}

struct RouteExpansionContext<'a> {
    request: &'a RouteRequestContext,
    conditional: ConditionalExpansion,
}

struct RouteExpansionFrame<'a> {
    route_name: &'a str,
    node: &'a RoutingNodeV4,
    node_path: &'a [String],
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
    compile_v4_route_plan_template_with_expansion(
        service_name,
        view,
        request,
        ConditionalExpansion::MatchRequest,
    )
}

pub fn compile_v4_route_plan_template_for_compat_runtime(
    service_name: &str,
    view: &ServiceViewV4,
) -> Result<RoutePlanTemplate> {
    compile_v4_route_plan_template_with_expansion(
        service_name,
        view,
        &RouteRequestContext::default(),
        ConditionalExpansion::AllBranchesForCompatibility,
    )
}

fn compile_v4_route_plan_template_with_expansion(
    service_name: &str,
    view: &ServiceViewV4,
    request: &RouteRequestContext,
    conditional: ConditionalExpansion,
) -> Result<RoutePlanTemplate> {
    let routing = effective_v4_routing(view);
    validate_route_provider_name_conflicts(service_name, view, &routing)?;

    let nodes = normalize_route_nodes(service_name, view, &routing)?;
    let expansion = RouteExpansionContext {
        request,
        conditional,
    };
    let leaves = expand_v4_route_leaves(service_name, view, &routing, &expansion)?;
    ensure_unique_route_leaves(service_name, &leaves)?;

    let expanded_provider_order = leaves
        .iter()
        .map(|leaf| leaf.provider_id.clone())
        .collect::<Vec<_>>();
    let candidates = route_candidates_from_leaves(service_name, view, &leaves)?;

    Ok(RoutePlanTemplate {
        service_name: service_name.to_string(),
        entry: routing.entry,
        affinity_policy: routing.affinity_policy,
        fallback_ttl_ms: routing.fallback_ttl_ms,
        reprobe_preferred_after_ms: routing.reprobe_preferred_after_ms,
        nodes,
        expanded_provider_order,
        compatibility_station_name: None,
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
                preference_group: 0,
                stable_index,
                compatibility_station_name: Some(service.name.clone()),
                compatibility_upstream_index: Some(upstream_index),
            });
        }
    }

    RoutePlanTemplate {
        service_name: service_name.to_string(),
        entry,
        affinity_policy: RoutingAffinityPolicyV5::FallbackSticky,
        fallback_ttl_ms: None,
        reprobe_preferred_after_ms: None,
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
    if let Some((provider_id, endpoint_id)) = split_provider_endpoint_ref(name)
        && let Some(provider) = view.providers.get(provider_id)
        && provider_endpoint_exists(provider, endpoint_id)
    {
        return Ok(RouteRef::ProviderEndpoint {
            provider_id: provider_id.to_string(),
            endpoint_id: endpoint_id.to_string(),
        });
    }
    anyhow::bail!("[{service_name}] routing references missing route or provider '{name}'");
}

fn split_provider_endpoint_ref(name: &str) -> Option<(&str, &str)> {
    let (provider_id, endpoint_id) = name.split_once('.')?;
    let provider_id = provider_id.trim();
    let endpoint_id = endpoint_id.trim();
    if provider_id.is_empty() || endpoint_id.is_empty() {
        return None;
    }
    Some((provider_id, endpoint_id))
}

fn provider_endpoint_exists(provider: &ProviderConfigV4, endpoint_id: &str) -> bool {
    if endpoint_id == "default" {
        return provider
            .base_url
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
            || provider.endpoints.contains_key(endpoint_id);
    }
    provider.endpoints.contains_key(endpoint_id)
}

fn provider_endpoint_enabled(provider: &ProviderConfigV4, endpoint_id: &str) -> bool {
    if endpoint_id == "default"
        && provider
            .base_url
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
    {
        return true;
    }
    provider
        .endpoints
        .get(endpoint_id)
        .is_some_and(|endpoint| endpoint.enabled)
}

fn expand_v4_route_leaves(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    expansion: &RouteExpansionContext<'_>,
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
                endpoint_id: None,
                route_path: vec![provider_id.clone()],
                preference_group: 0,
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
        expansion,
        &mut stack,
    )
}

fn expand_route_ref(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    child_name: &str,
    parent_path: &[String],
    expansion: &RouteExpansionContext<'_>,
    stack: &mut Vec<String>,
) -> Result<Vec<RouteLeaf>> {
    if view.providers.contains_key(child_name) {
        let mut route_path = parent_path.to_vec();
        route_path.push(child_name.to_string());
        return Ok(vec![RouteLeaf {
            provider_id: child_name.to_string(),
            endpoint_id: None,
            route_path,
            preference_group: 0,
        }]);
    }
    if routing.routes.contains_key(child_name) {
        return expand_route_node(
            service_name,
            view,
            routing,
            child_name,
            parent_path,
            expansion,
            stack,
        );
    }
    if let Some((provider_id, endpoint_id)) = split_provider_endpoint_ref(child_name)
        && view
            .providers
            .get(provider_id)
            .is_some_and(|provider| provider_endpoint_exists(provider, endpoint_id))
    {
        let mut route_path = parent_path.to_vec();
        route_path.push(child_name.to_string());
        return Ok(vec![RouteLeaf {
            provider_id: provider_id.to_string(),
            endpoint_id: Some(endpoint_id.to_string()),
            route_path,
            preference_group: 0,
        }]);
    }

    expand_route_node(
        service_name,
        view,
        routing,
        child_name,
        parent_path,
        expansion,
        stack,
    )
}

fn expand_route_node(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    route_name: &str,
    parent_path: &[String],
    expansion: &RouteExpansionContext<'_>,
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
    let frame = RouteExpansionFrame {
        route_name,
        node,
        node_path: &node_path,
    };
    let result = match node.strategy {
        RoutingPolicyV4::OrderedFailover => {
            expand_ordered_route_children(service_name, view, routing, &frame, expansion, stack)
        }
        RoutingPolicyV4::ManualSticky => {
            expand_manual_sticky_route(service_name, view, routing, &frame, expansion, stack)
        }
        RoutingPolicyV4::TagPreferred => {
            expand_tag_preferred_route(service_name, view, routing, &frame, expansion, stack)
        }
        RoutingPolicyV4::Conditional => {
            expand_conditional_route(service_name, view, routing, &frame, expansion, stack)
        }
    };
    stack.pop();
    result
}

fn expand_ordered_route_children(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    frame: &RouteExpansionFrame<'_>,
    expansion: &RouteExpansionContext<'_>,
    stack: &mut Vec<String>,
) -> Result<Vec<RouteLeaf>> {
    if frame.node.children.is_empty() {
        anyhow::bail!(
            "[{service_name}] ordered-failover route '{}' requires at least one child",
            frame.route_name
        );
    }

    let mut leaves = Vec::new();
    let mut next_preference_group = 0;
    for child_name in &frame.node.children {
        let child_leaves = expand_route_ref(
            service_name,
            view,
            routing,
            child_name.as_str(),
            frame.node_path,
            expansion,
            stack,
        )?;
        append_compacted_preference_groups(&mut leaves, &mut next_preference_group, child_leaves);
    }
    Ok(leaves)
}

fn expand_manual_sticky_route(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    frame: &RouteExpansionFrame<'_>,
    expansion: &RouteExpansionContext<'_>,
    stack: &mut Vec<String>,
) -> Result<Vec<RouteLeaf>> {
    let target = frame
        .node
        .target
        .as_deref()
        .or_else(|| frame.node.children.first().map(String::as_str))
        .with_context(|| {
            format!(
                "[{service_name}] manual-sticky route '{}' requires target",
                frame.route_name
            )
        })?;
    if let Some(provider) = view.providers.get(target)
        && !provider.enabled
    {
        anyhow::bail!(
            "[{service_name}] manual-sticky route '{}' targets disabled provider '{target}'",
            frame.route_name
        );
    }
    if !routing.routes.contains_key(target)
        && let Some((provider_id, endpoint_id)) = split_provider_endpoint_ref(target)
        && let Some(provider) = view.providers.get(provider_id)
        && (!provider.enabled || !provider_endpoint_enabled(provider, endpoint_id))
    {
        anyhow::bail!(
            "[{service_name}] manual-sticky route '{}' targets disabled provider endpoint '{target}'",
            frame.route_name
        );
    }

    expand_route_ref(
        service_name,
        view,
        routing,
        target,
        frame.node_path,
        expansion,
        stack,
    )
}

fn expand_tag_preferred_route(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    frame: &RouteExpansionFrame<'_>,
    expansion: &RouteExpansionContext<'_>,
    stack: &mut Vec<String>,
) -> Result<Vec<RouteLeaf>> {
    if frame.node.children.is_empty() {
        anyhow::bail!(
            "[{service_name}] tag-preferred route '{}' requires at least one child",
            frame.route_name
        );
    }
    if frame.node.prefer_tags.is_empty() {
        anyhow::bail!(
            "[{service_name}] tag-preferred route '{}' requires prefer_tags",
            frame.route_name
        );
    }

    let mut preferred = Vec::new();
    let mut fallback = Vec::new();
    let mut next_preferred_group = 0;
    let mut next_fallback_group = 0;
    for child_name in &frame.node.children {
        let child_leaves = expand_route_ref(
            service_name,
            view,
            routing,
            child_name.as_str(),
            frame.node_path,
            expansion,
            stack,
        )?;
        if child_route_matches_any_filter(view, &child_leaves, &frame.node.prefer_tags) {
            append_compacted_preference_groups(
                &mut preferred,
                &mut next_preferred_group,
                child_leaves,
            );
        } else {
            append_compacted_preference_groups(
                &mut fallback,
                &mut next_fallback_group,
                child_leaves,
            );
        }
    }

    if matches!(frame.node.on_exhausted, RoutingExhaustedActionV4::Stop) {
        if preferred.is_empty() {
            anyhow::bail!(
                "[{service_name}] tag-preferred route '{}' with on_exhausted = 'stop' matched no providers",
                frame.route_name
            );
        }
        return Ok(preferred);
    }

    offset_preference_groups(&mut fallback, next_preferred_group);
    preferred.extend(fallback);
    Ok(preferred)
}

fn append_compacted_preference_groups(
    out: &mut Vec<RouteLeaf>,
    next_group: &mut u32,
    mut leaves: Vec<RouteLeaf>,
) {
    let Some(min_group) = leaves.iter().map(|leaf| leaf.preference_group).min() else {
        return;
    };
    for leaf in &mut leaves {
        leaf.preference_group = leaf
            .preference_group
            .saturating_sub(min_group)
            .saturating_add(*next_group);
    }
    let max_group = leaves
        .iter()
        .map(|leaf| leaf.preference_group)
        .max()
        .unwrap_or(*next_group);
    *next_group = max_group.saturating_add(1);
    out.extend(leaves);
}

fn offset_preference_groups(leaves: &mut [RouteLeaf], offset: u32) {
    for leaf in leaves {
        leaf.preference_group = leaf.preference_group.saturating_add(offset);
    }
}

fn expand_conditional_route(
    service_name: &str,
    view: &ServiceViewV4,
    routing: &RoutingConfigV4,
    frame: &RouteExpansionFrame<'_>,
    expansion: &RouteExpansionContext<'_>,
    stack: &mut Vec<String>,
) -> Result<Vec<RouteLeaf>> {
    let condition = frame.node.when.as_ref().with_context(|| {
        format!(
            "[{service_name}] conditional route '{}' requires when",
            frame.route_name
        )
    })?;
    if condition.is_empty() {
        anyhow::bail!(
            "[{service_name}] conditional route '{}' requires at least one condition field",
            frame.route_name
        );
    }

    let then = frame.node.then.as_deref().with_context(|| {
        format!(
            "[{service_name}] conditional route '{}' requires then",
            frame.route_name
        )
    })?;
    let default_route = frame.node.default_route.as_deref().with_context(|| {
        format!(
            "[{service_name}] conditional route '{}' requires default",
            frame.route_name
        )
    })?;
    match expansion.conditional {
        ConditionalExpansion::MatchRequest => {
            let selected = if request_matches_condition(expansion.request, condition) {
                then
            } else {
                default_route
            };
            expand_route_ref(
                service_name,
                view,
                routing,
                selected,
                frame.node_path,
                expansion,
                stack,
            )
        }
        ConditionalExpansion::AllBranchesForCompatibility => {
            let mut leaves = Vec::new();
            leaves.extend(expand_route_ref(
                service_name,
                view,
                routing,
                then,
                frame.node_path,
                expansion,
                stack,
            )?);
            leaves.extend(expand_route_ref(
                service_name,
                view,
                routing,
                default_route,
                frame.node_path,
                expansion,
                stack,
            )?);
            dedupe_route_leaves_by_target(&mut leaves);
            Ok(leaves)
        }
    }
}

fn dedupe_route_leaves_by_target(leaves: &mut Vec<RouteLeaf>) {
    let mut seen = BTreeSet::new();
    leaves.retain(|leaf| seen.insert((leaf.provider_id.clone(), leaf.endpoint_id.clone())));
}

pub(crate) fn request_matches_condition(
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

fn ensure_unique_route_leaves(service_name: &str, leaves: &[RouteLeaf]) -> Result<()> {
    let mut provider_all_endpoint_refs = BTreeSet::new();
    let mut provider_endpoint_refs = BTreeSet::new();
    for leaf in leaves {
        match leaf.endpoint_id.as_deref() {
            Some(endpoint_id) => {
                if provider_all_endpoint_refs.contains(leaf.provider_id.as_str())
                    || !provider_endpoint_refs.insert((leaf.provider_id.as_str(), endpoint_id))
                {
                    anyhow::bail!(
                        "[{service_name}] routing graph expands provider endpoint '{}.{}' more than once; duplicate leaves are ambiguous",
                        leaf.provider_id,
                        endpoint_id
                    );
                }
            }
            None => {
                if provider_endpoint_refs
                    .iter()
                    .any(|(provider_id, _)| *provider_id == leaf.provider_id.as_str())
                    || !provider_all_endpoint_refs.insert(leaf.provider_id.as_str())
                {
                    anyhow::bail!(
                        "[{service_name}] routing graph expands provider '{}' more than once; duplicate leaves are ambiguous",
                        leaf.provider_id
                    );
                }
            }
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
            if leaf
                .endpoint_id
                .as_deref()
                .is_some_and(|endpoint_id| endpoint_id != endpoint.endpoint_id)
            {
                continue;
            }
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
                preference_group: leaf.preference_group,
                stable_index,
                compatibility_station_name: None,
                compatibility_upstream_index: None,
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
        .unwrap_or_else(|| V4_COMPATIBILITY_STATION_NAME.to_string())
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
        ProviderEndpointV4, ProxyConfigV4, RoutingAffinityPolicyV5, RoutingConditionV4,
        RoutingConfigV4, RoutingExhaustedActionV4, RoutingNodeV4, RoutingPolicyV4,
        compile_v4_to_runtime, resolved_v4_provider_order,
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

    fn provider_preference_groups(template: &RoutePlanTemplate) -> Vec<(String, u32)> {
        template
            .candidates
            .iter()
            .map(|candidate| (candidate.provider_id.clone(), candidate.preference_group))
            .collect()
    }

    fn endpoint_key(
        service_name: &str,
        provider_id: &str,
        endpoint_id: &str,
    ) -> ProviderEndpointKey {
        ProviderEndpointKey::new(service_name, provider_id, endpoint_id)
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
            .filter_map(|identity| {
                identity
                    .compatibility
                    .as_ref()
                    .map(LegacyUpstreamKey::stable_key)
            })
            .collect()
    }

    fn assert_provider_order_parity(view: &ServiceViewV4, template: &RoutePlanTemplate) {
        let resolved = resolved_v4_provider_order("routing-ir-test", view).expect("resolved order");
        assert_eq!(template.expanded_provider_order, resolved);
        assert_eq!(provider_ids(template), resolved);
    }

    #[test]
    fn route_graph_key_changes_when_route_rules_change() {
        let providers = BTreeMap::from([
            (
                "a".to_string(),
                ProviderConfigV4 {
                    base_url: Some("http://a.example/v1".to_string()),
                    tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                    ..ProviderConfigV4::default()
                },
            ),
            (
                "b".to_string(),
                ProviderConfigV4 {
                    base_url: Some("http://b.example/v1".to_string()),
                    tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                    ..ProviderConfigV4::default()
                },
            ),
        ]);
        let request = RouteRequestContext::default();
        let ordered = ServiceViewV4 {
            providers: providers.clone(),
            routing: Some(RoutingConfigV4 {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RoutingNodeV4 {
                        strategy: RoutingPolicyV4::OrderedFailover,
                        children: vec!["a".to_string(), "b".to_string()],
                        ..RoutingNodeV4::default()
                    },
                )]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        };
        let tag_preferred = ServiceViewV4 {
            providers,
            routing: Some(RoutingConfigV4 {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RoutingNodeV4 {
                        strategy: RoutingPolicyV4::TagPreferred,
                        children: vec!["a".to_string(), "b".to_string()],
                        prefer_tags: vec![BTreeMap::from([(
                            "billing".to_string(),
                            "monthly".to_string(),
                        )])],
                        on_exhausted: RoutingExhaustedActionV4::Continue,
                        ..RoutingNodeV4::default()
                    },
                )]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        };

        let ordered_template =
            compile_v4_route_plan_template_with_request("routing-ir-test", &ordered, &request)
                .expect("ordered template");
        let tag_preferred_template = compile_v4_route_plan_template_with_request(
            "routing-ir-test",
            &tag_preferred,
            &request,
        )
        .expect("tag-preferred template");

        assert_eq!(
            provider_ids(&ordered_template),
            provider_ids(&tag_preferred_template)
        );
        assert_ne!(
            ordered_template.route_graph_key(),
            tag_preferred_template.route_graph_key()
        );
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

    fn legacy_parity_tags(values: &HashMap<String, String>) -> BTreeMap<String, String> {
        values
            .iter()
            .filter(|(key, _)| {
                !matches!(
                    key.as_str(),
                    "endpoint_id" | "provider_endpoint_key" | "route_path" | "preference_group"
                )
            })
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
            tags: legacy_parity_tags(&selected.upstream.tags),
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
                        upstream: upstream_signature(
                            &executor.legacy_selected_upstream_for_candidate(skipped.candidate),
                        ),
                        avoid_for_station: skipped.avoided_candidate_indices,
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
                upstream: upstream_signature(
                    &executor.legacy_selected_upstream_for_candidate(selected.candidate),
                ),
                avoid_for_station: selection.avoided_candidate_indices,
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
    fn routing_ir_v4_candidate_identity_retains_provider_endpoint_without_synthetic_legacy_key() {
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
        assert!(identities[0].compatibility.is_none());
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
        assert_eq!(
            provider_preference_groups(&template),
            vec![
                ("input".to_string(), 0),
                ("input1".to_string(), 1),
                ("paygo".to_string(), 2),
            ]
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
        assert_eq!(
            provider_preference_groups(&template),
            vec![("monthly".to_string(), 0), ("paygo".to_string(), 1)]
        );
        assert_eq!(
            template.candidates[0]
                .to_upstream_config()
                .tags
                .get("preference_group")
                .map(String::as_str),
            Some("0")
        );
        assert_eq!(
            template.candidates[1]
                .to_upstream_config()
                .tags
                .get("preference_group")
                .map(String::as_str),
            Some("1")
        );
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
    fn routing_ir_tag_preferred_marks_nested_preference_groups() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "monthly-a".to_string(),
                    tagged_provider("https://monthly-a.example/v1", "billing", "monthly"),
                ),
                (
                    "monthly-b".to_string(),
                    tagged_provider("https://monthly-b.example/v1", "billing", "monthly"),
                ),
                (
                    "chili".to_string(),
                    tagged_provider("https://chili.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(RoutingConfigV4 {
                entry: "monthly_first".to_string(),
                routes: BTreeMap::from([
                    (
                        "monthly_pool".to_string(),
                        RoutingNodeV4 {
                            strategy: RoutingPolicyV4::OrderedFailover,
                            children: vec!["monthly-a".to_string(), "monthly-b".to_string()],
                            ..RoutingNodeV4::default()
                        },
                    ),
                    (
                        "monthly_first".to_string(),
                        RoutingNodeV4 {
                            strategy: RoutingPolicyV4::TagPreferred,
                            children: vec!["chili".to_string(), "monthly_pool".to_string()],
                            prefer_tags: vec![BTreeMap::from([(
                                "billing".to_string(),
                                "monthly".to_string(),
                            )])],
                            on_exhausted: RoutingExhaustedActionV4::Continue,
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
        assert_eq!(
            provider_ids(&template),
            vec!["monthly-a", "monthly-b", "chili"]
        );
        assert_eq!(
            provider_preference_groups(&template),
            vec![
                ("monthly-a".to_string(), 0),
                ("monthly-b".to_string(), 1),
                ("chili".to_string(), 2),
            ]
        );
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
    fn routing_ir_conditional_route_flattens_only_for_compat_runtime_path() {
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

        let runtime = compile_v4_to_runtime(&cfg).expect("compat runtime");
        let routing = runtime
            .codex
            .station("routing")
            .expect("compat routing station");

        assert_eq!(
            routing
                .upstreams
                .iter()
                .map(|upstream| upstream
                    .tags
                    .get("provider_id")
                    .map(String::as_str)
                    .unwrap_or(""))
                .collect::<Vec<_>>(),
            vec!["large", "small"]
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
        assert!(legacy_upstream_keys(&template).is_empty());
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
    fn routing_ir_manual_sticky_can_target_provider_endpoint() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfigV4 {
                    endpoints: BTreeMap::from([
                        (
                            "fast".to_string(),
                            ProviderEndpointV4 {
                                base_url: "https://fast.example/v1".to_string(),
                                enabled: true,
                                priority: 0,
                                tags: BTreeMap::new(),
                                supported_models: BTreeMap::new(),
                                model_mapping: BTreeMap::new(),
                            },
                        ),
                        (
                            "slow".to_string(),
                            ProviderEndpointV4 {
                                base_url: "https://slow.example/v1".to_string(),
                                enabled: true,
                                priority: 10,
                                tags: BTreeMap::new(),
                                supported_models: BTreeMap::new(),
                                model_mapping: BTreeMap::new(),
                            },
                        ),
                    ]),
                    ..ProviderConfigV4::default()
                },
            )]),
            routing: Some(RoutingConfigV4 {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RoutingNodeV4 {
                        strategy: RoutingPolicyV4::ManualSticky,
                        target: Some("input.slow".to_string()),
                        children: vec!["input".to_string()],
                        ..RoutingNodeV4::default()
                    },
                )]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        };

        let template = compile_v4_route_plan_template("codex", &view).expect("route template");

        assert_eq!(provider_ids(&template), vec!["input"]);
        assert_eq!(template.candidates[0].endpoint_id, "slow");
        assert_eq!(template.candidates[0].base_url, "https://slow.example/v1");
        assert_eq!(provider_endpoint_keys(&template), vec!["codex/input/slow"]);
        assert_eq!(
            template.candidates[0].route_path,
            vec!["root".to_string(), "input.slow".to_string()]
        );
    }

    #[test]
    fn routing_ir_manual_sticky_rejects_disabled_provider_endpoint_target() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfigV4 {
                    endpoints: BTreeMap::from([(
                        "fast".to_string(),
                        ProviderEndpointV4 {
                            base_url: "https://fast.example/v1".to_string(),
                            enabled: false,
                            priority: 0,
                            tags: BTreeMap::new(),
                            supported_models: BTreeMap::new(),
                            model_mapping: BTreeMap::new(),
                        },
                    )]),
                    ..ProviderConfigV4::default()
                },
            )]),
            routing: Some(RoutingConfigV4 {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RoutingNodeV4 {
                        strategy: RoutingPolicyV4::ManualSticky,
                        target: Some("input.fast".to_string()),
                        children: vec!["input".to_string()],
                        ..RoutingNodeV4::default()
                    },
                )]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        };

        let error = compile_v4_route_plan_template("codex", &view).expect_err("disabled endpoint");

        assert!(
            error
                .to_string()
                .contains("targets disabled provider endpoint 'input.fast'")
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
        assert_eq!(
            identities[0]
                .compatibility
                .as_ref()
                .map(LegacyUpstreamKey::stable_key)
                .as_deref(),
            Some("codex/legacy-station/0")
        );
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
        assert_eq!(selection.avoided_candidate_indices, vec![0, 1]);
        assert!(state.route_candidates_exhausted(&template));
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
        runtime.set_provider_endpoint(
            endpoint_key("codex", "disabled", "default"),
            RoutePlanUpstreamRuntimeState {
                runtime_disabled: true,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        runtime.set_provider_endpoint(
            endpoint_key("codex", "cooldown", "default"),
            RoutePlanUpstreamRuntimeState {
                cooldown_active: true,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        runtime.set_provider_endpoint(
            endpoint_key("codex", "breaker", "default"),
            RoutePlanUpstreamRuntimeState {
                failure_count: FAILURE_THRESHOLD,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        runtime.set_provider_endpoint(
            endpoint_key("codex", "usage", "default"),
            RoutePlanUpstreamRuntimeState {
                usage_exhausted: true,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        runtime.set_provider_endpoint(
            endpoint_key("codex", "missing-auth", "default"),
            RoutePlanUpstreamRuntimeState {
                missing_auth: true,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );

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
    fn route_plan_executor_keeps_fallback_last_good_inside_lower_preference_group() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    tagged_provider("https://monthly.example/v1", "billing", "monthly"),
                ),
                (
                    "chili".to_string(),
                    tagged_provider("https://chili.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(RoutingConfigV4::tag_preferred(
                vec!["chili".to_string(), "monthly".to_string()],
                vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                RoutingExhaustedActionV4::Continue,
            )),
            ..ServiceViewV4::default()
        };
        let template = compile_v4_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_affinity_provider_endpoint(Some(endpoint_key("codex", "chili", "default")));
        let mut state = RoutePlanAttemptState::default();

        let selection =
            executor.select_supported_candidate_with_runtime_state(&mut state, &runtime, None);
        let selected = selection.selected.expect("preferred candidate selected");

        assert_eq!(
            provider_preference_groups(&template),
            vec![("monthly".to_string(), 0), ("chili".to_string(), 1),]
        );
        assert_eq!(selected.candidate.provider_id, "monthly");
        assert_eq!(
            selected.provider_endpoint,
            endpoint_key("codex", "monthly", "default")
        );
    }

    #[test]
    fn route_plan_executor_prefers_ordered_primary_over_lower_order_affinity() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    tagged_provider("https://monthly.example/v1", "billing", "monthly"),
                ),
                (
                    "chili".to_string(),
                    tagged_provider("https://chili.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "monthly".to_string(),
                "chili".to_string(),
            ])),
            ..ServiceViewV4::default()
        };
        let template = compile_v4_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_affinity_provider_endpoint(Some(endpoint_key("codex", "chili", "default")));
        let mut state = RoutePlanAttemptState::default();

        let selection =
            executor.select_supported_candidate_with_runtime_state(&mut state, &runtime, None);
        let selected = selection.selected.expect("ordered primary selected");

        assert_eq!(
            provider_preference_groups(&template),
            vec![("monthly".to_string(), 0), ("chili".to_string(), 1)]
        );
        assert_eq!(selected.candidate.provider_id, "monthly");
        assert_eq!(
            selected.provider_endpoint,
            endpoint_key("codex", "monthly", "default")
        );
    }

    #[test]
    fn route_plan_executor_uses_fallback_group_only_when_preferred_usage_exhausted() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    tagged_provider("https://monthly.example/v1", "billing", "monthly"),
                ),
                (
                    "chili".to_string(),
                    tagged_provider("https://chili.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(RoutingConfigV4::tag_preferred(
                vec!["chili".to_string(), "monthly".to_string()],
                vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                RoutingExhaustedActionV4::Continue,
            )),
            ..ServiceViewV4::default()
        };
        let template = compile_v4_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_affinity_provider_endpoint(Some(endpoint_key("codex", "chili", "default")));
        runtime.set_provider_endpoint(
            endpoint_key("codex", "monthly", "default"),
            RoutePlanUpstreamRuntimeState {
                usage_exhausted: true,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        let mut state = RoutePlanAttemptState::default();

        let selection =
            executor.select_supported_candidate_with_runtime_state(&mut state, &runtime, None);
        let selected = selection.selected.expect("fallback candidate selected");

        assert_eq!(selected.candidate.provider_id, "chili");
        assert_eq!(
            selected.provider_endpoint,
            endpoint_key("codex", "chili", "default")
        );
    }

    #[test]
    fn route_plan_executor_uses_fallback_group_when_preferred_is_hard_unavailable() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    tagged_provider("https://monthly.example/v1", "billing", "monthly"),
                ),
                (
                    "chili".to_string(),
                    tagged_provider("https://chili.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(RoutingConfigV4::tag_preferred(
                vec!["chili".to_string(), "monthly".to_string()],
                vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                RoutingExhaustedActionV4::Continue,
            )),
            ..ServiceViewV4::default()
        };
        let template = compile_v4_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_affinity_provider_endpoint(Some(endpoint_key("codex", "chili", "default")));
        runtime.set_provider_endpoint(
            endpoint_key("codex", "monthly", "default"),
            RoutePlanUpstreamRuntimeState {
                runtime_disabled: true,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        let mut state = RoutePlanAttemptState::default();

        let selection =
            executor.select_supported_candidate_with_runtime_state(&mut state, &runtime, None);
        let selected = selection.selected.expect("fallback candidate selected");

        assert_eq!(selected.candidate.provider_id, "chili");
        assert_eq!(
            selected.provider_endpoint,
            endpoint_key("codex", "chili", "default")
        );
    }

    #[test]
    fn route_plan_executor_fallback_sticky_policy_can_keep_lower_preference_affinity() {
        let mut routing = RoutingConfigV4::tag_preferred(
            vec!["chili".to_string(), "monthly".to_string()],
            vec![BTreeMap::from([(
                "billing".to_string(),
                "monthly".to_string(),
            )])],
            RoutingExhaustedActionV4::Continue,
        );
        routing.affinity_policy = RoutingAffinityPolicyV5::FallbackSticky;
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    tagged_provider("https://monthly.example/v1", "billing", "monthly"),
                ),
                (
                    "chili".to_string(),
                    tagged_provider("https://chili.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        };
        let template = compile_v4_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_affinity_provider_endpoint(Some(endpoint_key("codex", "chili", "default")));
        let mut state = RoutePlanAttemptState::default();

        let selection =
            executor.select_supported_candidate_with_runtime_state(&mut state, &runtime, None);
        let selected = selection
            .selected
            .expect("fallback affinity candidate selected");

        assert_eq!(
            template.affinity_policy,
            RoutingAffinityPolicyV5::FallbackSticky
        );
        assert_eq!(selected.candidate.provider_id, "chili");
        assert_eq!(
            selected.provider_endpoint,
            endpoint_key("codex", "chili", "default")
        );
    }

    #[test]
    fn route_plan_executor_fallback_sticky_reprobes_preferred_after_window() {
        let mut routing = RoutingConfigV4::tag_preferred(
            vec!["chili".to_string(), "monthly".to_string()],
            vec![BTreeMap::from([(
                "billing".to_string(),
                "monthly".to_string(),
            )])],
            RoutingExhaustedActionV4::Continue,
        );
        routing.affinity_policy = RoutingAffinityPolicyV5::FallbackSticky;
        routing.reprobe_preferred_after_ms = Some(1);
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    tagged_provider("https://monthly.example/v1", "billing", "monthly"),
                ),
                (
                    "chili".to_string(),
                    tagged_provider("https://chili.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        };
        let template = compile_v4_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let now = crate::logging::now_ms();
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_affinity_provider_endpoint_with_observed_at(
            Some(endpoint_key("codex", "chili", "default")),
            Some(now),
            Some(now.saturating_sub(2)),
        );
        let mut state = RoutePlanAttemptState::default();

        let selection =
            executor.select_supported_candidate_with_runtime_state(&mut state, &runtime, None);
        let selected = selection
            .selected
            .expect("preferred candidate selected after reprobe window");

        assert_eq!(selected.candidate.provider_id, "monthly");
    }

    #[test]
    fn route_plan_executor_fallback_sticky_reprobes_preferred_after_fallback_ttl() {
        let mut routing = RoutingConfigV4::tag_preferred(
            vec!["chili".to_string(), "monthly".to_string()],
            vec![BTreeMap::from([(
                "billing".to_string(),
                "monthly".to_string(),
            )])],
            RoutingExhaustedActionV4::Continue,
        );
        routing.affinity_policy = RoutingAffinityPolicyV5::FallbackSticky;
        routing.fallback_ttl_ms = Some(1);
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    tagged_provider("https://monthly.example/v1", "billing", "monthly"),
                ),
                (
                    "chili".to_string(),
                    tagged_provider("https://chili.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        };
        let template = compile_v4_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let now = crate::logging::now_ms();
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_affinity_provider_endpoint_with_observed_at(
            Some(endpoint_key("codex", "chili", "default")),
            Some(now),
            Some(now.saturating_sub(2)),
        );
        let mut state = RoutePlanAttemptState::default();

        let selection =
            executor.select_supported_candidate_with_runtime_state(&mut state, &runtime, None);
        let selected = selection
            .selected
            .expect("preferred candidate selected after fallback ttl");

        assert_eq!(selected.candidate.provider_id, "monthly");
    }

    #[test]
    fn route_plan_executor_off_policy_ignores_affinity_inside_best_group() {
        let mut routing = RoutingConfigV4::tag_preferred(
            vec!["monthly-a".to_string(), "monthly-b".to_string()],
            vec![BTreeMap::from([(
                "billing".to_string(),
                "monthly".to_string(),
            )])],
            RoutingExhaustedActionV4::Continue,
        );
        routing.affinity_policy = RoutingAffinityPolicyV5::Off;
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "monthly-a".to_string(),
                    tagged_provider("https://monthly-a.example/v1", "billing", "monthly"),
                ),
                (
                    "monthly-b".to_string(),
                    tagged_provider("https://monthly-b.example/v1", "billing", "monthly"),
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        };
        let template = compile_v4_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_affinity_provider_endpoint(Some(endpoint_key("codex", "monthly-b", "default")));
        let mut state = RoutePlanAttemptState::default();

        let selection =
            executor.select_supported_candidate_with_runtime_state(&mut state, &runtime, None);
        let selected = selection.selected.expect("first candidate selected");

        assert_eq!(template.affinity_policy, RoutingAffinityPolicyV5::Off);
        assert_eq!(selected.candidate.provider_id, "monthly-a");
        assert_eq!(
            selected.provider_endpoint,
            endpoint_key("codex", "monthly-a", "default")
        );
    }

    #[test]
    fn route_plan_executor_hard_policy_stops_when_affinity_is_unavailable() {
        let mut routing = RoutingConfigV4::tag_preferred(
            vec!["monthly".to_string(), "chili".to_string()],
            vec![BTreeMap::from([(
                "billing".to_string(),
                "monthly".to_string(),
            )])],
            RoutingExhaustedActionV4::Continue,
        );
        routing.affinity_policy = RoutingAffinityPolicyV5::Hard;
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "monthly".to_string(),
                    tagged_provider("https://monthly.example/v1", "billing", "monthly"),
                ),
                (
                    "chili".to_string(),
                    tagged_provider("https://chili.example/v1", "billing", "paygo"),
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        };
        let template = compile_v4_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_affinity_provider_endpoint(Some(endpoint_key("codex", "chili", "default")));
        runtime.set_provider_endpoint(
            endpoint_key("codex", "chili", "default"),
            RoutePlanUpstreamRuntimeState {
                runtime_disabled: true,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        let mut state = RoutePlanAttemptState::default();

        let selection =
            executor.select_supported_candidate_with_runtime_state(&mut state, &runtime, None);

        assert_eq!(template.affinity_policy, RoutingAffinityPolicyV5::Hard);
        assert!(selection.selected.is_none());
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
        assert_eq!(first_selected.candidate.provider_id, "first");

        let same = executor.select_supported_candidate(&mut state, None);
        let same_selected = same.selected.as_ref().expect("same selected");
        assert_eq!(same_selected.candidate.provider_id, "first");

        assert!(state.avoid_selected(same_selected));
        let next = executor.select_supported_candidate(&mut state, None);
        let next_selected = next.selected.as_ref().expect("next selected");

        assert_eq!(next_selected.candidate.provider_id, "second");
        assert_eq!(next.avoided_candidate_indices, vec![0]);
    }
}

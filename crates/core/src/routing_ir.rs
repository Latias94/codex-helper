use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::config::{
    ProviderConcurrencyLimits, ProviderConfig, RouteAffinityPolicy, RouteCondition,
    RouteExhaustedAction, RouteGraphConfig, RouteNodeConfig, RouteStrategy, SchedulingPreset,
    ServiceRouteConfig, UpstreamAuth, effective_routing,
};
use crate::endpoint_health::FAILURE_THRESHOLD;
use crate::model_routing;
use crate::runtime_identity::{ContinuityDomainKey, ProviderEndpointKey, RuntimeUpstreamIdentity};

#[derive(Debug, Clone)]
pub struct RoutePlanTemplate {
    pub service_name: String,
    pub entry: String,
    pub affinity_policy: RouteAffinityPolicy,
    pub scheduling_preset: SchedulingPreset,
    pub fallback_ttl_ms: Option<u64>,
    pub reprobe_preferred_after_ms: Option<u64>,
    pub nodes: BTreeMap<String, RouteNodePlan>,
    pub expanded_provider_order: Vec<String>,
    pub candidates: Vec<RouteCandidate>,
}

impl RoutePlanTemplate {
    pub fn route_graph_key(&self) -> String {
        let mut digest = StableRouteDigest::with_namespace("codex-helper:route-affinity:v1");
        encode_affinity_route_identity(&mut digest, self);
        format!("route:v1:{}", digest.finish())
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

    pub fn candidate_continuity_domain_key(
        &self,
        candidate: &RouteCandidate,
    ) -> ContinuityDomainKey {
        let provider_endpoint = candidate_provider_endpoint_key(self, candidate);
        candidate
            .continuity_domain
            .as_ref()
            .and_then(|domain| ContinuityDomainKey::explicit(self.service_name.clone(), domain))
            .unwrap_or_else(|| ContinuityDomainKey::provider_endpoint(provider_endpoint))
    }

    pub fn candidate_identity(&self, candidate: &RouteCandidate) -> RuntimeUpstreamIdentity {
        RuntimeUpstreamIdentity::new_with_auth(
            candidate_provider_endpoint_key(self, candidate),
            candidate.base_url.clone(),
            candidate.continuity_domain.clone(),
            &candidate.auth,
        )
    }

    pub fn candidate_identities(&self) -> Vec<RuntimeUpstreamIdentity> {
        self.candidates
            .iter()
            .map(|candidate| self.candidate_identity(candidate))
            .collect()
    }

    pub(crate) fn capture_candidate(&self, candidate: &RouteCandidate) -> CapturedRouteCandidate {
        CapturedRouteCandidate::from_candidate(self.service_name.as_str(), candidate)
    }

    pub fn continuity_topology(&self) -> RoutePlanContinuityTopology<'_> {
        RoutePlanContinuityTopology { template: self }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RoutePlanContinuityTopology<'a> {
    template: &'a RoutePlanTemplate,
}

impl RoutePlanContinuityTopology<'_> {
    pub fn configured_provider_endpoint_count(&self) -> usize {
        self.template
            .candidates
            .iter()
            .map(|candidate| self.template.candidate_provider_endpoint_key(candidate))
            .collect::<BTreeSet<_>>()
            .len()
    }

    pub fn candidate_domain(&self, candidate: &RouteCandidate) -> ContinuityDomainKey {
        self.template.candidate_continuity_domain_key(candidate)
    }

    pub fn find_candidate_by_provider_endpoint_stable_key(
        &self,
        provider_endpoint_stable_key: &str,
    ) -> Option<&RouteCandidate> {
        self.template.candidates.iter().find(|candidate| {
            self.template
                .candidate_provider_endpoint_key(candidate)
                .stable_key()
                == provider_endpoint_stable_key
        })
    }

    pub fn find_candidate_by_provider_endpoint(
        &self,
        provider_endpoint: &ProviderEndpointKey,
    ) -> Option<&RouteCandidate> {
        self.template.candidates.iter().find(|candidate| {
            self.template.candidate_provider_endpoint_key(candidate) == *provider_endpoint
        })
    }

    pub fn same_domain_provider_endpoint_count(&self, domain: &ContinuityDomainKey) -> usize {
        self.template
            .candidates
            .iter()
            .filter(|candidate| self.template.candidate_continuity_domain_key(candidate) == *domain)
            .map(|candidate| self.template.candidate_provider_endpoint_key(candidate))
            .collect::<BTreeSet<_>>()
            .len()
    }

    pub fn selected_domain_summary(
        &self,
        provider_endpoint_stable_key: &str,
    ) -> Option<RoutePlanContinuityDomainSummary> {
        let selected =
            self.find_candidate_by_provider_endpoint_stable_key(provider_endpoint_stable_key)?;
        let domain = self.candidate_domain(selected);
        Some(RoutePlanContinuityDomainSummary {
            domain: domain.clone(),
            same_domain_endpoint_count: self.same_domain_provider_endpoint_count(&domain).max(1),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutePlanContinuityDomainSummary {
    pub domain: ContinuityDomainKey,
    pub same_domain_endpoint_count: usize,
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
    pub strategy: RouteStrategy,
    pub children: Vec<RouteRef>,
    pub target: Option<RouteRef>,
    pub prefer_tags: Vec<BTreeMap<String, String>>,
    pub on_exhausted: RouteExhaustedAction,
    pub metadata: BTreeMap<String, String>,
    pub when: Option<RouteCondition>,
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
    pub continuity_domain: Option<String>,
    pub auth: UpstreamAuth,
    pub tags: BTreeMap<String, String>,
    pub supported_models: BTreeMap<String, bool>,
    pub model_mapping: BTreeMap<String, String>,
    pub(crate) model_rules: Arc<model_routing::CompiledModelRules>,
    pub route_path: Vec<String>,
    pub preference_group: u32,
    pub stable_index: usize,
    pub concurrency: RouteCandidateConcurrency,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct RouteCandidateConcurrency {
    pub max_concurrent_requests: Option<u32>,
    pub limit_group: Option<String>,
}

impl RouteCandidateConcurrency {
    pub fn is_limited(&self) -> bool {
        self.max_concurrent_requests.is_some()
    }

    pub fn limit_key(
        &self,
        service_name: &str,
        provider_endpoint: &ProviderEndpointKey,
    ) -> Option<String> {
        self.max_concurrent_requests?;
        if let Some(scope) = self
            .limit_group
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(format!("group:{service_name}/{scope}"));
        }
        Some(format!("endpoint:{}", provider_endpoint.stable_key()))
    }
}

impl RouteCandidate {
    pub fn effective_model(&self, requested_model: &str) -> String {
        self.model_rules.effective_model(requested_model)
    }

    pub fn is_model_supported(&self, requested_model: &str) -> bool {
        self.model_rules.is_model_supported(requested_model)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CapturedRouteCandidate {
    candidate: Arc<RouteCandidate>,
    provider_endpoint: ProviderEndpointKey,
    continuity_domain: ContinuityDomainKey,
}

impl CapturedRouteCandidate {
    fn from_candidate(service_name: &str, candidate: &RouteCandidate) -> Self {
        let provider_endpoint = ProviderEndpointKey::new(
            service_name,
            candidate.provider_id.clone(),
            candidate.endpoint_id.clone(),
        );
        let continuity_domain = candidate
            .continuity_domain
            .as_ref()
            .and_then(|domain| ContinuityDomainKey::explicit(service_name, domain))
            .unwrap_or_else(|| ContinuityDomainKey::provider_endpoint(provider_endpoint.clone()));

        Self {
            candidate: Arc::new(candidate.clone()),
            provider_endpoint,
            continuity_domain,
        }
    }

    #[cfg(test)]
    pub(crate) fn capture_for_service(service_name: &str, candidate: &RouteCandidate) -> Self {
        Self::from_candidate(service_name, candidate)
    }

    pub(crate) fn candidate(&self) -> &RouteCandidate {
        self.candidate.as_ref()
    }

    pub(crate) fn base_url(&self) -> &str {
        self.candidate.base_url.as_str()
    }

    pub(crate) fn auth(&self) -> &UpstreamAuth {
        &self.candidate.auth
    }

    pub(crate) fn effective_model(&self, requested_model: &str) -> String {
        self.candidate.effective_model(requested_model)
    }

    pub(crate) fn is_model_supported(&self, requested_model: &str) -> bool {
        self.candidate.is_model_supported(requested_model)
    }

    pub(crate) fn log_target_label(&self) -> String {
        self.provider_endpoint.stable_key()
    }

    pub(crate) fn attempt_avoid_index(&self) -> usize {
        self.candidate.stable_index
    }

    pub(crate) fn provider_id(&self) -> &str {
        self.provider_endpoint.provider_id.as_str()
    }

    pub(crate) fn endpoint_id(&self) -> &str {
        self.provider_endpoint.endpoint_id.as_str()
    }

    pub(crate) fn provider_endpoint(&self) -> &ProviderEndpointKey {
        &self.provider_endpoint
    }

    pub(crate) fn continuity_domain(&self) -> &ContinuityDomainKey {
        &self.continuity_domain
    }

    pub(crate) fn provider_endpoint_key(&self) -> String {
        self.provider_endpoint.stable_key()
    }

    pub(crate) fn preference_group(&self) -> u32 {
        self.candidate.preference_group
    }

    pub(crate) fn route_path(&self) -> &[String] {
        &self.candidate.route_path
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
    allowed_continuity_domain: Option<ContinuityDomainKey>,
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

    pub fn restrict_to_continuity_domain(&mut self, continuity_domain: ContinuityDomainKey) {
        self.allowed_continuity_domain = Some(continuity_domain);
    }

    pub fn allowed_continuity_domain(&self) -> Option<&ContinuityDomainKey> {
        self.allowed_continuity_domain.as_ref()
    }

    pub fn allows_explicit_continuity_domain_failover(&self) -> bool {
        self.allowed_continuity_domain
            .as_ref()
            .is_some_and(ContinuityDomainKey::is_explicit)
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
            || self
                .allowed_continuity_domain
                .as_ref()
                .is_some_and(|domain| {
                    template.candidate_continuity_domain_key(candidate) != *domain
                })
    }

    pub fn avoid_selected(&mut self, selected: &SelectedRouteCandidate<'_>) -> bool {
        self.avoid_provider_endpoint(selected.provider_endpoint.clone())
    }

    pub fn avoided_total(&self) -> usize {
        self.avoided_total
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
    new_session_preference: Option<ProviderEndpointKey>,
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

    pub fn set_new_session_preference(&mut self, key: Option<ProviderEndpointKey>) {
        self.new_session_preference = key;
    }

    pub fn new_session_preference(&self) -> Option<&ProviderEndpointKey> {
        self.new_session_preference.as_ref()
    }

    fn runtime_state_for_candidate(
        &self,
        template: &RoutePlanTemplate,
        candidate: &RouteCandidate,
    ) -> RoutePlanUpstreamRuntimeState {
        self.provider_endpoint(&candidate_provider_endpoint_key(template, candidate))
    }

    pub fn candidate_runtime_snapshot(
        &self,
        template: &RoutePlanTemplate,
        candidate: &RouteCandidate,
    ) -> RoutePlanCandidateRuntimeSnapshot {
        RoutePlanCandidateRuntimeSnapshot::from_candidate_runtime(
            candidate,
            self.runtime_state_for_candidate(template, candidate),
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoutePlanUpstreamRuntimeState {
    pub runtime_disabled: bool,
    pub draining: bool,
    pub failure_count: u32,
    pub cooldown_active: bool,
    pub cooldown_remaining_secs: Option<u64>,
    pub usage_exhausted: bool,
    pub missing_auth: bool,
    pub concurrency_saturated: bool,
    pub concurrency_active: Option<u32>,
    pub concurrency_limit: Option<u32>,
}

impl RoutePlanUpstreamRuntimeState {
    fn breaker_open(self) -> bool {
        self.cooldown_active || self.failure_count >= FAILURE_THRESHOLD
    }

    fn hard_unavailable(self) -> bool {
        self.runtime_disabled || self.missing_auth || self.breaker_open()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutePlanCandidateRuntimeSnapshot {
    pub runtime_available: bool,
    pub affinity_runtime_available: bool,
    pub routable_except_usage: bool,
    pub hard_unavailable: bool,
    pub runtime_disabled: bool,
    pub draining: bool,
    pub cooldown_active: bool,
    pub cooldown_remaining_secs: Option<u64>,
    pub breaker_open: bool,
    pub failure_count: u32,
    pub usage_exhausted: bool,
    pub missing_auth: bool,
    pub concurrency_saturated: bool,
    pub concurrency_active: Option<u32>,
    pub concurrency_limit: Option<u32>,
    pub effective_max_concurrent_requests: Option<u32>,
    pub effective_limit_group: Option<String>,
}

impl RoutePlanCandidateRuntimeSnapshot {
    fn from_candidate_runtime(
        candidate: &RouteCandidate,
        runtime_state: RoutePlanUpstreamRuntimeState,
    ) -> Self {
        let breaker_open = runtime_state.breaker_open();
        let hard_unavailable = runtime_state.hard_unavailable();
        let affinity_routable_except_usage =
            !hard_unavailable && !runtime_state.concurrency_saturated;
        let routable_except_usage = affinity_routable_except_usage && !runtime_state.draining;
        let runtime_available = routable_except_usage && !runtime_state.usage_exhausted;
        let affinity_runtime_available =
            affinity_routable_except_usage && !runtime_state.usage_exhausted;

        Self {
            runtime_available,
            affinity_runtime_available,
            routable_except_usage,
            hard_unavailable,
            runtime_disabled: runtime_state.runtime_disabled,
            draining: runtime_state.draining,
            cooldown_active: runtime_state.cooldown_active,
            cooldown_remaining_secs: runtime_state.cooldown_remaining_secs,
            breaker_open,
            failure_count: runtime_state.failure_count,
            usage_exhausted: runtime_state.usage_exhausted,
            missing_auth: runtime_state.missing_auth,
            concurrency_saturated: runtime_state.concurrency_saturated,
            concurrency_active: runtime_state.concurrency_active,
            concurrency_limit: runtime_state.concurrency_limit,
            effective_max_concurrent_requests: candidate.concurrency.max_concurrent_requests,
            effective_limit_group: candidate.concurrency.limit_group.clone(),
        }
    }

    pub fn skip_reasons_for_candidate(
        &self,
        candidate: &RouteCandidate,
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
        reasons.extend(self.runtime_skip_reasons());
        reasons
    }

    pub fn runtime_skip_reasons(&self) -> Vec<RoutePlanSkipReason> {
        let mut reasons = Vec::new();
        if self.runtime_disabled {
            reasons.push(RoutePlanSkipReason::RuntimeDisabled);
        }
        if self.draining {
            reasons.push(RoutePlanSkipReason::Draining);
        }
        if self.cooldown_active {
            reasons.push(RoutePlanSkipReason::Cooldown);
        } else if self.failure_count >= FAILURE_THRESHOLD {
            reasons.push(RoutePlanSkipReason::BreakerOpen {
                failure_count: self.failure_count,
            });
        }
        if self.usage_exhausted {
            reasons.push(RoutePlanSkipReason::UsageExhausted);
        }
        if self.missing_auth {
            reasons.push(RoutePlanSkipReason::MissingAuth);
        }
        if self.concurrency_saturated {
            reasons.push(RoutePlanSkipReason::ConcurrencySaturated {
                active: self.concurrency_active,
                limit: self.concurrency_limit,
            });
        }
        reasons
    }

    pub fn dominant_runtime_skip_reason(&self) -> Option<RoutePlanSkipReason> {
        self.runtime_skip_reasons().into_iter().next()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutePlanSkipReason {
    UnsupportedModel {
        requested_model: String,
    },
    RuntimeDisabled,
    Draining,
    Cooldown,
    BreakerOpen {
        failure_count: u32,
    },
    UsageExhausted,
    MissingAuth,
    ConcurrencySaturated {
        active: Option<u32>,
        limit: Option<u32>,
    },
}

impl RoutePlanSkipReason {
    pub fn code(&self) -> &'static str {
        match self {
            RoutePlanSkipReason::UnsupportedModel { .. } => "unsupported_model",
            RoutePlanSkipReason::RuntimeDisabled => "runtime_disabled",
            RoutePlanSkipReason::Draining => "draining",
            RoutePlanSkipReason::Cooldown => "cooldown",
            RoutePlanSkipReason::BreakerOpen { .. } => "breaker_open",
            RoutePlanSkipReason::UsageExhausted => "usage_exhausted",
            RoutePlanSkipReason::MissingAuth => "missing_auth",
            RoutePlanSkipReason::ConcurrencySaturated { .. } => "concurrency_saturated",
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
pub struct RoutePlanAttemptSelection<'a> {
    pub selected: Option<SelectedRouteCandidate<'a>>,
    pub skipped: Vec<SkippedRouteCandidate<'a>>,
    pub avoided_candidate_indices: Vec<usize>,
    pub avoided_total: usize,
    pub total_upstreams: usize,
}

pub struct RoutePlanExecutor<'a> {
    template: &'a RoutePlanTemplate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoutePlanAffinitySelectionMode {
    Configured,
    SoftSessionPreferred,
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
                let snapshot = runtime.candidate_runtime_snapshot(self.template, candidate);
                let reasons = snapshot.skip_reasons_for_candidate(candidate, request_model);
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
        self.select_supported_candidate_with_affinity_mode(
            state,
            runtime,
            request_model,
            RoutePlanAffinitySelectionMode::Configured,
        )
    }

    pub fn select_supported_candidate_with_soft_affinity_runtime_state(
        &self,
        state: &mut RoutePlanAttemptState,
        runtime: &RoutePlanRuntimeState,
        request_model: Option<&str>,
    ) -> RoutePlanAttemptSelection<'_> {
        self.select_supported_candidate_with_affinity_mode(
            state,
            runtime,
            request_model,
            RoutePlanAffinitySelectionMode::SoftSessionPreferred,
        )
    }

    /// Revalidate an already selected candidate after waiting for a local
    /// concurrency permit without running the scheduler a second time.
    ///
    /// Re-selection would advance the process-wide weighted round-robin cursor
    /// and make admission itself change the distribution. The permit holder is
    /// therefore checked directly against the refreshed runtime snapshot.
    pub fn candidate_is_valid_after_runtime_update(
        &self,
        state: &RoutePlanAttemptState,
        runtime: &RoutePlanRuntimeState,
        candidate: &RouteCandidate,
        request_model: Option<&str>,
        affinity_policy: RouteAffinityPolicy,
    ) -> bool {
        let available = self
            .template
            .candidates
            .iter()
            .filter(|candidate| {
                !state.avoids_candidate(self.template, candidate)
                    && request_model.is_none_or(|model| candidate_supports_model(candidate, model))
                    && candidate_available_for_selection(self.template, runtime, candidate)
            })
            .collect::<Vec<_>>();
        if !available.iter().any(|available| {
            candidate_provider_endpoint_key(self.template, available)
                == candidate_provider_endpoint_key(self.template, candidate)
        }) {
            return false;
        }

        let best_group = available
            .iter()
            .map(|candidate| candidate.preference_group)
            .min();
        let matches_candidate = |other: &RouteCandidate| {
            candidate_provider_endpoint_key(self.template, other)
                == candidate_provider_endpoint_key(self.template, candidate)
        };

        match affinity_policy {
            RouteAffinityPolicy::Off => best_group == Some(candidate.preference_group),
            RouteAffinityPolicy::PreferredGroup => {
                best_group == Some(candidate.preference_group)
                    && affinity_candidate_in_group(
                        self.template,
                        runtime,
                        &available,
                        candidate.preference_group,
                    )
                    .is_none_or(matches_candidate)
            }
            RouteAffinityPolicy::FallbackSticky => {
                let affinity =
                    affinity_candidate(self.template, runtime, &available).filter(|affinity| {
                        fallback_affinity_within_configured_window(
                            self.template,
                            runtime,
                            &available,
                        ) || best_group.is_none_or(|best| best >= affinity.preference_group)
                    });
                affinity.map_or_else(
                    || best_group == Some(candidate.preference_group),
                    matches_candidate,
                )
            }
            RouteAffinityPolicy::Hard => {
                if let Some(affinity) = affinity_candidate(self.template, runtime, &available) {
                    matches_candidate(affinity)
                } else {
                    best_group == Some(candidate.preference_group)
                }
            }
        }
    }

    fn select_supported_candidate_with_affinity_mode(
        &self,
        state: &mut RoutePlanAttemptState,
        runtime: &RoutePlanRuntimeState,
        request_model: Option<&str>,
        affinity_mode: RoutePlanAffinitySelectionMode,
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

            let Some(candidate) =
                self.next_unavoided_candidate(state, runtime, affinity_mode, request_model)
            else {
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

    pub fn select_affinity_candidate_with_runtime_state(
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

            let route_candidates = self
                .template
                .candidates
                .iter()
                .filter(|candidate| !state.avoids_candidate(self.template, candidate))
                .collect::<Vec<_>>();
            let Some(candidate) = affinity_candidate(self.template, runtime, &route_candidates)
            else {
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

    fn selected_route_candidate_for_candidate(
        &self,
        candidate: &'a RouteCandidate,
    ) -> SelectedRouteCandidate<'a> {
        SelectedRouteCandidate {
            candidate,
            provider_endpoint: candidate_provider_endpoint_key(self.template, candidate),
        }
    }

    fn next_unavoided_candidate(
        &self,
        state: &RoutePlanAttemptState,
        runtime: &RoutePlanRuntimeState,
        affinity_mode: RoutePlanAffinitySelectionMode,
        request_model: Option<&str>,
    ) -> Option<&'a RouteCandidate> {
        let route_candidates = self
            .template
            .candidates
            .iter()
            .filter(|candidate| !state.avoids_candidate(self.template, candidate))
            .collect::<Vec<_>>();

        best_candidate_by_affinity_selection_mode(
            self.template,
            runtime,
            &route_candidates,
            affinity_mode,
            request_model,
        )
    }

    fn candidates_exhausted(&self, state: &RoutePlanAttemptState) -> bool {
        state.route_candidates_exhausted(self.template)
    }
}

fn best_candidate_by_affinity_selection_mode<'a>(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    candidates: &[&'a RouteCandidate],
    affinity_mode: RoutePlanAffinitySelectionMode,
    request_model: Option<&str>,
) -> Option<&'a RouteCandidate> {
    if runtime.affinity_provider_endpoint().is_none()
        && let Some(preferred) =
            new_session_preference_candidate(template, runtime, candidates, request_model)
    {
        return Some(preferred);
    }
    let affinity_policy = affinity_policy_for_selection(template.affinity_policy, affinity_mode);
    match affinity_policy {
        RouteAffinityPolicy::Off => {
            first_candidate_in_best_preference_group(template, runtime, candidates, request_model)
        }
        RouteAffinityPolicy::PreferredGroup => {
            best_candidate_in_preference_group(template, runtime, candidates, request_model)
        }
        RouteAffinityPolicy::FallbackSticky => affinity_candidate(template, runtime, candidates)
            .filter(|candidate| {
                fallback_affinity_within_configured_window(template, runtime, candidates)
                    || best_available_preference_group(template, runtime, candidates)
                        .is_none_or(|best_group| best_group >= candidate.preference_group)
            })
            .or_else(|| {
                first_candidate_in_best_preference_group(
                    template,
                    runtime,
                    candidates,
                    request_model,
                )
            }),
        RouteAffinityPolicy::Hard => {
            if runtime.affinity_provider_endpoint().is_some() {
                affinity_candidate(template, runtime, candidates)
            } else {
                first_candidate_in_best_preference_group(
                    template,
                    runtime,
                    candidates,
                    request_model,
                )
            }
        }
    }
}

fn affinity_policy_for_selection(
    configured: RouteAffinityPolicy,
    affinity_mode: RoutePlanAffinitySelectionMode,
) -> RouteAffinityPolicy {
    match (configured, affinity_mode) {
        (RouteAffinityPolicy::Hard, RoutePlanAffinitySelectionMode::SoftSessionPreferred) => {
            RouteAffinityPolicy::FallbackSticky
        }
        _ => configured,
    }
}

fn first_candidate_in_best_preference_group<'a>(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    candidates: &[&'a RouteCandidate],
    request_model: Option<&str>,
) -> Option<&'a RouteCandidate> {
    let best_group = best_available_preference_group(template, runtime, candidates)?;

    let best_group_candidates = candidates.iter().copied().filter(|candidate| {
        candidate.preference_group == best_group
            && candidate_available_in_runtime(template, runtime, candidate)
    });
    let best_group_candidates = best_group_candidates.collect::<Vec<_>>();
    weighted_round_robin_candidate(
        template,
        runtime,
        best_group,
        &best_group_candidates,
        request_model,
    )
    .or_else(|| best_group_candidates.into_iter().next())
}

fn best_candidate_in_preference_group<'a>(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    candidates: &[&'a RouteCandidate],
    request_model: Option<&str>,
) -> Option<&'a RouteCandidate> {
    let best_group = best_available_preference_group(template, runtime, candidates)?;

    if let Some(candidate) = affinity_candidate_in_group(template, runtime, candidates, best_group)
    {
        return Some(candidate);
    }

    let best_group_candidates = candidates.iter().copied().filter(|candidate| {
        candidate.preference_group == best_group
            && candidate_available_in_runtime(template, runtime, candidate)
    });
    let best_group_candidates = best_group_candidates.collect::<Vec<_>>();
    weighted_round_robin_candidate(
        template,
        runtime,
        best_group,
        &best_group_candidates,
        request_model,
    )
    .or_else(|| best_group_candidates.into_iter().next())
}

fn best_available_preference_group(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    candidates: &[&RouteCandidate],
) -> Option<u32> {
    candidates
        .iter()
        .copied()
        .filter(|candidate| candidate_available_in_runtime(template, runtime, candidate))
        .map(|candidate| candidate.preference_group)
        .min()
}

const MAX_ROUND_ROBIN_CURSOR_KEYS: usize = 4096;

#[derive(Debug, Default)]
struct WeightedRoundRobinCursor {
    scores: BTreeMap<String, i128>,
    member_offsets: BTreeMap<String, usize>,
}

type RoundRobinCursorKey = (String, u32, Vec<usize>);

static ROUND_ROBIN_CURSORS: OnceLock<
    Mutex<BTreeMap<RoundRobinCursorKey, WeightedRoundRobinCursor>>,
> = OnceLock::new();

fn weighted_round_robin_candidate<'a>(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    preference_group: u32,
    candidates: &[&'a RouteCandidate],
    request_model: Option<&str>,
) -> Option<&'a RouteCandidate> {
    let eligible = candidates
        .iter()
        .copied()
        .filter(|candidate| {
            candidate_uses_round_robin(template, candidate)
                && request_model.is_none_or(|model| candidate_supports_model(candidate, model))
        })
        .collect::<Vec<_>>();
    if eligible.is_empty() {
        return None;
    }
    let eligible_signature = eligible
        .iter()
        .map(|candidate| candidate.stable_index)
        .collect::<Vec<_>>();

    let mut entities = Vec::<(String, Vec<&RouteCandidate>, u64)>::new();
    let mut entity_indices = BTreeMap::<String, usize>::new();
    for candidate in eligible {
        let entity_key = round_robin_capacity_scope_key(template, candidate);
        if let Some(index) = entity_indices.get(&entity_key).copied() {
            entities[index].1.push(candidate);
        } else {
            entity_indices.insert(entity_key.clone(), entities.len());
            entities.push((entity_key, vec![candidate], 0));
        }
    }
    for (_, candidates, weight) in &mut entities {
        *weight = candidates
            .iter()
            .map(|candidate| round_robin_candidate_weight(template, runtime, candidate))
            .min()
            .unwrap_or(0);
    }
    if entities.iter().any(|(_, _, weight)| *weight > 0) {
        entities.retain(|(_, _, weight)| *weight > 0);
    } else {
        for (_, _, weight) in &mut entities {
            *weight = 1;
        }
    }
    let total_weight = entities
        .iter()
        .map(|(_, _, weight)| *weight)
        .fold(0_u64, u64::saturating_add);

    let route_graph_key = template.route_graph_key();
    let key = (route_graph_key, preference_group, eligible_signature);
    let cursors = ROUND_ROBIN_CURSORS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut cursors = cursors
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !cursors.contains_key(&key)
        && cursors.len() >= MAX_ROUND_ROBIN_CURSOR_KEYS
        && let Some(oldest_key) = cursors.keys().next().cloned()
    {
        cursors.remove(&oldest_key);
    }
    let cursor = cursors.entry(key).or_default();
    let eligible_keys = entities
        .iter()
        .map(|(key, _, _)| key.clone())
        .collect::<BTreeSet<_>>();
    cursor.scores.retain(|key, _| eligible_keys.contains(key));
    cursor
        .member_offsets
        .retain(|key, _| eligible_keys.contains(key));

    let mut selected_index = None;
    let mut selected_score = i128::MIN;
    for (index, (entity_key, _, weight)) in entities.iter().enumerate() {
        let score = cursor.scores.entry(entity_key.clone()).or_default();
        *score = score.saturating_add(i128::from(*weight));
        let score = *score;
        if score > selected_score {
            selected_score = score;
            selected_index = Some(index);
        }
    }

    let selected_index = selected_index?;
    let (selected_key, members, _) = &entities[selected_index];
    if let Some(score) = cursor.scores.get_mut(selected_key) {
        *score = score.saturating_sub(i128::from(total_weight));
    }
    let member_offset = cursor
        .member_offsets
        .entry(selected_key.clone())
        .or_default();
    let selected = members[*member_offset % members.len()];
    *member_offset = member_offset.wrapping_add(1);
    Some(selected)
}

fn round_robin_capacity_scope_key(
    template: &RoutePlanTemplate,
    candidate: &RouteCandidate,
) -> String {
    let provider_endpoint = candidate_provider_endpoint_key(template, candidate);
    candidate
        .concurrency
        .limit_key(template.service_name.as_str(), &provider_endpoint)
        .unwrap_or_else(|| format!("unlimited:{}", provider_endpoint.stable_key()))
}

fn round_robin_candidate_weight(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    candidate: &RouteCandidate,
) -> u64 {
    let key = candidate_provider_endpoint_key(template, candidate);
    let runtime = runtime.provider_endpoint(&key);
    let Some(limit) = runtime
        .concurrency_limit
        .or(candidate.concurrency.max_concurrent_requests)
    else {
        return 1;
    };
    let active = runtime.concurrency_active.unwrap_or(0);
    u64::from(limit.saturating_sub(active))
}

fn candidate_uses_round_robin(template: &RoutePlanTemplate, candidate: &RouteCandidate) -> bool {
    candidate.route_path.iter().any(|route_name| {
        template
            .nodes
            .get(route_name)
            .is_some_and(|node| node.strategy == RouteStrategy::RoundRobin)
    })
}

fn affinity_candidate<'a>(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    candidates: &[&'a RouteCandidate],
) -> Option<&'a RouteCandidate> {
    let affinity_key = runtime.affinity_provider_endpoint()?;
    candidates.iter().copied().find(|candidate| {
        candidate_provider_endpoint_key(template, candidate) == *affinity_key
            && candidate_available_for_affinity(template, runtime, candidate)
    })
}

fn affinity_candidate_in_group<'a>(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    candidates: &[&'a RouteCandidate],
    preference_group: u32,
) -> Option<&'a RouteCandidate> {
    let affinity_key = runtime.affinity_provider_endpoint()?;
    candidates.iter().copied().find(|candidate| {
        candidate.preference_group == preference_group
            && candidate_provider_endpoint_key(template, candidate) == *affinity_key
            && candidate_available_for_affinity(template, runtime, candidate)
    })
}

fn new_session_preference_candidate<'a>(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    candidates: &[&'a RouteCandidate],
    request_model: Option<&str>,
) -> Option<&'a RouteCandidate> {
    let preferred = runtime.new_session_preference()?;
    candidates.iter().copied().find(|candidate| {
        candidate_provider_endpoint_key(template, candidate) == *preferred
            && request_model.is_none_or(|model| candidate_supports_model(candidate, model))
            && candidate_available_in_runtime(template, runtime, candidate)
    })
}

fn fallback_affinity_within_configured_window(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    candidates: &[&RouteCandidate],
) -> bool {
    let Some(affinity_key) = runtime.affinity_provider_endpoint() else {
        return true;
    };
    let Some(affinity_candidate) = candidates
        .iter()
        .copied()
        .find(|candidate| candidate_provider_endpoint_key(template, candidate) == *affinity_key)
    else {
        return true;
    };
    let Some(best_group) = candidates
        .iter()
        .copied()
        .filter(|candidate| candidate_routable_except_usage(template, runtime, candidate))
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
) -> bool {
    runtime
        .candidate_runtime_snapshot(template, candidate)
        .runtime_available
}

fn candidate_available_for_affinity(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    candidate: &RouteCandidate,
) -> bool {
    runtime
        .candidate_runtime_snapshot(template, candidate)
        .affinity_runtime_available
}

fn candidate_available_for_selection(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    candidate: &RouteCandidate,
) -> bool {
    let key = candidate_provider_endpoint_key(template, candidate);
    if runtime.affinity_provider_endpoint() == Some(&key) {
        candidate_available_for_affinity(template, runtime, candidate)
    } else {
        candidate_available_in_runtime(template, runtime, candidate)
    }
}

fn candidate_routable_except_usage(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    candidate: &RouteCandidate,
) -> bool {
    runtime
        .candidate_runtime_snapshot(template, candidate)
        .routable_except_usage
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
    continuity_domain: Option<String>,
    enabled: bool,
    priority: u32,
    tags: BTreeMap<String, String>,
    supported_models: BTreeMap<String, bool>,
    model_mapping: BTreeMap<String, String>,
    limits: ProviderConcurrencyLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConditionalExpansion {
    MatchRequest,
    AllConditionalBranches,
}

struct RouteExpansionContext<'a> {
    request: &'a RouteRequestContext,
    conditional: ConditionalExpansion,
}

struct RouteExpansionFrame<'a> {
    route_name: &'a str,
    node: &'a RouteNodeConfig,
    node_path: &'a [String],
}

#[derive(Debug, Clone)]
pub struct CompiledRouteGraph {
    service_name: String,
    view: ServiceRouteConfig,
    routing: RouteGraphConfig,
    nodes: BTreeMap<String, RouteNodePlan>,
    static_leaves: Vec<RouteLeaf>,
    static_candidates: Vec<RouteCandidate>,
    digest: String,
}

impl CompiledRouteGraph {
    pub fn compile(service_name: &str, view: &ServiceRouteConfig) -> Result<Self> {
        let routing = effective_routing(view);
        validate_route_provider_name_conflicts(service_name, view, &routing)?;
        let nodes = normalize_route_nodes(service_name, view, &routing)?;
        let expansion = RouteExpansionContext {
            request: &RouteRequestContext::default(),
            conditional: ConditionalExpansion::AllConditionalBranches,
        };
        let static_leaves = expand_route_leaves(service_name, view, &routing, &expansion)?;
        ensure_unique_route_leaves(service_name, &static_leaves)?;
        let static_candidates = route_candidates_from_leaves(service_name, view, &static_leaves)?;
        validate_candidate_concurrency_groups(service_name, &static_candidates)?;
        let digest = static_route_graph_digest(service_name, &routing, &nodes, &static_candidates);

        Ok(Self {
            service_name: service_name.to_string(),
            view: view.clone(),
            routing,
            nodes,
            static_leaves,
            static_candidates,
            digest,
        })
    }

    pub fn digest(&self) -> &str {
        self.digest.as_str()
    }

    pub fn service_name(&self) -> &str {
        self.service_name.as_str()
    }

    pub fn candidates(&self) -> &[RouteCandidate] {
        self.static_candidates.as_slice()
    }

    pub fn candidate_identities(&self) -> Vec<RuntimeUpstreamIdentity> {
        self.static_candidates
            .iter()
            .map(|candidate| {
                RuntimeUpstreamIdentity::new_with_auth(
                    ProviderEndpointKey::new(
                        self.service_name.clone(),
                        candidate.provider_id.clone(),
                        candidate.endpoint_id.clone(),
                    ),
                    candidate.base_url.clone(),
                    candidate.continuity_domain.clone(),
                    &candidate.auth,
                )
            })
            .collect()
    }

    pub fn route_plan(&self, request: &RouteRequestContext) -> Result<RoutePlanTemplate> {
        self.route_plan_with_expansion(request, ConditionalExpansion::MatchRequest)
    }

    pub fn handshake_plan(&self) -> RoutePlanTemplate {
        self.route_plan_from_parts(
            self.static_leaves.as_slice(),
            self.static_candidates.clone(),
        )
    }

    fn route_plan_with_expansion(
        &self,
        request: &RouteRequestContext,
        conditional: ConditionalExpansion,
    ) -> Result<RoutePlanTemplate> {
        let expansion = RouteExpansionContext {
            request,
            conditional,
        };
        let leaves = expand_route_leaves(
            self.service_name.as_str(),
            &self.view,
            &self.routing,
            &expansion,
        )?;
        ensure_unique_route_leaves(self.service_name.as_str(), &leaves)?;
        let candidates =
            route_candidates_from_leaves(self.service_name.as_str(), &self.view, &leaves)?;
        Ok(self.route_plan_from_parts(leaves.as_slice(), candidates))
    }

    fn route_plan_from_parts(
        &self,
        leaves: &[RouteLeaf],
        candidates: Vec<RouteCandidate>,
    ) -> RoutePlanTemplate {
        RoutePlanTemplate {
            service_name: self.service_name.clone(),
            entry: self.routing.entry.clone(),
            affinity_policy: self.routing.affinity_policy,
            scheduling_preset: self.routing.scheduling_preset,
            fallback_ttl_ms: self.routing.fallback_ttl_ms,
            reprobe_preferred_after_ms: self.routing.reprobe_preferred_after_ms,
            nodes: self.nodes.clone(),
            expanded_provider_order: leaves.iter().map(|leaf| leaf.provider_id.clone()).collect(),
            candidates,
        }
    }
}

pub fn compile_route_plan_template(
    service_name: &str,
    view: &ServiceRouteConfig,
) -> Result<RoutePlanTemplate> {
    compile_route_plan_template_with_request(service_name, view, &RouteRequestContext::default())
}

pub fn compile_route_plan_template_with_request(
    service_name: &str,
    view: &ServiceRouteConfig,
    request: &RouteRequestContext,
) -> Result<RoutePlanTemplate> {
    CompiledRouteGraph::compile(service_name, view)?.route_plan(request)
}

pub fn compile_route_handshake_plan(
    service_name: &str,
    view: &ServiceRouteConfig,
) -> Result<RoutePlanTemplate> {
    Ok(CompiledRouteGraph::compile(service_name, view)?.handshake_plan())
}

fn validate_route_provider_name_conflicts(
    service_name: &str,
    view: &ServiceRouteConfig,
    routing: &RouteGraphConfig,
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
    view: &ServiceRouteConfig,
    routing: &RouteGraphConfig,
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
    view: &ServiceRouteConfig,
    routing: &RouteGraphConfig,
    route_name: &str,
    node: &RouteNodeConfig,
) -> Result<()> {
    match node.strategy {
        RouteStrategy::Conditional => {
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
    view: &ServiceRouteConfig,
    routing: &RouteGraphConfig,
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

fn provider_endpoint_exists(provider: &ProviderConfig, endpoint_id: &str) -> bool {
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

fn provider_endpoint_enabled(provider: &ProviderConfig, endpoint_id: &str) -> bool {
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

fn expand_route_leaves(
    service_name: &str,
    view: &ServiceRouteConfig,
    routing: &RouteGraphConfig,
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
    view: &ServiceRouteConfig,
    routing: &RouteGraphConfig,
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
    view: &ServiceRouteConfig,
    routing: &RouteGraphConfig,
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
        RouteStrategy::OrderedFailover => {
            expand_ordered_route_children(service_name, view, routing, &frame, expansion, stack)
        }
        RouteStrategy::RoundRobin => {
            expand_round_robin_route_children(service_name, view, routing, &frame, expansion, stack)
        }
        RouteStrategy::ManualSticky => {
            expand_manual_sticky_route(service_name, view, routing, &frame, expansion, stack)
        }
        RouteStrategy::TagPreferred => {
            expand_tag_preferred_route(service_name, view, routing, &frame, expansion, stack)
        }
        RouteStrategy::Conditional => {
            expand_conditional_route(service_name, view, routing, &frame, expansion, stack)
        }
    };
    stack.pop();
    result
}

fn expand_ordered_route_children(
    service_name: &str,
    view: &ServiceRouteConfig,
    routing: &RouteGraphConfig,
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

fn expand_round_robin_route_children(
    service_name: &str,
    view: &ServiceRouteConfig,
    routing: &RouteGraphConfig,
    frame: &RouteExpansionFrame<'_>,
    expansion: &RouteExpansionContext<'_>,
    stack: &mut Vec<String>,
) -> Result<Vec<RouteLeaf>> {
    if frame.node.children.is_empty() {
        anyhow::bail!(
            "[{service_name}] round-robin route '{}' requires at least one child",
            frame.route_name
        );
    }

    // Each child contributes its own best group to the shared round-robin group.
    // Relative fallback groups remain intact, so a saturated/failed primary can
    // still progress to the next group without changing ordered-failover behavior.
    let mut leaves = Vec::new();
    for child_name in &frame.node.children {
        let mut child_leaves = expand_route_ref(
            service_name,
            view,
            routing,
            child_name.as_str(),
            frame.node_path,
            expansion,
            stack,
        )?;
        let Some(min_group) = child_leaves.iter().map(|leaf| leaf.preference_group).min() else {
            continue;
        };
        for leaf in &mut child_leaves {
            leaf.preference_group = leaf.preference_group.saturating_sub(min_group);
        }
        leaves.extend(child_leaves);
    }
    Ok(leaves)
}

fn expand_manual_sticky_route(
    service_name: &str,
    view: &ServiceRouteConfig,
    routing: &RouteGraphConfig,
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
    view: &ServiceRouteConfig,
    routing: &RouteGraphConfig,
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

    if matches!(frame.node.on_exhausted, RouteExhaustedAction::Stop) {
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
    view: &ServiceRouteConfig,
    routing: &RouteGraphConfig,
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
        ConditionalExpansion::AllConditionalBranches => {
            let then_leaves = expand_route_ref(
                service_name,
                view,
                routing,
                then,
                frame.node_path,
                expansion,
                stack,
            )?;
            ensure_unique_route_leaves(service_name, &then_leaves)?;
            let default_leaves = expand_route_ref(
                service_name,
                view,
                routing,
                default_route,
                frame.node_path,
                expansion,
                stack,
            )?;
            ensure_unique_route_leaves(service_name, &default_leaves)?;

            let mut leaves = then_leaves;
            leaves.extend(default_leaves);
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
    condition: &RouteCondition,
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
    view: &ServiceRouteConfig,
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

struct StableRouteDigest {
    hasher: Sha256,
}

impl StableRouteDigest {
    fn new() -> Self {
        Self::with_namespace("codex-helper:compiled-route-graph")
    }

    fn with_namespace(namespace: &str) -> Self {
        let mut digest = Self {
            hasher: Sha256::new(),
        };
        digest.text(namespace);
        digest
    }

    fn text(&mut self, value: &str) {
        self.length(value.len());
        self.hasher.update(value.as_bytes());
    }

    fn length(&mut self, value: usize) {
        self.u64(u64::try_from(value).unwrap_or(u64::MAX));
    }

    fn bool(&mut self, value: bool) {
        self.hasher.update([u8::from(value)]);
    }

    fn u32(&mut self, value: u32) {
        self.hasher.update(value.to_be_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.hasher.update(value.to_be_bytes());
    }

    fn optional_text(&mut self, value: Option<&str>) {
        self.bool(value.is_some());
        if let Some(value) = value {
            self.text(value);
        }
    }

    fn optional_u32(&mut self, value: Option<u32>) {
        self.bool(value.is_some());
        if let Some(value) = value {
            self.u32(value);
        }
    }

    fn optional_u64(&mut self, value: Option<u64>) {
        self.bool(value.is_some());
        if let Some(value) = value {
            self.u64(value);
        }
    }

    fn string_map(&mut self, values: &BTreeMap<String, String>) {
        self.length(values.len());
        for (key, value) in values {
            self.text(key);
            self.text(value);
        }
    }

    fn bool_map(&mut self, values: &BTreeMap<String, bool>) {
        self.length(values.len());
        for (key, value) in values {
            self.text(key);
            self.bool(*value);
        }
    }

    fn finish(self) -> String {
        format!("sha256:{:x}", self.hasher.finalize())
    }
}

fn encode_affinity_route_identity(digest: &mut StableRouteDigest, template: &RoutePlanTemplate) {
    digest.text("service_name");
    digest.text(template.service_name.as_str());
    digest.text("entry");
    digest.text(template.entry.as_str());
    digest.text("affinity_policy");
    digest.text(affinity_policy_name(template.affinity_policy));
    digest.text("fallback_ttl_ms");
    digest.optional_u64(template.fallback_ttl_ms);
    digest.text("reprobe_preferred_after_ms");
    digest.optional_u64(template.reprobe_preferred_after_ms);
    digest.text("expanded_provider_order");
    digest.length(template.expanded_provider_order.len());
    for provider_id in &template.expanded_provider_order {
        digest.text(provider_id);
    }
    digest.text("nodes");
    digest.length(template.nodes.len());
    for (name, node) in &template.nodes {
        digest.text(name);
        encode_affinity_route_node(digest, node);
    }
    digest.text("candidates");
    digest.length(template.candidates.len());
    for candidate in &template.candidates {
        encode_affinity_route_candidate(digest, template, candidate);
    }
}

fn encode_affinity_route_node(digest: &mut StableRouteDigest, node: &RouteNodePlan) {
    digest.text("strategy");
    digest.text(routing_policy_name(node.strategy));
    digest.text("children");
    digest.length(node.children.len());
    for child in &node.children {
        encode_route_ref(digest, child);
    }
    digest.text("target");
    encode_optional_route_ref(digest, node.target.as_ref());
    digest.text("prefer_tags");
    digest.length(node.prefer_tags.len());
    for filter in &node.prefer_tags {
        digest.string_map(filter);
    }
    digest.text("on_exhausted");
    digest.text(exhausted_action_name(node.on_exhausted));
    digest.text("when");
    encode_optional_condition(digest, node.when.as_ref());
    digest.text("then");
    encode_optional_route_ref(digest, node.then.as_ref());
    digest.text("default");
    encode_optional_route_ref(digest, node.default_route.as_ref());
}

fn encode_affinity_route_candidate(
    digest: &mut StableRouteDigest,
    template: &RoutePlanTemplate,
    candidate: &RouteCandidate,
) {
    digest.text("provider_id");
    digest.text(candidate.provider_id.as_str());
    digest.text("endpoint_id");
    digest.text(candidate.endpoint_id.as_str());
    digest.text("base_url");
    digest.text(candidate.base_url.as_str());
    digest.text("continuity_domain");
    digest.optional_text(candidate.continuity_domain.as_deref());
    digest.text("credential_scope");
    digest.optional_text(
        template
            .candidate_identity(candidate)
            .credential_scope
            .as_deref(),
    );
    digest.text("tags");
    digest.string_map(&candidate.tags);
    digest.text("supported_models");
    digest.bool_map(&candidate.supported_models);
    digest.text("model_mapping");
    digest.string_map(&candidate.model_mapping);
    digest.text("route_path");
    digest.length(candidate.route_path.len());
    for segment in &candidate.route_path {
        digest.text(segment);
    }
    digest.text("preference_group");
    digest.u32(candidate.preference_group);
    digest.text("stable_index");
    digest.length(candidate.stable_index);
}

fn static_route_graph_digest(
    service_name: &str,
    routing: &RouteGraphConfig,
    nodes: &BTreeMap<String, RouteNodePlan>,
    candidates: &[RouteCandidate],
) -> String {
    let mut digest = StableRouteDigest::new();
    digest.text("service_name");
    digest.text(service_name);
    digest.text("entry");
    digest.text(routing.entry.as_str());
    digest.text("affinity_policy");
    digest.text(affinity_policy_name(routing.affinity_policy));
    digest.text("fallback_ttl_ms");
    digest.optional_u64(routing.fallback_ttl_ms);
    digest.text("reprobe_preferred_after_ms");
    digest.optional_u64(routing.reprobe_preferred_after_ms);
    digest.text("nodes");
    digest.length(nodes.len());
    for (name, node) in nodes {
        digest.text(name);
        encode_route_node(&mut digest, node);
    }
    digest.text("candidates");
    digest.length(candidates.len());
    for candidate in candidates {
        encode_route_candidate(&mut digest, candidate);
    }
    digest.finish()
}

fn encode_route_node(digest: &mut StableRouteDigest, node: &RouteNodePlan) {
    digest.text("name");
    digest.text(node.name.as_str());
    digest.text("strategy");
    digest.text(routing_policy_name(node.strategy));
    digest.text("children");
    digest.length(node.children.len());
    for child in &node.children {
        encode_route_ref(digest, child);
    }
    digest.text("target");
    encode_optional_route_ref(digest, node.target.as_ref());
    digest.text("prefer_tags");
    digest.length(node.prefer_tags.len());
    for filter in &node.prefer_tags {
        digest.string_map(filter);
    }
    digest.text("on_exhausted");
    digest.text(exhausted_action_name(node.on_exhausted));
    digest.text("metadata");
    digest.string_map(&node.metadata);
    digest.text("when");
    encode_optional_condition(digest, node.when.as_ref());
    digest.text("then");
    encode_optional_route_ref(digest, node.then.as_ref());
    digest.text("default");
    encode_optional_route_ref(digest, node.default_route.as_ref());
}

fn encode_route_ref(digest: &mut StableRouteDigest, route_ref: &RouteRef) {
    match route_ref {
        RouteRef::Route(name) => {
            digest.text("route");
            digest.text(name);
        }
        RouteRef::Provider(provider_id) => {
            digest.text("provider");
            digest.text(provider_id);
        }
        RouteRef::ProviderEndpoint {
            provider_id,
            endpoint_id,
        } => {
            digest.text("provider_endpoint");
            digest.text(provider_id);
            digest.text(endpoint_id);
        }
    }
}

fn encode_optional_route_ref(digest: &mut StableRouteDigest, route_ref: Option<&RouteRef>) {
    digest.bool(route_ref.is_some());
    if let Some(route_ref) = route_ref {
        encode_route_ref(digest, route_ref);
    }
}

fn encode_optional_condition(digest: &mut StableRouteDigest, condition: Option<&RouteCondition>) {
    digest.bool(condition.is_some());
    if let Some(condition) = condition {
        digest.optional_text(condition.model.as_deref());
        digest.optional_text(condition.service_tier.as_deref());
        digest.optional_text(condition.reasoning_effort.as_deref());
        digest.optional_text(condition.path.as_deref());
        digest.optional_text(condition.method.as_deref());
        digest.string_map(&condition.headers);
    }
}

fn encode_route_candidate(digest: &mut StableRouteDigest, candidate: &RouteCandidate) {
    digest.text("provider_id");
    digest.text(candidate.provider_id.as_str());
    digest.text("provider_alias");
    digest.optional_text(candidate.provider_alias.as_deref());
    digest.text("endpoint_id");
    digest.text(candidate.endpoint_id.as_str());
    digest.text("base_url");
    digest.text(candidate.base_url.as_str());
    digest.text("continuity_domain");
    digest.optional_text(candidate.continuity_domain.as_deref());
    digest.text("auth_shape");
    encode_auth_shape(digest, &candidate.auth);
    digest.text("tags");
    digest.string_map(&candidate.tags);
    digest.text("supported_models");
    digest.bool_map(&candidate.supported_models);
    digest.text("model_mapping");
    digest.string_map(&candidate.model_mapping);
    digest.text("route_path");
    digest.length(candidate.route_path.len());
    for segment in &candidate.route_path {
        digest.text(segment);
    }
    digest.text("preference_group");
    digest.u32(candidate.preference_group);
    digest.text("stable_index");
    digest.length(candidate.stable_index);
    digest.text("concurrency");
    digest.optional_u32(candidate.concurrency.max_concurrent_requests);
    digest.optional_text(candidate.concurrency.limit_group.as_deref());
}

fn encode_auth_shape(digest: &mut StableRouteDigest, auth: &UpstreamAuth) {
    digest.bool(auth.auth_token.is_some());
    digest.bool(auth.auth_token_env.is_some());
    digest.bool(auth.api_key.is_some());
    digest.bool(auth.api_key_env.is_some());
    digest.bool(auth.allow_anonymous == Some(true));
}

fn affinity_policy_name(policy: RouteAffinityPolicy) -> &'static str {
    match policy {
        RouteAffinityPolicy::Off => "off",
        RouteAffinityPolicy::PreferredGroup => "preferred-group",
        RouteAffinityPolicy::FallbackSticky => "fallback-sticky",
        RouteAffinityPolicy::Hard => "hard",
    }
}

fn routing_policy_name(policy: RouteStrategy) -> &'static str {
    match policy {
        RouteStrategy::ManualSticky => "manual-sticky",
        RouteStrategy::OrderedFailover => "ordered-failover",
        RouteStrategy::RoundRobin => "round-robin",
        RouteStrategy::TagPreferred => "tag-preferred",
        RouteStrategy::Conditional => "conditional",
    }
}

fn exhausted_action_name(action: RouteExhaustedAction) -> &'static str {
    match action {
        RouteExhaustedAction::Continue => "continue",
        RouteExhaustedAction::Stop => "stop",
    }
}

fn route_candidates_from_leaves(
    service_name: &str,
    view: &ServiceRouteConfig,
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

        let auth = provider.effective_auth();
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
            let supported_models =
                merge_bool_maps(&provider.supported_models, &endpoint.supported_models);
            let model_mapping = merge_string_maps(&provider.model_mapping, &endpoint.model_mapping);
            let model_rules = Arc::new(
                model_routing::CompiledModelRules::compile(
                    supported_models
                        .iter()
                        .map(|(pattern, supported)| (pattern.clone(), *supported)),
                    model_mapping
                        .iter()
                        .map(|(pattern, replacement)| (pattern.clone(), replacement.clone())),
                )
                .with_context(|| {
                    format!(
                        "[{service_name}] provider endpoint '{}.{}' has invalid model rules",
                        leaf.provider_id, endpoint.endpoint_id
                    )
                })?,
            );
            candidates.push(RouteCandidate {
                provider_id: leaf.provider_id.clone(),
                provider_alias: provider.alias.clone(),
                endpoint_id: endpoint.endpoint_id,
                base_url: endpoint.base_url,
                continuity_domain: endpoint.continuity_domain,
                auth: auth.clone(),
                tags: merge_string_maps_with_provider_id(
                    leaf.provider_id.as_str(),
                    &provider.tags,
                    &endpoint.tags,
                ),
                supported_models,
                model_mapping,
                model_rules,
                route_path: leaf.route_path.clone(),
                preference_group: leaf.preference_group,
                stable_index,
                concurrency: effective_candidate_concurrency(&provider.limits, &endpoint.limits),
            });
        }
    }
    Ok(candidates)
}

fn effective_candidate_concurrency(
    provider: &ProviderConcurrencyLimits,
    endpoint: &ProviderConcurrencyLimits,
) -> RouteCandidateConcurrency {
    RouteCandidateConcurrency {
        max_concurrent_requests: endpoint
            .max_concurrent_requests
            .or(provider.max_concurrent_requests),
        limit_group: endpoint
            .limit_group
            .as_ref()
            .or(provider.limit_group.as_ref())
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
    }
}

fn validate_candidate_concurrency_groups(
    service_name: &str,
    candidates: &[RouteCandidate],
) -> Result<()> {
    let mut limits_by_group = BTreeMap::<&str, Option<u32>>::new();
    for candidate in candidates {
        let Some(group) = candidate.concurrency.limit_group.as_deref() else {
            continue;
        };
        let limit = candidate.concurrency.max_concurrent_requests;
        if let Some(existing_limit) = limits_by_group.get(group)
            && *existing_limit != limit
        {
            let existing_limit = existing_limit
                .map(|value| value.to_string())
                .unwrap_or_else(|| "missing".to_string());
            let limit = limit
                .map(|value| value.to_string())
                .unwrap_or_else(|| "missing".to_string());
            anyhow::bail!(
                "[{service_name}] concurrency limit group '{group}' has conflicting \
                 max_concurrent_requests values {existing_limit} and {limit}; every candidate in \
                 an explicit group must declare the same limit"
            );
        }
        limits_by_group.insert(group, limit);
    }
    Ok(())
}

fn ordered_provider_endpoints(
    service_name: &str,
    provider_name: &str,
    provider: &ProviderConfig,
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
            continuity_domain: provider
                .continuity_domain
                .as_ref()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            enabled: true,
            priority: 0,
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
            limits: ProviderConcurrencyLimits::default(),
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
            continuity_domain: endpoint
                .continuity_domain
                .as_ref()
                .or(provider.continuity_domain.as_ref())
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            enabled: endpoint.enabled,
            priority: endpoint.priority,
            tags: endpoint.tags.clone(),
            supported_models: endpoint.supported_models.clone(),
            model_mapping: endpoint.model_mapping.clone(),
            limits: endpoint.limits.clone(),
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

fn candidate_supports_model(candidate: &RouteCandidate, requested_model: &str) -> bool {
    candidate.is_model_supported(requested_model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ProviderConcurrencyLimits, ProviderEndpointConfig, RouteAffinityPolicy, RouteCondition,
        RouteExhaustedAction, RouteGraphConfig, RouteNodeConfig, RouteStrategy,
        resolved_provider_order,
    };
    fn provider(base_url: &str) -> ProviderConfig {
        ProviderConfig {
            base_url: Some(base_url.to_string()),
            ..ProviderConfig::default()
        }
    }

    fn endpoint(base_url: &str, priority: u32) -> ProviderEndpointConfig {
        ProviderEndpointConfig {
            base_url: base_url.to_string(),
            enabled: true,
            priority,
            continuity_domain: None,
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
            limits: ProviderConcurrencyLimits::default(),
        }
    }

    fn tagged_provider(base_url: &str, key: &str, value: &str) -> ProviderConfig {
        ProviderConfig {
            base_url: Some(base_url.to_string()),
            tags: BTreeMap::from([(key.to_string(), value.to_string())]),
            ..ProviderConfig::default()
        }
    }

    fn limited_provider(base_url: &str, max_concurrent_requests: u32) -> ProviderConfig {
        ProviderConfig {
            base_url: Some(base_url.to_string()),
            limits: ProviderConcurrencyLimits {
                max_concurrent_requests: Some(max_concurrent_requests),
                limit_group: None,
            },
            ..ProviderConfig::default()
        }
    }

    fn limited_provider_in_group(
        base_url: &str,
        max_concurrent_requests: u32,
        limit_group: &str,
    ) -> ProviderConfig {
        ProviderConfig {
            base_url: Some(base_url.to_string()),
            limits: ProviderConcurrencyLimits {
                max_concurrent_requests: Some(max_concurrent_requests),
                limit_group: Some(limit_group.to_string()),
            },
            ..ProviderConfig::default()
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

    fn assert_provider_order_parity(view: &ServiceRouteConfig, template: &RoutePlanTemplate) {
        let resolved = resolved_provider_order("routing-ir-test", view).expect("resolved order");
        assert_eq!(template.expanded_provider_order, resolved);
        assert_eq!(provider_ids(template), resolved);
    }

    #[test]
    fn route_graph_key_changes_when_route_rules_change() {
        let providers = BTreeMap::from([
            (
                "a".to_string(),
                ProviderConfig {
                    base_url: Some("http://a.example/v1".to_string()),
                    tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                    ..ProviderConfig::default()
                },
            ),
            (
                "b".to_string(),
                ProviderConfig {
                    base_url: Some("http://b.example/v1".to_string()),
                    tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                    ..ProviderConfig::default()
                },
            ),
        ]);
        let request = RouteRequestContext::default();
        let ordered = ServiceRouteConfig {
            providers: providers.clone(),
            routing: Some(RouteGraphConfig {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RouteNodeConfig {
                        strategy: RouteStrategy::OrderedFailover,
                        children: vec!["a".to_string(), "b".to_string()],
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        };
        let tag_preferred = ServiceRouteConfig {
            providers,
            routing: Some(RouteGraphConfig {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RouteNodeConfig {
                        strategy: RouteStrategy::TagPreferred,
                        children: vec!["a".to_string(), "b".to_string()],
                        prefer_tags: vec![BTreeMap::from([(
                            "billing".to_string(),
                            "monthly".to_string(),
                        )])],
                        on_exhausted: RouteExhaustedAction::Continue,
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        };

        let ordered_template =
            compile_route_plan_template_with_request("routing-ir-test", &ordered, &request)
                .expect("ordered template");
        let tag_preferred_template =
            compile_route_plan_template_with_request("routing-ir-test", &tag_preferred, &request)
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

    #[test]
    fn route_graph_key_ignores_scheduling_preset() {
        let balanced = ServiceRouteConfig {
            providers: BTreeMap::from([
                ("a".to_string(), provider("https://a.example/v1")),
                ("b".to_string(), provider("https://b.example/v1")),
            ]),
            routing: Some(RouteGraphConfig {
                scheduling_preset: SchedulingPreset::Balanced,
                ..RouteGraphConfig::ordered_failover(vec!["a".to_string(), "b".to_string()])
            }),
            ..ServiceRouteConfig::default()
        };
        let mut throughput_first = balanced.clone();
        throughput_first
            .routing
            .as_mut()
            .expect("routing config")
            .scheduling_preset = SchedulingPreset::ThroughputFirst;

        let balanced_template =
            compile_route_plan_template("codex", &balanced).expect("balanced route template");
        let throughput_template = compile_route_plan_template("codex", &throughput_first)
            .expect("throughput-first route template");

        assert_eq!(
            balanced_template.route_graph_key(),
            throughput_template.route_graph_key()
        );
        assert_eq!(
            balanced_template.route_graph_key(),
            "route:v1:sha256:4201ef95b7971ae81af4daff01e413ff60705d1127a9621b6129ca0010abdd9f"
        );
    }

    #[test]
    fn route_graph_key_ignores_local_concurrency_policy() {
        let route = |limit, limit_group: Option<&str>| ServiceRouteConfig {
            providers: BTreeMap::from([(
                "relay".to_string(),
                ProviderConfig {
                    base_url: Some("https://relay.example/v1".to_string()),
                    limits: ProviderConcurrencyLimits {
                        max_concurrent_requests: Some(limit),
                        limit_group: limit_group.map(str::to_string),
                    },
                    ..ProviderConfig::default()
                },
            )]),
            ..ServiceRouteConfig::default()
        };

        let original = compile_route_plan_template("codex", &route(20, None))
            .expect("original route template")
            .route_graph_key();
        let raised = compile_route_plan_template("codex", &route(25, None))
            .expect("raised-limit route template")
            .route_graph_key();
        let grouped = compile_route_plan_template("codex", &route(25, Some("relay-account")))
            .expect("grouped route template")
            .route_graph_key();

        assert_eq!(original, raised);
        assert_eq!(original, grouped);
    }

    #[test]
    fn route_graph_key_changes_with_effective_runtime_credentials() {
        let route = |secret: &str| ServiceRouteConfig {
            providers: BTreeMap::from([(
                "relay".to_string(),
                ProviderConfig {
                    base_url: Some("https://relay.example/v1".to_string()),
                    auth: UpstreamAuth {
                        auth_token: Some(secret.to_string()),
                        ..UpstreamAuth::default()
                    },
                    ..ProviderConfig::default()
                },
            )]),
            ..ServiceRouteConfig::default()
        };

        let account_a = compile_route_plan_template("codex", &route("account-a-secret"))
            .expect("account A route template")
            .route_graph_key();
        let account_b = compile_route_plan_template("codex", &route("account-b-secret"))
            .expect("account B route template")
            .route_graph_key();

        assert_ne!(account_a, account_b);
        assert!(!account_a.contains("account-a-secret"));
        assert!(!account_b.contains("account-b-secret"));
    }

    #[test]
    fn route_graph_key_ignores_display_metadata() {
        let route = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "relay".to_string(),
                ProviderConfig {
                    alias: Some("Primary relay".to_string()),
                    base_url: Some("https://relay.example/v1".to_string()),
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(RouteGraphConfig {
                routes: BTreeMap::from([(
                    "main".to_string(),
                    RouteNodeConfig {
                        strategy: RouteStrategy::OrderedFailover,
                        children: vec!["relay".to_string()],
                        metadata: BTreeMap::from([(
                            "description".to_string(),
                            "Primary route".to_string(),
                        )]),
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        };
        let mut renamed = route.clone();
        renamed
            .providers
            .get_mut("relay")
            .expect("relay provider")
            .alias = Some("Renamed relay".to_string());
        renamed
            .routing
            .as_mut()
            .expect("routing config")
            .routes
            .get_mut("main")
            .expect("main route")
            .metadata
            .insert("description".to_string(), "Renamed route".to_string());

        let original = compile_route_plan_template("codex", &route)
            .expect("original route template")
            .route_graph_key();
        let renamed = compile_route_plan_template("codex", &renamed)
            .expect("renamed route template")
            .route_graph_key();

        assert_eq!(original, renamed);
    }

    #[test]
    fn compiled_route_graph_digest_is_static_canonical_and_child_order_sensitive() {
        fn graph(children: Vec<&str>, reverse_provider_insertion: bool) -> CompiledRouteGraph {
            let mut providers = BTreeMap::new();
            let provider_entries = if reverse_provider_insertion {
                vec![
                    ("beta", "https://beta.example/v1"),
                    ("alpha", "https://alpha.example/v1"),
                ]
            } else {
                vec![
                    ("alpha", "https://alpha.example/v1"),
                    ("beta", "https://beta.example/v1"),
                ]
            };
            for (name, base_url) in provider_entries {
                providers.insert(name.to_string(), provider(base_url));
            }

            CompiledRouteGraph::compile(
                "codex",
                &ServiceRouteConfig {
                    providers,
                    routing: Some(RouteGraphConfig {
                        entry: "root".to_string(),
                        routes: BTreeMap::from([(
                            "root".to_string(),
                            RouteNodeConfig {
                                strategy: RouteStrategy::OrderedFailover,
                                children: children.into_iter().map(str::to_string).collect(),
                                ..RouteNodeConfig::default()
                            },
                        )]),
                        ..RouteGraphConfig::default()
                    }),
                    ..ServiceRouteConfig::default()
                },
            )
            .expect("compile route graph")
        }

        let first = graph(vec!["alpha", "beta"], false);
        let reordered_maps = graph(vec!["alpha", "beta"], true);
        let reordered_children = graph(vec!["beta", "alpha"], true);

        assert_eq!(first.digest(), reordered_maps.digest());
        assert_ne!(first.digest(), reordered_children.digest());
        assert!(first.digest().starts_with("sha256:"));
        assert!(!first.digest().contains("source:"));
    }

    #[test]
    fn compiled_route_graph_digest_is_shared_by_all_conditional_requests() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                ("small".to_string(), provider("https://small.example/v1")),
                ("large".to_string(), provider("https://large.example/v1")),
            ]),
            routing: Some(RouteGraphConfig {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RouteNodeConfig {
                        strategy: RouteStrategy::Conditional,
                        when: Some(RouteCondition {
                            model: Some("gpt-5".to_string()),
                            ..RouteCondition::default()
                        }),
                        then: Some("large".to_string()),
                        default_route: Some("small".to_string()),
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        };
        let graph = CompiledRouteGraph::compile("codex", &view).expect("compile route graph");
        let digest = graph.digest().to_string();

        let matching = graph
            .route_plan(&RouteRequestContext {
                model: Some("gpt-5".to_string()),
                ..RouteRequestContext::default()
            })
            .expect("compile matching request plan");
        let defaulted = graph
            .route_plan(&RouteRequestContext {
                model: Some("gpt-4.1".to_string()),
                ..RouteRequestContext::default()
            })
            .expect("compile default request plan");

        assert_eq!(provider_ids(&matching), vec!["large"]);
        assert_eq!(provider_ids(&defaulted), vec!["small"]);
        assert_eq!(graph.digest(), digest);
    }

    #[test]
    fn compiled_route_graph_rejects_duplicate_endpoint_inside_unselected_conditional_branch() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfig {
                    endpoints: BTreeMap::from([
                        ("fast".to_string(), endpoint("https://fast.example/v1", 0)),
                        ("slow".to_string(), endpoint("https://slow.example/v1", 1)),
                    ]),
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(RouteGraphConfig {
                entry: "root".to_string(),
                routes: BTreeMap::from([
                    (
                        "bad".to_string(),
                        RouteNodeConfig {
                            strategy: RouteStrategy::OrderedFailover,
                            children: vec!["input.fast".to_string(), "input.fast".to_string()],
                            ..RouteNodeConfig::default()
                        },
                    ),
                    (
                        "root".to_string(),
                        RouteNodeConfig {
                            strategy: RouteStrategy::Conditional,
                            when: Some(RouteCondition {
                                model: Some("gpt-5".to_string()),
                                ..RouteCondition::default()
                            }),
                            then: Some("bad".to_string()),
                            default_route: Some("input.slow".to_string()),
                            ..RouteNodeConfig::default()
                        },
                    ),
                ]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        };

        let compile_error = CompiledRouteGraph::compile("codex", &view)
            .expect_err("static graph must reject duplicate endpoint");
        assert!(compile_error.to_string().contains("input.fast"));

        let request_error = compile_route_plan_template_with_request(
            "codex",
            &view,
            &RouteRequestContext {
                model: Some("gpt-4.1".to_string()),
                ..RouteRequestContext::default()
            },
        )
        .expect_err("request compile must validate the unselected branch");
        assert!(request_error.to_string().contains("input.fast"));

        let handshake_error = compile_route_handshake_plan("codex", &view)
            .expect_err("handshake compile must not hide the duplicate endpoint");
        assert!(handshake_error.to_string().contains("input.fast"));
    }

    #[test]
    fn compiled_route_graph_allows_same_endpoint_in_mutually_exclusive_branches() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfig {
                    endpoints: BTreeMap::from([(
                        "fast".to_string(),
                        endpoint("https://fast.example/v1", 0),
                    )]),
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(RouteGraphConfig {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RouteNodeConfig {
                        strategy: RouteStrategy::Conditional,
                        when: Some(RouteCondition {
                            model: Some("gpt-5".to_string()),
                            ..RouteCondition::default()
                        }),
                        then: Some("input.fast".to_string()),
                        default_route: Some("input.fast".to_string()),
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        };

        let graph = CompiledRouteGraph::compile("codex", &view)
            .expect("mutually exclusive branches may share an endpoint");

        assert_eq!(graph.candidates().len(), 1);
        assert_eq!(graph.candidates()[0].endpoint_id, "fast");
    }

    #[test]
    fn compiled_route_graph_rejects_ambiguous_candidate_model_rules() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfig {
                    base_url: Some("https://input.example/v1".to_string()),
                    model_mapping: BTreeMap::from([
                        ("ab*cd".to_string(), "first-*".to_string()),
                        ("abc*d".to_string(), "second-*".to_string()),
                    ]),
                    ..ProviderConfig::default()
                },
            )]),
            ..ServiceRouteConfig::default()
        };

        let error = CompiledRouteGraph::compile("codex", &view)
            .expect_err("candidate wildcard tie must fail graph compilation");
        let message = format!("{error:#}");
        assert!(message.contains("input.default"), "unexpected: {message}");
        assert!(message.contains("ab*cd"), "unexpected: {message}");
        assert!(message.contains("abc*d"), "unexpected: {message}");
    }

    #[test]
    fn compiled_route_graph_preserves_provider_endpoint_leaf() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfig {
                    endpoints: BTreeMap::from([
                        ("fast".to_string(), endpoint("https://fast.example/v1", 0)),
                        ("slow".to_string(), endpoint("https://slow.example/v1", 1)),
                    ]),
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "input.fast".to_string(),
                "input.slow".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };

        let graph = CompiledRouteGraph::compile("codex", &view).expect("compile endpoint leaf");
        let plan = graph
            .route_plan(&RouteRequestContext::default())
            .expect("compile request plan");

        assert_eq!(
            provider_endpoint_keys(&plan),
            vec!["codex/input/fast", "codex/input/slow"]
        );
        assert_eq!(
            resolved_provider_order("codex", &view).expect("resolve provider order"),
            vec!["input"]
        );
    }

    #[test]
    fn routing_ir_one_provider_matches_resolved_order() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "input".to_string(),
                provider("https://input.example/v1"),
            )]),
            ..ServiceRouteConfig::default()
        };

        let template = compile_route_plan_template("codex", &view).expect("route template");

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
    fn routing_ir_candidate_identity_is_provider_endpoint_scoped() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "input".to_string(),
                provider("https://input.example/v1"),
            )]),
            ..ServiceRouteConfig::default()
        };

        let template = compile_route_plan_template("codex", &view).expect("route template");
        let identities = template.candidate_identities();

        assert_eq!(identities.len(), 1);
        assert_eq!(
            identities[0].provider_endpoint.stable_key(),
            "codex/input/default"
        );
        assert_eq!(identities[0].base_url, "https://input.example/v1");
    }

    #[test]
    fn routing_ir_ordered_failover_matches_resolved_order() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "primary".to_string(),
                    provider("https://primary.example/v1"),
                ),
                ("backup".to_string(), provider("https://backup.example/v1")),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "backup".to_string(),
                "primary".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };

        let template = compile_route_plan_template("codex", &view).expect("route template");

        assert_provider_order_parity(&view, &template);
        assert_eq!(provider_ids(&template), vec!["backup", "primary"]);
    }

    #[test]
    fn routing_ir_nested_route_graph_preserves_candidate_order_and_path() {
        let view = ServiceRouteConfig {
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
            routing: Some(RouteGraphConfig {
                entry: "monthly_first".to_string(),
                routes: BTreeMap::from([
                    (
                        "monthly_pool".to_string(),
                        RouteNodeConfig {
                            strategy: RouteStrategy::OrderedFailover,
                            children: vec!["input".to_string(), "input1".to_string()],
                            ..RouteNodeConfig::default()
                        },
                    ),
                    (
                        "monthly_first".to_string(),
                        RouteNodeConfig {
                            strategy: RouteStrategy::OrderedFailover,
                            children: vec!["monthly_pool".to_string(), "paygo".to_string()],
                            ..RouteNodeConfig::default()
                        },
                    ),
                ]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        };

        let template = compile_route_plan_template("codex", &view).expect("route template");

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
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "primary".to_string(),
                    provider("https://primary.example/v1"),
                ),
                ("backup".to_string(), provider("https://backup.example/v1")),
            ]),
            routing: Some(RouteGraphConfig::manual_sticky(
                "backup".to_string(),
                vec!["backup".to_string(), "primary".to_string()],
            )),
            ..ServiceRouteConfig::default()
        };

        let template = compile_route_plan_template("codex", &view).expect("route template");

        assert_provider_order_parity(&view, &template);
        assert_eq!(provider_ids(&template), vec!["backup"]);
        assert_eq!(template.candidates[0].route_path, vec!["main", "backup"]);
    }

    #[test]
    fn routing_ir_tag_preferred_continue_matches_resolved_order() {
        let view = ServiceRouteConfig {
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
            routing: Some(RouteGraphConfig::tag_preferred(
                vec!["paygo".to_string(), "monthly".to_string()],
                vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                RouteExhaustedAction::Continue,
            )),
            ..ServiceRouteConfig::default()
        };

        let template = compile_route_plan_template("codex", &view).expect("route template");

        assert_provider_order_parity(&view, &template);
        assert_eq!(provider_ids(&template), vec!["monthly", "paygo"]);
        assert_eq!(
            provider_preference_groups(&template),
            vec![("monthly".to_string(), 0), ("paygo".to_string(), 1)]
        );
        assert_eq!(template.candidates[0].preference_group, 0);
        assert_eq!(template.candidates[1].preference_group, 1);
    }

    #[test]
    fn routing_ir_tag_preferred_stop_matches_resolved_order() {
        let view = ServiceRouteConfig {
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
            routing: Some(RouteGraphConfig::tag_preferred(
                vec!["paygo".to_string(), "monthly".to_string()],
                vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                RouteExhaustedAction::Stop,
            )),
            ..ServiceRouteConfig::default()
        };

        let template = compile_route_plan_template("codex", &view).expect("route template");

        assert_provider_order_parity(&view, &template);
        assert_eq!(provider_ids(&template), vec!["monthly"]);
    }

    #[test]
    fn routing_ir_tag_preferred_marks_nested_preference_groups() {
        let view = ServiceRouteConfig {
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
            routing: Some(RouteGraphConfig {
                entry: "monthly_first".to_string(),
                routes: BTreeMap::from([
                    (
                        "monthly_pool".to_string(),
                        RouteNodeConfig {
                            strategy: RouteStrategy::OrderedFailover,
                            children: vec!["monthly-a".to_string(), "monthly-b".to_string()],
                            ..RouteNodeConfig::default()
                        },
                    ),
                    (
                        "monthly_first".to_string(),
                        RouteNodeConfig {
                            strategy: RouteStrategy::TagPreferred,
                            children: vec!["chili".to_string(), "monthly_pool".to_string()],
                            prefer_tags: vec![BTreeMap::from([(
                                "billing".to_string(),
                                "monthly".to_string(),
                            )])],
                            on_exhausted: RouteExhaustedAction::Continue,
                            ..RouteNodeConfig::default()
                        },
                    ),
                ]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        };

        let template = compile_route_plan_template("codex", &view).expect("route template");

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
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                ("small".to_string(), provider("https://small.example/v1")),
                ("large".to_string(), provider("https://large.example/v1")),
            ]),
            routing: Some(RouteGraphConfig {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RouteNodeConfig {
                        strategy: RouteStrategy::Conditional,
                        when: Some(RouteCondition {
                            model: Some("gpt-5".to_string()),
                            ..RouteCondition::default()
                        }),
                        then: Some("large".to_string()),
                        default_route: Some("small".to_string()),
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        };

        let template = compile_route_plan_template_with_request(
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
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                ("small".to_string(), provider("https://small.example/v1")),
                ("large".to_string(), provider("https://large.example/v1")),
            ]),
            routing: Some(RouteGraphConfig {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RouteNodeConfig {
                        strategy: RouteStrategy::Conditional,
                        when: Some(RouteCondition {
                            service_tier: Some("priority".to_string()),
                            ..RouteCondition::default()
                        }),
                        then: Some("large".to_string()),
                        default_route: Some("small".to_string()),
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        };

        let template = compile_route_plan_template_with_request(
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
        let view = ServiceRouteConfig {
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
            routing: Some(RouteGraphConfig {
                entry: "root".to_string(),
                routes: BTreeMap::from([
                    (
                        "root".to_string(),
                        RouteNodeConfig {
                            strategy: RouteStrategy::Conditional,
                            when: Some(RouteCondition {
                                model: Some("gpt-5".to_string()),
                                ..RouteCondition::default()
                            }),
                            then: Some("large_pool".to_string()),
                            default_route: Some("small".to_string()),
                            ..RouteNodeConfig::default()
                        },
                    ),
                    (
                        "large_pool".to_string(),
                        RouteNodeConfig {
                            strategy: RouteStrategy::OrderedFailover,
                            children: vec!["large-primary".to_string(), "large-backup".to_string()],
                            ..RouteNodeConfig::default()
                        },
                    ),
                ]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        };

        let template = compile_route_plan_template_with_request(
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
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                ("small".to_string(), provider("https://small.example/v1")),
                ("large".to_string(), provider("https://large.example/v1")),
            ]),
            routing: Some(RouteGraphConfig {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RouteNodeConfig {
                        strategy: RouteStrategy::Conditional,
                        when: Some(RouteCondition {
                            model: Some("gpt-5".to_string()),
                            ..RouteCondition::default()
                        }),
                        then: Some("large".to_string()),
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        };

        let err = compile_route_plan_template_with_request(
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
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                ("small".to_string(), provider("https://small.example/v1")),
                ("large".to_string(), provider("https://large.example/v1")),
            ]),
            routing: Some(RouteGraphConfig {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RouteNodeConfig {
                        strategy: RouteStrategy::Conditional,
                        when: Some(RouteCondition::default()),
                        then: Some("large".to_string()),
                        default_route: Some("small".to_string()),
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        };

        let err = compile_route_plan_template_with_request(
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
    fn routing_ir_handshake_plan_contains_all_conditional_branches() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                ("small".to_string(), provider("https://small.example/v1")),
                ("large".to_string(), provider("https://large.example/v1")),
            ]),
            routing: Some(RouteGraphConfig {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RouteNodeConfig {
                        strategy: RouteStrategy::Conditional,
                        when: Some(RouteCondition {
                            model: Some("gpt-5".to_string()),
                            ..RouteCondition::default()
                        }),
                        then: Some("large".to_string()),
                        default_route: Some("small".to_string()),
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        };
        let plan = compile_route_handshake_plan("codex", &view).expect("handshake plan");

        assert_eq!(
            plan.candidates
                .iter()
                .map(|candidate| candidate.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec!["large", "small"]
        );
    }

    #[test]
    fn routing_ir_candidate_expands_provider_endpoints_in_runtime_order() {
        let mut endpoints = BTreeMap::new();
        endpoints.insert(
            "slow".to_string(),
            ProviderEndpointConfig {
                base_url: "https://slow.example/v1".to_string(),
                continuity_domain: None,
                enabled: true,
                priority: 10,
                tags: BTreeMap::from([("region".to_string(), "us".to_string())]),
                supported_models: BTreeMap::from([("gpt-4.1".to_string(), true)]),
                model_mapping: BTreeMap::new(),
                limits: ProviderConcurrencyLimits::default(),
            },
        );
        endpoints.insert(
            "fast".to_string(),
            ProviderEndpointConfig {
                base_url: "https://fast.example/v1".to_string(),
                continuity_domain: None,
                enabled: true,
                priority: 0,
                tags: BTreeMap::from([("region".to_string(), "hk".to_string())]),
                supported_models: BTreeMap::new(),
                model_mapping: BTreeMap::from([(
                    "gpt-5".to_string(),
                    "provider-gpt-5".to_string(),
                )]),
                limits: ProviderConcurrencyLimits::default(),
            },
        );
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfig {
                    tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                    supported_models: BTreeMap::from([("gpt-5".to_string(), true)]),
                    endpoints,
                    ..ProviderConfig::default()
                },
            )]),
            ..ServiceRouteConfig::default()
        };

        let template = compile_route_plan_template("codex", &view).expect("route template");

        assert_eq!(provider_ids(&template), vec!["input", "input"]);
        assert_eq!(template.candidates[0].endpoint_id, "fast");
        assert_eq!(template.candidates[1].endpoint_id, "slow");
        assert_eq!(
            provider_endpoint_keys(&template),
            vec!["codex/input/fast", "codex/input/slow"]
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
    fn routing_ir_provider_concurrency_limit_compiles_to_default_endpoint() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "relay".to_string(),
                ProviderConfig {
                    base_url: Some("https://relay.example/v1".to_string()),
                    limits: ProviderConcurrencyLimits {
                        max_concurrent_requests: Some(5),
                        limit_group: Some(" relay-account ".to_string()),
                    },
                    ..ProviderConfig::default()
                },
            )]),
            ..ServiceRouteConfig::default()
        };

        let template = compile_route_plan_template("codex", &view).expect("route template");
        let concurrency = &template.candidates[0].concurrency;

        assert_eq!(concurrency.max_concurrent_requests, Some(5));
        assert_eq!(concurrency.limit_group.as_deref(), Some("relay-account"));
        assert_eq!(
            concurrency.limit_key(
                "codex",
                &template.candidate_provider_endpoint_key(&template.candidates[0])
            ),
            Some("group:codex/relay-account".to_string())
        );
    }

    #[test]
    fn routing_ir_continuity_domain_defaults_to_endpoint_and_supports_explicit_overrides() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "opaque".to_string(),
                    ProviderConfig {
                        base_url: Some("https://same.example/v1".to_string()),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "shared".to_string(),
                    ProviderConfig {
                        continuity_domain: Some(" shared-cluster ".to_string()),
                        endpoints: BTreeMap::from([
                            (
                                "default".to_string(),
                                ProviderEndpointConfig {
                                    base_url: "https://shared-default.example/v1".to_string(),
                                    continuity_domain: None,
                                    enabled: true,
                                    priority: 0,
                                    tags: BTreeMap::new(),
                                    supported_models: BTreeMap::new(),
                                    model_mapping: BTreeMap::new(),
                                    limits: ProviderConcurrencyLimits::default(),
                                },
                            ),
                            (
                                "isolated".to_string(),
                                ProviderEndpointConfig {
                                    base_url: "https://shared-isolated.example/v1".to_string(),
                                    continuity_domain: Some("isolated-cluster".to_string()),
                                    enabled: true,
                                    priority: 1,
                                    tags: BTreeMap::new(),
                                    supported_models: BTreeMap::new(),
                                    model_mapping: BTreeMap::new(),
                                    limits: ProviderConcurrencyLimits::default(),
                                },
                            ),
                        ]),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "opaque".to_string(),
                "shared".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let domains = template
            .candidates
            .iter()
            .map(|candidate| {
                (
                    format!("{}/{}", candidate.provider_id, candidate.endpoint_id),
                    template
                        .candidate_continuity_domain_key(candidate)
                        .stable_key(),
                    candidate.continuity_domain.as_deref(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            domains,
            vec![
                (
                    "opaque/default".to_string(),
                    "provider_endpoint:codex/opaque/default".to_string(),
                    None,
                ),
                (
                    "shared/default".to_string(),
                    "explicit:codex/shared-cluster".to_string(),
                    Some("shared-cluster"),
                ),
                (
                    "shared/isolated".to_string(),
                    "explicit:codex/isolated-cluster".to_string(),
                    Some("isolated-cluster"),
                ),
            ]
        );
    }

    #[test]
    fn routing_ir_continuity_topology_summarizes_endpoint_and_domain_counts() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "alpha".to_string(),
                    ProviderConfig {
                        continuity_domain: Some("shared-relay".to_string()),
                        endpoints: BTreeMap::from([
                            (
                                "one".to_string(),
                                ProviderEndpointConfig {
                                    base_url: "https://alpha-one.example/v1".to_string(),
                                    continuity_domain: None,
                                    enabled: true,
                                    priority: 0,
                                    tags: BTreeMap::new(),
                                    supported_models: BTreeMap::new(),
                                    model_mapping: BTreeMap::new(),
                                    limits: ProviderConcurrencyLimits::default(),
                                },
                            ),
                            (
                                "two".to_string(),
                                ProviderEndpointConfig {
                                    base_url: "https://alpha-two.example/v1".to_string(),
                                    continuity_domain: None,
                                    enabled: true,
                                    priority: 1,
                                    tags: BTreeMap::new(),
                                    supported_models: BTreeMap::new(),
                                    model_mapping: BTreeMap::new(),
                                    limits: ProviderConcurrencyLimits::default(),
                                },
                            ),
                        ]),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "beta".to_string(),
                    ProviderConfig {
                        base_url: Some("https://beta.example/v1".to_string()),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "alpha".to_string(),
                "beta".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let topology = template.continuity_topology();

        assert_eq!(topology.configured_provider_endpoint_count(), 3);

        let alpha_summary = topology
            .selected_domain_summary("codex/alpha/one")
            .expect("alpha domain summary");
        assert_eq!(
            alpha_summary.domain.stable_key(),
            "explicit:codex/shared-relay"
        );
        assert_eq!(alpha_summary.same_domain_endpoint_count, 2);

        let beta_summary = topology
            .selected_domain_summary("codex/beta/default")
            .expect("beta domain summary");
        assert_eq!(
            beta_summary.domain.stable_key(),
            "provider_endpoint:codex/beta/default"
        );
        assert_eq!(beta_summary.same_domain_endpoint_count, 1);
        assert!(
            topology
                .selected_domain_summary("codex/missing/default")
                .is_none()
        );
    }

    #[test]
    fn routing_ir_continuity_topology_counts_unique_provider_endpoints_not_route_occurrences() {
        let template = RoutePlanTemplate {
            service_name: "codex".to_string(),
            entry: "main".to_string(),
            affinity_policy: RouteAffinityPolicy::FallbackSticky,
            scheduling_preset: SchedulingPreset::Balanced,
            fallback_ttl_ms: None,
            reprobe_preferred_after_ms: None,
            nodes: BTreeMap::new(),
            expanded_provider_order: vec!["relay".to_string()],
            candidates: vec![
                RouteCandidate {
                    provider_id: "relay".to_string(),
                    provider_alias: None,
                    endpoint_id: "default".to_string(),
                    base_url: "https://relay.example/v1".to_string(),
                    continuity_domain: Some("relay-cluster".to_string()),
                    auth: UpstreamAuth::default(),
                    tags: BTreeMap::new(),
                    supported_models: BTreeMap::new(),
                    model_mapping: BTreeMap::new(),
                    model_rules: Arc::default(),
                    route_path: vec!["main".to_string(), "preferred".to_string()],
                    preference_group: 0,
                    stable_index: 0,
                    concurrency: RouteCandidateConcurrency::default(),
                },
                RouteCandidate {
                    provider_id: "relay".to_string(),
                    provider_alias: None,
                    endpoint_id: "default".to_string(),
                    base_url: "https://relay.example/v1".to_string(),
                    continuity_domain: Some("relay-cluster".to_string()),
                    auth: UpstreamAuth::default(),
                    tags: BTreeMap::new(),
                    supported_models: BTreeMap::new(),
                    model_mapping: BTreeMap::new(),
                    model_rules: Arc::default(),
                    route_path: vec!["main".to_string(), "fallback".to_string()],
                    preference_group: 1,
                    stable_index: 1,
                    concurrency: RouteCandidateConcurrency::default(),
                },
            ],
        };
        assert_eq!(
            template
                .candidates
                .iter()
                .filter(|candidate| candidate.provider_id == "relay")
                .count(),
            2,
            "route graph intentionally expands the same provider endpoint twice"
        );

        let topology = template.continuity_topology();
        assert_eq!(topology.configured_provider_endpoint_count(), 1);
        let summary = topology
            .selected_domain_summary("codex/relay/default")
            .expect("relay summary");
        assert_eq!(summary.domain.stable_key(), "explicit:codex/relay-cluster");
        assert_eq!(summary.same_domain_endpoint_count, 1);
    }

    #[test]
    fn routing_ir_endpoint_concurrency_limit_overrides_provider_limit() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "relay".to_string(),
                ProviderConfig {
                    limits: ProviderConcurrencyLimits {
                        max_concurrent_requests: Some(5),
                        limit_group: Some("relay-account".to_string()),
                    },
                    endpoints: BTreeMap::from([(
                        "hk".to_string(),
                        ProviderEndpointConfig {
                            base_url: "https://hk.relay.example/v1".to_string(),
                            continuity_domain: None,
                            enabled: true,
                            priority: 0,
                            tags: BTreeMap::new(),
                            supported_models: BTreeMap::new(),
                            model_mapping: BTreeMap::new(),
                            limits: ProviderConcurrencyLimits {
                                max_concurrent_requests: Some(2),
                                limit_group: Some("relay-hk".to_string()),
                            },
                        },
                    )]),
                    ..ProviderConfig::default()
                },
            )]),
            ..ServiceRouteConfig::default()
        };

        let template = compile_route_plan_template("codex", &view).expect("route template");
        let concurrency = &template.candidates[0].concurrency;

        assert_eq!(concurrency.max_concurrent_requests, Some(2));
        assert_eq!(concurrency.limit_group.as_deref(), Some("relay-hk"));
        assert_eq!(
            concurrency.limit_key(
                "codex",
                &template.candidate_provider_endpoint_key(&template.candidates[0])
            ),
            Some("group:codex/relay-hk".to_string())
        );
    }

    #[test]
    fn routing_ir_default_concurrency_is_unlimited_and_keyed_by_provider_endpoint_without_group() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "relay".to_string(),
                ProviderConfig {
                    base_url: Some("https://relay.example/v1".to_string()),
                    ..ProviderConfig::default()
                },
            )]),
            ..ServiceRouteConfig::default()
        };

        let template = compile_route_plan_template("codex", &view).expect("route template");
        let concurrency = &template.candidates[0].concurrency;
        let provider_endpoint = template.candidate_provider_endpoint_key(&template.candidates[0]);

        assert_eq!(concurrency.max_concurrent_requests, None);
        assert_eq!(concurrency.limit_group, None);
        assert_eq!(concurrency.limit_key("codex", &provider_endpoint), None);

        let grouped = RouteCandidateConcurrency {
            max_concurrent_requests: Some(3),
            limit_group: None,
        };
        assert_eq!(
            grouped.limit_key("codex", &provider_endpoint),
            Some("endpoint:codex/relay/default".to_string())
        );
    }

    #[test]
    fn routing_ir_rejects_different_limits_for_the_same_explicit_group() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "primary".to_string(),
                    ProviderConfig {
                        base_url: Some("https://primary.example/v1".to_string()),
                        limits: ProviderConcurrencyLimits {
                            max_concurrent_requests: Some(1),
                            limit_group: Some("shared-account".to_string()),
                        },
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "backup".to_string(),
                    ProviderConfig {
                        base_url: Some("https://backup.example/v1".to_string()),
                        limits: ProviderConcurrencyLimits {
                            max_concurrent_requests: Some(2),
                            limit_group: Some("shared-account".to_string()),
                        },
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            ..ServiceRouteConfig::default()
        };

        let error = CompiledRouteGraph::compile("codex", &view)
            .expect_err("one capacity group cannot have two limits");
        let message = error.to_string();
        assert!(message.contains("shared-account"), "unexpected: {message}");
        assert!(message.contains("1"), "unexpected: {message}");
        assert!(message.contains("2"), "unexpected: {message}");
    }

    #[test]
    fn routing_ir_rejects_missing_limit_in_an_explicit_shared_group() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "bounded".to_string(),
                    ProviderConfig {
                        base_url: Some("https://bounded.example/v1".to_string()),
                        limits: ProviderConcurrencyLimits {
                            max_concurrent_requests: Some(20),
                            limit_group: Some("shared-account".to_string()),
                        },
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "unbounded".to_string(),
                    ProviderConfig {
                        base_url: Some("https://unbounded.example/v1".to_string()),
                        limits: ProviderConcurrencyLimits {
                            max_concurrent_requests: None,
                            limit_group: Some("shared-account".to_string()),
                        },
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            ..ServiceRouteConfig::default()
        };

        let error = CompiledRouteGraph::compile("codex", &view)
            .expect_err("every candidate in one capacity group must declare the same limit");
        let message = error.to_string();
        assert!(message.contains("shared-account"), "unexpected: {message}");
        assert!(message.contains("20"), "unexpected: {message}");
        assert!(message.contains("missing"), "unexpected: {message}");
    }

    #[test]
    fn routing_ir_manual_sticky_can_target_provider_endpoint() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfig {
                    endpoints: BTreeMap::from([
                        (
                            "fast".to_string(),
                            ProviderEndpointConfig {
                                base_url: "https://fast.example/v1".to_string(),
                                continuity_domain: None,
                                enabled: true,
                                priority: 0,
                                tags: BTreeMap::new(),
                                supported_models: BTreeMap::new(),
                                model_mapping: BTreeMap::new(),
                                limits: ProviderConcurrencyLimits::default(),
                            },
                        ),
                        (
                            "slow".to_string(),
                            ProviderEndpointConfig {
                                base_url: "https://slow.example/v1".to_string(),
                                continuity_domain: None,
                                enabled: true,
                                priority: 10,
                                tags: BTreeMap::new(),
                                supported_models: BTreeMap::new(),
                                model_mapping: BTreeMap::new(),
                                limits: ProviderConcurrencyLimits::default(),
                            },
                        ),
                    ]),
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(RouteGraphConfig {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RouteNodeConfig {
                        strategy: RouteStrategy::ManualSticky,
                        target: Some("input.slow".to_string()),
                        children: vec!["input".to_string()],
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        };

        let template = compile_route_plan_template("codex", &view).expect("route template");

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
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfig {
                    endpoints: BTreeMap::from([(
                        "fast".to_string(),
                        ProviderEndpointConfig {
                            base_url: "https://fast.example/v1".to_string(),
                            continuity_domain: None,
                            enabled: false,
                            priority: 0,
                            tags: BTreeMap::new(),
                            supported_models: BTreeMap::new(),
                            model_mapping: BTreeMap::new(),
                            limits: ProviderConcurrencyLimits::default(),
                        },
                    )]),
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(RouteGraphConfig {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RouteNodeConfig {
                        strategy: RouteStrategy::ManualSticky,
                        target: Some("input.fast".to_string()),
                        children: vec!["input".to_string()],
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        };

        let error = compile_route_plan_template("codex", &view).expect_err("disabled endpoint");

        assert!(
            error
                .to_string()
                .contains("targets disabled provider endpoint 'input.fast'")
        );
    }

    #[test]
    fn route_plan_executor_explains_structured_skip_reasons() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "unsupported".to_string(),
                    ProviderConfig {
                        base_url: Some("https://unsupported.example/v1".to_string()),
                        supported_models: BTreeMap::from([("gpt-4.1".to_string(), true)]),
                        ..ProviderConfig::default()
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
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "unsupported".to_string(),
                "disabled".to_string(),
                "cooldown".to_string(),
                "breaker".to_string(),
                "usage".to_string(),
                "missing-auth".to_string(),
                "healthy".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
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
    fn route_plan_executor_skips_saturated_candidate_without_failure_penalty() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "primary".to_string(),
                    limited_provider("https://primary.example/v1", 5),
                ),
                ("backup".to_string(), provider("https://backup.example/v1")),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "primary".to_string(),
                "backup".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        assert_eq!(
            template.candidates[0].concurrency.max_concurrent_requests,
            Some(5)
        );

        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_provider_endpoint(
            endpoint_key("codex", "primary", "default"),
            RoutePlanUpstreamRuntimeState {
                concurrency_saturated: true,
                concurrency_active: Some(5),
                concurrency_limit: Some(5),
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );

        let explanations =
            executor.explain_candidate_skip_reasons_with_runtime_state(&runtime, None);
        assert_eq!(explanations.len(), 1);
        assert_eq!(explanations[0].candidate.provider_id, "primary");
        assert_eq!(
            explanations[0]
                .reasons
                .iter()
                .map(RoutePlanSkipReason::code)
                .collect::<Vec<_>>(),
            vec!["concurrency_saturated"]
        );

        let mut state = RoutePlanAttemptState::default();
        let selection =
            executor.select_supported_candidate_with_runtime_state(&mut state, &runtime, None);
        let selected = selection.selected.expect("fallback candidate selected");

        assert_eq!(selected.candidate.provider_id, "backup");
        assert_eq!(selection.avoided_candidate_indices, Vec::<usize>::new());
    }

    #[test]
    fn route_plan_candidate_runtime_snapshot_reports_structured_availability() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "primary".to_string(),
                ProviderConfig {
                    base_url: Some("https://primary.example/v1".to_string()),
                    supported_models: BTreeMap::from([("gpt-4.1".to_string(), true)]),
                    limits: ProviderConcurrencyLimits {
                        max_concurrent_requests: Some(2),
                        limit_group: Some("shared".to_string()),
                    },
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "primary".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let candidate = &template.candidates[0];
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_provider_endpoint(
            endpoint_key("codex", "primary", "default"),
            RoutePlanUpstreamRuntimeState {
                cooldown_active: true,
                cooldown_remaining_secs: Some(30),
                usage_exhausted: true,
                missing_auth: true,
                concurrency_saturated: true,
                concurrency_active: Some(2),
                concurrency_limit: Some(2),
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );

        let snapshot = runtime.candidate_runtime_snapshot(&template, candidate);

        assert!(!snapshot.runtime_available);
        assert!(!snapshot.routable_except_usage);
        assert!(snapshot.hard_unavailable);
        assert!(snapshot.cooldown_active);
        assert!(snapshot.breaker_open);
        assert_eq!(snapshot.cooldown_remaining_secs, Some(30));
        assert!(snapshot.usage_exhausted);
        assert!(snapshot.missing_auth);
        assert!(snapshot.concurrency_saturated);
        assert_eq!(snapshot.concurrency_active, Some(2));
        assert_eq!(snapshot.concurrency_limit, Some(2));
        assert_eq!(snapshot.effective_max_concurrent_requests, Some(2));
        assert_eq!(snapshot.effective_limit_group.as_deref(), Some("shared"));
        assert_eq!(
            snapshot
                .skip_reasons_for_candidate(candidate, Some("gpt-5"))
                .iter()
                .map(RoutePlanSkipReason::code)
                .collect::<Vec<_>>(),
            vec![
                "unsupported_model",
                "cooldown",
                "usage_exhausted",
                "missing_auth",
                "concurrency_saturated"
            ]
        );
        assert_eq!(
            snapshot
                .dominant_runtime_skip_reason()
                .as_ref()
                .map(RoutePlanSkipReason::code),
            Some("cooldown")
        );
    }

    #[test]
    fn route_plan_executor_default_fallback_sticky_keeps_lower_preference_affinity() {
        let view = ServiceRouteConfig {
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
            routing: Some(RouteGraphConfig::tag_preferred(
                vec!["chili".to_string(), "monthly".to_string()],
                vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                RouteExhaustedAction::Continue,
            )),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_affinity_provider_endpoint(Some(endpoint_key("codex", "chili", "default")));
        let mut state = RoutePlanAttemptState::default();

        let selection =
            executor.select_supported_candidate_with_runtime_state(&mut state, &runtime, None);
        let selected = selection
            .selected
            .expect("sticky fallback candidate selected");

        assert_eq!(
            template.affinity_policy,
            RouteAffinityPolicy::FallbackSticky
        );
        assert_eq!(
            provider_preference_groups(&template),
            vec![("monthly".to_string(), 0), ("chili".to_string(), 1),]
        );
        assert_eq!(selected.candidate.provider_id, "chili");
        assert_eq!(
            selected.provider_endpoint,
            endpoint_key("codex", "chili", "default")
        );
    }

    #[test]
    fn route_plan_executor_default_fallback_sticky_keeps_lower_order_affinity() {
        let view = ServiceRouteConfig {
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
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "monthly".to_string(),
                "chili".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_affinity_provider_endpoint(Some(endpoint_key("codex", "chili", "default")));
        let mut state = RoutePlanAttemptState::default();

        let selection =
            executor.select_supported_candidate_with_runtime_state(&mut state, &runtime, None);
        let selected = selection
            .selected
            .expect("sticky fallback candidate selected");

        assert_eq!(
            template.affinity_policy,
            RouteAffinityPolicy::FallbackSticky
        );
        assert_eq!(
            provider_preference_groups(&template),
            vec![("monthly".to_string(), 0), ("chili".to_string(), 1)]
        );
        assert_eq!(selected.candidate.provider_id, "chili");
        assert_eq!(
            selected.provider_endpoint,
            endpoint_key("codex", "chili", "default")
        );
    }

    #[test]
    fn route_plan_executor_preferred_group_opt_in_prefers_primary() {
        let mut routing =
            RouteGraphConfig::ordered_failover(vec!["monthly".to_string(), "chili".to_string()]);
        routing.affinity_policy = RouteAffinityPolicy::PreferredGroup;
        let view = ServiceRouteConfig {
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
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_affinity_provider_endpoint(Some(endpoint_key("codex", "chili", "default")));
        let mut state = RoutePlanAttemptState::default();

        let selection =
            executor.select_supported_candidate_with_runtime_state(&mut state, &runtime, None);
        let selected = selection.selected.expect("ordered primary selected");

        assert_eq!(
            template.affinity_policy,
            RouteAffinityPolicy::PreferredGroup
        );
        assert_eq!(selected.candidate.provider_id, "monthly");
        assert_eq!(
            selected.provider_endpoint,
            endpoint_key("codex", "monthly", "default")
        );
    }

    #[test]
    fn route_plan_executor_affinity_selection_ignores_preferred_group_primary() {
        let mut routing =
            RouteGraphConfig::ordered_failover(vec!["monthly".to_string(), "chili".to_string()]);
        routing.affinity_policy = RouteAffinityPolicy::PreferredGroup;
        let view = ServiceRouteConfig {
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
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_affinity_provider_endpoint(Some(endpoint_key("codex", "chili", "default")));

        let normal = executor.select_supported_candidate_with_runtime_state(
            &mut RoutePlanAttemptState::default(),
            &runtime,
            None,
        );
        let normal_selected = normal.selected.expect("normal primary selected");
        assert_eq!(normal_selected.candidate.provider_id, "monthly");

        let affinity = executor.select_affinity_candidate_with_runtime_state(
            &mut RoutePlanAttemptState::default(),
            &runtime,
            None,
        );
        let affinity_selected = affinity.selected.expect("affinity candidate selected");
        assert_eq!(affinity_selected.candidate.provider_id, "chili");
        assert_eq!(
            affinity_selected.provider_endpoint,
            endpoint_key("codex", "chili", "default")
        );
    }

    #[test]
    fn route_plan_executor_affinity_selection_stops_when_affinity_is_unavailable() {
        let mut routing =
            RouteGraphConfig::ordered_failover(vec!["monthly".to_string(), "chili".to_string()]);
        routing.affinity_policy = RouteAffinityPolicy::PreferredGroup;
        let view = ServiceRouteConfig {
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
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_affinity_provider_endpoint(Some(endpoint_key("codex", "chili", "default")));
        runtime.set_provider_endpoint(
            endpoint_key("codex", "chili", "default"),
            RoutePlanUpstreamRuntimeState {
                cooldown_active: true,
                cooldown_remaining_secs: Some(5),
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );

        let normal = executor.select_supported_candidate_with_runtime_state(
            &mut RoutePlanAttemptState::default(),
            &runtime,
            None,
        );
        let normal_selected = normal.selected.expect("normal primary selected");
        assert_eq!(normal_selected.candidate.provider_id, "monthly");

        let affinity = executor.select_affinity_candidate_with_runtime_state(
            &mut RoutePlanAttemptState::default(),
            &runtime,
            None,
        );
        assert!(affinity.selected.is_none());
    }

    #[test]
    fn route_plan_executor_uses_fallback_group_only_when_preferred_usage_exhausted() {
        let view = ServiceRouteConfig {
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
            routing: Some(RouteGraphConfig::tag_preferred(
                vec!["chili".to_string(), "monthly".to_string()],
                vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                RouteExhaustedAction::Continue,
            )),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
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
    fn route_plan_executor_stops_when_all_candidates_usage_exhausted() {
        let view = ServiceRouteConfig {
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
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "monthly".to_string(),
                "chili".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_provider_endpoint(
            endpoint_key("codex", "monthly", "default"),
            RoutePlanUpstreamRuntimeState {
                usage_exhausted: true,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        runtime.set_provider_endpoint(
            endpoint_key("codex", "chili", "default"),
            RoutePlanUpstreamRuntimeState {
                usage_exhausted: true,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        let mut state = RoutePlanAttemptState::default();

        let selection =
            executor.select_supported_candidate_with_runtime_state(&mut state, &runtime, None);

        assert!(
            selection.selected.is_none(),
            "trusted usage exhaustion should make the route temporarily unroutable"
        );
        let reasons = executor
            .explain_candidate_skip_reasons_with_runtime_state(&runtime, None)
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
            reasons.get("monthly").cloned(),
            Some(vec!["usage_exhausted"])
        );
        assert_eq!(reasons.get("chili").cloned(), Some(vec!["usage_exhausted"]));
    }

    #[test]
    fn route_plan_executor_uses_fallback_group_when_preferred_is_hard_unavailable() {
        let view = ServiceRouteConfig {
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
            routing: Some(RouteGraphConfig::tag_preferred(
                vec!["chili".to_string(), "monthly".to_string()],
                vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                RouteExhaustedAction::Continue,
            )),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
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
        let mut routing = RouteGraphConfig::tag_preferred(
            vec!["chili".to_string(), "monthly".to_string()],
            vec![BTreeMap::from([(
                "billing".to_string(),
                "monthly".to_string(),
            )])],
            RouteExhaustedAction::Continue,
        );
        routing.affinity_policy = RouteAffinityPolicy::FallbackSticky;
        let view = ServiceRouteConfig {
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
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
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
            RouteAffinityPolicy::FallbackSticky
        );
        assert_eq!(selected.candidate.provider_id, "chili");
        assert_eq!(
            selected.provider_endpoint,
            endpoint_key("codex", "chili", "default")
        );
    }

    #[test]
    fn route_plan_executor_fallback_sticky_reprobes_preferred_after_window() {
        let mut routing = RouteGraphConfig::tag_preferred(
            vec!["chili".to_string(), "monthly".to_string()],
            vec![BTreeMap::from([(
                "billing".to_string(),
                "monthly".to_string(),
            )])],
            RouteExhaustedAction::Continue,
        );
        routing.affinity_policy = RouteAffinityPolicy::FallbackSticky;
        routing.reprobe_preferred_after_ms = Some(1);
        let view = ServiceRouteConfig {
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
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
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
        let mut routing = RouteGraphConfig::tag_preferred(
            vec!["chili".to_string(), "monthly".to_string()],
            vec![BTreeMap::from([(
                "billing".to_string(),
                "monthly".to_string(),
            )])],
            RouteExhaustedAction::Continue,
        );
        routing.affinity_policy = RouteAffinityPolicy::FallbackSticky;
        routing.fallback_ttl_ms = Some(1);
        let view = ServiceRouteConfig {
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
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
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
        let mut routing = RouteGraphConfig::tag_preferred(
            vec!["monthly-a".to_string(), "monthly-b".to_string()],
            vec![BTreeMap::from([(
                "billing".to_string(),
                "monthly".to_string(),
            )])],
            RouteExhaustedAction::Continue,
        );
        routing.affinity_policy = RouteAffinityPolicy::Off;
        let view = ServiceRouteConfig {
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
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_affinity_provider_endpoint(Some(endpoint_key("codex", "monthly-b", "default")));
        let mut state = RoutePlanAttemptState::default();

        let selection =
            executor.select_supported_candidate_with_runtime_state(&mut state, &runtime, None);
        let selected = selection.selected.expect("first candidate selected");

        assert_eq!(template.affinity_policy, RouteAffinityPolicy::Off);
        assert_eq!(selected.candidate.provider_id, "monthly-a");
        assert_eq!(
            selected.provider_endpoint,
            endpoint_key("codex", "monthly-a", "default")
        );
    }

    #[test]
    fn route_plan_executor_hard_policy_stops_when_affinity_is_unavailable() {
        let mut routing = RouteGraphConfig::tag_preferred(
            vec!["monthly".to_string(), "chili".to_string()],
            vec![BTreeMap::from([(
                "billing".to_string(),
                "monthly".to_string(),
            )])],
            RouteExhaustedAction::Continue,
        );
        routing.affinity_policy = RouteAffinityPolicy::Hard;
        let view = ServiceRouteConfig {
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
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
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

        assert_eq!(template.affinity_policy, RouteAffinityPolicy::Hard);
        assert!(selection.selected.is_none());
    }

    #[test]
    fn route_plan_executor_soft_affinity_escapes_unavailable_hard_affinity() {
        let mut routing = RouteGraphConfig::tag_preferred(
            vec!["monthly".to_string(), "chili".to_string()],
            vec![BTreeMap::from([(
                "billing".to_string(),
                "monthly".to_string(),
            )])],
            RouteExhaustedAction::Continue,
        );
        routing.affinity_policy = RouteAffinityPolicy::Hard;
        let view = ServiceRouteConfig {
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
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
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

        let selection = executor.select_supported_candidate_with_soft_affinity_runtime_state(
            &mut state, &runtime, None,
        );
        let selected = selection.selected.expect("soft fallback selected");

        assert_eq!(template.affinity_policy, RouteAffinityPolicy::Hard);
        assert_eq!(selected.candidate.provider_id, "monthly");
        assert_eq!(
            selected.provider_endpoint,
            endpoint_key("codex", "monthly", "default")
        );
    }

    #[test]
    fn route_plan_executor_keeps_same_candidate_until_caller_marks_avoidance() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                ("first".to_string(), provider("https://first.example/v1")),
                ("second".to_string(), provider("https://second.example/v1")),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "first".to_string(),
                "second".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
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

    #[test]
    fn round_robin_uses_concurrency_capacity_as_weight() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "input".to_string(),
                    limited_provider("https://rr-input.example/v1", 20),
                ),
                (
                    "ciii".to_string(),
                    limited_provider("https://rr-ciii.example/v1", 15),
                ),
            ]),
            routing: Some(RouteGraphConfig::round_robin(vec![
                "input".to_string(),
                "ciii".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        assert_eq!(
            provider_preference_groups(&template),
            vec![("input".to_string(), 0), ("ciii".to_string(), 0)]
        );
        let executor = RoutePlanExecutor::new(&template);
        let runtime = RoutePlanRuntimeState::default();
        let mut counts = BTreeMap::<String, usize>::new();

        for _ in 0..350 {
            let selection = executor.select_supported_candidate_with_runtime_state(
                &mut RoutePlanAttemptState::default(),
                &runtime,
                None,
            );
            let provider_id = selection
                .selected
                .expect("round-robin candidate")
                .candidate
                .provider_id
                .clone();
            *counts.entry(provider_id).or_default() += 1;
        }

        assert_eq!(counts.get("input"), Some(&200));
        assert_eq!(counts.get("ciii"), Some(&150));
    }

    #[test]
    fn new_session_preference_preempts_round_robin_and_falls_back_when_draining() {
        let mut routing =
            RouteGraphConfig::round_robin(vec!["input".to_string(), "ciii".to_string()]);
        routing.affinity_policy = RouteAffinityPolicy::Off;
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "input".to_string(),
                    limited_provider("https://preference-input.example/v1", 20),
                ),
                (
                    "ciii".to_string(),
                    limited_provider("https://preference-ciii.example/v1", 15),
                ),
            ]),
            routing: Some(routing),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let preferred = endpoint_key("codex", "ciii", "default");
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_new_session_preference(Some(preferred.clone()));

        let selected = executor
            .select_supported_candidate_with_runtime_state(
                &mut RoutePlanAttemptState::default(),
                &runtime,
                None,
            )
            .selected
            .expect("preferred candidate");
        assert_eq!(selected.provider_endpoint, preferred);

        runtime.set_provider_endpoint(
            preferred,
            RoutePlanUpstreamRuntimeState {
                draining: true,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        let selected = executor
            .select_supported_candidate_with_runtime_state(
                &mut RoutePlanAttemptState::default(),
                &runtime,
                None,
            )
            .selected
            .expect("automatic fallback candidate");
        assert_eq!(selected.candidate.provider_id, "input");
    }

    #[test]
    fn draining_endpoint_keeps_existing_affinity_but_rejects_new_sessions() {
        let mut routing =
            RouteGraphConfig::round_robin(vec!["input".to_string(), "ciii".to_string()]);
        routing.affinity_policy = RouteAffinityPolicy::Hard;
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "input".to_string(),
                    provider("https://drain-input.example/v1"),
                ),
                (
                    "ciii".to_string(),
                    provider("https://drain-ciii.example/v1"),
                ),
            ]),
            routing: Some(routing),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let draining = endpoint_key("codex", "ciii", "default");
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_provider_endpoint(
            draining.clone(),
            RoutePlanUpstreamRuntimeState {
                draining: true,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );

        let new_session = executor
            .select_supported_candidate_with_runtime_state(
                &mut RoutePlanAttemptState::default(),
                &runtime,
                None,
            )
            .selected
            .expect("new-session candidate");
        assert_eq!(new_session.candidate.provider_id, "input");

        runtime.set_affinity_provider_endpoint(Some(draining.clone()));
        let existing_session = executor
            .select_supported_candidate_with_runtime_state(
                &mut RoutePlanAttemptState::default(),
                &runtime,
                None,
            )
            .selected
            .expect("draining affinity candidate");
        assert_eq!(existing_session.provider_endpoint, draining);
        assert!(executor.candidate_is_valid_after_runtime_update(
            &RoutePlanAttemptState::default(),
            &runtime,
            existing_session.candidate,
            None,
            RouteAffinityPolicy::Hard,
        ));
    }

    #[test]
    fn round_robin_isolates_cursors_by_model_eligibility() {
        let mut input = limited_provider("https://rr-model-input.example/v1", 20);
        input.supported_models = BTreeMap::from([
            ("model-shared".to_string(), true),
            ("model-input-only".to_string(), true),
        ]);
        let mut ciii = limited_provider("https://rr-model-ciii.example/v1", 15);
        ciii.supported_models = BTreeMap::from([("model-shared".to_string(), true)]);
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([("input".to_string(), input), ("ciii".to_string(), ciii)]),
            routing: Some(RouteGraphConfig::round_robin(vec![
                "input".to_string(),
                "ciii".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let runtime = RoutePlanRuntimeState::default();
        let mut shared_counts = BTreeMap::<String, usize>::new();

        for _ in 0..350 {
            let shared = executor.select_supported_candidate_with_runtime_state(
                &mut RoutePlanAttemptState::default(),
                &runtime,
                Some("model-shared"),
            );
            let provider_id = shared
                .selected
                .expect("shared-model round-robin candidate")
                .candidate
                .provider_id
                .clone();
            *shared_counts.entry(provider_id).or_default() += 1;

            let input_only = executor.select_supported_candidate_with_runtime_state(
                &mut RoutePlanAttemptState::default(),
                &runtime,
                Some("model-input-only"),
            );
            assert_eq!(
                input_only
                    .selected
                    .expect("input-only candidate")
                    .candidate
                    .provider_id,
                "input"
            );
        }

        assert_eq!(shared_counts.get("input"), Some(&200));
        assert_eq!(shared_counts.get("ciii"), Some(&150));
    }

    #[test]
    fn round_robin_uses_remaining_active_capacity_as_weight() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "input".to_string(),
                    limited_provider("https://rr-active-input.example/v1", 20),
                ),
                (
                    "ciii".to_string(),
                    limited_provider("https://rr-active-ciii.example/v1", 15),
                ),
            ]),
            routing: Some(RouteGraphConfig::round_robin(vec![
                "input".to_string(),
                "ciii".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_provider_endpoint(
            endpoint_key("codex", "input", "default"),
            RoutePlanUpstreamRuntimeState {
                concurrency_active: Some(10),
                concurrency_limit: Some(20),
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        runtime.set_provider_endpoint(
            endpoint_key("codex", "ciii", "default"),
            RoutePlanUpstreamRuntimeState {
                concurrency_active: Some(0),
                concurrency_limit: Some(15),
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );

        let mut counts = BTreeMap::<String, usize>::new();
        for _ in 0..250 {
            let selection = executor.select_supported_candidate_with_runtime_state(
                &mut RoutePlanAttemptState::default(),
                &runtime,
                None,
            );
            let provider_id = selection
                .selected
                .expect("round-robin candidate")
                .candidate
                .provider_id
                .clone();
            *counts.entry(provider_id).or_default() += 1;
        }

        assert_eq!(counts.get("input"), Some(&100));
        assert_eq!(counts.get("ciii"), Some(&150));
    }

    #[test]
    fn round_robin_uses_runtime_limit_after_config_is_lowered() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "input".to_string(),
                    limited_provider("https://rr-lowered-input.example/v1", 20),
                ),
                (
                    "ciii".to_string(),
                    limited_provider("https://rr-lowered-ciii.example/v1", 15),
                ),
            ]),
            routing: Some(RouteGraphConfig::round_robin(vec![
                "input".to_string(),
                "ciii".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_provider_endpoint(
            endpoint_key("codex", "input", "default"),
            RoutePlanUpstreamRuntimeState {
                concurrency_active: Some(15),
                concurrency_limit: Some(15),
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        runtime.set_provider_endpoint(
            endpoint_key("codex", "ciii", "default"),
            RoutePlanUpstreamRuntimeState {
                concurrency_active: Some(0),
                concurrency_limit: Some(15),
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );

        for _ in 0..30 {
            let selection = executor.select_supported_candidate_with_runtime_state(
                &mut RoutePlanAttemptState::default(),
                &runtime,
                None,
            );
            assert_eq!(
                selection
                    .selected
                    .expect("remaining-capacity candidate")
                    .candidate
                    .provider_id,
                "ciii"
            );
        }
    }

    #[test]
    fn round_robin_counts_shared_limit_group_capacity_once() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "input-a".to_string(),
                    limited_provider_in_group(
                        "https://rr-shared-input-a.example/v1",
                        20,
                        "input-account",
                    ),
                ),
                (
                    "input-b".to_string(),
                    limited_provider_in_group(
                        "https://rr-shared-input-b.example/v1",
                        20,
                        "input-account",
                    ),
                ),
                (
                    "ciii".to_string(),
                    limited_provider("https://rr-shared-ciii.example/v1", 15),
                ),
            ]),
            routing: Some(RouteGraphConfig::round_robin(vec![
                "input-a".to_string(),
                "input-b".to_string(),
                "ciii".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let runtime = RoutePlanRuntimeState::default();
        let mut counts = BTreeMap::<String, usize>::new();

        for _ in 0..350 {
            let selection = executor.select_supported_candidate_with_runtime_state(
                &mut RoutePlanAttemptState::default(),
                &runtime,
                None,
            );
            let provider_id = selection
                .selected
                .expect("round-robin candidate")
                .candidate
                .provider_id
                .clone();
            *counts.entry(provider_id).or_default() += 1;
        }

        assert_eq!(counts.get("input-a"), Some(&100));
        assert_eq!(counts.get("input-b"), Some(&100));
        assert_eq!(counts.get("ciii"), Some(&150));
    }

    #[test]
    fn round_robin_prefers_positive_capacity_over_waiting_on_saturated_group() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "input".to_string(),
                    limited_provider("https://rr-zero-input.example/v1", 20),
                ),
                (
                    "ciii".to_string(),
                    limited_provider("https://rr-zero-ciii.example/v1", 15),
                ),
            ]),
            routing: Some(RouteGraphConfig::round_robin(vec![
                "input".to_string(),
                "ciii".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_provider_endpoint(
            endpoint_key("codex", "input", "default"),
            RoutePlanUpstreamRuntimeState {
                concurrency_saturated: false,
                concurrency_active: Some(20),
                concurrency_limit: Some(20),
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );
        runtime.set_provider_endpoint(
            endpoint_key("codex", "ciii", "default"),
            RoutePlanUpstreamRuntimeState {
                concurrency_active: Some(0),
                concurrency_limit: Some(15),
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );

        for _ in 0..20 {
            let selection = executor.select_supported_candidate_with_runtime_state(
                &mut RoutePlanAttemptState::default(),
                &runtime,
                None,
            );
            assert_eq!(
                selection
                    .selected
                    .expect("positive-capacity candidate")
                    .candidate
                    .provider_id,
                "ciii"
            );
        }
    }

    #[test]
    fn round_robin_skips_saturated_candidate() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "input".to_string(),
                    limited_provider("https://rr-saturated-input.example/v1", 20),
                ),
                (
                    "ciii".to_string(),
                    limited_provider("https://rr-saturated-ciii.example/v1", 15),
                ),
            ]),
            routing: Some(RouteGraphConfig::round_robin(vec![
                "input".to_string(),
                "ciii".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_provider_endpoint(
            endpoint_key("codex", "input", "default"),
            RoutePlanUpstreamRuntimeState {
                concurrency_saturated: true,
                concurrency_active: Some(20),
                concurrency_limit: Some(20),
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );

        for _ in 0..20 {
            let selection = executor.select_supported_candidate_with_runtime_state(
                &mut RoutePlanAttemptState::default(),
                &runtime,
                None,
            );
            assert_eq!(
                selection
                    .selected
                    .expect("unsaturated round-robin candidate")
                    .candidate
                    .provider_id,
                "ciii"
            );
        }
    }

    #[test]
    fn round_robin_revalidation_does_not_advance_cursor() {
        fn compile_round_robin_view(prefix: &str) -> RoutePlanTemplate {
            let mut routing =
                RouteGraphConfig::round_robin(vec!["input".to_string(), "ciii".to_string()]);
            routing.affinity_policy = RouteAffinityPolicy::Off;
            let view = ServiceRouteConfig {
                providers: BTreeMap::from([
                    (
                        "input".to_string(),
                        limited_provider(&format!("https://{prefix}-input.example/v1"), 1),
                    ),
                    (
                        "ciii".to_string(),
                        limited_provider(&format!("https://{prefix}-ciii.example/v1"), 1),
                    ),
                ]),
                routing: Some(routing),
                ..ServiceRouteConfig::default()
            };
            compile_route_plan_template("codex", &view).expect("route template")
        }

        let control_template = compile_round_robin_view("rr-control");
        let control_executor = RoutePlanExecutor::new(&control_template);
        let runtime = RoutePlanRuntimeState::default();
        let mut control_state = RoutePlanAttemptState::default();
        let control_first = control_executor
            .select_supported_candidate_with_runtime_state(&mut control_state, &runtime, None)
            .selected
            .expect("control first candidate")
            .candidate
            .provider_id
            .clone();
        let control_second = control_executor
            .select_supported_candidate_with_runtime_state(&mut control_state, &runtime, None)
            .selected
            .expect("control second candidate")
            .candidate
            .provider_id
            .clone();

        let subject_template = compile_round_robin_view("rr-revalidation");
        let subject_executor = RoutePlanExecutor::new(&subject_template);
        let mut subject_state = RoutePlanAttemptState::default();
        let subject_first_selection = subject_executor
            .select_supported_candidate_with_runtime_state(&mut subject_state, &runtime, None);
        let subject_first = subject_first_selection
            .selected
            .as_ref()
            .expect("subject first candidate")
            .candidate
            .provider_id
            .clone();
        let subject_candidate = subject_first_selection
            .selected
            .as_ref()
            .expect("subject first candidate")
            .candidate;
        for _ in 0..3 {
            assert!(subject_executor.candidate_is_valid_after_runtime_update(
                &subject_state,
                &runtime,
                subject_candidate,
                None,
                RouteAffinityPolicy::Off,
            ));
        }
        let subject_second = subject_executor
            .select_supported_candidate_with_runtime_state(&mut subject_state, &runtime, None)
            .selected
            .expect("subject second candidate")
            .candidate
            .provider_id
            .clone();

        assert_eq!(subject_first, control_first);
        assert_eq!(subject_second, control_second);
    }

    #[test]
    fn round_robin_keeps_available_runtime_affinity_before_cursor() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "input".to_string(),
                    limited_provider("https://affinity-input.example/v1", 20),
                ),
                (
                    "ciii".to_string(),
                    limited_provider("https://affinity-ciii.example/v1", 15),
                ),
            ]),
            routing: Some(RouteGraphConfig::round_robin(vec![
                "input".to_string(),
                "ciii".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_affinity_provider_endpoint(Some(endpoint_key("codex", "ciii", "default")));

        for _ in 0..20 {
            let selection = executor.select_supported_candidate_with_runtime_state(
                &mut RoutePlanAttemptState::default(),
                &runtime,
                None,
            );
            assert_eq!(
                selection
                    .selected
                    .expect("affinity candidate")
                    .candidate
                    .provider_id,
                "ciii"
            );
        }
    }

    #[test]
    fn ordered_failover_does_not_use_round_robin_cursor() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([
                (
                    "input".to_string(),
                    limited_provider("https://ordered-input.example/v1", 20),
                ),
                (
                    "ciii".to_string(),
                    limited_provider("https://ordered-ciii.example/v1", 15),
                ),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "input".to_string(),
                "ciii".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let executor = RoutePlanExecutor::new(&template);

        for _ in 0..20 {
            let selection = executor.select_supported_candidate_with_runtime_state(
                &mut RoutePlanAttemptState::default(),
                &RoutePlanRuntimeState::default(),
                None,
            );
            assert_eq!(
                selection
                    .selected
                    .expect("ordered candidate")
                    .candidate
                    .provider_id,
                "input"
            );
        }
    }
}

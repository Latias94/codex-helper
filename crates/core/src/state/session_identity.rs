use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::logging::RetryInfo;
use crate::policy_actions::PolicyAction;
use crate::pricing::{CostBreakdown, CostConfidence, PriceMultiplier, UsdAmount};
use crate::provider_signals::ProviderSignal;
use crate::quota_pool::{IdentityConfidence, PoolMembership};
use crate::runtime_identity::ProviderEndpointKey;
use crate::runtime_store::AttemptHandle;
use crate::sessions;
use crate::usage::{CacheAccountingConvention, UsageMetrics};

fn bool_is_false(value: &bool) -> bool {
    !*value
}

fn u64_is_zero(value: &u64) -> bool {
    *value == 0
}

const OUTPUT_RATE_SANITY_CEIL: f64 = 5_000.0;

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

fn effective_ttfb_ms_for_request(
    duration_ms: u64,
    ttfb_ms: Option<u64>,
    retry: Option<&RetryInfo>,
) -> Option<u64> {
    let raw_ttfb = ttfb_ms.filter(|value| *value > 0)?;
    let Some(retry) = retry else {
        return Some(raw_ttfb);
    };
    if duration_ms == 0 {
        return Some(raw_ttfb);
    }

    let Some(final_attempt) = retry.route_attempts.iter().rev().find(|attempt| {
        !attempt.skipped
            && (attempt.decision == "completed"
                || attempt
                    .status_code
                    .is_some_and(|status| (200..300).contains(&status)))
    }) else {
        return Some(raw_ttfb);
    };
    let (Some(final_attempt_elapsed_ms), Some(final_attempt_headers_ms)) =
        (final_attempt.duration_ms, final_attempt.upstream_headers_ms)
    else {
        return Some(raw_ttfb);
    };

    if final_attempt_elapsed_ms <= final_attempt_headers_ms
        || final_attempt_elapsed_ms >= duration_ms
        || raw_ttfb >= final_attempt_elapsed_ms
    {
        return Some(raw_ttfb);
    }

    let elapsed_before_final_attempt =
        final_attempt_elapsed_ms.saturating_sub(final_attempt_headers_ms);
    let corrected = elapsed_before_final_attempt.saturating_add(raw_ttfb);
    if corrected > raw_ttfb && corrected < duration_ms {
        Some(corrected)
    } else {
        Some(raw_ttfb)
    }
}

fn output_tokens_per_second(
    output_tokens: i64,
    duration_ms: u64,
    generation_ms: u64,
) -> Option<f64> {
    if output_tokens <= 0 || duration_ms == 0 || generation_ms == 0 {
        return None;
    }
    let rate = output_tokens as f64 / (generation_ms as f64 / 1000.0);
    let rate = if generation_ms.saturating_mul(10) < duration_ms && rate > OUTPUT_RATE_SANITY_CEIL {
        output_tokens as f64 / (duration_ms as f64 / 1000.0)
    } else {
        rate
    };
    rate.is_finite().then_some(rate).filter(|rate| *rate > 0.0)
}

pub(super) fn token_weighted_output_tokens_per_second(
    output_tokens: i64,
    generation_ms: u64,
) -> Option<f64> {
    if output_tokens <= 0 || generation_ms == 0 {
        return None;
    }
    let rate = output_tokens as f64 / (generation_ms as f64 / 1000.0);
    rate.is_finite().then_some(rate).filter(|rate| *rate > 0.0)
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
    pub cross_provider_failover: bool,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub same_provider_retry: bool,
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
            cross_provider_failover: false,
            same_provider_retry: false,
            fast_mode: false,
            streaming: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveRequest {
    pub id: u64,
    #[serde(default)]
    pub runtime_revision: u64,
    #[serde(default)]
    pub runtime_digest: String,
    #[serde(default)]
    pub policy_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_identity_source: Option<SessionIdentitySource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_addr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_service_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_decision: Option<RouteDecisionProvenance>,
    pub service: String,
    pub method: String,
    pub path: String,
    pub started_at_ms: u64,
}

#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum AccountingPriceCoverage {
    Captured,
    Reconstructed,
    InvalidCaptured,
    Unpriced,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
#[serde(default)]
pub struct AccountingPoolMembership {
    pub pool_key: String,
    pub revision: u64,
    pub confidence: IdentityConfidence,
    pub aggregation_eligible: bool,
}

impl AccountingPoolMembership {
    pub fn from_membership(membership: &PoolMembership) -> Self {
        Self {
            pool_key: membership.pool.key.clone(),
            revision: membership.pool.revision,
            confidence: membership.pool.confidence,
            aggregation_eligible: membership.pool.aggregation_eligible,
        }
    }

    pub fn is_reconciliation_eligible(&self) -> bool {
        self.aggregation_eligible && self.confidence.aggregation_eligible()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct RequestAccountingFacts {
    pub schema_version: u32,
    pub project: sessions::ProjectIdentity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_endpoint: Option<ProviderEndpointKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_membership: Option<AccountingPoolMembership>,
    pub price_coverage: AccountingPriceCoverage,
}

impl RequestAccountingFacts {
    pub const SCHEMA_VERSION: u32 = 1;

    pub fn is_legacy(&self) -> bool {
        self.schema_version == 0
            && self.project == sessions::ProjectIdentity::default()
            && self.provider_endpoint.is_none()
            && self.pool_membership.is_none()
            && self.price_coverage == AccountingPriceCoverage::Unknown
    }
}

pub fn classify_captured_cost(cost: &CostBreakdown) -> AccountingPriceCoverage {
    if cost.total_cost_usd.is_none()
        && cost.input_cost_usd.is_none()
        && cost.output_cost_usd.is_none()
        && cost.cache_read_cost_usd.is_none()
        && cost.cache_creation_cost_usd.is_none()
        && cost.confidence == CostConfidence::Unknown
    {
        return AccountingPriceCoverage::Unpriced;
    }

    let Some(total) = cost
        .total_cost_usd
        .as_deref()
        .and_then(UsdAmount::from_decimal_str)
    else {
        return AccountingPriceCoverage::InvalidCaptured;
    };
    if cost.confidence == CostConfidence::Unknown {
        return AccountingPriceCoverage::InvalidCaptured;
    }

    let mut expected = UsdAmount::ZERO;
    for component in [
        cost.input_cost_usd.as_deref(),
        cost.output_cost_usd.as_deref(),
        cost.cache_read_cost_usd.as_deref(),
        cost.cache_creation_cost_usd.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        let Some(component) = UsdAmount::from_decimal_str(component) else {
            return AccountingPriceCoverage::InvalidCaptured;
        };
        expected = expected.saturating_add(component);
    }

    for multiplier in [
        cost.service_tier_multiplier.as_deref(),
        cost.provider_cost_multiplier.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        let Some(multiplier) = PriceMultiplier::from_decimal_str(multiplier) else {
            return AccountingPriceCoverage::InvalidCaptured;
        };
        expected = multiplier.apply(expected);
    }

    if expected != total {
        return AccountingPriceCoverage::InvalidCaptured;
    }
    AccountingPriceCoverage::Captured
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FinishedRequest {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_identity_source: Option<SessionIdentitySource>,
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
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_decision: Option<RouteDecisionProvenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageMetrics>,
    #[serde(default, skip_serializing_if = "CostBreakdown::is_unknown")]
    pub cost: CostBreakdown,
    #[serde(default, skip_serializing_if = "RequestAccountingFacts::is_legacy")]
    pub accounting: RequestAccountingFacts,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<crate::logging::RetryInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_signals: Vec<ProviderSignal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policy_actions: Vec<PolicyAction>,
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
    pub fn provider_endpoint(&self) -> Option<ProviderEndpointKey> {
        provider_endpoint_from_route_decision(&self.service, self.route_decision.as_ref())
    }

    pub fn cache_hit_rate_with_convention(
        &self,
        convention: CacheAccountingConvention,
    ) -> Option<f64> {
        self.usage
            .as_ref()
            .and_then(|usage| usage.cache_hit_rate_with_convention(convention))
    }

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
}

impl ActiveRequest {
    pub fn provider_endpoint(&self) -> Option<ProviderEndpointKey> {
        provider_endpoint_from_route_decision(&self.service, self.route_decision.as_ref())
    }
}

fn provider_endpoint_from_route_decision(
    service: &str,
    route_decision: Option<&RouteDecisionProvenance>,
) -> Option<ProviderEndpointKey> {
    let decision = route_decision?;
    let service = service.trim();
    let provider_id = decision.provider_id.as_deref()?.trim();
    let endpoint_id = decision.endpoint_id.as_deref()?.trim();
    if service.is_empty() || provider_id.is_empty() || endpoint_id.is_empty() {
        return None;
    }
    Some(ProviderEndpointKey::new(service, provider_id, endpoint_id))
}

impl RequestObservability {
    pub fn from_finished_request(request: &FinishedRequest) -> Self {
        let retry = request.retry.as_ref();
        let attempt_count = retry.map(|retry| retry.attempts.max(1)).unwrap_or(1);
        let route_attempts = retry
            .map(|retry| retry.route_attempts.as_slice())
            .unwrap_or_default();
        let route_attempt_count = route_attempts.len();
        let final_provider = request
            .provider_id
            .as_deref()
            .map(str::trim)
            .filter(|provider| !provider.is_empty());
        let has_provider_context = final_provider.is_some()
            && route_attempts
                .iter()
                .any(|attempt| attempt.provider_id.as_deref().is_some());
        let cross_provider_failover = final_provider.is_some_and(|final_provider| {
            route_attempts
                .iter()
                .filter_map(|attempt| attempt.provider_id.as_deref())
                .any(|provider| provider != final_provider)
        });
        let retried = attempt_count > 1;
        let same_provider_retry = retried && has_provider_context && !cross_provider_failover;
        let ttfb_ms = effective_ttfb_ms_for_request(
            request.duration_ms,
            request.ttfb_ms,
            request.retry.as_ref(),
        );
        let generation_ms = generation_ms_from_duration(request.duration_ms, ttfb_ms);
        let output_tokens_per_second = request.usage.as_ref().and_then(|usage| {
            if usage.output_tokens == 0 {
                return None;
            }
            let generation_ms = generation_ms?;
            if generation_ms == 0 {
                return None;
            }
            output_tokens_per_second(usage.output_tokens, request.duration_ms, generation_ms)
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
            ttfb_ms,
            generation_ms,
            output_tokens_per_second,
            attempt_count,
            route_attempt_count,
            retried,
            cross_provider_failover,
            same_provider_retry,
            fast_mode: decided_fast || service_tier_is_fast(request.service_tier.as_deref()),
            streaming: request.streaming || request.observability.streaming,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FinishRequestParams {
    pub id: u64,
    pub winning_attempt: Option<AttemptHandle>,
    pub status_code: u16,
    pub duration_ms: u64,
    pub ended_at_ms: u64,
    pub observed_service_tier: Option<String>,
    pub reported_model: Option<String>,
    pub usage: Option<UsageMetrics>,
    pub retry: Option<crate::logging::RetryInfo>,
    pub ttfb_ms: Option<u64>,
    pub streaming: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SessionStats {
    pub turns_total: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_session_identity_source: Option<SessionIdentitySource>,
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
    pub last_route_decision: Option<RouteDecisionProvenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_usage: Option<UsageMetrics>,
    pub total_usage: UsageMetrics,
    pub turns_with_usage: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_output_tokens_per_second: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_output_tokens_per_second: Option<f64>,
    #[serde(default, skip_serializing_if = "u64_is_zero")]
    pub output_generation_ms_total: u64,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionIdentitySource {
    Header,
    BodySessionId,
    PromptCacheKey,
    MetadataSessionId,
    PreviousResponseId,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionBinding {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
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
    ProviderMapping,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_identity_source: Option<SessionIdentitySource>,
    pub provider_endpoint: ProviderEndpointKey,
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
    pub session_identity_source: Option<SessionIdentitySource>,
    pub provider_endpoint: ProviderEndpointKey,
    pub upstream_base_url: String,
    pub route_path: Vec<String>,
}

impl SessionRouteAffinityTarget {
    pub(crate) fn same_target(&self, affinity: &SessionRouteAffinity) -> bool {
        self.route_graph_key == affinity.route_graph_key
            && self.provider_endpoint == affinity.provider_endpoint
            && self.upstream_base_url == affinity.upstream_base_url
            && self.route_path == affinity.route_path
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SessionIdentityCard {
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_identity_source: Option<SessionIdentitySource>,
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
    pub last_usage: Option<UsageMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_usage: Option<UsageMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns_with_usage: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_output_tokens_per_second: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_output_tokens_per_second: Option<f64>,
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
}

#[derive(Debug, Clone)]
pub(super) struct SessionBindingEntry {
    pub(super) binding: SessionBinding,
}

fn empty_session_identity_card(session_id: Option<String>) -> SessionIdentityCard {
    SessionIdentityCard {
        session_id,
        session_identity_source: None,
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
        last_usage: None,
        total_usage: None,
        turns_total: None,
        turns_with_usage: None,
        last_output_tokens_per_second: None,
        avg_output_tokens_per_second: None,
        binding_profile_name: None,
        binding_continuity_mode: None,
        last_route_decision: None,
        route_affinity: None,
        effective_model: None,
        effective_reasoning_effort: None,
        effective_service_tier: None,
    }
}

fn non_empty_trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn resolve_effective_observed_value(
    observed_value: Option<&str>,
    binding_value: Option<&str>,
) -> Option<ResolvedRouteValue> {
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

fn binding_request_field_value<'a>(
    binding: Option<&'a SessionBinding>,
    value: impl FnOnce(&'a SessionBinding) -> Option<&'a str>,
) -> Option<&'a str> {
    let binding = binding?;
    if binding.continuity_mode != SessionContinuityMode::ManualProfile {
        return None;
    }
    value(binding)
}

fn classify_session_observation_scope(card: &SessionIdentityCard) -> SessionObservationScope {
    if card.cwd.is_some() {
        SessionObservationScope::HostLocalEnriched
    } else {
        SessionObservationScope::ObservedOnly
    }
}

fn apply_basic_effective_route(card: &mut SessionIdentityCard, binding: Option<&SessionBinding>) {
    card.effective_model = resolve_effective_observed_value(
        card.last_model.as_deref(),
        binding_request_field_value(binding, |binding| binding.model.as_deref()),
    );
    card.effective_reasoning_effort = resolve_effective_observed_value(
        card.last_reasoning_effort.as_deref(),
        binding_request_field_value(binding, |binding| binding.reasoning_effort.as_deref()),
    );
    card.effective_service_tier = resolve_effective_observed_value(
        card.last_service_tier.as_deref(),
        binding_request_field_value(binding, |binding| binding.service_tier.as_deref()),
    );
    card.binding_profile_name = binding.and_then(|binding| binding.profile_name.clone());
    card.binding_continuity_mode = binding.map(|binding| binding.continuity_mode);
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

fn merge_session_identity_source(
    target: &mut Option<SessionIdentitySource>,
    source: Option<SessionIdentitySource>,
) {
    if source.is_some() {
        *target = source;
    }
}

pub struct SessionIdentityCardBuildInputs<'a> {
    pub active: &'a [ActiveRequest],
    pub recent: &'a [FinishedRequest],
    pub bindings: &'a HashMap<String, SessionBinding>,
    pub route_affinities: &'a HashMap<String, SessionRouteAffinity>,
    pub stats: &'a HashMap<String, SessionStats>,
}

pub fn build_session_identity_cards_from_parts(
    inputs: SessionIdentityCardBuildInputs<'_>,
) -> Vec<SessionIdentityCard> {
    let SessionIdentityCardBuildInputs {
        active,
        recent,
        bindings,
        route_affinities,
        stats,
    } = inputs;

    use std::collections::HashMap as StdHashMap;

    let mut map: StdHashMap<Option<String>, SessionIdentityCard> = StdHashMap::new();

    for req in active {
        let key = req.session_id.clone();
        let entry = map
            .entry(key.clone())
            .or_insert_with(|| empty_session_identity_card(key));
        merge_session_identity_source(
            &mut entry.session_identity_source,
            req.session_identity_source,
        );

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
        update_card_route_decision(entry, req.route_decision.as_ref());
    }

    for r in recent {
        let key = r.session_id.clone();
        let entry = map
            .entry(key.clone())
            .or_insert_with(|| empty_session_identity_card(key));
        merge_session_identity_source(
            &mut entry.session_identity_source,
            r.session_identity_source,
        );

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
            entry.last_usage = r.usage.clone().or(entry.last_usage.clone());
            entry.last_output_tokens_per_second = r
                .observability_view()
                .output_tokens_per_second
                .or(entry.last_output_tokens_per_second);
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
        merge_session_identity_source(
            &mut entry.session_identity_source,
            st.last_session_identity_source,
        );

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
        if entry.last_usage.is_none() {
            entry.last_usage = st.last_usage.clone();
        }
        if entry.total_usage.is_none() {
            entry.total_usage = Some(st.total_usage.clone());
        }
        if entry.turns_with_usage.is_none() {
            entry.turns_with_usage = Some(st.turns_with_usage);
        }
        if entry.last_output_tokens_per_second.is_none() {
            entry.last_output_tokens_per_second = st.last_output_tokens_per_second;
        }
        if entry.avg_output_tokens_per_second.is_none() {
            entry.avg_output_tokens_per_second = st.avg_output_tokens_per_second;
        }
        update_card_route_decision(entry, st.last_route_decision.as_ref());
    }

    for (sid, affinity) in route_affinities {
        let key = Some(sid.clone());
        if let Some(entry) = map.get_mut(&key) {
            merge_session_identity_source(
                &mut entry.session_identity_source,
                affinity.session_identity_source,
            );
            entry.route_affinity = Some(affinity.clone());
        }
    }

    let mut cards = map.into_values().collect::<Vec<_>>();
    for card in &mut cards {
        let binding = card
            .session_id
            .as_deref()
            .and_then(|session_id| bindings.get(session_id));
        apply_basic_effective_route(card, binding);
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

    use crate::logging::{RetryInfo, RouteAttemptLog};
    use crate::pricing::CostBreakdown;
    use crate::usage::UsageMetrics;

    #[test]
    fn historical_override_provenance_values_remain_decodable() {
        assert_eq!(
            serde_json::from_str::<RouteValueSource>(r#""session_override""#)
                .expect("session override provenance"),
            RouteValueSource::SessionOverride
        );
        assert_eq!(
            serde_json::from_str::<RouteValueSource>(r#""global_override""#)
                .expect("global override provenance"),
            RouteValueSource::GlobalOverride
        );
    }

    #[test]
    fn session_identity_cards_do_not_surface_affinity_only_sessions() {
        let active = Vec::<ActiveRequest>::new();
        let recent = Vec::<FinishedRequest>::new();
        let bindings = HashMap::<String, SessionBinding>::new();
        let stats = HashMap::<String, SessionStats>::new();
        let mut route_affinities = HashMap::<String, SessionRouteAffinity>::new();
        route_affinities.insert(
            "sid-old".to_string(),
            SessionRouteAffinity {
                route_graph_key: "codex/default".to_string(),
                session_identity_source: Some(SessionIdentitySource::Header),
                provider_endpoint: ProviderEndpointKey::new("codex", "old", "default"),
                upstream_base_url: "https://old.example/v1".to_string(),
                route_path: vec!["entry".to_string()],
                last_selected_at_ms: 10,
                last_changed_at_ms: 10,
                change_reason: "restored".to_string(),
            },
        );

        let cards = build_session_identity_cards_from_parts(SessionIdentityCardBuildInputs {
            active: &active,
            recent: &recent,
            bindings: &bindings,
            route_affinities: &route_affinities,
            stats: &stats,
        });

        assert!(
            cards.is_empty(),
            "restored route affinity alone should not appear as an observed TUI session"
        );
    }

    fn sample_finished_request() -> FinishedRequest {
        FinishedRequest {
            id: 7,
            trace_id: Some("codex-7".to_string()),
            session_id: Some("sid".to_string()),
            session_identity_source: Some(SessionIdentitySource::Header),
            client_name: None,
            client_addr: None,
            cwd: None,
            model: Some("gpt-5".to_string()),
            reasoning_effort: None,
            service_tier: Some("default".to_string()),
            provider_id: Some("primary-provider".to_string()),
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
            accounting: RequestAccountingFacts::default(),
            retry: Some(RetryInfo {
                attempts: 2,
                route_attempts: vec![
                    RouteAttemptLog {
                        attempt_index: 0,
                        provider_id: Some("backup-provider".to_string()),
                        decision: "failed_transport".to_string(),
                        code: Some("failed_transport".to_string()),
                        error_class: Some("upstream_transport_error".to_string()),
                        ..RouteAttemptLog::default()
                    },
                    RouteAttemptLog {
                        attempt_index: 1,
                        provider_id: Some("primary-provider".to_string()),
                        decision: "completed".to_string(),
                        code: Some("completed".to_string()),
                        status_code: Some(200),
                        ..RouteAttemptLog::default()
                    },
                ],
            }),
            provider_signals: Vec::new(),
            policy_actions: Vec::new(),
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
        assert!(observability.cross_provider_failover);
        assert!(observability.fast_mode);
        assert!(observability.streaming);
        assert_eq!(observability.output_tokens_per_second, Some(200.0));
    }

    #[test]
    fn finished_request_observability_classifies_same_provider_retry() {
        let mut request = sample_finished_request();
        request
            .retry
            .as_mut()
            .expect("retry fixture")
            .route_attempts[0]
            .provider_id = Some("primary-provider".to_string());

        let observability = request.observability_view();

        assert!(observability.retried);
        assert!(!observability.cross_provider_failover);
        assert!(observability.same_provider_retry);
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
        assert!(value.get("station_name").is_none());
        assert!(value.get("upstream_base_url").is_none());
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

    #[test]
    fn finished_request_corrects_legacy_attempt_relative_stream_ttfb() {
        let mut request = sample_finished_request();
        request.duration_ms = 10_000;
        request.ttfb_ms = Some(1_210);
        request.usage = Some(UsageMetrics {
            output_tokens: 100,
            total_tokens: 100,
            ..UsageMetrics::default()
        });
        request.retry = Some(RetryInfo {
            attempts: 2,
            route_attempts: vec![
                RouteAttemptLog {
                    attempt_index: 0,
                    decision: "failed_status".to_string(),
                    status_code: Some(429),
                    upstream_headers_ms: Some(50),
                    duration_ms: Some(500),
                    ..RouteAttemptLog::default()
                },
                RouteAttemptLog {
                    attempt_index: 1,
                    decision: "completed".to_string(),
                    status_code: Some(200),
                    upstream_headers_ms: Some(1_200),
                    duration_ms: Some(2_200),
                    ..RouteAttemptLog::default()
                },
            ],
        });

        let observability = request.observability_view();

        assert_eq!(observability.ttfb_ms, Some(2_210));
        assert_eq!(observability.generation_ms, Some(7_790));
        let rate = observability.output_tokens_per_second.expect("output rate");
        assert!((rate - (100.0 / 7.79)).abs() < f64::EPSILON);
    }

    #[test]
    fn finished_request_does_not_double_correct_global_stream_ttfb() {
        let mut request = sample_finished_request();
        request.duration_ms = 10_000;
        request.ttfb_ms = Some(2_210);
        request.retry = Some(RetryInfo {
            attempts: 2,
            route_attempts: vec![RouteAttemptLog {
                attempt_index: 1,
                decision: "completed".to_string(),
                status_code: Some(200),
                upstream_headers_ms: Some(1_200),
                duration_ms: Some(2_200),
                ..RouteAttemptLog::default()
            }],
        });

        let observability = request.observability_view();

        assert_eq!(observability.ttfb_ms, Some(2_210));
        assert_eq!(observability.generation_ms, Some(7_790));
    }
}

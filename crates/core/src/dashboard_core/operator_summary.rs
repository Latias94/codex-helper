use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::balance::{
    BalanceSnapshotStatus, ProviderBalanceSnapshot, ProviderUsageAlertKind, ProviderUsageModelStat,
    ProviderUsageRateSnapshot, ProviderUsageWindow,
};
use crate::config::{
    RetryProfileName, RouteAffinityPolicy, RouteGraphConfig, RouteStrategy, SchedulingPreset,
    ServiceRouteConfig,
};
use crate::logging::{RouteAttemptLog, upstream_origin};
use crate::pricing::{CostBreakdown, ModelPriceCatalogSnapshot};
use crate::quota_analytics::QuotaAnalyticsView;
use crate::request_ledger::{RequestUsageSummary, RequestUsageSummaryGroup};
use crate::routing_ir::{RouteCandidate, RoutePlanTemplate};
use crate::runtime_identity::ProviderEndpointKey;
use crate::state::{
    ActiveRequest, FinishedRequest, ResolvedRouteValue, RouteDecisionProvenance,
    RuntimeConfigState, SessionContinuityMode, SessionIdentityCard, SessionStats,
    UsageDayDimensionRow, UsageDayView, UsageRollupView,
};
use crate::usage::UsageMetrics;

use super::types::{
    ControlProfileOption, ProviderCapacity, ProviderEndpointOption, ProviderOption,
};
use super::window_stats::WindowStats;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperatorReadStatus {
    Ready,
    Stale,
    Disconnected,
    AuthRequired,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperatorReadIssue {
    RefreshFailed,
    Disconnected,
    AuthRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorRevisionBundle {
    pub runtime_revision: u64,
    pub runtime_digest: String,
    pub route_digest: String,
    pub catalog_revision: String,
    pub pricing_revision: String,
    pub operator_pricing_revision: String,
    pub policy_revision: u64,
    pub ledger_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperatorReadModel {
    pub api_version: u32,
    pub service_name: String,
    pub status: OperatorReadStatus,
    pub captured_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revisions: Option<OperatorRevisionBundle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<OperatorReadData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<OperatorReadIssue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OperatorReadCapture {
    pub model: OperatorReadModel,
    pub local_session_ids: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperatorReadData {
    pub summary: ApiV1OperatorSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<OperatorRoutingSummary>,
    #[serde(default)]
    pub active_requests: Vec<OperatorActiveRequestSummary>,
    #[serde(default)]
    pub recent_requests: Vec<OperatorRequestSummary>,
    #[serde(default)]
    pub usage_summaries: Vec<RequestUsageSummary>,
    #[serde(default)]
    pub usage_day: UsageDayView,
    #[serde(default)]
    pub usage_rollup: UsageRollupView,
    #[serde(default)]
    pub quota_analytics: QuotaAnalyticsView,
    #[serde(default)]
    pub stats_5m: WindowStats,
    #[serde(default)]
    pub stats_1h: WindowStats,
    pub pricing_catalog: ModelPriceCatalogSnapshot,
    #[serde(default)]
    pub provider_balances: Vec<OperatorProviderBalanceSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorRoutingSummary {
    pub route_graph_key: String,
    pub control_revision: u64,
    pub provider_policy_revision: u64,
    pub entry: String,
    pub entry_strategy: RouteStrategy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_session_preference: Option<OperatorRouteTargetSummary>,
    pub affinity_policy: RouteAffinityPolicy,
    pub scheduling_preset: SchedulingPreset,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_ttl_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reprobe_preferred_after_ms: Option<u64>,
    #[serde(default)]
    pub candidates: Vec<OperatorRouteCandidateSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorRouteTargetSummary {
    pub provider_id: String,
    pub endpoint_id: String,
}

#[derive(Debug, Clone, Copy)]
pub struct OperatorRoutingControlView<'a> {
    pub route_graph_key: &'a str,
    pub control_revision: u64,
    pub provider_policy_revision: u64,
    pub new_session_preference: Option<&'a ProviderEndpointKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorRouteCandidateSummary {
    pub route_order: usize,
    pub provider_id: String,
    pub endpoint_id: String,
    pub preference_group: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route_path: Vec<String>,
}

impl From<&RouteCandidate> for OperatorRouteCandidateSummary {
    fn from(candidate: &RouteCandidate) -> Self {
        Self {
            route_order: candidate.stable_index,
            provider_id: candidate.provider_id.clone(),
            endpoint_id: candidate.endpoint_id.clone(),
            preference_group: candidate.preference_group,
            route_path: candidate.route_path.clone(),
        }
    }
}

pub fn build_operator_routing_summary(
    view: &ServiceRouteConfig,
    route_template: &RoutePlanTemplate,
    control: OperatorRoutingControlView<'_>,
) -> anyhow::Result<OperatorRoutingSummary> {
    let entry_strategy = route_template
        .nodes
        .get(route_template.entry.as_str())
        .map(|node| node.strategy)
        .unwrap_or(RouteStrategy::OrderedFailover);
    let configured_entry = view.routing.as_ref().and_then(RouteGraphConfig::entry_node);
    let entry_target = configured_entry.and_then(|node| node.target.clone());

    Ok(OperatorRoutingSummary {
        route_graph_key: control.route_graph_key.to_string(),
        control_revision: control.control_revision,
        provider_policy_revision: control.provider_policy_revision,
        entry: route_template.entry.clone(),
        entry_strategy,
        entry_target,
        new_session_preference: control.new_session_preference.map(|target| {
            OperatorRouteTargetSummary {
                provider_id: target.provider_id.clone(),
                endpoint_id: target.endpoint_id.clone(),
            }
        }),
        affinity_policy: route_template.affinity_policy,
        scheduling_preset: route_template.scheduling_preset,
        fallback_ttl_ms: route_template.fallback_ttl_ms,
        reprobe_preferred_after_ms: route_template.reprobe_preferred_after_ms,
        candidates: route_template
            .candidates
            .iter()
            .map(OperatorRouteCandidateSummary::from)
            .collect(),
    })
}

impl OperatorReadModel {
    pub fn ready(
        service_name: impl Into<String>,
        captured_at_ms: u64,
        revisions: OperatorRevisionBundle,
        data: OperatorReadData,
    ) -> Self {
        Self {
            api_version: 1,
            service_name: service_name.into(),
            status: OperatorReadStatus::Ready,
            captured_at_ms,
            revisions: Some(revisions),
            data: Some(data),
            issue: None,
        }
    }

    pub fn stale_from(previous: &Self) -> Self {
        match (&previous.revisions, &previous.data) {
            (Some(revisions), Some(data)) => Self {
                api_version: previous.api_version,
                service_name: previous.service_name.clone(),
                status: OperatorReadStatus::Stale,
                captured_at_ms: previous.captured_at_ms,
                revisions: Some(revisions.clone()),
                data: Some(data.clone()),
                issue: Some(OperatorReadIssue::RefreshFailed),
            },
            _ => Self::disconnected(previous.service_name.clone()),
        }
    }

    pub fn disconnected(service_name: impl Into<String>) -> Self {
        Self::unavailable(
            service_name,
            OperatorReadStatus::Disconnected,
            OperatorReadIssue::Disconnected,
        )
    }

    pub fn auth_required(service_name: impl Into<String>) -> Self {
        Self::unavailable(
            service_name,
            OperatorReadStatus::AuthRequired,
            OperatorReadIssue::AuthRequired,
        )
    }

    pub fn can_use_runtime_actions(&self) -> bool {
        self.status == OperatorReadStatus::Ready && self.validate().is_ok()
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.api_version != 1 {
            return Err("operator read-model api_version must be 1");
        }
        if self.service_name.trim().is_empty() {
            return Err("operator read-model service_name must not be empty");
        }
        match self.status {
            OperatorReadStatus::Ready => {
                if self.captured_at_ms == 0
                    || self.revisions.is_none()
                    || self.data.is_none()
                    || self.issue.is_some()
                {
                    return Err("ready operator data requires revisions and data without an issue");
                }
            }
            OperatorReadStatus::Stale => {
                if self.captured_at_ms == 0
                    || self.revisions.is_none()
                    || self.data.is_none()
                    || self.issue != Some(OperatorReadIssue::RefreshFailed)
                {
                    return Err(
                        "stale operator data requires retained revisions, data, and refresh_failed",
                    );
                }
            }
            OperatorReadStatus::Disconnected => {
                if self.revisions.is_some()
                    || self.data.is_some()
                    || self.issue != Some(OperatorReadIssue::Disconnected)
                {
                    return Err(
                        "disconnected operator data requires disconnected without runtime facts",
                    );
                }
            }
            OperatorReadStatus::AuthRequired => {
                if self.revisions.is_some()
                    || self.data.is_some()
                    || self.issue != Some(OperatorReadIssue::AuthRequired)
                {
                    return Err(
                        "auth-required operator data requires auth_required without runtime facts",
                    );
                }
            }
        }
        if let Some(revisions) = self.revisions.as_ref()
            && [
                revisions.runtime_digest.as_str(),
                revisions.route_digest.as_str(),
                revisions.catalog_revision.as_str(),
                revisions.pricing_revision.as_str(),
                revisions.operator_pricing_revision.as_str(),
                revisions.ledger_revision.as_str(),
            ]
            .iter()
            .any(|revision| revision.trim().is_empty())
        {
            return Err("operator read-model revisions must not be empty");
        }
        if let Some(data) = self.data.as_ref()
            && (data.summary.api_version != self.api_version
                || data.summary.service_name != self.service_name)
        {
            return Err("operator read-model summary identity must match the bundle");
        }
        Ok(())
    }

    fn unavailable(
        service_name: impl Into<String>,
        status: OperatorReadStatus,
        issue: OperatorReadIssue,
    ) -> Self {
        Self {
            api_version: 1,
            service_name: service_name.into(),
            status,
            captured_at_ms: 0,
            revisions: None,
            data: None,
            issue: Some(issue),
        }
    }
}

struct OperatorRequestRouteProjection {
    provider_id: Option<String>,
    endpoint_id: Option<String>,
    provider_endpoint_key: Option<String>,
    route_path: Vec<String>,
    upstream_origin: Option<String>,
}

fn operator_request_route_projection(
    service_name: &str,
    fallback_provider_id: Option<&str>,
    route_decision: Option<&RouteDecisionProvenance>,
) -> OperatorRequestRouteProjection {
    let provider_id = route_decision
        .and_then(|decision| decision.provider_id.as_deref())
        .or(fallback_provider_id)
        .map(str::to_string);
    let endpoint_id = route_decision.and_then(|decision| decision.endpoint_id.clone());
    let provider_endpoint_key =
        provider_id
            .as_deref()
            .zip(endpoint_id.as_deref())
            .map(|(provider_id, endpoint_id)| {
                let provider_endpoint =
                    ProviderEndpointKey::new(service_name, provider_id, endpoint_id);
                opaque_operator_key("endpoint", &provider_endpoint.stable_key())
            });
    let route_path = route_decision
        .map(|decision| decision.route_path.clone())
        .unwrap_or_default();
    let upstream_origin = route_decision
        .and_then(|decision| decision.effective_upstream_base_url.as_ref())
        .and_then(|upstream| upstream_origin(&upstream.value));

    OperatorRequestRouteProjection {
        provider_id,
        endpoint_id,
        provider_endpoint_key,
        route_path,
        upstream_origin,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorActiveRequestSummary {
    pub id: u64,
    pub runtime_revision: u64,
    pub policy_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_service_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_endpoint_key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route_path: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_origin: Option<String>,
    pub service: String,
    pub method: String,
    pub path: String,
    pub started_at_ms: u64,
}

impl OperatorActiveRequestSummary {
    pub fn from_active_request(request: &ActiveRequest) -> Self {
        let route = operator_request_route_projection(
            &request.service,
            request.provider_id.as_deref(),
            request.route_decision.as_ref(),
        );
        Self {
            id: request.id,
            runtime_revision: request.runtime_revision,
            policy_revision: request.policy_revision,
            session_key: request.session_id.as_deref().map(operator_session_key),
            model: request.model.clone(),
            requested_model: request.requested_model.clone(),
            reasoning_effort: request.reasoning_effort.clone(),
            service_tier: request.service_tier.clone(),
            requested_service_tier: request.requested_service_tier.clone(),
            provider_id: route.provider_id,
            endpoint_id: route.endpoint_id,
            provider_endpoint_key: route.provider_endpoint_key,
            route_path: route.route_path,
            upstream_origin: route.upstream_origin,
            service: request.service.clone(),
            method: request.method.clone(),
            path: operator_request_path(&request.path),
            started_at_ms: request.started_at_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperatorSessionSummary {
    pub session_key: String,
    pub active_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_started_at_ms_min: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ended_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_service_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_usage: Option<UsageMetrics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_usage: Option<UsageMetrics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns_total: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns_with_usage: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_output_tokens_per_second: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_output_tokens_per_second: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_profile_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_continuity_mode: Option<SessionContinuityMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_route_decision: Option<RouteDecisionProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_affinity: Option<OperatorSessionRouteAffinitySummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_model: Option<ResolvedRouteValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_reasoning_effort: Option<ResolvedRouteValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_service_tier: Option<ResolvedRouteValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorSessionRouteAffinitySummary {
    #[serde(default)]
    pub revision: String,
    pub provider_id: String,
    pub endpoint_id: String,
    pub upstream_origin: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route_path: Vec<String>,
    pub last_selected_at_ms: u64,
    pub last_changed_at_ms: u64,
    pub change_reason: String,
}

impl OperatorSessionRouteAffinitySummary {
    pub(crate) fn from_affinity(affinity: &crate::state::SessionRouteAffinity) -> Option<Self> {
        Some(Self {
            revision: crate::state::session_route_affinity_revision(affinity),
            provider_id: affinity.provider_endpoint.provider_id.clone(),
            endpoint_id: affinity.provider_endpoint.endpoint_id.clone(),
            upstream_origin: upstream_origin(&affinity.upstream_base_url)?,
            route_path: affinity.route_path.clone(),
            last_selected_at_ms: affinity.last_selected_at_ms,
            last_changed_at_ms: affinity.last_changed_at_ms,
            change_reason: affinity.change_reason.clone(),
        })
    }
}

impl OperatorSessionSummary {
    pub fn from_session_card(card: &SessionIdentityCard, fallback_index: usize) -> Self {
        let session_key = card
            .session_id
            .as_deref()
            .map(operator_session_key)
            .unwrap_or_else(|| format!("session:anonymous:{fallback_index}"));
        Self {
            session_key,
            active_count: card.active_count,
            active_started_at_ms_min: card.active_started_at_ms_min,
            last_status: card.last_status,
            last_duration_ms: card.last_duration_ms,
            last_ended_at_ms: card.last_ended_at_ms,
            last_model: card.last_model.clone(),
            last_reasoning_effort: card.last_reasoning_effort.clone(),
            last_service_tier: card.last_service_tier.clone(),
            last_provider_id: card.last_provider_id.clone(),
            last_usage: card.last_usage.clone(),
            total_usage: card.total_usage.clone(),
            turns_total: card.turns_total,
            turns_with_usage: card.turns_with_usage,
            last_output_tokens_per_second: card.last_output_tokens_per_second,
            avg_output_tokens_per_second: card.avg_output_tokens_per_second,
            binding_profile_name: card.binding_profile_name.clone(),
            binding_continuity_mode: card.binding_continuity_mode,
            last_route_decision: card
                .last_route_decision
                .as_ref()
                .map(redact_operator_route_decision),
            route_affinity: card
                .route_affinity
                .as_ref()
                .and_then(OperatorSessionRouteAffinitySummary::from_affinity),
            effective_model: card.effective_model.clone(),
            effective_reasoning_effort: card.effective_reasoning_effort.clone(),
            effective_service_tier: card.effective_service_tier.clone(),
        }
    }
}

fn redact_operator_upstream_value(value: &ResolvedRouteValue) -> Option<ResolvedRouteValue> {
    Some(ResolvedRouteValue {
        value: upstream_origin(&value.value)?,
        source: value.source,
    })
}

fn redact_operator_route_decision(decision: &RouteDecisionProvenance) -> RouteDecisionProvenance {
    let mut redacted = decision.clone();
    redacted.effective_upstream_base_url = decision
        .effective_upstream_base_url
        .as_ref()
        .and_then(redact_operator_upstream_value);
    redacted
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorPolicyActionSummary {
    pub active_cooldown: bool,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooldown_remaining_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OperatorProviderCapacity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configured_max_concurrent_requests: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_max_concurrent_requests: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default)]
    pub saturated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherited_from_provider: Option<bool>,
}

impl OperatorProviderCapacity {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

impl From<&ProviderCapacity> for OperatorProviderCapacity {
    fn from(capacity: &ProviderCapacity) -> Self {
        Self {
            configured_max_concurrent_requests: capacity.configured_max_concurrent_requests,
            effective_max_concurrent_requests: capacity.effective_max_concurrent_requests,
            active: capacity.active,
            limit: capacity.limit,
            saturated: capacity.saturated,
            inherited_from_provider: capacity.inherited_from_provider,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorProviderEndpointSummary {
    pub provider_name: String,
    pub name: String,
    pub provider_endpoint_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    pub priority: u32,
    pub configured_enabled: bool,
    pub effective_enabled: bool,
    pub routable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_enabled_override: Option<bool>,
    pub runtime_state: RuntimeConfigState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_state_override: Option<RuntimeConfigState>,
    #[serde(default, skip_serializing_if = "OperatorProviderCapacity::is_empty")]
    pub capacity: OperatorProviderCapacity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policy_actions: Vec<OperatorPolicyActionSummary>,
}

impl From<&ProviderEndpointOption> for OperatorProviderEndpointSummary {
    fn from(endpoint: &ProviderEndpointOption) -> Self {
        Self {
            provider_name: endpoint.provider_name.clone(),
            name: endpoint.name.clone(),
            provider_endpoint_key: opaque_operator_key("endpoint", &endpoint.provider_endpoint_key),
            origin: upstream_origin(&endpoint.base_url),
            priority: endpoint.priority,
            configured_enabled: endpoint.configured_enabled,
            effective_enabled: endpoint.effective_enabled,
            routable: endpoint.routable,
            runtime_enabled_override: endpoint.runtime_enabled_override,
            runtime_state: endpoint.runtime_state,
            runtime_state_override: endpoint.runtime_state_override,
            capacity: OperatorProviderCapacity::from(&endpoint.capacity),
            policy_actions: endpoint
                .policy_actions
                .iter()
                .map(|action| OperatorPolicyActionSummary {
                    active_cooldown: action.active_cooldown,
                    code: operator_policy_projection_code(action.code.as_deref()).to_string(),
                    cooldown_remaining_secs: action.cooldown_remaining_secs,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorProviderSummary {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub configured_enabled: bool,
    pub effective_enabled: bool,
    pub routable_endpoints: usize,
    #[serde(default)]
    pub endpoints: Vec<OperatorProviderEndpointSummary>,
    #[serde(default, skip_serializing_if = "OperatorProviderCapacity::is_empty")]
    pub capacity: OperatorProviderCapacity,
}

impl From<&ProviderOption> for OperatorProviderSummary {
    fn from(provider: &ProviderOption) -> Self {
        Self {
            name: provider.name.clone(),
            alias: provider.alias.clone(),
            configured_enabled: provider.configured_enabled,
            effective_enabled: provider.effective_enabled,
            routable_endpoints: provider.routable_endpoints,
            endpoints: provider.endpoints.iter().map(Into::into).collect(),
            capacity: OperatorProviderCapacity::from(&provider.capacity),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorProviderBalanceSummary {
    pub observation_provider_id: String,
    pub provider_id: String,
    pub endpoint_id: String,
    pub provider_endpoint_key: String,
    pub fetched_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_after_ms: Option<u64>,
    pub stale: bool,
    pub status: BalanceSnapshotStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exhausted: Option<bool>,
    pub exhaustion_affects_routing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_balance_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_balance_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paygo_balance_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_budget_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_spent_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_period: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_remaining_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_limit_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_used_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_resets_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlimited_quota: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_used_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub today_used_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_requests: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub today_requests: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub today_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub usage_windows: Vec<ProviderUsageWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_rate: Option<ProviderUsageRateSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub usage_model_stats: Vec<ProviderUsageModelStat>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alert_codes: Vec<ProviderUsageAlertKind>,
}

impl From<&ProviderBalanceSnapshot> for OperatorProviderBalanceSummary {
    fn from(snapshot: &ProviderBalanceSnapshot) -> Self {
        Self {
            observation_provider_id: snapshot.observation_provider_id.clone(),
            provider_id: snapshot.provider_endpoint.provider_id.clone(),
            endpoint_id: snapshot.provider_endpoint.endpoint_id.clone(),
            provider_endpoint_key: opaque_operator_key(
                "endpoint",
                snapshot.provider_endpoint.stable_key().as_str(),
            ),
            fetched_at_ms: snapshot.fetched_at_ms,
            stale_after_ms: snapshot.stale_after_ms,
            stale: snapshot.stale,
            status: snapshot.status,
            exhausted: snapshot.exhausted,
            exhaustion_affects_routing: snapshot.exhaustion_affects_routing,
            plan_name: snapshot.plan_name.clone(),
            total_balance_usd: snapshot.total_balance_usd.clone(),
            subscription_balance_usd: snapshot.subscription_balance_usd.clone(),
            paygo_balance_usd: snapshot.paygo_balance_usd.clone(),
            monthly_budget_usd: snapshot.monthly_budget_usd.clone(),
            monthly_spent_usd: snapshot.monthly_spent_usd.clone(),
            quota_period: snapshot.quota_period.clone(),
            quota_remaining_usd: snapshot.quota_remaining_usd.clone(),
            quota_limit_usd: snapshot.quota_limit_usd.clone(),
            quota_used_usd: snapshot.quota_used_usd.clone(),
            quota_resets_at_ms: snapshot.quota_resets_at_ms,
            unlimited_quota: snapshot.unlimited_quota,
            total_used_usd: snapshot.total_used_usd.clone(),
            today_used_usd: snapshot.today_used_usd.clone(),
            total_requests: snapshot.total_requests,
            today_requests: snapshot.today_requests,
            total_tokens: snapshot.total_tokens,
            today_tokens: snapshot.today_tokens,
            subscription_expires_at: snapshot.subscription_expires_at.clone(),
            usage_windows: snapshot.usage_windows.clone(),
            usage_rate: snapshot.usage_rate.clone(),
            usage_model_stats: snapshot.usage_model_stats.clone(),
            alert_codes: snapshot
                .usage_alerts
                .iter()
                .map(|alert| alert.kind)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperatorRequestSummary {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_endpoint_key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route_path: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageMetrics>,
    #[serde(default, skip_serializing_if = "CostBreakdown::is_unknown")]
    pub cost: CostBreakdown,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<OperatorRetrySummaryView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_signal_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policy_action_codes: Vec<String>,
    pub observability: OperatorRequestObservability,
    pub service: String,
    pub method: String,
    pub path: String,
    pub status_code: u16,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u64>,
    pub streaming: bool,
    pub ended_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorRetrySummaryView {
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route_attempts: Vec<OperatorRouteAttemptSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatorRouteAttemptSummary {
    pub attempt_index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_endpoint_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preference_group: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_attempt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_attempt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_max_attempts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_max_attempts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avoided_total: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_upstreams: Option<usize>,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_headers_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooldown_secs: Option<u64>,
    pub skipped: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_signal_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policy_action_codes: Vec<String>,
}

impl From<&RouteAttemptLog> for OperatorRouteAttemptSummary {
    fn from(attempt: &RouteAttemptLog) -> Self {
        Self {
            attempt_index: attempt.attempt_index,
            provider_id: attempt.provider_id.clone(),
            endpoint_id: attempt.endpoint_id.clone(),
            provider_endpoint_key: attempt
                .provider_endpoint_key
                .as_deref()
                .map(|key| opaque_operator_key("endpoint", key)),
            preference_group: attempt.preference_group,
            provider_attempt: attempt.provider_attempt,
            upstream_attempt: attempt.upstream_attempt,
            provider_max_attempts: attempt.provider_max_attempts,
            upstream_max_attempts: attempt.upstream_max_attempts,
            avoided_total: attempt.avoided_total,
            total_upstreams: attempt.total_upstreams,
            code: operator_route_attempt_code(attempt),
            status_code: attempt.status_code,
            model: attempt.model.clone(),
            upstream_headers_ms: attempt.upstream_headers_ms,
            duration_ms: attempt.duration_ms,
            cooldown_secs: attempt.cooldown_secs,
            skipped: attempt.skipped,
            provider_signal_codes: stable_signal_codes(&attempt.provider_signals),
            policy_action_codes: stable_policy_action_codes(&attempt.policy_actions),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperatorRequestObservability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens_per_second: Option<f64>,
    pub attempt_count: u32,
    pub route_attempt_count: usize,
    pub retried: bool,
    pub cross_provider_failover: bool,
    pub same_provider_retry: bool,
    pub fast_mode: bool,
    pub streaming: bool,
}

impl OperatorRequestSummary {
    pub fn from_finished_request(request: &FinishedRequest) -> Self {
        let observability = request.observability_view();
        let route = operator_request_route_projection(
            &request.service,
            request.provider_id.as_deref(),
            request.route_decision.as_ref(),
        );
        Self {
            id: request.id,
            session_key: request.session_id.as_deref().map(operator_session_key),
            model: request.model.clone(),
            reasoning_effort: request.reasoning_effort.clone(),
            service_tier: request.service_tier.clone(),
            provider_id: route.provider_id,
            endpoint_id: route.endpoint_id,
            provider_endpoint_key: route.provider_endpoint_key,
            route_path: route.route_path,
            upstream_origin: route.upstream_origin,
            usage: request.usage.clone(),
            cost: redact_operator_cost_breakdown(request.cost.clone()),
            retry: request
                .retry
                .as_ref()
                .map(|retry| OperatorRetrySummaryView {
                    attempts: retry.attempts,
                    route_attempts: retry
                        .route_attempts
                        .iter()
                        .map(OperatorRouteAttemptSummary::from)
                        .collect(),
                }),
            provider_signal_codes: stable_signal_codes(&request.provider_signals),
            policy_action_codes: stable_policy_action_codes(&request.policy_actions),
            observability: OperatorRequestObservability {
                duration_ms: observability.duration_ms,
                ttfb_ms: observability.ttfb_ms,
                generation_ms: observability.generation_ms,
                output_tokens_per_second: observability.output_tokens_per_second,
                attempt_count: observability.attempt_count,
                route_attempt_count: observability.route_attempt_count,
                retried: observability.retried,
                cross_provider_failover: observability.cross_provider_failover,
                same_provider_retry: observability.same_provider_retry,
                fast_mode: observability.fast_mode,
                streaming: observability.streaming,
            },
            service: request.service.clone(),
            method: request.method.clone(),
            path: operator_request_path(&request.path),
            status_code: request.status_code,
            duration_ms: request.duration_ms,
            ttfb_ms: request.ttfb_ms,
            streaming: request.streaming,
            ended_at_ms: request.ended_at_ms,
        }
    }
}

pub fn redact_operator_usage_day(mut usage_day: UsageDayView) -> UsageDayView {
    redact_dimension_names(&mut usage_day.session_rows, "session");
    redact_dimension_names(&mut usage_day.project_rows, "project");
    if !usage_day.retry_gate.reasons.is_empty() {
        let active = usage_day
            .retry_gate
            .reasons
            .iter()
            .map(|row| row.active)
            .sum();
        usage_day.retry_gate.reasons = vec![crate::state::UsageRetryGateReasonRow {
            reason: "redacted".to_string(),
            active,
        }];
    }
    usage_day.coverage.partial_reason = None;
    usage_day
}

pub fn redact_operator_usage_summaries(
    mut summaries: Vec<RequestUsageSummary>,
) -> Vec<RequestUsageSummary> {
    for summary in &mut summaries {
        if summary.group != RequestUsageSummaryGroup::Session {
            continue;
        }
        for row in &mut summary.rows {
            if row.group_value != "-" {
                row.group_value = operator_session_key(&row.group_value);
            }
        }
    }
    summaries
}

pub fn redact_operator_quota_analytics(mut analytics: QuotaAnalyticsView) -> QuotaAnalyticsView {
    for pool in &mut analytics.pools {
        for row in &mut pool.reconciliation.projects {
            row.project.path = row
                .project
                .path
                .as_deref()
                .filter(|path| !path.is_empty())
                .map(|path| opaque_operator_key("project", path));
        }
    }
    analytics
}

pub fn build_operator_session_stats(recent: &[FinishedRequest]) -> HashMap<String, SessionStats> {
    let mut stats = HashMap::<String, SessionStats>::new();
    for request in recent {
        let Some(session_id) = request.session_id.as_ref() else {
            continue;
        };
        let entry = stats.entry(session_id.clone()).or_default();
        entry.turns_total = entry.turns_total.saturating_add(1);
        entry.last_seen_ms = entry.last_seen_ms.max(request.ended_at_ms);
        if let Some(usage) = request.usage.as_ref() {
            entry.total_usage.add_assign(usage);
            entry.turns_with_usage = entry.turns_with_usage.saturating_add(1);
            if usage.output_tokens > 0
                && let Some(generation_ms) = request.observability_view().generation_ms
            {
                entry.output_generation_ms_total = entry
                    .output_generation_ms_total
                    .saturating_add(generation_ms);
            }
        }
        if entry.last_output_tokens_per_second.is_none() {
            entry.last_output_tokens_per_second =
                request.observability_view().output_tokens_per_second;
        }
    }
    for entry in stats.values_mut() {
        if entry.total_usage.output_tokens > 0 && entry.output_generation_ms_total > 0 {
            entry.avg_output_tokens_per_second = Some(
                entry.total_usage.output_tokens as f64 * 1000.0
                    / entry.output_generation_ms_total as f64,
            );
        }
    }
    stats
}

pub fn redact_operator_pricing_catalog(
    mut catalog: ModelPriceCatalogSnapshot,
) -> ModelPriceCatalogSnapshot {
    catalog.source = operator_pricing_source_label(&catalog.source).to_string();
    for model in &mut catalog.models {
        model.source = operator_pricing_source_label(&model.source).to_string();
    }
    catalog
}

fn redact_operator_cost_breakdown(mut cost: CostBreakdown) -> CostBreakdown {
    cost.pricing_source = cost
        .pricing_source
        .as_deref()
        .map(operator_pricing_source_label)
        .map(str::to_string);
    cost
}

fn operator_pricing_source_label(source: &str) -> &'static str {
    let source = source.trim();
    if source == "bundled" {
        "bundled"
    } else if source.starts_with("bundled+local-overrides(") {
        "bundled_with_local_overrides"
    } else if source.starts_with("local:") {
        "local_override"
    } else if source.starts_with("basellm:") || source.starts_with("basellm_") {
        "basellm"
    } else if source.starts_with("provider_catalog") {
        "provider_catalog"
    } else {
        "other"
    }
}

fn opaque_operator_key(namespace: &str, value: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"codex-helper:operator-read-model:v1\0");
    digest.update(namespace.as_bytes());
    digest.update([0]);
    digest.update(value.as_bytes());
    format!("{namespace}:sha256:{:x}", digest.finalize())
}

pub(crate) fn operator_session_key(session_id: &str) -> String {
    opaque_operator_key("session", session_id)
}

fn operator_request_path(value: &str) -> String {
    value
        .split(['?', '#'])
        .next()
        .filter(|path| path.starts_with('/'))
        .unwrap_or("/")
        .to_string()
}

fn stable_signal_codes(signals: &[crate::provider_signals::ProviderSignal]) -> Vec<String> {
    signals
        .iter()
        .map(|signal| signal.kind.code().to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn stable_policy_action_codes(actions: &[crate::policy_actions::PolicyAction]) -> Vec<String> {
    actions
        .iter()
        .map(|action| action.kind.code().to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn operator_policy_projection_code(code: Option<&str>) -> &'static str {
    match code {
        Some("cooldown") => "cooldown",
        Some("balance_exhausted") => "balance_exhausted",
        _ => "unknown",
    }
}

fn operator_route_attempt_code(attempt: &RouteAttemptLog) -> String {
    match attempt.stable_code() {
        "selected"
        | "observed"
        | "completed"
        | "failed_status"
        | "failed_client_request"
        | "failed_reasoning_guard"
        | "failed_transport"
        | "failed_target_build"
        | "failed_body_read"
        | "failed_body_too_large"
        | "failed_lifecycle_store"
        | "skipped_capability_mismatch"
        | "all_upstreams_avoided"
        | "route_unavailable" => attempt.stable_code().to_string(),
        _ => "unknown".to_string(),
    }
}

fn redact_dimension_names(rows: &mut [UsageDayDimensionRow], namespace: &str) {
    for row in rows {
        row.name = opaque_operator_key(namespace, &row.name);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApiV1OperatorSummary {
    pub api_version: u32,
    pub service_name: String,
    pub runtime: OperatorRuntimeSummary,
    pub counts: OperatorSummaryCounts,
    pub retry: OperatorRetrySummary,
    #[serde(default)]
    pub sessions: Vec<OperatorSessionSummary>,
    #[serde(default)]
    pub profiles: Vec<ControlProfileOption>,
    #[serde(default)]
    pub providers: Vec<OperatorProviderSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OperatorRuntimeSummary {
    #[serde(default)]
    pub runtime_loaded_at_ms: Option<u64>,
    #[serde(default)]
    pub runtime_source_mtime_ms: Option<u64>,
    #[serde(default)]
    pub configured_default_profile: Option<String>,
    #[serde(default)]
    pub default_profile: Option<String>,
    #[serde(default)]
    pub default_profile_summary: Option<OperatorProfileSummary>,
    #[serde(default, skip_serializing_if = "OperatorActionCapabilities::is_empty")]
    pub operator_actions: OperatorActionCapabilities,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OperatorActionCapabilities {
    #[serde(default)]
    pub refresh_provider_balances: bool,
    #[serde(default)]
    pub mutate_routing: bool,
    #[serde(default)]
    pub mutate_session_affinity: bool,
}

impl OperatorActionCapabilities {
    pub fn is_empty(&self) -> bool {
        !self.refresh_provider_balances && !self.mutate_routing && !self.mutate_session_affinity
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OperatorProfileSummary {
    pub name: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub fast_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OperatorRetrySummary {
    #[serde(default)]
    pub configured_profile: Option<RetryProfileName>,
    pub upstream_max_attempts: u32,
    pub provider_max_attempts: u32,
    #[serde(default)]
    pub recent_retried_requests: usize,
    #[serde(default)]
    pub recent_cross_provider_failovers: usize,
    #[serde(default)]
    pub recent_same_provider_retries: usize,
    #[serde(default)]
    pub recent_fast_mode_requests: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OperatorRetryObservations {
    pub recent_retried_requests: usize,
    pub recent_cross_provider_failovers: usize,
    pub recent_same_provider_retries: usize,
    pub recent_fast_mode_requests: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OperatorSummaryCounts {
    #[serde(default)]
    pub active_requests: usize,
    #[serde(default)]
    pub recent_requests: usize,
    #[serde(default)]
    pub sessions: usize,
    #[serde(default)]
    pub profiles: usize,
    #[serde(default)]
    pub providers: usize,
}

pub fn summarize_recent_retry_observations(
    recent: &[FinishedRequest],
) -> OperatorRetryObservations {
    let mut observations = OperatorRetryObservations::default();

    for request in recent {
        let observability = request.observability_view();
        if observability.fast_mode {
            observations.recent_fast_mode_requests += 1;
        }

        if !observability.retried {
            continue;
        }

        observations.recent_retried_requests += 1;
        if observability.cross_provider_failover {
            observations.recent_cross_provider_failovers += 1;
        } else if observability.same_provider_retry {
            observations.recent_same_provider_retries += 1;
        }
    }

    observations
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_store_failure_keeps_its_stable_operator_code() {
        let attempt = RouteAttemptLog {
            code: Some("failed_lifecycle_store".to_string()),
            ..Default::default()
        };

        assert_eq!(
            operator_route_attempt_code(&attempt),
            "failed_lifecycle_store"
        );
    }

    fn ready_operator_model() -> OperatorReadModel {
        OperatorReadModel::ready(
            "codex",
            1_700_000_000_000,
            OperatorRevisionBundle {
                runtime_revision: 7,
                runtime_digest: "runtime-7".to_string(),
                route_digest: "route-7".to_string(),
                catalog_revision: "catalog-7".to_string(),
                pricing_revision: "pricing-7".to_string(),
                operator_pricing_revision: "operator-pricing-7".to_string(),
                policy_revision: 8,
                ledger_revision: "operator-ledger-v1:test-store:9".to_string(),
            },
            OperatorReadData {
                summary: ApiV1OperatorSummary {
                    api_version: 1,
                    service_name: "codex".to_string(),
                    runtime: Default::default(),
                    counts: Default::default(),
                    retry: Default::default(),
                    sessions: Vec::new(),
                    profiles: Vec::new(),
                    providers: Vec::new(),
                },
                routing: None,
                active_requests: Vec::new(),
                recent_requests: Vec::new(),
                usage_summaries: Vec::new(),
                usage_day: Default::default(),
                usage_rollup: Default::default(),
                quota_analytics: Default::default(),
                stats_5m: Default::default(),
                stats_1h: Default::default(),
                pricing_catalog: Default::default(),
                provider_balances: Vec::new(),
            },
        )
    }

    #[test]
    fn operator_read_model_defaults_missing_quota_analytics_to_unsupported() {
        let mut encoded = serde_json::to_value(ready_operator_model()).expect("serialize model");
        encoded["data"]
            .as_object_mut()
            .expect("operator data")
            .remove("quota_analytics");

        let decoded: OperatorReadModel =
            serde_json::from_value(encoded).expect("deserialize legacy model");

        assert_eq!(
            decoded.data.expect("operator data").quota_analytics.support,
            crate::quota_analytics::QuotaAnalyticsSupport::Unsupported
        );
    }

    #[test]
    fn operator_quota_analytics_redacts_project_paths() {
        let mut analytics = QuotaAnalyticsView::default();
        let mut pool = crate::quota_analytics::PoolQuotaAnalytics::default();
        pool.reconciliation.projects = vec![crate::quota_analytics::QuotaProjectRow {
            project: crate::sessions::ProjectIdentity {
                kind: crate::sessions::ProjectIdentityKind::GitRoot,
                path: Some("/home/operator/private-project".to_string()),
            },
            ..Default::default()
        }];
        analytics.pools.push(pool);

        let redacted = redact_operator_quota_analytics(analytics);
        let project = &redacted.pools[0].reconciliation.projects[0].project;

        assert_eq!(project.kind, crate::sessions::ProjectIdentityKind::GitRoot);
        assert!(
            project
                .path
                .as_deref()
                .is_some_and(|path| path.starts_with("project:sha256:"))
        );
        assert_ne!(
            project.path.as_deref(),
            Some("/home/operator/private-project")
        );
    }

    #[test]
    fn operator_session_summary_preserves_redacted_route_provenance() {
        let mut card = SessionIdentityCard {
            session_id: Some("raw-session-id".to_string()),
            active_count: 1,
            effective_model: Some(crate::state::ResolvedRouteValue {
                value: "gpt-5.6".to_string(),
                source: crate::state::RouteValueSource::ProfileDefault,
            }),
            ..SessionIdentityCard::default()
        };
        card.last_route_decision = Some(crate::state::RouteDecisionProvenance {
            provider_id: Some("provider-a".to_string()),
            endpoint_id: Some("responses".to_string()),
            route_path: vec!["main".to_string(), "provider-a".to_string()],
            effective_upstream_base_url: Some(crate::state::ResolvedRouteValue {
                value: "https://user:secret@relay.example.test/v1?token=hidden".to_string(),
                source: crate::state::RouteValueSource::RuntimeFallback,
            }),
            ..Default::default()
        });
        card.route_affinity = Some(crate::state::SessionRouteAffinity {
            route_graph_key: "route-graph-secret".to_string(),
            session_identity_source: None,
            provider_endpoint: crate::runtime_identity::ProviderEndpointKey::new(
                "codex",
                "provider-a",
                "responses",
            ),
            upstream_base_url: "https://user:secret@relay.example.test/v1?token=hidden".to_string(),
            route_path: vec!["main".to_string(), "provider-a".to_string()],
            last_selected_at_ms: 10,
            last_changed_at_ms: 11,
            change_reason: "initial_selection".to_string(),
        });

        let summary = OperatorSessionSummary::from_session_card(&card, 0);

        assert_eq!(
            summary.effective_model.as_ref().map(|value| value.source),
            Some(crate::state::RouteValueSource::ProfileDefault)
        );
        assert_eq!(
            summary
                .last_route_decision
                .as_ref()
                .and_then(|decision| decision.endpoint_id.as_deref()),
            Some("responses")
        );
        assert_eq!(
            summary
                .last_route_decision
                .as_ref()
                .and_then(|decision| decision.effective_upstream_base_url.as_ref())
                .map(|upstream| upstream.value.as_str()),
            Some("https://relay.example.test")
        );
        assert_eq!(
            summary
                .route_affinity
                .as_ref()
                .map(|affinity| affinity.upstream_origin.as_str()),
            Some("https://relay.example.test")
        );
        let json = serde_json::to_string(&summary).expect("serialize session summary");
        for removed in [
            "last_station_name",
            "last_upstream_origin",
            "effective_upstream_origin",
            "route_graph_key",
        ] {
            assert!(
                !json.contains(removed),
                "session summary retained legacy route field {removed}"
            );
        }
        for secret in ["user:secret", "/v1", "token=hidden", "route-graph-secret"] {
            assert!(!json.contains(secret), "session summary leaked {secret}");
        }
    }

    fn finished_request(
        provider_id: Option<&str>,
        service_tier: Option<&str>,
        retry: Option<crate::logging::RetryInfo>,
    ) -> FinishedRequest {
        let mut request = serde_json::from_value::<FinishedRequest>(serde_json::json!({
            "id": 1,
            "trace_id": "codex-1",
            "service_tier": service_tier,
            "provider_id": provider_id,
            "service": "codex",
            "method": "POST",
            "path": "/v1/responses",
            "status_code": 200,
            "duration_ms": 100,
            "ended_at_ms": 1
        }))
        .expect("finished request fixture");
        request.retry = retry;
        request
    }

    fn assert_canonical_request_route(
        projection: &serde_json::Value,
        provider_id: &str,
        endpoint_id: &str,
        provider_endpoint_key: &str,
        upstream_origin: &str,
        route_path: &[&str],
    ) {
        assert_eq!(projection["provider_id"], provider_id);
        assert_eq!(projection["endpoint_id"], endpoint_id);
        assert_eq!(projection["provider_endpoint_key"], provider_endpoint_key);
        assert_eq!(projection["upstream_origin"], upstream_origin);
        assert_eq!(
            projection["route_path"],
            serde_json::json!(route_path),
            "operator route path must come from the final route decision"
        );
        assert!(
            projection.get("station_name").is_none(),
            "operator request DTO must not publish legacy station identity"
        );
    }

    #[test]
    fn operator_request_dtos_publish_only_canonical_final_route_identity() {
        let mut active = serde_json::from_value::<ActiveRequest>(serde_json::json!({
            "id": 41,
            "runtime_revision": 7,
            "policy_revision": 8,
            "provider_id": "fallback-provider",
            "service": "codex",
            "method": "POST",
            "path": "/v1/responses?api_key=request-secret",
            "started_at_ms": 100
        }))
        .expect("active request fixture");
        active.route_decision = Some(RouteDecisionProvenance {
            provider_id: Some("final-provider".to_string()),
            endpoint_id: Some("responses".to_string()),
            route_path: vec![
                "root".to_string(),
                "final-provider".to_string(),
                "responses".to_string(),
            ],
            effective_upstream_base_url: Some(ResolvedRouteValue {
                value: "https://route-user:route-password@active-route.example.test:8443/v1?api_key=route-secret"
                    .to_string(),
                source: crate::state::RouteValueSource::ProviderMapping,
            }),
            ..Default::default()
        });

        let mut finished = serde_json::from_value::<FinishedRequest>(serde_json::json!({
            "id": 42,
            "provider_id": "fallback-provider",
            "service": "codex",
            "method": "POST",
            "path": "/v1/chat/completions?api_key=request-secret",
            "status_code": 200,
            "duration_ms": 100,
            "ended_at_ms": 200
        }))
        .expect("finished request fixture");
        finished.route_decision = Some(RouteDecisionProvenance {
            endpoint_id: Some("chat".to_string()),
            route_path: vec![
                "root".to_string(),
                "fallback-provider".to_string(),
                "chat".to_string(),
            ],
            effective_upstream_base_url: Some(ResolvedRouteValue {
                value: "https://route-user:route-password@finished-route.example.test/v1?api_key=route-secret"
                    .to_string(),
                source: crate::state::RouteValueSource::RuntimeFallback,
            }),
            ..Default::default()
        });

        let active_projection =
            serde_json::to_value(OperatorActiveRequestSummary::from_active_request(&active))
                .expect("serialize active request projection");
        assert_canonical_request_route(
            &active_projection,
            "final-provider",
            "responses",
            "endpoint:sha256:b22d9f8dcec521e5496fc9be440c1d562feffa1532d8b1ccb9dc5346ca34ee78",
            "https://active-route.example.test:8443",
            &["root", "final-provider", "responses"],
        );

        let finished_projection =
            serde_json::to_value(OperatorRequestSummary::from_finished_request(&finished))
                .expect("serialize finished request projection");
        assert_canonical_request_route(
            &finished_projection,
            "fallback-provider",
            "chat",
            "endpoint:sha256:588284d2767bd6e373c71ae6c30734e2102ff2b454a3f10eeb79913a09f3a308",
            "https://finished-route.example.test",
            &["root", "fallback-provider", "chat"],
        );

        let serialized = format!("{active_projection}{finished_projection}");
        for forbidden in [
            "station_name",
            "route-user",
            "route-password",
            "route-secret",
            "request-secret",
            "codex/final-provider/responses",
            "codex/fallback-provider/chat",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "operator request projection leaked {forbidden}"
            );
        }
    }

    #[test]
    fn operator_read_model_accepts_only_the_four_canonical_state_shapes() {
        let ready = ready_operator_model();
        let stale = OperatorReadModel::stale_from(&ready);
        let disconnected = OperatorReadModel::disconnected("codex");
        let auth_required = OperatorReadModel::auth_required("codex");

        for model in [&ready, &stale, &disconnected, &auth_required] {
            assert!(
                model.validate().is_ok(),
                "unexpected invalid model: {model:?}"
            );
        }
        assert!(ready.can_use_runtime_actions());
        assert!(!stale.can_use_runtime_actions());
        assert!(!disconnected.can_use_runtime_actions());
        assert!(!auth_required.can_use_runtime_actions());

        let mut invalid_ready = ready.clone();
        invalid_ready.issue = Some(OperatorReadIssue::RefreshFailed);
        assert!(invalid_ready.validate().is_err());

        let mut invalid_stale = stale.clone();
        invalid_stale.data = None;
        assert!(invalid_stale.validate().is_err());

        let mut invalid_disconnected = disconnected.clone();
        invalid_disconnected.revisions = ready.revisions.clone();
        assert!(invalid_disconnected.validate().is_err());

        let mut invalid_auth = auth_required.clone();
        invalid_auth.data = ready.data.clone();
        assert!(invalid_auth.validate().is_err());

        let mut invalid_version = ready.clone();
        invalid_version.api_version = 2;
        assert!(invalid_version.validate().is_err());

        let mut invalid_revision = ready.clone();
        invalid_revision
            .revisions
            .as_mut()
            .expect("ready revisions")
            .route_digest
            .clear();
        assert!(invalid_revision.validate().is_err());

        let mut invalid_identity = ready;
        invalid_identity
            .data
            .as_mut()
            .expect("ready data")
            .summary
            .service_name = "claude".to_string();
        assert!(invalid_identity.validate().is_err());
    }

    #[test]
    fn stale_operator_read_model_retains_the_exact_previous_bundle() {
        let ready = ready_operator_model();
        let stale = OperatorReadModel::stale_from(&ready);

        assert_eq!(stale.captured_at_ms, ready.captured_at_ms);
        assert_eq!(stale.revisions, ready.revisions);
        assert_eq!(stale.data, ready.data);
        assert_eq!(stale.issue, Some(OperatorReadIssue::RefreshFailed));
    }

    #[test]
    fn operator_projections_drop_host_and_upstream_supplied_secrets() {
        use crate::policy_actions::{PolicyAction, PolicyActionKind, PolicyActionOwner};
        use crate::provider_signals::{
            ProviderSignal, ProviderSignalConfidence, ProviderSignalKind, ProviderSignalSource,
            ProviderSignalTarget, ProviderSignalTrace,
        };
        use crate::runtime_identity::ProviderEndpointKey;

        let signal = ProviderSignal {
            kind: ProviderSignalKind::Quota,
            code: Some("signal-code-secret".to_string()),
            source: ProviderSignalSource::RouteAttempt,
            target: ProviderSignalTarget::Service {
                service: "codex".to_string(),
            },
            confidence: ProviderSignalConfidence::High,
            observed_at_ms: 10,
            route_facing: true,
            retry_after_secs: Some(60),
            reset_after_secs: None,
            reason: Some("signal-reason-secret".to_string()),
            error_class: Some("signal-error-secret".to_string()),
            trace: ProviderSignalTrace {
                trace_id: Some("trace-secret".to_string()),
                cf_ray: Some("cf-ray-secret".to_string()),
                upstream_request_id: Some("upstream-request-secret".to_string()),
            },
        };
        let action = PolicyAction {
            id: "action-id-secret".to_string(),
            kind: PolicyActionKind::Cooldown,
            code: Some("action-code-secret".to_string()),
            owner: PolicyActionOwner::CodexHelper,
            provider_endpoint_key: ProviderEndpointKey::new(
                "codex",
                "provider-key-secret",
                "endpoint-key-secret",
            ),
            source_signal: signal.clone(),
            reason: "action-reason-secret".to_string(),
            confidence: ProviderSignalConfidence::High,
            created_at_ms: 10,
            expires_at_ms: 70_000,
            recovery_state: Default::default(),
            generation: 1,
        };

        let mut request = finished_request(Some("relay"), Some("priority"), None);
        request.trace_id = Some("request-trace-secret".to_string());
        request.session_id = Some("session-secret".to_string());
        request.client_addr = Some("client-address-secret".to_string());
        request.cwd = Some("/home/operator/project-secret".to_string());
        request.route_decision = Some(RouteDecisionProvenance {
            provider_id: Some("relay".to_string()),
            endpoint_id: Some("responses".to_string()),
            route_path: vec!["relay".to_string(), "responses".to_string()],
            effective_upstream_base_url: Some(ResolvedRouteValue {
                value: "https://user-secret:password-secret@relay.example.test:8443/v1?token=query-secret#fragment-secret"
                    .to_string(),
                source: crate::state::RouteValueSource::RuntimeFallback,
            }),
            ..Default::default()
        });
        request.path = "/v1/responses?api_key=request-query-secret#fragment".to_string();
        request.cost = serde_json::from_value(serde_json::json!({
            "total_cost_usd": "1.25",
            "confidence": "estimated",
            "pricing_source": "local:/home/operator/pricing-path-secret.toml"
        }))
        .expect("cost fixture");
        request.provider_signals = vec![signal.clone()];
        request.policy_actions = vec![action.clone()];
        request.retry = Some(
            serde_json::from_value(serde_json::json!({
                "attempts": 1,
                "upstream_chain": ["raw-upstream-chain-secret"],
                "route_attempts": [{
                    "attempt_index": 0,
                    "provider_endpoint_key": "provider-endpoint-key-secret",
                    "route_path": ["route-path-secret"],
                    "decision": "decision-secret",
                    "code": "attempt-code-secret",
                    "reason": "attempt-reason-secret",
                    "error_class": "attempt-error-secret",
                    "upstream_base_url": "https://attempt-url-secret.test/v1",
                    "provider_signals": [signal],
                    "policy_actions": [action],
                    "raw": "attempt-raw-secret"
                }]
            }))
            .expect("legacy retry fixture"),
        );

        let request_json =
            serde_json::to_string(&OperatorRequestSummary::from_finished_request(&request))
                .expect("serialize request projection");
        assert!(request_json.contains("https://relay.example.test:8443"));
        assert!(request_json.contains("local_override"));
        assert!(request_json.contains("quota"));
        assert!(request_json.contains("cooldown"));
        assert!(request_json.contains("\"code\":\"unknown\""));

        let card: SessionIdentityCard = serde_json::from_value(serde_json::json!({
            "session_id": "session-card-secret",
            "host_local_transcript_path": "/home/operator/transcript-path-secret.jsonl",
            "last_client_addr": "session-client-address-secret",
            "cwd": "/home/operator/session-cwd-secret",
            "active_count": 0
        }))
        .expect("session card fixture");
        let session_json =
            serde_json::to_string(&OperatorSessionSummary::from_session_card(&card, 0))
                .expect("serialize session projection");

        let pricing_json = serde_json::to_string(&redact_operator_pricing_catalog(
            ModelPriceCatalogSnapshot {
                source: "local:/home/operator/catalog-path-secret.toml".to_string(),
                model_count: 1,
                models: vec![crate::pricing::ModelPriceView {
                    provider: "openai".to_string(),
                    model_id: "gpt-test".to_string(),
                    display_name: None,
                    aliases: Vec::new(),
                    input_per_1m_usd: "1".to_string(),
                    output_per_1m_usd: "2".to_string(),
                    cache_read_input_per_1m_usd: None,
                    cache_creation_input_per_1m_usd: None,
                    tiers: Vec::new(),
                    source: "local:/home/operator/model-price-path-secret.toml".to_string(),
                    source_generation: None,
                    confidence: crate::pricing::CostConfidence::Estimated,
                }],
            },
        ))
        .expect("serialize pricing projection");

        let capacity_json =
            serde_json::to_string(&OperatorProviderCapacity::from(&ProviderCapacity {
                configured_limit_group: Some("configured-group-secret".to_string()),
                effective_limit_group: Some("effective-group-secret".to_string()),
                limit_key: Some("limit-key-secret".to_string()),
                ..Default::default()
            }))
            .expect("serialize capacity projection");

        let serialized = format!("{request_json}{session_json}{pricing_json}{capacity_json}");
        for secret in [
            "user-secret",
            "password-secret",
            "query-secret",
            "fragment-secret",
            "request-trace-secret",
            "session-secret",
            "client-address-secret",
            "project-secret",
            "signal-code-secret",
            "signal-reason-secret",
            "signal-error-secret",
            "trace-secret",
            "cf-ray-secret",
            "upstream-request-secret",
            "action-id-secret",
            "action-code-secret",
            "action-reason-secret",
            "provider-endpoint-key-secret",
            "route-path-secret",
            "decision-secret",
            "attempt-code-secret",
            "attempt-reason-secret",
            "attempt-error-secret",
            "attempt-url-secret",
            "attempt-raw-secret",
            "transcript-path-secret",
            "session-client-address-secret",
            "session-cwd-secret",
            "pricing-path-secret",
            "catalog-path-secret",
            "model-price-path-secret",
            "configured-group-secret",
            "effective-group-secret",
            "limit-key-secret",
        ] {
            assert!(
                !serialized.contains(secret),
                "leaked operator secret: {secret}"
            );
        }
    }

    #[test]
    fn summarize_recent_retry_observations_reports_retry_failover_and_fast_mode() {
        let recent = vec![
            finished_request(
                Some("alpha"),
                Some("PRIORITY"),
                Some(crate::logging::RetryInfo {
                    attempts: 2,
                    route_attempts: vec![
                        RouteAttemptLog {
                            attempt_index: 0,
                            provider_id: Some("alpha".to_string()),
                            decision: "failed_status".to_string(),
                            ..Default::default()
                        },
                        RouteAttemptLog {
                            attempt_index: 1,
                            provider_id: Some("alpha".to_string()),
                            decision: "completed".to_string(),
                            ..Default::default()
                        },
                    ],
                }),
            ),
            finished_request(
                Some("alpha"),
                Some("default"),
                Some(crate::logging::RetryInfo {
                    attempts: 3,
                    route_attempts: vec![
                        RouteAttemptLog {
                            attempt_index: 0,
                            provider_id: Some("beta".to_string()),
                            decision: "failed_status".to_string(),
                            ..Default::default()
                        },
                        RouteAttemptLog {
                            attempt_index: 1,
                            provider_id: Some("beta".to_string()),
                            decision: "failed_transport".to_string(),
                            ..Default::default()
                        },
                        RouteAttemptLog {
                            attempt_index: 2,
                            provider_id: Some("alpha".to_string()),
                            decision: "completed".to_string(),
                            ..Default::default()
                        },
                    ],
                }),
            ),
            finished_request(Some("alpha"), Some("priority"), None),
        ];

        let summary = summarize_recent_retry_observations(&recent);

        assert_eq!(summary.recent_retried_requests, 2);
        assert_eq!(summary.recent_cross_provider_failovers, 1);
        assert_eq!(summary.recent_same_provider_retries, 1);
        assert_eq!(summary.recent_fast_mode_requests, 2);
    }
}

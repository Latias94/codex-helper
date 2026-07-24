use std::collections::{HashMap, HashSet, VecDeque};
#[cfg(test)]
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock as SyncRwLock, Weak};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, RwLock, watch};
use tokio::time::Duration;

pub use crate::balance::{
    BalanceSnapshotStatus, ProviderBalanceSnapshot, ProviderRoutingBalanceSummary,
};
use crate::config::ServiceRouteConfig;
use crate::endpoint_health::{
    COOLDOWN_SECS, CooldownBackoff, FAILURE_THRESHOLD, RouteCapability, RuntimeHealthDomain,
    RuntimeHealthHalfOpenTerminal,
};
use crate::policy_actions::PolicyActionProjection;
#[cfg(test)]
use crate::pricing::capture_operator_model_price_catalog;
use crate::pricing::{
    CapturedModelPriceCatalog, CostAdjustments, CostBreakdown, CostConfidence,
    estimate_usage_cost_from_captured_provider_price,
};
use crate::provider_catalog::{
    AccountFingerprint, ProviderAdapter, ProviderCatalogEpoch, ProviderCatalogScope,
    ProviderCatalogSnapshot, ProviderModelRequestContract, ProviderPricingTier,
};
use crate::quota_analytics::{
    PoolAttributionResult, QuotaAnalyticsView, build_quota_analytics, plan_quota_attribution,
};
use crate::quota_pool::{
    DEFAULT_MAX_SAMPLES_PER_POOL, DEFAULT_SAMPLE_RETENTION_MS, QUOTA_CHECKPOINT_SCHEMA_VERSION,
    QuotaCapabilities, QuotaCounterKind, QuotaObservationContext, QuotaPoolRegistry,
    QuotaPoolState, QuotaRegistryCheckpoint, QuotaUnit, RegistryUpdate,
};
use crate::request_ledger::{
    RequestUsageAggregate, RequestUsageSummary, RequestUsageSummaryCoverage,
    RequestUsageSummaryGroup, RequestUsageSummaryRow, sort_usage_summary_rows,
};
use crate::routing_ir::{RoutePlanRuntimeState, RoutePlanUpstreamRuntimeState};
use crate::runtime_identity::{ProviderEndpointKey, RuntimeUpstreamIdentity};
use crate::runtime_store::{
    AttemptHandle, AttemptId, AttemptOutcome, AttemptPendingEvidence, AttemptRouteEvidence,
    AttemptTerminal, BeginDisposition, CommittedRequestQuery, EconomicsState,
    FrozenProviderCatalogScope, FrozenProviderEpochIdentity, FrozenProviderPriceKey,
    LogicalRequestHandle, LogicalRequestId, LogicalRequestOutcome, LogicalRequestTerminal,
    LogicalRequestTerminalPayload, NewAttempt, NewLogicalRequest, OperatorLedgerRevision,
    ProviderAutomaticEligibility, ProviderEligibilityProjection, ProviderManualEligibility,
    ProviderObservation, ProviderObservationCommit, ProviderObservationDisposition,
    ProviderObservationReservation, ProviderObservationScope, ProviderPolicySnapshot,
    RequestAccountingScope, RuntimeDocumentCommit, RuntimeDocumentKind, RuntimeDocumentWrite,
    RuntimeQuotaIdentity, RuntimeStore, RuntimeStoreError, SessionAffinityIdentitySource,
    SessionAffinityLimit, SessionAffinityRecord, TerminalDisposition,
};
#[cfg(test)]
use crate::runtime_store::{
    ProviderEffectiveEligibility, ProviderObservationAuthority, ProviderPolicyEffect,
};
use crate::sessions;
#[cfg(test)]
use crate::usage::UsageMetrics;
use crate::usage_day;
use crate::usage_providers::ProviderBalanceRefreshCoordinator;

mod attribution_index;
mod routing_control;
mod runtime_types;
mod session_affinity_control;
mod session_identity;

use self::attribution_index::AttributionIndex;
pub use self::attribution_index::{
    AttributionAggregate, AttributionBucket, AttributionBucketKey, AttributionCoverage,
    AttributionPoolKey, AttributionQuery, AttributionQueryResult,
};

pub(crate) use self::routing_control::PreparedRoutingOperatorRouteGraph;
pub use self::routing_control::{
    NewSessionPreference, RoutingOperatorControlCommit, RoutingOperatorControlError,
    RoutingOperatorControlSnapshot, RoutingOperatorControlUpdate,
};
use self::runtime_types::UsageRollup;
pub use self::runtime_types::{
    RuntimeConfigState, UsageBucket, UsageDayCoverage, UsageDayDimensionRow, UsageDayHourRow,
    UsageDayView, UsageRetryGateReasonRow, UsageRetryGateSummary, UsageRollupCoverage,
    UsageRollupView,
};
pub use self::session_affinity_control::{
    SessionRouteAffinityControlCommand, SessionRouteAffinityControlCommit,
    SessionRouteAffinityControlStatus, session_route_affinity_revision,
};
use self::session_identity::SessionBindingEntry;
pub(crate) use self::session_identity::SessionRouteAffinitySuccess;
pub use self::session_identity::{
    AccountingPoolMembership, AccountingPriceCoverage, ActiveRequest, FinishRequestParams,
    FinishedRequest, RequestAccountingFacts, RequestObservability, ResolvedRouteValue,
    RouteDecisionProvenance, RouteValueSource, SessionBinding, SessionBindingProjection,
    SessionContinuityMode, SessionIdentityCard, SessionIdentityCardBuildInputs,
    SessionIdentitySource, SessionObservationScope, SessionRouteAffinity,
    SessionRouteAffinityTarget, SessionStats, build_session_identity_cards_from_parts,
    classify_captured_cost, enrich_session_identity_cards_with_host_transcripts,
    session_binding_revision,
};
pub use crate::sessions::{ProjectIdentity, ProjectIdentityKind};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderBalanceRecord {
    snapshot: ProviderBalanceSnapshot,
    route_scope: Option<String>,
}

type ProviderBalanceMap = HashMap<ProviderEndpointKey, HashMap<String, ProviderBalanceRecord>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderManualEligibilityUpdate {
    Applied,
    Unchanged,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderManualEligibilityCommit {
    pub status: ProviderManualEligibilityUpdate,
    pub snapshot: Arc<ProviderPolicySnapshot>,
}

pub(crate) struct SessionRouteControlGuard {
    session_id: String,
    owner: Arc<()>,
    _guard: OwnedMutexGuard<()>,
}

impl SessionRouteControlGuard {
    pub(crate) fn session_id(&self) -> &str {
        self.session_id.as_str()
    }

    fn belongs_to(&self, owner: &Arc<()>) -> bool {
        Arc::ptr_eq(&self.owner, owner)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderBalanceSnapshotPublication {
    Published,
    IgnoredOlder,
    IgnoredInvalidIdentity,
    IgnoredInactiveRuntimeIdentity,
}

type OperatorUsageSummaryDayMap = HashMap<i32, RequestUsageAggregate>;
type OperatorUsageSummaryServiceMap =
    HashMap<RequestUsageSummaryGroup, HashMap<String, OperatorUsageSummaryDayMap>>;
type OperatorUsageSummaryMap = HashMap<String, OperatorUsageSummaryServiceMap>;

pub(crate) fn is_logical_request_success_status(status_code: u16) -> bool {
    status_code == 101 || (200..300).contains(&status_code)
}

#[derive(Debug, Clone)]
struct ProviderEndpointRuntimeHealth {
    failure_count: u32,
    cooldown_until: Option<std::time::Instant>,
    penalty_streak: u32,
    last_good_at_ms: Option<u64>,
    breaker_epoch: u64,
    half_open_probe_attempted_epoch: Option<u64>,
    half_open_probe_owner: Weak<()>,
}

impl Default for ProviderEndpointRuntimeHealth {
    fn default() -> Self {
        Self {
            failure_count: 0,
            cooldown_until: None,
            penalty_streak: 0,
            last_good_at_ms: None,
            breaker_epoch: 0,
            half_open_probe_attempted_epoch: None,
            half_open_probe_owner: Weak::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProviderEndpointRuntimeHealthKey {
    provider_endpoint: ProviderEndpointKey,
    route_scope: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProviderEndpointRuntimeHealthBucketKey {
    identity: ProviderEndpointRuntimeHealthKey,
    domain: RuntimeHealthDomain,
}

impl ProviderEndpointRuntimeHealthBucketKey {
    fn new(identity: ProviderEndpointRuntimeHealthKey, domain: RuntimeHealthDomain) -> Self {
        Self { identity, domain }
    }
}

struct RuntimeHealthHalfOpenProbeBucketLease {
    key: ProviderEndpointRuntimeHealthBucketKey,
    breaker_epoch: u64,
}

pub(crate) struct RuntimeHealthHalfOpenProbeLease {
    identity: ProviderEndpointRuntimeHealthKey,
    capability: RouteCapability,
    owner: Arc<()>,
    buckets: Vec<RuntimeHealthHalfOpenProbeBucketLease>,
}

pub(crate) struct DispatchedRuntimeHealthHalfOpenProbe {
    identity: ProviderEndpointRuntimeHealthKey,
    capability: RouteCapability,
    buckets: Vec<RuntimeHealthHalfOpenProbeBucketLease>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RuntimeHealthHalfOpenProbeInvalidated;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeHealthHalfOpenSettlement {
    Applied,
    Stale,
}

struct RuntimeHealthHalfOpenProbeBucketSpec {
    key: ProviderEndpointRuntimeHealthBucketKey,
    breaker_epoch: u64,
}

impl ProviderEndpointRuntimeHealthKey {
    fn for_service(service_name: &str, identity: &RuntimeUpstreamIdentity) -> Option<Self> {
        if identity.provider_endpoint.service_name != service_name {
            return None;
        }
        Some(Self {
            provider_endpoint: identity.provider_endpoint.clone(),
            route_scope: identity.policy_route_scope(),
        })
    }
}

#[derive(Debug, Clone, Default)]
struct ProviderEndpointRuntimeHealthState {
    active_revision: Option<u64>,
    identities_authoritative: bool,
    active_identities: HashSet<ProviderEndpointRuntimeHealthKey>,
    health: HashMap<ProviderEndpointRuntimeHealthBucketKey, ProviderEndpointRuntimeHealth>,
}

fn project_provider_endpoint_runtime_health(
    runtime: &mut RoutePlanRuntimeState,
    state: &mut ProviderEndpointRuntimeHealthState,
    identities: &[ProviderEndpointRuntimeHealthKey],
    capability: Option<RouteCapability>,
    now: std::time::Instant,
) {
    let Some(capability) = capability else {
        return;
    };
    let mut affinity: Option<(ProviderEndpointKey, u64)> = None;
    for identity in identities {
        let domains = [
            RuntimeHealthDomain::EndpointTransport,
            RuntimeHealthDomain::Credential,
            RuntimeHealthDomain::Capability(capability),
            RuntimeHealthDomain::Capacity(capability),
        ];
        let mut failure_count = 0;
        let mut cooldown_until = None;
        let mut projected = false;
        let mut capability_last_good_at_ms = None;
        for domain in domains {
            let key = ProviderEndpointRuntimeHealthBucketKey::new(identity.clone(), domain);
            let Some(health) = state.health.get_mut(&key) else {
                continue;
            };
            projected = true;
            reset_expired_runtime_health_breaker(health, now);
            failure_count = failure_count.max(health.failure_count);
            if let Some(until) = health.cooldown_until
                && cooldown_until.is_none_or(|current| until > current)
            {
                cooldown_until = Some(until);
            }
            if domain == RuntimeHealthDomain::Capability(capability) {
                capability_last_good_at_ms = health.last_good_at_ms;
            }
        }
        if !projected {
            continue;
        }
        let cooldown_active = cooldown_until.is_some_and(|until| now < until);
        let cooldown_remaining_secs = cooldown_until
            .and_then(|until| (now < until).then(|| until.duration_since(now).as_secs().max(1)));
        runtime.set_provider_endpoint(
            identity.provider_endpoint.clone(),
            RoutePlanUpstreamRuntimeState {
                runtime_disabled: false,
                draining: false,
                failure_count,
                cooldown_active,
                cooldown_remaining_secs,
                usage_exhausted: false,
                credential_readiness: Default::default(),
                concurrency_saturated: false,
                concurrency_active: None,
                concurrency_limit: None,
            },
        );
        if let Some(last_good_at_ms) = capability_last_good_at_ms
            && affinity
                .as_ref()
                .is_none_or(|(_, current)| last_good_at_ms > *current)
        {
            affinity = Some((identity.provider_endpoint.clone(), last_good_at_ms));
        }
    }
    if let Some((provider_endpoint, _)) = affinity {
        runtime.set_affinity_provider_endpoint(Some(provider_endpoint));
    }
}

fn reset_expired_runtime_health_breaker(
    health: &mut ProviderEndpointRuntimeHealth,
    now: std::time::Instant,
) {
    if health.cooldown_until.is_some_and(|until| now >= until) {
        health.failure_count = 0;
        health.cooldown_until = None;
    }
}

fn runtime_health_breaker_is_open(
    health: &ProviderEndpointRuntimeHealth,
    now: std::time::Instant,
) -> bool {
    health.failure_count >= FAILURE_THRESHOLD
        || health.cooldown_until.is_some_and(|until| now < until)
}

fn begin_runtime_health_breaker_epoch(health: &mut ProviderEndpointRuntimeHealth) {
    health.breaker_epoch = health.breaker_epoch.saturating_add(1).max(1);
    health.half_open_probe_attempted_epoch = None;
    health.half_open_probe_owner = Weak::new();
}

fn record_runtime_health_success(
    health: &mut ProviderEndpointRuntimeHealth,
    domain: RuntimeHealthDomain,
    capability: RouteCapability,
    now_ms: u64,
) {
    health.failure_count = 0;
    health.cooldown_until = None;
    health.penalty_streak = 0;
    health.half_open_probe_attempted_epoch = None;
    health.half_open_probe_owner = Weak::new();
    health.last_good_at_ms =
        (domain == RuntimeHealthDomain::Capability(capability)).then_some(now_ms);
}

fn record_runtime_health_failure(
    health: &mut ProviderEndpointRuntimeHealth,
    failure_threshold_cooldown_secs: u64,
    cooldown_backoff: CooldownBackoff,
    now: std::time::Instant,
) {
    reset_expired_runtime_health_breaker(health, now);
    let was_open = runtime_health_breaker_is_open(health, now);
    health.failure_count = health.failure_count.saturating_add(1);
    if health.failure_count < FAILURE_THRESHOLD {
        return;
    }

    if !was_open {
        begin_runtime_health_breaker_epoch(health);
    }
    let base_secs = if failure_threshold_cooldown_secs == 0 {
        COOLDOWN_SECS
    } else {
        failure_threshold_cooldown_secs
    };
    let effective_secs = cooldown_backoff.effective_cooldown_secs(base_secs, health.penalty_streak);
    let new_until = now + std::time::Duration::from_secs(effective_secs);
    if health
        .cooldown_until
        .is_none_or(|existing| new_until > existing)
    {
        health.cooldown_until = Some(new_until);
    }
    health.penalty_streak = health.penalty_streak.saturating_add(1);
    health.last_good_at_ms = None;
}

fn penalize_runtime_health(
    health: &mut ProviderEndpointRuntimeHealth,
    cooldown_secs: u64,
    cooldown_backoff: CooldownBackoff,
    now: std::time::Instant,
) {
    reset_expired_runtime_health_breaker(health, now);
    if !runtime_health_breaker_is_open(health, now) {
        begin_runtime_health_breaker_epoch(health);
    }
    let effective_secs =
        cooldown_backoff.effective_cooldown_secs(cooldown_secs, health.penalty_streak);
    health.failure_count = FAILURE_THRESHOLD;
    health.cooldown_until = Some(now + std::time::Duration::from_secs(effective_secs));
    health.penalty_streak = health.penalty_streak.saturating_add(1);
    health.last_good_at_ms = None;
}

fn half_open_probe_bucket_specs(
    state: &mut ProviderEndpointRuntimeHealthState,
    identity: &ProviderEndpointRuntimeHealthKey,
    capability: RouteCapability,
    now: std::time::Instant,
    expected_owner: Option<&Arc<()>>,
) -> Option<Vec<RuntimeHealthHalfOpenProbeBucketSpec>> {
    for domain in [
        RuntimeHealthDomain::Credential,
        RuntimeHealthDomain::Capacity(capability),
    ] {
        let key = ProviderEndpointRuntimeHealthBucketKey::new(identity.clone(), domain);
        let Some(health) = state.health.get_mut(&key) else {
            continue;
        };
        reset_expired_runtime_health_breaker(health, now);
        if runtime_health_breaker_is_open(health, now) {
            return None;
        }
    }

    let mut specs = Vec::new();
    for domain in [
        RuntimeHealthDomain::EndpointTransport,
        RuntimeHealthDomain::Capability(capability),
    ] {
        let key = ProviderEndpointRuntimeHealthBucketKey::new(identity.clone(), domain);
        let Some(health) = state.health.get_mut(&key) else {
            continue;
        };
        reset_expired_runtime_health_breaker(health, now);
        if !runtime_health_breaker_is_open(health, now) {
            continue;
        }
        if health.breaker_epoch == 0 {
            health.breaker_epoch = 1;
        }
        if health.half_open_probe_attempted_epoch == Some(health.breaker_epoch) {
            return None;
        }
        match (health.half_open_probe_owner.upgrade(), expected_owner) {
            (Some(owner), Some(expected)) if Arc::ptr_eq(expected, &owner) => {}
            (Some(_), _) | (None, Some(_)) => return None,
            (None, None) => {}
        }
        specs.push(RuntimeHealthHalfOpenProbeBucketSpec {
            key,
            breaker_epoch: health.breaker_epoch,
        });
    }
    (!specs.is_empty()).then_some(specs)
}

fn apply_provider_policy_to_route_runtime(
    runtime: &mut RoutePlanRuntimeState,
    service_name: &str,
    policy_snapshot: &ProviderPolicySnapshot,
    now_ms: u64,
) {
    for projection in policy_snapshot
        .projections
        .iter()
        .filter(|projection| projection.provider_endpoint.service_name == service_name)
    {
        let mut upstream_state = runtime.provider_endpoint(&projection.provider_endpoint);
        if projection.automatic == ProviderAutomaticEligibility::Blocked {
            upstream_state.usage_exhausted = true;
            if let Some(action) = projection.active_action.as_ref() {
                upstream_state.cooldown_active = true;
                upstream_state.cooldown_remaining_secs = action
                    .expires_at_unix_ms
                    .map(|expires_at| expires_at.saturating_sub(now_ms).div_ceil(1_000))
                    .filter(|remaining| *remaining > 0);
            }
        }
        match projection.manual {
            ProviderManualEligibility::Enabled => {}
            ProviderManualEligibility::Disabled => {
                upstream_state.runtime_disabled = true;
                upstream_state.draining = false;
            }
            ProviderManualEligibility::Draining => {
                upstream_state.runtime_disabled = false;
                upstream_state.draining = true;
            }
        }
        runtime.set_provider_endpoint(projection.provider_endpoint.clone(), upstream_state);
    }
}

fn active_provider_endpoint_runtime_health<'a>(
    service_name: &str,
    identity: &RuntimeUpstreamIdentity,
    domain: RuntimeHealthDomain,
    state: &'a mut ProviderEndpointRuntimeHealthState,
) -> Option<&'a mut ProviderEndpointRuntimeHealth> {
    let identity = ProviderEndpointRuntimeHealthKey::for_service(service_name, identity)?;
    if state.active_revision.is_some() && !state.active_identities.contains(&identity) {
        return None;
    }
    Some(
        state
            .health
            .entry(ProviderEndpointRuntimeHealthBucketKey::new(
                identity, domain,
            ))
            .or_default(),
    )
}

#[derive(Debug, Clone)]
struct SessionTranscriptPathCacheEntry {
    path: Option<String>,
    last_checked_ms: u64,
    last_seen_ms: u64,
}

#[derive(Debug, Clone)]
struct RuntimePolicy {
    session_stats_ttl_ms: u64,
    session_binding_ttl_ms: u64,
    session_binding_max_entries: usize,
    session_route_affinity_ttl_ms: u64,
    session_route_affinity_max_entries: usize,
    session_transcript_path_cache_ttl_ms: u64,
    session_transcript_path_cache_max_entries: usize,
}

#[cfg(test)]
#[derive(Debug)]
struct TerminalPublicationPause {
    committed: tokio::sync::oneshot::Sender<()>,
    resume: tokio::sync::oneshot::Receiver<()>,
}

#[cfg(test)]
#[derive(Debug)]
struct OperatorAggregationPause {
    captured: tokio::sync::oneshot::Sender<()>,
    resume: tokio::sync::oneshot::Receiver<()>,
}

pub fn recent_finished_max() -> usize {
    static MAX: OnceLock<usize> = OnceLock::new();
    *MAX.get_or_init(|| {
        recent_finished_max_from_env(std::env::var("CODEX_HELPER_RECENT_FINISHED_MAX").ok())
    })
}

fn recent_finished_max_from_env(raw: Option<String>) -> usize {
    raw.as_deref()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(1_000)
        .clamp(200, 10_000)
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn runtime_store_blocking<T>(operation: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            // Hand scheduling off without yielding across durable commit and projection publication.
            tokio::task::block_in_place(operation)
        }
        Ok(_) | Err(_) => operation(),
    }
}

fn quota_runtime_error(detail: impl Into<String>) -> RuntimeStoreError {
    RuntimeStoreError::InvariantViolation {
        entity: "quota registry",
        id: "quota_registry".to_string(),
        detail: detail.into(),
    }
}

fn load_quota_runtime_state(
    runtime_store: &RuntimeStore,
) -> Result<(QuotaPoolRegistry, RuntimeQuotaIdentity, Option<u64>), RuntimeStoreError> {
    let identity = runtime_store.load_or_create_quota_identity()?;
    let Some(document) = runtime_store.read_runtime_document(RuntimeDocumentKind::QuotaRegistry)?
    else {
        return Ok((QuotaPoolRegistry::default(), identity, None));
    };
    if document.schema_version != QUOTA_CHECKPOINT_SCHEMA_VERSION {
        return Err(quota_runtime_error(format!(
            "unsupported document schema version {}",
            document.schema_version
        )));
    }
    let checkpoint = serde_json::from_str::<QuotaRegistryCheckpoint>(&document.payload_json)
        .map_err(|error| quota_runtime_error(format!("invalid checkpoint payload: {error}")))?;
    if checkpoint.schema_version != document.schema_version {
        return Err(quota_runtime_error(format!(
            "checkpoint schema {} conflicts with document schema {}",
            checkpoint.schema_version, document.schema_version
        )));
    }
    let registry = QuotaPoolRegistry::from_checkpoint(
        checkpoint,
        DEFAULT_MAX_SAMPLES_PER_POOL,
        DEFAULT_SAMPLE_RETENTION_MS,
    )
    .ok_or_else(|| quota_runtime_error("checkpoint cannot be restored"))?;
    Ok((registry, identity, Some(document.revision)))
}

struct PreparedQuotaRegistryCheckpoint {
    expected_revision: Option<u64>,
    payload_json: String,
}

fn prepare_quota_registry_checkpoint(
    runtime_store: &RuntimeStore,
    checkpoint: &QuotaRegistryCheckpoint,
) -> Result<PreparedQuotaRegistryCheckpoint, RuntimeStoreError> {
    let current = runtime_store.read_runtime_document(RuntimeDocumentKind::QuotaRegistry)?;
    if let Some(document) = current.as_ref() {
        if document.schema_version != QUOTA_CHECKPOINT_SCHEMA_VERSION {
            return Err(quota_runtime_error(format!(
                "refusing to replace document schema {}",
                document.schema_version
            )));
        }
        let persisted = serde_json::from_str::<QuotaRegistryCheckpoint>(&document.payload_json)
            .map_err(|error| {
                quota_runtime_error(format!("invalid persisted checkpoint: {error}"))
            })?;
        if persisted.generation > checkpoint.generation {
            return Err(quota_runtime_error(format!(
                "stale checkpoint generation {} is older than persisted generation {}",
                checkpoint.generation, persisted.generation
            )));
        }
        if persisted.generation == checkpoint.generation && persisted != *checkpoint {
            return Err(quota_runtime_error(format!(
                "checkpoint generation {} has conflicting content",
                checkpoint.generation
            )));
        }
    }
    let payload_json = serialize_quota_registry_checkpoint(checkpoint)?;
    Ok(PreparedQuotaRegistryCheckpoint {
        expected_revision: current.map(|document| document.revision),
        payload_json,
    })
}

fn serialize_quota_registry_checkpoint(
    checkpoint: &QuotaRegistryCheckpoint,
) -> Result<String, RuntimeStoreError> {
    serde_json::to_string(checkpoint)
        .map_err(|error| quota_runtime_error(format!("serialize checkpoint: {error}")))
}

fn persist_quota_registry_checkpoint(
    runtime_store: &RuntimeStore,
    checkpoint: &QuotaRegistryCheckpoint,
    expected_revision: Option<u64>,
) -> Result<u64, RuntimeStoreError> {
    let prepared = prepare_quota_registry_checkpoint(runtime_store, checkpoint)?;
    if prepared.expected_revision != expected_revision {
        return Err(quota_runtime_error(format!(
            "quota registry revision changed before validation: expected {expected_revision:?}, found {:?}",
            prepared.expected_revision
        )));
    }
    match runtime_store.compare_and_write_runtime_document(
        prepared.expected_revision,
        RuntimeDocumentWrite {
            kind: RuntimeDocumentKind::QuotaRegistry,
            schema_version: QUOTA_CHECKPOINT_SCHEMA_VERSION,
            payload_json: &prepared.payload_json,
        },
    )? {
        RuntimeDocumentCommit::Committed(document) => Ok(document.revision),
        RuntimeDocumentCommit::Stale(current) => Err(quota_runtime_error(format!(
            "quota registry revision changed before commit: expected {:?}, found {:?}",
            prepared.expected_revision,
            current.map(|document| document.revision)
        ))),
    }
}

fn usage_rollup_unknown_key(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("-")
        .to_string()
}

fn usage_rollup_project_key(request: &FinishedRequest) -> String {
    if request.accounting.project != ProjectIdentity::default() {
        return request.accounting.project.display_key().to_string();
    }
    usage_rollup_unknown_key(request.cwd.as_deref())
}

fn usage_rollup_record_bucket(
    bucket: &mut UsageBucket,
    request: &FinishedRequest,
    cost: Option<&CostBreakdown>,
) {
    bucket.record(
        request.status_code,
        request.duration_ms,
        request.usage.as_ref(),
        cost,
        request.observability.ttfb_ms,
    );
}

fn usage_rollup_hourly_buckets(
    by_hour: &mut HashMap<i32, Vec<UsageBucket>>,
    day: i32,
) -> &mut Vec<UsageBucket> {
    let buckets = by_hour
        .entry(day)
        .or_insert_with(|| vec![UsageBucket::default(); 24]);
    if buckets.len() < 24 {
        buckets.resize_with(24, UsageBucket::default);
    } else if buckets.len() > 24 {
        buckets.truncate(24);
    }
    buckets
}

fn usage_rollup_mark_loaded_timestamp(rollup: &mut UsageRollup, day: i32, timestamp_ms: u64) {
    rollup.loaded_first_ms = Some(
        rollup
            .loaded_first_ms
            .map(|current| current.min(timestamp_ms))
            .unwrap_or(timestamp_ms),
    );
    rollup.loaded_last_ms = Some(
        rollup
            .loaded_last_ms
            .map(|current| current.max(timestamp_ms))
            .unwrap_or(timestamp_ms),
    );
    let range = rollup
        .terminal_range_by_day
        .entry(day)
        .or_insert((timestamp_ms, timestamp_ms));
    range.0 = range.0.min(timestamp_ms);
    range.1 = range.1.max(timestamp_ms);
    if rollup.coverage_source.is_empty() {
        rollup.coverage_source = "runtime_store".to_string();
    }
}

fn sum_usage_buckets<'a>(buckets: impl Iterator<Item = &'a UsageBucket>) -> UsageBucket {
    buckets.fold(UsageBucket::default(), |mut total, bucket| {
        total.add_assign(bucket);
        total
    })
}

fn usage_entity_totals(
    by_day: &HashMap<String, HashMap<i32, UsageBucket>>,
) -> HashMap<String, UsageBucket> {
    by_day
        .iter()
        .map(|(key, days)| (key.clone(), sum_usage_buckets(days.values())))
        .collect()
}

fn rebuild_usage_rollup_totals(rollup: &mut UsageRollup) {
    rollup.loaded = sum_usage_buckets(rollup.by_day.values());
    rollup.loaded_first_ms = rollup
        .terminal_range_by_day
        .values()
        .map(|(first_ms, _)| *first_ms)
        .min();
    rollup.loaded_last_ms = rollup
        .terminal_range_by_day
        .values()
        .map(|(_, last_ms)| *last_ms)
        .max();
    rollup.by_provider_endpoint = usage_entity_totals(&rollup.by_provider_endpoint_day);
    rollup.by_provider = usage_entity_totals(&rollup.by_provider_day);
    rollup.by_model = usage_entity_totals(&rollup.by_model_day);
    rollup.by_session = usage_entity_totals(&rollup.by_session_day);
    rollup.by_project = usage_entity_totals(&rollup.by_project_day);
}

fn usage_bucket_cost_sort_key(bucket: &UsageBucket) -> Option<f64> {
    bucket
        .cost
        .total_cost_usd
        .as_deref()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
}

fn compare_usage_day_rows(
    left: &UsageDayDimensionRow,
    right: &UsageDayDimensionRow,
) -> std::cmp::Ordering {
    match (
        usage_bucket_cost_sort_key(&left.bucket),
        usage_bucket_cost_sort_key(&right.bucket),
    ) {
        (Some(left_cost), Some(right_cost)) => right_cost
            .partial_cmp(&left_cost)
            .unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
    .then_with(|| {
        right
            .bucket
            .usage
            .total_tokens
            .cmp(&left.bucket.usage.total_tokens)
    })
    .then_with(|| right.bucket.requests_total.cmp(&left.bucket.requests_total))
    .then_with(|| left.name.cmp(&right.name))
}

fn usage_day_dimension_rows(
    source: &HashMap<String, HashMap<i32, UsageBucket>>,
    day: i32,
    top_n: usize,
) -> Vec<UsageDayDimensionRow> {
    let mut rows = source
        .iter()
        .filter_map(|(name, days)| {
            let bucket = days.get(&day)?;
            (bucket.requests_total > 0).then(|| UsageDayDimensionRow {
                name: name.clone(),
                bucket: bucket.clone(),
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by(compare_usage_day_rows);
    rows.truncate(top_n);
    rows
}

fn usage_day_hour_rows(rollup: &UsageRollup, day: i32) -> Vec<UsageDayHourRow> {
    let mut buckets = rollup
        .by_hour
        .get(&day)
        .cloned()
        .unwrap_or_else(|| vec![UsageBucket::default(); 24]);
    if buckets.len() < 24 {
        buckets.resize_with(24, UsageBucket::default);
    } else if buckets.len() > 24 {
        buckets.truncate(24);
    }
    buckets
        .into_iter()
        .enumerate()
        .map(|(hour, bucket)| UsageDayHourRow {
            hour: u8::try_from(hour).unwrap_or(0),
            bucket,
        })
        .collect()
}

fn usage_day_coverage(rollup: &UsageRollup, window: usage_day::UsageDayWindow) -> UsageDayCoverage {
    let mut reasons = Vec::new();
    if rollup
        .loaded_first_ms
        .is_some_and(|loaded_first_ms| loaded_first_ms > window.start_ms)
    {
        reasons.push("loaded data starts after local day start");
    }

    UsageDayCoverage {
        source: if rollup.coverage_source.is_empty() {
            "none".to_string()
        } else {
            rollup.coverage_source.clone()
        },
        loaded_first_ms: rollup.loaded_first_ms,
        loaded_last_ms: rollup.loaded_last_ms,
        loaded_requests: rollup.loaded.requests_total,
        day_may_be_partial: !reasons.is_empty(),
        partial_reason: (!reasons.is_empty()).then(|| reasons.join("; ")),
    }
}

fn record_usage_entity(
    totals: &mut HashMap<String, UsageBucket>,
    by_day: &mut HashMap<String, HashMap<i32, UsageBucket>>,
    key: String,
    day: i32,
    request: &FinishedRequest,
    cost: Option<&CostBreakdown>,
) {
    usage_rollup_record_bucket(totals.entry(key.clone()).or_default(), request, cost);
    usage_rollup_record_bucket(
        by_day.entry(key).or_default().entry(day).or_default(),
        request,
        cost,
    );
}

fn record_finished_request_into_usage_rollup(
    rollup: &mut UsageRollup,
    logical_request_id: &str,
    request: &FinishedRequest,
) -> bool {
    if rollup.recorded_requests.contains_key(logical_request_id) {
        return false;
    }

    let day = usage_day::local_day_from_ms(request.ended_at_ms);
    let hour = usize::from(usage_day::local_hour_from_ms(request.ended_at_ms).min(23));
    rollup
        .recorded_requests
        .insert(logical_request_id.to_string(), day);
    usage_rollup_mark_loaded_timestamp(rollup, day, request.ended_at_ms);

    let cost = Some(&request.cost);
    usage_rollup_record_bucket(&mut rollup.loaded, request, cost);
    usage_rollup_record_bucket(rollup.by_day.entry(day).or_default(), request, cost);
    usage_rollup_record_bucket(
        &mut usage_rollup_hourly_buckets(&mut rollup.by_hour, day)[hour],
        request,
        cost,
    );

    record_usage_entity(
        &mut rollup.by_provider_endpoint,
        &mut rollup.by_provider_endpoint_day,
        usage_rollup_unknown_key(
            request
                .provider_endpoint()
                .as_ref()
                .map(ProviderEndpointKey::stable_key)
                .as_deref(),
        ),
        day,
        request,
        cost,
    );
    record_usage_entity(
        &mut rollup.by_provider,
        &mut rollup.by_provider_day,
        usage_rollup_unknown_key(request.provider_id.as_deref()),
        day,
        request,
        cost,
    );
    record_usage_entity(
        &mut rollup.by_model,
        &mut rollup.by_model_day,
        usage_rollup_unknown_key(request.model.as_deref()),
        day,
        request,
        cost,
    );
    record_usage_entity(
        &mut rollup.by_session,
        &mut rollup.by_session_day,
        usage_rollup_unknown_key(request.session_id.as_deref()),
        day,
        request,
        cost,
    );
    record_usage_entity(
        &mut rollup.by_project,
        &mut rollup.by_project_day,
        usage_rollup_project_key(request),
        day,
        request,
        cost,
    );

    true
}

fn record_finished_request_into_operator_usage_summary(
    summaries: &mut OperatorUsageSummaryMap,
    request: &FinishedRequest,
    billable_usage: Option<&crate::usage::CanonicalUsageBuckets>,
) {
    let day = usage_day::local_day_from_ms(request.ended_at_ms);
    let service = summaries.entry(request.service.clone()).or_default();
    for group in RequestUsageSummaryGroup::ALL {
        service
            .entry(group)
            .or_default()
            .entry(group.key(request))
            .or_default()
            .entry(day)
            .or_default()
            .record_request(request, billable_usage);
    }
}

fn operator_usage_summary_rows(
    summaries: Option<&OperatorUsageSummaryServiceMap>,
    group: RequestUsageSummaryGroup,
    limit: usize,
) -> Vec<RequestUsageSummaryRow> {
    let mut rows = summaries
        .and_then(|groups| groups.get(&group))
        .into_iter()
        .flat_map(|groups| groups.iter())
        .map(|(group_value, days)| RequestUsageSummaryRow {
            group_value: group_value.clone(),
            aggregate: days.values().fold(
                RequestUsageAggregate::default(),
                |mut total, aggregate| {
                    total.add_assign(aggregate);
                    total
                },
            ),
        })
        .collect::<Vec<_>>();
    sort_usage_summary_rows(&mut rows, limit);
    rows
}

#[cfg(test)]
fn operator_usage_summaries(
    state: &RequestLifecycleProjectionState,
    service_name: &str,
    limit: usize,
) -> Vec<RequestUsageSummary> {
    operator_usage_summaries_from_sources(
        state.usage_rollups.get(service_name),
        state.operator_usage_summaries.get(service_name),
        limit,
    )
}

fn operator_usage_summaries_from_sources(
    rollup: Option<&UsageRollup>,
    summaries: Option<&OperatorUsageSummaryServiceMap>,
    limit: usize,
) -> Vec<RequestUsageSummary> {
    let coverage = rollup
        .map(|rollup| RequestUsageSummaryCoverage {
            source: if rollup.coverage_source.is_empty() {
                "runtime_store_retained_terminals".to_string()
            } else {
                format!("{}_retained_terminals", rollup.coverage_source)
            },
            first_terminal_at_ms: rollup.loaded_first_ms,
            last_terminal_at_ms: rollup.loaded_last_ms,
            requests: rollup.loaded.requests_total,
            all_history: false,
        })
        .unwrap_or_else(|| RequestUsageSummaryCoverage {
            source: "runtime_store_retained_terminals".to_string(),
            first_terminal_at_ms: None,
            last_terminal_at_ms: None,
            requests: 0,
            all_history: false,
        });
    RequestUsageSummaryGroup::ALL
        .into_iter()
        .map(|group| RequestUsageSummary {
            group,
            coverage: coverage.clone(),
            rows: operator_usage_summary_rows(summaries, group, limit),
        })
        .collect()
}

const RUNTIME_PROJECTION_PAGE_SIZE: usize = 1_024;
const DAY_MS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone)]
pub enum SessionRouteReservationAccess {
    None,
    Available(SessionRouteAffinity),
    Busy { owner_request_id: u64 },
}

#[derive(Debug, Clone)]
struct OwnedSessionRouteReservation {
    affinity: SessionRouteAffinity,
    owner_request_id: u64,
}

#[derive(Debug, Default)]
struct RequestLifecycleProjectionState {
    next_request_id: u64,
    committed_terminal_count: u64,
    active_requests: HashMap<u64, ActiveRequest>,
    lifecycle_handles: HashMap<u64, LogicalRequestHandle>,
    provider_catalogs: HashMap<u64, Arc<ProviderCatalogSnapshot>>,
    attempt_epochs: HashMap<u64, HashMap<AttemptId, ProviderCatalogEpoch>>,
    pricing_catalogs: HashMap<u64, Arc<CapturedModelPriceCatalog>>,
    recent_finished: VecDeque<FinishedRequest>,
    usage_rollups: HashMap<String, UsageRollup>,
    operator_usage_summaries: OperatorUsageSummaryMap,
    session_stats: HashMap<String, HashMap<String, SessionStats>>,
    attribution_index: AttributionIndex,
}

#[derive(Debug)]
pub(crate) struct OperatorLifecycleSnapshot {
    pub(crate) state_revision: u64,
    pub(crate) ledger_revision: OperatorLedgerRevision,
    pub(crate) active_requests: Vec<ActiveRequest>,
    pub(crate) recent_finished: Vec<FinishedRequest>,
    pub(crate) session_stats: HashMap<String, SessionStats>,
    usage_rollup: Option<UsageRollup>,
    usage_summaries: OperatorUsageSummaryServiceMap,
}

impl OperatorLifecycleSnapshot {
    pub(crate) fn usage_summaries(&self, limit: usize) -> Vec<RequestUsageSummary> {
        operator_usage_summaries_from_sources(
            self.usage_rollup.as_ref(),
            Some(&self.usage_summaries),
            limit,
        )
    }

    pub(crate) fn usage_rollup_view(&self, top_n: usize, days: usize) -> UsageRollupView {
        ProxyState::usage_rollup_view_from(self.usage_rollup.as_ref(), top_n, days)
    }

    pub(crate) fn usage_day_view(&self, top_n: usize, generated_at_ms: u64) -> UsageDayView {
        ProxyState::usage_day_view_from(self.usage_rollup.as_ref(), top_n, generated_at_ms)
    }
}

fn snapshot_usage_rollup(rollup: &UsageRollup) -> UsageRollup {
    UsageRollup {
        loaded: rollup.loaded.clone(),
        loaded_first_ms: rollup.loaded_first_ms,
        loaded_last_ms: rollup.loaded_last_ms,
        coverage_source: rollup.coverage_source.clone(),
        terminal_range_by_day: rollup.terminal_range_by_day.clone(),
        recorded_requests: HashMap::new(),
        by_day: rollup.by_day.clone(),
        by_hour: rollup.by_hour.clone(),
        by_provider_endpoint: rollup.by_provider_endpoint.clone(),
        by_provider_endpoint_day: rollup.by_provider_endpoint_day.clone(),
        by_provider: rollup.by_provider.clone(),
        by_provider_day: rollup.by_provider_day.clone(),
        by_model: rollup.by_model.clone(),
        by_model_day: rollup.by_model_day.clone(),
        by_session: rollup.by_session.clone(),
        by_session_day: rollup.by_session_day.clone(),
        by_project: rollup.by_project.clone(),
        by_project_day: rollup.by_project_day.clone(),
    }
}

fn hydrate_runtime_projections(
    runtime_store: &RuntimeStore,
) -> Result<RequestLifecycleProjectionState, RuntimeStoreError> {
    let metadata = runtime_store.committed_request_projection_metadata()?;
    let mut hydrated = RequestLifecycleProjectionState {
        next_request_id: metadata
            .max_numeric_request_id
            .map(|request_id| request_id.saturating_add(1))
            .unwrap_or(1),
        committed_terminal_count: metadata.terminal_count,
        ..RequestLifecycleProjectionState::default()
    };

    let recent = runtime_store.query_committed_requests(&CommittedRequestQuery {
        limit: recent_finished_max(),
        ..CommittedRequestQuery::default()
    })?;
    hydrated.recent_finished.extend(
        recent
            .items
            .into_iter()
            .map(|projection| projection.payload.finished_request),
    );

    let mut cursor = None;
    let cutoff_ms = usage_rollup_cutoff_ms(unix_now_ms());

    loop {
        let page = runtime_store.query_committed_requests(&CommittedRequestQuery {
            limit: RUNTIME_PROJECTION_PAGE_SIZE,
            cursor,
            terminal_at_or_after_unix_ms: Some(cutoff_ms),
            ..CommittedRequestQuery::default()
        })?;
        for projection in page.items {
            let accounting_scope = projection.payload.accounting_scope;
            let billable_usage = projection.payload.billable_usage;
            let finished = projection.payload.finished_request;
            if accounting_scope == RequestAccountingScope::Economic {
                hydrated.attribution_index.record(&finished);
                let rollup = hydrated
                    .usage_rollups
                    .entry(finished.service.clone())
                    .or_default();
                if record_finished_request_into_usage_rollup(
                    rollup,
                    &projection.logical_request_id.to_string(),
                    &finished,
                ) {
                    record_finished_request_into_operator_usage_summary(
                        &mut hydrated.operator_usage_summaries,
                        &finished,
                        billable_usage.as_ref(),
                    );
                    hydrate_session_stats_newest_first(&mut hydrated.session_stats, &finished);
                }
            }
        }
        let Some(next_cursor) = page.next_cursor else {
            break;
        };
        cursor = Some(next_cursor);
    }

    Ok(hydrated)
}

fn usage_rollup_keep_days() -> i32 {
    std::env::var("CODEX_HELPER_USAGE_ROLLUP_KEEP_DAYS")
        .ok()
        .and_then(|value| value.trim().parse::<i32>().ok())
        .filter(|days| *days > 0)
        .unwrap_or(60)
}

fn usage_rollup_cutoff_day(now_ms: u64) -> i32 {
    usage_day::local_day_from_ms(now_ms).saturating_sub(usage_rollup_keep_days())
}

fn usage_rollup_cutoff_ms(now_ms: u64) -> u64 {
    let keep_days = usage_rollup_keep_days();
    usage_day::local_day_window(usage_rollup_cutoff_day(now_ms))
        .map(|window| window.start_ms)
        .unwrap_or_else(|| now_ms.saturating_sub((keep_days as u64).saturating_mul(DAY_MS)))
}

fn hydrate_session_stats_newest_first(
    stats: &mut HashMap<String, HashMap<String, SessionStats>>,
    finished: &FinishedRequest,
) {
    let Some(session_id) = finished.session_id.as_deref() else {
        return;
    };
    let entry = stats
        .entry(finished.service.clone())
        .or_default()
        .entry(session_id.to_string())
        .or_default();
    entry.turns_total = entry.turns_total.saturating_add(1);
    if entry.last_session_identity_source.is_none() {
        entry.last_session_identity_source = finished.session_identity_source;
    }
    if entry.last_client_name.is_none() {
        entry.last_client_name = finished.client_name.clone();
    }
    if entry.last_client_addr.is_none() {
        entry.last_client_addr = finished.client_addr.clone();
    }
    if entry.last_model.is_none() {
        entry.last_model = finished.model.clone();
    }
    if entry.last_reasoning_effort.is_none() {
        entry.last_reasoning_effort = finished.reasoning_effort.clone();
    }
    if entry.last_service_tier.is_none() {
        entry.last_service_tier = finished.service_tier.clone();
    }
    if entry.last_provider_id.is_none() {
        entry.last_provider_id = finished.provider_id.clone();
    }
    if entry.last_route_decision.is_none() {
        entry.last_route_decision = finished.route_decision.clone();
    }
    if let Some(usage) = finished.usage.as_ref() {
        if entry.last_usage.is_none() {
            entry.last_usage = Some(usage.clone());
        }
        entry.total_usage.add_assign(usage);
        entry.turns_with_usage = entry.turns_with_usage.saturating_add(1);
    }
    if finished
        .usage
        .as_ref()
        .is_some_and(|usage| usage.output_tokens > 0)
    {
        if entry.last_output_tokens_per_second.is_none() {
            entry.last_output_tokens_per_second = finished.observability.output_tokens_per_second;
        }
        if let Some(generation_ms) = finished.observability.generation_ms {
            entry.output_generation_ms_total = entry
                .output_generation_ms_total
                .saturating_add(generation_ms);
        }
    }
    entry.avg_output_tokens_per_second =
        self::session_identity::token_weighted_output_tokens_per_second(
            entry.total_usage.output_tokens,
            entry.output_generation_ms_total,
        );
    if entry.last_ended_at_ms.is_none() {
        entry.last_status = Some(finished.status_code);
        entry.last_duration_ms = Some(finished.duration_ms);
        entry.last_ended_at_ms = Some(finished.ended_at_ms);
    }
    entry.last_seen_ms = entry.last_seen_ms.max(finished.ended_at_ms);
}

fn prune_usage_entity_days(
    by_day: &mut HashMap<String, HashMap<i32, UsageBucket>>,
    cutoff_day: i32,
) {
    by_day.retain(|_, days| {
        days.retain(|day, _| *day >= cutoff_day);
        !days.is_empty()
    });
}

fn prune_operator_usage_summary_days(summaries: &mut OperatorUsageSummaryMap, cutoff_day: i32) {
    summaries.retain(|_, groups| {
        groups.retain(|_, values| {
            values.retain(|_, days| {
                days.retain(|day, _| *day >= cutoff_day);
                !days.is_empty()
            });
            !values.is_empty()
        });
        !groups.is_empty()
    });
}

fn prune_lru_cache<T>(
    cache: &mut HashMap<String, T>,
    max_entries: usize,
    last_seen: impl Fn(&T) -> u64,
) {
    if max_entries == 0 || cache.len() <= max_entries {
        return;
    }

    let mut keys = cache
        .iter()
        .map(|(key, value)| (key.clone(), last_seen(value)))
        .collect::<Vec<_>>();
    keys.sort_by_key(|(_, seen)| *seen);
    let remove_count = keys.len().saturating_sub(max_entries);
    for (key, _) in keys.into_iter().take(remove_count) {
        cache.remove(&key);
    }
}

fn session_affinity_limit(max_entries: usize) -> SessionAffinityLimit {
    if max_entries == 0 {
        SessionAffinityLimit::Unlimited
    } else {
        SessionAffinityLimit::MaxEntries(max_entries)
    }
}

fn session_affinity_identity_source(
    source: SessionIdentitySource,
) -> SessionAffinityIdentitySource {
    match source {
        SessionIdentitySource::Header => SessionAffinityIdentitySource::Header,
        SessionIdentitySource::BodySessionId => SessionAffinityIdentitySource::BodySessionId,
        SessionIdentitySource::PromptCacheKey => SessionAffinityIdentitySource::PromptCacheKey,
        SessionIdentitySource::MetadataSessionId => {
            SessionAffinityIdentitySource::MetadataSessionId
        }
        SessionIdentitySource::PreviousResponseId => {
            SessionAffinityIdentitySource::PreviousResponseId
        }
    }
}

fn session_identity_source(source: SessionAffinityIdentitySource) -> SessionIdentitySource {
    match source {
        SessionAffinityIdentitySource::Header => SessionIdentitySource::Header,
        SessionAffinityIdentitySource::BodySessionId => SessionIdentitySource::BodySessionId,
        SessionAffinityIdentitySource::PromptCacheKey => SessionIdentitySource::PromptCacheKey,
        SessionAffinityIdentitySource::MetadataSessionId => {
            SessionIdentitySource::MetadataSessionId
        }
        SessionAffinityIdentitySource::PreviousResponseId => {
            SessionIdentitySource::PreviousResponseId
        }
    }
}

fn session_route_affinity_from_record(record: SessionAffinityRecord) -> SessionRouteAffinity {
    SessionRouteAffinity {
        route_graph_key: record.route_graph_key,
        session_identity_source: record.session_identity_source.map(session_identity_source),
        provider_endpoint: record.provider_endpoint,
        upstream_base_url: record.upstream_base_url,
        route_path: record.route_path,
        last_selected_at_ms: record.last_selected_at_unix_ms,
        last_changed_at_ms: record.last_changed_at_unix_ms,
        change_reason: record.change_reason,
    }
}

fn session_affinity_record(
    session_id: &str,
    affinity: &SessionRouteAffinity,
) -> SessionAffinityRecord {
    SessionAffinityRecord {
        session_id: session_id.to_string(),
        route_graph_key: affinity.route_graph_key.clone(),
        session_identity_source: affinity
            .session_identity_source
            .map(session_affinity_identity_source),
        provider_endpoint: affinity.provider_endpoint.clone(),
        upstream_base_url: affinity.upstream_base_url.clone(),
        route_path: affinity.route_path.clone(),
        last_selected_at_unix_ms: affinity.last_selected_at_ms,
        last_changed_at_unix_ms: affinity.last_changed_at_ms,
        change_reason: affinity.change_reason.clone(),
    }
}

fn next_session_route_affinity(
    existing: Option<SessionRouteAffinity>,
    target: SessionRouteAffinityTarget,
    reason_hint: Option<String>,
    now_ms: u64,
) -> SessionRouteAffinity {
    match existing {
        Some(mut existing) if target.same_target(&existing) => {
            existing.last_selected_at_ms = existing.last_selected_at_ms.max(now_ms);
            if target.session_identity_source.is_some() {
                existing.session_identity_source = target.session_identity_source;
            }
            existing
        }
        Some(existing) => {
            let selected_at_ms = existing.last_selected_at_ms.max(now_ms);
            SessionRouteAffinity {
                route_graph_key: target.route_graph_key,
                session_identity_source: target.session_identity_source,
                provider_endpoint: target.provider_endpoint,
                upstream_base_url: target.upstream_base_url,
                route_path: target.route_path,
                last_selected_at_ms: selected_at_ms,
                last_changed_at_ms: selected_at_ms,
                change_reason: reason_hint.unwrap_or_else(|| "target_changed".to_string()),
            }
        }
        None => SessionRouteAffinity {
            route_graph_key: target.route_graph_key,
            session_identity_source: target.session_identity_source,
            provider_endpoint: target.provider_endpoint,
            upstream_base_url: target.upstream_base_url,
            route_path: target.route_path,
            last_selected_at_ms: now_ms,
            last_changed_at_ms: now_ms,
            change_reason: reason_hint.unwrap_or_else(|| "first_success".to_string()),
        },
    }
}

fn provider_policy_snapshot_with_projection(
    current: &ProviderPolicySnapshot,
    policy_revision: u64,
    projection: ProviderEligibilityProjection,
) -> ProviderPolicySnapshot {
    let mut projections = current.projections.clone();
    if let Some(existing) = projections
        .iter_mut()
        .find(|existing| existing.provider_endpoint == projection.provider_endpoint)
    {
        *existing = projection;
    } else {
        projections.push(projection);
    }
    projections.sort_by(|left, right| {
        left.provider_endpoint
            .stable_key()
            .cmp(&right.provider_endpoint.stable_key())
    });
    ProviderPolicySnapshot {
        policy_revision,
        projections,
    }
}

fn policy_action_projection(
    projection: &ProviderEligibilityProjection,
    now_ms: u64,
) -> Option<PolicyActionProjection> {
    let action = projection.active_action.as_ref()?;
    let cooldown_remaining_secs = action
        .expires_at_unix_ms
        .map(|expires_at| expires_at.saturating_sub(now_ms).div_ceil(1_000))
        .filter(|remaining| *remaining > 0);
    Some(PolicyActionProjection {
        provider_endpoint_key: projection.provider_endpoint.clone(),
        active_cooldown: projection.automatic == ProviderAutomaticEligibility::Blocked,
        code: action
            .code
            .clone()
            .or_else(|| Some(action.action_kind.clone())),
        cooldown_remaining_secs,
        reason: Some(action.reason.clone()),
        action_id: Some(action.action_id.to_string()),
    })
}

#[derive(Debug, Clone)]
pub(crate) struct AttemptProviderScopeCapture {
    pub endpoint: reqwest::Url,
    pub route_scope: String,
    pub account_fingerprint: AccountFingerprint,
}

#[derive(Debug, Clone)]
pub(crate) struct CapturedUpstreamAttemptContext {
    request_id: u64,
    logical_request: LogicalRequestHandle,
    runtime_revision: u64,
    runtime_digest: String,
    route_evidence: AttemptRouteEvidence,
    scope: ProviderCatalogScope,
    provider_epoch: Option<ProviderCatalogEpoch>,
    request_contract: Option<ProviderModelRequestContract>,
}

impl CapturedUpstreamAttemptContext {
    pub(crate) fn request_contract(&self) -> Option<&ProviderModelRequestContract> {
        self.request_contract.as_ref()
    }

    pub(crate) fn provider_epoch(&self) -> Option<&ProviderCatalogEpoch> {
        self.provider_epoch.as_ref()
    }
}

fn freeze_provider_epoch(
    scope: &ProviderCatalogScope,
    epoch: Option<&ProviderCatalogEpoch>,
) -> FrozenProviderEpochIdentity {
    FrozenProviderEpochIdentity {
        scope: FrozenProviderCatalogScope {
            adapter: scope.adapter(),
            endpoint_origin: scope.endpoint_origin().to_string(),
            route_scope: scope.route_scope().to_string(),
            account_fingerprint: scope.account_fingerprint().to_string(),
            config_revision: scope.config_revision().to_string(),
        },
        catalog_revision: epoch.map(|epoch| epoch.revision().as_str().to_string()),
        pricing_revision: epoch.map(|epoch| epoch.pricing_revision().as_str().to_string()),
    }
}

fn winning_route_decision(
    previous: Option<RouteDecisionProvenance>,
    route: &AttemptRouteEvidence,
) -> RouteDecisionProvenance {
    let mut decision = previous.unwrap_or_default();
    let model_source = decision
        .effective_model
        .as_ref()
        .map(|value| value.source)
        .unwrap_or(RouteValueSource::RuntimeFallback);
    let upstream_source = decision
        .effective_upstream_base_url
        .as_ref()
        .map(|value| value.source)
        .unwrap_or(RouteValueSource::RuntimeFallback);
    decision.effective_model = route
        .mapped_model
        .clone()
        .map(|value| ResolvedRouteValue::new(value, model_source));
    decision.effective_upstream_base_url = route
        .upstream_base_url
        .clone()
        .map(|value| ResolvedRouteValue::new(value, upstream_source));
    decision.provider_id = route.provider_id.clone();
    decision.endpoint_id = route.endpoint_id.clone();
    decision.route_path = route.route_path.clone();
    decision
}

/// Runtime-only state for the proxy process.
///
/// Most state is process-local. Session route affinity is persisted in the
/// helper-owned runtime store because compaction can depend on endpoint continuity.
#[derive(Debug)]
pub struct ProxyState {
    session_stats_ttl_ms: u64,
    // Bindings are sticky by default; operators can opt into pruning with a separate TTL.
    session_binding_ttl_ms: u64,
    session_binding_max_entries: usize,
    session_route_affinity_ttl_ms: u64,
    session_route_affinity_max_entries: usize,
    session_route_affinity_updates: AsyncMutex<()>,
    session_route_reservations: AsyncMutex<HashMap<String, OwnedSessionRouteReservation>>,
    session_route_control_locks: AsyncMutex<HashMap<String, Weak<AsyncMutex<()>>>>,
    session_route_control_owner: Arc<()>,
    session_transcript_path_cache_ttl_ms: u64,
    session_transcript_path_cache_max_entries: usize,
    session_bindings: RwLock<HashMap<String, SessionBindingEntry>>,
    session_transcript_path_cache: RwLock<HashMap<String, SessionTranscriptPathCacheEntry>>,
    request_lifecycle_projection: RwLock<RequestLifecycleProjectionState>,
    provider_balances: RwLock<ProviderBalanceMap>,
    provider_balance_refresh_coordinator: Arc<ProviderBalanceRefreshCoordinator>,
    quota_pool_registry: SyncRwLock<QuotaPoolRegistry>,
    quota_registry_document_revision: AtomicU64,
    quota_identity: RuntimeQuotaIdentity,
    provider_endpoint_runtime_health: RwLock<HashMap<String, ProviderEndpointRuntimeHealthState>>,
    provider_policy_updates: AsyncMutex<()>,
    provider_policy_snapshot: RwLock<Arc<ProviderPolicySnapshot>>,
    routing_operator_control: RwLock<RoutingOperatorControlSnapshot>,
    state_version_tx: watch::Sender<u64>,
    operator_capture: RwLock<()>,
    #[cfg(test)]
    terminal_publication_pause: AsyncMutex<Option<TerminalPublicationPause>>,
    #[cfg(test)]
    operator_aggregation_pause: AsyncMutex<Option<OperatorAggregationPause>>,
    #[cfg(test)]
    provider_balance_lock_wait_signal: AsyncMutex<Option<tokio::sync::oneshot::Sender<()>>>,
    #[cfg(test)]
    session_route_control_lock_wait_signal: AsyncMutex<Option<tokio::sync::oneshot::Sender<()>>>,
    runtime_store: Arc<RuntimeStore>,
}

impl ProxyState {
    #[cfg(test)]
    pub(crate) fn new() -> Arc<Self> {
        Self::new_with_runtime_store(Self::isolated_runtime_store())
            .expect("hydrate isolated runtime projections")
    }

    pub fn new_with_runtime_store(
        runtime_store: Arc<RuntimeStore>,
    ) -> Result<Arc<Self>, RuntimeStoreError> {
        let session_stats_ttl_ms = 30_u64 * 60 * 1000;
        let binding_ttl_secs = std::env::var("CODEX_HELPER_SESSION_BINDING_TTL_SECS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        let binding_ttl_ms = binding_ttl_secs.saturating_mul(1000);
        let binding_max_entries = std::env::var("CODEX_HELPER_SESSION_BINDING_MAX_ENTRIES")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(2_000);
        let route_affinity_ttl_secs = std::env::var("CODEX_HELPER_SESSION_ROUTE_AFFINITY_TTL_SECS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        let route_affinity_ttl_ms = route_affinity_ttl_secs.saturating_mul(1000);
        let route_affinity_max_entries =
            std::env::var("CODEX_HELPER_SESSION_ROUTE_AFFINITY_MAX_ENTRIES")
                .ok()
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(5_000);

        let transcript_path_cache_ttl_secs =
            std::env::var("CODEX_HELPER_SESSION_TRANSCRIPT_PATH_CACHE_TTL_SECS")
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(30);
        let transcript_path_cache_ttl_ms = transcript_path_cache_ttl_secs.saturating_mul(1000);
        let transcript_path_cache_max_entries =
            std::env::var("CODEX_HELPER_SESSION_TRANSCRIPT_PATH_CACHE_MAX_ENTRIES")
                .ok()
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(5_000);

        Self::new_with_runtime_policy_and_store(
            RuntimePolicy {
                session_stats_ttl_ms,
                session_binding_ttl_ms: binding_ttl_ms,
                session_binding_max_entries: binding_max_entries,
                session_route_affinity_ttl_ms: route_affinity_ttl_ms,
                session_route_affinity_max_entries: route_affinity_max_entries,
                session_transcript_path_cache_ttl_ms: transcript_path_cache_ttl_ms,
                session_transcript_path_cache_max_entries: transcript_path_cache_max_entries,
            },
            runtime_store,
        )
    }

    #[cfg(test)]
    fn new_with_runtime_policy(policy: RuntimePolicy) -> Arc<Self> {
        Self::new_with_runtime_policy_and_store(policy, Self::isolated_runtime_store())
            .expect("hydrate isolated runtime projections")
    }

    fn new_with_runtime_policy_and_store(
        policy: RuntimePolicy,
        runtime_store: Arc<RuntimeStore>,
    ) -> Result<Arc<Self>, RuntimeStoreError> {
        let (quota_pool_registry, quota_identity, quota_registry_document_revision) =
            load_quota_runtime_state(runtime_store.as_ref())?;
        let hydrated = hydrate_runtime_projections(&runtime_store)?;
        runtime_store.prune_session_affinities(
            unix_now_ms(),
            policy.session_route_affinity_ttl_ms,
            session_affinity_limit(policy.session_route_affinity_max_entries),
        )?;
        let provider_policy_snapshot = Arc::new(runtime_store.provider_policy_snapshot()?);

        Ok(Arc::new(Self {
            session_stats_ttl_ms: policy.session_stats_ttl_ms,
            session_binding_ttl_ms: policy.session_binding_ttl_ms,
            session_binding_max_entries: policy.session_binding_max_entries,
            session_route_affinity_ttl_ms: policy.session_route_affinity_ttl_ms,
            session_route_affinity_max_entries: policy.session_route_affinity_max_entries,
            session_route_affinity_updates: AsyncMutex::new(()),
            session_route_reservations: AsyncMutex::new(HashMap::new()),
            session_route_control_locks: AsyncMutex::new(HashMap::new()),
            session_route_control_owner: Arc::new(()),
            session_transcript_path_cache_ttl_ms: policy.session_transcript_path_cache_ttl_ms,
            session_transcript_path_cache_max_entries: policy
                .session_transcript_path_cache_max_entries,
            session_bindings: RwLock::new(HashMap::new()),
            session_transcript_path_cache: RwLock::new(HashMap::new()),
            request_lifecycle_projection: RwLock::new(hydrated),
            provider_balances: RwLock::new(HashMap::new()),
            provider_balance_refresh_coordinator: Arc::new(
                ProviderBalanceRefreshCoordinator::default(),
            ),
            quota_pool_registry: SyncRwLock::new(quota_pool_registry),
            quota_registry_document_revision: AtomicU64::new(
                quota_registry_document_revision.unwrap_or(0),
            ),
            quota_identity,
            provider_endpoint_runtime_health: RwLock::new(HashMap::new()),
            provider_policy_updates: AsyncMutex::new(()),
            provider_policy_snapshot: RwLock::new(provider_policy_snapshot),
            routing_operator_control: RwLock::new(RoutingOperatorControlSnapshot::default()),
            state_version_tx: watch::channel(0).0,
            operator_capture: RwLock::new(()),
            #[cfg(test)]
            terminal_publication_pause: AsyncMutex::new(None),
            #[cfg(test)]
            operator_aggregation_pause: AsyncMutex::new(None),
            #[cfg(test)]
            provider_balance_lock_wait_signal: AsyncMutex::new(None),
            #[cfg(test)]
            session_route_control_lock_wait_signal: AsyncMutex::new(None),
            runtime_store,
        }))
    }

    #[cfg(test)]
    fn isolated_runtime_store() -> Arc<RuntimeStore> {
        Arc::new(
            RuntimeStore::open_in_memory()
                .expect("an isolated in-memory runtime store should open"),
        )
    }

    pub(crate) fn runtime_store(&self) -> &RuntimeStore {
        self.runtime_store.as_ref()
    }

    pub(crate) fn derive_usage_account_fingerprint(
        &self,
        token: &[u8],
        new_api_user_id: Option<&str>,
    ) -> String {
        self.quota_identity
            .derive_usage_account_fingerprint(token, new_api_user_id)
    }

    pub(crate) fn derive_provider_account_fingerprint(
        &self,
        credential_scope: Option<&str>,
        final_headers: &http::HeaderMap,
    ) -> AccountFingerprint {
        if let Some(credential_scope) = credential_scope {
            return AccountFingerprint::from_credential_scope(credential_scope);
        }
        self.quota_identity
            .derive_provider_account_fingerprint(final_headers)
            .map(AccountFingerprint::from_keyed_account_digest)
            .unwrap_or_else(AccountFingerprint::unscoped)
    }

    pub(crate) fn provider_balance_refresh_coordinator(
        &self,
    ) -> Arc<ProviderBalanceRefreshCoordinator> {
        Arc::clone(&self.provider_balance_refresh_coordinator)
    }

    fn with_runtime_store_blocking<T>(&self, operation: impl FnOnce(&RuntimeStore) -> T) -> T {
        runtime_store_blocking(|| operation(self.runtime_store.as_ref()))
    }

    fn quota_registry_document_revision(&self) -> Option<u64> {
        match self
            .quota_registry_document_revision
            .load(Ordering::Acquire)
        {
            0 => None,
            revision => Some(revision),
        }
    }

    fn publish_quota_registry_document_revision(&self, revision: u64) {
        debug_assert!(revision > 0);
        self.quota_registry_document_revision
            .store(revision, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn runtime_store_handle(&self) -> Arc<RuntimeStore> {
        self.runtime_store.clone()
    }

    #[cfg(test)]
    async fn hold_request_publication_for_test(&self) -> impl Drop + '_ {
        self.request_lifecycle_projection.write().await
    }

    #[cfg(test)]
    async fn hold_attempt_publication_for_test(&self) -> impl Drop + '_ {
        self.request_lifecycle_projection.write().await
    }

    #[cfg(test)]
    pub(crate) async fn hold_provider_runtime_health_publication_for_test(&self) -> impl Drop + '_ {
        self.provider_endpoint_runtime_health.write().await
    }

    #[cfg(test)]
    async fn lifecycle_handle_count_for_test(&self) -> usize {
        self.request_lifecycle_projection
            .read()
            .await
            .lifecycle_handles
            .len()
    }

    pub fn subscribe_state_changes(&self) -> watch::Receiver<u64> {
        self.state_version_tx.subscribe()
    }

    pub fn operator_revision(&self) -> u64 {
        *self.state_version_tx.borrow()
    }

    #[cfg(test)]
    pub(crate) async fn operator_ledger_revision(&self) -> OperatorLedgerRevision {
        let request_state = self.request_lifecycle_projection.read().await;
        OperatorLedgerRevision::new(
            self.runtime_store.identity().store_id(),
            request_state.committed_terminal_count,
        )
    }

    #[cfg(test)]
    pub(crate) async fn operator_usage_summaries(
        &self,
        service_name: &str,
        limit: usize,
    ) -> Vec<RequestUsageSummary> {
        let request_state = self.request_lifecycle_projection.read().await;
        operator_usage_summaries(&request_state, service_name, limit)
    }

    pub(crate) async fn capture_operator_lifecycle_snapshot(
        &self,
        service_name: &str,
        recent_limit: usize,
    ) -> OperatorLifecycleSnapshot {
        let _capture = self.operator_capture.read().await;
        let state_revision = self.operator_revision();
        let request_state = self.request_lifecycle_projection.read().await;
        let mut active_requests = request_state
            .active_requests
            .values()
            .filter(|request| request.service == service_name)
            .cloned()
            .collect::<Vec<_>>();
        active_requests.sort_by_key(|request| request.started_at_ms);
        let recent_finished = request_state
            .recent_finished
            .iter()
            .filter(|request| request.service == service_name)
            .take(recent_limit)
            .cloned()
            .collect();

        OperatorLifecycleSnapshot {
            state_revision,
            ledger_revision: OperatorLedgerRevision::new(
                self.runtime_store.identity().store_id(),
                request_state.committed_terminal_count,
            ),
            active_requests,
            recent_finished,
            session_stats: request_state
                .session_stats
                .get(service_name)
                .cloned()
                .unwrap_or_default(),
            usage_rollup: request_state
                .usage_rollups
                .get(service_name)
                .map(snapshot_usage_rollup),
            usage_summaries: request_state
                .operator_usage_summaries
                .get(service_name)
                .cloned()
                .unwrap_or_default(),
        }
    }

    #[cfg(test)]
    async fn pause_next_terminal_publication_after_commit_for_test(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (committed_tx, committed_rx) = tokio::sync::oneshot::channel();
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
        let mut pause = self.terminal_publication_pause.lock().await;
        assert!(
            pause.is_none(),
            "a terminal publication pause is already armed"
        );
        *pause = Some(TerminalPublicationPause {
            committed: committed_tx,
            resume: resume_rx,
        });
        (committed_rx, resume_tx)
    }

    #[cfg(test)]
    async fn pause_terminal_publication_after_commit_for_test(&self) {
        let pause = self.terminal_publication_pause.lock().await.take();
        if let Some(pause) = pause {
            let _ = pause.committed.send(());
            let _ = pause.resume.await;
        }
    }

    #[cfg(test)]
    async fn arm_provider_balance_lock_wait_signal_for_test(
        &self,
    ) -> tokio::sync::oneshot::Receiver<()> {
        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
        let mut signal = self.provider_balance_lock_wait_signal.lock().await;
        assert!(
            signal.is_none(),
            "a provider balance lock wait signal is already armed"
        );
        *signal = Some(reached_tx);
        reached_rx
    }

    #[cfg(test)]
    async fn signal_provider_balance_lock_wait_for_test(&self) {
        if let Some(signal) = self.provider_balance_lock_wait_signal.lock().await.take() {
            let _ = signal.send(());
        }
    }

    #[cfg(test)]
    pub(crate) async fn pause_next_operator_aggregation_after_snapshot_for_test(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (captured_tx, captured_rx) = tokio::sync::oneshot::channel();
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
        let mut pause = self.operator_aggregation_pause.lock().await;
        assert!(
            pause.is_none(),
            "an operator aggregation pause is already armed"
        );
        *pause = Some(OperatorAggregationPause {
            captured: captured_tx,
            resume: resume_rx,
        });
        (captured_rx, resume_tx)
    }

    #[cfg(test)]
    pub(crate) async fn pause_operator_aggregation_after_snapshot_for_test(&self) {
        let pause = self.operator_aggregation_pause.lock().await.take();
        if let Some(pause) = pause {
            let _ = pause.captured.send(());
            let _ = pause.resume.await;
        }
    }

    fn notify_state_changed(&self) {
        self.state_version_tx.send_modify(|version| {
            *version = version.wrapping_add(1);
        });
    }

    pub(crate) fn notify_runtime_snapshot_changed(&self) {
        self.notify_state_changed();
    }

    pub async fn get_session_binding(&self, session_id: &str) -> Option<SessionBinding> {
        let guard = self.session_bindings.read().await;
        guard.get(session_id).map(|entry| entry.binding.clone())
    }

    pub async fn list_session_bindings(&self) -> HashMap<String, SessionBinding> {
        let guard = self.session_bindings.read().await;
        guard
            .iter()
            .map(|(sid, entry)| (sid.clone(), entry.binding.clone()))
            .collect()
    }

    pub async fn get_session_route_affinity(
        &self,
        session_id: &str,
    ) -> Option<SessionRouteAffinity> {
        match self.read_session_route_affinity(session_id, unix_now_ms()) {
            Ok(affinity) => affinity,
            Err(error) => {
                tracing::warn!(
                    session_id,
                    error = %error,
                    "failed to read session route affinity from runtime store"
                );
                None
            }
        }
    }

    /// Reads route affinity without pruning expired durable state.
    pub async fn peek_session_route_affinity(
        &self,
        session_id: &str,
    ) -> Option<SessionRouteAffinity> {
        self.get_session_route_affinity(session_id).await
    }

    pub async fn list_session_route_affinities(&self) -> HashMap<String, SessionRouteAffinity> {
        match self.with_runtime_store_blocking(|runtime_store| {
            runtime_store.list_session_affinities(unix_now_ms(), self.session_route_affinity_ttl_ms)
        }) {
            Ok(records) => records
                .into_iter()
                .map(|record| {
                    let session_id = record.session_id.clone();
                    (session_id, session_route_affinity_from_record(record))
                })
                .collect(),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "failed to list session route affinities from runtime store"
                );
                HashMap::new()
            }
        }
    }

    async fn session_route_control_lock(&self, session_id: &str) -> Arc<AsyncMutex<()>> {
        let key = session_id.trim().to_string();
        {
            let mut locks = self.session_route_control_locks.lock().await;
            locks.retain(|_, lock| lock.strong_count() > 0);
            if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(AsyncMutex::new(()));
                locks.insert(key, Arc::downgrade(&lock));
                lock
            }
        }
    }

    fn validate_session_route_control_guard(
        &self,
        guard: &SessionRouteControlGuard,
    ) -> Result<(), RuntimeStoreError> {
        if guard.belongs_to(&self.session_route_control_owner) {
            return Ok(());
        }
        Err(RuntimeStoreError::InvariantViolation {
            entity: "session route control guard",
            id: guard.session_id().to_string(),
            detail: "guard belongs to a different ProxyState".to_string(),
        })
    }

    #[cfg(test)]
    pub(crate) async fn signal_next_session_route_control_lock_wait_for_test(
        &self,
    ) -> tokio::sync::oneshot::Receiver<()> {
        let (waiting_tx, waiting_rx) = tokio::sync::oneshot::channel();
        let mut signal = self.session_route_control_lock_wait_signal.lock().await;
        assert!(
            signal.is_none(),
            "a session route control wait signal is already armed"
        );
        *signal = Some(waiting_tx);
        waiting_rx
    }

    #[cfg(test)]
    async fn signal_session_route_control_lock_wait_for_test(&self) {
        if let Some(signal) = self
            .session_route_control_lock_wait_signal
            .lock()
            .await
            .take()
        {
            let _ = signal.send(());
        }
    }

    /// Serializes request admission, first-target selection, and explicit route
    /// control for one canonical session ID.
    pub(crate) async fn lock_session_route_control(
        &self,
        session_id: &str,
    ) -> SessionRouteControlGuard {
        let session_id = session_id.trim().to_string();
        let lock = self.session_route_control_lock(session_id.as_str()).await;
        #[cfg(test)]
        self.signal_session_route_control_lock_wait_for_test().await;
        let guard = lock.lock_owned().await;
        SessionRouteControlGuard {
            session_id,
            owner: Arc::clone(&self.session_route_control_owner),
            _guard: guard,
        }
    }

    #[cfg(test)]
    pub(crate) async fn try_lock_session_route_control(
        &self,
        session_id: &str,
    ) -> Option<SessionRouteControlGuard> {
        let session_id = session_id.trim().to_string();
        let guard = self
            .session_route_control_lock(session_id.as_str())
            .await
            .try_lock_owned()
            .ok()?;
        Some(SessionRouteControlGuard {
            session_id,
            owner: Arc::clone(&self.session_route_control_owner),
            _guard: guard,
        })
    }

    async fn active_request_ids(&self) -> HashSet<u64> {
        self.request_lifecycle_projection
            .read()
            .await
            .active_requests
            .keys()
            .copied()
            .collect()
    }

    pub async fn get_session_route_reservation(
        &self,
        session_id: &str,
        route_graph_key: &str,
        request_id: u64,
        now_ms: u64,
    ) -> Result<SessionRouteReservationAccess, RuntimeStoreError> {
        let active_request_ids = self.active_request_ids().await;
        let _update_guard = self.session_route_affinity_updates.lock().await;
        let mut reservations = self.session_route_reservations.lock().await;
        reservations.retain(|_, reservation| {
            reservation.owner_request_id == request_id
                || active_request_ids.contains(&reservation.owner_request_id)
        });
        if let Some(existing) = reservations.get_mut(session_id) {
            if existing.owner_request_id != request_id {
                return Ok(SessionRouteReservationAccess::Busy {
                    owner_request_id: existing.owner_request_id,
                });
            }
            if existing.affinity.route_graph_key != route_graph_key {
                return Ok(SessionRouteReservationAccess::None);
            }
            existing.affinity.last_selected_at_ms = now_ms;
            return Ok(SessionRouteReservationAccess::Available(
                existing.affinity.clone(),
            ));
        }
        drop(reservations);

        Ok(self
            .read_session_route_affinity(session_id, now_ms)?
            .filter(|existing| existing.route_graph_key == route_graph_key)
            .map_or(SessionRouteReservationAccess::None, |existing| {
                SessionRouteReservationAccess::Available(existing)
            }))
    }

    pub async fn claim_session_route_reservation(
        &self,
        session_id: &str,
        target: SessionRouteAffinityTarget,
        request_id: u64,
        now_ms: u64,
    ) -> Result<SessionRouteReservationAccess, RuntimeStoreError> {
        let active_request_ids = self.active_request_ids().await;
        let _update_guard = self.session_route_affinity_updates.lock().await;
        let mut reservations = self.session_route_reservations.lock().await;
        reservations.retain(|_, reservation| {
            reservation.owner_request_id == request_id
                || active_request_ids.contains(&reservation.owner_request_id)
        });
        if let Some(existing) = reservations.get_mut(session_id) {
            if existing.owner_request_id != request_id {
                return Ok(SessionRouteReservationAccess::Busy {
                    owner_request_id: existing.owner_request_id,
                });
            }
            if target.same_target(&existing.affinity) {
                existing.affinity.last_selected_at_ms = now_ms;
                if target.session_identity_source.is_some() {
                    existing.affinity.session_identity_source = target.session_identity_source;
                }
            } else {
                existing.affinity = SessionRouteAffinity {
                    route_graph_key: target.route_graph_key,
                    session_identity_source: target.session_identity_source,
                    provider_endpoint: target.provider_endpoint,
                    upstream_base_url: target.upstream_base_url,
                    route_path: target.route_path,
                    last_selected_at_ms: now_ms,
                    last_changed_at_ms: now_ms,
                    change_reason: "provisional_target_changed".to_string(),
                };
            }
            return Ok(SessionRouteReservationAccess::Available(
                existing.affinity.clone(),
            ));
        }

        if let Some(existing) = self.read_session_route_affinity(session_id, now_ms)?
            && target.same_target(&existing)
        {
            return Ok(SessionRouteReservationAccess::Available(existing));
        }

        let reservation = OwnedSessionRouteReservation {
            affinity: SessionRouteAffinity {
                route_graph_key: target.route_graph_key,
                session_identity_source: target.session_identity_source,
                provider_endpoint: target.provider_endpoint,
                upstream_base_url: target.upstream_base_url,
                route_path: target.route_path,
                last_selected_at_ms: now_ms,
                last_changed_at_ms: now_ms,
                change_reason: "initial_selection".to_string(),
            },
            owner_request_id: request_id,
        };
        reservations.insert(session_id.to_string(), reservation.clone());
        Ok(SessionRouteReservationAccess::Available(
            reservation.affinity,
        ))
    }

    async fn prepare_session_route_affinity_transaction(
        &self,
        success: &SessionRouteAffinitySuccess,
        now_ms: u64,
    ) -> Result<Option<SessionRouteAffinity>, RuntimeStoreError> {
        let existing = self.read_session_route_affinity(success.session_id.as_str(), now_ms)?;
        let reservation = self
            .session_route_reservations
            .lock()
            .await
            .get(success.session_id.as_str())
            .cloned();

        match reservation {
            Some(reservation) if reservation.owner_request_id == success.request_id => {
                if !success.target.same_target(&reservation.affinity) {
                    return Err(RuntimeStoreError::InvariantViolation {
                        entity: "session route reservation",
                        id: success.session_id.clone(),
                        detail: format!(
                            "request {} succeeded on a target that differs from its reservation",
                            success.request_id
                        ),
                    });
                }
                Ok(Some(next_session_route_affinity(
                    existing,
                    success.target.clone(),
                    success.reason_hint.clone(),
                    now_ms,
                )))
            }
            Some(reservation) => {
                tracing::warn!(
                    request_id = success.request_id,
                    reservation_owner_request_id = reservation.owner_request_id,
                    session_id = success.session_id,
                    "preserving newer session route reservation after stale request success"
                );
                Ok(None)
            }
            None => match existing {
                Some(existing) if success.target.same_target(&existing) => {
                    Ok(Some(next_session_route_affinity(
                        Some(existing),
                        success.target.clone(),
                        success.reason_hint.clone(),
                        now_ms,
                    )))
                }
                Some(existing) => {
                    tracing::warn!(
                        request_id = success.request_id,
                        session_id = success.session_id,
                        current_provider_endpoint = existing.provider_endpoint.stable_key(),
                        successful_provider_endpoint =
                            success.target.provider_endpoint.stable_key(),
                        "ignoring stale cross-target session route affinity success"
                    );
                    Ok(None)
                }
                None => Err(RuntimeStoreError::InvariantViolation {
                    entity: "session route reservation",
                    id: success.session_id.clone(),
                    detail: format!(
                        "request {} has no provisional reservation for its first success",
                        success.request_id
                    ),
                }),
            },
        }
    }

    pub async fn record_session_route_affinity_success(
        &self,
        request_id: Option<u64>,
        session_id: &str,
        target: SessionRouteAffinityTarget,
        reason_hint: Option<String>,
        now_ms: u64,
    ) -> Result<SessionRouteAffinity, RuntimeStoreError> {
        let _update_guard = self.session_route_affinity_updates.lock().await;
        let affinity = if let Some(request_id) = request_id {
            let success = SessionRouteAffinitySuccess {
                request_id,
                session_id: session_id.to_string(),
                target,
                reason_hint,
            };
            self.prepare_session_route_affinity_transaction(&success, now_ms)
                .await?
                .ok_or_else(|| RuntimeStoreError::InvariantViolation {
                    entity: "session route affinity",
                    id: session_id.to_string(),
                    detail: format!("request {request_id} no longer owns the affinity transition"),
                })?
        } else {
            let existing = self.read_session_route_affinity(session_id, now_ms)?;
            next_session_route_affinity(existing, target, reason_hint, now_ms)
        };
        self.with_runtime_store_blocking(|runtime_store| {
            runtime_store.upsert_session_affinity(
                session_affinity_record(session_id, &affinity),
                session_affinity_limit(self.session_route_affinity_max_entries),
            )
        })?;
        if let Some(request_id) = request_id {
            let mut reservations = self.session_route_reservations.lock().await;
            if reservations
                .get(session_id)
                .is_some_and(|reservation| reservation.owner_request_id == request_id)
            {
                reservations.remove(session_id);
            }
        }
        drop(_update_guard);
        self.notify_state_changed();
        Ok(affinity)
    }

    fn read_session_route_affinity(
        &self,
        session_id: &str,
        now_ms: u64,
    ) -> Result<Option<SessionRouteAffinity>, RuntimeStoreError> {
        self.with_runtime_store_blocking(|runtime_store| {
            runtime_store
                .get_session_affinity(session_id, now_ms, self.session_route_affinity_ttl_ms)
                .map(|record| record.map(session_route_affinity_from_record))
        })
    }

    pub async fn set_session_binding(&self, binding: SessionBinding) {
        let mut guard = self.session_bindings.write().await;
        let binding = if let Some(existing) = guard.get(binding.session_id.as_str()) {
            SessionBinding {
                created_at_ms: existing.binding.created_at_ms,
                ..binding
            }
        } else {
            binding
        };
        guard.insert(binding.session_id.clone(), SessionBindingEntry { binding });
        self.notify_state_changed();
    }

    pub async fn clear_session_binding(&self, session_id: &str) {
        let mut guard = self.session_bindings.write().await;
        if guard.remove(session_id).is_some() {
            self.notify_state_changed();
        }
    }

    pub async fn touch_session_binding(&self, session_id: &str, now_ms: u64) {
        let mut guard = self.session_bindings.write().await;
        if let Some(entry) = guard.get_mut(session_id) {
            entry.binding.last_seen_ms = now_ms;
        }
    }

    #[cfg(test)]
    pub async fn route_plan_runtime_state_for_provider_endpoints(
        &self,
        service_name: &str,
    ) -> RoutePlanRuntimeState {
        let policy_snapshot = self.capture_provider_policy_snapshot().await;
        let mut runtime = RoutePlanRuntimeState::default();
        let now = std::time::Instant::now();
        {
            let mut guard = self.provider_endpoint_runtime_health.write().await;
            if let Some(per_service) = guard.get_mut(service_name) {
                let keys = per_service
                    .health
                    .keys()
                    .map(|bucket| bucket.identity.clone())
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                project_provider_endpoint_runtime_health(
                    &mut runtime,
                    per_service,
                    keys.as_slice(),
                    Some(RouteCapability::Inference),
                    now,
                );
            }
        }
        apply_provider_policy_to_route_runtime(
            &mut runtime,
            service_name,
            policy_snapshot.as_ref(),
            unix_now_ms(),
        );
        runtime
    }

    pub async fn route_plan_runtime_state_with_provider_policy(
        &self,
        service_name: &str,
        policy_snapshot: &ProviderPolicySnapshot,
        runtime_revision: u64,
        runtime_identities: &[RuntimeUpstreamIdentity],
    ) -> RoutePlanRuntimeState {
        self.route_plan_runtime_state_with_provider_policy_for_capability(
            service_name,
            policy_snapshot,
            runtime_revision,
            runtime_identities,
            Some(RouteCapability::Inference),
        )
        .await
    }

    pub(crate) async fn route_plan_runtime_state_with_provider_policy_for_capability(
        &self,
        service_name: &str,
        policy_snapshot: &ProviderPolicySnapshot,
        runtime_revision: u64,
        runtime_identities: &[RuntimeUpstreamIdentity],
        capability: Option<RouteCapability>,
    ) -> RoutePlanRuntimeState {
        let projected_keys = runtime_identities
            .iter()
            .filter_map(|identity| {
                ProviderEndpointRuntimeHealthKey::for_service(service_name, identity)
            })
            .collect::<HashSet<_>>();
        let mut runtime = RoutePlanRuntimeState::default();
        let now = std::time::Instant::now();
        {
            let mut guard = self.provider_endpoint_runtime_health.write().await;
            let per_service = guard.entry(service_name.to_string()).or_default();
            if per_service.identities_authoritative {
                if per_service
                    .active_revision
                    .is_none_or(|active_revision| runtime_revision > active_revision)
                {
                    per_service.active_revision = Some(runtime_revision);
                }
            } else {
                match per_service.active_revision {
                    Some(active_revision) if runtime_revision < active_revision => {}
                    Some(active_revision) if runtime_revision == active_revision => {
                        per_service
                            .active_identities
                            .extend(projected_keys.iter().cloned());
                    }
                    _ => {
                        per_service.active_revision = Some(runtime_revision);
                        per_service.active_identities = projected_keys.clone();
                        per_service
                            .health
                            .retain(|bucket, _| projected_keys.contains(&bucket.identity));
                    }
                }
            }

            let active_keys = projected_keys
                .into_iter()
                .filter(|identity| per_service.active_identities.contains(identity))
                .collect::<Vec<_>>();
            project_provider_endpoint_runtime_health(
                &mut runtime,
                per_service,
                active_keys.as_slice(),
                capability,
                now,
            );
        }

        apply_provider_policy_to_route_runtime(
            &mut runtime,
            service_name,
            policy_snapshot,
            unix_now_ms(),
        );
        runtime
    }

    pub async fn prune_provider_endpoint_runtime_for_service(
        &self,
        service_name: &str,
        runtime_revision: u64,
        active_runtime_identities: &[RuntimeUpstreamIdentity],
    ) {
        let active_identities = active_runtime_identities
            .iter()
            .filter_map(|identity| {
                ProviderEndpointRuntimeHealthKey::for_service(service_name, identity)
            })
            .collect::<HashSet<_>>();
        let mut changed = false;
        {
            let mut guard = self.provider_endpoint_runtime_health.write().await;
            let per_service = guard.entry(service_name.to_string()).or_default();
            if per_service
                .active_revision
                .is_none_or(|active_revision| runtime_revision >= active_revision)
            {
                changed = !per_service.identities_authoritative
                    || per_service.active_revision != Some(runtime_revision)
                    || per_service.active_identities != active_identities;
                per_service.active_revision = Some(runtime_revision);
                per_service.identities_authoritative = true;
                per_service.active_identities = active_identities;
                let before = per_service.health.len();
                per_service
                    .health
                    .retain(|bucket, _| per_service.active_identities.contains(&bucket.identity));
                changed |= before != per_service.health.len();
            }
        }
        if changed {
            self.notify_state_changed();
        }
    }

    pub async fn capture_provider_policy_snapshot(&self) -> Arc<ProviderPolicySnapshot> {
        self.provider_policy_snapshot.read().await.clone()
    }

    pub async fn capture_routing_operator_control(&self) -> RoutingOperatorControlSnapshot {
        self.routing_operator_control.read().await.clone()
    }

    pub async fn compare_and_set_new_session_preference(
        &self,
        service_name: &str,
        route_graph_key: &str,
        expected_revision: u64,
        target: Option<ProviderEndpointKey>,
    ) -> Result<RoutingOperatorControlCommit, RoutingOperatorControlError> {
        let service_name = service_name.trim();
        if service_name.is_empty() {
            return Err(RoutingOperatorControlError::EmptyServiceName);
        }
        let route_graph_key = route_graph_key.trim();
        if route_graph_key.is_empty() {
            return Err(RoutingOperatorControlError::EmptyRouteGraphKey);
        }
        if let Some(target) = target.as_ref()
            && target.service_name != service_name
        {
            return Err(RoutingOperatorControlError::ServiceMismatch {
                expected: service_name.to_string(),
                actual: target.service_name.clone(),
            });
        }

        let mut control = self.routing_operator_control.write().await;
        if control.revision() != expected_revision {
            return Ok(RoutingOperatorControlCommit {
                status: RoutingOperatorControlUpdate::Conflict,
                snapshot: control.clone(),
            });
        }
        let status = control.apply_new_session_preference(service_name, route_graph_key, target);
        let snapshot = control.clone();
        drop(control);
        if status == RoutingOperatorControlUpdate::Applied {
            self.notify_state_changed();
        }
        Ok(RoutingOperatorControlCommit { status, snapshot })
    }

    pub async fn compare_and_set_new_session_preference_for_policy(
        &self,
        service_name: &str,
        route_graph_key: &str,
        expected_control_revision: u64,
        expected_policy_revision: u64,
        target: Option<ProviderEndpointKey>,
    ) -> Result<RoutingOperatorControlCommit, RoutingOperatorControlError> {
        let _update_guard = self.provider_policy_updates.lock().await;
        let policy_revision = self.provider_policy_snapshot.read().await.policy_revision;
        if policy_revision != expected_policy_revision {
            return Ok(RoutingOperatorControlCommit {
                status: RoutingOperatorControlUpdate::Conflict,
                snapshot: self.routing_operator_control.read().await.clone(),
            });
        }
        self.compare_and_set_new_session_preference(
            service_name,
            route_graph_key,
            expected_control_revision,
            target,
        )
        .await
    }

    pub(crate) async fn commit_runtime_reload<T>(
        &self,
        route_graphs: &[PreparedRoutingOperatorRouteGraph],
        identities: &[RuntimeUpstreamIdentity],
        reconcile_identities: bool,
        updated_at_ms: u64,
        commit: impl FnOnce(Arc<ProviderPolicySnapshot>) -> T,
    ) -> Result<(RoutingOperatorControlSnapshot, T), RuntimeStoreError> {
        let _update_guard = self.provider_policy_updates.lock().await;
        let mut current = self.provider_policy_snapshot.write().await;
        let mut health = self.provider_endpoint_runtime_health.write().await;
        let mut control = self.routing_operator_control.write().await;

        let (next, next_health, health_changed) = if reconcile_identities {
            let (next_health, health_changed) =
                Self::replaced_active_provider_runtime_health_identities(&health, identities);
            let next = Arc::new(self.with_runtime_store_blocking(|runtime_store| {
                runtime_store.reconcile_runtime_upstream_identities(identities, updated_at_ms)
            })?);
            (next, Some(next_health), health_changed)
        } else {
            (Arc::clone(&current), None, false)
        };
        let policy_changed = **current != *next;
        if policy_changed {
            *current = Arc::clone(&next);
        }
        if let Some(next_health) = next_health
            && health_changed
        {
            *health = next_health;
        }

        let mut control_changed = false;
        for route_graph in route_graphs {
            control_changed |= control
                .reconcile_route_graph(route_graph.service_name(), route_graph.route_graph_key())
                == RoutingOperatorControlUpdate::Applied;
        }
        let committed = commit(next);
        let snapshot = control.clone();
        drop(control);
        drop(health);
        drop(current);
        drop(_update_guard);
        if policy_changed || health_changed || control_changed {
            self.notify_state_changed();
        }
        Ok((snapshot, committed))
    }

    #[cfg(test)]
    pub(crate) async fn hold_routing_operator_control_publication_for_test(
        &self,
    ) -> tokio::sync::RwLockWriteGuard<'_, RoutingOperatorControlSnapshot> {
        self.routing_operator_control.write().await
    }

    pub async fn reconcile_runtime_upstream_identities(
        &self,
        identities: &[RuntimeUpstreamIdentity],
        updated_at_ms: u64,
    ) -> Result<Arc<ProviderPolicySnapshot>, RuntimeStoreError> {
        let _update_guard = self.provider_policy_updates.lock().await;
        let mut current = self.provider_policy_snapshot.write().await;
        let mut health = self.provider_endpoint_runtime_health.write().await;
        let (next_health, health_changed) =
            Self::replaced_active_provider_runtime_health_identities(&health, identities);
        let next = Arc::new(self.with_runtime_store_blocking(|runtime_store| {
            runtime_store.reconcile_runtime_upstream_identities(identities, updated_at_ms)
        })?);
        let policy_changed = **current != *next;
        if policy_changed {
            *current = Arc::clone(&next);
        }
        if health_changed {
            *health = next_health;
        }
        if policy_changed || health_changed {
            self.notify_state_changed();
        }
        Ok(next)
    }

    fn replaced_active_provider_runtime_health_identities(
        current: &HashMap<String, ProviderEndpointRuntimeHealthState>,
        identities: &[RuntimeUpstreamIdentity],
    ) -> (HashMap<String, ProviderEndpointRuntimeHealthState>, bool) {
        let mut active_by_service =
            HashMap::<String, HashSet<ProviderEndpointRuntimeHealthKey>>::new();
        for identity in identities {
            let service_name = identity.provider_endpoint.service_name.as_str();
            let Some(key) = ProviderEndpointRuntimeHealthKey::for_service(service_name, identity)
            else {
                continue;
            };
            active_by_service
                .entry(service_name.to_string())
                .or_default()
                .insert(key);
        }

        let mut next = current.clone();
        let service_names = next
            .keys()
            .cloned()
            .chain(active_by_service.keys().cloned())
            .collect::<HashSet<_>>();
        let mut changed = false;
        for service_name in service_names {
            let active_identities = active_by_service.remove(&service_name).unwrap_or_default();
            let state = next.entry(service_name).or_default();
            changed |=
                !state.identities_authoritative || state.active_identities != active_identities;
            state.identities_authoritative = true;
            state.active_identities = active_identities;
            let before = state.health.len();
            state
                .health
                .retain(|bucket, _| state.active_identities.contains(&bucket.identity));
            changed |= before != state.health.len();
        }
        (next, changed)
    }

    pub async fn reserve_provider_observation(
        &self,
        scope: ProviderObservationScope,
        reserved_at_ms: u64,
    ) -> Result<ProviderObservationReservation, RuntimeStoreError> {
        let _update_guard = self.provider_policy_updates.lock().await;
        let mut current = self.provider_policy_snapshot.write().await;
        let reservation = self.with_runtime_store_blocking(|runtime_store| {
            runtime_store.reserve_provider_observation(scope, reserved_at_ms)
        })?;
        self.publish_provider_policy_projection(
            &mut current,
            reservation.policy_revision,
            reservation.projection.clone(),
        );
        Ok(reservation)
    }

    pub async fn commit_provider_observation(
        &self,
        reservation: ProviderObservationReservation,
        observation: ProviderObservation,
    ) -> Result<ProviderObservationCommit, RuntimeStoreError> {
        let _update_guard = self.provider_policy_updates.lock().await;
        let mut current = self.provider_policy_snapshot.write().await;
        let committed = self.with_runtime_store_blocking(|runtime_store| {
            runtime_store.commit_provider_observation(reservation.ticket, observation)
        })?;
        self.publish_provider_policy_projection(
            &mut current,
            committed.policy_revision,
            committed.projection.clone(),
        );
        Ok(committed)
    }

    pub async fn set_provider_manual_eligibility(
        &self,
        provider_endpoint: ProviderEndpointKey,
        manual: ProviderManualEligibility,
        reason: Option<String>,
        updated_at_ms: u64,
    ) -> Result<ProviderEligibilityProjection, RuntimeStoreError> {
        let _update_guard = self.provider_policy_updates.lock().await;
        let mut current = self.provider_policy_snapshot.write().await;
        let projection = self.with_runtime_store_blocking(|runtime_store| {
            runtime_store.set_provider_manual_eligibility(
                provider_endpoint,
                manual,
                reason,
                updated_at_ms,
            )
        })?;
        self.publish_provider_policy_projection(
            &mut current,
            projection.policy_revision,
            projection.clone(),
        );
        Ok(projection)
    }

    pub async fn compare_and_set_provider_manual_eligibility(
        &self,
        expected_policy_revision: u64,
        provider_endpoint: ProviderEndpointKey,
        manual: ProviderManualEligibility,
        reason: Option<String>,
        updated_at_ms: u64,
    ) -> Result<ProviderManualEligibilityCommit, RuntimeStoreError> {
        let _update_guard = self.provider_policy_updates.lock().await;
        let mut current = self.provider_policy_snapshot.write().await;
        if current.policy_revision != expected_policy_revision {
            return Ok(ProviderManualEligibilityCommit {
                status: ProviderManualEligibilityUpdate::Conflict,
                snapshot: Arc::clone(&current),
            });
        }
        let current_manual = current
            .projections
            .iter()
            .find(|projection| projection.provider_endpoint == provider_endpoint)
            .map_or(ProviderManualEligibility::Enabled, |projection| {
                projection.manual
            });
        if current_manual == manual {
            return Ok(ProviderManualEligibilityCommit {
                status: ProviderManualEligibilityUpdate::Unchanged,
                snapshot: Arc::clone(&current),
            });
        }

        let projection = self.with_runtime_store_blocking(|runtime_store| {
            runtime_store.set_provider_manual_eligibility(
                provider_endpoint,
                manual,
                reason,
                updated_at_ms,
            )
        })?;
        self.publish_provider_policy_projection(
            &mut current,
            projection.policy_revision,
            projection,
        );
        Ok(ProviderManualEligibilityCommit {
            status: ProviderManualEligibilityUpdate::Applied,
            snapshot: Arc::clone(&current),
        })
    }

    #[cfg(test)]
    async fn set_provider_automatic_block_for_test(
        &self,
        provider_endpoint: ProviderEndpointKey,
        blocked: bool,
        observed_at_ms: u64,
    ) -> ProviderObservationCommit {
        self.set_provider_automatic_block_for_runtime_identity_for_test(
            RuntimeUpstreamIdentity::new(provider_endpoint, "https://provider.test"),
            blocked,
            observed_at_ms,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn set_provider_automatic_block_for_runtime_identity_for_test(
        &self,
        identity: RuntimeUpstreamIdentity,
        blocked: bool,
        observed_at_ms: u64,
    ) -> ProviderObservationCommit {
        const ACCOUNT_FINGERPRINT: &str =
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        const CONFIG_REVISION: &str =
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let route_scope = identity.policy_route_scope();
        let scope = ProviderObservationScope::new(
            identity.provider_endpoint,
            identity.base_url,
            route_scope,
            "test:provider-policy",
            "https://provider.test/usage",
            ACCOUNT_FINGERPRINT,
            CONFIG_REVISION,
        )
        .expect("test provider observation scope");
        let reservation = self
            .reserve_provider_observation(scope, observed_at_ms)
            .await
            .expect("reserve test provider observation");
        self.commit_provider_observation(
            reservation,
            ProviderObservation {
                observed_at_unix_ms: observed_at_ms,
                completed_at_unix_ms: observed_at_ms,
                authority: ProviderObservationAuthority::Authoritative,
                evidence: serde_json::json!({ "blocked": blocked }),
                effect: if blocked {
                    ProviderPolicyEffect::Block {
                        action_kind: "balance_exhausted".to_string(),
                        code: Some("balance_exhausted".to_string()),
                        reason: "test authoritative balance exhaustion".to_string(),
                        expires_at_unix_ms: None,
                    }
                } else {
                    ProviderPolicyEffect::Recover {
                        reason: "test authoritative balance recovery".to_string(),
                    }
                },
            },
        )
        .await
        .expect("commit test provider observation")
    }

    fn publish_provider_policy_projection(
        &self,
        current: &mut Arc<ProviderPolicySnapshot>,
        policy_revision: u64,
        projection: ProviderEligibilityProjection,
    ) {
        if self.update_provider_policy_projection(current, policy_revision, projection) {
            self.notify_state_changed();
        }
    }

    fn update_provider_policy_projection(
        &self,
        current: &mut Arc<ProviderPolicySnapshot>,
        policy_revision: u64,
        projection: ProviderEligibilityProjection,
    ) -> bool {
        let next = Arc::new(provider_policy_snapshot_with_projection(
            current.as_ref(),
            policy_revision,
            projection,
        ));
        if **current == *next {
            return false;
        }
        *current = next;
        true
    }

    pub async fn active_policy_action_projections(
        &self,
        service_name: &str,
        now_ms: u64,
    ) -> Vec<PolicyActionProjection> {
        let snapshot = self.capture_provider_policy_snapshot().await;
        self.policy_action_projections_for_snapshot(service_name, now_ms, snapshot.as_ref())
    }

    pub fn policy_action_projections_for_snapshot(
        &self,
        service_name: &str,
        now_ms: u64,
        snapshot: &ProviderPolicySnapshot,
    ) -> Vec<PolicyActionProjection> {
        snapshot
            .projections
            .iter()
            .filter(|projection| projection.provider_endpoint.service_name == service_name)
            .filter_map(|projection| policy_action_projection(projection, now_ms))
            .collect()
    }

    pub(crate) async fn half_open_probe_eligible_provider_endpoints(
        &self,
        service_name: &str,
        identities: &[RuntimeUpstreamIdentity],
        capability: RouteCapability,
    ) -> HashSet<ProviderEndpointKey> {
        let mut guard = self.provider_endpoint_runtime_health.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return HashSet::new();
        };
        let now = std::time::Instant::now();
        identities
            .iter()
            .filter_map(|identity| {
                let identity_key =
                    ProviderEndpointRuntimeHealthKey::for_service(service_name, identity)?;
                if per_service.active_revision.is_some()
                    && !per_service.active_identities.contains(&identity_key)
                {
                    return None;
                }
                half_open_probe_bucket_specs(per_service, &identity_key, capability, now, None)
                    .is_some()
                    .then(|| identity.provider_endpoint.clone())
            })
            .collect()
    }

    pub(crate) async fn try_acquire_runtime_half_open_probe(
        &self,
        service_name: &str,
        identity: &RuntimeUpstreamIdentity,
        capability: RouteCapability,
    ) -> Option<RuntimeHealthHalfOpenProbeLease> {
        let identity_key = ProviderEndpointRuntimeHealthKey::for_service(service_name, identity)?;
        let owner = Arc::new(());
        let mut guard = self.provider_endpoint_runtime_health.write().await;
        let per_service = guard.get_mut(service_name)?;
        if per_service.active_revision.is_some()
            && !per_service.active_identities.contains(&identity_key)
        {
            return None;
        }
        let specs = half_open_probe_bucket_specs(
            per_service,
            &identity_key,
            capability,
            std::time::Instant::now(),
            None,
        )?;
        for spec in &specs {
            let health = per_service.health.get_mut(&spec.key)?;
            health.half_open_probe_owner = Arc::downgrade(&owner);
        }
        let buckets = specs
            .into_iter()
            .map(|spec| RuntimeHealthHalfOpenProbeBucketLease {
                key: spec.key,
                breaker_epoch: spec.breaker_epoch,
            })
            .collect();
        Some(RuntimeHealthHalfOpenProbeLease {
            identity: identity_key,
            capability,
            owner,
            buckets,
        })
    }

    pub(crate) async fn validate_runtime_half_open_probe(
        &self,
        probe: &RuntimeHealthHalfOpenProbeLease,
    ) -> bool {
        let service_name = probe.identity.provider_endpoint.service_name.as_str();
        let mut guard = self.provider_endpoint_runtime_health.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return false;
        };
        if per_service.active_revision.is_some()
            && !per_service.active_identities.contains(&probe.identity)
        {
            return false;
        }
        let Some(specs) = half_open_probe_bucket_specs(
            per_service,
            &probe.identity,
            probe.capability,
            std::time::Instant::now(),
            Some(&probe.owner),
        ) else {
            return false;
        };
        specs.len() == probe.buckets.len()
            && probe.buckets.iter().all(|bucket| {
                specs.iter().any(|spec| {
                    spec.key == bucket.key && spec.breaker_epoch == bucket.breaker_epoch
                })
            })
    }

    pub(crate) async fn dispatch_runtime_half_open_probe(
        &self,
        probe: RuntimeHealthHalfOpenProbeLease,
    ) -> Result<DispatchedRuntimeHealthHalfOpenProbe, RuntimeHealthHalfOpenProbeInvalidated> {
        let service_name = probe.identity.provider_endpoint.service_name.as_str();
        let mut guard = self.provider_endpoint_runtime_health.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return Err(RuntimeHealthHalfOpenProbeInvalidated);
        };
        if per_service.active_revision.is_some()
            && !per_service.active_identities.contains(&probe.identity)
        {
            return Err(RuntimeHealthHalfOpenProbeInvalidated);
        }
        let Some(specs) = half_open_probe_bucket_specs(
            per_service,
            &probe.identity,
            probe.capability,
            std::time::Instant::now(),
            Some(&probe.owner),
        ) else {
            return Err(RuntimeHealthHalfOpenProbeInvalidated);
        };
        if specs.len() != probe.buckets.len()
            || probe.buckets.iter().any(|bucket| {
                !specs.iter().any(|spec| {
                    spec.key == bucket.key && spec.breaker_epoch == bucket.breaker_epoch
                })
            })
        {
            return Err(RuntimeHealthHalfOpenProbeInvalidated);
        }

        for bucket in &probe.buckets {
            let Some(health) = per_service.health.get_mut(&bucket.key) else {
                return Err(RuntimeHealthHalfOpenProbeInvalidated);
            };
            health.half_open_probe_attempted_epoch = Some(bucket.breaker_epoch);
            health.half_open_probe_owner = Weak::new();
        }
        Ok(DispatchedRuntimeHealthHalfOpenProbe {
            identity: probe.identity,
            capability: probe.capability,
            buckets: probe.buckets,
        })
    }

    pub(crate) async fn validate_dispatched_runtime_half_open_probe(
        &self,
        probe: &DispatchedRuntimeHealthHalfOpenProbe,
    ) -> bool {
        let service_name = probe.identity.provider_endpoint.service_name.as_str();
        let guard = self.provider_endpoint_runtime_health.read().await;
        let Some(per_service) = guard.get(service_name) else {
            return false;
        };
        if per_service.active_revision.is_some()
            && !per_service.active_identities.contains(&probe.identity)
        {
            return false;
        }
        !probe.buckets.is_empty()
            && probe.buckets.iter().all(|bucket| {
                bucket.key.identity == probe.identity
                    && per_service.health.get(&bucket.key).is_some_and(|health| {
                        health.breaker_epoch == bucket.breaker_epoch
                            && health.half_open_probe_attempted_epoch == Some(bucket.breaker_epoch)
                    })
            })
    }

    pub(crate) async fn settle_runtime_half_open_probe(
        &self,
        probe: DispatchedRuntimeHealthHalfOpenProbe,
        terminal: RuntimeHealthHalfOpenTerminal,
    ) -> RuntimeHealthHalfOpenSettlement {
        let service_name = probe.identity.provider_endpoint.service_name.as_str();
        let mut guard = self.provider_endpoint_runtime_health.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return RuntimeHealthHalfOpenSettlement::Stale;
        };
        if per_service.active_revision.is_some()
            && !per_service.active_identities.contains(&probe.identity)
        {
            return RuntimeHealthHalfOpenSettlement::Stale;
        }
        let current = !probe.buckets.is_empty()
            && probe.buckets.iter().all(|bucket| {
                bucket.key.identity == probe.identity
                    && per_service.health.get(&bucket.key).is_some_and(|health| {
                        health.breaker_epoch == bucket.breaker_epoch
                            && health.half_open_probe_attempted_epoch == Some(bucket.breaker_epoch)
                    })
            });
        if !current {
            return RuntimeHealthHalfOpenSettlement::Stale;
        }

        match terminal {
            RuntimeHealthHalfOpenTerminal::Success { now_ms } => {
                for bucket in &probe.buckets {
                    let Some(health) = per_service.health.get_mut(&bucket.key) else {
                        return RuntimeHealthHalfOpenSettlement::Stale;
                    };
                    record_runtime_health_success(
                        health,
                        bucket.key.domain,
                        probe.capability,
                        now_ms,
                    );
                }
            }
            RuntimeHealthHalfOpenTerminal::CountedFailure {
                domain,
                failure_threshold_cooldown_secs,
                cooldown_backoff,
            } => {
                let health = per_service
                    .health
                    .entry(ProviderEndpointRuntimeHealthBucketKey::new(
                        probe.identity,
                        domain,
                    ))
                    .or_default();
                record_runtime_health_failure(
                    health,
                    failure_threshold_cooldown_secs,
                    cooldown_backoff,
                    std::time::Instant::now(),
                );
            }
            RuntimeHealthHalfOpenTerminal::Penalty {
                domain,
                cooldown_secs,
                cooldown_backoff,
            } => {
                let health = per_service
                    .health
                    .entry(ProviderEndpointRuntimeHealthBucketKey::new(
                        probe.identity,
                        domain,
                    ))
                    .or_default();
                penalize_runtime_health(
                    health,
                    cooldown_secs,
                    cooldown_backoff,
                    std::time::Instant::now(),
                );
            }
            RuntimeHealthHalfOpenTerminal::Neutral => {}
        }
        RuntimeHealthHalfOpenSettlement::Applied
    }

    pub async fn record_runtime_upstream_attempt_success(
        &self,
        service_name: &str,
        identity: &RuntimeUpstreamIdentity,
        now_ms: u64,
    ) {
        self.record_runtime_upstream_attempt_success_for_capability(
            service_name,
            identity,
            RouteCapability::Inference,
            now_ms,
        )
        .await;
    }

    pub(crate) async fn record_runtime_upstream_attempt_success_for_capability(
        &self,
        service_name: &str,
        identity: &RuntimeUpstreamIdentity,
        capability: RouteCapability,
        now_ms: u64,
    ) {
        let mut guard = self.provider_endpoint_runtime_health.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        for domain in [
            RuntimeHealthDomain::EndpointTransport,
            RuntimeHealthDomain::Credential,
            RuntimeHealthDomain::Capability(capability),
            RuntimeHealthDomain::Capacity(capability),
        ] {
            let Some(entry) = active_provider_endpoint_runtime_health(
                service_name,
                identity,
                domain,
                per_service,
            ) else {
                return;
            };
            record_runtime_health_success(entry, domain, capability, now_ms);
        }
    }

    pub async fn record_runtime_upstream_attempt_failure(
        &self,
        service_name: &str,
        identity: &RuntimeUpstreamIdentity,
        failure_threshold_cooldown_secs: u64,
        cooldown_backoff: CooldownBackoff,
    ) {
        self.record_runtime_upstream_attempt_failure_for_domain(
            service_name,
            identity,
            RuntimeHealthDomain::Capability(RouteCapability::Inference),
            failure_threshold_cooldown_secs,
            cooldown_backoff,
        )
        .await;
    }

    pub(crate) async fn record_runtime_upstream_attempt_failure_for_domain(
        &self,
        service_name: &str,
        identity: &RuntimeUpstreamIdentity,
        domain: RuntimeHealthDomain,
        failure_threshold_cooldown_secs: u64,
        cooldown_backoff: CooldownBackoff,
    ) {
        let mut guard = self.provider_endpoint_runtime_health.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        let Some(entry) =
            active_provider_endpoint_runtime_health(service_name, identity, domain, per_service)
        else {
            return;
        };

        record_runtime_health_failure(
            entry,
            failure_threshold_cooldown_secs,
            cooldown_backoff,
            std::time::Instant::now(),
        );
    }

    pub async fn penalize_runtime_upstream_attempt(
        &self,
        service_name: &str,
        identity: &RuntimeUpstreamIdentity,
        cooldown_secs: u64,
        cooldown_backoff: CooldownBackoff,
    ) {
        self.penalize_runtime_upstream_attempt_for_domain(
            service_name,
            identity,
            RuntimeHealthDomain::Capability(RouteCapability::Inference),
            cooldown_secs,
            cooldown_backoff,
        )
        .await;
    }

    pub(crate) async fn penalize_runtime_upstream_attempt_for_domain(
        &self,
        service_name: &str,
        identity: &RuntimeUpstreamIdentity,
        domain: RuntimeHealthDomain,
        cooldown_secs: u64,
        cooldown_backoff: CooldownBackoff,
    ) {
        let mut guard = self.provider_endpoint_runtime_health.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        let Some(entry) =
            active_provider_endpoint_runtime_health(service_name, identity, domain, per_service)
        else {
            return;
        };
        penalize_runtime_health(
            entry,
            cooldown_secs,
            cooldown_backoff,
            std::time::Instant::now(),
        );
    }

    #[cfg(test)]
    pub async fn record_provider_endpoint_attempt_success(
        &self,
        service_name: &str,
        endpoint: ProviderEndpointKey,
        now_ms: u64,
    ) {
        self.record_runtime_upstream_attempt_success(
            service_name,
            &RuntimeUpstreamIdentity::new(endpoint, "https://provider.test"),
            now_ms,
        )
        .await;
    }

    #[cfg(test)]
    pub async fn record_provider_endpoint_attempt_failure(
        &self,
        service_name: &str,
        endpoint: ProviderEndpointKey,
        failure_threshold_cooldown_secs: u64,
        cooldown_backoff: CooldownBackoff,
    ) {
        self.record_runtime_upstream_attempt_failure(
            service_name,
            &RuntimeUpstreamIdentity::new(endpoint, "https://provider.test"),
            failure_threshold_cooldown_secs,
            cooldown_backoff,
        )
        .await;
    }

    pub async fn prune_runtime_observability_for_service(
        &self,
        service_name: &str,
        view: &ServiceRouteConfig,
    ) {
        let Ok(route) = crate::routing_ir::compile_route_handshake_plan(service_name, view) else {
            return;
        };
        let mut changed = false;
        let active_provider_endpoint_keys = route
            .candidates
            .iter()
            .map(|candidate| {
                ProviderEndpointKey::new(
                    service_name,
                    &candidate.provider_id,
                    &candidate.endpoint_id,
                )
            })
            .collect::<HashSet<_>>();
        let active_provider_endpoint_stable_keys = std::iter::once("-".to_string())
            .chain(
                active_provider_endpoint_keys
                    .iter()
                    .map(ProviderEndpointKey::stable_key),
            )
            .collect::<HashSet<_>>();
        let mut active_provider_ids = HashSet::from(["-".to_string()]);
        for candidate in &route.candidates {
            active_provider_ids.insert(candidate.provider_id.clone());
        }

        {
            let mut provider_balances = self.provider_balances.write().await;
            let before = provider_balances.len();
            provider_balances.retain(|provider_endpoint, _| {
                provider_endpoint.service_name != service_name
                    || active_provider_endpoint_keys.contains(provider_endpoint)
            });
            changed |= provider_balances.len() != before;
        }

        {
            let mut guard = self.provider_endpoint_runtime_health.write().await;
            if let Some(per_service) = guard.get_mut(service_name) {
                per_service.active_identities.retain(|identity| {
                    active_provider_endpoint_keys.contains(&identity.provider_endpoint)
                });
                per_service.health.retain(|bucket, _| {
                    active_provider_endpoint_keys.contains(&bucket.identity.provider_endpoint)
                });
                if !per_service.identities_authoritative
                    && per_service.active_identities.is_empty()
                    && per_service.health.is_empty()
                {
                    guard.remove(service_name);
                }
            }
        }

        {
            let mut request_state = self.request_lifecycle_projection.write().await;
            if let Some(rollup) = request_state.usage_rollups.get_mut(service_name) {
                let before_by_provider_endpoint = rollup.by_provider_endpoint.len();
                rollup.by_provider_endpoint.retain(|endpoint_key, _| {
                    active_provider_endpoint_stable_keys.contains(endpoint_key)
                });
                changed |= rollup.by_provider_endpoint.len() != before_by_provider_endpoint;
                let before_by_provider_endpoint_day = rollup.by_provider_endpoint_day.len();
                rollup
                    .by_provider_endpoint_day
                    .retain(|endpoint_key, _day_map| {
                        active_provider_endpoint_stable_keys.contains(endpoint_key)
                    });
                changed |= rollup.by_provider_endpoint_day.len() != before_by_provider_endpoint_day;
                let before_by_provider = rollup.by_provider.len();
                rollup
                    .by_provider
                    .retain(|provider_id, _| active_provider_ids.contains(provider_id));
                changed |= rollup.by_provider.len() != before_by_provider;
                let before_by_provider_day = rollup.by_provider_day.len();
                rollup.by_provider_day.retain(|provider_id, _day_map| {
                    if !active_provider_ids.contains(provider_id) {
                        return false;
                    }
                    true
                });
                changed |= rollup.by_provider_day.len() != before_by_provider_day;
            }
        }
        if changed {
            self.notify_state_changed();
        }
    }

    async fn try_record_quota_snapshot(
        &self,
        endpoint: ProviderEndpointKey,
        mut context: QuotaObservationContext,
        snapshot: &ProviderBalanceSnapshot,
    ) -> Result<RegistryUpdate, RuntimeStoreError> {
        if context.install_key.is_none() {
            context.install_key = Some(self.quota_identity.key().to_vec());
        }
        let mut registry = self
            .quota_pool_registry
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut candidate = registry.clone();
        let update = candidate.record_snapshot(&endpoint, &context, snapshot);
        let checkpoint = candidate.checkpoint();
        let expected_revision = self.quota_registry_document_revision();
        let committed_revision = self.with_runtime_store_blocking(|runtime_store| {
            persist_quota_registry_checkpoint(runtime_store, &checkpoint, expected_revision)
        })?;
        self.publish_quota_registry_document_revision(committed_revision);
        *registry = candidate;
        drop(registry);
        self.notify_state_changed();
        Ok(update)
    }

    pub async fn record_quota_snapshot(
        &self,
        endpoint: ProviderEndpointKey,
        context: QuotaObservationContext,
        snapshot: &ProviderBalanceSnapshot,
    ) -> RegistryUpdate {
        match self
            .try_record_quota_snapshot(endpoint, context, snapshot)
            .await
        {
            Ok(update) => update,
            Err(error) => {
                tracing::warn!(error = %error, "quota registry commit failed");
                RegistryUpdate::default()
            }
        }
    }

    pub async fn quota_pool_membership(
        &self,
        endpoint: &ProviderEndpointKey,
    ) -> Option<crate::quota_pool::PoolMembership> {
        self.quota_pool_registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .membership_for_endpoint(endpoint)
    }

    pub async fn quota_pool_states(&self) -> Vec<QuotaPoolState> {
        self.quota_pool_registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pools()
    }

    pub async fn quota_samples_for_pool(
        &self,
        pool_key: &str,
    ) -> Vec<crate::quota_pool::QuotaObservation> {
        self.quota_pool_registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .samples_for_pool(pool_key)
    }

    pub async fn quota_registry_checkpoint(&self) -> QuotaRegistryCheckpoint {
        self.quota_pool_registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .checkpoint()
    }

    pub async fn quota_analytics_view(
        &self,
        service_name: &str,
        now_ms: u64,
    ) -> QuotaAnalyticsView {
        let checkpoint = self.quota_registry_checkpoint().await;
        let mut windows = plan_quota_attribution(&checkpoint, now_ms);
        for window in &mut windows {
            window.query.service_name = Some(service_name.to_string());
        }

        let request_state = self.request_lifecycle_projection.read().await;
        let attribution = windows
            .into_iter()
            .map(|window| PoolAttributionResult {
                attribution: request_state.attribution_index.query(window.query.clone()),
                window,
            })
            .collect::<Vec<_>>();
        drop(request_state);

        build_quota_analytics(&checkpoint, now_ms, &attribution)
    }

    pub async fn query_attribution(&self, query: AttributionQuery) -> AttributionQueryResult {
        self.request_lifecycle_projection
            .read()
            .await
            .attribution_index
            .query(query)
    }

    fn quota_context_for_balance_snapshot(
        snapshot: &ProviderBalanceSnapshot,
    ) -> QuotaObservationContext {
        let mut context = QuotaObservationContext::new(format!(
            "{}:{}",
            snapshot.source, snapshot.observation_provider_id
        ));
        context.counter_kind = QuotaCounterKind::Used;
        context.unit = if snapshot.quota_used_usd.is_some()
            || snapshot.quota_remaining_usd.is_some()
            || snapshot.quota_limit_usd.is_some()
            || snapshot.today_used_usd.is_some()
        {
            QuotaUnit::Usd
        } else {
            QuotaUnit::Raw
        };
        context.capabilities = QuotaCapabilities {
            used: snapshot.quota_used_usd.is_some(),
            remaining: snapshot.quota_remaining_usd.is_some(),
            limit: snapshot.quota_limit_usd.is_some(),
            direct_total: snapshot.today_used_usd.is_some(),
            ..QuotaCapabilities::default()
        };
        context
    }

    fn provider_balance_snapshot_is_not_newer(
        balances: &ProviderBalanceMap,
        snapshot: &ProviderBalanceSnapshot,
    ) -> bool {
        balances
            .get(&snapshot.provider_endpoint)
            .and_then(|endpoint_balances| endpoint_balances.get(&snapshot.observation_provider_id))
            .is_some_and(|previous| snapshot.fetched_at_ms <= previous.snapshot.fetched_at_ms)
    }

    fn publish_provider_balance_snapshot_locked(
        balances: &mut ProviderBalanceMap,
        mut snapshot: ProviderBalanceSnapshot,
        route_scope: Option<&str>,
        now_ms: u64,
    ) {
        let provider_endpoint = snapshot.provider_endpoint.clone();
        let observation_provider_id = snapshot.observation_provider_id.clone();
        snapshot.refresh_status(now_ms);
        let endpoint_balances = balances.entry(provider_endpoint).or_default();
        if !snapshot.has_amount_data()
            && let Some(previous) = endpoint_balances.get(&observation_provider_id)
            && previous.route_scope.as_deref() == route_scope
        {
            snapshot.carry_forward_amount_data_from(&previous.snapshot);
            snapshot.refresh_status(now_ms);
        }
        endpoint_balances.insert(
            observation_provider_id,
            ProviderBalanceRecord {
                snapshot,
                route_scope: route_scope.map(str::to_string),
            },
        );
    }

    pub(crate) async fn try_record_provider_balance_snapshot(
        &self,
        snapshot: ProviderBalanceSnapshot,
    ) -> Result<ProviderBalanceSnapshotPublication, RuntimeStoreError> {
        let context = Self::quota_context_for_balance_snapshot(&snapshot);
        self.try_record_provider_balance_snapshot_with_quota_context(snapshot, context)
            .await
    }

    pub async fn record_provider_balance_snapshot(&self, snapshot: ProviderBalanceSnapshot) {
        if let Err(error) = self.try_record_provider_balance_snapshot(snapshot).await {
            tracing::warn!(error = %error, "provider balance snapshot commit failed");
        }
    }

    pub(crate) async fn try_record_provider_balance_snapshot_with_quota_context(
        &self,
        snapshot: ProviderBalanceSnapshot,
        context: QuotaObservationContext,
    ) -> Result<ProviderBalanceSnapshotPublication, RuntimeStoreError> {
        self.try_record_provider_balance_snapshot_with_quota_context_and_route_scope(
            snapshot, context, None,
        )
        .await
    }

    async fn try_record_provider_balance_snapshot_with_quota_context_and_route_scope(
        &self,
        mut snapshot: ProviderBalanceSnapshot,
        mut context: QuotaObservationContext,
        route_scope: Option<&str>,
    ) -> Result<ProviderBalanceSnapshotPublication, RuntimeStoreError> {
        if snapshot.observation_provider_id.trim().is_empty()
            || snapshot.provider_endpoint.service_name.trim().is_empty()
            || snapshot.provider_endpoint.provider_id.trim().is_empty()
            || snapshot.provider_endpoint.endpoint_id.trim().is_empty()
        {
            return Ok(ProviderBalanceSnapshotPublication::IgnoredInvalidIdentity);
        }
        let provider_endpoint = snapshot.provider_endpoint.clone();
        let now_ms = unix_now_ms();
        snapshot.refresh_status(now_ms);
        let mut balances = self.provider_balances.write().await;
        if Self::provider_balance_snapshot_is_not_newer(&balances, &snapshot) {
            return Ok(ProviderBalanceSnapshotPublication::IgnoredOlder);
        }
        if context.install_key.is_none() {
            context.install_key = Some(self.quota_identity.key().to_vec());
        }
        let mut registry = self
            .quota_pool_registry
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut candidate = registry.clone();
        let registry_update = candidate.record_snapshot(&provider_endpoint, &context, &snapshot);
        if let Some(pool) = registry_update.pool {
            snapshot.quota_pool_key = Some(pool.key);
            snapshot.quota_pool_revision = Some(pool.revision);
        }
        let checkpoint = candidate.checkpoint();
        let expected_revision = self.quota_registry_document_revision();
        let committed_revision = self.with_runtime_store_blocking(|runtime_store| {
            persist_quota_registry_checkpoint(runtime_store, &checkpoint, expected_revision)
        })?;

        self.publish_quota_registry_document_revision(committed_revision);
        *registry = candidate;
        Self::publish_provider_balance_snapshot_locked(
            &mut balances,
            snapshot,
            route_scope,
            now_ms,
        );
        drop(registry);
        drop(balances);
        self.notify_state_changed();
        Ok(ProviderBalanceSnapshotPublication::Published)
    }

    pub(crate) async fn try_record_provider_balance_snapshot_for_runtime_identity(
        &self,
        snapshot: ProviderBalanceSnapshot,
        identity: &RuntimeUpstreamIdentity,
    ) -> Result<ProviderBalanceSnapshotPublication, RuntimeStoreError> {
        let context = Self::quota_context_for_balance_snapshot(&snapshot);
        self.try_record_provider_balance_snapshot_with_quota_context_for_runtime_identity(
            snapshot, context, identity,
        )
        .await
    }

    pub(crate) async fn try_record_provider_balance_snapshot_with_quota_context_for_runtime_identity(
        &self,
        snapshot: ProviderBalanceSnapshot,
        context: QuotaObservationContext,
        identity: &RuntimeUpstreamIdentity,
    ) -> Result<ProviderBalanceSnapshotPublication, RuntimeStoreError> {
        let _update_guard = self.provider_policy_updates.lock().await;
        let active = self.with_runtime_store_blocking(|runtime_store| {
            runtime_store.runtime_upstream_identity_is_active(identity)
        })?;
        if !active {
            return Ok(ProviderBalanceSnapshotPublication::IgnoredInactiveRuntimeIdentity);
        }
        let route_scope = identity.policy_route_scope();
        self.try_record_provider_balance_snapshot_with_quota_context_and_route_scope(
            snapshot,
            context,
            Some(route_scope.as_str()),
        )
        .await
    }

    pub(crate) async fn commit_provider_observation_and_balance_snapshot(
        &self,
        reservation: ProviderObservationReservation,
        observation: ProviderObservation,
        snapshot: ProviderBalanceSnapshot,
    ) -> Result<ProviderObservationCommit, RuntimeStoreError> {
        let context = Self::quota_context_for_balance_snapshot(&snapshot);
        self.commit_provider_observation_and_balance_snapshot_with_quota_context(
            reservation,
            observation,
            snapshot,
            context,
        )
        .await
    }

    pub(crate) async fn commit_provider_observation_and_balance_snapshot_with_quota_context(
        &self,
        reservation: ProviderObservationReservation,
        observation: ProviderObservation,
        mut snapshot: ProviderBalanceSnapshot,
        mut context: QuotaObservationContext,
    ) -> Result<ProviderObservationCommit, RuntimeStoreError> {
        if snapshot.observation_provider_id.trim().is_empty()
            || snapshot.provider_endpoint.service_name.trim().is_empty()
            || snapshot.provider_endpoint.provider_id.trim().is_empty()
            || snapshot.provider_endpoint.endpoint_id.trim().is_empty()
        {
            return Err(quota_runtime_error(
                "provider balance snapshot has an incomplete endpoint identity",
            ));
        }
        if reservation.ticket.provider_endpoint() != &snapshot.provider_endpoint {
            return Err(quota_runtime_error(format!(
                "provider observation endpoint {} does not match quota endpoint {}",
                reservation.ticket.provider_endpoint().stable_key(),
                snapshot.provider_endpoint.stable_key()
            )));
        }
        let route_scope = reservation.route_scope().to_string();
        if context.install_key.is_none() {
            context.install_key = Some(self.quota_identity.key().to_vec());
        }
        snapshot.refresh_status(unix_now_ms());

        let committed = {
            let _update_guard = self.provider_policy_updates.lock().await;
            let mut current_policy = self.provider_policy_snapshot.write().await;
            #[cfg(test)]
            self.signal_provider_balance_lock_wait_for_test().await;
            let mut balances = self.provider_balances.write().await;
            let mut registry = self
                .quota_pool_registry
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut candidate = registry.clone();
            let registry_update =
                candidate.record_snapshot(&snapshot.provider_endpoint, &context, &snapshot);
            if let Some(pool) = registry_update.pool {
                snapshot.quota_pool_key = Some(pool.key);
                snapshot.quota_pool_revision = Some(pool.revision);
            }
            let checkpoint = candidate.checkpoint();
            let expected_revision = self.quota_registry_document_revision();
            let payload_json = serialize_quota_registry_checkpoint(&checkpoint)?;
            let (committed, quota_document) =
                self.with_runtime_store_blocking(|runtime_store| {
                    runtime_store.commit_provider_observation_and_quota_registry(
                        reservation.ticket,
                        observation,
                        expected_revision,
                        RuntimeDocumentWrite {
                            kind: RuntimeDocumentKind::QuotaRegistry,
                            schema_version: QUOTA_CHECKPOINT_SCHEMA_VERSION,
                            payload_json: &payload_json,
                        },
                    )
                })?;

            if committed.disposition == ProviderObservationDisposition::Accepted {
                let quota_document = quota_document.ok_or_else(|| {
                    quota_runtime_error(
                        "accepted provider observation did not commit quota registry",
                    )
                })?;
                self.publish_quota_registry_document_revision(quota_document.revision);
                *registry = candidate;
                self.update_provider_policy_projection(
                    &mut current_policy,
                    committed.policy_revision,
                    committed.projection.clone(),
                );
                Self::publish_provider_balance_snapshot_locked(
                    &mut balances,
                    snapshot,
                    Some(route_scope.as_str()),
                    unix_now_ms(),
                );
                drop(registry);
                drop(balances);
                drop(current_policy);
                self.notify_state_changed();
            } else {
                debug_assert!(quota_document.is_none());
            }
            committed
        };
        Ok(committed)
    }

    pub async fn get_provider_balance_view(
        &self,
        service_name: &str,
    ) -> Vec<ProviderBalanceSnapshot> {
        let now_ms = unix_now_ms();
        let guard = self.provider_balances.read().await;
        let mut snapshots = guard
            .iter()
            .filter(|(provider_endpoint, _)| provider_endpoint.service_name == service_name)
            .flat_map(|(_, providers)| providers.values().map(|record| record.snapshot.clone()))
            .collect::<Vec<_>>();
        for snapshot in &mut snapshots {
            snapshot.refresh_status(now_ms);
        }
        snapshots.sort_by(|left, right| {
            left.provider_endpoint
                .cmp(&right.provider_endpoint)
                .then_with(|| {
                    left.observation_provider_id
                        .cmp(&right.observation_provider_id)
                })
        });
        snapshots
    }

    pub(crate) async fn get_provider_balance_view_for_runtime_identity(
        &self,
        identity: &RuntimeUpstreamIdentity,
    ) -> Vec<ProviderBalanceSnapshot> {
        let now_ms = unix_now_ms();
        let route_scope = identity.policy_route_scope();
        let guard = self.provider_balances.read().await;
        let mut snapshots = guard
            .get(&identity.provider_endpoint)
            .into_iter()
            .flat_map(|providers| providers.values())
            .filter(|record| record.route_scope.as_deref() == Some(route_scope.as_str()))
            .map(|record| record.snapshot.clone())
            .collect::<Vec<_>>();
        for snapshot in &mut snapshots {
            snapshot.refresh_status(now_ms);
        }
        snapshots.sort_by(|left, right| {
            left.observation_provider_id
                .cmp(&right.observation_provider_id)
        });
        snapshots
    }

    pub async fn get_usage_rollup_view(
        &self,
        service_name: &str,
        top_n: usize,
        days: usize,
    ) -> UsageRollupView {
        let request_state = self.request_lifecycle_projection.read().await;
        Self::usage_rollup_view_from(request_state.usage_rollups.get(service_name), top_n, days)
    }

    fn usage_rollup_view_from(
        rollup: Option<&UsageRollup>,
        top_n: usize,
        days: usize,
    ) -> UsageRollupView {
        let Some(rollup) = rollup else {
            return UsageRollupView::default();
        };

        fn sorted_day_series(map: &HashMap<i32, UsageBucket>) -> Vec<(i32, UsageBucket)> {
            let mut out = map.iter().map(|(k, v)| (*k, v.clone())).collect::<Vec<_>>();
            out.sort_by_key(|(k, _)| *k);
            out
        }

        fn filled_day_series(
            map: &HashMap<i32, UsageBucket>,
            start_day: i32,
            end_day: i32,
        ) -> Vec<(i32, UsageBucket)> {
            if start_day > end_day {
                return Vec::new();
            }
            (start_day..=end_day)
                .map(|day| (day, map.get(&day).cloned().unwrap_or_default()))
                .collect()
        }

        fn sum_series(series: &[(i32, UsageBucket)]) -> UsageBucket {
            let mut out = UsageBucket::default();
            for (_, bucket) in series {
                out.add_assign(bucket);
            }
            out
        }

        fn aggregate_entity_window(
            source: &HashMap<String, HashMap<i32, UsageBucket>>,
            start_day: Option<i32>,
            end_day: Option<i32>,
            top_n: usize,
        ) -> Vec<(String, UsageBucket)> {
            let mut out = Vec::new();
            for (name, days) in source {
                let mut bucket = UsageBucket::default();
                for (day, value) in days {
                    let include = match (start_day, end_day) {
                        (Some(start), Some(end)) => *day >= start && *day <= end,
                        _ => true,
                    };
                    if include {
                        bucket.add_assign(value);
                    }
                }
                if bucket.requests_total > 0 {
                    out.push((name.clone(), bucket));
                }
            }
            out.sort_by(|(left_name, left), (right_name, right)| {
                right
                    .usage
                    .total_tokens
                    .cmp(&left.usage.total_tokens)
                    .then_with(|| right.requests_total.cmp(&left.requests_total))
                    .then_with(|| left_name.cmp(right_name))
            });
            out.truncate(top_n);
            out
        }

        let all_loaded = days == 0;
        let loaded_first_day = rollup.by_day.keys().min().copied();
        let loaded_last_day = rollup.by_day.keys().max().copied();
        let loaded_days_with_data = rollup
            .by_day
            .values()
            .filter(|bucket| bucket.requests_total > 0)
            .count();

        let (start_day, end_day) = if all_loaded {
            (loaded_first_day, loaded_last_day)
        } else {
            let end = usage_day::current_local_day();
            let offset = i32::try_from(days.saturating_sub(1)).unwrap_or(i32::MAX);
            (Some(end.saturating_sub(offset)), Some(end))
        };

        let by_day = match (all_loaded, start_day, end_day) {
            (true, _, _) => sorted_day_series(&rollup.by_day),
            (false, Some(start), Some(end)) => filled_day_series(&rollup.by_day, start, end),
            _ => Vec::new(),
        };
        let window = if all_loaded {
            rollup.loaded.clone()
        } else {
            sum_series(&by_day)
        };

        let mut by_provider_endpoint =
            aggregate_entity_window(&rollup.by_provider_endpoint_day, start_day, end_day, top_n);
        if all_loaded && by_provider_endpoint.is_empty() {
            by_provider_endpoint = rollup
                .by_provider_endpoint
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>();
            by_provider_endpoint.sort_by(|(left_name, left), (right_name, right)| {
                right
                    .usage
                    .total_tokens
                    .cmp(&left.usage.total_tokens)
                    .then_with(|| right.requests_total.cmp(&left.requests_total))
                    .then_with(|| left_name.cmp(right_name))
            });
            by_provider_endpoint.truncate(top_n);
        }

        let mut by_provider =
            aggregate_entity_window(&rollup.by_provider_day, start_day, end_day, top_n);
        if all_loaded && by_provider.is_empty() {
            by_provider = rollup
                .by_provider
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>();
            by_provider.sort_by(|(left_name, left), (right_name, right)| {
                right
                    .usage
                    .total_tokens
                    .cmp(&left.usage.total_tokens)
                    .then_with(|| right.requests_total.cmp(&left.requests_total))
                    .then_with(|| left_name.cmp(right_name))
            });
            by_provider.truncate(top_n);
        }

        let mut by_provider_endpoint_day = HashMap::new();
        for (name, _) in &by_provider_endpoint {
            let series = rollup
                .by_provider_endpoint_day
                .get(name)
                .map(|m| match (all_loaded, start_day, end_day) {
                    (true, _, _) => sorted_day_series(m),
                    (false, Some(start), Some(end)) => filled_day_series(m, start, end),
                    _ => Vec::new(),
                })
                .unwrap_or_default();
            by_provider_endpoint_day.insert(name.clone(), series);
        }

        let mut by_provider_day = HashMap::new();
        for (name, _) in &by_provider {
            let series = rollup
                .by_provider_day
                .get(name)
                .map(|m| match (all_loaded, start_day, end_day) {
                    (true, _, _) => sorted_day_series(m),
                    (false, Some(start), Some(end)) => filled_day_series(m, start, end),
                    _ => Vec::new(),
                })
                .unwrap_or_default();
            by_provider_day.insert(name.clone(), series);
        }

        let window_days_with_data = by_day
            .iter()
            .filter(|(_, bucket)| bucket.requests_total > 0)
            .count();
        let coverage = UsageRollupCoverage {
            requested_days: days,
            all_loaded,
            loaded_first_day,
            loaded_last_day,
            loaded_days_with_data,
            loaded_requests: rollup.loaded.requests_total,
            window_first_day: start_day,
            window_last_day: end_day,
            window_days_with_data,
            window_requests: window.requests_total,
            window_exceeds_loaded_start: matches!(
                (all_loaded, start_day, loaded_first_day),
                (false, Some(start), Some(first)) if start < first
            ),
        };

        UsageRollupView {
            loaded: rollup.loaded.clone(),
            window,
            coverage,
            by_day,
            by_provider_endpoint,
            by_provider_endpoint_day,
            by_provider,
            by_provider_day,
        }
    }

    pub async fn get_usage_day_view(
        &self,
        service_name: &str,
        top_n: usize,
        generated_at_ms: u64,
    ) -> UsageDayView {
        let request_state = self.request_lifecycle_projection.read().await;
        Self::usage_day_view_from(
            request_state.usage_rollups.get(service_name),
            top_n,
            generated_at_ms,
        )
    }

    fn usage_day_view_from(
        rollup: Option<&UsageRollup>,
        top_n: usize,
        generated_at_ms: u64,
    ) -> UsageDayView {
        let day = usage_day::current_local_day();
        let window = usage_day::local_day_window(day).unwrap_or(usage_day::UsageDayWindow {
            day,
            start_ms: 0,
            end_ms: 0,
        });

        let Some(rollup) = rollup else {
            return UsageDayView {
                day,
                label: usage_day::format_day(day),
                start_ms: window.start_ms,
                end_ms: window.end_ms,
                generated_at_ms,
                hourly: (0..24)
                    .map(|hour| UsageDayHourRow {
                        hour,
                        bucket: UsageBucket::default(),
                    })
                    .collect(),
                coverage: UsageDayCoverage {
                    source: "none".to_string(),
                    ..UsageDayCoverage::default()
                },
                ..UsageDayView::default()
            };
        };

        UsageDayView {
            day,
            label: usage_day::format_day(day),
            start_ms: window.start_ms,
            end_ms: window.end_ms,
            generated_at_ms,
            summary: rollup.by_day.get(&day).cloned().unwrap_or_default(),
            hourly: usage_day_hour_rows(rollup, day),
            provider_rows: usage_day_dimension_rows(&rollup.by_provider_day, day, top_n),
            provider_endpoint_rows: usage_day_dimension_rows(
                &rollup.by_provider_endpoint_day,
                day,
                top_n,
            ),
            model_rows: usage_day_dimension_rows(&rollup.by_model_day, day, top_n),
            session_rows: usage_day_dimension_rows(&rollup.by_session_day, day, top_n),
            project_rows: usage_day_dimension_rows(&rollup.by_project_day, day, top_n),
            coverage: usage_day_coverage(rollup, window),
            ..UsageDayView::default()
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn try_begin_request(
        &self,
        service: &str,
        method: &str,
        path: &str,
        session_id: Option<String>,
        session_identity_source: Option<SessionIdentitySource>,
        client_name: Option<String>,
        client_addr: Option<String>,
        cwd: Option<String>,
        model: Option<String>,
        requested_model: Option<String>,
        reasoning_effort: Option<String>,
        service_tier: Option<String>,
        requested_service_tier: Option<String>,
        provider_catalog: Arc<ProviderCatalogSnapshot>,
        operator_pricing_catalog: Arc<CapturedModelPriceCatalog>,
        runtime_revision: u64,
        runtime_digest: String,
        policy_revision: u64,
        started_at_ms: u64,
    ) -> Result<u64, RuntimeStoreError> {
        let session_id = session_id.and_then(|session_id| {
            let canonical = session_id.trim();
            (!canonical.is_empty()).then(|| canonical.to_string())
        });
        let session_route_control = match session_id.as_deref() {
            Some(session_id) => Some(self.lock_session_route_control(session_id).await),
            None => None,
        };
        self.try_begin_request_with_session_route_control(
            session_route_control.as_ref(),
            service,
            method,
            path,
            session_identity_source,
            client_name,
            client_addr,
            cwd,
            model,
            requested_model,
            reasoning_effort,
            service_tier,
            requested_service_tier,
            provider_catalog,
            operator_pricing_catalog,
            runtime_revision,
            runtime_digest,
            policy_revision,
            started_at_ms,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn try_begin_request_with_session_route_control(
        &self,
        session_route_control: Option<&SessionRouteControlGuard>,
        service: &str,
        method: &str,
        path: &str,
        session_identity_source: Option<SessionIdentitySource>,
        client_name: Option<String>,
        client_addr: Option<String>,
        cwd: Option<String>,
        model: Option<String>,
        requested_model: Option<String>,
        reasoning_effort: Option<String>,
        service_tier: Option<String>,
        requested_service_tier: Option<String>,
        provider_catalog: Arc<ProviderCatalogSnapshot>,
        operator_pricing_catalog: Arc<CapturedModelPriceCatalog>,
        runtime_revision: u64,
        runtime_digest: String,
        policy_revision: u64,
        started_at_ms: u64,
    ) -> Result<u64, RuntimeStoreError> {
        if let Some(session_route_control) = session_route_control {
            self.validate_session_route_control_guard(session_route_control)?;
        }
        let session_id = session_route_control.map(|guard| guard.session_id().to_string());
        let session_identity_source = session_id.as_ref().and(session_identity_source);
        let lifecycle_id = LogicalRequestId::new();
        let mut request_state = self.request_lifecycle_projection.write().await;
        let lifecycle = self.with_runtime_store_blocking(|runtime_store| {
            runtime_store.transaction(|transaction| {
                transaction.begin_logical_request(NewLogicalRequest {
                    id: lifecycle_id,
                    begun_at_unix_ms: started_at_ms,
                })
            })
        })?;
        debug_assert_eq!(lifecycle.disposition, BeginDisposition::Inserted);

        let id = request_state.next_request_id;
        request_state.next_request_id = request_state.next_request_id.saturating_add(1);
        let trace_id = Some(crate::logging::request_trace_id(service, id));
        let req = ActiveRequest {
            id,
            runtime_revision,
            runtime_digest,
            policy_revision,
            trace_id,
            session_id,
            session_identity_source,
            client_name,
            client_addr,
            cwd,
            model,
            requested_model,
            reasoning_effort,
            service_tier,
            requested_service_tier,
            provider_id: None,
            route_decision: None,
            service: service.to_string(),
            method: method.to_string(),
            path: path.to_string(),
            started_at_ms,
        };
        request_state.lifecycle_handles.insert(id, lifecycle.handle);
        request_state.provider_catalogs.insert(id, provider_catalog);
        request_state
            .pricing_catalogs
            .insert(id, operator_pricing_catalog);
        request_state.active_requests.insert(id, req);
        self.notify_state_changed();
        Ok(id)
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn begin_request(
        &self,
        service: &str,
        method: &str,
        path: &str,
        session_id: Option<String>,
        session_identity_source: Option<SessionIdentitySource>,
        client_name: Option<String>,
        client_addr: Option<String>,
        cwd: Option<String>,
        model: Option<String>,
        reasoning_effort: Option<String>,
        service_tier: Option<String>,
        started_at_ms: u64,
    ) -> u64 {
        self.try_begin_request(
            service,
            method,
            path,
            session_id,
            session_identity_source,
            client_name,
            client_addr,
            cwd,
            model.clone(),
            model,
            reasoning_effort,
            service_tier.clone(),
            service_tier,
            Arc::new(ProviderCatalogSnapshot::bundled()),
            Arc::new(capture_operator_model_price_catalog()),
            1,
            "test-runtime".to_string(),
            self.capture_provider_policy_snapshot()
                .await
                .policy_revision,
            started_at_ms,
        )
        .await
        .expect("test logical request should begin durably")
    }

    #[cfg(test)]
    pub(crate) fn begin_request_for_test(&self) -> BeginRequestTestBuilder<'_> {
        BeginRequestTestBuilder::new(self)
    }

    pub async fn update_request_route(
        &self,
        request_id: u64,
        route_decision: RouteDecisionProvenance,
    ) {
        let mut request_state = self.request_lifecycle_projection.write().await;
        let Some(req) = request_state.active_requests.get_mut(&request_id) else {
            return;
        };
        req.provider_id = route_decision.provider_id.clone();
        req.route_decision = Some(route_decision);
        self.notify_state_changed();
    }

    pub(crate) async fn capture_upstream_attempt_context(
        &self,
        request_id: u64,
        route_evidence: AttemptRouteEvidence,
        provider_scope: AttemptProviderScopeCapture,
    ) -> Result<CapturedUpstreamAttemptContext, RuntimeStoreError> {
        let request_state = self.request_lifecycle_projection.read().await;
        let logical_request = request_state
            .lifecycle_handles
            .get(&request_id)
            .copied()
            .ok_or_else(|| RuntimeStoreError::InvariantViolation {
                entity: "upstream attempt",
                id: request_id.to_string(),
                detail: "logical request handle is unavailable".to_string(),
            })?;
        let (runtime_revision, runtime_digest) = request_state
            .active_requests
            .get(&request_id)
            .map(|request| (request.runtime_revision, request.runtime_digest.clone()))
            .ok_or_else(|| RuntimeStoreError::InvariantViolation {
                entity: "upstream attempt",
                id: request_id.to_string(),
                detail: "active request evidence is unavailable".to_string(),
            })?;
        let catalog_snapshot = request_state
            .provider_catalogs
            .get(&request_id)
            .cloned()
            .ok_or_else(|| RuntimeStoreError::InvariantViolation {
                entity: "upstream attempt",
                id: request_id.to_string(),
                detail: "captured provider catalog is unavailable".to_string(),
            })?;
        drop(request_state);
        let adapter = ProviderAdapter::for_endpoint(&provider_scope.endpoint);
        let scope = ProviderCatalogScope::new(
            adapter,
            provider_scope.endpoint.as_str(),
            provider_scope.route_scope,
            provider_scope.account_fingerprint,
            runtime_digest.as_str(),
        )
        .map_err(|error| RuntimeStoreError::InvariantViolation {
            entity: "upstream attempt",
            id: request_id.to_string(),
            detail: format!("provider scope is invalid: {error}"),
        })?;
        let provider_epoch = if adapter == ProviderAdapter::OpenAiCodex {
            Some(
                catalog_snapshot
                    .capture_epoch(scope.clone())
                    .map_err(|error| RuntimeStoreError::InvariantViolation {
                        entity: "upstream attempt",
                        id: request_id.to_string(),
                        detail: format!("provider catalog capture failed: {error}"),
                    })?,
            )
        } else {
            None
        };
        let request_contract = provider_epoch.as_ref().and_then(|epoch| {
            route_evidence
                .mapped_model
                .as_deref()
                .and_then(|model| epoch.capture_model_request_contract(model))
        });

        Ok(CapturedUpstreamAttemptContext {
            request_id,
            logical_request,
            runtime_revision,
            runtime_digest,
            route_evidence,
            scope,
            provider_epoch,
            request_contract,
        })
    }

    pub(crate) async fn begin_upstream_attempt(
        &self,
        context: &CapturedUpstreamAttemptContext,
        begun_at_unix_ms: u64,
    ) -> Result<AttemptHandle, RuntimeStoreError> {
        let frozen_epoch = freeze_provider_epoch(&context.scope, context.provider_epoch.as_ref());
        let attempt_id = AttemptId::new();
        let mut request_state = self.request_lifecycle_projection.write().await;
        let result = self.with_runtime_store_blocking(|runtime_store| {
            runtime_store.transaction(|transaction| {
                transaction.begin_attempt(
                    context.logical_request,
                    NewAttempt {
                        id: attempt_id,
                        logical_request_id: context.logical_request.id(),
                        begun_at_unix_ms,
                        evidence: AttemptPendingEvidence::new(
                            context.runtime_revision,
                            context.runtime_digest.clone(),
                            context.route_evidence.clone(),
                        )
                        .with_provider_epoch(frozen_epoch),
                    },
                )
            })
        })?;
        debug_assert_eq!(result.disposition, BeginDisposition::Inserted);
        if let Some(provider_epoch) = context.provider_epoch.clone() {
            request_state
                .attempt_epochs
                .entry(context.request_id)
                .or_default()
                .insert(attempt_id, provider_epoch);
        }
        Ok(result.handle)
    }

    pub fn finish_upstream_attempt(
        &self,
        attempt: AttemptHandle,
        outcome: AttemptOutcome,
        terminal_at_unix_ms: u64,
        economics_state: EconomicsState,
    ) -> Result<TerminalDisposition, RuntimeStoreError> {
        self.with_runtime_store_blocking(|runtime_store| {
            runtime_store.transaction(|transaction| {
                transaction.commit_attempt_terminal(
                    attempt,
                    AttemptTerminal {
                        outcome,
                        terminal_at_unix_ms,
                        economics_state,
                    },
                )
            })
        })
    }

    pub async fn finish_request(&self, params: FinishRequestParams) -> bool {
        self.finish_request_inner(params, true, None).await
    }

    pub(crate) async fn finish_request_with_session_route_affinity(
        &self,
        params: FinishRequestParams,
        route_affinity_success: SessionRouteAffinitySuccess,
    ) -> bool {
        self.finish_request_inner(params, true, Some(route_affinity_success))
            .await
    }

    pub async fn finish_non_economic_request(&self, mut params: FinishRequestParams) -> bool {
        params.usage = None;
        self.finish_request_inner(params, false, None).await
    }

    pub(crate) async fn finish_non_economic_request_with_session_route_affinity(
        &self,
        mut params: FinishRequestParams,
        route_affinity_success: SessionRouteAffinitySuccess,
    ) -> bool {
        params.usage = None;
        self.finish_request_inner(params, false, Some(route_affinity_success))
            .await
    }

    async fn finish_request_inner(
        &self,
        params: FinishRequestParams,
        include_in_economics: bool,
        route_affinity_success: Option<SessionRouteAffinitySuccess>,
    ) -> bool {
        let _operator_capture = self.operator_capture.write().await;
        let winning_attempt = match params.winning_attempt {
            Some(attempt) if attempt.store_id() == self.runtime_store.identity().store_id() => {
                Some(attempt)
            }
            Some(attempt) => {
                tracing::error!(
                    request_id = params.id,
                    attempt_id = %attempt.id(),
                    "refusing logical terminal with an attempt from another runtime store"
                );
                return false;
            }
            None => None,
        };
        let winning_attempt_record = match winning_attempt {
            Some(attempt) => match self
                .with_runtime_store_blocking(|runtime_store| runtime_store.read_attempt(attempt))
            {
                Ok(Some(record)) => Some(record),
                Ok(None) => {
                    tracing::error!(
                        request_id = params.id,
                        attempt_id = %attempt.id(),
                        "winning attempt is missing from the runtime store"
                    );
                    return false;
                }
                Err(error) => {
                    tracing::error!(
                        request_id = params.id,
                        attempt_id = %attempt.id(),
                        error = %error,
                        "failed to read winning attempt evidence"
                    );
                    return false;
                }
            },
            None => None,
        };
        let winning_attempt_id = winning_attempt.map(|attempt| attempt.id());
        let mut request_state = self.request_lifecycle_projection.write().await;
        let Some(lifecycle) = request_state.lifecycle_handles.get(&params.id).copied() else {
            return false;
        };
        let captured_pricing_catalog = request_state.pricing_catalogs.get(&params.id).cloned();
        let captured_provider_epoch = match winning_attempt_id {
            Some(attempt_id) => request_state
                .attempt_epochs
                .get(&params.id)
                .and_then(|epochs| epochs.get(&attempt_id))
                .cloned(),
            None => None,
        };
        let Some(req) = request_state.active_requests.get(&params.id).cloned() else {
            return false;
        };
        if let Some(success) = route_affinity_success.as_ref()
            && (success.request_id != params.id
                || req.session_id.as_deref() != Some(success.session_id.as_str()))
        {
            tracing::error!(
                request_id = params.id,
                affinity_request_id = success.request_id,
                affinity_session_id = success.session_id,
                request_session_id = ?req.session_id,
                "refusing a logical terminal with mismatched session route affinity evidence"
            );
            return false;
        }

        let winner_evidence = winning_attempt_record
            .as_ref()
            .map(|record| &record.attempt.evidence);
        let winner_route = winner_evidence.map(|evidence| &evidence.route);
        let requested_model = req.requested_model.clone();
        let mapped_model = match winner_route {
            Some(route) => route.mapped_model.clone(),
            None => req
                .route_decision
                .as_ref()
                .and_then(|decision| decision.effective_model.as_ref())
                .map(|value| value.value.clone())
                .or_else(|| requested_model.clone()),
        };
        let requested_service_tier = req.requested_service_tier.clone();
        let effective_service_tier = req.service_tier.clone();
        let actual_service_tier = params.observed_service_tier.clone();
        let reported_model = params.reported_model.clone();
        let model_conflict = mapped_model
            .as_deref()
            .zip(reported_model.as_deref())
            .is_some_and(|(mapped, reported)| !mapped.trim().eq_ignore_ascii_case(reported.trim()));
        let actual_pricing_tier =
            ProviderPricingTier::from_actual_service_tier(actual_service_tier.as_deref());
        let pricing_service_tier = actual_service_tier
            .as_ref()
            .map(|_| actual_pricing_tier.as_str().to_string());
        let provider_epoch = winner_evidence.and_then(|evidence| evidence.provider_epoch.clone());
        if let (Some(captured), Some(frozen)) =
            (captured_provider_epoch.as_ref(), provider_epoch.as_ref())
            && freeze_provider_epoch(captured.scope(), Some(captured)) != *frozen
        {
            tracing::error!(
                request_id = params.id,
                attempt_id = ?winning_attempt_id,
                "captured provider catalog conflicts with durable winning attempt epoch"
            );
            return false;
        }
        if provider_epoch
            .as_ref()
            .is_some_and(|epoch| epoch.catalog_revision.is_some())
            && captured_provider_epoch.is_none()
        {
            tracing::error!(
                request_id = params.id,
                attempt_id = ?winning_attempt_id,
                "winning attempt provider catalog is unavailable"
            );
            return false;
        }

        let mut cache_accounting_convention = crate::usage::CacheAccountingConvention::UNKNOWN;
        let mut provider_price_key = None;
        let mut pricing_model = None;
        let mut cost = CostBreakdown::unknown();
        if include_in_economics && !model_conflict {
            if let (Some(epoch), Some(model), Some(frozen_epoch)) = (
                captured_provider_epoch.as_ref(),
                mapped_model.as_deref(),
                provider_epoch.clone(),
            ) {
                let key = epoch.capture_price_key(model, actual_pricing_tier);
                if let Some(quote) = epoch.price_quote(&key) {
                    cache_accounting_convention = quote.cache_accounting_convention();
                }
                pricing_model = Some(key.model().to_string());
                provider_price_key = Some(FrozenProviderPriceKey {
                    epoch: frozen_epoch,
                    model: key.model().to_string(),
                    tier: key.tier(),
                });
                if let Some(usage) = params.usage.as_ref() {
                    cost = estimate_usage_cost_from_captured_provider_price(epoch, &key, usage);
                }
            } else if winning_attempt_record.is_none()
                && let (Some(catalog), Some(model), Some(usage)) = (
                    captured_pricing_catalog.as_ref(),
                    mapped_model.as_deref(),
                    params.usage.as_ref(),
                )
            {
                pricing_model = Some(model.to_string());
                cost = catalog.estimate_usage_cost_with_convention(
                    model,
                    usage,
                    CostAdjustments::default(),
                    cache_accounting_convention,
                );
            }
        }
        let billable_usage = if include_in_economics {
            params
                .usage
                .as_ref()
                .map(|usage| usage.canonical_usage_buckets(cache_accounting_convention))
        } else {
            None
        };
        let billable_usage_for_projection = billable_usage;

        let route_decision = winner_route.map_or_else(
            || req.route_decision.clone(),
            |route| Some(winning_route_decision(req.route_decision.clone(), route)),
        );
        let reasoning_effort = route_decision
            .as_ref()
            .and_then(|decision| decision.effective_reasoning_effort.as_ref())
            .map(|effort| effort.value.clone())
            .or(req.reasoning_effort);
        let provider_id = winner_route
            .and_then(|route| route.provider_id.clone())
            .or_else(|| {
                winning_attempt_record
                    .is_none()
                    .then(|| {
                        req.route_decision
                            .as_ref()
                            .and_then(|decision| decision.provider_id.clone())
                            .or(req.provider_id.clone())
                    })
                    .flatten()
            });
        let runtime_revision = winner_evidence
            .map(|evidence| evidence.runtime_revision)
            .unwrap_or(req.runtime_revision);
        let runtime_digest = winner_evidence
            .map(|evidence| evidence.runtime_digest.clone())
            .unwrap_or_else(|| req.runtime_digest.clone());

        let mut finished = FinishedRequest {
            id: params.id,
            trace_id: req.trace_id,
            session_id: req.session_id,
            session_identity_source: req.session_identity_source,
            client_name: req.client_name,
            client_addr: req.client_addr,
            cwd: req.cwd,
            model: req.model,
            reasoning_effort,
            service_tier: actual_service_tier
                .clone()
                .or_else(|| effective_service_tier.clone()),
            provider_id,
            route_decision,
            usage: params.usage.clone(),
            cost,
            accounting: RequestAccountingFacts::default(),
            retry: params.retry,
            provider_signals: Vec::new(),
            policy_actions: Vec::new(),
            observability: RequestObservability::default(),
            service: req.service,
            method: req.method,
            path: req.path,
            status_code: params.status_code,
            duration_ms: params.duration_ms,
            ttfb_ms: params.ttfb_ms,
            streaming: params.streaming,
            ended_at_ms: params.ended_at_ms,
        };
        let provider_endpoint = finished.provider_endpoint();
        let pool_membership = provider_endpoint.as_ref().and_then(|endpoint| {
            self.quota_pool_registry
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .membership_for_endpoint(endpoint)
                .as_ref()
                .map(AccountingPoolMembership::from_membership)
        });
        finished.accounting = RequestAccountingFacts {
            schema_version: RequestAccountingFacts::SCHEMA_VERSION,
            project: ProjectIdentity::from_cwd(finished.cwd.as_deref()),
            provider_endpoint,
            pool_membership,
            price_coverage: classify_captured_cost(&finished.cost),
            cache_accounting_convention,
        };
        finished.refresh_observability();

        let outcome = if is_logical_request_success_status(params.status_code) {
            LogicalRequestOutcome::Succeeded
        } else {
            LogicalRequestOutcome::Failed
        };
        let economics_state = if !include_in_economics || finished.cost.is_unknown() {
            EconomicsState::Unknown
        } else {
            match finished.cost.confidence {
                CostConfidence::Exact => EconomicsState::Known,
                CostConfidence::Estimated | CostConfidence::Partial => EconomicsState::Partial,
                CostConfidence::Unknown => EconomicsState::Unknown,
            }
        };
        let terminal = LogicalRequestTerminal {
            outcome,
            terminal_at_unix_ms: params.ended_at_ms,
            economics_state,
            payload: Some(LogicalRequestTerminalPayload {
                finished_request: finished.clone(),
                winning_attempt_id,
                runtime_revision,
                runtime_digest,
                policy_revision: Some(req.policy_revision),
                provider_epoch,
                provider_price_key,
                requested_model,
                mapped_model,
                reported_model,
                pricing_model,
                requested_service_tier,
                effective_service_tier,
                actual_service_tier,
                pricing_service_tier,
                cache_accounting_convention,
                billable_usage,
                accounting_scope: if include_in_economics {
                    RequestAccountingScope::Economic
                } else {
                    RequestAccountingScope::NonEconomic
                },
            }),
        };
        let route_affinity_update_guard = if route_affinity_success.is_some() {
            Some(self.session_route_affinity_updates.lock().await)
        } else {
            None
        };
        let committed_route_affinity = match route_affinity_success.as_ref() {
            Some(success) => match self
                .prepare_session_route_affinity_transaction(success, params.ended_at_ms)
                .await
            {
                Ok(affinity) => affinity,
                Err(error) => {
                    tracing::error!(
                        request_id = params.id,
                        session_id = success.session_id,
                        error = %error,
                        "refusing a logical terminal with invalid session route affinity ownership"
                    );
                    return false;
                }
            },
            None => None,
        };
        let affinity_record = committed_route_affinity.as_ref().map(|affinity| {
            let success = route_affinity_success
                .as_ref()
                .expect("committed affinity requires success evidence");
            session_affinity_record(success.session_id.as_str(), affinity)
        });
        let terminal_disposition = match self.with_runtime_store_blocking(|runtime_store| {
            runtime_store.transaction(|transaction| {
                let disposition =
                    transaction.commit_logical_request_terminal(lifecycle, terminal)?;
                if let Some(record) = affinity_record {
                    transaction.upsert_session_affinity(
                        record,
                        session_affinity_limit(self.session_route_affinity_max_entries),
                    )?;
                }
                Ok(disposition)
            })
        }) {
            Ok(disposition) => disposition,
            Err(error) => {
                tracing::error!(
                    request_id = params.id,
                    error = %error,
                    "failed to commit durable logical request terminal"
                );
                return false;
            }
        };
        if let Some(success) = route_affinity_success.as_ref() {
            let mut reservations = self.session_route_reservations.lock().await;
            if reservations
                .get(success.session_id.as_str())
                .is_some_and(|reservation| reservation.owner_request_id == success.request_id)
            {
                reservations.remove(success.session_id.as_str());
            }
        }
        drop(route_affinity_update_guard);
        #[cfg(test)]
        self.pause_terminal_publication_after_commit_for_test()
            .await;
        request_state.active_requests.remove(&params.id);
        request_state.lifecycle_handles.remove(&params.id);
        request_state.provider_catalogs.remove(&params.id);
        request_state.attempt_epochs.remove(&params.id);
        request_state.pricing_catalogs.remove(&params.id);

        if terminal_disposition == TerminalDisposition::Committed {
            request_state.committed_terminal_count =
                request_state.committed_terminal_count.saturating_add(1);
        }

        if include_in_economics {
            request_state.attribution_index.record(&finished);
            let recorded = {
                let rollup = request_state
                    .usage_rollups
                    .entry(finished.service.clone())
                    .or_default();
                record_finished_request_into_usage_rollup(
                    rollup,
                    &lifecycle.id().to_string(),
                    &finished,
                )
            };
            if recorded {
                record_finished_request_into_operator_usage_summary(
                    &mut request_state.operator_usage_summaries,
                    &finished,
                    billable_usage_for_projection.as_ref(),
                );
            }
        }

        if include_in_economics && let Some(sid) = finished.session_id.as_deref() {
            let entry = request_state
                .session_stats
                .entry(finished.service.clone())
                .or_default()
                .entry(sid.to_string())
                .or_default();
            entry.turns_total = entry.turns_total.saturating_add(1);
            if finished.session_identity_source.is_some() {
                entry.last_session_identity_source = finished.session_identity_source;
            }
            entry.last_client_name = finished
                .client_name
                .clone()
                .or(entry.last_client_name.clone());
            entry.last_client_addr = finished
                .client_addr
                .clone()
                .or(entry.last_client_addr.clone());
            entry.last_model = finished.model.clone().or(entry.last_model.clone());
            entry.last_reasoning_effort = finished
                .reasoning_effort
                .clone()
                .or(entry.last_reasoning_effort.clone());
            entry.last_service_tier = finished
                .service_tier
                .clone()
                .or(entry.last_service_tier.clone());
            entry.last_provider_id = finished
                .provider_id
                .clone()
                .or(entry.last_provider_id.clone());
            if finished.route_decision.is_some() {
                entry.last_route_decision = finished.route_decision.clone();
            }
            if let Some(u) = finished.usage.as_ref() {
                entry.last_usage = Some(u.clone());
                entry.total_usage.add_assign(u);
                entry.turns_with_usage = entry.turns_with_usage.saturating_add(1);
            }
            if finished
                .usage
                .as_ref()
                .is_some_and(|usage| usage.output_tokens > 0)
            {
                entry.last_output_tokens_per_second =
                    finished.observability.output_tokens_per_second;
                if let Some(generation_ms) = finished.observability.generation_ms {
                    entry.output_generation_ms_total = entry
                        .output_generation_ms_total
                        .saturating_add(generation_ms);
                    entry.avg_output_tokens_per_second =
                        self::session_identity::token_weighted_output_tokens_per_second(
                            entry.total_usage.output_tokens,
                            entry.output_generation_ms_total,
                        );
                }
            }
            entry.last_status = Some(finished.status_code);
            entry.last_duration_ms = Some(finished.duration_ms);
            entry.last_ended_at_ms = Some(finished.ended_at_ms);
            entry.last_seen_ms = finished.ended_at_ms;
        }

        request_state.recent_finished.push_front(finished);
        while request_state.recent_finished.len() > recent_finished_max() {
            request_state.recent_finished.pop_back();
        }
        self.notify_state_changed();
        true
    }

    pub async fn list_active_requests(&self) -> Vec<ActiveRequest> {
        let request_state = self.request_lifecycle_projection.read().await;
        let mut vec = request_state
            .active_requests
            .values()
            .cloned()
            .collect::<Vec<_>>();
        vec.sort_by_key(|r| r.started_at_ms);
        vec
    }

    pub async fn list_recent_finished(&self, limit: usize) -> Vec<FinishedRequest> {
        self.request_lifecycle_projection
            .read()
            .await
            .recent_finished
            .iter()
            .take(limit)
            .cloned()
            .collect()
    }

    pub async fn list_session_stats(&self, service_name: &str) -> HashMap<String, SessionStats> {
        self.request_lifecycle_projection
            .read()
            .await
            .session_stats
            .get(service_name)
            .cloned()
            .unwrap_or_default()
    }

    pub async fn list_session_identity_cards(
        &self,
        service_name: &str,
        recent_limit: usize,
    ) -> Vec<SessionIdentityCard> {
        let recent_limit = recent_limit.clamp(1, recent_finished_max());
        let (active, recent, bindings, route_affinities, stats) = tokio::join!(
            self.list_active_requests(),
            self.list_recent_finished(recent_finished_max()),
            self.list_session_bindings(),
            self.list_session_route_affinities(),
            self.list_session_stats(service_name),
        );
        let active = active
            .into_iter()
            .filter(|request| request.service == service_name)
            .collect::<Vec<_>>();
        let recent = recent
            .into_iter()
            .filter(|request| request.service == service_name)
            .take(recent_limit)
            .collect::<Vec<_>>();
        build_session_identity_cards_from_parts(SessionIdentityCardBuildInputs {
            active: &active,
            recent: &recent,
            bindings: &bindings,
            route_affinities: &route_affinities,
            stats: &stats,
        })
    }

    async fn resolve_host_transcript_paths_cached(
        &self,
        session_ids: &[String],
    ) -> HashMap<String, Option<String>> {
        let mut unique = session_ids
            .iter()
            .map(|sid| sid.trim())
            .filter(|sid| !sid.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        unique.sort();
        unique.dedup();
        if unique.is_empty() {
            return HashMap::new();
        }

        if self.session_transcript_path_cache_max_entries == 0 {
            return sessions::find_codex_session_files_by_ids(&unique)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|(sid, path)| (sid, Some(path.to_string_lossy().to_string())))
                .collect();
        }

        let now_ms = unix_now_ms();
        let ttl_ms = self.session_transcript_path_cache_ttl_ms;
        let mut resolved = HashMap::<String, Option<String>>::new();
        let mut stale_or_missing = Vec::<String>::new();

        {
            let cache = self.session_transcript_path_cache.read().await;
            for sid in &unique {
                let fresh = cache.get(sid).filter(|entry| {
                    ttl_ms == 0 || now_ms.saturating_sub(entry.last_checked_ms) <= ttl_ms
                });
                if let Some(entry) = fresh {
                    resolved.insert(sid.clone(), entry.path.clone());
                } else {
                    stale_or_missing.push(sid.clone());
                }
            }
        }

        if !stale_or_missing.is_empty() {
            let found = sessions::find_codex_session_files_by_ids(&stale_or_missing)
                .await
                .unwrap_or_default();
            let mut cache = self.session_transcript_path_cache.write().await;
            for sid in stale_or_missing {
                let path = found
                    .get(&sid)
                    .map(|path| path.to_string_lossy().to_string());
                cache.insert(
                    sid.clone(),
                    SessionTranscriptPathCacheEntry {
                        path: path.clone(),
                        last_checked_ms: now_ms,
                        last_seen_ms: now_ms,
                    },
                );
                resolved.insert(sid, path);
            }
            prune_lru_cache(
                &mut cache,
                self.session_transcript_path_cache_max_entries,
                |entry| entry.last_seen_ms,
            );
        }

        {
            let mut cache = self.session_transcript_path_cache.write().await;
            for sid in &unique {
                if let Some(entry) = cache.get_mut(sid) {
                    entry.last_seen_ms = now_ms;
                }
            }
        }

        resolved
    }

    pub async fn enrich_session_identity_cards_with_cached_host_transcripts(
        &self,
        cards: &mut [SessionIdentityCard],
    ) {
        let session_ids = cards
            .iter()
            .filter_map(|card| card.session_id.as_deref())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let resolved = self
            .resolve_host_transcript_paths_cached(&session_ids)
            .await;
        for card in cards {
            card.host_local_transcript_path = card
                .session_id
                .as_deref()
                .and_then(|sid| resolved.get(sid))
                .and_then(Clone::clone);
        }
    }

    pub async fn list_session_identity_cards_with_host_transcripts(
        &self,
        service_name: &str,
        recent_limit: usize,
    ) -> Vec<SessionIdentityCard> {
        let mut cards = self
            .list_session_identity_cards(service_name, recent_limit)
            .await;
        self.enrich_session_identity_cards_with_cached_host_transcripts(&mut cards)
            .await;
        cards
    }

    pub fn spawn_cleanup_task(state: &Arc<Self>) {
        let state = Arc::downgrade(state);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let Some(state) = state.upgrade() else {
                    break;
                };
                state.prune_periodic().await;
            }
        });
    }

    async fn prune_periodic(&self) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Collect active session_ids to avoid clearing overrides for currently running requests.
        let (active_sessions, active_service_sessions) = {
            let request_state = self.request_lifecycle_projection.read().await;
            let mut sessions = HashSet::new();
            let mut service_sessions = HashMap::<String, HashSet<String>>::new();
            for request in request_state.active_requests.values() {
                let Some(session_id) = request.session_id.as_ref() else {
                    continue;
                };
                sessions.insert(session_id.clone());
                service_sessions
                    .entry(request.service.clone())
                    .or_default()
                    .insert(session_id.clone());
            }
            (sessions, service_sessions)
        };

        if self.session_binding_ttl_ms > 0 && now_ms >= self.session_binding_ttl_ms {
            let cutoff_binding = now_ms - self.session_binding_ttl_ms;
            let mut bindings = self.session_bindings.write().await;
            bindings.retain(|sid, entry| {
                if active_sessions.contains(sid) {
                    return true;
                }
                entry.binding.last_seen_ms >= cutoff_binding
            });
        }
        if self.session_binding_max_entries > 0 {
            let mut bindings = self.session_bindings.write().await;
            if bindings.len() > self.session_binding_max_entries {
                let mut removable = bindings
                    .iter()
                    .filter(|(sid, _)| !active_sessions.contains(*sid))
                    .map(|(sid, entry)| (sid.clone(), entry.binding.last_seen_ms))
                    .collect::<Vec<_>>();
                removable.sort_by_key(|(_, last_seen_ms)| *last_seen_ms);
                let remove_count = bindings
                    .len()
                    .saturating_sub(self.session_binding_max_entries)
                    .min(removable.len());
                for (sid, _) in removable.into_iter().take(remove_count) {
                    bindings.remove(&sid);
                }
            }
        }

        {
            let _update_guard = self.session_route_affinity_updates.lock().await;
            if let Err(error) = self.with_runtime_store_blocking(|runtime_store| {
                runtime_store.prune_session_affinities(
                    now_ms,
                    self.session_route_affinity_ttl_ms,
                    session_affinity_limit(self.session_route_affinity_max_entries),
                )
            }) {
                tracing::warn!(
                    error = %error,
                    "failed to prune session route affinities in runtime store"
                );
            }
        }

        // Keep a bounded number of days of rollup data to avoid unbounded growth.
        let cutoff_day = usage_rollup_cutoff_day(now_ms);
        self.prune_session_transcript_path_cache(now_ms).await;

        let mut request_state = self.request_lifecycle_projection.write().await;
        for rollup in request_state.usage_rollups.values_mut() {
            rollup.recorded_requests.retain(|_, day| *day >= cutoff_day);
            rollup
                .terminal_range_by_day
                .retain(|day, _| *day >= cutoff_day);
            rollup.by_day.retain(|day, _| *day >= cutoff_day);
            rollup.by_hour.retain(|day, _| *day >= cutoff_day);
            prune_usage_entity_days(&mut rollup.by_provider_endpoint_day, cutoff_day);
            prune_usage_entity_days(&mut rollup.by_provider_day, cutoff_day);
            prune_usage_entity_days(&mut rollup.by_model_day, cutoff_day);
            prune_usage_entity_days(&mut rollup.by_session_day, cutoff_day);
            prune_usage_entity_days(&mut rollup.by_project_day, cutoff_day);
            rebuild_usage_rollup_totals(rollup);
        }
        prune_operator_usage_summary_days(&mut request_state.operator_usage_summaries, cutoff_day);

        if self.session_stats_ttl_ms > 0 && now_ms >= self.session_stats_ttl_ms {
            let cutoff_stats = now_ms - self.session_stats_ttl_ms;
            request_state.session_stats.retain(|service, stats| {
                stats.retain(|sid, stats| {
                    active_service_sessions
                        .get(service)
                        .is_some_and(|sessions| sessions.contains(sid))
                        || stats.last_seen_ms >= cutoff_stats
                });
                !stats.is_empty()
            });
        }
    }

    async fn prune_session_transcript_path_cache(&self, now_ms: u64) {
        let mut cache = self.session_transcript_path_cache.write().await;
        if self.session_transcript_path_cache_max_entries == 0 {
            cache.clear();
            return;
        }

        if self.session_transcript_path_cache_ttl_ms > 0 {
            let cutoff = now_ms.saturating_sub(self.session_transcript_path_cache_ttl_ms);
            cache.retain(|_, entry| entry.last_seen_ms >= cutoff);
        }

        prune_lru_cache(
            &mut cache,
            self.session_transcript_path_cache_max_entries,
            |entry| entry.last_seen_ms,
        );
    }
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct BeginRequestTestBuilder<'a> {
    state: &'a ProxyState,
    service: &'static str,
    method: &'static str,
    path: &'static str,
    session_id: Option<String>,
    session_identity_source: Option<SessionIdentitySource>,
    client_name: Option<String>,
    client_addr: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    service_tier: Option<String>,
    started_at_ms: u64,
}

#[cfg(test)]
impl<'a> BeginRequestTestBuilder<'a> {
    pub(crate) fn new(state: &'a ProxyState) -> Self {
        Self {
            state,
            service: "codex",
            method: "POST",
            path: "/v1/responses",
            session_id: None,
            session_identity_source: None,
            client_name: None,
            client_addr: None,
            cwd: None,
            model: None,
            reasoning_effort: None,
            service_tier: None,
            started_at_ms: 0,
        }
    }

    pub(crate) fn session_id(mut self, value: impl Into<String>) -> Self {
        self.session_id = Some(value.into());
        self
    }

    pub(crate) fn model(mut self, value: impl Into<String>) -> Self {
        self.model = Some(value.into());
        self
    }

    pub(crate) fn cwd(mut self, value: impl Into<String>) -> Self {
        self.cwd = Some(value.into());
        self
    }

    pub(crate) fn service_tier(mut self, value: impl Into<String>) -> Self {
        self.service_tier = Some(value.into());
        self
    }

    pub(crate) fn started_at_ms(mut self, value: u64) -> Self {
        self.started_at_ms = value;
        self
    }

    pub(crate) async fn begin(self) -> u64 {
        self.state
            .begin_request(
                self.service,
                self.method,
                self.path,
                self.session_id,
                self.session_identity_source,
                self.client_name,
                self.client_addr,
                self.cwd,
                self.model,
                self.reasoning_effort,
                self.service_tier,
                self.started_at_ms,
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::{ProviderConfig, RouteGraphConfig, ServiceRouteConfig};
    use crate::runtime_identity::ProviderEndpointKey;
    use std::path::Path;
    use std::sync::OnceLock;

    fn route_view(providers: &[(&str, &str)]) -> ServiceRouteConfig {
        ServiceRouteConfig {
            providers: providers
                .iter()
                .map(|(provider_id, base_url)| {
                    (
                        (*provider_id).to_string(),
                        ProviderConfig {
                            base_url: Some((*base_url).to_string()),
                            ..ProviderConfig::default()
                        },
                    )
                })
                .collect(),
            routing: Some(RouteGraphConfig::ordered_failover(
                providers
                    .iter()
                    .map(|(provider_id, _)| (*provider_id).to_string())
                    .collect(),
            )),
            ..ServiceRouteConfig::default()
        }
    }

    fn provider_route_decision(
        provider_id: &str,
        endpoint_id: &str,
        upstream_base_url: &str,
    ) -> RouteDecisionProvenance {
        RouteDecisionProvenance {
            effective_upstream_base_url: Some(ResolvedRouteValue::new(
                upstream_base_url,
                RouteValueSource::RuntimeFallback,
            )),
            provider_id: Some(provider_id.to_string()),
            endpoint_id: Some(endpoint_id.to_string()),
            route_path: vec![provider_id.to_string(), endpoint_id.to_string()],
            ..RouteDecisionProvenance::default()
        }
    }

    fn finished_request_for_operator_snapshot(
        id: u64,
        service: &str,
        session_id: &str,
    ) -> FinishedRequest {
        FinishedRequest {
            id,
            trace_id: Some(format!("{service}-{id}")),
            session_id: Some(session_id.to_string()),
            session_identity_source: Some(SessionIdentitySource::Header),
            client_name: None,
            client_addr: None,
            cwd: None,
            model: Some("gpt-test".to_string()),
            reasoning_effort: None,
            service_tier: None,
            provider_id: None,
            route_decision: None,
            usage: Some(UsageMetrics {
                input_tokens: 1,
                total_tokens: 1,
                ..UsageMetrics::default()
            }),
            cost: CostBreakdown::default(),
            accounting: RequestAccountingFacts::default(),
            retry: None,
            provider_signals: Vec::new(),
            policy_actions: Vec::new(),
            observability: RequestObservability::default(),
            service: service.to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 10,
            ttfb_ms: None,
            streaming: false,
            ended_at_ms: id,
        }
    }

    async fn provider_attempt_context_for_test(
        state: &ProxyState,
        request_id: u64,
        provider_id: &str,
        endpoint_id: &str,
        route_scope: &str,
        mapped_model: &str,
        fingerprint_seed: u8,
    ) -> CapturedUpstreamAttemptContext {
        state
            .capture_upstream_attempt_context(
                request_id,
                AttemptRouteEvidence {
                    provider_endpoint_key: Some(format!("codex/{provider_id}/{endpoint_id}")),
                    provider_id: Some(provider_id.to_string()),
                    endpoint_id: Some(endpoint_id.to_string()),
                    route_path: vec![provider_id.to_string(), endpoint_id.to_string()],
                    upstream_base_url: Some(format!("https://api.openai.com/v1/{provider_id}")),
                    mapped_model: Some(mapped_model.to_string()),
                },
                AttemptProviderScopeCapture {
                    endpoint: reqwest::Url::parse("https://api.openai.com/v1/responses")
                        .expect("OpenAI endpoint"),
                    route_scope: route_scope.to_string(),
                    account_fingerprint: AccountFingerprint::from_digest([fingerprint_seed; 32]),
                },
            )
            .await
            .expect("capture provider attempt context")
    }

    async fn begin_provider_attempt_for_test(
        state: &ProxyState,
        request_id: u64,
        provider_id: &str,
        endpoint_id: &str,
        route_scope: &str,
        mapped_model: &str,
        fingerprint_seed: u8,
    ) -> AttemptHandle {
        let context = provider_attempt_context_for_test(
            state,
            request_id,
            provider_id,
            endpoint_id,
            route_scope,
            mapped_model,
            fingerprint_seed,
        )
        .await;
        state
            .begin_upstream_attempt(&context, 101)
            .await
            .expect("begin provider attempt")
    }

    async fn env_lock() -> tokio::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await
    }

    #[derive(Default)]
    struct ScopedEnv {
        saved: Vec<(String, Option<String>)>,
    }

    impl ScopedEnv {
        unsafe fn set(&mut self, key: &str, value: &str) {
            if !self.saved.iter().any(|(saved_key, _)| saved_key == key) {
                self.saved.push((key.to_string(), std::env::var(key).ok()));
            }
            unsafe {
                std::env::set_var(key, value);
            }
        }

        unsafe fn set_path(&mut self, key: &str, value: &Path) {
            unsafe {
                self.set(key, value.to_string_lossy().as_ref());
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            for (key, value) in self.saved.iter().rev() {
                match value {
                    Some(value) => unsafe {
                        std::env::set_var(key, value);
                    },
                    None => unsafe {
                        std::env::remove_var(key);
                    },
                }
            }
        }
    }

    fn test_runtime_policy(
        session_stats_ttl_ms: u64,
        session_binding_ttl_ms: u64,
        session_binding_max_entries: usize,
    ) -> RuntimePolicy {
        RuntimePolicy {
            session_stats_ttl_ms,
            session_binding_ttl_ms,
            session_binding_max_entries,
            session_route_affinity_ttl_ms: 0,
            session_route_affinity_max_entries: 5_000,
            session_transcript_path_cache_ttl_ms: 30_000,
            session_transcript_path_cache_max_entries: 5_000,
        }
    }

    fn session_route_target(
        route_graph_key: &str,
        provider_id: &str,
    ) -> SessionRouteAffinityTarget {
        SessionRouteAffinityTarget {
            route_graph_key: route_graph_key.to_string(),
            session_identity_source: Some(SessionIdentitySource::Header),
            provider_endpoint: ProviderEndpointKey::new("codex", provider_id, "default"),
            upstream_base_url: format!("https://{provider_id}.example/v1"),
            route_path: vec!["entry".to_string(), provider_id.to_string()],
        }
    }

    fn available_reservation(access: SessionRouteReservationAccess) -> SessionRouteAffinity {
        match access {
            SessionRouteReservationAccess::Available(affinity) => affinity,
            SessionRouteReservationAccess::None => panic!("expected an available reservation"),
            SessionRouteReservationAccess::Busy { owner_request_id } => {
                panic!("reservation unexpectedly busy for request {owner_request_id}")
            }
        }
    }

    struct TempStateHome(PathBuf);

    impl TempStateHome {
        fn new(test_name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "codex-helper-state-home-{test_name}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&path).expect("create temporary helper home");
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempStateHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn explicitly_injected_runtime_store_is_retained() {
        let runtime_store = Arc::new(
            RuntimeStore::open_in_memory().expect("open injected in-memory runtime store"),
        );
        let state = ProxyState::new_with_runtime_policy_and_store(
            test_runtime_policy(0, 0, 2_000),
            runtime_store.clone(),
        )
        .expect("hydrate injected runtime projections");

        assert!(Arc::ptr_eq(&runtime_store, &state.runtime_store_handle()));
    }

    #[test]
    fn usage_account_fingerprint_delegates_to_runtime_store_identity() {
        let runtime_store = Arc::new(
            RuntimeStore::open_in_memory().expect("open injected in-memory runtime store"),
        );
        let expected_identity = runtime_store
            .load_or_create_quota_identity()
            .expect("load runtime quota identity");
        let state = ProxyState::new_with_runtime_policy_and_store(
            test_runtime_policy(0, 0, 2_000),
            runtime_store,
        )
        .expect("hydrate injected runtime projections");

        assert_eq!(
            state.derive_usage_account_fingerprint(b"provider-token", Some("user-42")),
            expected_identity.derive_usage_account_fingerprint(b"provider-token", Some("user-42"))
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn runtime_store_blocking_boundary_keeps_tokio_worker_available() {
        let state = ProxyState::new();
        state
            .runtime_store()
            .delay_next_transaction_for_test(Duration::from_millis(500));

        let timer_started = std::time::Instant::now();
        let timer = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(20)).await;
        });
        tokio::task::yield_now().await;

        let begin_state = Arc::clone(&state);
        let begin = tokio::spawn(async move {
            begin_state
                .begin_request_for_test()
                .started_at_ms(100)
                .begin()
                .await
        });
        tokio::task::yield_now().await;

        timer.await.expect("unrelated timer task should complete");
        assert!(
            timer_started.elapsed() < Duration::from_millis(250),
            "slow runtime-store work blocked the only Tokio worker"
        );
        assert!(
            !begin.is_finished(),
            "delayed runtime-store transaction unexpectedly completed before the timer"
        );
        let request_id = begin.await.expect("join delayed request begin");
        assert_eq!(request_id, 1);
    }

    #[test]
    fn request_begin_does_not_publish_partial_state_when_cancelled() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let begin_state = Arc::clone(&state);
            let publication = state.hold_request_publication_for_test().await;
            let begin = tokio::spawn(async move {
                begin_state
                    .begin_request_for_test()
                    .started_at_ms(100)
                    .begin()
                    .await
            });

            let partial_publication = tokio::time::timeout(Duration::from_millis(200), async {
                loop {
                    if state.lifecycle_handle_count_for_test().await > 0 {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .is_ok();

            begin.abort();
            drop(publication);
            let _ = begin.await;
            assert!(
                !partial_publication,
                "logical request state became partially visible before publication could finish"
            );
        });
    }

    #[test]
    fn attempt_begin_does_not_commit_before_projection_is_available() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let request_id = state
                .begin_request_for_test()
                .model("gpt-5.6-sol")
                .started_at_ms(100)
                .begin()
                .await;
            let context = provider_attempt_context_for_test(
                state.as_ref(),
                request_id,
                "official",
                "default",
                "codex/official/default",
                "gpt-5.6-sol",
                7,
            )
            .await;
            let logical_request = context.logical_request;
            let store = state.runtime_store_handle();
            let attempt_state = Arc::clone(&state);
            let publication = state.hold_attempt_publication_for_test().await;
            let begin =
                tokio::spawn(
                    async move { attempt_state.begin_upstream_attempt(&context, 101).await },
                );

            let durable_attempt = tokio::time::timeout(Duration::from_millis(200), async {
                loop {
                    if !store
                        .read_attempts_for_logical_request(logical_request)
                        .expect("read durable attempts")
                        .is_empty()
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .is_ok();

            begin.abort();
            drop(publication);
            let _ = begin.await;
            assert!(
                !durable_attempt,
                "durable attempt committed before its in-process projection was publishable"
            );
        });
    }

    #[test]
    fn terminal_does_not_commit_before_projection_is_available() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let request_id = state
                .begin_request_for_test()
                .session_id("sid-terminal-cancellation")
                .started_at_ms(100)
                .begin()
                .await;
            let store = state.runtime_store_handle();
            let initial_revision = store
                .operator_ledger_revision()
                .expect("read initial ledger revision");
            let finish_state = Arc::clone(&state);
            let publication = state.hold_request_publication_for_test().await;
            let finish = tokio::spawn(async move {
                finish_state
                    .finish_request(FinishRequestParams {
                        id: request_id,
                        winning_attempt: None,
                        status_code: 200,
                        duration_ms: 10,
                        ended_at_ms: 110,
                        observed_service_tier: None,
                        reported_model: None,
                        usage: Some(UsageMetrics {
                            input_tokens: 10,
                            total_tokens: 10,
                            ..UsageMetrics::default()
                        }),
                        retry: None,
                        ttfb_ms: Some(4),
                        streaming: false,
                    })
                    .await
            });

            let durable_terminal = tokio::time::timeout(Duration::from_millis(200), async {
                loop {
                    if store
                        .operator_ledger_revision()
                        .expect("read ledger revision")
                        != initial_revision
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .is_ok();

            finish.abort();
            drop(publication);
            let _ = finish.await;
            assert!(
                !durable_terminal,
                "durable terminal committed before all in-process projections were publishable"
            );
        });
    }

    #[test]
    fn policy_reconcile_does_not_commit_before_snapshot_is_publishable() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let store = state.runtime_store_handle();
            let initial_snapshot = store
                .provider_policy_snapshot()
                .expect("read initial policy snapshot");
            let reconcile_state = Arc::clone(&state);
            let current = state.provider_policy_snapshot.read().await;
            let reconcile = tokio::spawn(async move {
                reconcile_state
                    .reconcile_runtime_upstream_identities(
                        &[RuntimeUpstreamIdentity::new(
                            ProviderEndpointKey::new("codex", "official", "default"),
                            "https://api.openai.com/v1",
                        )],
                        100,
                    )
                    .await
            });

            let durable_policy = tokio::time::timeout(Duration::from_millis(200), async {
                loop {
                    if store
                        .provider_policy_snapshot()
                        .expect("read durable policy snapshot")
                        != initial_snapshot
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .is_ok();

            reconcile.abort();
            drop(current);
            let _ = reconcile.await;
            assert!(
                !durable_policy,
                "durable policy committed before the in-memory snapshot was publishable"
            );
        });
    }

    #[test]
    fn attempt_context_uses_scoped_effective_model_contract_and_fails_closed() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let request_id = state
                .begin_request_for_test()
                .model("gpt-5.6-alias")
                .started_at_ms(100)
                .begin()
                .await;
            let fingerprint = AccountFingerprint::from_digest([11; 32]);
            let route_evidence = |model: &str| AttemptRouteEvidence {
                provider_endpoint_key: Some("codex/official/default".to_string()),
                provider_id: Some("official".to_string()),
                endpoint_id: Some("default".to_string()),
                route_path: vec!["provider:official".to_string()],
                upstream_base_url: Some("https://api.openai.com/v1".to_string()),
                mapped_model: Some(model.to_string()),
            };
            let provider_scope = |endpoint: &str| AttemptProviderScopeCapture {
                endpoint: reqwest::Url::parse(endpoint).expect("provider endpoint"),
                route_scope: "codex/official/default".to_string(),
                account_fingerprint: fingerprint,
            };

            let sol = state
                .capture_upstream_attempt_context(
                    request_id,
                    route_evidence("gpt-5.6-sol"),
                    provider_scope("https://api.openai.com/v1/responses"),
                )
                .await
                .expect("official Sol context");
            let sol_contract = sol.request_contract().expect("official Sol contract");
            assert_eq!(sol_contract.model(), "gpt-5.6-sol");
            assert!(sol_contract.ultra_maps_to_max());
            assert_eq!(sol_contract.scope().account_fingerprint(), fingerprint);

            let luna = state
                .capture_upstream_attempt_context(
                    request_id,
                    route_evidence("gpt-5.6-luna"),
                    provider_scope("https://api.openai.com/v1/responses"),
                )
                .await
                .expect("official Luna context");
            assert!(
                !luna
                    .request_contract()
                    .expect("official Luna contract")
                    .ultra_maps_to_max()
            );

            let compatible = state
                .capture_upstream_attempt_context(
                    request_id,
                    route_evidence("gpt-5.6-sol"),
                    provider_scope("https://relay.example/v1/responses"),
                )
                .await
                .expect("compatible relay context");
            assert!(compatible.request_contract().is_none());
        });
    }

    #[test]
    fn begin_and_finish_requests_keep_trace_id() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let request_id = state
                .begin_request_for_test()
                .model("gpt-5")
                .service_tier("priority")
                .started_at_ms(100)
                .begin()
                .await;

            let active = state.list_active_requests().await;
            let trace_id = active[0].trace_id.clone().expect("active trace ID");
            assert!(crate::logging::is_versioned_request_trace_id(&trace_id));

            state
                .finish_request(FinishRequestParams {
                    id: request_id,
                    winning_attempt: None,
                    status_code: 200,
                    duration_ms: 10,
                    ended_at_ms: 110,
                    observed_service_tier: Some("priority".to_string()),
                    reported_model: None,
                    usage: None,
                    retry: None,
                    ttfb_ms: Some(4),
                    streaming: false,
                })
                .await;

            let recent = state.list_recent_finished(1).await;
            assert_eq!(recent[0].trace_id.as_deref(), Some(trace_id.as_str()));
            assert_eq!(
                recent[0].observability.trace_id.as_deref(),
                Some(trace_id.as_str())
            );
            assert!(recent[0].observability.fast_mode);
            assert_eq!(recent[0].observability.generation_ms, Some(6));
        });
    }

    #[test]
    fn recent_finished_max_defaults_to_one_thousand() {
        assert_eq!(recent_finished_max_from_env(None), 1_000);
    }

    #[test]
    fn finish_request_reports_exactly_once_publication() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let request_id = state
                .begin_request_for_test()
                .started_at_ms(100)
                .begin()
                .await;

            let first = state
                .finish_request(FinishRequestParams {
                    id: request_id,
                    winning_attempt: None,
                    status_code: 200,
                    duration_ms: 10,
                    ended_at_ms: 110,
                    observed_service_tier: None,
                    reported_model: None,
                    usage: None,
                    retry: None,
                    ttfb_ms: Some(4),
                    streaming: false,
                })
                .await;
            let second = state
                .finish_request(FinishRequestParams {
                    id: request_id,
                    winning_attempt: None,
                    status_code: 500,
                    duration_ms: 20,
                    ended_at_ms: 120,
                    observed_service_tier: None,
                    reported_model: None,
                    usage: None,
                    retry: None,
                    ttfb_ms: None,
                    streaming: false,
                })
                .await;

            assert!(first);
            assert!(!second);

            let recent = state.list_recent_finished(10).await;
            assert_eq!(recent.len(), 1);
            assert_eq!(recent[0].id, request_id);
            assert_eq!(recent[0].status_code, 200);
        });
    }

    #[test]
    fn terminal_commit_failure_keeps_request_active_and_skips_projections() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let request_id = state
                .begin_request_for_test()
                .session_id("sid-terminal-failure")
                .started_at_ms(100)
                .begin()
                .await;
            state
                .runtime_store_handle()
                .fail_next_logical_terminal_commit_for_test();

            let published = state
                .finish_request(FinishRequestParams {
                    id: request_id,
                    winning_attempt: None,
                    status_code: 200,
                    duration_ms: 10,
                    ended_at_ms: 110,
                    observed_service_tier: None,
                    reported_model: None,
                    usage: Some(UsageMetrics {
                        input_tokens: 10,
                        total_tokens: 10,
                        ..UsageMetrics::default()
                    }),
                    retry: None,
                    ttfb_ms: Some(4),
                    streaming: false,
                })
                .await;

            assert!(!published, "a failed durable commit must not publish");
            assert_eq!(state.list_active_requests().await.len(), 1);
            assert!(state.list_recent_finished(10).await.is_empty());
            assert!(
                state
                    .request_lifecycle_projection
                    .read()
                    .await
                    .usage_rollups
                    .is_empty()
            );
            assert!(state.list_session_stats("codex").await.is_empty());
        });
    }

    #[test]
    fn operator_capture_waits_for_terminal_publication_to_finish() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let request_id = state
                .begin_request_for_test()
                .session_id("sid-operator-capture")
                .started_at_ms(100)
                .begin()
                .await;
            let (committed, resume) = state
                .pause_next_terminal_publication_after_commit_for_test()
                .await;

            let finishing_state = Arc::clone(&state);
            let finish = tokio::spawn(async move {
                finishing_state
                    .finish_request(FinishRequestParams {
                        id: request_id,
                        winning_attempt: None,
                        status_code: 200,
                        duration_ms: 10,
                        ended_at_ms: 110,
                        observed_service_tier: None,
                        reported_model: None,
                        usage: Some(UsageMetrics {
                            input_tokens: 10,
                            total_tokens: 10,
                            ..UsageMetrics::default()
                        }),
                        retry: None,
                        ttfb_ms: Some(4),
                        streaming: false,
                    })
                    .await
            });
            committed
                .await
                .expect("terminal commit pause must be reached");

            let mut capture = Box::pin(state.capture_operator_lifecycle_snapshot("codex", 200));
            tokio::select! {
                biased;
                _snapshot = &mut capture => panic!("capture crossed a partially published terminal"),
                () = async {} => {}
            }

            resume.send(()).expect("resume terminal publication");
            assert!(finish.await.expect("join terminal publication"));
            let snapshot = capture.await;
            assert!(snapshot.active_requests.is_empty());
            assert_eq!(snapshot.recent_finished.len(), 1);
            assert_eq!(snapshot.usage_rollup_view(12, 1).loaded.requests_total, 1);
            assert!(state.list_active_requests().await.is_empty());
            assert_eq!(state.list_recent_finished(10).await.len(), 1);
            assert_eq!(
                state
                    .get_usage_rollup_view("codex", 12, 1)
                    .await
                    .loaded
                    .requests_total,
                1
            );
        });
    }

    #[tokio::test]
    async fn operator_capture_filters_service_before_limit_and_keeps_full_session_stats() {
        let state = ProxyState::new();
        {
            let mut request_state = state.request_lifecycle_projection.write().await;
            for id in 1..=200 {
                request_state
                    .recent_finished
                    .push_back(finished_request_for_operator_snapshot(
                        id,
                        "claude",
                        "claude-session",
                    ));
            }
            request_state
                .recent_finished
                .push_back(finished_request_for_operator_snapshot(
                    201,
                    "codex",
                    "codex-session",
                ));
            request_state.session_stats.insert(
                "codex".to_string(),
                HashMap::from([(
                    "codex-session".to_string(),
                    SessionStats {
                        turns_total: 999,
                        turns_with_usage: 998,
                        total_usage: UsageMetrics {
                            input_tokens: 50_000,
                            total_tokens: 50_000,
                            ..UsageMetrics::default()
                        },
                        last_seen_ms: 201,
                        ..SessionStats::default()
                    },
                )]),
            );
            request_state.session_stats.insert(
                "claude".to_string(),
                HashMap::from([(
                    "claude-session".to_string(),
                    SessionStats {
                        turns_total: 200,
                        last_seen_ms: 200,
                        ..SessionStats::default()
                    },
                )]),
            );
        }

        let snapshot = state
            .capture_operator_lifecycle_snapshot("codex", 200)
            .await;

        assert_eq!(snapshot.recent_finished.len(), 1);
        assert_eq!(snapshot.recent_finished[0].service, "codex");
        let stats = snapshot
            .session_stats
            .get("codex-session")
            .expect("full session stats");
        assert_eq!(stats.turns_total, 999);
        assert_eq!(stats.total_usage.input_tokens, 50_000);
    }

    #[test]
    fn crash_gate_after_commit_abort_rehydrates_exactly_once() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let _environment = env_lock().await;
            let home = TempStateHome::new("after-commit-abort");
            let mut scoped = ScopedEnv::default();
            unsafe {
                scoped.set_path("CODEX_HELPER_HOME", home.path());
            }
            std::fs::write(
                home.path().join("pricing_overrides.toml"),
                r#"[models.crash-gate-model]
input_per_1m_usd = "1"
output_per_1m_usd = "2"
confidence = "exact"
"#,
            )
            .expect("write crash-gate pricing override");

            let store = Arc::new(
                RuntimeStore::open_in_home(home.path()).expect("open crash-gate runtime store"),
            );
            let state = ProxyState::new_with_runtime_store(Arc::clone(&store))
                .expect("hydrate initial crash-gate state");
            let terminal_at_ms = unix_now_ms();
            let request_id = state
                .begin_request_for_test()
                .session_id("sid-after-commit-abort")
                .model("crash-gate-model")
                .started_at_ms(terminal_at_ms.saturating_sub(10))
                .begin()
                .await;
            let (committed, resume) = state
                .pause_next_terminal_publication_after_commit_for_test()
                .await;

            let finishing_state = Arc::clone(&state);
            let finish = tokio::spawn(async move {
                finishing_state
                    .finish_request(FinishRequestParams {
                        id: request_id,
                        winning_attempt: None,
                        status_code: 200,
                        duration_ms: 10,
                        ended_at_ms: terminal_at_ms,
                        observed_service_tier: None,
                        reported_model: None,
                        usage: Some(UsageMetrics {
                            input_tokens: 1_000_000,
                            total_tokens: 1_000_000,
                            ..UsageMetrics::default()
                        }),
                        retry: None,
                        ttfb_ms: Some(4),
                        streaming: false,
                    })
                    .await
            });
            committed
                .await
                .expect("durable terminal commit pause must be reached");
            finish.abort();
            let aborted = finish
                .await
                .expect_err("terminal publication task is aborted");
            assert!(aborted.is_cancelled());
            drop(resume);

            assert_eq!(state.list_active_requests().await.len(), 1);
            assert!(state.list_recent_finished(10).await.is_empty());
            assert!(
                state
                    .request_lifecycle_projection
                    .read()
                    .await
                    .usage_rollups
                    .is_empty()
            );
            assert!(state.list_session_stats("codex").await.is_empty());
            let committed_revision = store
                .operator_ledger_revision()
                .expect("read post-commit operator revision");
            let committed_page = store
                .query_committed_requests(&CommittedRequestQuery::default())
                .expect("read post-commit ledger");
            assert_eq!(committed_page.items.len(), 1);
            assert_eq!(
                committed_page.items[0]
                    .payload
                    .finished_request
                    .cost
                    .total_cost_usd
                    .as_deref(),
                Some("1")
            );
            drop(state);
            drop(store);

            for expected_recovery_ordinal in [2, 3] {
                let reopened_store = Arc::new(
                    RuntimeStore::open_in_home(home.path())
                        .expect("reopen after committed terminal abort"),
                );
                let recovery = reopened_store.startup_recovery_report();
                assert_eq!(recovery.recovery_ordinal, expected_recovery_ordinal);
                assert_eq!(recovery.interrupted_logical_count, 0);
                assert_eq!(recovery.interrupted_attempt_count, 0);
                assert_eq!(
                    reopened_store
                        .operator_ledger_revision()
                        .expect("read reopened operator revision"),
                    committed_revision
                );
                let ledger = reopened_store
                    .query_committed_requests(&CommittedRequestQuery::default())
                    .expect("read reopened committed ledger");
                assert_eq!(ledger.items.len(), 1);
                assert_eq!(
                    ledger.items[0]
                        .payload
                        .finished_request
                        .cost
                        .total_cost_usd
                        .as_deref(),
                    Some("1")
                );

                let reopened = ProxyState::new_with_runtime_store(Arc::clone(&reopened_store))
                    .expect("rehydrate committed terminal projection");
                let recent = reopened.list_recent_finished(10).await;
                assert_eq!(recent.len(), 1);
                assert_eq!(recent[0].id, request_id);
                assert_eq!(recent[0].cost.total_cost_usd.as_deref(), Some("1"));
                let rollup = reopened.get_usage_rollup_view("codex", 12, 0).await;
                assert_eq!(rollup.loaded.requests_total, 1);
                assert_eq!(rollup.loaded.usage.input_tokens, 1_000_000);
                assert_eq!(rollup.loaded.usage.total_tokens, 1_000_000);
                assert_eq!(rollup.loaded.cost.total_cost_usd.as_deref(), Some("1"));
                assert_eq!(rollup.loaded.cost.priced_requests, 1);
                assert_eq!(rollup.loaded.cost.unpriced_requests, 0);
                let sessions = reopened.list_session_stats("codex").await;
                assert_eq!(sessions.len(), 1);
                assert_eq!(sessions["sid-after-commit-abort"].turns_total, 1);

                drop(reopened);
                drop(reopened_store);
            }
        });
    }

    #[test]
    fn persistent_state_hydrates_committed_projections_across_restart() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let home = TempStateHome::new("projection-hydration");
            let store = Arc::new(
                RuntimeStore::open_in_home(home.path()).expect("open first runtime store"),
            );
            let state = ProxyState::new_with_runtime_store(store.clone())
                .expect("hydrate first runtime projections");
            let base_ms = unix_now_ms();

            let economic_id = state
                .begin_request_for_test()
                .session_id("sid-economic")
                .model("gpt-5")
                .started_at_ms(base_ms)
                .begin()
                .await;
            assert!(
                state
                    .finish_request(FinishRequestParams {
                        id: economic_id,
                        winning_attempt: None,
                        status_code: 200,
                        duration_ms: 10,
                        ended_at_ms: base_ms.saturating_add(10),
                        observed_service_tier: None,
                        reported_model: None,
                        usage: Some(UsageMetrics {
                            input_tokens: 1_000,
                            total_tokens: 1_000,
                            ..UsageMetrics::default()
                        }),
                        retry: None,
                        ttfb_ms: Some(4),
                        streaming: false,
                    })
                    .await
            );

            let non_economic_id = state
                .begin_request_for_test()
                .session_id("sid-non-economic")
                .started_at_ms(base_ms.saturating_add(100))
                .begin()
                .await;
            assert!(
                state
                    .finish_non_economic_request(FinishRequestParams {
                        id: non_economic_id,
                        winning_attempt: None,
                        status_code: 426,
                        duration_ms: 10,
                        ended_at_ms: base_ms.saturating_add(110),
                        observed_service_tier: None,
                        reported_model: None,
                        usage: Some(UsageMetrics {
                            input_tokens: 999,
                            total_tokens: 999,
                            ..UsageMetrics::default()
                        }),
                        retry: None,
                        ttfb_ms: None,
                        streaming: false,
                    })
                    .await
            );
            drop(state);
            drop(store);

            let reopened_store =
                Arc::new(RuntimeStore::open_in_home(home.path()).expect("reopen runtime store"));
            let reopened = ProxyState::new_with_runtime_store(reopened_store)
                .expect("hydrate reopened runtime projections");

            assert_eq!(
                reopened
                    .list_recent_finished(10)
                    .await
                    .iter()
                    .map(|request| request.id)
                    .collect::<Vec<_>>(),
                vec![non_economic_id, economic_id]
            );
            let rollup = reopened.get_usage_rollup_view("codex", 12, 1).await;
            assert_eq!(rollup.loaded.requests_total, 1);
            assert_eq!(rollup.loaded.usage.input_tokens, 1_000);
            let usage_summaries = reopened.operator_usage_summaries("codex", 100).await;
            let provider_summary = usage_summaries
                .iter()
                .find(|summary| summary.group == RequestUsageSummaryGroup::Provider)
                .expect("provider usage summary");
            assert_eq!(provider_summary.rows.len(), 1);
            assert_eq!(provider_summary.rows[0].group_value, "-");
            assert_eq!(provider_summary.rows[0].aggregate.requests, 1);
            assert_eq!(provider_summary.rows[0].aggregate.total_tokens, 1_000);
            assert_eq!(
                reopened.operator_ledger_revision().await,
                reopened
                    .runtime_store()
                    .operator_ledger_revision()
                    .expect("read durable operator ledger revision")
            );
            assert_eq!(
                reopened
                    .request_lifecycle_projection
                    .read()
                    .await
                    .usage_rollups["codex"]
                    .coverage_source,
                "runtime_store"
            );
            let stats = reopened.list_session_stats("codex").await;
            assert_eq!(stats["sid-economic"].turns_total, 1);
            assert!(!stats.contains_key("sid-non-economic"));

            let next_id = reopened
                .begin_request_for_test()
                .started_at_ms(base_ms.saturating_add(200))
                .begin()
                .await;
            assert!(next_id > non_economic_id);
        });
    }

    #[test]
    fn startup_hydration_does_not_resurrect_economic_projections_outside_retention() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
            let state = ProxyState::new_with_runtime_store(Arc::clone(&store))
                .expect("hydrate empty runtime projections");
            let now_ms = unix_now_ms();
            let old_ms = now_ms.saturating_sub(90 * 24 * 60 * 60 * 1_000);

            let old_id = state
                .begin_request_for_test()
                .session_id("sid-expired")
                .started_at_ms(old_ms.saturating_sub(10))
                .begin()
                .await;
            assert!(
                state
                    .finish_request(FinishRequestParams {
                        id: old_id,
                        winning_attempt: None,
                        status_code: 200,
                        duration_ms: 10,
                        ended_at_ms: old_ms,
                        observed_service_tier: None,
                        reported_model: None,
                        usage: Some(UsageMetrics {
                            input_tokens: 100,
                            total_tokens: 100,
                            ..UsageMetrics::default()
                        }),
                        retry: None,
                        ttfb_ms: None,
                        streaming: false,
                    })
                    .await
            );

            let current_id = state
                .begin_request_for_test()
                .session_id("sid-current")
                .started_at_ms(now_ms.saturating_sub(10))
                .begin()
                .await;
            assert!(
                state
                    .finish_request(FinishRequestParams {
                        id: current_id,
                        winning_attempt: None,
                        status_code: 200,
                        duration_ms: 10,
                        ended_at_ms: now_ms,
                        observed_service_tier: None,
                        reported_model: None,
                        usage: Some(UsageMetrics {
                            input_tokens: 200,
                            total_tokens: 200,
                            ..UsageMetrics::default()
                        }),
                        retry: None,
                        ttfb_ms: None,
                        streaming: false,
                    })
                    .await
            );

            state.prune_periodic().await;
            let live_rollup = state.get_usage_rollup_view("codex", 12, 60).await;
            assert_eq!(live_rollup.loaded.requests_total, 1);
            assert_eq!(live_rollup.loaded.usage.input_tokens, 200);
            let live_provider_summary = state
                .operator_usage_summaries("codex", 100)
                .await
                .into_iter()
                .find(|summary| summary.group == RequestUsageSummaryGroup::Provider)
                .expect("live provider usage summary");
            assert_eq!(live_provider_summary.coverage.requests, 1);
            assert_eq!(live_provider_summary.rows.len(), 1);
            assert_eq!(live_provider_summary.rows[0].aggregate.requests, 1);
            assert_eq!(live_provider_summary.rows[0].aggregate.input_tokens, 200);

            let hydrated =
                hydrate_runtime_projections(&store).expect("rehydrate bounded runtime projections");
            assert_eq!(hydrated.committed_terminal_count, 2);
            assert_eq!(hydrated.next_request_id, current_id + 1);
            assert_eq!(hydrated.usage_rollups["codex"].loaded.requests_total, 1);
            assert_eq!(
                hydrated.usage_rollups["codex"].loaded.usage.input_tokens,
                200
            );
            let hydrated_provider_summary = operator_usage_summaries(&hydrated, "codex", 100)
                .into_iter()
                .find(|summary| summary.group == RequestUsageSummaryGroup::Provider)
                .expect("hydrated provider usage summary");
            assert_eq!(hydrated_provider_summary.coverage.requests, 1);
            assert_eq!(hydrated_provider_summary.rows[0].aggregate.requests, 1);
            let hydrated_stats = hydrated
                .session_stats
                .get("codex")
                .expect("hydrated Codex session stats");
            assert!(!hydrated_stats.contains_key("sid-expired"));
            assert!(hydrated_stats.contains_key("sid-current"));
        });
    }

    #[test]
    fn canonical_usage_summaries_use_frozen_buckets_beyond_recent_window_and_restart() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            const REQUEST_COUNT: u64 = 201;

            let store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
            let state = ProxyState::new_with_runtime_store(Arc::clone(&store))
                .expect("hydrate empty runtime projections");
            let now_ms = unix_now_ms();

            for index in 0..REQUEST_COUNT {
                let request_id = state
                    .begin_request_for_test()
                    .session_id("sid-canonical-summary")
                    .model("gpt-5.6-sol")
                    .started_at_ms(now_ms.saturating_add(index))
                    .begin()
                    .await;
                let winner = begin_provider_attempt_for_test(
                    state.as_ref(),
                    request_id,
                    "sol",
                    "responses",
                    "codex/sol/responses",
                    "gpt-5.6-sol",
                    7,
                )
                .await;
                state
                    .finish_upstream_attempt(
                        winner,
                        AttemptOutcome::Succeeded,
                        now_ms.saturating_add(index).saturating_add(1),
                        EconomicsState::Known,
                    )
                    .expect("finish winning attempt");
                assert!(
                    state
                        .finish_request(FinishRequestParams {
                            id: request_id,
                            winning_attempt: Some(winner),
                            status_code: 200,
                            duration_ms: 10,
                            ended_at_ms: now_ms.saturating_add(index).saturating_add(2),
                            observed_service_tier: Some("default".to_string()),
                            reported_model: Some("gpt-5.6-sol".to_string()),
                            usage: Some(UsageMetrics {
                                input_tokens: 1_000,
                                cache_read_input_tokens: 100,
                                cache_creation_input_tokens: 200,
                                total_tokens: 1_000,
                                ..UsageMetrics::default()
                            }),
                            retry: None,
                            ttfb_ms: None,
                            streaming: false,
                        })
                        .await
                );
            }

            let terminal_payload = store
                .read_recent_logical_requests(1)
                .expect("read canonical summary terminal")
                .pop()
                .and_then(|request| request.terminal)
                .and_then(|terminal| terminal.terminal.payload)
                .expect("canonical summary terminal payload");
            assert_eq!(
                terminal_payload.cache_accounting_convention,
                crate::usage::CacheAccountingConvention::INCLUDED_IN_INPUT
            );
            assert!(terminal_payload.provider_price_key.is_some());
            let frozen_buckets = terminal_payload
                .billable_usage
                .expect("frozen billable usage");
            assert_eq!(
                frozen_buckets.status,
                crate::usage::EconomicsStatus::Complete
            );
            assert_eq!(frozen_buckets.ordinary_input_tokens, 700);
            assert_eq!(frozen_buckets.cache_read_input_tokens, 100);
            assert_eq!(frozen_buckets.cache_write_input_tokens, 200);

            assert_eq!(state.list_recent_finished(200).await.len(), 200);
            let assert_summaries = |summaries: Vec<RequestUsageSummary>| {
                let expected = [
                    (
                        RequestUsageSummaryGroup::ProviderEndpoint,
                        "codex/sol/responses",
                    ),
                    (RequestUsageSummaryGroup::Provider, "sol"),
                    (RequestUsageSummaryGroup::Model, "gpt-5.6-sol"),
                    (RequestUsageSummaryGroup::Session, "sid-canonical-summary"),
                ];
                for (group, group_value) in expected {
                    let summary = summaries
                        .iter()
                        .find(|summary| summary.group == group)
                        .expect("canonical usage summary group");
                    assert_eq!(summary.coverage.requests, REQUEST_COUNT);
                    assert_eq!(summary.rows.len(), 1);
                    assert_eq!(summary.rows[0].group_value, group_value);
                    assert_eq!(summary.rows[0].aggregate.requests, REQUEST_COUNT);
                    assert_eq!(
                        summary.rows[0].aggregate.input_tokens,
                        i64::try_from(REQUEST_COUNT).expect("request count") * 700
                    );
                    assert_eq!(
                        summary.rows[0].aggregate.cache_read_input_tokens,
                        i64::try_from(REQUEST_COUNT).expect("request count") * 100
                    );
                    assert_eq!(
                        summary.rows[0].aggregate.cache_creation_input_tokens,
                        i64::try_from(REQUEST_COUNT).expect("request count") * 200
                    );
                }
            };

            assert_summaries(state.operator_usage_summaries("codex", 100).await);
            drop(state);

            let reopened = ProxyState::new_with_runtime_store(store)
                .expect("rehydrate canonical usage summaries");
            assert_summaries(reopened.operator_usage_summaries("codex", 100).await);
        });
    }

    #[test]
    fn session_cards_expose_last_and_average_output_token_speed() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();

            let first_id = state
                .begin_request_for_test()
                .session_id("sid-speed")
                .started_at_ms(100)
                .begin()
                .await;
            state
                .finish_request(FinishRequestParams {
                    id: first_id,
                    winning_attempt: None,
                    status_code: 200,
                    duration_ms: 1_500,
                    ended_at_ms: 1_600,
                    observed_service_tier: None,
                    reported_model: None,
                    usage: Some(UsageMetrics {
                        output_tokens: 200,
                        total_tokens: 200,
                        ..UsageMetrics::default()
                    }),
                    retry: None,
                    ttfb_ms: Some(500),
                    streaming: true,
                })
                .await;

            let second_id = state
                .begin_request_for_test()
                .session_id("sid-speed")
                .started_at_ms(2_000)
                .begin()
                .await;
            state
                .finish_request(FinishRequestParams {
                    id: second_id,
                    winning_attempt: None,
                    status_code: 200,
                    duration_ms: 2_500,
                    ended_at_ms: 4_500,
                    observed_service_tier: None,
                    reported_model: None,
                    usage: Some(UsageMetrics {
                        output_tokens: 300,
                        total_tokens: 300,
                        ..UsageMetrics::default()
                    }),
                    retry: None,
                    ttfb_ms: Some(500),
                    streaming: true,
                })
                .await;

            let stats = state.list_session_stats("codex").await;
            let stats = stats.get("sid-speed").expect("session stats");
            assert_eq!(stats.last_output_tokens_per_second, Some(150.0));
            assert_eq!(stats.avg_output_tokens_per_second, Some(500.0 / 3.0));

            let cards = state.list_session_identity_cards("codex", 16).await;
            let card = cards
                .iter()
                .find(|card| card.session_id.as_deref() == Some("sid-speed"))
                .expect("session card");
            assert_eq!(card.last_output_tokens_per_second, Some(150.0));
            assert_eq!(card.avg_output_tokens_per_second, Some(500.0 / 3.0));
        });
    }

    #[test]
    fn state_change_subscription_tracks_dashboard_mutations() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let mut changes = state.subscribe_state_changes();

            state
                .record_provider_balance_snapshot(ProviderBalanceSnapshot {
                    observation_provider_id: "input6".to_string(),
                    provider_endpoint: ProviderEndpointKey::new("codex", "routing", "default"),
                    fetched_at_ms: 100,
                    stale_after_ms: Some(1_000),
                    status: BalanceSnapshotStatus::Ok,
                    total_balance_usd: Some("12.50".to_string()),
                    ..ProviderBalanceSnapshot::default()
                })
                .await;
            changes.changed().await.expect("balance change");
            let balance_version = *changes.borrow();
            assert!(balance_version > 0);

            state
                .set_provider_automatic_block_for_test(
                    ProviderEndpointKey::new("codex", "input6", "default"),
                    true,
                    100,
                )
                .await;
            changes.changed().await.expect("usage exhausted change");
            let usage_version = *changes.borrow();
            assert!(usage_version > balance_version);

            let request_id = state
                .begin_request_for_test()
                .session_id("sid-1")
                .model("gpt-5")
                .started_at_ms(200)
                .begin()
                .await;
            changes.changed().await.expect("begin request change");
            let active_version = *changes.borrow();
            assert!(active_version > usage_version);

            state
                .finish_request(FinishRequestParams {
                    id: request_id,
                    winning_attempt: None,
                    status_code: 200,
                    duration_ms: 25,
                    ended_at_ms: 250,
                    observed_service_tier: None,
                    reported_model: None,
                    usage: None,
                    retry: None,
                    ttfb_ms: Some(5),
                    streaming: false,
                })
                .await;
            changes.changed().await.expect("finish request change");
            assert!(*changes.borrow() > active_version);
        });
    }

    #[test]
    fn finish_request_keeps_captured_price_across_reload_and_reopen() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let _home = env_lock().await;
            let temp_dir = std::env::temp_dir().join(format!(
                "codex-helper-pricing-capture-test-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&temp_dir).expect("create temp helper home");
            let mut scoped = ScopedEnv::default();
            unsafe {
                scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
            }
            let pricing_path = temp_dir.join("pricing_overrides.toml");
            std::fs::write(
                &pricing_path,
                r#"[models.catalog-race]
input_per_1m_usd = "1"
output_per_1m_usd = "2"
confidence = "exact"
"#,
            )
            .expect("write initial pricing override");

            let state = ProxyState::new_with_runtime_store(Arc::new(
                RuntimeStore::open_in_home(&temp_dir).expect("open persistent runtime store"),
            ))
            .expect("hydrate persistent runtime state");
            let request_id = state
                .begin_request_for_test()
                .model("catalog-race")
                .started_at_ms(100)
                .begin()
                .await;

            std::fs::write(
                &pricing_path,
                r#"[models.catalog-race]
input_per_1m_usd = "9"
output_per_1m_usd = "18"
confidence = "exact"
"#,
            )
            .expect("reload pricing override during request");

            let reloaded_request_id = state
                .begin_request_for_test()
                .model("catalog-race")
                .started_at_ms(101)
                .begin()
                .await;

            let finish_params = |id, ended_at_ms| FinishRequestParams {
                id,
                winning_attempt: None,
                status_code: 200,
                duration_ms: 10,
                ended_at_ms,
                observed_service_tier: None,
                reported_model: None,
                usage: Some(UsageMetrics {
                    input_tokens: 1_000_000,
                    total_tokens: 1_000_000,
                    ..UsageMetrics::default()
                }),
                retry: None,
                ttfb_ms: Some(4),
                streaming: false,
            };

            assert!(state.finish_request(finish_params(request_id, 110)).await);
            assert!(
                state
                    .finish_request(finish_params(reloaded_request_id, 111))
                    .await
            );

            drop(state);

            let reader = crate::runtime_store::RuntimeStoreReader::open_in_home(&temp_dir)
                .expect("reopen runtime store reader");
            {
                let ledger = crate::request_ledger::RequestLedger::new(&reader);
                let request_by_id = |id| {
                    ledger
                        .find_finished_requests(
                            &crate::request_ledger::RequestLogFilters {
                                request_id: Some(id),
                                ..crate::request_ledger::RequestLogFilters::default()
                            },
                            1,
                        )
                        .expect("read reopened request ledger")
                        .pop()
                        .expect("request remains in reopened ledger")
                };
                let captured_request = request_by_id(request_id);
                let reloaded_request = request_by_id(reloaded_request_id);
                assert_eq!(captured_request.cost.total_cost_usd.as_deref(), Some("1"));
                assert_eq!(reloaded_request.cost.total_cost_usd.as_deref(), Some("9"));
                assert!(
                    crate::request_ledger::format_finished_request_lines(&captured_request)[1]
                        .contains("cost=$1 (exact)")
                );
            }
            drop(reader);
            drop(scoped);
            std::fs::remove_dir_all(temp_dir).expect("remove temp helper home");
        });
    }

    #[test]
    fn logical_terminal_uses_winning_attempt_provider_epoch_and_route() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let request_id = state
                .begin_request_for_test()
                .model("gpt-5.6-sol")
                .started_at_ms(100)
                .begin()
                .await;

            let failed = begin_provider_attempt_for_test(
                state.as_ref(),
                request_id,
                "provider-a",
                "endpoint-a",
                "route/provider-a",
                "gpt-5.6-sol",
                1,
            )
            .await;
            state
                .finish_upstream_attempt(
                    failed,
                    AttemptOutcome::Failed,
                    102,
                    EconomicsState::Unknown,
                )
                .expect("finish failed attempt");

            let winner = begin_provider_attempt_for_test(
                state.as_ref(),
                request_id,
                "provider-b",
                "endpoint-b",
                "route/provider-b",
                "gpt-5.6-terra",
                2,
            )
            .await;
            state
                .finish_upstream_attempt(
                    winner,
                    AttemptOutcome::Succeeded,
                    103,
                    EconomicsState::Unknown,
                )
                .expect("finish winning attempt");

            state
                .update_request_route(
                    request_id,
                    RouteDecisionProvenance {
                        effective_model: Some(ResolvedRouteValue::new(
                            "gpt-5.6-luna",
                            RouteValueSource::RuntimeFallback,
                        )),
                        effective_reasoning_effort: Some(ResolvedRouteValue::new(
                            "max",
                            RouteValueSource::RequestPayload,
                        )),
                        provider_id: Some("provider-c".to_string()),
                        endpoint_id: Some("endpoint-c".to_string()),
                        route_path: vec!["provider-c".to_string()],
                        ..RouteDecisionProvenance::default()
                    },
                )
                .await;

            assert!(
                state
                    .finish_request(FinishRequestParams {
                        id: request_id,
                        winning_attempt: Some(winner),
                        status_code: 200,
                        duration_ms: 10,
                        ended_at_ms: 110,
                        observed_service_tier: Some("priority".to_string()),
                        reported_model: Some("gpt-5.6-terra".to_string()),
                        usage: Some(UsageMetrics {
                            input_tokens: 1_000_000,
                            total_tokens: 1_000_000,
                            ..UsageMetrics::default()
                        }),
                        retry: None,
                        ttfb_ms: Some(4),
                        streaming: false,
                    })
                    .await
            );

            let terminal = state
                .runtime_store_handle()
                .read_recent_logical_requests(1)
                .expect("read durable terminal")
                .pop()
                .and_then(|request| request.terminal)
                .and_then(|terminal| terminal.terminal.payload)
                .expect("runtime terminal payload");
            assert_eq!(terminal.winning_attempt_id, Some(winner.id()));
            assert_eq!(terminal.mapped_model.as_deref(), Some("gpt-5.6-terra"));
            assert_eq!(
                terminal.finished_request.reasoning_effort.as_deref(),
                Some("max")
            );
            assert_eq!(
                terminal.finished_request.provider_id.as_deref(),
                Some("provider-b")
            );
            assert_eq!(
                terminal
                    .finished_request
                    .route_decision
                    .as_ref()
                    .and_then(|decision| decision.endpoint_id.as_deref()),
                Some("endpoint-b")
            );
            let epoch = terminal.provider_epoch.as_ref().expect("provider epoch");
            assert_eq!(epoch.scope.route_scope, "route/provider-b");
            assert_eq!(
                epoch.catalog_revision.as_deref(),
                Some(crate::provider_catalog::OPENAI_CODEX_CATALOG_REVISION)
            );
            let price_key = terminal
                .provider_price_key
                .as_ref()
                .expect("provider price key");
            assert_eq!(price_key.epoch, *epoch);
            assert_eq!(price_key.model, "gpt-5.6-terra");
            assert_eq!(price_key.tier, ProviderPricingTier::Priority);
            assert_eq!(
                terminal.finished_request.cost.total_cost_usd.as_deref(),
                Some("5")
            );
            let request_state = state.request_lifecycle_projection.read().await;
            assert!(!request_state.active_requests.contains_key(&request_id));
            assert!(!request_state.attempt_epochs.contains_key(&request_id));
        });
    }

    #[test]
    fn reported_model_conflict_forces_unknown_winner_economics() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let request_id = state
                .begin_request_for_test()
                .model("gpt-5.6-terra")
                .started_at_ms(100)
                .begin()
                .await;
            let winner = begin_provider_attempt_for_test(
                state.as_ref(),
                request_id,
                "provider-b",
                "endpoint-b",
                "route/provider-b",
                "gpt-5.6-terra",
                3,
            )
            .await;
            state
                .finish_upstream_attempt(
                    winner,
                    AttemptOutcome::Succeeded,
                    103,
                    EconomicsState::Unknown,
                )
                .expect("finish winning attempt");

            assert!(
                state
                    .finish_request(FinishRequestParams {
                        id: request_id,
                        winning_attempt: Some(winner),
                        status_code: 200,
                        duration_ms: 10,
                        ended_at_ms: 110,
                        observed_service_tier: Some("standard".to_string()),
                        reported_model: Some("gpt-5.6-sol".to_string()),
                        usage: Some(UsageMetrics {
                            input_tokens: 1_000_000,
                            total_tokens: 1_000_000,
                            ..UsageMetrics::default()
                        }),
                        retry: None,
                        ttfb_ms: Some(4),
                        streaming: false,
                    })
                    .await
            );
            let terminal = state
                .runtime_store_handle()
                .read_recent_logical_requests(1)
                .expect("read durable terminal")
                .pop()
                .and_then(|request| request.terminal)
                .expect("logical terminal");
            assert_eq!(terminal.terminal.economics_state, EconomicsState::Unknown);
            let payload = terminal.terminal.payload.expect("terminal payload");
            assert!(payload.provider_epoch.is_some());
            assert!(payload.provider_price_key.is_none());
            assert!(payload.pricing_model.is_none());
            assert!(payload.finished_request.cost.is_unknown());
        });
    }

    #[test]
    fn provider_pricing_uses_only_actual_service_tier() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let cases = [
                (None, ProviderPricingTier::Unknown, None),
                (Some(""), ProviderPricingTier::Unknown, None),
                (Some("flex"), ProviderPricingTier::Unknown, None),
                (Some("garbage"), ProviderPricingTier::Unknown, None),
                (Some("default"), ProviderPricingTier::Standard, Some("2.5")),
                (Some("standard"), ProviderPricingTier::Standard, Some("2.5")),
                (Some("priority"), ProviderPricingTier::Priority, Some("5")),
            ];

            for (index, (actual_tier, expected_tier, expected_cost)) in
                cases.into_iter().enumerate()
            {
                let request_id = state
                    .begin_request_for_test()
                    .model("gpt-5.6-terra")
                    .service_tier("priority")
                    .started_at_ms(100 + index as u64)
                    .begin()
                    .await;
                let winner = begin_provider_attempt_for_test(
                    state.as_ref(),
                    request_id,
                    "provider-tier",
                    "endpoint-tier",
                    "route/provider-tier",
                    "gpt-5.6-terra",
                    index as u8,
                )
                .await;
                state
                    .finish_upstream_attempt(
                        winner,
                        AttemptOutcome::Succeeded,
                        200 + index as u64,
                        EconomicsState::Unknown,
                    )
                    .expect("finish winning attempt");
                assert!(
                    state
                        .finish_request(FinishRequestParams {
                            id: request_id,
                            winning_attempt: Some(winner),
                            status_code: 200,
                            duration_ms: 10,
                            ended_at_ms: 300 + index as u64,
                            observed_service_tier: actual_tier.map(str::to_string),
                            reported_model: Some("gpt-5.6-terra".to_string()),
                            usage: Some(UsageMetrics {
                                input_tokens: 1_000_000,
                                total_tokens: 1_000_000,
                                ..UsageMetrics::default()
                            }),
                            retry: None,
                            ttfb_ms: Some(4),
                            streaming: false,
                        })
                        .await,
                    "actual tier: {actual_tier:?}"
                );
                let payload = state
                    .runtime_store_handle()
                    .read_recent_logical_requests(1)
                    .expect("read durable terminal")
                    .pop()
                    .and_then(|request| request.terminal)
                    .and_then(|terminal| terminal.terminal.payload)
                    .expect("runtime terminal payload");
                assert_eq!(
                    payload.provider_price_key.as_ref().map(|key| key.tier),
                    Some(expected_tier),
                    "actual tier: {actual_tier:?}"
                );
                assert_eq!(
                    payload.finished_request.cost.total_cost_usd.as_deref(),
                    expected_cost,
                    "actual tier: {actual_tier:?}"
                );
            }
        });
    }

    #[test]
    fn finish_request_without_captured_convention_keeps_cache_economics_unknown() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let request_id = state
                .begin_request_for_test()
                .model("gpt-5")
                .started_at_ms(100)
                .begin()
                .await;

            state
                .finish_request(FinishRequestParams {
                    id: request_id,
                    winning_attempt: None,
                    status_code: 200,
                    duration_ms: 10,
                    ended_at_ms: 110,
                    observed_service_tier: None,
                    reported_model: None,
                    usage: Some(UsageMetrics {
                        input_tokens: 1_000,
                        output_tokens: 500,
                        cached_input_tokens: 100,
                        total_tokens: 1_500,
                        ..UsageMetrics::default()
                    }),
                    retry: None,
                    ttfb_ms: Some(4),
                    streaming: false,
                })
                .await;

            let payload = state
                .runtime_store_handle()
                .read_recent_logical_requests(1)
                .expect("read durable terminal")
                .pop()
                .and_then(|request| request.terminal)
                .and_then(|terminal| terminal.terminal.payload)
                .expect("terminal payload");
            assert_eq!(
                payload.cache_accounting_convention,
                crate::usage::CacheAccountingConvention::UNKNOWN
            );
            assert_eq!(
                payload.billable_usage.map(|usage| usage.status),
                Some(crate::usage::EconomicsStatus::Partial)
            );

            let recent = state.list_recent_finished(1).await;
            assert!(recent[0].cost.is_unknown());

            let rollup = state.get_usage_rollup_view("codex", 12, 1).await;
            assert_eq!(rollup.loaded.cost.total_cost_usd, None);
            assert_eq!(rollup.loaded.cost.priced_requests, 0);
            assert_eq!(rollup.loaded.cost.unpriced_requests, 1);
        });
    }

    #[test]
    fn usage_rollup_records_hour_dimensions_and_dedupes_trace() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let ended_at_ms = 1_704_038_400_000_u64;
            let request_id = state
                .begin_request_for_test()
                .session_id("sid-day")
                .cwd("F:/SourceCodes/Rust/codex-helper")
                .model("gpt-5")
                .started_at_ms(ended_at_ms.saturating_sub(1_000))
                .begin()
                .await;
            state
                .update_request_route(
                    request_id,
                    provider_route_decision(
                        "provider-day",
                        "default",
                        "https://provider.example/v1",
                    ),
                )
                .await;
            state
                .finish_request(FinishRequestParams {
                    id: request_id,
                    winning_attempt: None,
                    status_code: 200,
                    duration_ms: 10,
                    ended_at_ms,
                    observed_service_tier: None,
                    reported_model: None,
                    usage: Some(UsageMetrics {
                        total_tokens: 42,
                        ..UsageMetrics::default()
                    }),
                    retry: None,
                    ttfb_ms: Some(3),
                    streaming: false,
                })
                .await;

            let recent = state.list_recent_finished(1).await;
            let day = usage_day::local_day_from_ms(ended_at_ms);
            let hour = usize::from(usage_day::local_hour_from_ms(ended_at_ms));
            let mut request_state = state.request_lifecycle_projection.write().await;
            let rollup = request_state
                .usage_rollups
                .get_mut("codex")
                .expect("codex rollup");

            assert_eq!(rollup.by_hour[&day][hour].requests_total, 1);
            assert_eq!(rollup.by_model["gpt-5"].requests_total, 1);
            assert_eq!(rollup.by_model_day["gpt-5"][&day].usage.total_tokens, 42);
            assert_eq!(rollup.by_session["sid-day"].requests_total, 1);
            assert_eq!(
                rollup.by_project["F:/SourceCodes/Rust/codex-helper"].requests_total,
                1
            );

            let before = rollup.loaded.requests_total;
            let logical_request_id = rollup
                .recorded_requests
                .keys()
                .next()
                .expect("durable logical request key")
                .clone();
            assert!(!record_finished_request_into_usage_rollup(
                rollup,
                &logical_request_id,
                &recent[0]
            ));
            assert_eq!(rollup.loaded.requests_total, before);
        });
    }

    #[test]
    fn usage_day_view_includes_hour_rows_dimensions_and_cost_sorting() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let window =
                usage_day::local_day_window(usage_day::current_local_day()).expect("window");
            let small_ended_at_ms = window.start_ms.saturating_add(3_600_000);
            let large_ended_at_ms = window.start_ms.saturating_add(7_200_000);

            let small_id = state
                .begin_request_for_test()
                .session_id("sid-small")
                .cwd("F:/small")
                .model("gpt-5")
                .started_at_ms(small_ended_at_ms.saturating_sub(500))
                .begin()
                .await;
            state
                .update_request_route(
                    small_id,
                    provider_route_decision(
                        "provider-small",
                        "default",
                        "https://small.example/v1",
                    ),
                )
                .await;
            state
                .finish_request(FinishRequestParams {
                    id: small_id,
                    winning_attempt: None,
                    status_code: 200,
                    duration_ms: 50,
                    ended_at_ms: small_ended_at_ms,
                    observed_service_tier: None,
                    reported_model: None,
                    usage: Some(UsageMetrics {
                        input_tokens: 100,
                        output_tokens: 10,
                        total_tokens: 110,
                        ..UsageMetrics::default()
                    }),
                    retry: None,
                    ttfb_ms: Some(10),
                    streaming: false,
                })
                .await;

            let large_id = state
                .begin_request_for_test()
                .session_id("sid-large")
                .cwd("F:/large")
                .model("gpt-5")
                .started_at_ms(large_ended_at_ms.saturating_sub(500))
                .begin()
                .await;
            state
                .update_request_route(
                    large_id,
                    provider_route_decision(
                        "provider-large",
                        "default",
                        "https://large.example/v1",
                    ),
                )
                .await;
            state
                .finish_request(FinishRequestParams {
                    id: large_id,
                    winning_attempt: None,
                    status_code: 200,
                    duration_ms: 100,
                    ended_at_ms: large_ended_at_ms,
                    observed_service_tier: None,
                    reported_model: None,
                    usage: Some(UsageMetrics {
                        input_tokens: 1_000,
                        output_tokens: 500,
                        total_tokens: 1_500,
                        ..UsageMetrics::default()
                    }),
                    retry: None,
                    ttfb_ms: Some(20),
                    streaming: false,
                })
                .await;

            let view = state
                .get_usage_day_view("codex", 12, large_ended_at_ms)
                .await;

            assert_eq!(view.hourly.len(), 24);
            assert_eq!(view.summary.requests_total, 2);
            assert_eq!(view.provider_rows[0].name, "provider-large");
            assert_eq!(
                view.provider_endpoint_rows[0].name,
                "codex/provider-large/default"
            );
            assert_eq!(view.session_rows[0].name, "sid-large");
            assert_eq!(view.project_rows[0].name, "F:/large");
            assert_eq!(view.model_rows[0].name, "gpt-5");
            assert_eq!(view.retry_gate.active, 0);
            assert_eq!(view.coverage.source, "runtime_store");
            assert_eq!(view.coverage.loaded_first_ms, Some(small_ended_at_ms));
            assert_eq!(view.coverage.loaded_last_ms, Some(large_ended_at_ms));
            assert_eq!(view.coverage.loaded_requests, 2);
            assert!(view.coverage.day_may_be_partial);
            assert_eq!(
                view.coverage.partial_reason.as_deref(),
                Some("loaded data starts after local day start")
            );
        });
    }

    #[test]
    fn usage_rollup_view_scores_entities_inside_selected_window() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let old_ms = now_ms.saturating_sub(10 * 86_400_000);

            let old_id = state
                .begin_request_for_test()
                .model("gpt-5")
                .started_at_ms(old_ms.saturating_sub(1_000))
                .begin()
                .await;
            state
                .update_request_route(
                    old_id,
                    provider_route_decision("old-provider", "default", "https://old.example/v1"),
                )
                .await;
            state
                .finish_request(FinishRequestParams {
                    id: old_id,
                    winning_attempt: None,
                    status_code: 200,
                    duration_ms: 20,
                    ended_at_ms: old_ms,
                    observed_service_tier: None,
                    reported_model: None,
                    usage: Some(UsageMetrics {
                        total_tokens: 100_000,
                        ..UsageMetrics::default()
                    }),
                    retry: None,
                    ttfb_ms: Some(5),
                    streaming: false,
                })
                .await;

            let fresh_id = state
                .begin_request_for_test()
                .model("gpt-5")
                .started_at_ms(now_ms.saturating_sub(1_000))
                .begin()
                .await;
            state
                .update_request_route(
                    fresh_id,
                    provider_route_decision(
                        "fresh-provider",
                        "default",
                        "https://fresh.example/v1",
                    ),
                )
                .await;
            state
                .finish_request(FinishRequestParams {
                    id: fresh_id,
                    winning_attempt: None,
                    status_code: 200,
                    duration_ms: 10,
                    ended_at_ms: now_ms,
                    observed_service_tier: None,
                    reported_model: None,
                    usage: Some(UsageMetrics {
                        total_tokens: 10,
                        ..UsageMetrics::default()
                    }),
                    retry: None,
                    ttfb_ms: Some(3),
                    streaming: false,
                })
                .await;

            let week = state.get_usage_rollup_view("codex", 10, 7).await;
            assert_eq!(week.loaded.requests_total, 2);
            assert_eq!(week.window.requests_total, 1);
            assert_eq!(week.by_day.len(), 7);
            assert_eq!(
                week.by_provider_endpoint[0].0,
                "codex/fresh-provider/default"
            );
            assert_eq!(week.by_provider[0].0, "fresh-provider");

            let loaded = state.get_usage_rollup_view("codex", 10, 0).await;
            assert_eq!(loaded.window.requests_total, 2);
            assert_eq!(
                loaded.by_provider_endpoint[0].0,
                "codex/old-provider/default"
            );
            assert_eq!(loaded.by_provider[0].0, "old-provider");
        });
    }

    #[test]
    fn build_session_identity_cards_merges_sources_and_sorts_newest_first() {
        let active = vec![ActiveRequest {
            id: 1,
            runtime_revision: 1,
            runtime_digest: "test-runtime".to_string(),
            policy_revision: 0,
            trace_id: Some("codex-1".to_string()),
            session_id: Some("sid-active".to_string()),
            session_identity_source: Some(SessionIdentitySource::Header),
            client_name: Some("Frank-Laptop".to_string()),
            client_addr: Some("100.64.0.8".to_string()),
            cwd: Some("G:/codes/project".to_string()),
            model: Some("gpt-5.4".to_string()),
            requested_model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: Some("priority".to_string()),
            requested_service_tier: Some("priority".to_string()),
            provider_id: Some("right".to_string()),
            route_decision: None,
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            started_at_ms: 500,
        }];
        let recent = vec![
            FinishedRequest {
                id: 2,
                trace_id: Some("codex-2".to_string()),
                session_id: Some("sid-recent".to_string()),
                session_identity_source: Some(SessionIdentitySource::PromptCacheKey),
                client_name: Some("Studio-Mini".to_string()),
                client_addr: Some("100.64.0.9".to_string()),
                cwd: Some("G:/codes/other".to_string()),
                model: Some("gpt-5.3".to_string()),
                reasoning_effort: Some("high".to_string()),
                service_tier: Some("default".to_string()),
                provider_id: Some("vibe".to_string()),
                route_decision: None,
                usage: Some(UsageMetrics {
                    input_tokens: 1,
                    output_tokens: 2,
                    reasoning_tokens: 3,
                    total_tokens: 6,
                    ..UsageMetrics::default()
                }),
                cost: CostBreakdown::default(),
                accounting: RequestAccountingFacts::default(),
                retry: None,
                provider_signals: Vec::new(),
                policy_actions: Vec::new(),
                observability: RequestObservability::default(),
                service: "codex".to_string(),
                method: "POST".to_string(),
                path: "/v1/responses".to_string(),
                status_code: 200,
                duration_ms: 1200,
                ttfb_ms: Some(100),
                streaming: false,
                ended_at_ms: 2_000,
            },
            FinishedRequest {
                id: 3,
                trace_id: Some("codex-3".to_string()),
                session_id: Some("sid-active".to_string()),
                session_identity_source: Some(SessionIdentitySource::Header),
                client_name: Some("Frank-Laptop".to_string()),
                client_addr: Some("100.64.0.8".to_string()),
                cwd: Some("G:/codes/project".to_string()),
                model: Some("gpt-5.4".to_string()),
                reasoning_effort: Some("low".to_string()),
                service_tier: Some("flex".to_string()),
                provider_id: Some("right".to_string()),
                route_decision: None,
                usage: None,
                cost: CostBreakdown::default(),
                accounting: RequestAccountingFacts::default(),
                retry: None,
                provider_signals: Vec::new(),
                policy_actions: Vec::new(),
                observability: RequestObservability::default(),
                service: "codex".to_string(),
                method: "POST".to_string(),
                path: "/v1/responses".to_string(),
                status_code: 429,
                duration_ms: 900,
                ttfb_ms: None,
                streaming: false,
                ended_at_ms: 1_000,
            },
        ];
        let stats = HashMap::from([(
            "sid-active".to_string(),
            SessionStats {
                turns_total: 3,
                last_session_identity_source: Some(SessionIdentitySource::Header),
                last_client_name: Some("Frank-Laptop".to_string()),
                last_client_addr: Some("100.64.0.8".to_string()),
                last_model: Some("gpt-5.4".to_string()),
                last_reasoning_effort: Some("low".to_string()),
                last_service_tier: Some("flex".to_string()),
                last_provider_id: Some("right".to_string()),
                last_route_decision: None,
                last_usage: None,
                total_usage: UsageMetrics {
                    input_tokens: 10,
                    output_tokens: 20,
                    reasoning_tokens: 5,
                    total_tokens: 35,
                    ..UsageMetrics::default()
                },
                turns_with_usage: 2,
                last_output_tokens_per_second: None,
                avg_output_tokens_per_second: None,
                output_generation_ms_total: 0,
                last_status: Some(429),
                last_duration_ms: Some(900),
                last_ended_at_ms: Some(1_000),
                last_seen_ms: 1_000,
            },
        )]);

        let cards = build_session_identity_cards_from_parts(SessionIdentityCardBuildInputs {
            active: &active,
            recent: &recent,
            bindings: &HashMap::new(),
            route_affinities: &HashMap::new(),
            stats: &stats,
        });

        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].session_id.as_deref(), Some("sid-recent"));
        assert_eq!(
            cards[0].session_identity_source,
            Some(SessionIdentitySource::PromptCacheKey)
        );
        assert_eq!(
            cards[0].observation_scope,
            SessionObservationScope::HostLocalEnriched
        );
        assert_eq!(cards[0].last_client_name.as_deref(), Some("Studio-Mini"));
        assert_eq!(cards[0].last_client_addr.as_deref(), Some("100.64.0.9"));
        assert_eq!(cards[1].session_id.as_deref(), Some("sid-active"));
        assert_eq!(
            cards[1].session_identity_source,
            Some(SessionIdentitySource::Header)
        );
        assert_eq!(
            cards[1].observation_scope,
            SessionObservationScope::HostLocalEnriched
        );
        assert_eq!(cards[1].active_count, 1);
        assert_eq!(cards[1].last_client_name.as_deref(), Some("Frank-Laptop"));
        assert_eq!(cards[1].last_client_addr.as_deref(), Some("100.64.0.8"));
        assert_eq!(cards[1].last_status, Some(429));
        assert_eq!(
            cards[1]
                .effective_model
                .as_ref()
                .map(|value| value.value.as_str()),
            Some("gpt-5.4")
        );
        assert_eq!(
            cards[1].effective_model.as_ref().map(|value| value.source),
            Some(RouteValueSource::RequestPayload)
        );
        assert_eq!(
            cards[1]
                .effective_reasoning_effort
                .as_ref()
                .map(|value| value.source),
            Some(RouteValueSource::RequestPayload)
        );
        assert_eq!(
            cards[1]
                .effective_service_tier
                .as_ref()
                .map(|value| value.source),
            Some(RouteValueSource::RequestPayload)
        );
        assert_eq!(cards[1].turns_total, Some(3));
        assert_eq!(cards[1].last_service_tier.as_deref(), Some("flex"));
        assert_eq!(
            cards[1].total_usage.as_ref().map(|u| u.total_tokens),
            Some(35)
        );
    }

    #[test]
    fn build_session_identity_cards_default_profile_keeps_request_fields() {
        let active = vec![ActiveRequest {
            id: 1,
            runtime_revision: 1,
            runtime_digest: "test-runtime".to_string(),
            policy_revision: 0,
            trace_id: Some("codex-1".to_string()),
            session_id: Some("sid-bound".to_string()),
            session_identity_source: Some(SessionIdentitySource::Header),
            client_name: Some("Workstation".to_string()),
            client_addr: Some("100.64.0.10".to_string()),
            cwd: None,
            model: Some("gpt-observed".to_string()),
            requested_model: Some("gpt-observed".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: Some("default".to_string()),
            requested_service_tier: Some("default".to_string()),
            provider_id: None,
            route_decision: None,
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            started_at_ms: 10,
        }];
        let bindings = HashMap::from([(
            "sid-bound".to_string(),
            SessionBinding {
                session_id: "sid-bound".to_string(),
                profile_name: Some("daily".to_string()),
                model: Some("gpt-bound".to_string()),
                reasoning_effort: Some("high".to_string()),
                service_tier: Some("priority".to_string()),
                continuity_mode: SessionContinuityMode::DefaultProfile,
                created_at_ms: 1,
                updated_at_ms: 1,
                last_seen_ms: 10,
            },
        )]);

        let cards = build_session_identity_cards_from_parts(SessionIdentityCardBuildInputs {
            active: &active,
            recent: &[],
            bindings: &bindings,
            route_affinities: &HashMap::new(),
            stats: &HashMap::new(),
        });

        assert_eq!(cards[0].binding_profile_name.as_deref(), Some("daily"));
        assert_eq!(
            cards[0].observation_scope,
            SessionObservationScope::ObservedOnly
        );
        assert_eq!(
            cards[0].binding_continuity_mode,
            Some(SessionContinuityMode::DefaultProfile)
        );
        assert_eq!(
            cards[0]
                .effective_model
                .as_ref()
                .map(|value| (value.value.as_str(), value.source)),
            Some(("gpt-observed", RouteValueSource::RequestPayload))
        );
        assert_eq!(
            cards[0]
                .effective_reasoning_effort
                .as_ref()
                .map(|value| (value.value.as_str(), value.source)),
            Some(("medium", RouteValueSource::RequestPayload))
        );
        assert_eq!(
            cards[0]
                .effective_service_tier
                .as_ref()
                .map(|value| (value.value.as_str(), value.source)),
            Some(("default", RouteValueSource::RequestPayload))
        );
    }

    #[test]
    fn provider_balance_snapshots_are_keyed_by_canonical_endpoint_identity() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let provider_endpoint = ProviderEndpointKey::new("codex", "right", "responses");
            let mut snapshot = ProviderBalanceSnapshot::new(
                "packycode",
                provider_endpoint.clone(),
                "usage_provider:budget_http_json",
                10,
                Some(0),
            );
            snapshot.exhausted = Some(false);
            snapshot.total_balance_usd = Some("3.5".to_string());
            snapshot.monthly_budget_usd = Some("5".to_string());
            snapshot.monthly_spent_usd = Some("1.5".to_string());
            state.record_provider_balance_snapshot(snapshot).await;

            let view = state.get_provider_balance_view("codex").await;
            assert_eq!(view.len(), 1);
            assert_eq!(view[0].observation_provider_id, "packycode");
            assert_eq!(view[0].provider_endpoint, provider_endpoint);
            assert_eq!(view[0].status, BalanceSnapshotStatus::Stale);
            assert_eq!(view[0].exhausted, Some(false));

            let summary = ProviderRoutingBalanceSummary::from_snapshots(Some(&view));
            assert_eq!(summary.snapshots, 1);
            assert_eq!(summary.stale, 1);
            assert_eq!(summary.routing_snapshots, 1);
        });
    }

    #[test]
    fn provider_balance_snapshots_keep_multiple_providers_per_upstream() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            for (provider_id, status) in [
                ("general", BalanceSnapshotStatus::Error),
                ("newapi", BalanceSnapshotStatus::Ok),
            ] {
                state
                    .record_provider_balance_snapshot(ProviderBalanceSnapshot {
                        observation_provider_id: provider_id.to_string(),
                        provider_endpoint: ProviderEndpointKey::new("codex", "routing", "default"),
                        source: "usage_provider:test".to_string(),
                        fetched_at_ms: 10,
                        stale_after_ms: None,
                        stale: false,
                        status,
                        exhausted: if status == BalanceSnapshotStatus::Ok {
                            Some(false)
                        } else {
                            None
                        },
                        exhaustion_affects_routing: true,
                        plan_name: None,
                        total_balance_usd: if status == BalanceSnapshotStatus::Ok {
                            Some("3.5".to_string())
                        } else {
                            None
                        },
                        subscription_balance_usd: None,
                        paygo_balance_usd: None,
                        monthly_budget_usd: None,
                        monthly_spent_usd: None,
                        quota_period: None,
                        quota_remaining_usd: None,
                        quota_limit_usd: None,
                        quota_used_usd: None,
                        unlimited_quota: None,
                        total_used_usd: None,
                        today_used_usd: None,
                        total_requests: None,
                        today_requests: None,
                        total_tokens: None,
                        today_tokens: None,
                        error: if status == BalanceSnapshotStatus::Error {
                            Some("decode failed".to_string())
                        } else {
                            None
                        },
                        ..ProviderBalanceSnapshot::default()
                    })
                    .await;
            }

            let view = state.get_provider_balance_view("codex").await;
            assert_eq!(view.len(), 2);
            assert_eq!(
                view.iter()
                    .map(|snapshot| snapshot.observation_provider_id.as_str())
                    .collect::<Vec<_>>(),
                vec!["general", "newapi"]
            );
        });
    }

    #[test]
    fn provider_balance_error_refresh_preserves_previous_amounts() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let fetched_at_ms = unix_now_ms();
            state
                .record_provider_balance_snapshot(ProviderBalanceSnapshot {
                    observation_provider_id: "input".to_string(),
                    provider_endpoint: ProviderEndpointKey::new("codex", "routing", "default"),
                    source: "usage_provider:sub2api_usage".to_string(),
                    fetched_at_ms,
                    stale_after_ms: None,
                    exhausted: Some(false),
                    quota_period: Some("daily".to_string()),
                    quota_remaining_usd: Some("263.68".to_string()),
                    quota_limit_usd: Some("300.00".to_string()),
                    ..ProviderBalanceSnapshot::default()
                })
                .await;

            state
                .record_provider_balance_snapshot(ProviderBalanceSnapshot {
                    observation_provider_id: "input".to_string(),
                    provider_endpoint: ProviderEndpointKey::new("codex", "routing", "default"),
                    source: "usage_provider:openai_balance_http_json".to_string(),
                    fetched_at_ms: fetched_at_ms.saturating_add(1),
                    stale_after_ms: None,
                    error: Some("usage provider response read failed".to_string()),
                    ..ProviderBalanceSnapshot::default()
                })
                .await;

            let view = state.get_provider_balance_view("codex").await;
            let snapshot = view
                .iter()
                .find(|snapshot| {
                    snapshot.provider_endpoint
                        == ProviderEndpointKey::new("codex", "routing", "default")
                        && snapshot.observation_provider_id == "input"
                })
                .expect("routing balance snapshot");

            assert_eq!(snapshot.status, BalanceSnapshotStatus::Error);
            assert_eq!(snapshot.quota_period.as_deref(), Some("daily"));
            assert_eq!(snapshot.quota_remaining_usd.as_deref(), Some("263.68"));
            assert_eq!(snapshot.quota_limit_usd.as_deref(), Some("300.00"));
            assert_eq!(
                snapshot.error.as_deref(),
                Some("usage provider response read failed")
            );
        });
    }

    #[test]
    fn prune_runtime_observability_keeps_catalog_provider_balances_on_routing_layout_change() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();

            let initial_view = route_view(&[
                ("input", "https://input.example/v1"),
                ("backup", "https://backup.example/v1"),
            ]);
            state
                .prune_runtime_observability_for_service("codex", &initial_view)
                .await;

            state
                .record_provider_balance_snapshot(ProviderBalanceSnapshot {
                    observation_provider_id: "balance".to_string(),
                    provider_endpoint: ProviderEndpointKey::new("codex", "input", "default"),
                    source: "usage_provider:test".to_string(),
                    fetched_at_ms: 10,
                    stale_after_ms: None,
                    stale: false,
                    status: BalanceSnapshotStatus::Ok,
                    exhausted: Some(false),
                    exhaustion_affects_routing: true,
                    total_balance_usd: Some("3.5".to_string()),
                    ..ProviderBalanceSnapshot::default()
                })
                .await;
            state
                .record_provider_balance_snapshot(ProviderBalanceSnapshot {
                    observation_provider_id: "balance".to_string(),
                    provider_endpoint: ProviderEndpointKey::new("codex", "routing", "default"),
                    source: "usage_provider:test".to_string(),
                    fetched_at_ms: 10,
                    stale_after_ms: None,
                    stale: false,
                    status: BalanceSnapshotStatus::Ok,
                    exhausted: Some(false),
                    exhaustion_affects_routing: true,
                    total_balance_usd: Some("3.5".to_string()),
                    ..ProviderBalanceSnapshot::default()
                })
                .await;

            let pinned_view = route_view(&[("input", "https://input.example/v1")]);
            state
                .prune_runtime_observability_for_service("codex", &pinned_view)
                .await;

            let view = state.get_provider_balance_view("codex").await;
            assert!(view.iter().any(|snapshot| {
                snapshot.provider_endpoint == ProviderEndpointKey::new("codex", "input", "default")
            }));
            assert!(!view.iter().any(|snapshot| {
                snapshot.provider_endpoint
                    == ProviderEndpointKey::new("codex", "routing", "default")
            }));
        });
    }

    #[test]
    fn prune_runtime_observability_keeps_active_provider_endpoint_usage() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let request_id = state
                .begin_request_for_test()
                .model("gpt-5")
                .started_at_ms(30)
                .begin()
                .await;
            state
                .update_request_route(
                    request_id,
                    provider_route_decision(
                        "provider-active",
                        "default",
                        "https://active.example/v1",
                    ),
                )
                .await;
            state
                .finish_request(FinishRequestParams {
                    id: request_id,
                    winning_attempt: None,
                    status_code: 200,
                    duration_ms: 5,
                    ended_at_ms: 35,
                    observed_service_tier: None,
                    reported_model: None,
                    usage: None,
                    retry: None,
                    ttfb_ms: None,
                    streaming: false,
                })
                .await;

            let view = route_view(&[("provider-active", "https://active.example/v1")]);
            state
                .prune_runtime_observability_for_service("codex", &view)
                .await;

            let rollup = state.get_usage_rollup_view("codex", 10, 0).await;
            assert_eq!(
                rollup
                    .by_provider_endpoint
                    .first()
                    .map(|(key, _)| key.as_str()),
                Some("codex/provider-active/default")
            );
        });
    }

    #[test]
    fn prune_runtime_observability_removes_stale_service_keys() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();

            state
                .record_provider_balance_snapshot(ProviderBalanceSnapshot {
                    observation_provider_id: "budget".to_string(),
                    provider_endpoint: ProviderEndpointKey::new("codex", "provider-old", "default"),
                    source: "usage_provider:budget_http_json".to_string(),
                    fetched_at_ms: 10,
                    stale_after_ms: None,
                    stale: false,
                    status: BalanceSnapshotStatus::Ok,
                    exhausted: Some(false),
                    exhaustion_affects_routing: true,
                    plan_name: None,
                    total_balance_usd: Some("3.5".to_string()),
                    subscription_balance_usd: None,
                    paygo_balance_usd: None,
                    monthly_budget_usd: None,
                    monthly_spent_usd: None,
                    quota_period: None,
                    quota_remaining_usd: None,
                    quota_limit_usd: None,
                    quota_used_usd: None,
                    unlimited_quota: None,
                    total_used_usd: None,
                    today_used_usd: None,
                    total_requests: None,
                    today_requests: None,
                    total_tokens: None,
                    today_tokens: None,
                    error: None,
                    ..ProviderBalanceSnapshot::default()
                })
                .await;

            let request_id = state
                .begin_request_for_test()
                .session_id("sid-old")
                .model("gpt-5")
                .started_at_ms(30)
                .begin()
                .await;
            state
                .update_request_route(
                    request_id,
                    provider_route_decision("provider-old", "default", "https://old.example/v1"),
                )
                .await;
            state
                .finish_request(FinishRequestParams {
                    id: request_id,
                    winning_attempt: None,
                    status_code: 200,
                    duration_ms: 5,
                    ended_at_ms: 35,
                    observed_service_tier: None,
                    reported_model: None,
                    usage: None,
                    retry: None,
                    ttfb_ms: None,
                    streaming: false,
                })
                .await;

            let view = route_view(&[("provider-new", "https://new.example/v1")]);

            state
                .prune_runtime_observability_for_service("codex", &view)
                .await;

            assert!(state.get_provider_balance_view("codex").await.is_empty());

            let rollup = state.get_usage_rollup_view("codex", 10, 10).await;
            assert!(rollup.by_provider_endpoint.is_empty());
            assert!(rollup.by_provider.is_empty());
        });
    }

    #[test]
    fn provider_runtime_health_projects_endpoint_state_and_policy() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let monthly = ProviderEndpointKey::new("codex", "monthly", "default");
            let fallback = ProviderEndpointKey::new("codex", "fallback", "default");

            state
                .record_provider_endpoint_attempt_success("codex", fallback.clone(), 10)
                .await;
            state
                .set_provider_automatic_block_for_test(monthly.clone(), true, 10)
                .await;
            state
                .record_provider_endpoint_attempt_failure(
                    "codex",
                    monthly.clone(),
                    0,
                    crate::endpoint_health::CooldownBackoff {
                        factor: 1,
                        max_secs: 0,
                    },
                )
                .await;
            state
                .record_provider_endpoint_attempt_failure(
                    "codex",
                    monthly.clone(),
                    0,
                    crate::endpoint_health::CooldownBackoff {
                        factor: 1,
                        max_secs: 0,
                    },
                )
                .await;
            state
                .record_provider_endpoint_attempt_failure(
                    "codex",
                    monthly.clone(),
                    30,
                    crate::endpoint_health::CooldownBackoff {
                        factor: 1,
                        max_secs: 0,
                    },
                )
                .await;

            let runtime = state
                .route_plan_runtime_state_for_provider_endpoints("codex")
                .await;
            let monthly_state = runtime.provider_endpoint(&monthly);
            assert_eq!(
                monthly_state.failure_count,
                crate::endpoint_health::FAILURE_THRESHOLD
            );
            assert!(monthly_state.cooldown_active);
            assert!(monthly_state.usage_exhausted);
            assert_eq!(runtime.affinity_provider_endpoint(), Some(&fallback));

            state
                .set_provider_manual_eligibility(
                    fallback.clone(),
                    ProviderManualEligibility::Disabled,
                    Some("operator disabled endpoint".to_string()),
                    20,
                )
                .await
                .expect("commit manual eligibility");
            let runtime = state
                .route_plan_runtime_state_for_provider_endpoints("codex")
                .await;
            assert!(runtime.provider_endpoint(&fallback).runtime_disabled);
        });
    }

    #[test]
    fn credential_rotation_ignores_late_passive_health_writes_from_old_identity() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "relay", "default");
            let identity_a = RuntimeUpstreamIdentity::new_with_credential_scope(
                endpoint.clone(),
                "https://relay.example/v1",
                None,
                Some("credential-a".to_string()),
            );
            let identity_b = RuntimeUpstreamIdentity::new_with_credential_scope(
                endpoint.clone(),
                "https://relay.example/v1",
                None,
                Some("credential-b".to_string()),
            );
            let policy = state.capture_provider_policy_snapshot().await;
            let cooldown_backoff = CooldownBackoff {
                factor: 1,
                max_secs: 0,
            };

            state
                .reconcile_runtime_upstream_identities(std::slice::from_ref(&identity_a), 1)
                .await
                .expect("publish credential A runtime identity");
            state
                .route_plan_runtime_state_with_provider_policy(
                    "codex",
                    policy.as_ref(),
                    1,
                    std::slice::from_ref(&identity_a),
                )
                .await;
            state
                .penalize_runtime_upstream_attempt("codex", &identity_a, 30, cooldown_backoff)
                .await;

            state
                .reconcile_runtime_upstream_identities(std::slice::from_ref(&identity_b), 2)
                .await
                .expect("publish credential B runtime identity");
            let rotated = state
                .route_plan_runtime_state_with_provider_policy(
                    "codex",
                    policy.as_ref(),
                    2,
                    std::slice::from_ref(&identity_b),
                )
                .await;
            assert!(!rotated.provider_endpoint(&endpoint).cooldown_active);

            state
                .penalize_runtime_upstream_attempt("codex", &identity_a, 30, cooldown_backoff)
                .await;
            state
                .penalize_runtime_upstream_attempt("codex", &identity_b, 30, cooldown_backoff)
                .await;

            state
                .route_plan_runtime_state_with_provider_policy(
                    "codex",
                    policy.as_ref(),
                    1,
                    std::slice::from_ref(&identity_a),
                )
                .await;
            state
                .record_runtime_upstream_attempt_success("codex", &identity_a, 10)
                .await;
            state
                .record_runtime_upstream_attempt_failure("codex", &identity_a, 30, cooldown_backoff)
                .await;

            let current = state
                .route_plan_runtime_state_with_provider_policy(
                    "codex",
                    policy.as_ref(),
                    2,
                    std::slice::from_ref(&identity_b),
                )
                .await;
            let projected = current.provider_endpoint(&endpoint);
            assert_eq!(projected.failure_count, FAILURE_THRESHOLD);
            assert!(projected.cooldown_active);

            let guard = state.provider_endpoint_runtime_health.read().await;
            let per_service = guard.get("codex").expect("codex passive health state");
            let identity_b_key =
                ProviderEndpointRuntimeHealthKey::for_service("codex", &identity_b)
                    .expect("credential B runtime health key");
            assert_eq!(per_service.active_revision, Some(2));
            assert_eq!(per_service.active_identities.len(), 1);
            assert!(per_service.active_identities.contains(&identity_b_key));
            assert_eq!(per_service.health.len(), 1);
            assert!(
                per_service
                    .health
                    .contains_key(&ProviderEndpointRuntimeHealthBucketKey::new(
                        identity_b_key,
                        RuntimeHealthDomain::Capability(RouteCapability::Inference),
                    ))
            );
        });
    }

    #[test]
    fn runtime_health_is_scoped_to_the_requested_capability() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "relay", "default");
            let identity =
                RuntimeUpstreamIdentity::new(endpoint.clone(), "https://relay.example/v1");
            let policy = state.capture_provider_policy_snapshot().await;
            let cooldown_backoff = CooldownBackoff {
                factor: 1,
                max_secs: 0,
            };

            state
                .reconcile_runtime_upstream_identities(std::slice::from_ref(&identity), 1)
                .await
                .expect("publish runtime identity");
            state
                .penalize_runtime_upstream_attempt_for_domain(
                    "codex",
                    &identity,
                    RuntimeHealthDomain::Capability(RouteCapability::HostedImageGeneration),
                    30,
                    cooldown_backoff,
                )
                .await;

            let inference = state
                .route_plan_runtime_state_with_provider_policy_for_capability(
                    "codex",
                    policy.as_ref(),
                    1,
                    std::slice::from_ref(&identity),
                    Some(RouteCapability::Inference),
                )
                .await;
            assert!(!inference.provider_endpoint(&endpoint).cooldown_active);

            let image = state
                .route_plan_runtime_state_with_provider_policy_for_capability(
                    "codex",
                    policy.as_ref(),
                    1,
                    std::slice::from_ref(&identity),
                    Some(RouteCapability::HostedImageGeneration),
                )
                .await;
            assert!(image.provider_endpoint(&endpoint).cooldown_active);

            let request_local = state
                .route_plan_runtime_state_with_provider_policy_for_capability(
                    "codex",
                    policy.as_ref(),
                    1,
                    std::slice::from_ref(&identity),
                    None,
                )
                .await;
            assert!(!request_local.provider_endpoint(&endpoint).cooldown_active);
        });
    }

    #[test]
    fn half_open_probe_is_singleflight_and_once_per_breaker_epoch() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "relay", "default");
            let identity =
                RuntimeUpstreamIdentity::new(endpoint.clone(), "https://relay.example/v1");
            let cooldown_backoff = CooldownBackoff {
                factor: 1,
                max_secs: 0,
            };
            state
                .reconcile_runtime_upstream_identities(std::slice::from_ref(&identity), 1)
                .await
                .expect("publish runtime identity");
            state
                .penalize_runtime_upstream_attempt_for_domain(
                    "codex",
                    &identity,
                    RuntimeHealthDomain::Capability(RouteCapability::Inference),
                    30,
                    cooldown_backoff,
                )
                .await;

            let abandoned = state
                .try_acquire_runtime_half_open_probe("codex", &identity, RouteCapability::Inference)
                .await
                .expect("pre-dispatch half-open owner");
            drop(abandoned);
            let replacement = state
                .try_acquire_runtime_half_open_probe("codex", &identity, RouteCapability::Inference)
                .await
                .expect("an undispatched probe must release ownership on drop");
            drop(replacement);

            let start = Arc::new(tokio::sync::Barrier::new(17));
            let mut tasks = tokio::task::JoinSet::new();
            for _ in 0..16 {
                let state = Arc::clone(&state);
                let identity = identity.clone();
                let start = Arc::clone(&start);
                tasks.spawn(async move {
                    start.wait().await;
                    let Some(probe) = state
                        .try_acquire_runtime_half_open_probe(
                            "codex",
                            &identity,
                            RouteCapability::Inference,
                        )
                        .await
                    else {
                        return 0;
                    };
                    assert!(state.dispatch_runtime_half_open_probe(probe).await.is_ok());
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    1
                });
            }
            start.wait().await;

            let mut acquired = 0;
            while let Some(result) = tasks.join_next().await {
                acquired += result.expect("half-open task");
            }
            assert_eq!(acquired, 1);
            assert!(
                state
                    .try_acquire_runtime_half_open_probe(
                        "codex",
                        &identity,
                        RouteCapability::Inference,
                    )
                    .await
                    .is_none(),
                "a dispatched probe must not repeat within the same breaker epoch"
            );

            state
                .record_runtime_upstream_attempt_success_for_capability(
                    "codex",
                    &identity,
                    RouteCapability::Inference,
                    10,
                )
                .await;
            state
                .penalize_runtime_upstream_attempt_for_domain(
                    "codex",
                    &identity,
                    RuntimeHealthDomain::Capability(RouteCapability::Inference),
                    30,
                    cooldown_backoff,
                )
                .await;
            assert!(
                state
                    .try_acquire_runtime_half_open_probe(
                        "codex",
                        &identity,
                        RouteCapability::Inference,
                    )
                    .await
                    .is_some(),
                "a recovered endpoint may probe again after a new breaker epoch opens"
            );
        });
    }

    #[test]
    fn stale_half_open_success_does_not_clear_a_new_breaker_epoch() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "relay", "default");
            let identity =
                RuntimeUpstreamIdentity::new(endpoint.clone(), "https://relay.example/v1");
            let cooldown_backoff = CooldownBackoff {
                factor: 1,
                max_secs: 0,
            };
            state
                .reconcile_runtime_upstream_identities(std::slice::from_ref(&identity), 1)
                .await
                .expect("publish runtime identity");
            state
                .penalize_runtime_upstream_attempt_for_domain(
                    "codex",
                    &identity,
                    RuntimeHealthDomain::Capability(RouteCapability::Inference),
                    30,
                    cooldown_backoff,
                )
                .await;
            let acquired = state
                .try_acquire_runtime_half_open_probe("codex", &identity, RouteCapability::Inference)
                .await
                .expect("acquire old half-open probe");
            let dispatched = state
                .dispatch_runtime_half_open_probe(acquired)
                .await
                .expect("dispatch old half-open probe");
            assert!(
                state
                    .validate_dispatched_runtime_half_open_probe(&dispatched)
                    .await,
                "the current dispatched probe must validate before a newer breaker epoch opens"
            );

            state
                .record_runtime_upstream_attempt_success_for_capability(
                    "codex",
                    &identity,
                    RouteCapability::Inference,
                    10,
                )
                .await;
            state
                .penalize_runtime_upstream_attempt_for_domain(
                    "codex",
                    &identity,
                    RuntimeHealthDomain::Capability(RouteCapability::Inference),
                    30,
                    cooldown_backoff,
                )
                .await;

            assert!(
                !state
                    .validate_dispatched_runtime_half_open_probe(&dispatched)
                    .await,
                "an old dispatched probe must not be allowed to settle a newer breaker epoch"
            );

            assert_eq!(
                state
                    .settle_runtime_half_open_probe(
                        dispatched,
                        RuntimeHealthHalfOpenTerminal::Success { now_ms: 20 },
                    )
                    .await,
                RuntimeHealthHalfOpenSettlement::Stale
            );
            let projected = state
                .route_plan_runtime_state_for_provider_endpoints("codex")
                .await
                .provider_endpoint(&endpoint);
            assert!(projected.cooldown_active);
            assert_eq!(projected.failure_count, FAILURE_THRESHOLD);
        });
    }

    #[test]
    fn half_open_success_only_clears_its_leased_transient_buckets() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "relay", "default");
            let identity = RuntimeUpstreamIdentity::new(endpoint, "https://relay.example/v1");
            let cooldown_backoff = CooldownBackoff {
                factor: 1,
                max_secs: 0,
            };
            state
                .reconcile_runtime_upstream_identities(std::slice::from_ref(&identity), 1)
                .await
                .expect("publish runtime identity");
            state
                .penalize_runtime_upstream_attempt_for_domain(
                    "codex",
                    &identity,
                    RuntimeHealthDomain::Capability(RouteCapability::Inference),
                    30,
                    cooldown_backoff,
                )
                .await;
            let acquired = state
                .try_acquire_runtime_half_open_probe("codex", &identity, RouteCapability::Inference)
                .await
                .expect("acquire half-open probe");
            let dispatched = state
                .dispatch_runtime_half_open_probe(acquired)
                .await
                .expect("dispatch half-open probe");

            for domain in [
                RuntimeHealthDomain::Credential,
                RuntimeHealthDomain::Capacity(RouteCapability::Inference),
            ] {
                state
                    .penalize_runtime_upstream_attempt_for_domain(
                        "codex",
                        &identity,
                        domain,
                        30,
                        cooldown_backoff,
                    )
                    .await;
            }
            assert_eq!(
                state
                    .settle_runtime_half_open_probe(
                        dispatched,
                        RuntimeHealthHalfOpenTerminal::Success { now_ms: 20 },
                    )
                    .await,
                RuntimeHealthHalfOpenSettlement::Applied
            );

            let identity_key = ProviderEndpointRuntimeHealthKey::for_service("codex", &identity)
                .expect("runtime health identity");
            let guard = state.provider_endpoint_runtime_health.read().await;
            let service = guard.get("codex").expect("codex health state");
            let capability = service
                .health
                .get(&ProviderEndpointRuntimeHealthBucketKey::new(
                    identity_key.clone(),
                    RuntimeHealthDomain::Capability(RouteCapability::Inference),
                ))
                .expect("capability health");
            assert_eq!(capability.failure_count, 0);
            assert!(capability.cooldown_until.is_none());
            for domain in [
                RuntimeHealthDomain::Credential,
                RuntimeHealthDomain::Capacity(RouteCapability::Inference),
            ] {
                let health = service
                    .health
                    .get(&ProviderEndpointRuntimeHealthBucketKey::new(
                        identity_key.clone(),
                        domain,
                    ))
                    .expect("new health domain");
                assert_eq!(health.failure_count, FAILURE_THRESHOLD);
                assert!(health.cooldown_until.is_some());
            }
        });
    }

    #[test]
    fn half_open_probe_rejects_credential_and_capacity_domains() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let cooldown_backoff = CooldownBackoff {
                factor: 1,
                max_secs: 0,
            };
            for (case, domains) in [
                ("credential_only", vec![RuntimeHealthDomain::Credential]),
                (
                    "capacity_only",
                    vec![RuntimeHealthDomain::Capacity(RouteCapability::Inference)],
                ),
                (
                    "transport_plus_credential",
                    vec![
                        RuntimeHealthDomain::EndpointTransport,
                        RuntimeHealthDomain::Credential,
                    ],
                ),
                (
                    "capability_plus_capacity",
                    vec![
                        RuntimeHealthDomain::Capability(RouteCapability::Inference),
                        RuntimeHealthDomain::Capacity(RouteCapability::Inference),
                    ],
                ),
            ] {
                let state = ProxyState::new();
                let endpoint = ProviderEndpointKey::new("codex", case, "default");
                let identity =
                    RuntimeUpstreamIdentity::new(endpoint, format!("https://{case}.example/v1"));
                state
                    .reconcile_runtime_upstream_identities(std::slice::from_ref(&identity), 1)
                    .await
                    .expect("publish runtime identity");
                for domain in domains {
                    state
                        .penalize_runtime_upstream_attempt_for_domain(
                            "codex",
                            &identity,
                            domain,
                            30,
                            cooldown_backoff,
                        )
                        .await;
                }

                assert!(
                    state
                        .try_acquire_runtime_half_open_probe(
                            "codex",
                            &identity,
                            RouteCapability::Inference,
                        )
                        .await
                        .is_none(),
                    "{case} must not be bypassed by half-open probing"
                );
            }
        });
    }

    #[test]
    fn credential_rotation_invalidates_an_unpublished_half_open_probe() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "relay", "default");
            let identity_a = RuntimeUpstreamIdentity::new_with_credential_scope(
                endpoint.clone(),
                "https://relay.example/v1",
                None,
                Some("credential-a".to_string()),
            );
            let identity_b = RuntimeUpstreamIdentity::new_with_credential_scope(
                endpoint,
                "https://relay.example/v1",
                None,
                Some("credential-b".to_string()),
            );
            let cooldown_backoff = CooldownBackoff {
                factor: 1,
                max_secs: 0,
            };
            state
                .reconcile_runtime_upstream_identities(std::slice::from_ref(&identity_a), 1)
                .await
                .expect("publish credential A identity");
            state
                .penalize_runtime_upstream_attempt_for_domain(
                    "codex",
                    &identity_a,
                    RuntimeHealthDomain::EndpointTransport,
                    30,
                    cooldown_backoff,
                )
                .await;
            let probe = state
                .try_acquire_runtime_half_open_probe(
                    "codex",
                    &identity_a,
                    RouteCapability::Inference,
                )
                .await
                .expect("credential A half-open probe");

            state
                .reconcile_runtime_upstream_identities(std::slice::from_ref(&identity_b), 2)
                .await
                .expect("publish credential B identity");

            assert!(
                state.dispatch_runtime_half_open_probe(probe).await.is_err(),
                "a probe captured from the replaced credential generation must not dispatch"
            );
            assert!(
                state
                    .try_acquire_runtime_half_open_probe(
                        "codex",
                        &identity_b,
                        RouteCapability::Inference,
                    )
                    .await
                    .is_none(),
                "the replacement credential must not inherit the old cooldown"
            );
        });
    }

    #[test]
    fn capacity_health_requires_repeated_failures_and_clears_on_success() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "relay", "default");
            let identity =
                RuntimeUpstreamIdentity::new(endpoint.clone(), "https://relay.example/v1");
            let policy = state.capture_provider_policy_snapshot().await;
            let cooldown_backoff = CooldownBackoff {
                factor: 1,
                max_secs: 0,
            };
            state
                .reconcile_runtime_upstream_identities(std::slice::from_ref(&identity), 1)
                .await
                .expect("publish runtime identity");

            for expected_failures in 1..FAILURE_THRESHOLD {
                state
                    .record_runtime_upstream_attempt_failure_for_domain(
                        "codex",
                        &identity,
                        RuntimeHealthDomain::Capacity(RouteCapability::Inference),
                        30,
                        cooldown_backoff,
                    )
                    .await;
                let runtime = state
                    .route_plan_runtime_state_with_provider_policy_for_capability(
                        "codex",
                        policy.as_ref(),
                        1,
                        std::slice::from_ref(&identity),
                        Some(RouteCapability::Inference),
                    )
                    .await;
                let projected = runtime.provider_endpoint(&endpoint);
                assert_eq!(projected.failure_count, expected_failures);
                assert!(!projected.cooldown_active);
            }

            state
                .record_runtime_upstream_attempt_failure_for_domain(
                    "codex",
                    &identity,
                    RuntimeHealthDomain::Capacity(RouteCapability::Inference),
                    30,
                    cooldown_backoff,
                )
                .await;
            let inference = state
                .route_plan_runtime_state_with_provider_policy_for_capability(
                    "codex",
                    policy.as_ref(),
                    1,
                    std::slice::from_ref(&identity),
                    Some(RouteCapability::Inference),
                )
                .await;
            assert!(inference.provider_endpoint(&endpoint).cooldown_active);

            let image = state
                .route_plan_runtime_state_with_provider_policy_for_capability(
                    "codex",
                    policy.as_ref(),
                    1,
                    std::slice::from_ref(&identity),
                    Some(RouteCapability::HostedImageGeneration),
                )
                .await;
            assert!(!image.provider_endpoint(&endpoint).cooldown_active);

            state
                .record_runtime_upstream_attempt_success_for_capability(
                    "codex",
                    &identity,
                    RouteCapability::Inference,
                    10,
                )
                .await;
            let recovered = state
                .route_plan_runtime_state_with_provider_policy_for_capability(
                    "codex",
                    policy.as_ref(),
                    1,
                    std::slice::from_ref(&identity),
                    Some(RouteCapability::Inference),
                )
                .await;
            assert!(!recovered.provider_endpoint(&endpoint).cooldown_active);
        });
    }

    #[test]
    fn committed_provider_observation_projects_below_manual_eligibility() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            let now_ms = unix_now_ms();
            let committed = state
                .set_provider_automatic_block_for_test(endpoint.clone(), true, now_ms)
                .await;
            assert_eq!(
                committed.disposition,
                crate::runtime_store::ProviderObservationDisposition::Accepted
            );
            let runtime = state
                .route_plan_runtime_state_for_provider_endpoints("codex")
                .await;
            let projected = runtime.provider_endpoint(&endpoint);
            assert!(projected.cooldown_active);
            assert!(projected.usage_exhausted);
            assert!(!projected.runtime_disabled);

            state
                .set_provider_manual_eligibility(
                    endpoint.clone(),
                    ProviderManualEligibility::Disabled,
                    Some("operator disabled endpoint".to_string()),
                    now_ms.saturating_add(1_000),
                )
                .await
                .expect("commit manual eligibility");
            let runtime = state
                .route_plan_runtime_state_for_provider_endpoints("codex")
                .await;
            let projected = runtime.provider_endpoint(&endpoint);
            assert!(projected.cooldown_active);
            assert!(projected.usage_exhausted);
            assert!(
                projected.runtime_disabled,
                "manual eligibility must outrank automatic observation projection"
            );
        });
    }

    #[test]
    fn newer_provider_observation_replaces_active_action() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            let first = state
                .set_provider_automatic_block_for_test(endpoint.clone(), true, 1_000)
                .await;
            let second = state
                .set_provider_automatic_block_for_test(endpoint.clone(), true, 2_000)
                .await;

            assert!(second.policy_revision > first.policy_revision);
            let snapshot = state.capture_provider_policy_snapshot().await;
            let projection = snapshot
                .projections
                .iter()
                .find(|projection| projection.provider_endpoint == endpoint)
                .expect("provider projection");
            let action = projection.active_action.as_ref().expect("active action");
            assert_eq!(action.generation, 2);
            assert_eq!(action.opened_at_unix_ms, 2_000);
        });
    }

    #[test]
    fn logical_terminal_keeps_request_captured_policy_revision() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            state
                .set_provider_automatic_block_for_test(endpoint.clone(), true, 1_000)
                .await;
            let captured_revision = state
                .capture_provider_policy_snapshot()
                .await
                .policy_revision;
            let request_id = state
                .begin_request_for_test()
                .model("gpt-5")
                .started_at_ms(1_100)
                .begin()
                .await;

            state
                .set_provider_automatic_block_for_test(endpoint, false, 2_000)
                .await;
            assert!(
                state
                    .capture_provider_policy_snapshot()
                    .await
                    .policy_revision
                    > captured_revision
            );
            assert!(
                state
                    .finish_request(FinishRequestParams {
                        id: request_id,
                        winning_attempt: None,
                        status_code: 200,
                        duration_ms: 1_000,
                        ended_at_ms: 2_100,
                        observed_service_tier: None,
                        reported_model: None,
                        usage: None,
                        retry: None,
                        ttfb_ms: Some(10),
                        streaming: false,
                    })
                    .await
            );

            let records = state
                .runtime_store_handle()
                .read_recent_logical_requests(10)
                .expect("read durable logical requests");
            let terminal = records
                .iter()
                .filter_map(|record| record.terminal.as_ref()?.terminal.payload.as_ref())
                .find(|payload| payload.finished_request.id == request_id)
                .expect("durable request terminal payload");
            assert_eq!(terminal.policy_revision, Some(captured_revision));
        });
    }

    #[test]
    fn authoritative_recovery_closes_active_action() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            state
                .set_provider_automatic_block_for_test(endpoint.clone(), true, 1_000)
                .await;
            state
                .set_provider_automatic_block_for_test(endpoint.clone(), false, 2_000)
                .await;

            let snapshot = state.capture_provider_policy_snapshot().await;
            let projection = snapshot
                .projections
                .iter()
                .find(|projection| projection.provider_endpoint == endpoint)
                .expect("provider projection");
            assert_eq!(projection.automatic, ProviderAutomaticEligibility::Eligible);
            assert_eq!(projection.effective, ProviderEffectiveEligibility::Eligible);
            assert!(projection.active_action.is_none());
        });
    }

    #[test]
    fn passive_attempt_success_does_not_recover_quota_eligibility() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            state
                .set_provider_automatic_block_for_test(endpoint.clone(), true, 1_000)
                .await;
            state
                .record_provider_endpoint_attempt_success("codex", endpoint.clone(), 2_000)
                .await;

            let runtime = state
                .route_plan_runtime_state_for_provider_endpoints("codex")
                .await;
            let projected = runtime.provider_endpoint(&endpoint);
            assert!(projected.cooldown_active);
            assert!(projected.usage_exhausted);
        });
    }

    #[test]
    fn prune_periodic_keeps_sticky_binding_when_session_stats_expire() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new_with_runtime_policy(test_runtime_policy(1, 0, 2_000));
            state
                .set_session_binding(SessionBinding {
                    session_id: "sid-sticky".to_string(),
                    profile_name: Some("daily".to_string()),
                    model: Some("gpt-5.4".to_string()),
                    reasoning_effort: Some("medium".to_string()),
                    service_tier: Some("default".to_string()),
                    continuity_mode: SessionContinuityMode::DefaultProfile,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                    last_seen_ms: 0,
                })
                .await;

            state.prune_periodic().await;

            assert!(state.get_session_binding("sid-sticky").await.is_some());
        });
    }

    #[test]
    fn prune_periodic_honors_opt_in_binding_ttl() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new_with_runtime_policy(test_runtime_policy(0, 1, 2_000));
            state
                .set_session_binding(SessionBinding {
                    session_id: "sid-expire".to_string(),
                    profile_name: Some("daily".to_string()),
                    model: Some("gpt-5.4".to_string()),
                    reasoning_effort: Some("medium".to_string()),
                    service_tier: Some("default".to_string()),
                    continuity_mode: SessionContinuityMode::DefaultProfile,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                    last_seen_ms: 0,
                })
                .await;

            state.prune_periodic().await;

            assert!(state.get_session_binding("sid-expire").await.is_none());
        });
    }

    #[test]
    fn session_route_affinity_ttl_is_enforced_on_read() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let mut policy = test_runtime_policy(0, 0, 2_000);
            policy.session_route_affinity_ttl_ms = 1;
            let state = ProxyState::new_with_runtime_policy(policy);
            state
                .record_session_route_affinity_success(
                    None,
                    "sid-expire",
                    SessionRouteAffinityTarget {
                        route_graph_key: "graph".to_string(),
                        session_identity_source: None,
                        provider_endpoint: ProviderEndpointKey::new("codex", "monthly", "default"),
                        upstream_base_url: "https://monthly.example/v1".to_string(),
                        route_path: vec!["monthly_first".to_string(), "monthly".to_string()],
                    },
                    Some("first_success".to_string()),
                    0,
                )
                .await
                .expect("persist route affinity");

            assert!(
                state
                    .get_session_route_affinity("sid-expire")
                    .await
                    .is_none()
            );
            assert!(state.list_session_route_affinities().await.is_empty());
        });
    }

    #[test]
    fn session_route_affinity_peek_does_not_prune_expired_ledger_entry() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let mut policy = test_runtime_policy(0, 0, 2_000);
            policy.session_route_affinity_ttl_ms = 1;
            let state = ProxyState::new_with_runtime_policy(policy);
            state
                .record_session_route_affinity_success(
                    None,
                    "sid-peek-expired",
                    SessionRouteAffinityTarget {
                        route_graph_key: "graph".to_string(),
                        session_identity_source: None,
                        provider_endpoint: ProviderEndpointKey::new("codex", "monthly", "default"),
                        upstream_base_url: "https://monthly.example/v1".to_string(),
                        route_path: vec!["monthly".to_string()],
                    },
                    None,
                    1,
                )
                .await
                .expect("persist route affinity");

            assert!(
                state
                    .peek_session_route_affinity("sid-peek-expired")
                    .await
                    .is_none()
            );
            assert!(
                state
                    .runtime_store()
                    .count_session_affinities()
                    .expect("count durable affinities")
                    == 1,
                "preflight peek must not mutate the affinity ledger"
            );
        });
    }

    #[test]
    fn session_route_affinity_sqlite_does_not_restore_expired_entries() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let home = TempStateHome::new("expired-affinity");
            let mut policy = test_runtime_policy(0, 0, 2_000);
            policy.session_route_affinity_ttl_ms = 1;
            let first_store = Arc::new(
                RuntimeStore::open_in_home(home.path()).expect("open first runtime store"),
            );
            let first_state =
                ProxyState::new_with_runtime_policy_and_store(policy.clone(), first_store.clone())
                    .expect("create first state");
            first_state
                .record_session_route_affinity_success(
                    None,
                    "sid-expired-ledger",
                    SessionRouteAffinityTarget {
                        route_graph_key: "graph".to_string(),
                        session_identity_source: None,
                        provider_endpoint: ProviderEndpointKey::new("codex", "monthly", "default"),
                        upstream_base_url: "https://monthly.example/v1".to_string(),
                        route_path: vec!["monthly_first".to_string(), "monthly".to_string()],
                    },
                    Some("first_success".to_string()),
                    0,
                )
                .await
                .expect("persist route affinity");

            drop(first_state);
            drop(first_store);

            let second_store =
                Arc::new(RuntimeStore::open_in_home(home.path()).expect("reopen runtime store"));
            let second_state = ProxyState::new_with_runtime_policy_and_store(policy, second_store)
                .expect("create second state");
            assert!(
                second_state
                    .get_session_route_affinity("sid-expired-ledger")
                    .await
                    .is_none()
            );
            assert_eq!(
                second_state
                    .runtime_store()
                    .count_session_affinities()
                    .expect("count pruned affinities"),
                0
            );
        });
    }

    #[test]
    fn session_route_affinity_commit_failure_is_not_published() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let state_changes = state.subscribe_state_changes();
            state.runtime_store().fail_next_affinity_commit_for_test();

            let error = state
                .record_session_route_affinity_success(
                    None,
                    "sid-failed-commit",
                    SessionRouteAffinityTarget {
                        route_graph_key: "graph".to_string(),
                        session_identity_source: Some(SessionIdentitySource::Header),
                        provider_endpoint: ProviderEndpointKey::new("codex", "monthly", "default"),
                        upstream_base_url: "https://monthly.example/v1".to_string(),
                        route_path: vec!["monthly".to_string()],
                    },
                    None,
                    100,
                )
                .await
                .expect_err("failed SQLite commit must not return affinity success");

            assert!(matches!(error, RuntimeStoreError::InjectedFailure { .. }));
            assert!(
                state
                    .peek_session_route_affinity("sid-failed-commit")
                    .await
                    .is_none()
            );
            assert!(!state_changes.has_changed().expect("read state change flag"));
        });
    }

    #[tokio::test]
    async fn session_route_reservation_active_owner_blocks_followers_and_can_move_target() {
        let state = ProxyState::new();
        let owner_request_id = BeginRequestTestBuilder::new(&state)
            .session_id("sid-reservation-owner")
            .begin()
            .await;
        let follower_request_id = BeginRequestTestBuilder::new(&state)
            .session_id("sid-reservation-owner")
            .begin()
            .await;
        let first = available_reservation(
            state
                .claim_session_route_reservation(
                    "sid-reservation-owner",
                    session_route_target("graph:owner", "input"),
                    owner_request_id,
                    1_000,
                )
                .await
                .expect("claim first reservation"),
        );
        let follower = state
            .claim_session_route_reservation(
                "sid-reservation-owner",
                session_route_target("graph:owner", "ciii"),
                follower_request_id,
                1_001,
            )
            .await
            .expect("inspect reservation as follower");
        let moved = available_reservation(
            state
                .claim_session_route_reservation(
                    "sid-reservation-owner",
                    session_route_target("graph:owner", "ciii"),
                    owner_request_id,
                    1_002,
                )
                .await
                .expect("owner should move its provisional target"),
        );

        assert_eq!(first.provider_endpoint.provider_id, "input");
        assert!(matches!(
            follower,
            SessionRouteReservationAccess::Busy {
                owner_request_id: busy_owner
            } if busy_owner == owner_request_id
        ));
        assert_eq!(moved.provider_endpoint.provider_id, "ciii");
        assert!(
            state
                .peek_session_route_affinity("sid-reservation-owner")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn session_route_control_lock_is_scoped_to_session() {
        let state = ProxyState::new();
        let first = state.lock_session_route_control("sid-lock").await;

        let same_session = {
            let state = state.clone();
            tokio::spawn(async move { state.lock_session_route_control("sid-lock").await })
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(20), async {
                while !same_session.is_finished() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .is_err(),
            "the same session must wait for its active selector"
        );

        tokio::time::timeout(
            Duration::from_millis(100),
            state.lock_session_route_control("sid-other"),
        )
        .await
        .expect("different sessions must not share a selection lock");

        drop(first);
        tokio::time::timeout(Duration::from_millis(100), same_session)
            .await
            .expect("same-session selector should resume after release")
            .expect("same-session selector task should finish");
    }

    #[tokio::test]
    async fn session_route_reservation_does_not_expire_while_owner_is_active() {
        let state = ProxyState::new();
        let owner_request_id = BeginRequestTestBuilder::new(&state)
            .session_id("sid-reservation-active")
            .begin()
            .await;
        let follower_request_id = BeginRequestTestBuilder::new(&state)
            .session_id("sid-reservation-active")
            .begin()
            .await;
        let first = available_reservation(
            state
                .claim_session_route_reservation(
                    "sid-reservation-active",
                    session_route_target("graph:active", "input"),
                    owner_request_id,
                    1_000,
                )
                .await
                .expect("claim first reservation"),
        );
        let follower = state
            .get_session_route_reservation(
                "sid-reservation-active",
                "graph:active",
                follower_request_id,
                1_000 + 365 * DAY_MS,
            )
            .await
            .expect("inspect long-running reservation");

        assert_eq!(first.provider_endpoint.provider_id, "input");
        assert!(matches!(
            follower,
            SessionRouteReservationAccess::Busy {
                owner_request_id: busy_owner
            } if busy_owner == owner_request_id
        ));
    }

    #[tokio::test]
    async fn session_route_reservation_is_cleared_after_success() {
        let state = ProxyState::new();
        state
            .claim_session_route_reservation(
                "sid-reservation-success",
                session_route_target("graph:success", "input"),
                7,
                1_000,
            )
            .await
            .expect("claim provisional reservation");

        let durable = state
            .record_session_route_affinity_success(
                Some(7),
                "sid-reservation-success",
                session_route_target("graph:success", "input"),
                Some("test_success".to_string()),
                1_001,
            )
            .await
            .expect("persist successful affinity");
        assert_eq!(durable.provider_endpoint.provider_id, "input");
        assert!(state.session_route_reservations.lock().await.is_empty());

        let claimed_after_success = state
            .claim_session_route_reservation(
                "sid-reservation-success",
                session_route_target("graph:success", "input"),
                8,
                1_002,
            )
            .await
            .expect("claim after successful affinity");
        assert_eq!(
            available_reservation(claimed_after_success)
                .provider_endpoint
                .provider_id,
            "input"
        );
    }

    #[tokio::test]
    async fn session_route_transition_reservation_blocks_cross_key_followers() {
        let state = ProxyState::new();
        state
            .record_session_route_affinity_success(
                None,
                "sid-transition",
                session_route_target("graph:transition", "input"),
                Some("seed".to_string()),
                1_000,
            )
            .await
            .expect("seed durable affinity");
        let owner_request_id = BeginRequestTestBuilder::new(&state)
            .session_id("sid-transition")
            .begin()
            .await;
        let follower_request_id = BeginRequestTestBuilder::new(&state)
            .session_id("sid-transition")
            .begin()
            .await;

        let transition = available_reservation(
            state
                .claim_session_route_reservation(
                    "sid-transition",
                    session_route_target("graph:transition", "ciii"),
                    owner_request_id,
                    1_001,
                )
                .await
                .expect("claim cross-key transition"),
        );
        let follower = state
            .claim_session_route_reservation(
                "sid-transition",
                session_route_target("graph:transition", "input"),
                follower_request_id,
                1_002,
            )
            .await
            .expect("inspect transition as follower");

        assert_eq!(transition.provider_endpoint.provider_id, "ciii");
        assert!(matches!(
            follower,
            SessionRouteReservationAccess::Busy {
                owner_request_id: busy_owner
            } if busy_owner == owner_request_id
        ));

        state
            .record_session_route_affinity_success(
                Some(owner_request_id),
                "sid-transition",
                session_route_target("graph:transition", "ciii"),
                Some("provider_failover".to_string()),
                1_003,
            )
            .await
            .expect("commit owned transition");
        let after_transition = available_reservation(
            state
                .get_session_route_reservation(
                    "sid-transition",
                    "graph:transition",
                    follower_request_id,
                    1_004,
                )
                .await
                .expect("read durable transition"),
        );
        assert_eq!(after_transition.provider_endpoint.provider_id, "ciii");
    }

    #[tokio::test]
    async fn stale_owner_success_does_not_remove_new_owner_reservation() {
        let state = ProxyState::new();
        let stale_request_id = BeginRequestTestBuilder::new(&state)
            .session_id("sid-stale-owner")
            .begin()
            .await;
        let current_request_id = BeginRequestTestBuilder::new(&state)
            .session_id("sid-stale-owner")
            .begin()
            .await;
        state
            .claim_session_route_reservation(
                "sid-stale-owner",
                session_route_target("graph:stale", "input"),
                stale_request_id,
                1_000,
            )
            .await
            .expect("claim stale owner reservation");
        assert!(
            state
                .finish_request(FinishRequestParams {
                    id: stale_request_id,
                    winning_attempt: None,
                    status_code: 500,
                    duration_ms: 1,
                    ended_at_ms: 1_001,
                    observed_service_tier: None,
                    reported_model: None,
                    usage: None,
                    retry: None,
                    ttfb_ms: None,
                    streaming: false,
                })
                .await
        );
        let current = available_reservation(
            state
                .claim_session_route_reservation(
                    "sid-stale-owner",
                    session_route_target("graph:stale", "ciii"),
                    current_request_id,
                    1_002,
                )
                .await
                .expect("replace inactive owner reservation"),
        );
        assert_eq!(current.provider_endpoint.provider_id, "ciii");

        let error = state
            .record_session_route_affinity_success(
                Some(stale_request_id),
                "sid-stale-owner",
                session_route_target("graph:stale", "input"),
                Some("stale_success".to_string()),
                1_003,
            )
            .await
            .expect_err("stale owner must not publish affinity");
        assert!(matches!(
            error,
            RuntimeStoreError::InvariantViolation { .. }
        ));
        let reservations = state.session_route_reservations.lock().await;
        let reservation = reservations
            .get("sid-stale-owner")
            .expect("current owner reservation must remain");
        assert_eq!(reservation.owner_request_id, current_request_id);
        assert_eq!(reservation.affinity.provider_endpoint.provider_id, "ciii");
        drop(reservations);
        assert!(
            state
                .peek_session_route_affinity("sid-stale-owner")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn logical_terminal_and_affinity_publish_before_owner_becomes_inactive() {
        let state = ProxyState::new();
        let owner_request_id = BeginRequestTestBuilder::new(&state)
            .session_id("sid-atomic-affinity")
            .begin()
            .await;
        let follower_request_id = BeginRequestTestBuilder::new(&state)
            .session_id("sid-atomic-affinity")
            .begin()
            .await;
        let target = session_route_target("graph:atomic", "input");
        state
            .claim_session_route_reservation(
                "sid-atomic-affinity",
                target.clone(),
                owner_request_id,
                1_000,
            )
            .await
            .expect("claim owner reservation");
        let (committed, resume) = state
            .pause_next_terminal_publication_after_commit_for_test()
            .await;
        let finishing_state = state.clone();
        let finish = tokio::spawn(async move {
            finishing_state
                .finish_request_with_session_route_affinity(
                    FinishRequestParams {
                        id: owner_request_id,
                        winning_attempt: None,
                        status_code: 200,
                        duration_ms: 1,
                        ended_at_ms: 1_001,
                        observed_service_tier: None,
                        reported_model: None,
                        usage: None,
                        retry: None,
                        ttfb_ms: None,
                        streaming: false,
                    },
                    SessionRouteAffinitySuccess {
                        request_id: owner_request_id,
                        session_id: "sid-atomic-affinity".to_string(),
                        target,
                        reason_hint: Some("first_success".to_string()),
                    },
                )
                .await
        });
        committed
            .await
            .expect("terminal and affinity transaction must reach pause");

        let durable = state
            .peek_session_route_affinity("sid-atomic-affinity")
            .await
            .expect("affinity must be durable before active owner removal");
        assert_eq!(durable.provider_endpoint.provider_id, "input");
        assert!(state.session_route_reservations.lock().await.is_empty());

        let follower_state = state.clone();
        let follower = tokio::spawn(async move {
            follower_state
                .get_session_route_reservation(
                    "sid-atomic-affinity",
                    "graph:atomic",
                    follower_request_id,
                    1_002,
                )
                .await
        });
        tokio::task::yield_now().await;
        assert!(
            !follower.is_finished(),
            "follower must not observe a partially published owner terminal"
        );

        resume.send(()).expect("resume terminal publication");
        assert!(finish.await.expect("join owner terminal publication"));
        let follower = available_reservation(
            follower
                .await
                .expect("join follower reservation")
                .expect("read affinity after owner terminal"),
        );
        assert_eq!(follower.provider_endpoint.provider_id, "input");
    }

    #[tokio::test]
    async fn affinity_failure_rolls_back_logical_terminal_transaction() {
        let state = ProxyState::new();
        let request_id = BeginRequestTestBuilder::new(&state)
            .session_id("sid-affinity-rollback")
            .begin()
            .await;
        let target = session_route_target("graph:rollback", "input");
        state
            .claim_session_route_reservation(
                "sid-affinity-rollback",
                target.clone(),
                request_id,
                1_000,
            )
            .await
            .expect("claim rollback reservation");
        state.runtime_store().fail_next_affinity_commit_for_test();

        let published = state
            .finish_request_with_session_route_affinity(
                FinishRequestParams {
                    id: request_id,
                    winning_attempt: None,
                    status_code: 200,
                    duration_ms: 1,
                    ended_at_ms: 1_001,
                    observed_service_tier: None,
                    reported_model: None,
                    usage: None,
                    retry: None,
                    ttfb_ms: None,
                    streaming: false,
                },
                SessionRouteAffinitySuccess {
                    request_id,
                    session_id: "sid-affinity-rollback".to_string(),
                    target,
                    reason_hint: Some("first_success".to_string()),
                },
            )
            .await;

        assert!(!published);
        assert_eq!(state.list_active_requests().await.len(), 1);
        assert!(state.list_recent_finished(10).await.is_empty());
        assert!(
            state
                .peek_session_route_affinity("sid-affinity-rollback")
                .await
                .is_none()
        );
        let reservations = state.session_route_reservations.lock().await;
        assert_eq!(
            reservations
                .get("sid-affinity-rollback")
                .map(|reservation| reservation.owner_request_id),
            Some(request_id)
        );
    }

    fn atomic_quota_scope_with_revision(
        endpoint: &ProviderEndpointKey,
        config_revision: &str,
    ) -> ProviderObservationScope {
        const DIGEST: &str =
            "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
        ProviderObservationScope::new(
            endpoint.clone(),
            "https://provider.test",
            endpoint.stable_key(),
            "test:atomic-quota",
            "https://provider.test/usage",
            DIGEST,
            config_revision,
        )
        .expect("provider observation scope")
    }

    fn atomic_quota_scope(endpoint: &ProviderEndpointKey) -> ProviderObservationScope {
        const CONFIG_REVISION: &str =
            "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
        atomic_quota_scope_with_revision(endpoint, CONFIG_REVISION)
    }

    fn exhausted_quota_snapshot(
        endpoint: &ProviderEndpointKey,
        observed_at_ms: u64,
    ) -> ProviderBalanceSnapshot {
        let mut snapshot = ProviderBalanceSnapshot::new(
            "quota-test",
            endpoint.clone(),
            "usage_provider:test",
            observed_at_ms,
            Some(observed_at_ms.saturating_add(60_000)),
        );
        snapshot.exhausted = Some(true);
        snapshot.exhaustion_affects_routing = true;
        snapshot.quota_remaining_usd = Some("0".to_string());
        snapshot.quota_limit_usd = Some("10".to_string());
        snapshot.refresh_status(observed_at_ms);
        snapshot
    }

    fn exhausted_provider_observation(observed_at_ms: u64) -> ProviderObservation {
        ProviderObservation {
            observed_at_unix_ms: observed_at_ms,
            completed_at_unix_ms: observed_at_ms.saturating_add(1),
            authority: ProviderObservationAuthority::Authoritative,
            evidence: serde_json::json!({ "exhausted": true }),
            effect: ProviderPolicyEffect::Block {
                action_kind: "balance_exhausted".to_string(),
                code: Some("balance_exhausted".to_string()),
                reason: "test exhaustion".to_string(),
                expires_at_unix_ms: None,
            },
        }
    }

    fn healthy_quota_snapshot(
        endpoint: &ProviderEndpointKey,
        observed_at_ms: u64,
    ) -> ProviderBalanceSnapshot {
        let mut snapshot = ProviderBalanceSnapshot::new(
            "quota-test",
            endpoint.clone(),
            "usage_provider:test",
            observed_at_ms,
            Some(observed_at_ms.saturating_add(60_000)),
        );
        snapshot.exhausted = Some(false);
        snapshot.exhaustion_affects_routing = true;
        snapshot.quota_remaining_usd = Some("10".to_string());
        snapshot.quota_limit_usd = Some("10".to_string());
        snapshot.refresh_status(observed_at_ms);
        snapshot
    }

    fn recovered_provider_observation(observed_at_ms: u64) -> ProviderObservation {
        ProviderObservation {
            observed_at_unix_ms: observed_at_ms,
            completed_at_unix_ms: observed_at_ms.saturating_add(1),
            authority: ProviderObservationAuthority::Authoritative,
            evidence: serde_json::json!({ "exhausted": false }),
            effect: ProviderPolicyEffect::Recover {
                reason: "test recovery".to_string(),
            },
        }
    }

    fn explicit_quota_context(
        snapshot: &ProviderBalanceSnapshot,
        pool_id: &str,
    ) -> QuotaObservationContext {
        let mut context = ProxyState::quota_context_for_balance_snapshot(snapshot);
        context.explicit_pool_id = Some(pool_id.to_string());
        context
    }

    #[derive(Debug, PartialEq, Eq)]
    struct AtomicProjectionBundle {
        durable_policy: ProviderPolicySnapshot,
        quota_document: Option<crate::runtime_store::RuntimeDocument>,
        memory_policy: ProviderPolicySnapshot,
        quota_checkpoint: QuotaRegistryCheckpoint,
        balances: ProviderBalanceMap,
        state_revision: u64,
    }

    async fn atomic_projection_bundle(state: &ProxyState) -> AtomicProjectionBundle {
        AtomicProjectionBundle {
            durable_policy: state
                .runtime_store()
                .provider_policy_snapshot()
                .expect("read durable provider policy"),
            quota_document: state
                .runtime_store()
                .read_runtime_document(RuntimeDocumentKind::QuotaRegistry)
                .expect("read quota document"),
            memory_policy: state
                .capture_provider_policy_snapshot()
                .await
                .as_ref()
                .clone(),
            quota_checkpoint: state.quota_registry_checkpoint().await,
            balances: state.provider_balances.read().await.clone(),
            state_revision: state.operator_revision(),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn atomic_provider_observation_abort_before_balance_lock_commits_nothing() {
        let state = ProxyState::new();
        let endpoint = ProviderEndpointKey::new("codex", "abort", "default");
        let observed_at_ms = unix_now_ms();
        let reservation = state
            .reserve_provider_observation(atomic_quota_scope(&endpoint), observed_at_ms)
            .await
            .expect("reserve observation");
        let before = atomic_projection_bundle(state.as_ref()).await;
        let history_before = state
            .runtime_store()
            .read_provider_observation_history(&endpoint, 10)
            .expect("read observation history");
        let balance_guard = state.provider_balances.write().await;
        let reached = state.arm_provider_balance_lock_wait_signal_for_test().await;

        let commit_state = Arc::clone(&state);
        let commit = tokio::spawn(async move {
            commit_state
                .commit_provider_observation_and_balance_snapshot(
                    reservation,
                    exhausted_provider_observation(observed_at_ms),
                    exhausted_quota_snapshot(&endpoint, observed_at_ms),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(30), reached)
            .await
            .expect("commit reached balance lock")
            .expect("balance lock signal sender retained");
        commit.abort();
        let cancelled = commit.await.expect_err("commit task must be cancelled");
        assert!(cancelled.is_cancelled());
        drop(balance_guard);

        assert_eq!(atomic_projection_bundle(state.as_ref()).await, before);
        assert_eq!(
            state
                .runtime_store()
                .read_provider_observation_history(
                    &ProviderEndpointKey::new("codex", "abort", "default"),
                    10,
                )
                .expect("read observation history after abort"),
            history_before
        );
    }

    #[tokio::test]
    async fn ignored_stale_observation_changes_only_history() {
        let state = ProxyState::new();
        let endpoint = ProviderEndpointKey::new("codex", "stale", "default");
        let observed_at_ms = unix_now_ms();
        let stale = state
            .reserve_provider_observation(atomic_quota_scope(&endpoint), observed_at_ms)
            .await
            .expect("reserve stale generation");
        let current = state
            .reserve_provider_observation(
                atomic_quota_scope(&endpoint),
                observed_at_ms.saturating_add(10),
            )
            .await
            .expect("reserve current generation");
        state
            .commit_provider_observation_and_balance_snapshot(
                current,
                recovered_provider_observation(observed_at_ms.saturating_add(10)),
                healthy_quota_snapshot(&endpoint, observed_at_ms.saturating_add(10)),
            )
            .await
            .expect("commit current generation");
        let before = atomic_projection_bundle(state.as_ref()).await;
        let changes = state.subscribe_state_changes();

        let ignored = state
            .commit_provider_observation_and_balance_snapshot(
                stale,
                exhausted_provider_observation(observed_at_ms),
                exhausted_quota_snapshot(&endpoint, observed_at_ms),
            )
            .await
            .expect("record ignored stale observation");

        assert_eq!(
            ignored.disposition,
            ProviderObservationDisposition::IgnoredStale
        );
        assert_eq!(atomic_projection_bundle(state.as_ref()).await, before);
        assert!(!changes.has_changed().expect("read state change flag"));
        let history = state
            .runtime_store()
            .read_provider_observation_history(&endpoint, 10)
            .expect("read observation history");
        assert_eq!(history.len(), 2);
        assert!(
            history
                .iter()
                .any(|entry| { entry.disposition == ProviderObservationDisposition::Accepted })
        );
        assert!(
            history
                .iter()
                .any(|entry| { entry.disposition == ProviderObservationDisposition::IgnoredStale })
        );
    }

    #[tokio::test]
    async fn ignored_stale_observation_is_independent_of_quota_document_schema() {
        let state = ProxyState::new();
        let endpoint = ProviderEndpointKey::new("codex", "stale-schema", "default");
        let observed_at_ms = unix_now_ms();
        let stale = state
            .reserve_provider_observation(atomic_quota_scope(&endpoint), observed_at_ms)
            .await
            .expect("reserve stale generation");
        let current = state
            .reserve_provider_observation(
                atomic_quota_scope(&endpoint),
                observed_at_ms.saturating_add(10),
            )
            .await
            .expect("reserve current generation");
        state
            .commit_provider_observation_and_balance_snapshot(
                current,
                recovered_provider_observation(observed_at_ms.saturating_add(10)),
                healthy_quota_snapshot(&endpoint, observed_at_ms.saturating_add(10)),
            )
            .await
            .expect("commit current generation");
        let quota_document = state
            .runtime_store()
            .read_runtime_document(RuntimeDocumentKind::QuotaRegistry)
            .expect("read quota document")
            .expect("quota document exists");
        let tampered = state
            .runtime_store()
            .compare_and_write_runtime_document(
                Some(quota_document.revision),
                RuntimeDocumentWrite {
                    kind: RuntimeDocumentKind::QuotaRegistry,
                    schema_version: QUOTA_CHECKPOINT_SCHEMA_VERSION.saturating_add(1),
                    payload_json: r#"{"future":true}"#,
                },
            )
            .expect("install future quota document");
        assert!(matches!(tampered, RuntimeDocumentCommit::Committed(_)));

        let ignored = state
            .commit_provider_observation_and_balance_snapshot(
                stale,
                exhausted_provider_observation(observed_at_ms),
                exhausted_quota_snapshot(&endpoint, observed_at_ms),
            )
            .await
            .expect("record ignored observation without parsing quota document");

        assert_eq!(
            ignored.disposition,
            ProviderObservationDisposition::IgnoredStale
        );
        let history = state
            .runtime_store()
            .read_provider_observation_history(&endpoint, 10)
            .expect("read observation history");
        assert_eq!(history.len(), 2);
        assert!(
            history
                .iter()
                .any(|entry| { entry.disposition == ProviderObservationDisposition::IgnoredStale })
        );
        let quota_document = state
            .runtime_store()
            .read_runtime_document(RuntimeDocumentKind::QuotaRegistry)
            .expect("read future quota document")
            .expect("future quota document exists");
        assert_eq!(
            quota_document.schema_version,
            QUOTA_CHECKPOINT_SCHEMA_VERSION.saturating_add(1)
        );
    }

    #[tokio::test]
    async fn inactive_incarnation_cannot_replace_active_quota_or_balance() {
        const OLD_REVISION: &str =
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        const NEW_REVISION: &str =
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let state = ProxyState::new();
        let endpoint = ProviderEndpointKey::new("codex", "incarnation", "default");
        let observed_at_ms = unix_now_ms();
        let old = state
            .reserve_provider_observation(
                atomic_quota_scope_with_revision(&endpoint, OLD_REVISION),
                observed_at_ms,
            )
            .await
            .expect("reserve old incarnation");
        let current = state
            .reserve_provider_observation(
                atomic_quota_scope_with_revision(&endpoint, NEW_REVISION),
                observed_at_ms.saturating_add(10),
            )
            .await
            .expect("reserve current incarnation");
        let current_snapshot = healthy_quota_snapshot(&endpoint, observed_at_ms.saturating_add(10));
        state
            .commit_provider_observation_and_balance_snapshot_with_quota_context(
                current,
                recovered_provider_observation(observed_at_ms.saturating_add(10)),
                current_snapshot.clone(),
                explicit_quota_context(&current_snapshot, "new-pool"),
            )
            .await
            .expect("commit current incarnation");
        let before = atomic_projection_bundle(state.as_ref()).await;
        let changes = state.subscribe_state_changes();
        let old_snapshot = exhausted_quota_snapshot(&endpoint, observed_at_ms);

        let ignored = state
            .commit_provider_observation_and_balance_snapshot_with_quota_context(
                old,
                exhausted_provider_observation(observed_at_ms),
                old_snapshot.clone(),
                explicit_quota_context(&old_snapshot, "old-pool"),
            )
            .await
            .expect("record inactive incarnation observation");

        assert_eq!(
            ignored.disposition,
            ProviderObservationDisposition::IgnoredInactiveIncarnation
        );
        assert_eq!(atomic_projection_bundle(state.as_ref()).await, before);
        assert!(!changes.has_changed().expect("read state change flag"));
        let membership = state
            .quota_pool_membership(&endpoint)
            .await
            .expect("active pool membership");
        assert!(membership.pool.key.contains("new-pool"));
        assert!(!membership.pool.key.contains("old-pool"));
    }

    #[tokio::test]
    async fn equal_timestamp_late_unreserved_error_does_not_replace_balance_or_quota() {
        let state = ProxyState::new();
        let endpoint = ProviderEndpointKey::new("codex", "unreserved", "default");
        let fetched_at_ms = unix_now_ms();
        let current = healthy_quota_snapshot(&endpoint, fetched_at_ms);
        let published = state
            .try_record_provider_balance_snapshot_with_quota_context(
                current.clone(),
                explicit_quota_context(&current, "fresh-pool"),
            )
            .await
            .expect("publish current balance");
        assert_eq!(published, ProviderBalanceSnapshotPublication::Published);
        let before = atomic_projection_bundle(state.as_ref()).await;
        let changes = state.subscribe_state_changes();
        let mut stale = ProviderBalanceSnapshot::new(
            current.observation_provider_id.clone(),
            endpoint,
            "usage_provider:test",
            fetched_at_ms,
            Some(fetched_at_ms.saturating_add(60_000)),
        )
        .with_error("authentication failed");
        stale.refresh_status(fetched_at_ms);

        let ignored = state
            .try_record_provider_balance_snapshot(stale)
            .await
            .expect("ignore stale unreserved balance");

        assert_eq!(ignored, ProviderBalanceSnapshotPublication::IgnoredOlder);
        assert_eq!(atomic_projection_bundle(state.as_ref()).await, before);
        assert!(!changes.has_changed().expect("read state change flag"));
        let pool = state
            .quota_pool_states()
            .await
            .into_iter()
            .next()
            .expect("quota pool");
        assert_eq!(pool.last_attempt_at_ms, Some(fetched_at_ms));
    }

    #[tokio::test]
    async fn published_balance_carries_its_canonical_quota_pool_identity() {
        let state = ProxyState::new();
        let endpoint = ProviderEndpointKey::new("codex", "identity-link", "default");
        let fetched_at_ms = unix_now_ms();
        let snapshot = healthy_quota_snapshot(&endpoint, fetched_at_ms);

        state
            .try_record_provider_balance_snapshot_with_quota_context(
                snapshot.clone(),
                explicit_quota_context(&snapshot, "identity-link-pool"),
            )
            .await
            .expect("publish balance");

        let membership = state
            .quota_pool_membership(&endpoint)
            .await
            .expect("quota pool membership");
        let balance = state
            .get_provider_balance_view("codex")
            .await
            .into_iter()
            .next()
            .expect("published balance");

        assert_eq!(
            balance.quota_pool_key.as_deref(),
            Some(membership.pool.key.as_str())
        );
        assert_eq!(balance.quota_pool_revision, Some(membership.pool.revision));
    }

    #[test]
    fn provider_policy_snapshot_restores_from_helper_sqlite() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let temp_dir = std::env::temp_dir().join(format!(
                "codex-helper-provider-policy-sqlite-test-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&temp_dir).expect("create temp dir");
            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            let store = Arc::new(RuntimeStore::open_in_home(&temp_dir).expect("open policy store"));
            let first_state =
                ProxyState::new_with_runtime_store(Arc::clone(&store)).expect("hydrate state");
            first_state
                .set_provider_automatic_block_for_test(endpoint.clone(), true, 1_000)
                .await;
            drop(first_state);
            drop(store);

            let reopened_store =
                Arc::new(RuntimeStore::open_in_home(&temp_dir).expect("reopen policy store"));
            let restored = ProxyState::new_with_runtime_store(reopened_store)
                .expect("restore provider policy projection");
            let snapshot = restored.capture_provider_policy_snapshot().await;
            let projection = snapshot
                .projections
                .iter()
                .find(|projection| projection.provider_endpoint == endpoint)
                .expect("restored provider projection");
            assert_eq!(projection.automatic, ProviderAutomaticEligibility::Blocked);
            assert!(projection.active_action.is_some());

            drop(restored);
            std::fs::remove_dir_all(temp_dir).expect("remove temp dir");
        });
    }

    #[test]
    fn provider_policy_commit_failure_preserves_last_known_good_snapshot() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            const DIGEST: &str =
                "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            let scope = ProviderObservationScope::new(
                endpoint.clone(),
                "https://provider.test",
                endpoint.stable_key(),
                "test:commit-failure",
                "https://provider.test/usage",
                DIGEST,
                DIGEST,
            )
            .expect("provider observation scope");
            let reservation = state
                .reserve_provider_observation(scope, 1_000)
                .await
                .expect("reserve observation");
            let before = state.capture_provider_policy_snapshot().await;
            let changes = state.subscribe_state_changes();
            state.runtime_store().fail_next_policy_commit_for_test();

            let error = state
                .commit_provider_observation(
                    reservation,
                    ProviderObservation {
                        observed_at_unix_ms: 1_000,
                        completed_at_unix_ms: 1_001,
                        authority: ProviderObservationAuthority::Authoritative,
                        evidence: serde_json::json!({ "exhausted": true }),
                        effect: ProviderPolicyEffect::Block {
                            action_kind: "balance_exhausted".to_string(),
                            code: Some("balance_exhausted".to_string()),
                            reason: "test exhaustion".to_string(),
                            expires_at_unix_ms: None,
                        },
                    },
                )
                .await
                .expect_err("injected persistence failure");

            assert!(matches!(error, RuntimeStoreError::InjectedFailure { .. }));
            assert_eq!(*state.capture_provider_policy_snapshot().await, *before);
            assert!(!changes.has_changed().expect("read state change flag"));
        });
    }

    #[test]
    fn provider_policy_failure_does_not_write_quota_registry() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            let observed_at_ms = unix_now_ms();
            let reservation = state
                .reserve_provider_observation(atomic_quota_scope(&endpoint), observed_at_ms)
                .await
                .expect("reserve observation");
            let policy_before = state.capture_provider_policy_snapshot().await;
            let quota_before = state.quota_registry_checkpoint().await;
            state.runtime_store().fail_next_policy_commit_for_test();

            let error = state
                .commit_provider_observation_and_balance_snapshot(
                    reservation,
                    exhausted_provider_observation(observed_at_ms),
                    exhausted_quota_snapshot(&endpoint, observed_at_ms),
                )
                .await
                .expect_err("policy failure must roll back quota");

            assert!(matches!(error, RuntimeStoreError::InjectedFailure { .. }));
            assert_eq!(
                *state.capture_provider_policy_snapshot().await,
                *policy_before
            );
            assert_eq!(state.quota_registry_checkpoint().await, quota_before);
            assert!(state.get_provider_balance_view("codex").await.is_empty());
            assert!(
                state
                    .runtime_store()
                    .read_runtime_document(RuntimeDocumentKind::QuotaRegistry)
                    .expect("read quota document")
                    .is_none()
            );
        });
    }

    #[test]
    fn atomic_provider_quota_failure_stays_consistent_after_reopen() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let temp_dir = std::env::temp_dir().join(format!(
                "codex-helper-provider-quota-rollback-test-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&temp_dir).expect("create temp dir");
            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            let observed_at_ms = unix_now_ms();
            let store =
                Arc::new(RuntimeStore::open_in_home(&temp_dir).expect("open runtime store"));
            let state =
                ProxyState::new_with_runtime_store(Arc::clone(&store)).expect("hydrate state");
            let reservation = state
                .reserve_provider_observation(atomic_quota_scope(&endpoint), observed_at_ms)
                .await
                .expect("reserve observation");
            let policy_before = state.capture_provider_policy_snapshot().await;
            let quota_before = state.quota_registry_checkpoint().await;
            store.fail_next_provider_quota_commit_for_test();

            let error = state
                .commit_provider_observation_and_balance_snapshot(
                    reservation,
                    exhausted_provider_observation(observed_at_ms),
                    exhausted_quota_snapshot(&endpoint, observed_at_ms),
                )
                .await
                .expect_err("transaction-end failure must roll back both projections");

            assert!(matches!(error, RuntimeStoreError::InjectedFailure { .. }));
            assert_eq!(
                *state.capture_provider_policy_snapshot().await,
                *policy_before
            );
            assert_eq!(state.quota_registry_checkpoint().await, quota_before);
            assert!(state.get_provider_balance_view("codex").await.is_empty());
            drop(state);
            drop(store);

            let reopened_store =
                Arc::new(RuntimeStore::open_in_home(&temp_dir).expect("reopen runtime store"));
            let restored = ProxyState::new_with_runtime_store(Arc::clone(&reopened_store))
                .expect("restore runtime state");
            assert_eq!(
                *restored.capture_provider_policy_snapshot().await,
                *policy_before
            );
            assert_eq!(restored.quota_registry_checkpoint().await, quota_before);
            assert!(
                reopened_store
                    .read_runtime_document(RuntimeDocumentKind::QuotaRegistry)
                    .expect("read rolled-back quota document")
                    .is_none()
            );
            drop(restored);
            drop(reopened_store);
            std::fs::remove_dir_all(temp_dir).expect("remove temp dir");
        });
    }

    #[test]
    fn atomic_provider_quota_commit_restores_both_projections() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let temp_dir = std::env::temp_dir().join(format!(
                "codex-helper-provider-quota-reopen-test-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&temp_dir).expect("create temp dir");
            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            let observed_at_ms = unix_now_ms();
            let store =
                Arc::new(RuntimeStore::open_in_home(&temp_dir).expect("open runtime store"));
            let state =
                ProxyState::new_with_runtime_store(Arc::clone(&store)).expect("hydrate state");
            let reservation = state
                .reserve_provider_observation(atomic_quota_scope(&endpoint), observed_at_ms)
                .await
                .expect("reserve observation");

            state
                .commit_provider_observation_and_balance_snapshot(
                    reservation,
                    exhausted_provider_observation(observed_at_ms),
                    exhausted_quota_snapshot(&endpoint, observed_at_ms),
                )
                .await
                .expect("commit provider observation and quota");
            let expected_policy = state.capture_provider_policy_snapshot().await;
            let expected_quota = state.quota_registry_checkpoint().await;
            assert_eq!(expected_quota.generation, 1);
            drop(state);
            drop(store);

            let reopened_store =
                Arc::new(RuntimeStore::open_in_home(&temp_dir).expect("reopen runtime store"));
            let restored =
                ProxyState::new_with_runtime_store(reopened_store).expect("restore runtime state");
            assert_eq!(
                *restored.capture_provider_policy_snapshot().await,
                *expected_policy
            );
            assert_eq!(restored.quota_registry_checkpoint().await, expected_quota);
            let projection = expected_policy
                .projections
                .iter()
                .find(|projection| projection.provider_endpoint == endpoint)
                .expect("restored provider projection");
            assert_eq!(projection.automatic, ProviderAutomaticEligibility::Blocked);
            drop(restored);
            std::fs::remove_dir_all(temp_dir).expect("remove temp dir");
        });
    }

    #[test]
    fn runtime_health_prune_does_not_delete_durable_provider_policy() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            state
                .set_provider_automatic_block_for_test(endpoint.clone(), true, 1_000)
                .await;

            let view = route_view(&[("backup", "https://backup.example/v1")]);
            state
                .prune_runtime_observability_for_service("codex", &view)
                .await;

            let snapshot = state.capture_provider_policy_snapshot().await;
            let projection = snapshot
                .projections
                .iter()
                .find(|projection| projection.provider_endpoint == endpoint)
                .expect("durable provider projection");
            assert_eq!(projection.automatic, ProviderAutomaticEligibility::Blocked);
        });
    }

    #[test]
    fn prune_periodic_caps_sticky_bindings_by_last_seen() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new_with_runtime_policy(test_runtime_policy(0, 0, 2));
            for idx in 0..3 {
                state
                    .set_session_binding(SessionBinding {
                        session_id: format!("sid-{idx}"),
                        profile_name: Some("daily".to_string()),
                        model: Some("gpt-5.4".to_string()),
                        reasoning_effort: Some("medium".to_string()),
                        service_tier: Some("default".to_string()),
                        continuity_mode: SessionContinuityMode::DefaultProfile,
                        created_at_ms: idx,
                        updated_at_ms: idx,
                        last_seen_ms: idx,
                    })
                    .await;
            }

            state.prune_periodic().await;

            assert!(state.get_session_binding("sid-0").await.is_none());
            assert!(state.get_session_binding("sid-1").await.is_some());
            assert!(state.get_session_binding("sid-2").await.is_some());
        });
    }

    #[test]
    fn transcript_path_cache_records_negative_lookups() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new_with_runtime_policy(test_runtime_policy(0, 0, 2_000));
            let paths = state
                .resolve_host_transcript_paths_cached(&["missing-session".to_string()])
                .await;

            assert_eq!(paths.get("missing-session"), Some(&None));
            let cache = state.session_transcript_path_cache.read().await;
            assert!(cache.contains_key("missing-session"));
        });
    }
}

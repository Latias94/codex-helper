use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value as JsonValue;
use tokio::sync::{RwLock, watch};
use tokio::time::{Duration, interval};

pub use crate::balance::{
    BalanceSnapshotStatus, ProviderBalanceSnapshot, StationRoutingBalanceSummary,
};
use crate::config::ServiceConfigManager;
use crate::lb::{COOLDOWN_SECS, CooldownBackoff, FAILURE_THRESHOLD, LbState};
use crate::policy_actions::{PolicyAction, PolicyActionKind, PolicyActionProjection};
use crate::pricing::{
    CostAdjustments, CostBreakdown, estimate_request_cost_from_operator_catalog_for_service,
};
use crate::routing_ir::{RoutePlanRuntimeState, RoutePlanUpstreamRuntimeState};
use crate::runtime_identity::ProviderEndpointKey;
use crate::sessions;
#[cfg(test)]
use crate::usage::UsageMetrics;
use crate::usage_day;

mod policy_action_store;
mod runtime_types;
mod session_identity;
mod session_route_ledger;

use self::policy_action_store::{PolicyActionMap, PolicyActionStore};
use self::runtime_types::{
    ConfigMetaOverride, RuntimeDefaultProfileOverride, UsageRollup, merge_station_health,
};
pub use self::runtime_types::{
    HealthCheckStatus, LbConfigView, LbUpstreamView, PassiveHealthState, PassiveUpstreamHealth,
    RuntimeConfigState, StationHealth, UpstreamHealth, UsageBucket, UsageDayCoverage,
    UsageDayDimensionRow, UsageDayHourRow, UsageDayView, UsageRetryGateReasonRow,
    UsageRetryGateSummary, UsageRollupCoverage, UsageRollupView,
};
pub use self::session_identity::{
    ActiveRequest, FinishRequestParams, FinishedRequest, RequestObservability, ResolvedRouteValue,
    RouteDecisionProvenance, RouteValueSource, SessionBinding, SessionContinuityMode,
    SessionIdentityCard, SessionIdentityCardBuildInputs, SessionIdentitySource,
    SessionManualOverrides, SessionObservationScope, SessionRouteAffinity,
    SessionRouteAffinityTarget, SessionStats, build_session_identity_cards_from_parts,
    enrich_session_identity_cards_with_host_transcripts,
    enrich_session_identity_cards_with_runtime,
};
use self::session_identity::{
    SessionBindingEntry, SessionCwdCacheEntry, SessionEffortOverride, SessionModelOverride,
    SessionRouteTargetOverride, SessionServiceTierOverride, SessionStationOverride,
};
use self::session_route_ledger::SessionRouteAffinityStore;

type PassiveStationHealthMap =
    HashMap<String, HashMap<String, HashMap<String, PassiveUpstreamHealth>>>;
type ProviderBalanceMap =
    HashMap<String, HashMap<String, HashMap<usize, HashMap<String, ProviderBalanceSnapshot>>>>;
type ProviderBalanceHistoryMap = HashMap<
    String,
    HashMap<String, HashMap<usize, HashMap<String, VecDeque<ProviderBalanceSnapshot>>>>,
>;
type ProviderBalanceSummaryMap = HashMap<String, HashMap<String, StationRoutingBalanceSummary>>;
type ServiceLayoutSignature = Vec<(String, Vec<String>)>;

#[derive(Debug, Clone, Default)]
struct ProviderEndpointRuntimeHealth {
    failure_count: u32,
    cooldown_until: Option<std::time::Instant>,
    usage_exhausted: bool,
    penalty_streak: u32,
    last_good_at_ms: Option<u64>,
}

#[derive(Debug, Clone)]
struct SessionTranscriptPathCacheEntry {
    path: Option<String>,
    last_checked_ms: u64,
    last_seen_ms: u64,
}

#[derive(Debug, Clone)]
struct RuntimePolicy {
    session_override_ttl_ms: u64,
    session_binding_ttl_ms: u64,
    session_binding_max_entries: usize,
    session_route_affinity_ttl_ms: u64,
    session_route_affinity_max_entries: usize,
    session_route_affinity_store: SessionRouteAffinityStore,
    session_cwd_cache_ttl_ms: u64,
    session_cwd_cache_max_entries: usize,
    session_transcript_path_cache_ttl_ms: u64,
    session_transcript_path_cache_max_entries: usize,
}

pub struct PassiveUpstreamFailureRecord {
    pub service_name: String,
    pub station_name: String,
    pub base_url: String,
    pub status_code: Option<u16>,
    pub error_class: Option<String>,
    pub error: Option<String>,
    pub now_ms: u64,
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

fn provider_balance_history_max() -> usize {
    static MAX: OnceLock<usize> = OnceLock::new();
    *MAX.get_or_init(|| {
        std::env::var("CODEX_HELPER_PROVIDER_BALANCE_HISTORY_MAX")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&n| n > 1)
            .unwrap_or(64)
            .clamp(2, 512)
    })
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn usage_rollup_unknown_key(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("-")
        .to_string()
}

fn usage_rollup_project_key(cwd: Option<&str>) -> String {
    usage_rollup_unknown_key(cwd)
}

fn usage_rollup_request_key(request: &FinishedRequest) -> String {
    request
        .trace_id
        .as_deref()
        .map(|trace_id| format!("trace|{trace_id}"))
        .unwrap_or_else(|| format!("{}|{}", request.ended_at_ms, request.id))
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

fn usage_rollup_mark_loaded_timestamp(rollup: &mut UsageRollup, timestamp_ms: u64) {
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
    if rollup.coverage_source.is_empty() {
        rollup.coverage_source = "live".to_string();
    }
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
    if rollup.replay_bytes_truncated {
        reasons.push("request log byte limit truncated older data");
    }
    if rollup.replay_lines_truncated {
        reasons.push("request log line limit truncated older data");
    }
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
        scanned_lines: rollup.replay_scanned_lines,
        max_lines: rollup.replay_max_lines,
        max_bytes: rollup.replay_max_bytes,
        bytes_truncated: rollup.replay_bytes_truncated,
        lines_truncated: rollup.replay_lines_truncated,
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
    request: &FinishedRequest,
) -> bool {
    let request_key = usage_rollup_request_key(request);
    if rollup.recorded_requests.contains_key(&request_key) {
        return false;
    }

    let day = usage_day::local_day_from_ms(request.ended_at_ms);
    let hour = usize::from(usage_day::local_hour_from_ms(request.ended_at_ms).min(23));
    rollup.recorded_requests.insert(request_key, day);
    usage_rollup_mark_loaded_timestamp(rollup, request.ended_at_ms);

    let cost = Some(&request.cost);
    usage_rollup_record_bucket(&mut rollup.loaded, request, cost);
    usage_rollup_record_bucket(rollup.by_day.entry(day).or_default(), request, cost);
    usage_rollup_record_bucket(
        &mut usage_rollup_hourly_buckets(&mut rollup.by_hour, day)[hour],
        request,
        cost,
    );

    record_usage_entity(
        &mut rollup.by_config,
        &mut rollup.by_config_day,
        usage_rollup_unknown_key(request.station_name.as_deref()),
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
        usage_rollup_project_key(request.cwd.as_deref()),
        day,
        request,
        cost,
    );

    true
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

fn session_route_affinity_is_expired_with_ttl(
    ttl_ms: u64,
    affinity: &SessionRouteAffinity,
    now_ms: u64,
) -> bool {
    ttl_ms > 0 && now_ms.saturating_sub(affinity.last_selected_at_ms) >= ttl_ms
}

fn prune_session_route_affinity_map(
    affinities: &mut HashMap<String, SessionRouteAffinity>,
    ttl_ms: u64,
    max_entries: usize,
    now_ms: u64,
) {
    affinities.retain(|_, affinity| {
        !session_route_affinity_is_expired_with_ttl(ttl_ms, affinity, now_ms)
    });
    prune_lru_cache(affinities, max_entries, |entry| entry.last_selected_at_ms);
}

fn service_layout_signature(mgr: &ServiceConfigManager) -> ServiceLayoutSignature {
    let mut entries = mgr
        .stations()
        .iter()
        .map(|(station_name, service)| {
            (
                station_name.clone(),
                service
                    .upstreams
                    .iter()
                    .map(|upstream| upstream.base_url.clone())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    entries
}

fn changed_service_layout_stations(
    previous: &ServiceLayoutSignature,
    current: &ServiceLayoutSignature,
) -> HashSet<String> {
    let previous_by_station = previous
        .iter()
        .map(|(station_name, upstreams)| (station_name.as_str(), upstreams.as_slice()))
        .collect::<HashMap<_, _>>();
    let current_by_station = current
        .iter()
        .map(|(station_name, upstreams)| (station_name.as_str(), upstreams.as_slice()))
        .collect::<HashMap<_, _>>();
    let mut changed = HashSet::new();

    for (station_name, upstreams) in previous {
        if current_by_station
            .get(station_name.as_str())
            .is_none_or(|current_upstreams| *current_upstreams != upstreams.as_slice())
        {
            changed.insert(station_name.clone());
        }
    }

    for (station_name, upstreams) in current {
        if previous_by_station
            .get(station_name.as_str())
            .is_none_or(|previous_upstreams| *previous_upstreams != upstreams.as_slice())
        {
            changed.insert(station_name.clone());
        }
    }

    changed
}

/// Runtime-only state for the proxy process.
///
/// Most state is process-local. Session route affinity is persisted separately
/// because Codex remote compaction can depend on provider-endpoint continuity.
#[derive(Debug)]
pub struct ProxyState {
    next_request_id: AtomicU64,
    // Manual per-session overrides remain runtime-scoped and expire after inactivity.
    session_override_ttl_ms: u64,
    // Bindings are sticky by default; operators can opt into pruning with a separate TTL.
    session_binding_ttl_ms: u64,
    session_binding_max_entries: usize,
    session_route_affinity_ttl_ms: u64,
    session_route_affinity_max_entries: usize,
    session_route_affinity_store: SessionRouteAffinityStore,
    session_cwd_cache_ttl_ms: u64,
    session_cwd_cache_max_entries: usize,
    session_transcript_path_cache_ttl_ms: u64,
    session_transcript_path_cache_max_entries: usize,
    session_effort_overrides: RwLock<HashMap<String, SessionEffortOverride>>,
    session_station_overrides: RwLock<HashMap<String, SessionStationOverride>>,
    session_route_target_overrides: RwLock<HashMap<String, SessionRouteTargetOverride>>,
    session_model_overrides: RwLock<HashMap<String, SessionModelOverride>>,
    session_service_tier_overrides: RwLock<HashMap<String, SessionServiceTierOverride>>,
    session_bindings: RwLock<HashMap<String, SessionBindingEntry>>,
    session_route_affinities: RwLock<HashMap<String, SessionRouteAffinity>>,
    global_station_override: RwLock<Option<String>>,
    global_route_target_override: RwLock<Option<String>>,
    runtime_default_profiles: RwLock<HashMap<String, RuntimeDefaultProfileOverride>>,
    station_meta_overrides: RwLock<HashMap<String, HashMap<String, ConfigMetaOverride>>>,
    // Primary provider-endpoint overrides keyed by stable provider identity.
    provider_endpoint_meta_overrides:
        RwLock<HashMap<String, HashMap<ProviderEndpointKey, ConfigMetaOverride>>>,
    // Legacy base_url-keyed overrides kept for compatibility with station-oriented callers.
    upstream_meta_overrides: RwLock<HashMap<String, HashMap<String, ConfigMetaOverride>>>,
    session_cwd_cache: RwLock<HashMap<String, SessionCwdCacheEntry>>,
    session_transcript_path_cache: RwLock<HashMap<String, SessionTranscriptPathCacheEntry>>,
    session_stats: RwLock<HashMap<String, SessionStats>>,
    active_requests: RwLock<HashMap<u64, ActiveRequest>>,
    recent_finished: RwLock<VecDeque<FinishedRequest>>,
    usage_rollups: RwLock<HashMap<String, UsageRollup>>,
    station_health: RwLock<HashMap<String, HashMap<String, StationHealth>>>,
    passive_station_health: RwLock<PassiveStationHealthMap>,
    provider_balances: RwLock<ProviderBalanceMap>,
    provider_balance_history: RwLock<ProviderBalanceHistoryMap>,
    provider_balance_summaries: RwLock<ProviderBalanceSummaryMap>,
    provider_endpoint_runtime_health:
        RwLock<HashMap<String, HashMap<ProviderEndpointKey, ProviderEndpointRuntimeHealth>>>,
    policy_action_store: PolicyActionStore,
    policy_actions: RwLock<PolicyActionMap>,
    station_health_checks: RwLock<HashMap<String, HashMap<String, HealthCheckStatus>>>,
    service_layout_signatures: RwLock<HashMap<String, ServiceLayoutSignature>>,
    state_version_tx: watch::Sender<u64>,
    lb_states: Option<Arc<Mutex<HashMap<String, LbState>>>>,
}

impl ProxyState {
    const MAX_HEALTH_RECORDS_PER_STATION: usize = 200;

    #[allow(dead_code)]
    pub fn new() -> Arc<Self> {
        Self::new_with_lb_states(None)
    }

    pub fn new_with_lb_states(
        lb_states: Option<Arc<Mutex<HashMap<String, LbState>>>>,
    ) -> Arc<Self> {
        let ttl_secs = std::env::var("CODEX_HELPER_SESSION_OVERRIDE_TTL_SECS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(30 * 60);
        let ttl_ms = ttl_secs.saturating_mul(1000);
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

        let cwd_cache_ttl_secs = std::env::var("CODEX_HELPER_SESSION_CWD_CACHE_TTL_SECS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(12 * 60 * 60);
        let cwd_cache_ttl_ms = cwd_cache_ttl_secs.saturating_mul(1000);
        let cwd_cache_max_entries = std::env::var("CODEX_HELPER_SESSION_CWD_CACHE_MAX_ENTRIES")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(2_000);
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

        Self::new_with_runtime_policy(
            lb_states,
            RuntimePolicy {
                session_override_ttl_ms: ttl_ms,
                session_binding_ttl_ms: binding_ttl_ms,
                session_binding_max_entries: binding_max_entries,
                session_route_affinity_ttl_ms: route_affinity_ttl_ms,
                session_route_affinity_max_entries: route_affinity_max_entries,
                session_route_affinity_store: SessionRouteAffinityStore::from_env(),
                session_cwd_cache_ttl_ms: cwd_cache_ttl_ms,
                session_cwd_cache_max_entries: cwd_cache_max_entries,
                session_transcript_path_cache_ttl_ms: transcript_path_cache_ttl_ms,
                session_transcript_path_cache_max_entries: transcript_path_cache_max_entries,
            },
        )
    }

    fn new_with_runtime_policy(
        lb_states: Option<Arc<Mutex<HashMap<String, LbState>>>>,
        policy: RuntimePolicy,
    ) -> Arc<Self> {
        let mut restored_session_route_affinities = policy.session_route_affinity_store.load();
        prune_session_route_affinity_map(
            &mut restored_session_route_affinities,
            policy.session_route_affinity_ttl_ms,
            policy.session_route_affinity_max_entries,
            unix_now_ms(),
        );
        let policy_action_store = PolicyActionStore::from_env();
        let now_ms = unix_now_ms();
        let (restored_policy_actions, pruned_policy_actions) = policy_action_store.load(now_ms);
        if pruned_policy_actions
            && let Err(err) = policy_action_store.save_blocking(&restored_policy_actions, now_ms)
        {
            tracing::warn!(error = %err, "failed to compact expired policy action ledger");
        }

        Arc::new(Self {
            next_request_id: AtomicU64::new(1),
            session_override_ttl_ms: policy.session_override_ttl_ms,
            session_binding_ttl_ms: policy.session_binding_ttl_ms,
            session_binding_max_entries: policy.session_binding_max_entries,
            session_route_affinity_ttl_ms: policy.session_route_affinity_ttl_ms,
            session_route_affinity_max_entries: policy.session_route_affinity_max_entries,
            session_route_affinity_store: policy.session_route_affinity_store,
            session_cwd_cache_ttl_ms: policy.session_cwd_cache_ttl_ms,
            session_cwd_cache_max_entries: policy.session_cwd_cache_max_entries,
            session_transcript_path_cache_ttl_ms: policy.session_transcript_path_cache_ttl_ms,
            session_transcript_path_cache_max_entries: policy
                .session_transcript_path_cache_max_entries,
            session_effort_overrides: RwLock::new(HashMap::new()),
            session_station_overrides: RwLock::new(HashMap::new()),
            session_route_target_overrides: RwLock::new(HashMap::new()),
            session_model_overrides: RwLock::new(HashMap::new()),
            session_service_tier_overrides: RwLock::new(HashMap::new()),
            session_bindings: RwLock::new(HashMap::new()),
            session_route_affinities: RwLock::new(restored_session_route_affinities),
            global_station_override: RwLock::new(None),
            global_route_target_override: RwLock::new(None),
            runtime_default_profiles: RwLock::new(HashMap::new()),
            station_meta_overrides: RwLock::new(HashMap::new()),
            provider_endpoint_meta_overrides: RwLock::new(HashMap::new()),
            upstream_meta_overrides: RwLock::new(HashMap::new()),
            session_cwd_cache: RwLock::new(HashMap::new()),
            session_transcript_path_cache: RwLock::new(HashMap::new()),
            session_stats: RwLock::new(HashMap::new()),
            active_requests: RwLock::new(HashMap::new()),
            recent_finished: RwLock::new(VecDeque::new()),
            usage_rollups: RwLock::new(HashMap::new()),
            station_health: RwLock::new(HashMap::new()),
            passive_station_health: RwLock::new(HashMap::new()),
            provider_balances: RwLock::new(HashMap::new()),
            provider_balance_history: RwLock::new(HashMap::new()),
            provider_balance_summaries: RwLock::new(HashMap::new()),
            provider_endpoint_runtime_health: RwLock::new(HashMap::new()),
            policy_action_store,
            policy_actions: RwLock::new(restored_policy_actions),
            station_health_checks: RwLock::new(HashMap::new()),
            service_layout_signatures: RwLock::new(HashMap::new()),
            state_version_tx: watch::channel(0).0,
            lb_states,
        })
    }

    pub fn subscribe_state_changes(&self) -> watch::Receiver<u64> {
        self.state_version_tx.subscribe()
    }

    fn notify_state_changed(&self) {
        self.state_version_tx.send_modify(|version| {
            *version = version.wrapping_add(1);
        });
    }

    pub async fn get_session_effort_override(&self, session_id: &str) -> Option<String> {
        let guard = self.session_effort_overrides.read().await;
        guard.get(session_id).map(|v| v.effort.clone())
    }

    pub async fn get_session_reasoning_effort_override(&self, session_id: &str) -> Option<String> {
        self.get_session_effort_override(session_id).await
    }

    pub async fn set_session_effort_override(
        &self,
        session_id: String,
        effort: String,
        now_ms: u64,
    ) {
        let mut guard = self.session_effort_overrides.write().await;
        guard.insert(
            session_id,
            SessionEffortOverride {
                effort,
                updated_at_ms: now_ms,
                last_seen_ms: now_ms,
            },
        );
        self.notify_state_changed();
    }

    pub async fn set_session_reasoning_effort_override(
        &self,
        session_id: String,
        reasoning_effort: String,
        now_ms: u64,
    ) {
        self.set_session_effort_override(session_id, reasoning_effort, now_ms)
            .await;
    }

    pub async fn clear_session_effort_override(&self, session_id: &str) {
        let mut guard = self.session_effort_overrides.write().await;
        if guard.remove(session_id).is_some() {
            self.notify_state_changed();
        }
    }

    pub async fn clear_session_reasoning_effort_override(&self, session_id: &str) {
        self.clear_session_effort_override(session_id).await;
    }

    pub async fn list_session_effort_overrides(&self) -> HashMap<String, String> {
        let guard = self.session_effort_overrides.read().await;
        guard
            .iter()
            .map(|(k, v)| (k.clone(), v.effort.clone()))
            .collect()
    }

    pub async fn list_session_reasoning_effort_overrides(&self) -> HashMap<String, String> {
        self.list_session_effort_overrides().await
    }

    pub async fn touch_session_override(&self, session_id: &str, now_ms: u64) {
        let mut guard = self.session_effort_overrides.write().await;
        if let Some(v) = guard.get_mut(session_id) {
            v.last_seen_ms = now_ms;
        }
    }

    pub async fn touch_session_reasoning_effort_override(&self, session_id: &str, now_ms: u64) {
        self.touch_session_override(session_id, now_ms).await;
    }

    pub async fn get_session_station_override(&self, session_id: &str) -> Option<String> {
        let guard = self.session_station_overrides.read().await;
        guard.get(session_id).map(|v| v.station_name.clone())
    }

    pub async fn get_session_route_target_override(&self, session_id: &str) -> Option<String> {
        let guard = self.session_route_target_overrides.read().await;
        guard.get(session_id).map(|v| v.target.clone())
    }

    pub async fn get_session_model_override(&self, session_id: &str) -> Option<String> {
        let guard = self.session_model_overrides.read().await;
        guard.get(session_id).map(|v| v.model.clone())
    }

    pub async fn set_session_model_override(&self, session_id: String, model: String, now_ms: u64) {
        let mut guard = self.session_model_overrides.write().await;
        guard.insert(
            session_id,
            SessionModelOverride {
                model,
                updated_at_ms: now_ms,
                last_seen_ms: now_ms,
            },
        );
        self.notify_state_changed();
    }

    pub async fn clear_session_model_override(&self, session_id: &str) {
        let mut guard = self.session_model_overrides.write().await;
        if guard.remove(session_id).is_some() {
            self.notify_state_changed();
        }
    }

    pub async fn list_session_model_overrides(&self) -> HashMap<String, String> {
        let guard = self.session_model_overrides.read().await;
        guard
            .iter()
            .map(|(k, v)| (k.clone(), v.model.clone()))
            .collect()
    }

    pub async fn touch_session_model_override(&self, session_id: &str, now_ms: u64) {
        let mut guard = self.session_model_overrides.write().await;
        if let Some(v) = guard.get_mut(session_id) {
            v.last_seen_ms = now_ms;
        }
    }

    pub async fn get_session_service_tier_override(&self, session_id: &str) -> Option<String> {
        let guard = self.session_service_tier_overrides.read().await;
        guard.get(session_id).map(|v| v.service_tier.clone())
    }

    pub async fn set_session_service_tier_override(
        &self,
        session_id: String,
        service_tier: String,
        now_ms: u64,
    ) {
        let mut guard = self.session_service_tier_overrides.write().await;
        guard.insert(
            session_id,
            SessionServiceTierOverride {
                service_tier,
                updated_at_ms: now_ms,
                last_seen_ms: now_ms,
            },
        );
        self.notify_state_changed();
    }

    pub async fn clear_session_service_tier_override(&self, session_id: &str) {
        let mut guard = self.session_service_tier_overrides.write().await;
        if guard.remove(session_id).is_some() {
            self.notify_state_changed();
        }
    }

    pub async fn list_session_service_tier_overrides(&self) -> HashMap<String, String> {
        let guard = self.session_service_tier_overrides.read().await;
        guard
            .iter()
            .map(|(k, v)| (k.clone(), v.service_tier.clone()))
            .collect()
    }

    pub async fn touch_session_service_tier_override(&self, session_id: &str, now_ms: u64) {
        let mut guard = self.session_service_tier_overrides.write().await;
        if let Some(v) = guard.get_mut(session_id) {
            v.last_seen_ms = now_ms;
        }
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
        let (affinity, snapshot) = {
            let mut guard = self.session_route_affinities.write().await;
            let affinity = guard.get(session_id).cloned()?;
            if self.session_route_affinity_is_expired(&affinity, unix_now_ms()) {
                guard.remove(session_id);
                (None, Some(guard.clone()))
            } else {
                (Some(affinity), None)
            }
        };
        if let Some(snapshot) = snapshot {
            self.persist_session_route_affinities(snapshot).await;
        }
        affinity
    }

    pub async fn list_session_route_affinities(&self) -> HashMap<String, SessionRouteAffinity> {
        let (affinities, snapshot) = {
            let mut guard = self.session_route_affinities.write().await;
            let now_ms = unix_now_ms();
            let before_len = guard.len();
            guard.retain(|_, affinity| !self.session_route_affinity_is_expired(affinity, now_ms));
            let affinities = guard.clone();
            let snapshot = (guard.len() != before_len).then(|| guard.clone());
            (affinities, snapshot)
        };
        if let Some(snapshot) = snapshot {
            self.persist_session_route_affinities(snapshot).await;
        }
        affinities
    }

    pub async fn record_session_route_affinity_success(
        &self,
        session_id: &str,
        target: SessionRouteAffinityTarget,
        reason_hint: Option<String>,
        now_ms: u64,
    ) -> SessionRouteAffinity {
        let (affinity, snapshot) = {
            let mut guard = self.session_route_affinities.write().await;
            let affinity = match guard.get_mut(session_id) {
                Some(existing) if target.same_target(existing) => {
                    existing.last_selected_at_ms = now_ms;
                    if target.session_identity_source.is_some() {
                        existing.session_identity_source = target.session_identity_source;
                    }
                    existing.clone()
                }
                Some(_) => {
                    let reason = reason_hint.unwrap_or_else(|| "target_changed".to_string());
                    let affinity = SessionRouteAffinity {
                        route_graph_key: target.route_graph_key,
                        session_identity_source: target.session_identity_source,
                        provider_endpoint: target.provider_endpoint,
                        upstream_base_url: target.upstream_base_url,
                        route_path: target.route_path,
                        last_selected_at_ms: now_ms,
                        last_changed_at_ms: now_ms,
                        change_reason: reason,
                    };
                    guard.insert(session_id.to_string(), affinity.clone());
                    affinity
                }
                None => {
                    let reason = reason_hint.unwrap_or_else(|| "first_success".to_string());
                    let affinity = SessionRouteAffinity {
                        route_graph_key: target.route_graph_key,
                        session_identity_source: target.session_identity_source,
                        provider_endpoint: target.provider_endpoint,
                        upstream_base_url: target.upstream_base_url,
                        route_path: target.route_path,
                        last_selected_at_ms: now_ms,
                        last_changed_at_ms: now_ms,
                        change_reason: reason,
                    };
                    guard.insert(session_id.to_string(), affinity.clone());
                    affinity
                }
            };
            prune_lru_cache(
                &mut guard,
                self.session_route_affinity_max_entries,
                |entry| entry.last_selected_at_ms,
            );
            (affinity, guard.clone())
        };
        self.persist_session_route_affinities(snapshot).await;
        self.notify_state_changed();
        affinity
    }

    fn session_route_affinity_is_expired(
        &self,
        affinity: &SessionRouteAffinity,
        now_ms: u64,
    ) -> bool {
        session_route_affinity_is_expired_with_ttl(
            self.session_route_affinity_ttl_ms,
            affinity,
            now_ms,
        )
    }

    async fn persist_session_route_affinities(
        &self,
        snapshot: HashMap<String, SessionRouteAffinity>,
    ) {
        if let Err(err) = self
            .session_route_affinity_store
            .save(&snapshot, unix_now_ms())
            .await
        {
            tracing::warn!(error = %err, "failed to persist session route affinity ledger");
        }
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

    pub async fn clear_session_manual_overrides(&self, session_id: &str) {
        self.clear_session_station_override(session_id).await;
        self.clear_session_route_target_override(session_id).await;
        self.clear_session_model_override(session_id).await;
        self.clear_session_effort_override(session_id).await;
        self.clear_session_service_tier_override(session_id).await;
    }

    pub async fn get_session_manual_overrides(&self, session_id: &str) -> SessionManualOverrides {
        let (reasoning_effort, station_name, route_target, model, service_tier) = tokio::join!(
            self.get_session_reasoning_effort_override(session_id),
            self.get_session_station_override(session_id),
            self.get_session_route_target_override(session_id),
            self.get_session_model_override(session_id),
            self.get_session_service_tier_override(session_id),
        );

        SessionManualOverrides {
            reasoning_effort,
            station_name,
            route_target,
            model,
            service_tier,
        }
    }

    pub async fn list_session_manual_overrides(&self) -> HashMap<String, SessionManualOverrides> {
        let (reasoning_effort_map, station_map, route_target_map, model_map, service_tier_map) = tokio::join!(
            self.list_session_reasoning_effort_overrides(),
            self.list_session_station_overrides(),
            self.list_session_route_target_overrides(),
            self.list_session_model_overrides(),
            self.list_session_service_tier_overrides(),
        );

        let mut merged = HashMap::<String, SessionManualOverrides>::new();
        for (session_id, reasoning_effort) in reasoning_effort_map {
            merged.entry(session_id).or_default().reasoning_effort = Some(reasoning_effort);
        }
        for (session_id, station_name) in station_map {
            merged.entry(session_id).or_default().station_name = Some(station_name);
        }
        for (session_id, route_target) in route_target_map {
            merged.entry(session_id).or_default().route_target = Some(route_target);
        }
        for (session_id, model) in model_map {
            merged.entry(session_id).or_default().model = Some(model);
        }
        for (session_id, service_tier) in service_tier_map {
            merged.entry(session_id).or_default().service_tier = Some(service_tier);
        }
        merged.retain(|_, overrides| !overrides.is_empty());
        merged
    }

    pub async fn apply_session_profile_binding(
        &self,
        service_name: &str,
        mgr: &ServiceConfigManager,
        session_id: String,
        profile_name: String,
        now_ms: u64,
    ) -> anyhow::Result<()> {
        let profile = crate::config::resolve_service_profile(mgr, profile_name.as_str())?;
        crate::config::validate_profile_station_compatibility(
            service_name,
            mgr,
            profile_name.as_str(),
            &profile,
        )?;

        self.set_session_binding(SessionBinding {
            session_id: session_id.clone(),
            profile_name: Some(profile_name),
            station_name: profile.station.clone(),
            model: profile.model.clone(),
            reasoning_effort: profile.reasoning_effort.clone(),
            service_tier: profile.service_tier.clone(),
            continuity_mode: SessionContinuityMode::ManualProfile,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            last_seen_ms: now_ms,
        })
        .await;
        self.clear_session_manual_overrides(session_id.as_str())
            .await;
        Ok(())
    }

    pub async fn touch_session_binding(&self, session_id: &str, now_ms: u64) {
        let mut guard = self.session_bindings.write().await;
        if let Some(entry) = guard.get_mut(session_id) {
            entry.binding.last_seen_ms = now_ms;
        }
    }

    pub async fn set_session_station_override(
        &self,
        session_id: String,
        station_name: String,
        now_ms: u64,
    ) {
        let mut guard = self.session_station_overrides.write().await;
        guard.insert(
            session_id,
            SessionStationOverride {
                station_name,
                updated_at_ms: now_ms,
                last_seen_ms: now_ms,
            },
        );
        self.notify_state_changed();
    }

    pub async fn set_session_route_target_override(
        &self,
        session_id: String,
        target: String,
        now_ms: u64,
    ) {
        let mut guard = self.session_route_target_overrides.write().await;
        guard.insert(
            session_id,
            SessionRouteTargetOverride {
                target,
                updated_at_ms: now_ms,
                last_seen_ms: now_ms,
            },
        );
        self.notify_state_changed();
    }

    pub async fn clear_session_station_override(&self, session_id: &str) {
        let mut guard = self.session_station_overrides.write().await;
        if guard.remove(session_id).is_some() {
            self.notify_state_changed();
        }
    }

    pub async fn clear_session_route_target_override(&self, session_id: &str) {
        let mut guard = self.session_route_target_overrides.write().await;
        if guard.remove(session_id).is_some() {
            self.notify_state_changed();
        }
    }

    pub async fn list_session_station_overrides(&self) -> HashMap<String, String> {
        let guard = self.session_station_overrides.read().await;
        guard
            .iter()
            .map(|(k, v)| (k.clone(), v.station_name.clone()))
            .collect()
    }

    pub async fn list_session_route_target_overrides(&self) -> HashMap<String, String> {
        let guard = self.session_route_target_overrides.read().await;
        guard
            .iter()
            .map(|(k, v)| (k.clone(), v.target.clone()))
            .collect()
    }

    pub async fn touch_session_station_override(&self, session_id: &str, now_ms: u64) {
        let mut guard = self.session_station_overrides.write().await;
        if let Some(v) = guard.get_mut(session_id) {
            v.last_seen_ms = now_ms;
        }
    }

    pub async fn touch_session_route_target_override(&self, session_id: &str, now_ms: u64) {
        let mut guard = self.session_route_target_overrides.write().await;
        if let Some(v) = guard.get_mut(session_id) {
            v.last_seen_ms = now_ms;
        }
    }

    pub async fn get_global_station_override(&self) -> Option<String> {
        let guard = self.global_station_override.read().await;
        guard.clone()
    }

    pub async fn get_global_route_target_override(&self) -> Option<String> {
        let guard = self.global_route_target_override.read().await;
        guard.clone()
    }

    pub async fn set_global_station_override(&self, station_name: String, _now_ms: u64) {
        let mut guard = self.global_station_override.write().await;
        *guard = Some(station_name);
        self.notify_state_changed();
    }

    pub async fn set_global_route_target_override(&self, target: String, _now_ms: u64) {
        let mut guard = self.global_route_target_override.write().await;
        *guard = Some(target);
        self.notify_state_changed();
    }

    pub async fn clear_global_station_override(&self) {
        let mut guard = self.global_station_override.write().await;
        if guard.take().is_some() {
            self.notify_state_changed();
        }
    }

    pub async fn clear_global_route_target_override(&self) {
        let mut guard = self.global_route_target_override.write().await;
        if guard.take().is_some() {
            self.notify_state_changed();
        }
    }

    pub async fn get_runtime_default_profile_override(&self, service_name: &str) -> Option<String> {
        let guard = self.runtime_default_profiles.read().await;
        guard
            .get(service_name)
            .map(|entry| entry.profile_name.clone())
    }

    pub async fn set_runtime_default_profile_override(
        &self,
        service_name: String,
        profile_name: String,
        now_ms: u64,
    ) {
        let mut guard = self.runtime_default_profiles.write().await;
        guard.insert(
            service_name,
            RuntimeDefaultProfileOverride {
                profile_name,
                updated_at_ms: now_ms,
            },
        );
        self.notify_state_changed();
    }

    pub async fn clear_runtime_default_profile_override(&self, service_name: &str) {
        let mut guard = self.runtime_default_profiles.write().await;
        if guard.remove(service_name).is_some() {
            self.notify_state_changed();
        }
    }

    pub async fn set_station_enabled_override(
        &self,
        service_name: &str,
        station_name: String,
        enabled: bool,
        now_ms: u64,
    ) {
        let mut guard = self.station_meta_overrides.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        let entry = per_service.entry(station_name).or_default();
        entry.enabled = Some(enabled);
        entry.updated_at_ms = now_ms;
        self.notify_state_changed();
    }

    pub async fn set_station_level_override(
        &self,
        service_name: &str,
        station_name: String,
        level: u8,
        now_ms: u64,
    ) {
        let mut guard = self.station_meta_overrides.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        let entry = per_service.entry(station_name).or_default();
        entry.level = Some(level.clamp(1, 10));
        entry.updated_at_ms = now_ms;
        self.notify_state_changed();
    }

    pub async fn set_station_runtime_state_override(
        &self,
        service_name: &str,
        station_name: String,
        state: RuntimeConfigState,
        now_ms: u64,
    ) {
        let mut guard = self.station_meta_overrides.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        let entry = per_service.entry(station_name).or_default();
        entry.state = Some(state);
        entry.updated_at_ms = now_ms;
        self.notify_state_changed();
    }

    pub async fn clear_station_enabled_override(&self, service_name: &str, station_name: &str) {
        let mut guard = self.station_meta_overrides.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return;
        };
        let Some(entry) = per_service.get_mut(station_name) else {
            return;
        };
        if entry.enabled.take().is_none() {
            return;
        }
        if entry.enabled.is_none() && entry.level.is_none() && entry.state.is_none() {
            per_service.remove(station_name);
        }
        if per_service.is_empty() {
            guard.remove(service_name);
        }
        self.notify_state_changed();
    }

    pub async fn clear_station_level_override(&self, service_name: &str, station_name: &str) {
        let mut guard = self.station_meta_overrides.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return;
        };
        let Some(entry) = per_service.get_mut(station_name) else {
            return;
        };
        if entry.level.take().is_none() {
            return;
        }
        if entry.enabled.is_none() && entry.level.is_none() && entry.state.is_none() {
            per_service.remove(station_name);
        }
        if per_service.is_empty() {
            guard.remove(service_name);
        }
        self.notify_state_changed();
    }

    pub async fn clear_station_runtime_state_override(
        &self,
        service_name: &str,
        station_name: &str,
    ) {
        let mut guard = self.station_meta_overrides.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return;
        };
        let Some(entry) = per_service.get_mut(station_name) else {
            return;
        };
        if entry.state.take().is_none() {
            return;
        }
        if entry.enabled.is_none() && entry.level.is_none() && entry.state.is_none() {
            per_service.remove(station_name);
        }
        if per_service.is_empty() {
            guard.remove(service_name);
        }
        self.notify_state_changed();
    }

    pub async fn get_station_meta_overrides(
        &self,
        service_name: &str,
    ) -> HashMap<String, (Option<bool>, Option<u8>)> {
        let guard = self.station_meta_overrides.read().await;
        guard
            .get(service_name)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), (v.enabled, v.level)))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default()
    }

    pub async fn get_station_runtime_state_overrides(
        &self,
        service_name: &str,
    ) -> HashMap<String, RuntimeConfigState> {
        let guard = self.station_meta_overrides.read().await;
        guard
            .get(service_name)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.state.map(|state| (k.clone(), state)))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default()
    }

    pub async fn set_provider_endpoint_enabled_override(
        &self,
        service_name: &str,
        endpoint_key: ProviderEndpointKey,
        enabled: bool,
        now_ms: u64,
    ) {
        let mut guard = self.provider_endpoint_meta_overrides.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        let entry = per_service.entry(endpoint_key).or_default();
        entry.enabled = Some(enabled);
        entry.updated_at_ms = now_ms;
        self.notify_state_changed();
    }

    pub async fn clear_provider_endpoint_enabled_override(
        &self,
        service_name: &str,
        endpoint_key: &ProviderEndpointKey,
    ) {
        let mut guard = self.provider_endpoint_meta_overrides.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return;
        };
        let Some(entry) = per_service.get_mut(endpoint_key) else {
            return;
        };
        if entry.enabled.take().is_none() {
            return;
        }
        if entry.enabled.is_none() && entry.level.is_none() && entry.state.is_none() {
            per_service.remove(endpoint_key);
        }
        if per_service.is_empty() {
            guard.remove(service_name);
        }
        self.notify_state_changed();
    }

    pub async fn set_provider_endpoint_runtime_state_override(
        &self,
        service_name: &str,
        endpoint_key: ProviderEndpointKey,
        state: RuntimeConfigState,
        now_ms: u64,
    ) {
        let mut guard = self.provider_endpoint_meta_overrides.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        let entry = per_service.entry(endpoint_key).or_default();
        entry.state = Some(state);
        entry.updated_at_ms = now_ms;
        self.notify_state_changed();
    }

    pub async fn clear_provider_endpoint_runtime_state_override(
        &self,
        service_name: &str,
        endpoint_key: &ProviderEndpointKey,
    ) {
        let mut guard = self.provider_endpoint_meta_overrides.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return;
        };
        let Some(entry) = per_service.get_mut(endpoint_key) else {
            return;
        };
        if entry.state.take().is_none() {
            return;
        }
        if entry.enabled.is_none() && entry.level.is_none() && entry.state.is_none() {
            per_service.remove(endpoint_key);
        }
        if per_service.is_empty() {
            guard.remove(service_name);
        }
        self.notify_state_changed();
    }

    pub async fn set_upstream_enabled_override(
        &self,
        service_name: &str,
        base_url: String,
        enabled: bool,
        now_ms: u64,
    ) {
        let mut guard = self.upstream_meta_overrides.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        let entry = per_service.entry(base_url).or_default();
        entry.enabled = Some(enabled);
        entry.updated_at_ms = now_ms;
        self.notify_state_changed();
    }

    pub async fn clear_upstream_enabled_override(&self, service_name: &str, base_url: &str) {
        let mut guard = self.upstream_meta_overrides.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return;
        };
        let Some(entry) = per_service.get_mut(base_url) else {
            return;
        };
        if entry.enabled.take().is_none() {
            return;
        }
        if entry.enabled.is_none() && entry.level.is_none() && entry.state.is_none() {
            per_service.remove(base_url);
        }
        if per_service.is_empty() {
            guard.remove(service_name);
        }
        self.notify_state_changed();
    }

    pub async fn set_upstream_runtime_state_override(
        &self,
        service_name: &str,
        base_url: String,
        state: RuntimeConfigState,
        now_ms: u64,
    ) {
        let mut guard = self.upstream_meta_overrides.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        let entry = per_service.entry(base_url).or_default();
        entry.state = Some(state);
        entry.updated_at_ms = now_ms;
        self.notify_state_changed();
    }

    pub async fn clear_upstream_runtime_state_override(&self, service_name: &str, base_url: &str) {
        let mut guard = self.upstream_meta_overrides.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return;
        };
        let Some(entry) = per_service.get_mut(base_url) else {
            return;
        };
        if entry.state.take().is_none() {
            return;
        }
        if entry.enabled.is_none() && entry.level.is_none() && entry.state.is_none() {
            per_service.remove(base_url);
        }
        if per_service.is_empty() {
            guard.remove(service_name);
        }
        self.notify_state_changed();
    }

    pub async fn get_upstream_meta_overrides(
        &self,
        service_name: &str,
    ) -> HashMap<String, (Option<bool>, Option<RuntimeConfigState>)> {
        let mut overrides = HashMap::new();

        {
            let guard = self.upstream_meta_overrides.read().await;
            if let Some(per_service) = guard.get(service_name) {
                overrides.extend(
                    per_service
                        .iter()
                        .map(|(k, v)| (k.clone(), (v.enabled, v.state))),
                );
            }
        }

        {
            let guard = self.provider_endpoint_meta_overrides.read().await;
            if let Some(per_service) = guard.get(service_name) {
                overrides.extend(
                    per_service
                        .iter()
                        .map(|(k, v)| (k.stable_key(), (v.enabled, v.state))),
                );
            }
        }

        overrides
    }

    pub async fn route_plan_runtime_state_for_provider_endpoints(
        &self,
        service_name: &str,
    ) -> RoutePlanRuntimeState {
        let mut runtime = RoutePlanRuntimeState::default();
        let now = std::time::Instant::now();
        let now_ms = unix_now_ms();
        {
            let mut guard = self.provider_endpoint_runtime_health.write().await;
            if let Some(per_service) = guard.get_mut(service_name) {
                let mut affinity: Option<(ProviderEndpointKey, u64)> = None;
                for (endpoint_key, health) in per_service.iter_mut() {
                    if health.cooldown_until.is_some_and(|until| now >= until) {
                        health.failure_count = 0;
                        health.cooldown_until = None;
                    }
                    let cooldown_active = health.cooldown_until.is_some_and(|until| now < until);
                    let cooldown_remaining_secs = health.cooldown_until.and_then(|until| {
                        if now < until {
                            Some(until.duration_since(now).as_secs().max(1))
                        } else {
                            None
                        }
                    });
                    runtime.set_provider_endpoint(
                        endpoint_key.clone(),
                        RoutePlanUpstreamRuntimeState {
                            runtime_disabled: false,
                            failure_count: health.failure_count,
                            cooldown_active,
                            cooldown_remaining_secs,
                            usage_exhausted: health.usage_exhausted,
                            missing_auth: false,
                            concurrency_saturated: false,
                            concurrency_active: None,
                            concurrency_limit: None,
                        },
                    );
                    if let Some(last_good_at_ms) = health.last_good_at_ms
                        && affinity
                            .as_ref()
                            .is_none_or(|(_, current)| last_good_at_ms > *current)
                    {
                        affinity = Some((endpoint_key.clone(), last_good_at_ms));
                    }
                }
                if let Some((endpoint_key, _)) = affinity {
                    runtime.set_affinity_provider_endpoint(Some(endpoint_key));
                }
            }
        }

        {
            let guard = self.policy_actions.read().await;
            if let Some(per_service) = guard.get(service_name) {
                for (endpoint_key, actions) in per_service {
                    for action in actions {
                        let Some(projection) = PolicyActionProjection::from_action(action, now_ms)
                        else {
                            continue;
                        };
                        let mut upstream_state = runtime.provider_endpoint(endpoint_key);
                        if projection.active_cooldown {
                            upstream_state.cooldown_active = true;
                            upstream_state.cooldown_remaining_secs =
                                projection.cooldown_remaining_secs;
                            if matches!(action.kind, PolicyActionKind::Cooldown)
                                && matches!(
                                    action.source_signal.kind,
                                    crate::provider_signals::ProviderSignalKind::Quota
                                        | crate::provider_signals::ProviderSignalKind::RateLimit
                                )
                            {
                                upstream_state.usage_exhausted = true;
                            }
                        }
                        runtime.set_provider_endpoint(endpoint_key.clone(), upstream_state);
                    }
                }
            }
        }

        {
            let guard = self.provider_endpoint_meta_overrides.read().await;
            if let Some(per_service) = guard.get(service_name) {
                for (endpoint_key, meta) in per_service {
                    let mut upstream_state = runtime.provider_endpoint(endpoint_key);
                    if meta.enabled == Some(false)
                        || meta
                            .state
                            .is_some_and(|state| state != RuntimeConfigState::Normal)
                    {
                        upstream_state.runtime_disabled = true;
                    }
                    runtime.set_provider_endpoint(endpoint_key.clone(), upstream_state);
                }
            }
        }

        runtime
    }

    pub async fn upsert_owned_policy_action(&self, service_name: &str, action: PolicyAction) {
        let mut guard = self.policy_actions.write().await;
        let mut snapshot = guard.clone();
        {
            let per_service = snapshot.entry(service_name.to_string()).or_default();
            let actions = per_service
                .entry(action.provider_endpoint_key.clone())
                .or_default();
            if let Some(existing) = actions.iter_mut().find(|existing| {
                existing.kind == action.kind
                    && existing.owner == action.owner
                    && existing.source_signal.kind == action.source_signal.kind
                    && existing.source_signal.source == action.source_signal.source
            }) {
                *existing = action;
            } else {
                actions.push(action);
            }
        }
        if !self.persist_policy_actions(&snapshot).await {
            return;
        }
        *guard = snapshot;
        self.notify_state_changed();
    }

    pub async fn clear_owned_policy_action(
        &self,
        service_name: &str,
        endpoint_key: &ProviderEndpointKey,
        kind: PolicyActionKind,
        source_kind: crate::provider_signals::ProviderSignalKind,
        source: crate::provider_signals::ProviderSignalSource,
    ) {
        let mut guard = self.policy_actions.write().await;
        let mut snapshot = guard.clone();
        let changed = {
            let mut changed = false;
            if let Some(per_service) = snapshot.get_mut(service_name) {
                if let Some(actions) = per_service.get_mut(endpoint_key) {
                    let before = actions.len();
                    actions.retain(|action| {
                        !(action.kind == kind
                            && action.owner
                                == crate::policy_actions::PolicyActionOwner::CodexHelper)
                            || action.source_signal.kind != source_kind
                            || action.source_signal.source != source
                    });
                    changed |= actions.len() != before;
                    if actions.is_empty() {
                        per_service.remove(endpoint_key);
                    }
                }
                if per_service.is_empty() {
                    snapshot.remove(service_name);
                }
            }
            changed
        };
        if changed {
            if !self.persist_policy_actions(&snapshot).await {
                return;
            }
            *guard = snapshot;
            self.notify_state_changed();
        }
    }

    async fn persist_policy_actions(&self, snapshot: &PolicyActionMap) -> bool {
        match self.policy_action_store.save(snapshot, unix_now_ms()).await {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!(error = %err, "failed to persist policy action ledger");
                false
            }
        }
    }

    pub async fn list_policy_actions(&self, service_name: &str) -> Vec<PolicyAction> {
        let guard = self.policy_actions.read().await;
        guard
            .get(service_name)
            .into_iter()
            .flat_map(|per_service| per_service.values())
            .flat_map(|actions| actions.iter().cloned())
            .collect()
    }

    pub async fn active_policy_action_projections(
        &self,
        service_name: &str,
        now_ms: u64,
    ) -> Vec<PolicyActionProjection> {
        let guard = self.policy_actions.read().await;
        guard
            .get(service_name)
            .into_iter()
            .flat_map(|per_service| per_service.values())
            .flat_map(|actions| actions.iter())
            .filter_map(|action| PolicyActionProjection::from_action(action, now_ms))
            .collect()
    }

    pub async fn record_provider_endpoint_attempt_success(
        &self,
        service_name: &str,
        endpoint_key: ProviderEndpointKey,
        now_ms: u64,
    ) {
        let mut guard = self.provider_endpoint_runtime_health.write().await;
        let entry = guard
            .entry(service_name.to_string())
            .or_default()
            .entry(endpoint_key)
            .or_default();
        entry.failure_count = 0;
        entry.cooldown_until = None;
        entry.penalty_streak = 0;
        entry.last_good_at_ms = Some(now_ms);
    }

    pub async fn record_provider_endpoint_attempt_failure(
        &self,
        service_name: &str,
        endpoint_key: ProviderEndpointKey,
        failure_threshold_cooldown_secs: u64,
        cooldown_backoff: CooldownBackoff,
    ) {
        let mut guard = self.provider_endpoint_runtime_health.write().await;
        let entry = guard
            .entry(service_name.to_string())
            .or_default()
            .entry(endpoint_key)
            .or_default();

        entry.failure_count = entry.failure_count.saturating_add(1);
        if entry.failure_count >= FAILURE_THRESHOLD {
            let base_secs = if failure_threshold_cooldown_secs == 0 {
                COOLDOWN_SECS
            } else {
                failure_threshold_cooldown_secs
            };
            let effective_secs =
                cooldown_backoff.effective_cooldown_secs(base_secs, entry.penalty_streak);
            let now = std::time::Instant::now();
            let new_until = now + std::time::Duration::from_secs(effective_secs);
            if entry
                .cooldown_until
                .is_none_or(|existing| new_until > existing)
            {
                entry.cooldown_until = Some(new_until);
            }
            entry.penalty_streak = entry.penalty_streak.saturating_add(1);
            entry.last_good_at_ms = None;
        }
    }

    pub async fn penalize_provider_endpoint_attempt(
        &self,
        service_name: &str,
        endpoint_key: ProviderEndpointKey,
        cooldown_secs: u64,
        cooldown_backoff: CooldownBackoff,
    ) {
        let mut guard = self.provider_endpoint_runtime_health.write().await;
        let entry = guard
            .entry(service_name.to_string())
            .or_default()
            .entry(endpoint_key)
            .or_default();
        let effective_secs =
            cooldown_backoff.effective_cooldown_secs(cooldown_secs, entry.penalty_streak);
        entry.failure_count = FAILURE_THRESHOLD;
        entry.cooldown_until =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(effective_secs));
        entry.penalty_streak = entry.penalty_streak.saturating_add(1);
        entry.last_good_at_ms = None;
    }

    pub async fn set_provider_endpoint_usage_exhausted(
        &self,
        service_name: &str,
        endpoint_key: ProviderEndpointKey,
        exhausted: bool,
    ) {
        let mut guard = self.provider_endpoint_runtime_health.write().await;
        let entry = guard
            .entry(service_name.to_string())
            .or_default()
            .entry(endpoint_key)
            .or_default();
        if entry.usage_exhausted == exhausted {
            return;
        }
        entry.usage_exhausted = exhausted;
        self.notify_state_changed();
    }

    pub async fn prune_runtime_observability_for_service(
        &self,
        service_name: &str,
        mgr: &ServiceConfigManager,
    ) {
        let mut changed = false;
        let active_stations = mgr.stations().keys().cloned().collect::<HashSet<_>>();
        let active_upstreams = mgr
            .stations()
            .iter()
            .map(|(station_name, service)| {
                (
                    station_name.clone(),
                    service
                        .upstreams
                        .iter()
                        .map(|upstream| upstream.base_url.clone())
                        .collect::<HashSet<_>>(),
                )
            })
            .collect::<HashMap<_, _>>();
        let active_base_urls = active_upstreams
            .values()
            .flat_map(|upstreams| upstreams.iter().cloned())
            .collect::<HashSet<_>>();
        let active_provider_endpoint_keys = mgr
            .stations()
            .iter()
            .flat_map(|(station_name, service)| {
                service.upstreams.iter().enumerate().map(|(idx, upstream)| {
                    Self::active_provider_endpoint_key_for_upstream(
                        service_name,
                        station_name.as_str(),
                        idx,
                        upstream,
                    )
                })
            })
            .collect::<HashSet<_>>();
        let mut active_provider_ids = HashSet::from(["-".to_string()]);
        for service in mgr.stations().values() {
            for upstream in &service.upstreams {
                if let Some(provider_id) = upstream.tags.get("provider_id") {
                    active_provider_ids.insert(provider_id.clone());
                }
            }
        }

        let layout = service_layout_signature(mgr);
        let balance_prune_stations = {
            let mut signatures = self.service_layout_signatures.write().await;
            let changed = signatures.get(service_name).map_or_else(
                || {
                    if active_stations.len() == 1 && active_stations.contains("routing") {
                        Some(HashSet::from(["routing".to_string()]))
                    } else {
                        None
                    }
                },
                |previous| Some(changed_service_layout_stations(previous, &layout)),
            );
            signatures.insert(service_name.to_string(), layout);
            changed
        };

        match balance_prune_stations {
            Some(changed_layout_stations) if !changed_layout_stations.is_empty() => {
                let mut provider_balances = self.provider_balances.write().await;
                if let Some(per_service) = provider_balances.get_mut(service_name) {
                    let before = per_service.len();
                    per_service
                        .retain(|station_name, _| !changed_layout_stations.contains(station_name));
                    changed |= per_service.len() != before;
                    if per_service.is_empty() {
                        provider_balances.remove(service_name);
                    }
                }
                let mut provider_balance_history = self.provider_balance_history.write().await;
                if let Some(per_service) = provider_balance_history.get_mut(service_name) {
                    let before = per_service.len();
                    per_service
                        .retain(|station_name, _| !changed_layout_stations.contains(station_name));
                    changed |= per_service.len() != before;
                    if per_service.is_empty() {
                        provider_balance_history.remove(service_name);
                    }
                }
                let mut provider_balance_summaries = self.provider_balance_summaries.write().await;
                if let Some(per_service) = provider_balance_summaries.get_mut(service_name) {
                    let before = per_service.len();
                    per_service
                        .retain(|station_name, _| !changed_layout_stations.contains(station_name));
                    changed |= per_service.len() != before;
                    if per_service.is_empty() {
                        provider_balance_summaries.remove(service_name);
                    }
                }
            }
            None => {
                let mut provider_balances = self.provider_balances.write().await;
                changed |= provider_balances.remove(service_name).is_some();
                let mut provider_balance_history = self.provider_balance_history.write().await;
                changed |= provider_balance_history.remove(service_name).is_some();
                let mut provider_balance_summaries = self.provider_balance_summaries.write().await;
                changed |= provider_balance_summaries.remove(service_name).is_some();
            }
            Some(_) => {}
        }

        {
            let mut guard = self.station_meta_overrides.write().await;
            if let Some(per_service) = guard.get_mut(service_name) {
                let before = per_service.len();
                per_service.retain(|station_name, _| active_stations.contains(station_name));
                changed |= per_service.len() != before;
                if per_service.is_empty() {
                    guard.remove(service_name);
                }
            }
        }

        {
            let mut guard = self.upstream_meta_overrides.write().await;
            if let Some(per_service) = guard.get_mut(service_name) {
                let before = per_service.len();
                per_service.retain(|base_url, _| active_base_urls.contains(base_url));
                changed |= per_service.len() != before;
                if per_service.is_empty() {
                    guard.remove(service_name);
                }
            }
        }

        {
            let mut guard = self.provider_endpoint_meta_overrides.write().await;
            if let Some(per_service) = guard.get_mut(service_name) {
                let before = per_service.len();
                per_service
                    .retain(|endpoint_key, _| active_provider_endpoint_keys.contains(endpoint_key));
                changed |= per_service.len() != before;
                if per_service.is_empty() {
                    guard.remove(service_name);
                }
            }
        }

        {
            let mut guard = self.provider_endpoint_runtime_health.write().await;
            if let Some(per_service) = guard.get_mut(service_name) {
                per_service
                    .retain(|endpoint_key, _| active_provider_endpoint_keys.contains(endpoint_key));
                if per_service.is_empty() {
                    guard.remove(service_name);
                }
            }
        }

        {
            let mut guard = self.policy_actions.write().await;
            let mut snapshot = guard.clone();
            let mut policy_actions_changed = false;
            if let Some(per_service) = snapshot.get_mut(service_name) {
                let before = per_service.len();
                per_service
                    .retain(|endpoint_key, _| active_provider_endpoint_keys.contains(endpoint_key));
                policy_actions_changed |= per_service.len() != before;
                if per_service.is_empty() {
                    snapshot.remove(service_name);
                }
            }
            if policy_actions_changed && self.persist_policy_actions(&snapshot).await {
                *guard = snapshot;
                changed = true;
            }
        }

        {
            let mut guard = self.station_health.write().await;
            if let Some(per_service) = guard.get_mut(service_name) {
                let before_stations = per_service.len();
                let before_upstreams = per_service
                    .values()
                    .map(|station_health| station_health.upstreams.len())
                    .sum::<usize>();
                per_service.retain(|station_name, station_health| {
                    if !active_stations.contains(station_name) {
                        return false;
                    }
                    if let Some(allowed_upstreams) = active_upstreams.get(station_name) {
                        station_health
                            .upstreams
                            .retain(|upstream| allowed_upstreams.contains(&upstream.base_url));
                    }
                    !station_health.upstreams.is_empty()
                });
                let after_upstreams = per_service
                    .values()
                    .map(|station_health| station_health.upstreams.len())
                    .sum::<usize>();
                changed |=
                    per_service.len() != before_stations || after_upstreams != before_upstreams;
                if per_service.is_empty() {
                    guard.remove(service_name);
                }
            }
        }

        {
            let mut guard = self.passive_station_health.write().await;
            if let Some(per_service) = guard.get_mut(service_name) {
                let before_stations = per_service.len();
                let before_upstreams = per_service.values().map(HashMap::len).sum::<usize>();
                per_service.retain(|station_name, station_health| {
                    if !active_stations.contains(station_name) {
                        return false;
                    }
                    if let Some(allowed_upstreams) = active_upstreams.get(station_name) {
                        station_health.retain(|base_url, _| allowed_upstreams.contains(base_url));
                    }
                    !station_health.is_empty()
                });
                let after_upstreams = per_service.values().map(HashMap::len).sum::<usize>();
                changed |=
                    per_service.len() != before_stations || after_upstreams != before_upstreams;
                if per_service.is_empty() {
                    guard.remove(service_name);
                }
            }
        }

        {
            let mut guard = self.station_health_checks.write().await;
            if let Some(per_service) = guard.get_mut(service_name) {
                let before = per_service.len();
                per_service.retain(|station_name, _| active_stations.contains(station_name));
                changed |= per_service.len() != before;
                if per_service.is_empty() {
                    guard.remove(service_name);
                }
            }
        }

        {
            let mut guard = self.usage_rollups.write().await;
            if let Some(rollup) = guard.get_mut(service_name) {
                let before_by_config = rollup.by_config.len();
                rollup
                    .by_config
                    .retain(|station_name, _| active_stations.contains(station_name));
                changed |= rollup.by_config.len() != before_by_config;
                let before_by_config_day = rollup.by_config_day.len();
                rollup.by_config_day.retain(|station_name, _day_map| {
                    if !active_stations.contains(station_name) {
                        return false;
                    }
                    true
                });
                changed |= rollup.by_config_day.len() != before_by_config_day;
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

    fn active_provider_endpoint_key_for_upstream(
        service_name: &str,
        station_name: &str,
        upstream_index: usize,
        upstream: &crate::config::UpstreamConfig,
    ) -> ProviderEndpointKey {
        let provider_id = upstream
            .tags
            .get("provider_id")
            .cloned()
            .unwrap_or_else(|| format!("{station_name}#{upstream_index}"));
        let endpoint_id = upstream
            .tags
            .get("endpoint_id")
            .cloned()
            .unwrap_or_else(|| upstream_index.to_string());

        ProviderEndpointKey::new(service_name, provider_id, endpoint_id)
    }

    pub async fn record_station_health(
        &self,
        service_name: &str,
        station_name: String,
        health: StationHealth,
    ) {
        let mut guard = self.station_health.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        per_service.insert(station_name, health);
        self.notify_state_changed();
    }

    pub async fn get_station_health(&self, service_name: &str) -> HashMap<String, StationHealth> {
        let active = {
            let guard = self.station_health.read().await;
            guard.get(service_name).cloned().unwrap_or_default()
        };
        let passive = {
            let guard = self.passive_station_health.read().await;
            guard.get(service_name).cloned().unwrap_or_default()
        };
        merge_station_health(active, passive)
    }

    pub async fn record_provider_balance_snapshot(
        &self,
        service_name: &str,
        mut snapshot: ProviderBalanceSnapshot,
    ) {
        let (Some(station_name), Some(upstream_index)) =
            (snapshot.station_name.clone(), snapshot.upstream_index)
        else {
            return;
        };
        let now_ms = unix_now_ms();
        snapshot.refresh_status(now_ms);

        let station_summary = {
            let mut guard = self.provider_balances.write().await;
            let station_balances = guard
                .entry(service_name.to_string())
                .or_default()
                .entry(station_name.clone())
                .or_default();
            if !snapshot.has_amount_data()
                && let Some(previous) = station_balances
                    .get(&upstream_index)
                    .and_then(|providers| providers.get(&snapshot.provider_id))
            {
                snapshot.carry_forward_amount_data_from(previous);
                snapshot.refresh_status(now_ms);
            }
            station_balances
                .entry(upstream_index)
                .or_default()
                .insert(snapshot.provider_id.clone(), snapshot.clone());
            StationRoutingBalanceSummary::from_snapshot_iter_at(
                station_balances
                    .values()
                    .flat_map(|providers| providers.values()),
                now_ms,
            )
        };

        {
            let mut summaries = self.provider_balance_summaries.write().await;
            summaries
                .entry(service_name.to_string())
                .or_default()
                .insert(station_name, station_summary);
        }

        let history_station_name = snapshot.station_name.clone().unwrap_or_default();
        self.record_provider_balance_history_snapshot(
            service_name,
            &history_station_name,
            upstream_index,
            snapshot,
        )
        .await;
        self.notify_state_changed();
    }

    async fn record_provider_balance_history_snapshot(
        &self,
        service_name: &str,
        station_name: &str,
        upstream_index: usize,
        snapshot: ProviderBalanceSnapshot,
    ) {
        let mut guard = self.provider_balance_history.write().await;
        let history = guard
            .entry(service_name.to_string())
            .or_default()
            .entry(station_name.to_string())
            .or_default()
            .entry(upstream_index)
            .or_default()
            .entry(snapshot.provider_id.clone())
            .or_default();
        if history
            .back()
            .is_some_and(|previous| previous.fetched_at_ms == snapshot.fetched_at_ms)
        {
            history.pop_back();
        }
        history.push_back(snapshot);
        let max = provider_balance_history_max();
        while history.len() > max {
            history.pop_front();
        }
    }

    pub async fn get_provider_balance_view(
        &self,
        service_name: &str,
    ) -> HashMap<String, Vec<ProviderBalanceSnapshot>> {
        let now_ms = unix_now_ms();
        let guard = self.provider_balances.read().await;
        let Some(per_service) = guard.get(service_name) else {
            return HashMap::new();
        };

        per_service
            .iter()
            .map(|(station_name, upstreams)| {
                let mut snapshots = upstreams
                    .values()
                    .flat_map(|providers| providers.values().cloned())
                    .collect::<Vec<_>>();
                for snapshot in &mut snapshots {
                    snapshot.refresh_status(now_ms);
                }
                snapshots.sort_by(|a, b| {
                    a.upstream_index
                        .cmp(&b.upstream_index)
                        .then_with(|| a.provider_id.cmp(&b.provider_id))
                });
                (station_name.clone(), snapshots)
            })
            .collect()
    }

    pub async fn get_provider_balance_history_view(
        &self,
        service_name: &str,
    ) -> HashMap<String, Vec<ProviderBalanceSnapshot>> {
        let now_ms = unix_now_ms();
        let guard = self.provider_balance_history.read().await;
        let Some(per_service) = guard.get(service_name) else {
            return HashMap::new();
        };

        per_service
            .iter()
            .map(|(station_name, upstreams)| {
                let mut snapshots = upstreams
                    .values()
                    .flat_map(|providers| providers.values())
                    .flat_map(|history| history.iter().cloned())
                    .collect::<Vec<_>>();
                for snapshot in &mut snapshots {
                    snapshot.refresh_status(now_ms);
                }
                snapshots.sort_by(|a, b| {
                    a.fetched_at_ms
                        .cmp(&b.fetched_at_ms)
                        .then_with(|| a.upstream_index.cmp(&b.upstream_index))
                        .then_with(|| a.provider_id.cmp(&b.provider_id))
                });
                (station_name.clone(), snapshots)
            })
            .collect()
    }

    pub async fn get_provider_balance_summary_view(
        &self,
        service_name: &str,
    ) -> HashMap<String, StationRoutingBalanceSummary> {
        let guard = self.provider_balance_summaries.read().await;
        let Some(per_service) = guard.get(service_name) else {
            return HashMap::new();
        };

        per_service.clone()
    }

    pub async fn record_passive_upstream_success(
        &self,
        service_name: &str,
        station_name: &str,
        base_url: &str,
        status_code: Option<u16>,
        now_ms: u64,
    ) {
        let mut guard = self.passive_station_health.write().await;
        let entry = guard
            .entry(service_name.to_string())
            .or_default()
            .entry(station_name.to_string())
            .or_default()
            .entry(base_url.to_string())
            .or_default();
        entry.record_success(now_ms, status_code);
        self.notify_state_changed();
    }

    pub async fn record_passive_upstream_failure(&self, params: PassiveUpstreamFailureRecord) {
        let PassiveUpstreamFailureRecord {
            service_name,
            station_name,
            base_url,
            status_code,
            error_class,
            error,
            now_ms,
        } = params;

        let mut guard = self.passive_station_health.write().await;
        let entry = guard
            .entry(service_name)
            .or_default()
            .entry(station_name)
            .or_default()
            .entry(base_url)
            .or_default();
        entry.record_failure(now_ms, status_code, error_class, error);
        self.notify_state_changed();
    }

    pub async fn get_lb_view(&self) -> HashMap<String, LbConfigView> {
        let Some(lb_states) = self.lb_states.as_ref() else {
            return HashMap::new();
        };
        let mut map = match lb_states.lock() {
            Ok(m) => m,
            Err(e) => e.into_inner(),
        };

        let now = std::time::Instant::now();
        let mut out = HashMap::new();
        for (cfg_name, st) in map.iter_mut() {
            let len = st
                .failure_counts
                .len()
                .max(st.cooldown_until.len())
                .max(st.usage_exhausted.len());
            if len == 0 {
                continue;
            }

            // 如果结构变化导致长度不一致，做一次对齐，避免 UI 读到越界/脏数据。
            if st.failure_counts.len() != len {
                st.failure_counts.resize(len, 0);
            }
            if st.cooldown_until.len() != len {
                st.cooldown_until.resize(len, None);
            }
            if st.usage_exhausted.len() != len {
                st.usage_exhausted.resize(len, false);
            }

            let mut upstreams = Vec::with_capacity(len);
            for idx in 0..len {
                let failure_count = st.failure_counts.get(idx).copied().unwrap_or(0);
                let cooldown_remaining_secs = st
                    .cooldown_until
                    .get(idx)
                    .and_then(|v| *v)
                    .map(|until| until.saturating_duration_since(now).as_secs())
                    .filter(|&s| s > 0);
                let usage_exhausted = st.usage_exhausted.get(idx).copied().unwrap_or(false);
                upstreams.push(LbUpstreamView {
                    failure_count,
                    cooldown_remaining_secs,
                    usage_exhausted,
                });
            }

            out.insert(
                cfg_name.clone(),
                LbConfigView {
                    last_good_index: st.last_good_index,
                    upstreams,
                },
            );
        }
        out
    }

    pub async fn list_health_checks(
        &self,
        service_name: &str,
    ) -> HashMap<String, HealthCheckStatus> {
        let guard = self.station_health_checks.read().await;
        guard.get(service_name).cloned().unwrap_or_default()
    }

    pub async fn try_begin_station_health_check(
        &self,
        service_name: &str,
        station_name: &str,
        total: usize,
        now_ms: u64,
    ) -> bool {
        let mut guard = self.station_health_checks.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        if let Some(existing) = per_service.get(station_name)
            && !existing.done
        {
            return false;
        }
        per_service.insert(
            station_name.to_string(),
            HealthCheckStatus {
                started_at_ms: now_ms,
                updated_at_ms: now_ms,
                total: total.min(u32::MAX as usize) as u32,
                completed: 0,
                ok: 0,
                err: 0,
                cancel_requested: false,
                canceled: false,
                done: false,
                last_error: None,
            },
        );
        self.notify_state_changed();
        true
    }

    pub async fn request_cancel_station_health_check(
        &self,
        service_name: &str,
        station_name: &str,
        now_ms: u64,
    ) -> bool {
        let mut guard = self.station_health_checks.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return false;
        };
        let Some(st) = per_service.get_mut(station_name) else {
            return false;
        };
        if st.done {
            return false;
        }
        st.cancel_requested = true;
        st.updated_at_ms = now_ms;
        self.notify_state_changed();
        true
    }

    pub async fn is_station_health_check_cancel_requested(
        &self,
        service_name: &str,
        station_name: &str,
    ) -> bool {
        let guard = self.station_health_checks.read().await;
        guard
            .get(service_name)
            .and_then(|m| m.get(station_name))
            .is_some_and(|s| s.cancel_requested && !s.done)
    }

    pub async fn record_station_health_check_result(
        &self,
        service_name: &str,
        station_name: &str,
        now_ms: u64,
        upstream: UpstreamHealth,
    ) {
        {
            let mut guard = self.station_health.write().await;
            let per_service = guard.entry(service_name.to_string()).or_default();
            let entry = per_service
                .entry(station_name.to_string())
                .or_insert_with(|| StationHealth {
                    checked_at_ms: now_ms,
                    upstreams: Vec::new(),
                });
            entry.checked_at_ms = entry.checked_at_ms.max(now_ms);
            entry.upstreams.push(upstream.clone());
            if entry.upstreams.len() > Self::MAX_HEALTH_RECORDS_PER_STATION {
                let extra = entry
                    .upstreams
                    .len()
                    .saturating_sub(Self::MAX_HEALTH_RECORDS_PER_STATION);
                if extra > 0 {
                    entry.upstreams.drain(0..extra);
                }
            }
        }

        let mut guard = self.station_health_checks.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        let st = per_service.entry(station_name.to_string()).or_default();
        st.updated_at_ms = now_ms;
        st.completed = st.completed.saturating_add(1);
        match upstream.ok {
            Some(true) => st.ok = st.ok.saturating_add(1),
            Some(false) => {
                st.err = st.err.saturating_add(1);
                if st.last_error.is_none() {
                    st.last_error = upstream.error.clone();
                }
            }
            None => {}
        }
        self.notify_state_changed();
    }

    pub async fn finish_station_health_check(
        &self,
        service_name: &str,
        station_name: &str,
        now_ms: u64,
        canceled: bool,
    ) {
        let mut guard = self.station_health_checks.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        let st = per_service.entry(station_name.to_string()).or_default();
        st.updated_at_ms = now_ms;
        st.canceled = canceled;
        st.done = true;
        self.notify_state_changed();
    }

    pub async fn get_usage_rollup_view(
        &self,
        service_name: &str,
        top_n: usize,
        days: usize,
    ) -> UsageRollupView {
        let guard = self.usage_rollups.read().await;
        let Some(rollup) = guard.get(service_name) else {
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

        let mut by_config =
            aggregate_entity_window(&rollup.by_config_day, start_day, end_day, top_n);
        if all_loaded && by_config.is_empty() {
            by_config = rollup
                .by_config
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>();
            by_config.sort_by(|(left_name, left), (right_name, right)| {
                right
                    .usage
                    .total_tokens
                    .cmp(&left.usage.total_tokens)
                    .then_with(|| right.requests_total.cmp(&left.requests_total))
                    .then_with(|| left_name.cmp(right_name))
            });
            by_config.truncate(top_n);
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

        let mut by_config_day = HashMap::new();
        for (name, _) in &by_config {
            let series = rollup
                .by_config_day
                .get(name)
                .map(|m| match (all_loaded, start_day, end_day) {
                    (true, _, _) => sorted_day_series(m),
                    (false, Some(start), Some(end)) => filled_day_series(m, start, end),
                    _ => Vec::new(),
                })
                .unwrap_or_default();
            by_config_day.insert(name.clone(), series);
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
            by_config,
            by_config_day,
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
        let day = usage_day::current_local_day();
        let window = usage_day::local_day_window(day).unwrap_or(usage_day::UsageDayWindow {
            day,
            start_ms: 0,
            end_ms: 0,
        });

        let guard = self.usage_rollups.read().await;
        let Some(rollup) = guard.get(service_name) else {
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
            station_rows: usage_day_dimension_rows(&rollup.by_config_day, day, top_n),
            model_rows: usage_day_dimension_rows(&rollup.by_model_day, day, top_n),
            session_rows: usage_day_dimension_rows(&rollup.by_session_day, day, top_n),
            project_rows: usage_day_dimension_rows(&rollup.by_project_day, day, top_n),
            coverage: usage_day_coverage(rollup, window),
            ..UsageDayView::default()
        }
    }

    pub async fn replay_usage_from_requests_log(
        &self,
        service_name: &str,
        log_path: PathBuf,
        base_url_to_provider_id: HashMap<String, String>,
    ) -> usize {
        let enabled = std::env::var("CODEX_HELPER_USAGE_REPLAY_ON_STARTUP")
            .ok()
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "y" | "on"
                )
            })
            .unwrap_or(true);
        if !enabled {
            return 0;
        }

        let already_has_data = {
            let guard = self.usage_rollups.read().await;
            guard
                .get(service_name)
                .is_some_and(|r| r.loaded.requests_total > 0)
        };
        if already_has_data {
            return 0;
        }

        if !log_path.exists() {
            return 0;
        }

        let max_bytes = std::env::var("CODEX_HELPER_USAGE_REPLAY_MAX_BYTES")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(8 * 1024 * 1024);
        let max_lines = std::env::var("CODEX_HELPER_USAGE_REPLAY_MAX_LINES")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(20_000);

        let mut file = match std::fs::File::open(&log_path) {
            Ok(f) => f,
            Err(_) => return 0,
        };
        let len: u64 = file.metadata().map(|m| m.len()).unwrap_or_default();
        let start = len.saturating_sub(max_bytes as u64);
        let bytes_truncated = start > 0;
        if file.seek(SeekFrom::Start(start)).is_err() {
            return 0;
        }
        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).is_err() {
            return 0;
        }
        if start > 0 {
            if let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                buf = buf[pos + 1..].to_vec();
            } else {
                return 0;
            }
        }

        let text = match std::str::from_utf8(&buf) {
            Ok(s) => s,
            Err(_) => return 0,
        };
        let lines = text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>();
        let start_idx = lines.len().saturating_sub(max_lines);
        let scanned_lines = lines.len().saturating_sub(start_idx);
        let lines_truncated = start_idx > 0;

        let mut requests = Vec::new();
        for line in &lines[start_idx..] {
            let Ok(v) = serde_json::from_str::<JsonValue>(line) else {
                continue;
            };
            let Some(svc) = v.get("service").and_then(|x| x.as_str()) else {
                continue;
            };
            if svc != service_name {
                continue;
            }

            let Some(mut request) =
                crate::request_ledger::finished_request_from_request_log_record(&v)
            else {
                continue;
            };
            if request
                .provider_id
                .as_deref()
                .map(str::trim)
                .filter(|provider_id| !provider_id.is_empty())
                .is_none()
                && let Some(provider_id) = request
                    .upstream_base_url
                    .as_deref()
                    .and_then(|base_url| base_url_to_provider_id.get(base_url))
            {
                request.provider_id = Some(provider_id.clone());
            }
            requests.push(request);
        }

        if requests.is_empty() {
            return 0;
        }

        let mut guard = self.usage_rollups.write().await;
        let rollup = guard.entry(service_name.to_string()).or_default();
        rollup.coverage_source = "request_log".to_string();
        rollup.replay_scanned_lines = scanned_lines;
        rollup.replay_max_lines = max_lines;
        rollup.replay_max_bytes = max_bytes;
        rollup.replay_bytes_truncated = bytes_truncated;
        rollup.replay_lines_truncated = lines_truncated;
        let mut replayed = 0;
        for request in &requests {
            if record_finished_request_into_usage_rollup(rollup, request) {
                replayed += 1;
            }
        }

        self.notify_state_changed();
        replayed
    }

    pub async fn resolve_session_cwd(&self, session_id: &str) -> Option<String> {
        if self.session_cwd_cache_max_entries == 0 {
            return sessions::find_codex_session_cwd_by_id(session_id)
                .await
                .ok()
                .flatten();
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        {
            let guard = self.session_cwd_cache.read().await;
            if let Some(v) = guard.get(session_id) {
                let out = v.cwd.clone();
                drop(guard);
                let mut guard = self.session_cwd_cache.write().await;
                if let Some(v) = guard.get_mut(session_id) {
                    v.last_seen_ms = now_ms;
                }
                return out;
            }
        }

        // Cache miss: resolve from disk and record last_seen.

        let resolved = sessions::find_codex_session_cwd_by_id(session_id)
            .await
            .ok()
            .flatten();

        let mut guard = self.session_cwd_cache.write().await;
        guard.insert(
            session_id.to_string(),
            SessionCwdCacheEntry {
                cwd: resolved.clone(),
                last_seen_ms: now_ms,
            },
        );
        resolved
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn begin_request(
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
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let trace_id = Some(crate::logging::request_trace_id(service, id));
        let req = ActiveRequest {
            id,
            trace_id,
            session_id,
            session_identity_source,
            client_name,
            client_addr,
            cwd,
            model,
            reasoning_effort,
            service_tier,
            station_name: None,
            provider_id: None,
            upstream_base_url: None,
            route_decision: None,
            service: service.to_string(),
            method: method.to_string(),
            path: path.to_string(),
            started_at_ms,
        };
        let mut guard = self.active_requests.write().await;
        guard.insert(id, req);
        self.notify_state_changed();
        id
    }

    #[cfg(test)]
    pub(crate) fn begin_request_for_test(&self) -> BeginRequestTestBuilder<'_> {
        BeginRequestTestBuilder::new(self)
    }

    pub async fn update_request_route(
        &self,
        request_id: u64,
        station_name: Option<String>,
        provider_id: Option<String>,
        upstream_base_url: String,
        route_decision: Option<RouteDecisionProvenance>,
    ) {
        let mut guard = self.active_requests.write().await;
        let Some(req) = guard.get_mut(&request_id) else {
            return;
        };
        req.station_name = station_name;
        req.provider_id = provider_id;
        req.upstream_base_url = Some(upstream_base_url);
        req.route_decision = route_decision;
        self.notify_state_changed();
    }

    pub async fn finish_request(&self, params: FinishRequestParams) -> bool {
        let mut active = self.active_requests.write().await;
        let Some(req) = active.remove(&params.id) else {
            return false;
        };
        drop(active);

        let pricing_model = req
            .route_decision
            .as_ref()
            .and_then(|decision| decision.effective_model.as_ref())
            .map(|value| value.value.as_str())
            .or(req.model.as_deref());
        let cost = estimate_request_cost_from_operator_catalog_for_service(
            pricing_model,
            params.usage.as_ref(),
            CostAdjustments::default(),
            &req.service,
        );

        let mut finished = FinishedRequest {
            id: params.id,
            trace_id: req.trace_id,
            session_id: req.session_id,
            session_identity_source: req.session_identity_source,
            client_name: req.client_name,
            client_addr: req.client_addr,
            cwd: req.cwd,
            model: req.model,
            reasoning_effort: req.reasoning_effort,
            service_tier: params.observed_service_tier.or(req.service_tier),
            station_name: req.station_name,
            provider_id: req.provider_id,
            upstream_base_url: req.upstream_base_url,
            route_decision: req.route_decision,
            usage: params.usage.clone(),
            cost,
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
        finished.refresh_observability();

        {
            let mut rollups = self.usage_rollups.write().await;
            let rollup = rollups.entry(finished.service.clone()).or_default();
            record_finished_request_into_usage_rollup(rollup, &finished);
        }

        if let Some(sid) = finished.session_id.as_deref() {
            let mut stats = self.session_stats.write().await;
            let entry = stats.entry(sid.to_string()).or_default();
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
            entry.last_station_name = finished
                .station_name
                .clone()
                .or(entry.last_station_name.clone());
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

        let mut recent = self.recent_finished.write().await;
        recent.push_front(finished);
        while recent.len() > recent_finished_max() {
            recent.pop_back();
        }
        self.notify_state_changed();
        true
    }

    pub async fn list_active_requests(&self) -> Vec<ActiveRequest> {
        let guard = self.active_requests.read().await;
        let mut vec = guard.values().cloned().collect::<Vec<_>>();
        vec.sort_by_key(|r| r.started_at_ms);
        vec
    }

    pub async fn list_recent_finished(&self, limit: usize) -> Vec<FinishedRequest> {
        let guard = self.recent_finished.read().await;
        guard.iter().take(limit).cloned().collect()
    }

    pub async fn list_session_stats(&self) -> HashMap<String, SessionStats> {
        let guard = self.session_stats.read().await;
        guard.clone()
    }

    pub async fn list_session_identity_cards(
        &self,
        recent_limit: usize,
    ) -> Vec<SessionIdentityCard> {
        let recent_limit = recent_limit.clamp(1, recent_finished_max());
        let (
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
        ) = tokio::join!(
            self.list_active_requests(),
            self.list_recent_finished(recent_limit),
            self.list_session_effort_overrides(),
            self.list_session_station_overrides(),
            self.list_session_model_overrides(),
            self.list_session_service_tier_overrides(),
            self.list_session_bindings(),
            self.list_session_route_affinities(),
            self.get_global_station_override(),
            self.list_session_stats(),
        );
        build_session_identity_cards_from_parts(SessionIdentityCardBuildInputs {
            active: &active,
            recent: &recent,
            overrides: &overrides,
            station_overrides: &station_overrides,
            model_overrides: &model_overrides,
            service_tier_overrides: &service_tier_overrides,
            bindings: &bindings,
            route_affinities: &route_affinities,
            global_station_override: global_station_override.as_deref(),
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
        recent_limit: usize,
    ) -> Vec<SessionIdentityCard> {
        let mut cards = self.list_session_identity_cards(recent_limit).await;
        self.enrich_session_identity_cards_with_cached_host_transcripts(&mut cards)
            .await;
        cards
    }

    pub fn spawn_cleanup_task(state: Arc<Self>) {
        // Run periodically; no need to be super frequent.
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(30));
            loop {
                tick.tick().await;
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
        let active = self.active_requests.read().await;
        let mut active_sessions: HashMap<String, ()> = HashMap::new();
        for req in active.values() {
            if let Some(sid) = req.session_id.as_deref() {
                active_sessions.insert(sid.to_string(), ());
            }
        }

        if self.session_override_ttl_ms > 0 && now_ms >= self.session_override_ttl_ms {
            let cutoff_override = now_ms - self.session_override_ttl_ms;
            let mut overrides = self.session_effort_overrides.write().await;
            overrides.retain(|sid, v| {
                if active_sessions.contains_key(sid) {
                    return true;
                }
                v.last_seen_ms >= cutoff_override
            });
        }

        if self.session_override_ttl_ms > 0 && now_ms >= self.session_override_ttl_ms {
            let cutoff_override = now_ms - self.session_override_ttl_ms;
            let mut overrides = self.session_station_overrides.write().await;
            overrides.retain(|sid, v| {
                if active_sessions.contains_key(sid) {
                    return true;
                }
                v.last_seen_ms >= cutoff_override
            });
        }

        if self.session_override_ttl_ms > 0 && now_ms >= self.session_override_ttl_ms {
            let cutoff_override = now_ms - self.session_override_ttl_ms;
            let mut overrides = self.session_route_target_overrides.write().await;
            overrides.retain(|sid, v| {
                if active_sessions.contains_key(sid) {
                    return true;
                }
                v.last_seen_ms >= cutoff_override
            });
        }

        if self.session_override_ttl_ms > 0 && now_ms >= self.session_override_ttl_ms {
            let cutoff_override = now_ms - self.session_override_ttl_ms;
            let mut overrides = self.session_model_overrides.write().await;
            overrides.retain(|sid, v| {
                if active_sessions.contains_key(sid) {
                    return true;
                }
                v.last_seen_ms >= cutoff_override
            });
        }

        if self.session_override_ttl_ms > 0 && now_ms >= self.session_override_ttl_ms {
            let cutoff_override = now_ms - self.session_override_ttl_ms;
            let mut overrides = self.session_service_tier_overrides.write().await;
            overrides.retain(|sid, v| {
                if active_sessions.contains_key(sid) {
                    return true;
                }
                v.last_seen_ms >= cutoff_override
            });
        }

        if self.session_binding_ttl_ms > 0 && now_ms >= self.session_binding_ttl_ms {
            let cutoff_binding = now_ms - self.session_binding_ttl_ms;
            let mut bindings = self.session_bindings.write().await;
            bindings.retain(|sid, entry| {
                if active_sessions.contains_key(sid) {
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
                    .filter(|(sid, _)| !active_sessions.contains_key(*sid))
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
            let mut affinities = self.session_route_affinities.write().await;
            if self.session_route_affinity_ttl_ms > 0
                && now_ms >= self.session_route_affinity_ttl_ms
            {
                let cutoff_affinity = now_ms - self.session_route_affinity_ttl_ms;
                affinities.retain(|sid, affinity| {
                    active_sessions.contains_key(sid)
                        || affinity.last_selected_at_ms >= cutoff_affinity
                });
            }
            prune_lru_cache(
                &mut affinities,
                self.session_route_affinity_max_entries,
                |entry| entry.last_selected_at_ms,
            );
        }

        // Keep a bounded number of days of rollup data to avoid unbounded growth.
        let keep_days: i32 = std::env::var("CODEX_HELPER_USAGE_ROLLUP_KEEP_DAYS")
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(60);
        let now_day = usage_day::local_day_from_ms(now_ms);
        let cutoff_day = now_day.saturating_sub(keep_days);
        let mut rollups = self.usage_rollups.write().await;
        for rollup in rollups.values_mut() {
            rollup.recorded_requests.retain(|_, day| *day >= cutoff_day);
            rollup.by_day.retain(|day, _| *day >= cutoff_day);
            rollup.by_hour.retain(|day, _| *day >= cutoff_day);
            prune_usage_entity_days(&mut rollup.by_config_day, cutoff_day);
            prune_usage_entity_days(&mut rollup.by_provider_day, cutoff_day);
            prune_usage_entity_days(&mut rollup.by_model_day, cutoff_day);
            prune_usage_entity_days(&mut rollup.by_session_day, cutoff_day);
            prune_usage_entity_days(&mut rollup.by_project_day, cutoff_day);
        }

        let cutoff_cwd =
            if self.session_cwd_cache_ttl_ms == 0 || now_ms < self.session_cwd_cache_ttl_ms {
                0
            } else {
                now_ms - self.session_cwd_cache_ttl_ms
            };
        self.prune_session_cwd_cache(&active_sessions, cutoff_cwd)
            .await;
        self.prune_session_transcript_path_cache(now_ms).await;

        if self.session_override_ttl_ms > 0 && now_ms >= self.session_override_ttl_ms {
            let cutoff_stats = now_ms - self.session_override_ttl_ms;
            let mut stats = self.session_stats.write().await;
            stats.retain(|sid, v| {
                active_sessions.contains_key(sid) || v.last_seen_ms >= cutoff_stats
            });
        }
    }

    async fn prune_session_cwd_cache(&self, active_sessions: &HashMap<String, ()>, cutoff: u64) {
        if self.session_cwd_cache_max_entries == 0 {
            return;
        }
        let mut cache = self.session_cwd_cache.write().await;

        if self.session_cwd_cache_ttl_ms > 0 {
            cache.retain(|sid, v| {
                if active_sessions.contains_key(sid) {
                    return true;
                }
                v.last_seen_ms >= cutoff
            });
        }

        let max = self.session_cwd_cache_max_entries;
        if max == 0 || cache.len() <= max {
            return;
        }

        // Drop least-recently-seen entries first.
        let mut keys = cache
            .iter()
            .map(|(sid, v)| (sid.clone(), v.last_seen_ms))
            .collect::<Vec<_>>();
        keys.sort_by_key(|(_, t)| *t);
        let remove_count = keys.len().saturating_sub(max);
        for (sid, _) in keys.into_iter().take(remove_count) {
            cache.remove(&sid);
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

    use crate::config::{ServiceConfig, ServiceConfigManager, UpstreamAuth, UpstreamConfig};
    use crate::runtime_identity::ProviderEndpointKey;
    use std::path::Path;
    use std::sync::OnceLock;

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
        session_override_ttl_ms: u64,
        session_binding_ttl_ms: u64,
        session_binding_max_entries: usize,
    ) -> RuntimePolicy {
        RuntimePolicy {
            session_override_ttl_ms,
            session_binding_ttl_ms,
            session_binding_max_entries,
            session_route_affinity_ttl_ms: 0,
            session_route_affinity_max_entries: 5_000,
            session_route_affinity_store: SessionRouteAffinityStore::from_env(),
            session_cwd_cache_ttl_ms: 0,
            session_cwd_cache_max_entries: 0,
            session_transcript_path_cache_ttl_ms: 30_000,
            session_transcript_path_cache_max_entries: 5_000,
        }
    }

    fn temp_state_log_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "codex-helper-state-{test_name}-{}-{}.jsonl",
            std::process::id(),
            unix_now_ms()
        ))
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
            assert_eq!(active[0].trace_id.as_deref(), Some("codex-1"));

            state
                .finish_request(FinishRequestParams {
                    id: request_id,
                    status_code: 200,
                    duration_ms: 10,
                    ended_at_ms: 110,
                    observed_service_tier: Some("priority".to_string()),
                    usage: None,
                    retry: None,
                    ttfb_ms: Some(4),
                    streaming: false,
                })
                .await;

            let recent = state.list_recent_finished(1).await;
            assert_eq!(recent[0].trace_id.as_deref(), Some("codex-1"));
            assert_eq!(recent[0].observability.trace_id.as_deref(), Some("codex-1"));
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
                    status_code: 200,
                    duration_ms: 10,
                    ended_at_ms: 110,
                    observed_service_tier: None,
                    usage: None,
                    retry: None,
                    ttfb_ms: Some(4),
                    streaming: false,
                })
                .await;
            let second = state
                .finish_request(FinishRequestParams {
                    id: request_id,
                    status_code: 500,
                    duration_ms: 20,
                    ended_at_ms: 120,
                    observed_service_tier: None,
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
                    status_code: 200,
                    duration_ms: 1_500,
                    ended_at_ms: 1_600,
                    observed_service_tier: None,
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
                    status_code: 200,
                    duration_ms: 2_500,
                    ended_at_ms: 4_500,
                    observed_service_tier: None,
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

            let stats = state.list_session_stats().await;
            let stats = stats.get("sid-speed").expect("session stats");
            assert_eq!(stats.last_output_tokens_per_second, Some(150.0));
            assert_eq!(stats.avg_output_tokens_per_second, Some(500.0 / 3.0));

            let cards = state.list_session_identity_cards(16).await;
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
                .record_provider_balance_snapshot(
                    "codex",
                    ProviderBalanceSnapshot {
                        provider_id: "input6".to_string(),
                        station_name: Some("routing".to_string()),
                        upstream_index: Some(0),
                        fetched_at_ms: 100,
                        stale_after_ms: Some(1_000),
                        status: BalanceSnapshotStatus::Ok,
                        total_balance_usd: Some("12.50".to_string()),
                        ..ProviderBalanceSnapshot::default()
                    },
                )
                .await;
            changes.changed().await.expect("balance change");
            let balance_version = *changes.borrow();
            assert!(balance_version > 0);

            state
                .set_provider_endpoint_usage_exhausted(
                    "codex",
                    ProviderEndpointKey::new("codex", "input6", "default"),
                    true,
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
                    status_code: 200,
                    duration_ms: 25,
                    ended_at_ms: 250,
                    observed_service_tier: None,
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
    fn state_change_subscription_ignores_session_touch_only_updates() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let mut changes = state.subscribe_state_changes();

            state
                .set_session_model_override("sid-1".to_string(), "gpt-5".to_string(), 100)
                .await;
            changes.changed().await.expect("model override change");
            let version = *changes.borrow();

            state.touch_session_model_override("sid-1", 200).await;
            assert_eq!(*changes.borrow(), version);
        });
    }

    #[test]
    fn finish_request_estimates_cost_and_rolls_up_cost() {
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
                    status_code: 200,
                    duration_ms: 10,
                    ended_at_ms: 110,
                    observed_service_tier: None,
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

            let recent = state.list_recent_finished(1).await;
            assert_eq!(recent[0].cost.total_cost_usd.as_deref(), Some("0.0061375"));

            let rollup = state.get_usage_rollup_view("codex", 12, 1).await;
            assert_eq!(
                rollup.loaded.cost.total_cost_usd.as_deref(),
                Some("0.0061375")
            );
            assert_eq!(rollup.loaded.cost.priced_requests, 1);
            assert_eq!(rollup.loaded.cost.unpriced_requests, 0);
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
                    Some("station-day".to_string()),
                    Some("provider-day".to_string()),
                    "https://provider.example/v1".to_string(),
                    None,
                )
                .await;
            state
                .finish_request(FinishRequestParams {
                    id: request_id,
                    status_code: 200,
                    duration_ms: 10,
                    ended_at_ms,
                    observed_service_tier: None,
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
            let mut rollups = state.usage_rollups.write().await;
            let rollup = rollups.get_mut("codex").expect("codex rollup");

            assert_eq!(rollup.by_hour[&day][hour].requests_total, 1);
            assert_eq!(rollup.by_model["gpt-5"].requests_total, 1);
            assert_eq!(rollup.by_model_day["gpt-5"][&day].usage.total_tokens, 42);
            assert_eq!(rollup.by_session["sid-day"].requests_total, 1);
            assert_eq!(
                rollup.by_project["F:/SourceCodes/Rust/codex-helper"].requests_total,
                1
            );

            let before = rollup.loaded.requests_total;
            assert!(!record_finished_request_into_usage_rollup(
                rollup, &recent[0]
            ));
            assert_eq!(rollup.loaded.requests_total, before);
        });
    }

    #[test]
    fn usage_replay_projects_finished_requests_and_keeps_provider_fallback() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let log_path = temp_state_log_path("usage-replay-projection");
            let window =
                usage_day::local_day_window(usage_day::current_local_day()).expect("window");
            let ended_at_ms = window.start_ms.saturating_add(3_600_000);
            let record = serde_json::json!({
                "timestamp_ms": ended_at_ms,
                "request_id": 77,
                "trace_id": "codex-replay-77",
                "service": "codex",
                "method": "POST",
                "path": "/v1/responses",
                "status_code": 200,
                "duration_ms": 100,
                "ttfb_ms": 25,
                "station_name": "station-replay",
                "upstream_base_url": "https://legacy.example/v1",
                "session_id": "sid-replay",
                "cwd": "F:/SourceCodes/Rust/codex-helper",
                "model": "gpt-5",
                "usage": {
                    "input_tokens": 1,
                    "output_tokens": 2,
                    "total_tokens": 3
                }
            });
            std::fs::write(&log_path, format!("{record}\n")).expect("write request log");

            let state = ProxyState::new();
            let replayed = state
                .replay_usage_from_requests_log(
                    "codex",
                    log_path.clone(),
                    HashMap::from([(
                        "https://legacy.example/v1".to_string(),
                        "provider-fallback".to_string(),
                    )]),
                )
                .await;

            assert_eq!(replayed, 1);
            let day = usage_day::local_day_from_ms(ended_at_ms);
            let rollups = state.usage_rollups.read().await;
            let rollup = rollups.get("codex").expect("codex rollup");
            assert_eq!(rollup.by_provider["provider-fallback"].requests_total, 1);
            assert_eq!(
                rollup.by_provider_day["provider-fallback"][&day]
                    .usage
                    .total_tokens,
                3
            );
            assert_eq!(rollup.by_model["gpt-5"].requests_total, 1);
            assert_eq!(rollup.by_session["sid-replay"].requests_total, 1);
            assert_eq!(
                rollup.by_project["F:/SourceCodes/Rust/codex-helper"].requests_total,
                1
            );

            let day_view = state.get_usage_day_view("codex", 12, ended_at_ms).await;
            assert_eq!(day_view.coverage.source, "request_log");
            assert_eq!(day_view.coverage.loaded_first_ms, Some(ended_at_ms));
            assert!(day_view.coverage.day_may_be_partial);
            assert!(
                day_view
                    .coverage
                    .partial_reason
                    .as_deref()
                    .unwrap_or_default()
                    .contains("loaded data starts after local day start")
            );

            let _ = std::fs::remove_file(log_path);
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
                    Some("station-small".to_string()),
                    Some("provider-small".to_string()),
                    "https://small.example/v1".to_string(),
                    None,
                )
                .await;
            state
                .finish_request(FinishRequestParams {
                    id: small_id,
                    status_code: 200,
                    duration_ms: 50,
                    ended_at_ms: small_ended_at_ms,
                    observed_service_tier: None,
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
                    Some("station-large".to_string()),
                    Some("provider-large".to_string()),
                    "https://large.example/v1".to_string(),
                    None,
                )
                .await;
            state
                .finish_request(FinishRequestParams {
                    id: large_id,
                    status_code: 200,
                    duration_ms: 100,
                    ended_at_ms: large_ended_at_ms,
                    observed_service_tier: None,
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
            assert_eq!(view.station_rows[0].name, "station-large");
            assert_eq!(view.session_rows[0].name, "sid-large");
            assert_eq!(view.project_rows[0].name, "F:/large");
            assert_eq!(view.model_rows[0].name, "gpt-5");
            assert_eq!(view.retry_gate.active, 0);
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
                    Some("old-station".to_string()),
                    Some("old-provider".to_string()),
                    "https://old.example/v1".to_string(),
                    None,
                )
                .await;
            state
                .finish_request(FinishRequestParams {
                    id: old_id,
                    status_code: 200,
                    duration_ms: 20,
                    ended_at_ms: old_ms,
                    observed_service_tier: None,
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
                    Some("fresh-station".to_string()),
                    Some("fresh-provider".to_string()),
                    "https://fresh.example/v1".to_string(),
                    None,
                )
                .await;
            state
                .finish_request(FinishRequestParams {
                    id: fresh_id,
                    status_code: 200,
                    duration_ms: 10,
                    ended_at_ms: now_ms,
                    observed_service_tier: None,
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
            assert_eq!(week.by_config[0].0, "fresh-station");
            assert_eq!(week.by_provider[0].0, "fresh-provider");

            let loaded = state.get_usage_rollup_view("codex", 10, 0).await;
            assert_eq!(loaded.window.requests_total, 2);
            assert_eq!(loaded.by_config[0].0, "old-station");
            assert_eq!(loaded.by_provider[0].0, "old-provider");
        });
    }

    #[test]
    fn build_session_identity_cards_merges_sources_and_sorts_newest_first() {
        let active = vec![ActiveRequest {
            id: 1,
            trace_id: Some("codex-1".to_string()),
            session_id: Some("sid-active".to_string()),
            session_identity_source: Some(SessionIdentitySource::Header),
            client_name: Some("Frank-Laptop".to_string()),
            client_addr: Some("100.64.0.8".to_string()),
            cwd: Some("G:/codes/project".to_string()),
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: Some("priority".to_string()),
            station_name: Some("right".to_string()),
            provider_id: Some("right".to_string()),
            upstream_base_url: Some("https://right.example/v1".to_string()),
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
                station_name: Some("vibe".to_string()),
                provider_id: Some("vibe".to_string()),
                upstream_base_url: Some("https://vibe.example/v1".to_string()),
                route_decision: None,
                usage: Some(UsageMetrics {
                    input_tokens: 1,
                    output_tokens: 2,
                    reasoning_tokens: 3,
                    total_tokens: 6,
                    ..UsageMetrics::default()
                }),
                cost: CostBreakdown::default(),
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
                station_name: Some("right".to_string()),
                provider_id: Some("right".to_string()),
                upstream_base_url: Some("https://right.example/v1".to_string()),
                route_decision: None,
                usage: None,
                cost: CostBreakdown::default(),
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
        let overrides = HashMap::from([("sid-active".to_string(), "xhigh".to_string())]);
        let config_overrides = HashMap::from([("sid-active".to_string(), "temp".to_string())]);
        let model_overrides =
            HashMap::from([("sid-active".to_string(), "gpt-5.4-mini".to_string())]);
        let service_tier_overrides =
            HashMap::from([("sid-active".to_string(), "priority".to_string())]);
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
                last_station_name: Some("right".to_string()),
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
            overrides: &overrides,
            station_overrides: &config_overrides,
            model_overrides: &model_overrides,
            service_tier_overrides: &service_tier_overrides,
            bindings: &HashMap::new(),
            route_affinities: &HashMap::new(),
            global_station_override: None,
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
        assert_eq!(cards[1].override_effort.as_deref(), Some("xhigh"));
        assert_eq!(cards[1].override_station_name.as_deref(), Some("temp"));
        assert_eq!(cards[1].override_model.as_deref(), Some("gpt-5.4-mini"));
        assert_eq!(cards[1].override_service_tier.as_deref(), Some("priority"));
        assert_eq!(
            cards[1]
                .effective_model
                .as_ref()
                .map(|value| value.value.as_str()),
            Some("gpt-5.4-mini")
        );
        assert_eq!(
            cards[1].effective_model.as_ref().map(|value| value.source),
            Some(RouteValueSource::SessionOverride)
        );
        assert_eq!(
            cards[1]
                .effective_reasoning_effort
                .as_ref()
                .map(|value| value.source),
            Some(RouteValueSource::SessionOverride)
        );
        assert_eq!(
            cards[1]
                .effective_service_tier
                .as_ref()
                .map(|value| value.source),
            Some(RouteValueSource::SessionOverride)
        );
        assert_eq!(
            cards[1]
                .effective_station
                .as_ref()
                .map(|value| value.source),
            Some(RouteValueSource::SessionOverride)
        );
        assert!(cards[1].effective_upstream_base_url.is_none());
        assert_eq!(
            cards[1].last_upstream_base_url.as_deref(),
            Some("https://right.example/v1")
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
            trace_id: Some("codex-1".to_string()),
            session_id: Some("sid-bound".to_string()),
            session_identity_source: Some(SessionIdentitySource::Header),
            client_name: Some("Workstation".to_string()),
            client_addr: Some("100.64.0.10".to_string()),
            cwd: None,
            model: Some("gpt-observed".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: Some("default".to_string()),
            station_name: Some("right".to_string()),
            provider_id: None,
            upstream_base_url: None,
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
                station_name: Some("vibe".to_string()),
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
            overrides: &HashMap::new(),
            station_overrides: &HashMap::new(),
            model_overrides: &HashMap::new(),
            service_tier_overrides: &HashMap::new(),
            bindings: &bindings,
            route_affinities: &HashMap::new(),
            global_station_override: None,
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
        assert_eq!(
            cards[0]
                .effective_station
                .as_ref()
                .map(|value| (value.value.as_str(), value.source)),
            Some(("vibe", RouteValueSource::ProfileDefault))
        );
    }

    #[test]
    fn build_session_identity_cards_keeps_request_fields_but_allows_global_config_override() {
        let active = vec![ActiveRequest {
            id: 1,
            trace_id: Some("codex-1".to_string()),
            session_id: Some("sid-bound".to_string()),
            session_identity_source: Some(SessionIdentitySource::Header),
            client_name: Some("Workstation".to_string()),
            client_addr: Some("100.64.0.10".to_string()),
            cwd: None,
            model: Some("gpt-observed".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: Some("default".to_string()),
            station_name: Some("vibe".to_string()),
            provider_id: None,
            upstream_base_url: Some("https://vibe.example/v1".to_string()),
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
                station_name: Some("vibe".to_string()),
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
            overrides: &HashMap::new(),
            station_overrides: &HashMap::new(),
            model_overrides: &HashMap::new(),
            service_tier_overrides: &HashMap::new(),
            bindings: &bindings,
            route_affinities: &HashMap::new(),
            global_station_override: Some("right"),
            stats: &HashMap::new(),
        });

        assert_eq!(cards[0].binding_profile_name.as_deref(), Some("daily"));
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
        assert_eq!(
            cards[0]
                .effective_station
                .as_ref()
                .map(|value| (value.value.as_str(), value.source)),
            Some(("right", RouteValueSource::GlobalOverride))
        );
        assert!(cards[0].effective_upstream_base_url.is_none());
    }

    #[test]
    fn enrich_session_identity_cards_with_runtime_applies_station_mapping_and_single_upstream() {
        let mut cards = vec![SessionIdentityCard {
            session_id: Some("sid-1".to_string()),
            last_model: Some("gpt-5.4".to_string()),
            last_station_name: Some("right".to_string()),
            last_upstream_base_url: Some("https://right.example/v1".to_string()),
            effective_model: Some(ResolvedRouteValue::new(
                "gpt-5.4",
                RouteValueSource::RequestPayload,
            )),
            effective_station: Some(ResolvedRouteValue::new(
                "right",
                RouteValueSource::RuntimeFallback,
            )),
            ..SessionIdentityCard::default()
        }];

        let mut mgr = ServiceConfigManager {
            active: Some("right".to_string()),
            ..ServiceConfigManager::default()
        };
        mgr.configs.insert(
            "right".to_string(),
            ServiceConfig {
                name: "right".to_string(),
                alias: None,
                enabled: true,
                level: 1,
                upstreams: vec![UpstreamConfig {
                    base_url: "https://right.example/v1".to_string(),
                    auth: UpstreamAuth::default(),
                    tags: HashMap::new(),
                    supported_models: HashMap::new(),
                    model_mapping: HashMap::from([(
                        "gpt-5.4".to_string(),
                        "gpt-5.4-fast".to_string(),
                    )]),
                }],
            },
        );

        enrich_session_identity_cards_with_runtime(&mut cards, &mgr);

        assert_eq!(
            cards[0]
                .effective_model
                .as_ref()
                .map(|value| value.value.as_str()),
            Some("gpt-5.4-fast")
        );
        assert_eq!(
            cards[0].effective_model.as_ref().map(|value| value.source),
            Some(RouteValueSource::StationMapping)
        );
        assert_eq!(
            cards[0]
                .effective_upstream_base_url
                .as_ref()
                .map(|value| value.value.as_str()),
            Some("https://right.example/v1")
        );
    }

    #[test]
    fn apply_session_profile_binding_sets_binding_and_clears_manual_overrides() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let now_ms = 42;
            let mut mgr = ServiceConfigManager::default();
            mgr.configs.insert(
                "right".to_string(),
                ServiceConfig {
                    name: "right".to_string(),
                    alias: None,
                    enabled: true,
                    level: 1,
                    upstreams: vec![UpstreamConfig {
                        base_url: "https://right.example/v1".to_string(),
                        auth: UpstreamAuth::default(),
                        tags: HashMap::from([
                            ("supports_reasoning_effort".to_string(), "true".to_string()),
                            ("supports_service_tier".to_string(), "true".to_string()),
                        ]),
                        supported_models: HashMap::from([("gpt-5.4".to_string(), true)]),
                        model_mapping: HashMap::new(),
                    }],
                },
            );
            mgr.profiles.insert(
                "fast".to_string(),
                crate::config::ServiceControlProfile {
                    extends: None,
                    station: Some("right".to_string()),
                    model: Some("gpt-5.4".to_string()),
                    reasoning_effort: Some("low".to_string()),
                    service_tier: Some("flex".to_string()),
                },
            );

            state
                .set_session_station_override("sid-1".to_string(), "other".to_string(), 1)
                .await;
            state
                .set_session_model_override("sid-1".to_string(), "gpt-x".to_string(), 1)
                .await;
            state
                .set_session_effort_override("sid-1".to_string(), "high".to_string(), 1)
                .await;
            state
                .set_session_service_tier_override("sid-1".to_string(), "priority".to_string(), 1)
                .await;

            state
                .apply_session_profile_binding(
                    "codex",
                    &mgr,
                    "sid-1".to_string(),
                    "fast".to_string(),
                    now_ms,
                )
                .await
                .expect("apply profile");

            let binding = state
                .get_session_binding("sid-1")
                .await
                .expect("binding exists");
            assert_eq!(binding.profile_name.as_deref(), Some("fast"));
            assert_eq!(binding.station_name.as_deref(), Some("right"));
            assert_eq!(binding.model.as_deref(), Some("gpt-5.4"));
            assert_eq!(binding.reasoning_effort.as_deref(), Some("low"));
            assert_eq!(binding.service_tier.as_deref(), Some("flex"));
            assert_eq!(
                binding.continuity_mode,
                SessionContinuityMode::ManualProfile
            );
            assert_eq!(binding.updated_at_ms, now_ms);
            assert!(state.get_session_station_override("sid-1").await.is_none());
            assert!(state.get_session_model_override("sid-1").await.is_none());
            assert!(state.get_session_effort_override("sid-1").await.is_none());
            assert!(
                state
                    .get_session_service_tier_override("sid-1")
                    .await
                    .is_none()
            );
        });
    }

    #[test]
    fn list_session_manual_overrides_merges_all_dimensions() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            state
                .set_session_reasoning_effort_override("sid-1".to_string(), "high".to_string(), 1)
                .await;
            state
                .set_session_station_override("sid-1".to_string(), "right".to_string(), 1)
                .await;
            state
                .set_session_model_override("sid-1".to_string(), "gpt-5.4".to_string(), 1)
                .await;
            state
                .set_session_service_tier_override("sid-1".to_string(), "priority".to_string(), 1)
                .await;
            state
                .set_session_model_override("sid-2".to_string(), "gpt-5.4-mini".to_string(), 2)
                .await;

            let merged = state.list_session_manual_overrides().await;
            assert_eq!(merged.len(), 2);
            assert_eq!(
                merged
                    .get("sid-1")
                    .and_then(|overrides| overrides.reasoning_effort.as_deref()),
                Some("high")
            );
            assert_eq!(
                merged
                    .get("sid-1")
                    .and_then(|overrides| overrides.station_name.as_deref()),
                Some("right")
            );
            assert_eq!(
                merged
                    .get("sid-1")
                    .and_then(|overrides| overrides.model.as_deref()),
                Some("gpt-5.4")
            );
            assert_eq!(
                merged
                    .get("sid-1")
                    .and_then(|overrides| overrides.service_tier.as_deref()),
                Some("priority")
            );
            assert_eq!(
                merged
                    .get("sid-2")
                    .and_then(|overrides| overrides.model.as_deref()),
                Some("gpt-5.4-mini")
            );
            assert!(
                merged
                    .get("sid-2")
                    .is_some_and(|overrides| overrides.reasoning_effort.is_none())
            );
        });
    }

    #[test]
    fn provider_balance_snapshots_are_recorded_and_refreshed() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            state
                .record_provider_balance_snapshot(
                    "codex",
                    ProviderBalanceSnapshot {
                        provider_id: "packycode".to_string(),
                        station_name: Some("right".to_string()),
                        upstream_index: Some(2),
                        source: "usage_provider:budget_http_json".to_string(),
                        fetched_at_ms: 10,
                        stale_after_ms: Some(0),
                        stale: false,
                        status: BalanceSnapshotStatus::Unknown,
                        exhausted: Some(false),
                        exhaustion_affects_routing: true,
                        plan_name: None,
                        total_balance_usd: Some("3.5".to_string()),
                        subscription_balance_usd: None,
                        paygo_balance_usd: None,
                        monthly_budget_usd: Some("5".to_string()),
                        monthly_spent_usd: Some("1.5".to_string()),
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
                    },
                )
                .await;

            let view = state.get_provider_balance_view("codex").await;
            let balances = view.get("right").expect("station balance");
            assert_eq!(balances.len(), 1);
            assert_eq!(balances[0].provider_id, "packycode");
            assert_eq!(balances[0].status, BalanceSnapshotStatus::Stale);
            assert_eq!(balances[0].exhausted, Some(false));

            let summary = state
                .get_provider_balance_summary_view("codex")
                .await
                .get("right")
                .cloned()
                .expect("station balance summary");
            assert_eq!(summary.snapshots, 1);
            assert_eq!(summary.stale, 1);
            assert_eq!(summary.routing_snapshots, 1);
        });
    }

    #[test]
    fn provider_balance_history_keeps_recent_refreshes() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            for (fetched_at_ms, remaining) in [(10, "10"), (20, "8")] {
                state
                    .record_provider_balance_snapshot(
                        "codex",
                        ProviderBalanceSnapshot {
                            provider_id: "packycode".to_string(),
                            station_name: Some("right".to_string()),
                            upstream_index: Some(2),
                            source: "usage_provider:test".to_string(),
                            fetched_at_ms,
                            quota_period: Some("daily".to_string()),
                            quota_remaining_usd: Some(remaining.to_string()),
                            quota_limit_usd: Some("20".to_string()),
                            exhausted: Some(false),
                            ..ProviderBalanceSnapshot::default()
                        },
                    )
                    .await;
            }

            let history = state.get_provider_balance_history_view("codex").await;
            let balances = history.get("right").expect("station history");

            assert_eq!(balances.len(), 2);
            assert_eq!(balances[0].quota_remaining_usd.as_deref(), Some("10"));
            assert_eq!(balances[1].quota_remaining_usd.as_deref(), Some("8"));
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
                    .record_provider_balance_snapshot(
                        "codex",
                        ProviderBalanceSnapshot {
                            provider_id: provider_id.to_string(),
                            station_name: Some("routing".to_string()),
                            upstream_index: Some(1),
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
                        },
                    )
                    .await;
            }

            let view = state.get_provider_balance_view("codex").await;
            let balances = view.get("routing").expect("station balance");
            assert_eq!(balances.len(), 2);
            assert_eq!(
                balances
                    .iter()
                    .map(|snapshot| snapshot.provider_id.as_str())
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
            state
                .record_provider_balance_snapshot(
                    "codex",
                    ProviderBalanceSnapshot {
                        provider_id: "input".to_string(),
                        station_name: Some("routing".to_string()),
                        upstream_index: Some(0),
                        source: "usage_provider:sub2api_usage".to_string(),
                        fetched_at_ms: unix_now_ms(),
                        stale_after_ms: None,
                        exhausted: Some(false),
                        quota_period: Some("daily".to_string()),
                        quota_remaining_usd: Some("263.68".to_string()),
                        quota_limit_usd: Some("300.00".to_string()),
                        ..ProviderBalanceSnapshot::default()
                    },
                )
                .await;

            state
                .record_provider_balance_snapshot(
                    "codex",
                    ProviderBalanceSnapshot {
                        provider_id: "input".to_string(),
                        station_name: Some("routing".to_string()),
                        upstream_index: Some(0),
                        source: "usage_provider:openai_balance_http_json".to_string(),
                        fetched_at_ms: unix_now_ms(),
                        stale_after_ms: None,
                        error: Some("usage provider response read failed".to_string()),
                        ..ProviderBalanceSnapshot::default()
                    },
                )
                .await;

            let view = state.get_provider_balance_view("codex").await;
            let snapshot = view
                .get("routing")
                .and_then(|snapshots| snapshots.first())
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

            let mut initial_mgr = ServiceConfigManager::default();
            initial_mgr.configs.insert(
                "routing".to_string(),
                ServiceConfig {
                    name: "routing".to_string(),
                    alias: None,
                    enabled: true,
                    level: 1,
                    upstreams: vec![
                        UpstreamConfig {
                            base_url: "https://input.example/v1".to_string(),
                            auth: UpstreamAuth::default(),
                            tags: HashMap::from([("provider_id".to_string(), "input".to_string())]),
                            supported_models: HashMap::new(),
                            model_mapping: HashMap::new(),
                        },
                        UpstreamConfig {
                            base_url: "https://backup.example/v1".to_string(),
                            auth: UpstreamAuth::default(),
                            tags: HashMap::from([(
                                "provider_id".to_string(),
                                "backup".to_string(),
                            )]),
                            supported_models: HashMap::new(),
                            model_mapping: HashMap::new(),
                        },
                    ],
                },
            );
            state
                .prune_runtime_observability_for_service("codex", &initial_mgr)
                .await;

            state
                .record_provider_balance_snapshot(
                    "codex",
                    ProviderBalanceSnapshot {
                        provider_id: "input".to_string(),
                        station_name: Some("input".to_string()),
                        upstream_index: Some(0),
                        source: "usage_provider:test".to_string(),
                        fetched_at_ms: 10,
                        stale_after_ms: None,
                        stale: false,
                        status: BalanceSnapshotStatus::Ok,
                        exhausted: Some(false),
                        exhaustion_affects_routing: true,
                        total_balance_usd: Some("3.5".to_string()),
                        ..ProviderBalanceSnapshot::default()
                    },
                )
                .await;
            state
                .record_provider_balance_snapshot(
                    "codex",
                    ProviderBalanceSnapshot {
                        provider_id: "input".to_string(),
                        station_name: Some("routing".to_string()),
                        upstream_index: Some(0),
                        source: "usage_provider:test".to_string(),
                        fetched_at_ms: 10,
                        stale_after_ms: None,
                        stale: false,
                        status: BalanceSnapshotStatus::Ok,
                        exhausted: Some(false),
                        exhaustion_affects_routing: true,
                        total_balance_usd: Some("3.5".to_string()),
                        ..ProviderBalanceSnapshot::default()
                    },
                )
                .await;

            let mut pinned_mgr = ServiceConfigManager::default();
            pinned_mgr.configs.insert(
                "routing".to_string(),
                ServiceConfig {
                    name: "routing".to_string(),
                    alias: None,
                    enabled: true,
                    level: 1,
                    upstreams: vec![UpstreamConfig {
                        base_url: "https://input.example/v1".to_string(),
                        auth: UpstreamAuth::default(),
                        tags: HashMap::from([("provider_id".to_string(), "input".to_string())]),
                        supported_models: HashMap::new(),
                        model_mapping: HashMap::new(),
                    }],
                },
            );
            state
                .prune_runtime_observability_for_service("codex", &pinned_mgr)
                .await;

            let view = state.get_provider_balance_view("codex").await;
            assert!(view.contains_key("input"));
            assert!(!view.contains_key("routing"));
        });
    }

    #[test]
    fn prune_runtime_observability_removes_stale_service_keys() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();

            state
                .set_station_enabled_override("codex", "old".to_string(), false, 1)
                .await;
            state
                .set_upstream_enabled_override(
                    "codex",
                    "https://old.example/v1".to_string(),
                    false,
                    1,
                )
                .await;
            state
                .set_provider_endpoint_enabled_override(
                    "codex",
                    ProviderEndpointKey::new("codex", "provider-old", "default"),
                    false,
                    1,
                )
                .await;
            state
                .set_provider_endpoint_runtime_state_override(
                    "codex",
                    ProviderEndpointKey::new("codex", "provider-old", "default"),
                    RuntimeConfigState::BreakerOpen,
                    1,
                )
                .await;
            state
                .record_station_health(
                    "codex",
                    "old".to_string(),
                    StationHealth {
                        checked_at_ms: 10,
                        upstreams: vec![UpstreamHealth {
                            base_url: "https://old.example/v1".to_string(),
                            ok: Some(false),
                            status_code: Some(500),
                            latency_ms: Some(10),
                            error: Some("boom".to_string()),
                            passive: None,
                        }],
                    },
                )
                .await;
            state
                .record_passive_upstream_failure(PassiveUpstreamFailureRecord {
                    service_name: "codex".to_string(),
                    station_name: "old".to_string(),
                    base_url: "https://old.example/v1".to_string(),
                    status_code: Some(500),
                    error_class: Some("upstream_transport_error".to_string()),
                    error: Some("boom".to_string()),
                    now_ms: 20,
                })
                .await;
            state
                .record_provider_balance_snapshot(
                    "codex",
                    ProviderBalanceSnapshot {
                        provider_id: "provider-old".to_string(),
                        station_name: Some("old".to_string()),
                        upstream_index: Some(0),
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
                    },
                )
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
                    Some("old".to_string()),
                    Some("provider-old".to_string()),
                    "https://old.example/v1".to_string(),
                    None,
                )
                .await;
            state
                .finish_request(FinishRequestParams {
                    id: request_id,
                    status_code: 200,
                    duration_ms: 5,
                    ended_at_ms: 35,
                    observed_service_tier: None,
                    usage: None,
                    retry: None,
                    ttfb_ms: None,
                    streaming: false,
                })
                .await;

            let mut mgr = ServiceConfigManager::default();
            mgr.configs.insert(
                "new".to_string(),
                ServiceConfig {
                    name: "new".to_string(),
                    alias: None,
                    enabled: true,
                    level: 1,
                    upstreams: vec![UpstreamConfig {
                        base_url: "https://new.example/v1".to_string(),
                        auth: UpstreamAuth::default(),
                        tags: HashMap::from([(
                            "provider_id".to_string(),
                            "provider-new".to_string(),
                        )]),
                        supported_models: HashMap::new(),
                        model_mapping: HashMap::new(),
                    }],
                },
            );

            state
                .prune_runtime_observability_for_service("codex", &mgr)
                .await;

            assert!(state.get_station_meta_overrides("codex").await.is_empty());
            assert!(state.get_upstream_meta_overrides("codex").await.is_empty());
            assert!(state.get_station_health("codex").await.is_empty());
            assert!(state.get_provider_balance_view("codex").await.is_empty());

            let rollup = state.get_usage_rollup_view("codex", 10, 10).await;
            assert!(rollup.by_config.is_empty());
            assert!(rollup.by_provider.is_empty());
        });
    }

    #[test]
    fn get_upstream_meta_overrides_merges_endpoint_and_legacy_base_url_entries() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();

            state
                .set_provider_endpoint_enabled_override(
                    "codex",
                    ProviderEndpointKey::new("codex", "alpha", "default"),
                    false,
                    1,
                )
                .await;
            state
                .set_provider_endpoint_runtime_state_override(
                    "codex",
                    ProviderEndpointKey::new("codex", "alpha", "default"),
                    RuntimeConfigState::BreakerOpen,
                    2,
                )
                .await;
            state
                .set_upstream_enabled_override(
                    "codex",
                    "https://legacy.example/v1".to_string(),
                    true,
                    3,
                )
                .await;
            state
                .set_upstream_runtime_state_override(
                    "codex",
                    "https://legacy.example/v1".to_string(),
                    RuntimeConfigState::Draining,
                    4,
                )
                .await;

            let overrides = state.get_upstream_meta_overrides("codex").await;

            assert_eq!(
                overrides.get("codex/alpha/default"),
                Some(&(Some(false), Some(RuntimeConfigState::BreakerOpen)))
            );
            assert_eq!(
                overrides.get("https://legacy.example/v1"),
                Some(&(Some(true), Some(RuntimeConfigState::Draining)))
            );
        });
    }

    #[test]
    fn provider_endpoint_runtime_health_is_keyed_by_provider_endpoint() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let monthly = ProviderEndpointKey::new("codex", "monthly", "default");
            let fallback = ProviderEndpointKey::new("codex", "fallback", "default");

            state
                .record_provider_endpoint_attempt_success("codex", fallback.clone(), 10)
                .await;
            state
                .set_provider_endpoint_usage_exhausted("codex", monthly.clone(), true)
                .await;
            state
                .record_provider_endpoint_attempt_failure(
                    "codex",
                    monthly.clone(),
                    0,
                    crate::lb::CooldownBackoff {
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
                    crate::lb::CooldownBackoff {
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
                    crate::lb::CooldownBackoff {
                        factor: 1,
                        max_secs: 0,
                    },
                )
                .await;

            let runtime = state
                .route_plan_runtime_state_for_provider_endpoints("codex")
                .await;
            let monthly_state = runtime.provider_endpoint(&monthly);
            assert_eq!(monthly_state.failure_count, crate::lb::FAILURE_THRESHOLD);
            assert!(monthly_state.cooldown_active);
            assert!(monthly_state.usage_exhausted);
            assert_eq!(runtime.affinity_provider_endpoint(), Some(&fallback));

            state
                .set_provider_endpoint_runtime_state_override(
                    "codex",
                    fallback.clone(),
                    RuntimeConfigState::BreakerOpen,
                    20,
                )
                .await;
            let runtime = state
                .route_plan_runtime_state_for_provider_endpoints("codex")
                .await;
            assert!(runtime.provider_endpoint(&fallback).runtime_disabled);
        });
    }

    #[test]
    fn owned_policy_action_projects_to_runtime_state_below_manual_overrides() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            let now_ms = unix_now_ms();
            let mut signal = crate::provider_signals::ProviderSignal::high_confidence_route_facing(
                crate::provider_signals::ProviderSignalKind::Quota,
                crate::provider_signals::ProviderSignalSource::UpstreamResponse,
                crate::provider_signals::ProviderSignalTarget::ProviderEndpoint {
                    provider_endpoint_key: endpoint.clone(),
                },
                now_ms,
            );
            signal.reset_after_secs = Some(60);
            signal.reason = Some("usage_limit_reached".to_string());
            let action =
                crate::policy_actions::PolicyAction::cooldown_from_signal(signal, now_ms, 0, 1)
                    .expect("owned cooldown action");

            state.upsert_owned_policy_action("codex", action).await;
            let runtime = state
                .route_plan_runtime_state_for_provider_endpoints("codex")
                .await;
            let projected = runtime.provider_endpoint(&endpoint);
            assert!(projected.cooldown_active);
            assert!(projected.usage_exhausted);
            assert!(!projected.runtime_disabled);

            state
                .set_provider_endpoint_runtime_state_override(
                    "codex",
                    endpoint.clone(),
                    RuntimeConfigState::BreakerOpen,
                    now_ms.saturating_add(1_000),
                )
                .await;
            let runtime = state
                .route_plan_runtime_state_for_provider_endpoints("codex")
                .await;
            let projected = runtime.provider_endpoint(&endpoint);
            assert!(projected.cooldown_active);
            assert!(projected.usage_exhausted);
            assert!(
                projected.runtime_disabled,
                "manual runtime override must outrank automatic action projection"
            );
        });
    }

    #[test]
    fn owned_policy_action_upsert_replaces_existing_action_for_same_source() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            let signal = crate::provider_signals::ProviderSignal::high_confidence_route_facing(
                crate::provider_signals::ProviderSignalKind::Capacity,
                crate::provider_signals::ProviderSignalSource::UpstreamResponse,
                crate::provider_signals::ProviderSignalTarget::ProviderEndpoint {
                    provider_endpoint_key: endpoint,
                },
                1_000,
            );
            let first = crate::policy_actions::PolicyAction::cooldown_from_signal(
                signal.clone(),
                1_000,
                10,
                1,
            )
            .expect("first cooldown");
            let second =
                crate::policy_actions::PolicyAction::cooldown_from_signal(signal, 2_000, 20, 2)
                    .expect("second cooldown");

            state.upsert_owned_policy_action("codex", first).await;
            state.upsert_owned_policy_action("codex", second).await;

            let actions = state.list_policy_actions("codex").await;
            assert_eq!(actions.len(), 1);
            assert_eq!(actions[0].generation, 2);
            assert_eq!(actions[0].expires_at_ms, 22_000);
        });
    }

    #[test]
    fn owned_policy_action_clear_is_source_aware() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            let mut response_signal =
                crate::provider_signals::ProviderSignal::high_confidence_route_facing(
                    crate::provider_signals::ProviderSignalKind::RateLimit,
                    crate::provider_signals::ProviderSignalSource::UpstreamResponse,
                    crate::provider_signals::ProviderSignalTarget::ProviderEndpoint {
                        provider_endpoint_key: endpoint.clone(),
                    },
                    1_000,
                );
            response_signal.reset_after_secs = Some(30);
            let balance_signal = crate::provider_signals::ProviderSignal {
                kind: crate::provider_signals::ProviderSignalKind::Balance,
                code: Some("balance".to_string()),
                source: crate::provider_signals::ProviderSignalSource::BalanceSnapshot,
                target: crate::provider_signals::ProviderSignalTarget::ProviderEndpoint {
                    provider_endpoint_key: endpoint.clone(),
                },
                confidence: crate::provider_signals::ProviderSignalConfidence::High,
                observed_at_ms: 1_000,
                route_facing: true,
                retry_after_secs: None,
                reset_after_secs: Some(60),
                reason: Some("balance_exhausted".to_string()),
                error_class: None,
                trace: Default::default(),
            };
            let response_action = crate::policy_actions::PolicyAction::cooldown_from_signal(
                response_signal,
                1_000,
                30,
                1,
            )
            .expect("response cooldown");
            let balance_action = crate::policy_actions::PolicyAction::cooldown_from_signal(
                balance_signal,
                1_100,
                0,
                2,
            )
            .expect("balance cooldown");

            state
                .upsert_owned_policy_action("codex", response_action)
                .await;
            state
                .upsert_owned_policy_action("codex", balance_action)
                .await;
            assert_eq!(state.list_policy_actions("codex").await.len(), 2);

            state
                .clear_owned_policy_action(
                    "codex",
                    &endpoint,
                    crate::policy_actions::PolicyActionKind::Cooldown,
                    crate::provider_signals::ProviderSignalKind::Balance,
                    crate::provider_signals::ProviderSignalSource::BalanceSnapshot,
                )
                .await;

            let actions = state.list_policy_actions("codex").await;
            assert_eq!(actions.len(), 1);
            assert_eq!(
                actions[0].source_signal.kind,
                crate::provider_signals::ProviderSignalKind::RateLimit
            );
        });
    }

    #[test]
    fn expired_owned_policy_action_is_not_projected() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            let signal = crate::provider_signals::ProviderSignal::high_confidence_route_facing(
                crate::provider_signals::ProviderSignalKind::Transport,
                crate::provider_signals::ProviderSignalSource::UpstreamResponse,
                crate::provider_signals::ProviderSignalTarget::ProviderEndpoint {
                    provider_endpoint_key: endpoint.clone(),
                },
                1_000,
            );
            let action =
                crate::policy_actions::PolicyAction::cooldown_from_signal(signal, 1_000, 1, 1)
                    .expect("transport cooldown");

            state.upsert_owned_policy_action("codex", action).await;

            assert!(
                state
                    .active_policy_action_projections("codex", 2_001)
                    .await
                    .is_empty()
            );
            let runtime = state
                .route_plan_runtime_state_for_provider_endpoints("codex")
                .await;
            let projected = runtime.provider_endpoint(&endpoint);
            assert!(!projected.cooldown_active);
            assert!(!projected.usage_exhausted);
        });
    }

    #[test]
    fn get_station_health_merges_passive_runtime_observations() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            state
                .record_station_health(
                    "codex",
                    "right".to_string(),
                    StationHealth {
                        checked_at_ms: 10,
                        upstreams: vec![UpstreamHealth {
                            base_url: "https://right.example/v1".to_string(),
                            ok: Some(true),
                            status_code: Some(200),
                            latency_ms: Some(120),
                            error: None,
                            passive: None,
                        }],
                    },
                )
                .await;
            state
                .record_passive_upstream_failure(PassiveUpstreamFailureRecord {
                    service_name: "codex".to_string(),
                    station_name: "right".to_string(),
                    base_url: "https://right.example/v1".to_string(),
                    status_code: Some(500),
                    error_class: Some("cloudflare_timeout".to_string()),
                    error: Some("upstream timed out".to_string()),
                    now_ms: 20,
                })
                .await;
            state
                .record_passive_upstream_success(
                    "codex",
                    "right",
                    "https://backup.example/v1",
                    Some(200),
                    30,
                )
                .await;

            let health = state.get_station_health("codex").await;
            let right = health.get("right").expect("right health");
            assert_eq!(right.checked_at_ms, 30);
            assert_eq!(right.upstreams.len(), 2);

            let primary = right
                .upstreams
                .iter()
                .find(|upstream| upstream.base_url == "https://right.example/v1")
                .expect("primary upstream");
            assert_eq!(primary.ok, Some(true));
            let primary_passive = primary.passive.as_ref().expect("primary passive");
            assert_eq!(primary_passive.state, PassiveHealthState::Degraded);
            assert_eq!(primary_passive.score, 50);
            assert_eq!(primary_passive.last_status_code, Some(500));
            assert_eq!(
                primary_passive.last_error_class.as_deref(),
                Some("cloudflare_timeout")
            );

            let backup = right
                .upstreams
                .iter()
                .find(|upstream| upstream.base_url == "https://backup.example/v1")
                .expect("backup upstream");
            assert_eq!(backup.ok, None);
            let backup_passive = backup.passive.as_ref().expect("backup passive");
            assert_eq!(backup_passive.state, PassiveHealthState::Healthy);
            assert_eq!(backup_passive.score, 100);
            assert_eq!(backup_passive.last_status_code, Some(200));
        });
    }

    #[test]
    fn passive_health_success_recovers_after_failure() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            state
                .record_passive_upstream_failure(PassiveUpstreamFailureRecord {
                    service_name: "codex".to_string(),
                    station_name: "right".to_string(),
                    base_url: "https://right.example/v1".to_string(),
                    status_code: Some(500),
                    error_class: Some("cloudflare_timeout".to_string()),
                    error: Some("upstream timed out".to_string()),
                    now_ms: 10,
                })
                .await;
            state
                .record_passive_upstream_success(
                    "codex",
                    "right",
                    "https://right.example/v1",
                    Some(200),
                    20,
                )
                .await;

            let health = state.get_station_health("codex").await;
            let right = health.get("right").expect("right health");
            let upstream = right.upstreams.first().expect("upstream");
            let passive = upstream.passive.as_ref().expect("passive");
            assert_eq!(passive.state, PassiveHealthState::Healthy);
            assert_eq!(passive.score, 100);
            assert_eq!(passive.consecutive_failures, 0);
            assert_eq!(passive.last_success_at_ms, Some(20));
            assert_eq!(passive.last_failure_at_ms, Some(10));
            assert_eq!(passive.last_error_class, None);
            assert_eq!(passive.last_error, None);
        });
    }

    #[test]
    fn apply_session_profile_binding_uses_inherited_profile_values() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            let mut mgr = ServiceConfigManager::default();
            mgr.configs.insert(
                "right".to_string(),
                ServiceConfig {
                    name: "right".to_string(),
                    alias: None,
                    enabled: true,
                    level: 1,
                    upstreams: vec![UpstreamConfig {
                        base_url: "https://right.example/v1".to_string(),
                        auth: UpstreamAuth::default(),
                        tags: HashMap::from([
                            ("supports_reasoning_effort".to_string(), "true".to_string()),
                            ("supports_service_tier".to_string(), "true".to_string()),
                        ]),
                        supported_models: HashMap::from([("gpt-5.4".to_string(), true)]),
                        model_mapping: HashMap::new(),
                    }],
                },
            );
            mgr.profiles.insert(
                "base".to_string(),
                crate::config::ServiceControlProfile {
                    extends: None,
                    station: Some("right".to_string()),
                    model: Some("gpt-5.4".to_string()),
                    reasoning_effort: None,
                    service_tier: Some("priority".to_string()),
                },
            );
            mgr.profiles.insert(
                "fast".to_string(),
                crate::config::ServiceControlProfile {
                    extends: Some("base".to_string()),
                    station: None,
                    model: None,
                    reasoning_effort: Some("low".to_string()),
                    service_tier: None,
                },
            );

            state
                .apply_session_profile_binding(
                    "codex",
                    &mgr,
                    "sid-inherited".to_string(),
                    "fast".to_string(),
                    100,
                )
                .await
                .expect("apply inherited profile");

            let binding = state
                .get_session_binding("sid-inherited")
                .await
                .expect("binding exists");
            assert_eq!(binding.profile_name.as_deref(), Some("fast"));
            assert_eq!(binding.station_name.as_deref(), Some("right"));
            assert_eq!(binding.model.as_deref(), Some("gpt-5.4"));
            assert_eq!(binding.reasoning_effort.as_deref(), Some("low"));
            assert_eq!(binding.service_tier.as_deref(), Some("priority"));
        });
    }

    #[test]
    fn prune_periodic_keeps_sticky_binding_after_manual_override_ttl_expires() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new_with_runtime_policy(None, test_runtime_policy(1, 0, 2_000));
            state
                .set_session_model_override("sid-sticky".to_string(), "gpt-5.4".to_string(), 0)
                .await;
            state
                .set_session_binding(SessionBinding {
                    session_id: "sid-sticky".to_string(),
                    profile_name: Some("daily".to_string()),
                    station_name: Some("right".to_string()),
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

            assert!(
                state
                    .get_session_model_override("sid-sticky")
                    .await
                    .is_none()
            );
            assert!(state.get_session_binding("sid-sticky").await.is_some());
        });
    }

    #[test]
    fn prune_periodic_honors_opt_in_binding_ttl() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new_with_runtime_policy(None, test_runtime_policy(0, 1, 2_000));
            state
                .set_session_binding(SessionBinding {
                    session_id: "sid-expire".to_string(),
                    profile_name: Some("daily".to_string()),
                    station_name: Some("right".to_string()),
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
            let state = ProxyState::new_with_runtime_policy(None, policy);
            state
                .record_session_route_affinity_success(
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
                .await;

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
    fn session_route_affinity_ledger_does_not_restore_expired_entries() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let _home = env_lock().await;
            let temp_dir = std::env::temp_dir().join(format!(
                "codex-helper-route-ledger-test-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&temp_dir).expect("create temp dir");
            let mut scoped = ScopedEnv::default();
            unsafe {
                scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
            }

            let mut policy = test_runtime_policy(0, 0, 2_000);
            policy.session_route_affinity_ttl_ms = 1;
            let first_state = ProxyState::new_with_runtime_policy(None, policy.clone());
            first_state
                .record_session_route_affinity_success(
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
                .await;

            let second_state = ProxyState::new_with_runtime_policy(None, policy);
            assert!(
                second_state
                    .get_session_route_affinity("sid-expired-ledger")
                    .await
                    .is_none()
            );
        });
    }

    #[test]
    fn policy_action_ledger_restores_owned_actions() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let _home = env_lock().await;
            let temp_dir = std::env::temp_dir().join(format!(
                "codex-helper-policy-action-ledger-test-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&temp_dir).expect("create temp dir");
            let mut scoped = ScopedEnv::default();
            unsafe {
                scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
                scoped.set_path(
                    "CODEX_HELPER_POLICY_ACTION_LEDGER",
                    temp_dir.join("policy-actions.json").as_path(),
                );
            }

            let first_state =
                ProxyState::new_with_runtime_policy(None, test_runtime_policy(0, 0, 2_000));
            let mut signal = crate::provider_signals::ProviderSignal::high_confidence_route_facing(
                crate::provider_signals::ProviderSignalKind::RateLimit,
                crate::provider_signals::ProviderSignalSource::UpstreamResponse,
                crate::provider_signals::ProviderSignalTarget::ProviderEndpoint {
                    provider_endpoint_key: ProviderEndpointKey::new("codex", "monthly", "default"),
                },
                unix_now_ms(),
            );
            signal.reset_after_secs = Some(60);
            let action = crate::policy_actions::PolicyAction::cooldown_from_signal(
                signal,
                unix_now_ms(),
                0,
                1,
            )
            .expect("cooldown action");
            first_state
                .upsert_owned_policy_action("codex", action)
                .await;

            let second_state =
                ProxyState::new_with_runtime_policy(None, test_runtime_policy(0, 0, 2_000));
            let actions = second_state.list_policy_actions("codex").await;
            assert_eq!(actions.len(), 1);
            assert_eq!(
                actions[0].provider_endpoint_key.stable_key(),
                "codex/monthly/default"
            );
        });
    }

    #[test]
    fn policy_action_upsert_does_not_publish_when_ledger_save_fails() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let _home = env_lock().await;
            let temp_dir = std::env::temp_dir().join(format!(
                "codex-helper-policy-action-save-failure-test-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&temp_dir).expect("create temp dir");
            let mut scoped = ScopedEnv::default();
            unsafe {
                scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
                scoped.set_path("CODEX_HELPER_POLICY_ACTION_LEDGER", temp_dir.as_path());
            }

            let state = ProxyState::new_with_runtime_policy(None, test_runtime_policy(0, 0, 2_000));
            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            let mut signal = crate::provider_signals::ProviderSignal::high_confidence_route_facing(
                crate::provider_signals::ProviderSignalKind::RateLimit,
                crate::provider_signals::ProviderSignalSource::UpstreamResponse,
                crate::provider_signals::ProviderSignalTarget::ProviderEndpoint {
                    provider_endpoint_key: endpoint.clone(),
                },
                unix_now_ms(),
            );
            signal.reset_after_secs = Some(60);
            let action = crate::policy_actions::PolicyAction::cooldown_from_signal(
                signal,
                unix_now_ms(),
                0,
                1,
            )
            .expect("cooldown action");

            state.upsert_owned_policy_action("codex", action).await;

            assert!(state.list_policy_actions("codex").await.is_empty());
            let runtime_state = state
                .route_plan_runtime_state_for_provider_endpoints("codex")
                .await;
            assert!(!runtime_state.provider_endpoint(&endpoint).cooldown_active);
        });
    }

    #[test]
    fn policy_action_ledger_drops_expired_actions_on_restore() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let _home = env_lock().await;
            let temp_dir = std::env::temp_dir().join(format!(
                "codex-helper-policy-action-expired-ledger-test-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&temp_dir).expect("create temp dir");
            let ledger_path = temp_dir.join("policy-actions.json");
            let mut scoped = ScopedEnv::default();
            unsafe {
                scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
                scoped.set_path("CODEX_HELPER_POLICY_ACTION_LEDGER", ledger_path.as_path());
            }

            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            let signal = crate::provider_signals::ProviderSignal::high_confidence_route_facing(
                crate::provider_signals::ProviderSignalKind::Transport,
                crate::provider_signals::ProviderSignalSource::UpstreamResponse,
                crate::provider_signals::ProviderSignalTarget::ProviderEndpoint {
                    provider_endpoint_key: endpoint,
                },
                1_000,
            );
            let action =
                crate::policy_actions::PolicyAction::cooldown_from_signal(signal, 1_000, 1, 1)
                    .expect("transport cooldown");
            let ledger = serde_json::json!({
                "schema_version": 1,
                "updated_at_ms": 2_000,
                "entries": [
                    {
                        "service_name": "codex",
                        "action": action
                    }
                ]
            });
            std::fs::write(
                &ledger_path,
                serde_json::to_string_pretty(&ledger).expect("ledger json"),
            )
            .expect("write ledger");

            let state = ProxyState::new_with_runtime_policy(None, test_runtime_policy(0, 0, 2_000));

            assert!(state.list_policy_actions("codex").await.is_empty());
            let compacted: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(&ledger_path).expect("compacted ledger"),
            )
            .expect("parse compacted ledger");
            assert_eq!(
                compacted
                    .get("entries")
                    .and_then(|entries| entries.as_array())
                    .map(Vec::len),
                Some(0)
            );
        });
    }

    #[test]
    fn policy_action_prune_persists_removed_endpoint_actions() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let _home = env_lock().await;
            let temp_dir = std::env::temp_dir().join(format!(
                "codex-helper-policy-action-prune-ledger-test-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&temp_dir).expect("create temp dir");
            let ledger_path = temp_dir.join("policy-actions.json");
            let mut scoped = ScopedEnv::default();
            unsafe {
                scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
                scoped.set_path("CODEX_HELPER_POLICY_ACTION_LEDGER", ledger_path.as_path());
            }

            let state = ProxyState::new_with_runtime_policy(None, test_runtime_policy(0, 0, 2_000));
            let endpoint = ProviderEndpointKey::new("codex", "monthly", "default");
            let mut signal = crate::provider_signals::ProviderSignal::high_confidence_route_facing(
                crate::provider_signals::ProviderSignalKind::RateLimit,
                crate::provider_signals::ProviderSignalSource::UpstreamResponse,
                crate::provider_signals::ProviderSignalTarget::ProviderEndpoint {
                    provider_endpoint_key: endpoint,
                },
                unix_now_ms(),
            );
            signal.reset_after_secs = Some(60);
            let action = crate::policy_actions::PolicyAction::cooldown_from_signal(
                signal,
                unix_now_ms(),
                0,
                1,
            )
            .expect("cooldown action");
            state.upsert_owned_policy_action("codex", action).await;
            assert_eq!(state.list_policy_actions("codex").await.len(), 1);

            let mut mgr = ServiceConfigManager::default();
            mgr.configs.insert(
                "routing".to_string(),
                ServiceConfig {
                    name: "routing".to_string(),
                    alias: None,
                    enabled: true,
                    level: 1,
                    upstreams: vec![UpstreamConfig {
                        base_url: "https://backup.example/v1".to_string(),
                        auth: UpstreamAuth::default(),
                        tags: HashMap::from([
                            ("provider_id".to_string(), "backup".to_string()),
                            ("endpoint_id".to_string(), "default".to_string()),
                        ]),
                        supported_models: HashMap::new(),
                        model_mapping: HashMap::new(),
                    }],
                },
            );
            state
                .prune_runtime_observability_for_service("codex", &mgr)
                .await;
            assert!(state.list_policy_actions("codex").await.is_empty());

            let restored =
                ProxyState::new_with_runtime_policy(None, test_runtime_policy(0, 0, 2_000));
            assert!(restored.list_policy_actions("codex").await.is_empty());
        });
    }

    #[test]
    fn prune_periodic_caps_sticky_bindings_by_last_seen() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new_with_runtime_policy(None, test_runtime_policy(0, 0, 2));
            for idx in 0..3 {
                state
                    .set_session_binding(SessionBinding {
                        session_id: format!("sid-{idx}"),
                        profile_name: Some("daily".to_string()),
                        station_name: Some("right".to_string()),
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
            let state = ProxyState::new_with_runtime_policy(None, test_runtime_policy(0, 0, 2_000));
            let paths = state
                .resolve_host_transcript_paths_cached(&["missing-session".to_string()])
                .await;

            assert_eq!(paths.get("missing-session"), Some(&None));
            let cache = state.session_transcript_path_cache.read().await;
            assert!(cache.contains_key("missing-session"));
        });
    }
}

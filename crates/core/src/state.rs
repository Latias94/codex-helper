use std::collections::{HashMap, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value as JsonValue;
use tokio::sync::RwLock;
use tokio::time::{Duration, interval};

use crate::config::ServiceConfigManager;
use crate::lb::LbState;
use crate::sessions;
use crate::usage::UsageMetrics;

mod runtime_types;
mod session_identity;

use self::runtime_types::{
    ConfigMetaOverride, RuntimeDefaultProfileOverride, UsageRollup, merge_station_health,
};
pub use self::runtime_types::{
    HealthCheckStatus, LbConfigView, LbUpstreamView, PassiveHealthState, PassiveUpstreamHealth,
    RuntimeConfigState, StationHealth, UpstreamHealth, UsageBucket, UsageRollupView,
};
pub use self::session_identity::{
    ActiveRequest, FinishRequestParams, FinishedRequest, ResolvedRouteValue,
    RouteDecisionProvenance, RouteValueSource, SessionBinding, SessionContinuityMode,
    SessionIdentityCard, SessionIdentityCardBuildInputs, SessionManualOverrides,
    SessionObservationScope, SessionStats, build_session_identity_cards_from_parts,
    enrich_session_identity_cards_with_host_transcripts,
    enrich_session_identity_cards_with_runtime,
};
use self::session_identity::{
    SessionBindingEntry, SessionCwdCacheEntry, SessionEffortOverride, SessionModelOverride,
    SessionServiceTierOverride, SessionStationOverride,
};

type PassiveStationHealthMap =
    HashMap<String, HashMap<String, HashMap<String, PassiveUpstreamHealth>>>;

pub struct PassiveUpstreamFailureRecord {
    pub service_name: String,
    pub station_name: String,
    pub base_url: String,
    pub status_code: Option<u16>,
    pub error_class: Option<String>,
    pub error: Option<String>,
    pub now_ms: u64,
}

fn recent_finished_max() -> usize {
    static MAX: OnceLock<usize> = OnceLock::new();
    *MAX.get_or_init(|| {
        std::env::var("CODEX_HELPER_RECENT_FINISHED_MAX")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(2_000)
            .clamp(200, 20_000)
    })
}

/// Runtime-only state for the proxy process.
///
/// This state is intentionally not persisted across restarts.
#[derive(Debug)]
pub struct ProxyState {
    next_request_id: AtomicU64,
    // Manual per-session overrides remain runtime-scoped and expire after inactivity.
    session_override_ttl_ms: u64,
    // Bindings are sticky by default; operators can opt into pruning with a separate TTL.
    session_binding_ttl_ms: u64,
    session_cwd_cache_ttl_ms: u64,
    session_cwd_cache_max_entries: usize,
    session_effort_overrides: RwLock<HashMap<String, SessionEffortOverride>>,
    session_station_overrides: RwLock<HashMap<String, SessionStationOverride>>,
    session_model_overrides: RwLock<HashMap<String, SessionModelOverride>>,
    session_service_tier_overrides: RwLock<HashMap<String, SessionServiceTierOverride>>,
    session_bindings: RwLock<HashMap<String, SessionBindingEntry>>,
    global_station_override: RwLock<Option<String>>,
    runtime_default_profiles: RwLock<HashMap<String, RuntimeDefaultProfileOverride>>,
    station_meta_overrides: RwLock<HashMap<String, HashMap<String, ConfigMetaOverride>>>,
    upstream_meta_overrides: RwLock<HashMap<String, HashMap<String, ConfigMetaOverride>>>,
    session_cwd_cache: RwLock<HashMap<String, SessionCwdCacheEntry>>,
    session_stats: RwLock<HashMap<String, SessionStats>>,
    active_requests: RwLock<HashMap<u64, ActiveRequest>>,
    recent_finished: RwLock<VecDeque<FinishedRequest>>,
    usage_rollups: RwLock<HashMap<String, UsageRollup>>,
    station_health: RwLock<HashMap<String, HashMap<String, StationHealth>>>,
    passive_station_health: RwLock<PassiveStationHealthMap>,
    station_health_checks: RwLock<HashMap<String, HashMap<String, HealthCheckStatus>>>,
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

        let cwd_cache_ttl_secs = std::env::var("CODEX_HELPER_SESSION_CWD_CACHE_TTL_SECS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(12 * 60 * 60);
        let cwd_cache_ttl_ms = cwd_cache_ttl_secs.saturating_mul(1000);
        let cwd_cache_max_entries = std::env::var("CODEX_HELPER_SESSION_CWD_CACHE_MAX_ENTRIES")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(2_000);

        Self::new_with_runtime_policy(
            lb_states,
            ttl_ms,
            binding_ttl_ms,
            cwd_cache_ttl_ms,
            cwd_cache_max_entries,
        )
    }

    fn new_with_runtime_policy(
        lb_states: Option<Arc<Mutex<HashMap<String, LbState>>>>,
        session_override_ttl_ms: u64,
        session_binding_ttl_ms: u64,
        session_cwd_cache_ttl_ms: u64,
        session_cwd_cache_max_entries: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            next_request_id: AtomicU64::new(1),
            session_override_ttl_ms,
            session_binding_ttl_ms,
            session_cwd_cache_ttl_ms,
            session_cwd_cache_max_entries,
            session_effort_overrides: RwLock::new(HashMap::new()),
            session_station_overrides: RwLock::new(HashMap::new()),
            session_model_overrides: RwLock::new(HashMap::new()),
            session_service_tier_overrides: RwLock::new(HashMap::new()),
            session_bindings: RwLock::new(HashMap::new()),
            global_station_override: RwLock::new(None),
            runtime_default_profiles: RwLock::new(HashMap::new()),
            station_meta_overrides: RwLock::new(HashMap::new()),
            upstream_meta_overrides: RwLock::new(HashMap::new()),
            session_cwd_cache: RwLock::new(HashMap::new()),
            session_stats: RwLock::new(HashMap::new()),
            active_requests: RwLock::new(HashMap::new()),
            recent_finished: RwLock::new(VecDeque::new()),
            usage_rollups: RwLock::new(HashMap::new()),
            station_health: RwLock::new(HashMap::new()),
            passive_station_health: RwLock::new(HashMap::new()),
            station_health_checks: RwLock::new(HashMap::new()),
            lb_states,
        })
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
        guard.remove(session_id);
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
    }

    pub async fn clear_session_model_override(&self, session_id: &str) {
        let mut guard = self.session_model_overrides.write().await;
        guard.remove(session_id);
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
    }

    pub async fn clear_session_service_tier_override(&self, session_id: &str) {
        let mut guard = self.session_service_tier_overrides.write().await;
        guard.remove(session_id);
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
    }

    pub async fn clear_session_binding(&self, session_id: &str) {
        let mut guard = self.session_bindings.write().await;
        guard.remove(session_id);
    }

    pub async fn clear_session_manual_overrides(&self, session_id: &str) {
        self.clear_session_station_override(session_id).await;
        self.clear_session_model_override(session_id).await;
        self.clear_session_effort_override(session_id).await;
        self.clear_session_service_tier_override(session_id).await;
    }

    pub async fn get_session_manual_overrides(&self, session_id: &str) -> SessionManualOverrides {
        let (reasoning_effort, station_name, model, service_tier) = tokio::join!(
            self.get_session_reasoning_effort_override(session_id),
            self.get_session_station_override(session_id),
            self.get_session_model_override(session_id),
            self.get_session_service_tier_override(session_id),
        );

        SessionManualOverrides {
            reasoning_effort,
            station_name,
            model,
            service_tier,
        }
    }

    pub async fn list_session_manual_overrides(&self) -> HashMap<String, SessionManualOverrides> {
        let (reasoning_effort_map, station_map, model_map, service_tier_map) = tokio::join!(
            self.list_session_reasoning_effort_overrides(),
            self.list_session_station_overrides(),
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
    }

    pub async fn clear_session_station_override(&self, session_id: &str) {
        let mut guard = self.session_station_overrides.write().await;
        guard.remove(session_id);
    }

    pub async fn list_session_station_overrides(&self) -> HashMap<String, String> {
        let guard = self.session_station_overrides.read().await;
        guard
            .iter()
            .map(|(k, v)| (k.clone(), v.station_name.clone()))
            .collect()
    }

    pub async fn touch_session_station_override(&self, session_id: &str, now_ms: u64) {
        let mut guard = self.session_station_overrides.write().await;
        if let Some(v) = guard.get_mut(session_id) {
            v.last_seen_ms = now_ms;
        }
    }

    pub async fn get_global_station_override(&self) -> Option<String> {
        let guard = self.global_station_override.read().await;
        guard.clone()
    }

    pub async fn set_global_station_override(&self, station_name: String, _now_ms: u64) {
        let mut guard = self.global_station_override.write().await;
        *guard = Some(station_name);
    }

    pub async fn clear_global_station_override(&self) {
        let mut guard = self.global_station_override.write().await;
        *guard = None;
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
    }

    pub async fn clear_runtime_default_profile_override(&self, service_name: &str) {
        let mut guard = self.runtime_default_profiles.write().await;
        guard.remove(service_name);
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
    }

    pub async fn clear_station_enabled_override(&self, service_name: &str, station_name: &str) {
        let mut guard = self.station_meta_overrides.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return;
        };
        let Some(entry) = per_service.get_mut(station_name) else {
            return;
        };
        entry.enabled = None;
        if entry.enabled.is_none() && entry.level.is_none() && entry.state.is_none() {
            per_service.remove(station_name);
        }
        if per_service.is_empty() {
            guard.remove(service_name);
        }
    }

    pub async fn clear_station_level_override(&self, service_name: &str, station_name: &str) {
        let mut guard = self.station_meta_overrides.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return;
        };
        let Some(entry) = per_service.get_mut(station_name) else {
            return;
        };
        entry.level = None;
        if entry.enabled.is_none() && entry.level.is_none() && entry.state.is_none() {
            per_service.remove(station_name);
        }
        if per_service.is_empty() {
            guard.remove(service_name);
        }
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
        entry.state = None;
        if entry.enabled.is_none() && entry.level.is_none() && entry.state.is_none() {
            per_service.remove(station_name);
        }
        if per_service.is_empty() {
            guard.remove(service_name);
        }
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
    }

    pub async fn clear_upstream_enabled_override(&self, service_name: &str, base_url: &str) {
        let mut guard = self.upstream_meta_overrides.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return;
        };
        let Some(entry) = per_service.get_mut(base_url) else {
            return;
        };
        entry.enabled = None;
        if entry.enabled.is_none() && entry.level.is_none() && entry.state.is_none() {
            per_service.remove(base_url);
        }
        if per_service.is_empty() {
            guard.remove(service_name);
        }
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
    }

    pub async fn clear_upstream_runtime_state_override(&self, service_name: &str, base_url: &str) {
        let mut guard = self.upstream_meta_overrides.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return;
        };
        let Some(entry) = per_service.get_mut(base_url) else {
            return;
        };
        entry.state = None;
        if entry.enabled.is_none() && entry.level.is_none() && entry.state.is_none() {
            per_service.remove(base_url);
        }
        if per_service.is_empty() {
            guard.remove(service_name);
        }
    }

    pub async fn get_upstream_meta_overrides(
        &self,
        service_name: &str,
    ) -> HashMap<String, (Option<bool>, Option<RuntimeConfigState>)> {
        let guard = self.upstream_meta_overrides.read().await;
        guard
            .get(service_name)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), (v.enabled, v.state)))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default()
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

        fn day_series(map: &HashMap<i32, UsageBucket>, days: usize) -> Vec<(i32, UsageBucket)> {
            let mut out = map.iter().map(|(k, v)| (*k, v.clone())).collect::<Vec<_>>();
            out.sort_by_key(|(k, _)| *k);
            if out.len() > days {
                out = out[out.len().saturating_sub(days)..].to_vec();
            }
            out
        }

        let mut by_day = rollup
            .by_day
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect::<Vec<_>>();
        by_day.sort_by_key(|(k, _)| *k);
        if by_day.len() > days {
            by_day = by_day[by_day.len().saturating_sub(days)..].to_vec();
        }

        let mut by_config = rollup
            .by_config
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>();
        by_config.sort_by_key(|(_, v)| std::cmp::Reverse(v.usage.total_tokens));
        by_config.truncate(top_n);

        let mut by_provider = rollup
            .by_provider
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>();
        by_provider.sort_by_key(|(_, v)| std::cmp::Reverse(v.usage.total_tokens));
        by_provider.truncate(top_n);

        let mut by_config_day = HashMap::new();
        for (name, _) in &by_config {
            if let Some(m) = rollup.by_config_day.get(name) {
                by_config_day.insert(name.clone(), day_series(m, days));
            } else {
                by_config_day.insert(name.clone(), Vec::new());
            }
        }

        let mut by_provider_day = HashMap::new();
        for (name, _) in &by_provider {
            if let Some(m) = rollup.by_provider_day.get(name) {
                by_provider_day.insert(name.clone(), day_series(m, days));
            } else {
                by_provider_day.insert(name.clone(), Vec::new());
            }
        }

        UsageRollupView {
            since_start: rollup.since_start.clone(),
            by_day,
            by_config,
            by_config_day,
            by_provider,
            by_provider_day,
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
                .is_some_and(|r| r.since_start.requests_total > 0)
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

        let mut events = Vec::new();
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

            let ended_at_ms = v.get("timestamp_ms").and_then(|x| x.as_u64()).unwrap_or(0);
            let status_code = v.get("status_code").and_then(|x| x.as_u64()).unwrap_or(0) as u16;
            let duration_ms = v.get("duration_ms").and_then(|x| x.as_u64()).unwrap_or(0);
            let station_name = v
                .get("station_name")
                .and_then(|x| x.as_str())
                .unwrap_or("-")
                .to_string();
            let upstream_base_url = v
                .get("upstream_base_url")
                .and_then(|x| x.as_str())
                .unwrap_or("-")
                .to_string();
            let provider_id = v
                .get("provider_id")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .or_else(|| base_url_to_provider_id.get(&upstream_base_url).cloned())
                .unwrap_or_else(|| "-".to_string());
            let usage = v
                .get("usage")
                .and_then(|u| serde_json::from_value::<UsageMetrics>(u.clone()).ok());
            let ttfb_ms = v.get("ttfb_ms").and_then(|x| x.as_u64());

            events.push((
                ended_at_ms,
                status_code,
                duration_ms,
                station_name,
                provider_id,
                usage,
                ttfb_ms,
            ));
        }

        if events.is_empty() {
            return 0;
        }

        let mut guard = self.usage_rollups.write().await;
        let rollup = guard.entry(service_name.to_string()).or_default();
        for (ended_at_ms, status_code, duration_ms, cfg_key, provider_key, usage, ttfb_ms) in
            events.iter()
        {
            let day = (*ended_at_ms / 86_400_000) as i32;
            rollup
                .since_start
                .record(*status_code, *duration_ms, usage.as_ref(), *ttfb_ms);
            rollup.by_day.entry(day).or_default().record(
                *status_code,
                *duration_ms,
                usage.as_ref(),
                *ttfb_ms,
            );
            rollup.by_config.entry(cfg_key.clone()).or_default().record(
                *status_code,
                *duration_ms,
                usage.as_ref(),
                *ttfb_ms,
            );
            rollup
                .by_config_day
                .entry(cfg_key.clone())
                .or_default()
                .entry(day)
                .or_default()
                .record(*status_code, *duration_ms, usage.as_ref(), *ttfb_ms);
            rollup
                .by_provider
                .entry(provider_key.clone())
                .or_default()
                .record(*status_code, *duration_ms, usage.as_ref(), *ttfb_ms);
            rollup
                .by_provider_day
                .entry(provider_key.clone())
                .or_default()
                .entry(day)
                .or_default()
                .record(*status_code, *duration_ms, usage.as_ref(), *ttfb_ms);
        }

        events.len()
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
        client_name: Option<String>,
        client_addr: Option<String>,
        cwd: Option<String>,
        model: Option<String>,
        reasoning_effort: Option<String>,
        service_tier: Option<String>,
        started_at_ms: u64,
    ) -> u64 {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let req = ActiveRequest {
            id,
            session_id,
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
        id
    }

    pub async fn update_request_route(
        &self,
        request_id: u64,
        station_name: String,
        provider_id: Option<String>,
        upstream_base_url: String,
        route_decision: Option<RouteDecisionProvenance>,
    ) {
        let mut guard = self.active_requests.write().await;
        let Some(req) = guard.get_mut(&request_id) else {
            return;
        };
        req.station_name = Some(station_name);
        req.provider_id = provider_id;
        req.upstream_base_url = Some(upstream_base_url);
        req.route_decision = route_decision;
    }

    pub async fn finish_request(&self, params: FinishRequestParams) {
        let mut active = self.active_requests.write().await;
        let Some(req) = active.remove(&params.id) else {
            return;
        };

        let finished = FinishedRequest {
            id: params.id,
            session_id: req.session_id,
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
            retry: params.retry,
            service: req.service,
            method: req.method,
            path: req.path,
            status_code: params.status_code,
            duration_ms: params.duration_ms,
            ttfb_ms: params.ttfb_ms,
            ended_at_ms: params.ended_at_ms,
        };

        {
            let day = (finished.ended_at_ms / 86_400_000) as i32;
            let cfg_key = finished
                .station_name
                .clone()
                .unwrap_or_else(|| "-".to_string());
            let provider_key = finished
                .provider_id
                .clone()
                .unwrap_or_else(|| "-".to_string());

            let mut rollups = self.usage_rollups.write().await;
            let rollup = rollups.entry(finished.service.clone()).or_default();
            rollup.since_start.record(
                finished.status_code,
                finished.duration_ms,
                finished.usage.as_ref(),
                finished.ttfb_ms,
            );
            rollup.by_day.entry(day).or_default().record(
                finished.status_code,
                finished.duration_ms,
                finished.usage.as_ref(),
                finished.ttfb_ms,
            );
            rollup.by_config.entry(cfg_key.clone()).or_default().record(
                finished.status_code,
                finished.duration_ms,
                finished.usage.as_ref(),
                finished.ttfb_ms,
            );
            rollup
                .by_config_day
                .entry(cfg_key)
                .or_default()
                .entry(day)
                .or_default()
                .record(
                    finished.status_code,
                    finished.duration_ms,
                    finished.usage.as_ref(),
                    finished.ttfb_ms,
                );

            rollup
                .by_provider
                .entry(provider_key.clone())
                .or_default()
                .record(
                    finished.status_code,
                    finished.duration_ms,
                    finished.usage.as_ref(),
                    finished.ttfb_ms,
                );
            rollup
                .by_provider_day
                .entry(provider_key)
                .or_default()
                .entry(day)
                .or_default()
                .record(
                    finished.status_code,
                    finished.duration_ms,
                    finished.usage.as_ref(),
                    finished.ttfb_ms,
                );
        }

        if let Some(sid) = finished.session_id.as_deref() {
            let mut stats = self.session_stats.write().await;
            let entry = stats.entry(sid.to_string()).or_default();
            entry.turns_total = entry.turns_total.saturating_add(1);
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
            global_station_override: global_station_override.as_deref(),
            stats: &stats,
        })
    }

    pub async fn list_session_identity_cards_with_host_transcripts(
        &self,
        recent_limit: usize,
    ) -> Vec<SessionIdentityCard> {
        let mut cards = self.list_session_identity_cards(recent_limit).await;
        enrich_session_identity_cards_with_host_transcripts(&mut cards).await;
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

        // Keep a bounded number of days of rollup data to avoid unbounded growth.
        let keep_days: i32 = std::env::var("CODEX_HELPER_USAGE_ROLLUP_KEEP_DAYS")
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(60);
        let now_day = (now_ms / 86_400_000) as i32;
        let cutoff_day = now_day.saturating_sub(keep_days);
        let mut rollups = self.usage_rollups.write().await;
        for rollup in rollups.values_mut() {
            rollup.by_day.retain(|day, _| *day >= cutoff_day);
            rollup.by_config_day.retain(|_, m| {
                m.retain(|day, _| *day >= cutoff_day);
                !m.is_empty()
            });
            rollup.by_provider_day.retain(|_, m| {
                m.retain(|day, _| *day >= cutoff_day);
                !m.is_empty()
            });
        }

        let cutoff_cwd =
            if self.session_cwd_cache_ttl_ms == 0 || now_ms < self.session_cwd_cache_ttl_ms {
                0
            } else {
                now_ms - self.session_cwd_cache_ttl_ms
            };
        self.prune_session_cwd_cache(&active_sessions, cutoff_cwd)
            .await;

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
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::{ServiceConfig, ServiceConfigManager, UpstreamAuth, UpstreamConfig};

    #[test]
    fn build_session_identity_cards_merges_sources_and_sorts_newest_first() {
        let active = vec![ActiveRequest {
            id: 1,
            session_id: Some("sid-active".to_string()),
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
                session_id: Some("sid-recent".to_string()),
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
                }),
                retry: None,
                service: "codex".to_string(),
                method: "POST".to_string(),
                path: "/v1/responses".to_string(),
                status_code: 200,
                duration_ms: 1200,
                ttfb_ms: Some(100),
                ended_at_ms: 2_000,
            },
            FinishedRequest {
                id: 3,
                session_id: Some("sid-active".to_string()),
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
                retry: None,
                service: "codex".to_string(),
                method: "POST".to_string(),
                path: "/v1/responses".to_string(),
                status_code: 429,
                duration_ms: 900,
                ttfb_ms: None,
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
                },
                turns_with_usage: 2,
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
            global_station_override: None,
            stats: &stats,
        });

        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].session_id.as_deref(), Some("sid-recent"));
        assert_eq!(
            cards[0].observation_scope,
            SessionObservationScope::HostLocalEnriched
        );
        assert_eq!(cards[0].last_client_name.as_deref(), Some("Studio-Mini"));
        assert_eq!(cards[0].last_client_addr.as_deref(), Some("100.64.0.9"));
        assert_eq!(cards[1].session_id.as_deref(), Some("sid-active"));
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
    fn build_session_identity_cards_prefers_binding_defaults_for_effective_route() {
        let active = vec![ActiveRequest {
            id: 1,
            session_id: Some("sid-bound".to_string()),
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
            Some(("gpt-bound", RouteValueSource::ProfileDefault))
        );
        assert_eq!(
            cards[0]
                .effective_reasoning_effort
                .as_ref()
                .map(|value| (value.value.as_str(), value.source)),
            Some(("high", RouteValueSource::ProfileDefault))
        );
        assert_eq!(
            cards[0]
                .effective_service_tier
                .as_ref()
                .map(|value| (value.value.as_str(), value.source)),
            Some(("priority", RouteValueSource::ProfileDefault))
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
    fn build_session_identity_cards_keeps_binding_values_but_allows_global_config_override() {
        let active = vec![ActiveRequest {
            id: 1,
            session_id: Some("sid-bound".to_string()),
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
            global_station_override: Some("right"),
            stats: &HashMap::new(),
        });

        assert_eq!(cards[0].binding_profile_name.as_deref(), Some("daily"));
        assert_eq!(
            cards[0]
                .effective_model
                .as_ref()
                .map(|value| (value.value.as_str(), value.source)),
            Some(("gpt-bound", RouteValueSource::ProfileDefault))
        );
        assert_eq!(
            cards[0]
                .effective_reasoning_effort
                .as_ref()
                .map(|value| (value.value.as_str(), value.source)),
            Some(("high", RouteValueSource::ProfileDefault))
        );
        assert_eq!(
            cards[0]
                .effective_service_tier
                .as_ref()
                .map(|value| (value.value.as_str(), value.source)),
            Some(("priority", RouteValueSource::ProfileDefault))
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
            let state = ProxyState::new_with_runtime_policy(None, 1, 0, 0, 0);
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
            let state = ProxyState::new_with_runtime_policy(None, 0, 1, 0, 0);
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
}

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::sync::RwLock;
use tokio::time::{Duration, interval};

use crate::config::ServiceConfigManager;
use crate::lb::LbState;
use crate::logging::RetryInfo;
use crate::sessions;
use crate::usage::UsageMetrics;

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

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UsageBucket {
    pub requests_total: u64,
    pub requests_error: u64,
    pub duration_ms_total: u64,
    pub requests_with_usage: u64,
    pub duration_ms_with_usage_total: u64,
    pub generation_ms_total: u64,
    pub ttfb_ms_total: u64,
    pub ttfb_samples: u64,
    pub usage: UsageMetrics,
}

impl UsageBucket {
    fn record(
        &mut self,
        status_code: u16,
        duration_ms: u64,
        usage: Option<&UsageMetrics>,
        ttfb_ms: Option<u64>,
    ) {
        self.requests_total = self.requests_total.saturating_add(1);
        if status_code >= 400 {
            self.requests_error = self.requests_error.saturating_add(1);
        }
        self.duration_ms_total = self.duration_ms_total.saturating_add(duration_ms);
        if let Some(u) = usage {
            self.usage.add_assign(u);
            self.requests_with_usage = self.requests_with_usage.saturating_add(1);
            self.duration_ms_with_usage_total = self
                .duration_ms_with_usage_total
                .saturating_add(duration_ms);

            let gen_ms = match ttfb_ms {
                Some(ttfb) if ttfb > 0 && ttfb < duration_ms => duration_ms.saturating_sub(ttfb),
                _ => duration_ms,
            };
            self.generation_ms_total = self.generation_ms_total.saturating_add(gen_ms);
            if let Some(ttfb) = ttfb_ms.filter(|v| *v > 0) {
                self.ttfb_ms_total = self.ttfb_ms_total.saturating_add(ttfb);
                self.ttfb_samples = self.ttfb_samples.saturating_add(1);
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UsageRollupView {
    pub since_start: UsageBucket,
    pub by_day: Vec<(i32, UsageBucket)>,
    pub by_config: Vec<(String, UsageBucket)>,
    pub by_config_day: HashMap<String, Vec<(i32, UsageBucket)>>,
    pub by_provider: Vec<(String, UsageBucket)>,
    pub by_provider_day: HashMap<String, Vec<(i32, UsageBucket)>>,
}

#[derive(Debug, Clone, Default)]
struct UsageRollup {
    since_start: UsageBucket,
    by_day: HashMap<i32, UsageBucket>,
    by_config: HashMap<String, UsageBucket>,
    by_config_day: HashMap<String, HashMap<i32, UsageBucket>>,
    by_provider: HashMap<String, UsageBucket>,
    by_provider_day: HashMap<String, HashMap<i32, UsageBucket>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UpstreamHealth {
    pub base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ConfigHealth {
    pub checked_at_ms: u64,
    #[serde(default)]
    pub upstreams: Vec<UpstreamHealth>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct LbUpstreamView {
    pub failure_count: u32,
    pub cooldown_remaining_secs: Option<u64>,
    pub usage_exhausted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct LbConfigView {
    pub last_good_index: Option<usize>,
    pub upstreams: Vec<LbUpstreamView>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeConfigState {
    #[default]
    Normal,
    Draining,
    BreakerOpen,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct HealthCheckStatus {
    pub started_at_ms: u64,
    pub updated_at_ms: u64,
    pub total: u32,
    pub completed: u32,
    pub ok: u32,
    pub err: u32,
    pub cancel_requested: bool,
    pub canceled: bool,
    pub done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveRequest {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_base_url: Option<String>,
    pub service: String,
    pub method: String,
    pub path: String,
    pub started_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FinishedRequest {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryInfo>,
    pub service: String,
    pub method: String,
    pub path: String,
    pub status_code: u16,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u64>,
    pub ended_at_ms: u64,
}

#[derive(Debug, Clone)]
pub struct FinishRequestParams {
    pub id: u64,
    pub status_code: u16,
    pub duration_ms: u64,
    pub ended_at_ms: u64,
    pub usage: Option<UsageMetrics>,
    pub retry: Option<RetryInfo>,
    pub ttfb_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionStats {
    pub turns_total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_service_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_config_name: Option<String>,
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
    fn new(value: impl Into<String>, source: RouteValueSource) -> Self {
        Self {
            value: value.into(),
            source,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionIdentityCard {
    pub session_id: Option<String>,
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
    pub last_config_name: Option<String>,
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
    pub effective_model: Option<ResolvedRouteValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_reasoning_effort: Option<ResolvedRouteValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_service_tier: Option<ResolvedRouteValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_config_name: Option<ResolvedRouteValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_upstream_base_url: Option<ResolvedRouteValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub override_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub override_config_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub override_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub override_service_tier: Option<String>,
}

#[derive(Debug, Clone)]
struct SessionEffortOverride {
    effort: String,
    #[allow(dead_code)]
    updated_at_ms: u64,
    last_seen_ms: u64,
}

#[derive(Debug, Clone)]
struct SessionConfigOverride {
    config_name: String,
    #[allow(dead_code)]
    updated_at_ms: u64,
    last_seen_ms: u64,
}

#[derive(Debug, Clone)]
struct SessionModelOverride {
    model: String,
    #[allow(dead_code)]
    updated_at_ms: u64,
    last_seen_ms: u64,
}

#[derive(Debug, Clone)]
struct SessionServiceTierOverride {
    service_tier: String,
    #[allow(dead_code)]
    updated_at_ms: u64,
    last_seen_ms: u64,
}

#[derive(Debug, Clone)]
struct SessionBindingEntry {
    binding: SessionBinding,
}

#[derive(Debug, Clone)]
struct SessionCwdCacheEntry {
    cwd: Option<String>,
    last_seen_ms: u64,
}

#[derive(Debug, Clone, Default)]
struct ConfigMetaOverride {
    enabled: Option<bool>,
    level: Option<u8>,
    state: Option<RuntimeConfigState>,
    #[allow(dead_code)]
    updated_at_ms: u64,
}

#[derive(Debug, Clone)]
struct RuntimeDefaultProfileOverride {
    profile_name: String,
    #[allow(dead_code)]
    updated_at_ms: u64,
}

/// Runtime-only state for the proxy process.
///
/// This state is intentionally not persisted across restarts.
#[derive(Debug)]
pub struct ProxyState {
    next_request_id: AtomicU64,
    session_override_ttl_ms: u64,
    session_cwd_cache_ttl_ms: u64,
    session_cwd_cache_max_entries: usize,
    session_effort_overrides: RwLock<HashMap<String, SessionEffortOverride>>,
    session_config_overrides: RwLock<HashMap<String, SessionConfigOverride>>,
    session_model_overrides: RwLock<HashMap<String, SessionModelOverride>>,
    session_service_tier_overrides: RwLock<HashMap<String, SessionServiceTierOverride>>,
    session_bindings: RwLock<HashMap<String, SessionBindingEntry>>,
    global_config_override: RwLock<Option<String>>,
    runtime_default_profiles: RwLock<HashMap<String, RuntimeDefaultProfileOverride>>,
    config_meta_overrides: RwLock<HashMap<String, HashMap<String, ConfigMetaOverride>>>,
    session_cwd_cache: RwLock<HashMap<String, SessionCwdCacheEntry>>,
    session_stats: RwLock<HashMap<String, SessionStats>>,
    active_requests: RwLock<HashMap<u64, ActiveRequest>>,
    recent_finished: RwLock<VecDeque<FinishedRequest>>,
    usage_rollups: RwLock<HashMap<String, UsageRollup>>,
    config_health: RwLock<HashMap<String, HashMap<String, ConfigHealth>>>,
    health_checks: RwLock<HashMap<String, HashMap<String, HealthCheckStatus>>>,
    lb_states: Option<Arc<Mutex<HashMap<String, LbState>>>>,
}

impl ProxyState {
    const MAX_HEALTH_RECORDS_PER_CONFIG: usize = 200;

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

        let cwd_cache_ttl_secs = std::env::var("CODEX_HELPER_SESSION_CWD_CACHE_TTL_SECS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(12 * 60 * 60);
        let cwd_cache_ttl_ms = cwd_cache_ttl_secs.saturating_mul(1000);
        let cwd_cache_max_entries = std::env::var("CODEX_HELPER_SESSION_CWD_CACHE_MAX_ENTRIES")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(2_000);

        Arc::new(Self {
            next_request_id: AtomicU64::new(1),
            session_override_ttl_ms: ttl_ms,
            session_cwd_cache_ttl_ms: cwd_cache_ttl_ms,
            session_cwd_cache_max_entries: cwd_cache_max_entries,
            session_effort_overrides: RwLock::new(HashMap::new()),
            session_config_overrides: RwLock::new(HashMap::new()),
            session_model_overrides: RwLock::new(HashMap::new()),
            session_service_tier_overrides: RwLock::new(HashMap::new()),
            session_bindings: RwLock::new(HashMap::new()),
            global_config_override: RwLock::new(None),
            runtime_default_profiles: RwLock::new(HashMap::new()),
            config_meta_overrides: RwLock::new(HashMap::new()),
            session_cwd_cache: RwLock::new(HashMap::new()),
            session_stats: RwLock::new(HashMap::new()),
            active_requests: RwLock::new(HashMap::new()),
            recent_finished: RwLock::new(VecDeque::new()),
            usage_rollups: RwLock::new(HashMap::new()),
            config_health: RwLock::new(HashMap::new()),
            health_checks: RwLock::new(HashMap::new()),
            lb_states,
        })
    }

    pub async fn get_session_effort_override(&self, session_id: &str) -> Option<String> {
        let guard = self.session_effort_overrides.read().await;
        guard.get(session_id).map(|v| v.effort.clone())
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

    pub async fn clear_session_effort_override(&self, session_id: &str) {
        let mut guard = self.session_effort_overrides.write().await;
        guard.remove(session_id);
    }

    pub async fn list_session_effort_overrides(&self) -> HashMap<String, String> {
        let guard = self.session_effort_overrides.read().await;
        guard
            .iter()
            .map(|(k, v)| (k.clone(), v.effort.clone()))
            .collect()
    }

    pub async fn touch_session_override(&self, session_id: &str, now_ms: u64) {
        let mut guard = self.session_effort_overrides.write().await;
        if let Some(v) = guard.get_mut(session_id) {
            v.last_seen_ms = now_ms;
        }
    }

    pub async fn get_session_config_override(&self, session_id: &str) -> Option<String> {
        let guard = self.session_config_overrides.read().await;
        guard.get(session_id).map(|v| v.config_name.clone())
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

    pub async fn touch_session_binding(&self, session_id: &str, now_ms: u64) {
        let mut guard = self.session_bindings.write().await;
        if let Some(entry) = guard.get_mut(session_id) {
            entry.binding.last_seen_ms = now_ms;
        }
    }

    pub async fn set_session_config_override(
        &self,
        session_id: String,
        config_name: String,
        now_ms: u64,
    ) {
        let mut guard = self.session_config_overrides.write().await;
        guard.insert(
            session_id,
            SessionConfigOverride {
                config_name,
                updated_at_ms: now_ms,
                last_seen_ms: now_ms,
            },
        );
    }

    pub async fn clear_session_config_override(&self, session_id: &str) {
        let mut guard = self.session_config_overrides.write().await;
        guard.remove(session_id);
    }

    pub async fn list_session_config_overrides(&self) -> HashMap<String, String> {
        let guard = self.session_config_overrides.read().await;
        guard
            .iter()
            .map(|(k, v)| (k.clone(), v.config_name.clone()))
            .collect()
    }

    pub async fn touch_session_config_override(&self, session_id: &str, now_ms: u64) {
        let mut guard = self.session_config_overrides.write().await;
        if let Some(v) = guard.get_mut(session_id) {
            v.last_seen_ms = now_ms;
        }
    }

    pub async fn get_global_config_override(&self) -> Option<String> {
        let guard = self.global_config_override.read().await;
        guard.clone()
    }

    pub async fn set_global_config_override(&self, config_name: String, _now_ms: u64) {
        let mut guard = self.global_config_override.write().await;
        *guard = Some(config_name);
    }

    pub async fn clear_global_config_override(&self) {
        let mut guard = self.global_config_override.write().await;
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

    pub async fn set_config_enabled_override(
        &self,
        service_name: &str,
        config_name: String,
        enabled: bool,
        now_ms: u64,
    ) {
        let mut guard = self.config_meta_overrides.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        let entry = per_service.entry(config_name).or_default();
        entry.enabled = Some(enabled);
        entry.updated_at_ms = now_ms;
    }

    pub async fn set_config_level_override(
        &self,
        service_name: &str,
        config_name: String,
        level: u8,
        now_ms: u64,
    ) {
        let mut guard = self.config_meta_overrides.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        let entry = per_service.entry(config_name).or_default();
        entry.level = Some(level.clamp(1, 10));
        entry.updated_at_ms = now_ms;
    }

    pub async fn set_config_runtime_state_override(
        &self,
        service_name: &str,
        config_name: String,
        state: RuntimeConfigState,
        now_ms: u64,
    ) {
        let mut guard = self.config_meta_overrides.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        let entry = per_service.entry(config_name).or_default();
        entry.state = Some(state);
        entry.updated_at_ms = now_ms;
    }

    pub async fn clear_config_enabled_override(&self, service_name: &str, config_name: &str) {
        let mut guard = self.config_meta_overrides.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return;
        };
        let Some(entry) = per_service.get_mut(config_name) else {
            return;
        };
        entry.enabled = None;
        if entry.enabled.is_none() && entry.level.is_none() && entry.state.is_none() {
            per_service.remove(config_name);
        }
        if per_service.is_empty() {
            guard.remove(service_name);
        }
    }

    pub async fn clear_config_level_override(&self, service_name: &str, config_name: &str) {
        let mut guard = self.config_meta_overrides.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return;
        };
        let Some(entry) = per_service.get_mut(config_name) else {
            return;
        };
        entry.level = None;
        if entry.enabled.is_none() && entry.level.is_none() && entry.state.is_none() {
            per_service.remove(config_name);
        }
        if per_service.is_empty() {
            guard.remove(service_name);
        }
    }

    pub async fn clear_config_runtime_state_override(&self, service_name: &str, config_name: &str) {
        let mut guard = self.config_meta_overrides.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return;
        };
        let Some(entry) = per_service.get_mut(config_name) else {
            return;
        };
        entry.state = None;
        if entry.enabled.is_none() && entry.level.is_none() && entry.state.is_none() {
            per_service.remove(config_name);
        }
        if per_service.is_empty() {
            guard.remove(service_name);
        }
    }

    pub async fn get_config_meta_overrides(
        &self,
        service_name: &str,
    ) -> HashMap<String, (Option<bool>, Option<u8>)> {
        let guard = self.config_meta_overrides.read().await;
        guard
            .get(service_name)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), (v.enabled, v.level)))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default()
    }

    pub async fn get_config_runtime_state_overrides(
        &self,
        service_name: &str,
    ) -> HashMap<String, RuntimeConfigState> {
        let guard = self.config_meta_overrides.read().await;
        guard
            .get(service_name)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.state.map(|state| (k.clone(), state)))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default()
    }

    pub async fn record_config_health(
        &self,
        service_name: &str,
        config_name: String,
        health: ConfigHealth,
    ) {
        let mut guard = self.config_health.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        per_service.insert(config_name, health);
    }

    pub async fn get_config_health(&self, service_name: &str) -> HashMap<String, ConfigHealth> {
        let guard = self.config_health.read().await;
        guard.get(service_name).cloned().unwrap_or_default()
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
        let guard = self.health_checks.read().await;
        guard.get(service_name).cloned().unwrap_or_default()
    }

    pub async fn try_begin_health_check(
        &self,
        service_name: &str,
        config_name: &str,
        total: usize,
        now_ms: u64,
    ) -> bool {
        let mut guard = self.health_checks.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        if let Some(existing) = per_service.get(config_name)
            && !existing.done
        {
            return false;
        }
        per_service.insert(
            config_name.to_string(),
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

    pub async fn request_cancel_health_check(
        &self,
        service_name: &str,
        config_name: &str,
        now_ms: u64,
    ) -> bool {
        let mut guard = self.health_checks.write().await;
        let Some(per_service) = guard.get_mut(service_name) else {
            return false;
        };
        let Some(st) = per_service.get_mut(config_name) else {
            return false;
        };
        if st.done {
            return false;
        }
        st.cancel_requested = true;
        st.updated_at_ms = now_ms;
        true
    }

    pub async fn is_health_check_cancel_requested(
        &self,
        service_name: &str,
        config_name: &str,
    ) -> bool {
        let guard = self.health_checks.read().await;
        guard
            .get(service_name)
            .and_then(|m| m.get(config_name))
            .is_some_and(|s| s.cancel_requested && !s.done)
    }

    pub async fn record_health_check_result(
        &self,
        service_name: &str,
        config_name: &str,
        now_ms: u64,
        upstream: UpstreamHealth,
    ) {
        {
            let mut guard = self.config_health.write().await;
            let per_service = guard.entry(service_name.to_string()).or_default();
            let entry = per_service
                .entry(config_name.to_string())
                .or_insert_with(|| ConfigHealth {
                    checked_at_ms: now_ms,
                    upstreams: Vec::new(),
                });
            entry.checked_at_ms = entry.checked_at_ms.max(now_ms);
            entry.upstreams.push(upstream.clone());
            if entry.upstreams.len() > Self::MAX_HEALTH_RECORDS_PER_CONFIG {
                let extra = entry
                    .upstreams
                    .len()
                    .saturating_sub(Self::MAX_HEALTH_RECORDS_PER_CONFIG);
                if extra > 0 {
                    entry.upstreams.drain(0..extra);
                }
            }
        }

        let mut guard = self.health_checks.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        let st = per_service.entry(config_name.to_string()).or_default();
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

    pub async fn finish_health_check(
        &self,
        service_name: &str,
        config_name: &str,
        now_ms: u64,
        canceled: bool,
    ) {
        let mut guard = self.health_checks.write().await;
        let per_service = guard.entry(service_name.to_string()).or_default();
        let st = per_service.entry(config_name.to_string()).or_default();
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
            let config_name = v
                .get("config_name")
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
                config_name,
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
            cwd,
            model,
            reasoning_effort,
            service_tier,
            config_name: None,
            provider_id: None,
            upstream_base_url: None,
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
        config_name: String,
        provider_id: Option<String>,
        upstream_base_url: String,
    ) {
        let mut guard = self.active_requests.write().await;
        let Some(req) = guard.get_mut(&request_id) else {
            return;
        };
        req.config_name = Some(config_name);
        req.provider_id = provider_id;
        req.upstream_base_url = Some(upstream_base_url);
    }

    pub async fn finish_request(&self, params: FinishRequestParams) {
        let mut active = self.active_requests.write().await;
        let Some(req) = active.remove(&params.id) else {
            return;
        };

        let finished = FinishedRequest {
            id: params.id,
            session_id: req.session_id,
            cwd: req.cwd,
            model: req.model,
            reasoning_effort: req.reasoning_effort,
            service_tier: req.service_tier,
            config_name: req.config_name,
            provider_id: req.provider_id,
            upstream_base_url: req.upstream_base_url,
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
                .config_name
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
            entry.last_config_name = finished
                .config_name
                .clone()
                .or(entry.last_config_name.clone());
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
            config_overrides,
            model_overrides,
            service_tier_overrides,
            bindings,
            global_override,
            stats,
        ) = tokio::join!(
            self.list_active_requests(),
            self.list_recent_finished(recent_limit),
            self.list_session_effort_overrides(),
            self.list_session_config_overrides(),
            self.list_session_model_overrides(),
            self.list_session_service_tier_overrides(),
            self.list_session_bindings(),
            self.get_global_config_override(),
            self.list_session_stats(),
        );
        build_session_identity_cards_from_parts(
            &active,
            &recent,
            &overrides,
            &config_overrides,
            &model_overrides,
            &service_tier_overrides,
            &bindings,
            global_override.as_deref(),
            &stats,
        )
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
            let mut overrides = self.session_config_overrides.write().await;
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

        if self.session_override_ttl_ms > 0 && now_ms >= self.session_override_ttl_ms {
            let cutoff_binding = now_ms - self.session_override_ttl_ms;
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

fn empty_session_identity_card(session_id: Option<String>) -> SessionIdentityCard {
    SessionIdentityCard {
        session_id,
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
        last_config_name: None,
        last_upstream_base_url: None,
        last_usage: None,
        total_usage: None,
        turns_total: None,
        turns_with_usage: None,
        binding_profile_name: None,
        binding_continuity_mode: None,
        effective_model: None,
        effective_reasoning_effort: None,
        effective_service_tier: None,
        effective_config_name: None,
        effective_upstream_base_url: None,
        override_effort: None,
        override_config_name: None,
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

fn resolve_effective_config_value(
    card: &SessionIdentityCard,
    global_config_override: Option<&str>,
    binding_station_name: Option<&str>,
) -> Option<ResolvedRouteValue> {
    if let Some(value) = non_empty_trimmed(card.override_config_name.as_deref()) {
        return Some(ResolvedRouteValue::new(
            value,
            RouteValueSource::SessionOverride,
        ));
    }
    if let Some(value) = non_empty_trimmed(global_config_override) {
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
    non_empty_trimmed(card.last_config_name.as_deref())
        .map(|observed| ResolvedRouteValue::new(observed, RouteValueSource::RuntimeFallback))
}

fn apply_basic_effective_route(
    card: &mut SessionIdentityCard,
    global_config_override: Option<&str>,
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
    card.effective_config_name = resolve_effective_config_value(
        card,
        global_config_override,
        binding.and_then(|binding| binding.station_name.as_deref()),
    );
    card.effective_upstream_base_url = match (
        card.effective_config_name.as_ref(),
        non_empty_trimmed(card.last_config_name.as_deref()),
        non_empty_trimmed(card.last_upstream_base_url.as_deref()),
    ) {
        (Some(config), Some(last_config), Some(upstream)) if config.value == last_config => Some(
            ResolvedRouteValue::new(upstream, RouteValueSource::RuntimeFallback),
        ),
        _ => None,
    };
}

pub fn enrich_session_identity_cards_with_runtime(
    cards: &mut [SessionIdentityCard],
    mgr: &ServiceConfigManager,
) {
    for card in cards {
        if card.effective_config_name.is_none()
            && let Some(active) = mgr.active_config()
        {
            card.effective_config_name = Some(ResolvedRouteValue::new(
                active.name.clone(),
                RouteValueSource::RuntimeFallback,
            ));
        }

        let effective_config_name = card
            .effective_config_name
            .as_ref()
            .map(|value| value.value.as_str());
        if card.effective_upstream_base_url.is_none()
            && let Some(config_name) = effective_config_name
            && let Some(config) = mgr.configs.get(config_name)
            && config.upstreams.len() == 1
        {
            card.effective_upstream_base_url = Some(ResolvedRouteValue::new(
                config.upstreams[0].base_url.clone(),
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
        let Some(config_name) = effective_config_name else {
            continue;
        };
        let Some(last_config_name) = card.last_config_name.as_deref() else {
            continue;
        };
        if last_config_name != config_name {
            continue;
        }
        let Some(last_upstream_base_url) = card.last_upstream_base_url.as_deref() else {
            continue;
        };
        let Some(config) = mgr.configs.get(config_name) else {
            continue;
        };
        let Some(upstream) = config
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

pub fn build_session_identity_cards_from_parts(
    active: &[ActiveRequest],
    recent: &[FinishedRequest],
    overrides: &HashMap<String, String>,
    config_overrides: &HashMap<String, String>,
    model_overrides: &HashMap<String, String>,
    service_tier_overrides: &HashMap<String, String>,
    bindings: &HashMap<String, SessionBinding>,
    global_config_override: Option<&str>,
    stats: &HashMap<String, SessionStats>,
) -> Vec<SessionIdentityCard> {
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
        if entry.last_config_name.is_none() {
            entry.last_config_name = req.config_name.clone();
        }
        if entry.last_upstream_base_url.is_none() {
            entry.last_upstream_base_url = req.upstream_base_url.clone();
        }
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
            entry.last_model = r.model.clone().or(entry.last_model.clone());
            entry.last_reasoning_effort = r
                .reasoning_effort
                .clone()
                .or(entry.last_reasoning_effort.clone());
            entry.last_service_tier = r.service_tier.clone().or(entry.last_service_tier.clone());
            entry.last_provider_id = r.provider_id.clone().or(entry.last_provider_id.clone());
            entry.last_config_name = r.config_name.clone().or(entry.last_config_name.clone());
            entry.last_upstream_base_url = r
                .upstream_base_url
                .clone()
                .or(entry.last_upstream_base_url.clone());
            entry.last_usage = r.usage.clone().or(entry.last_usage.clone());
        }
        if entry.cwd.is_none() {
            entry.cwd = r.cwd.clone();
        }
    }

    for (sid, st) in stats {
        let key = Some(sid.clone());
        let entry = map
            .entry(key.clone())
            .or_insert_with(|| empty_session_identity_card(key));

        if entry.turns_total.is_none() {
            entry.turns_total = Some(st.turns_total);
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
        if entry.last_config_name.is_none() {
            entry.last_config_name = st.last_config_name.clone();
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
    }

    for (sid, eff) in overrides {
        let key = Some(sid.clone());
        let entry = map
            .entry(key.clone())
            .or_insert_with(|| empty_session_identity_card(key));
        entry.override_effort = Some(eff.clone());
    }

    for (sid, cfg_name) in config_overrides {
        let key = Some(sid.clone());
        let entry = map
            .entry(key.clone())
            .or_insert_with(|| empty_session_identity_card(key));
        entry.override_config_name = Some(cfg_name.clone());
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

    let mut cards = map.into_values().collect::<Vec<_>>();
    for card in &mut cards {
        let binding = card
            .session_id
            .as_deref()
            .and_then(|session_id| bindings.get(session_id));
        apply_basic_effective_route(card, global_config_override, binding);
    }
    cards.sort_by_key(|card| std::cmp::Reverse(session_identity_sort_key(card)));
    cards
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
            cwd: Some("G:/codes/project".to_string()),
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: Some("priority".to_string()),
            config_name: Some("right".to_string()),
            provider_id: Some("right".to_string()),
            upstream_base_url: Some("https://right.example/v1".to_string()),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            started_at_ms: 500,
        }];
        let recent = vec![
            FinishedRequest {
                id: 2,
                session_id: Some("sid-recent".to_string()),
                cwd: Some("G:/codes/other".to_string()),
                model: Some("gpt-5.3".to_string()),
                reasoning_effort: Some("high".to_string()),
                service_tier: Some("default".to_string()),
                config_name: Some("vibe".to_string()),
                provider_id: Some("vibe".to_string()),
                upstream_base_url: Some("https://vibe.example/v1".to_string()),
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
                cwd: Some("G:/codes/project".to_string()),
                model: Some("gpt-5.4".to_string()),
                reasoning_effort: Some("low".to_string()),
                service_tier: Some("flex".to_string()),
                config_name: Some("right".to_string()),
                provider_id: Some("right".to_string()),
                upstream_base_url: Some("https://right.example/v1".to_string()),
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
                last_model: Some("gpt-5.4".to_string()),
                last_reasoning_effort: Some("low".to_string()),
                last_service_tier: Some("flex".to_string()),
                last_provider_id: Some("right".to_string()),
                last_config_name: Some("right".to_string()),
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

        let cards = build_session_identity_cards_from_parts(
            &active,
            &recent,
            &overrides,
            &config_overrides,
            &model_overrides,
            &service_tier_overrides,
            &HashMap::new(),
            None,
            &stats,
        );

        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].session_id.as_deref(), Some("sid-recent"));
        assert_eq!(cards[1].session_id.as_deref(), Some("sid-active"));
        assert_eq!(cards[1].active_count, 1);
        assert_eq!(cards[1].last_status, Some(429));
        assert_eq!(cards[1].override_effort.as_deref(), Some("xhigh"));
        assert_eq!(cards[1].override_config_name.as_deref(), Some("temp"));
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
                .effective_config_name
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
            cwd: None,
            model: Some("gpt-observed".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: Some("default".to_string()),
            config_name: Some("right".to_string()),
            provider_id: None,
            upstream_base_url: None,
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

        let cards = build_session_identity_cards_from_parts(
            &active,
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &bindings,
            None,
            &HashMap::new(),
        );

        assert_eq!(cards[0].binding_profile_name.as_deref(), Some("daily"));
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
                .effective_config_name
                .as_ref()
                .map(|value| (value.value.as_str(), value.source)),
            Some(("vibe", RouteValueSource::ProfileDefault))
        );
    }

    #[test]
    fn enrich_session_identity_cards_with_runtime_applies_station_mapping_and_single_upstream() {
        let mut cards = vec![SessionIdentityCard {
            session_id: Some("sid-1".to_string()),
            last_model: Some("gpt-5.4".to_string()),
            last_config_name: Some("right".to_string()),
            last_upstream_base_url: Some("https://right.example/v1".to_string()),
            effective_model: Some(ResolvedRouteValue::new(
                "gpt-5.4",
                RouteValueSource::RequestPayload,
            )),
            effective_config_name: Some(ResolvedRouteValue::new(
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
}

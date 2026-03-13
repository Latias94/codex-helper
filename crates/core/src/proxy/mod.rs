use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use axum::Json;
use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{ConnectInfo, Path, Query};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, Uri};
use axum::middleware;
use axum::routing::{any, get, post, put};
use reqwest::Client;
use std::sync::OnceLock;
use tracing::{instrument, warn};

mod admin;
mod classify;
mod retry;
mod runtime_config;
mod stream;
#[cfg(test)]
mod tests;

use crate::config::{ProxyConfig, RetryStrategy, ServiceConfigManager};
use crate::dashboard_core::{
    ApiV1Capabilities, HostLocalControlPlaneCapabilities, SharedControlPlaneCapabilities,
    build_profile_options_from_mgr,
};
use crate::filter::RequestFilter;
use crate::lb::{LbState, LoadBalancer, SelectedUpstream};
use crate::logging::{
    AuthResolutionLog, BodyPreview, HeaderEntry, HttpDebugLog, ServiceTierLog, http_debug_options,
    http_warn_options, log_request_with_debug, log_retry_trace, make_body_preview, now_ms,
    should_include_http_warn, should_log_request_body_preview,
};
use crate::model_routing;
use crate::state::{
    ActiveRequest, FinishedRequest, ProxyState, ResolvedRouteValue, RouteDecisionProvenance,
    RouteValueSource, RuntimeConfigState,
};
use crate::usage::extract_usage_from_bytes;

use self::admin::{
    AdminAccessConfig, ProxyAdminDiscovery, admin_access_capabilities,
    reject_admin_paths_from_proxy, require_admin_access, require_admin_path_only,
};
pub use self::admin::{
    admin_base_url_from_proxy_base_url, admin_loopback_addr_for_proxy_port,
    admin_port_for_proxy_port, local_admin_base_url_for_proxy_port, local_proxy_base_url,
};
use self::classify::{class_is_health_neutral, classify_upstream_response};
use self::retry::{
    backoff_sleep, retry_info_for_chain, retry_plan, retry_sleep, should_never_retry,
    should_retry_class, should_retry_status,
};
use self::runtime_config::RuntimeConfig;
use self::stream::{SseSuccessMeta, build_sse_success_response};

pub const ADMIN_TOKEN_ENV_VAR: &str = "CODEX_HELPER_ADMIN_TOKEN";
pub const ADMIN_TOKEN_HEADER: &str = "x-codex-helper-admin-token";
pub const CLIENT_NAME_HEADER: &str = "x-codex-helper-client-name";
pub const ADMIN_PORT_OFFSET: u16 = 1000;

#[cfg(test)]
const AUTH_FILE_CACHE_MIN_CHECK_INTERVAL: Duration = Duration::from_millis(20);
#[cfg(not(test))]
const AUTH_FILE_CACHE_MIN_CHECK_INTERVAL: Duration = Duration::from_millis(800);

fn effective_runtime_config_state(
    state_overrides: &HashMap<String, RuntimeConfigState>,
    station_name: &str,
) -> RuntimeConfigState {
    state_overrides
        .get(station_name)
        .copied()
        .unwrap_or_default()
}

fn runtime_state_allows_general_routing(state: RuntimeConfigState) -> bool {
    state == RuntimeConfigState::Normal
}

fn runtime_state_allows_pinned_routing(state: RuntimeConfigState) -> bool {
    state != RuntimeConfigState::BreakerOpen
}

fn sorted_avoid_indices(avoid: &HashSet<usize>) -> Vec<usize> {
    let mut indices = avoid.iter().copied().collect::<Vec<_>>();
    indices.sort_unstable();
    indices
}

fn log_same_station_failover_trace(
    service_name: &str,
    request_id: u64,
    station_name: &str,
    upstream_total: usize,
    avoid_set: &HashSet<usize>,
    exhausted: bool,
) {
    let event = if exhausted {
        "same_station_exhausted"
    } else {
        "same_station_failover"
    };
    log_retry_trace(serde_json::json!({
        "event": event,
        "service": service_name,
        "request_id": request_id,
        "station_name": station_name,
        "upstream_total": upstream_total,
        "avoided_indices": sorted_avoid_indices(avoid_set),
        "next_action": if exhausted {
            "consider_next_station"
        } else {
            "retry_another_upstream_within_station"
        },
    }));
}

fn trim_non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn resolve_request_field_provenance(
    request_value: Option<&str>,
    override_value: Option<&str>,
    binding_value: Option<&str>,
) -> Option<ResolvedRouteValue> {
    if let Some(value) = trim_non_empty(override_value) {
        return Some(ResolvedRouteValue::new(
            value,
            RouteValueSource::SessionOverride,
        ));
    }
    if let Some(value) = trim_non_empty(binding_value) {
        return Some(ResolvedRouteValue::new(
            value,
            RouteValueSource::ProfileDefault,
        ));
    }
    trim_non_empty(request_value)
        .map(|value| ResolvedRouteValue::new(value, RouteValueSource::RequestPayload))
}

fn resolve_station_provenance(
    selected_station_name: &str,
    session_override_config: Option<&str>,
    global_config_override: Option<&str>,
    binding_station_name: Option<&str>,
) -> ResolvedRouteValue {
    if let Some(value) = trim_non_empty(session_override_config) {
        return ResolvedRouteValue::new(value, RouteValueSource::SessionOverride);
    }
    if let Some(value) = trim_non_empty(global_config_override) {
        return ResolvedRouteValue::new(value, RouteValueSource::GlobalOverride);
    }
    if let Some(value) = trim_non_empty(binding_station_name) {
        return ResolvedRouteValue::new(value, RouteValueSource::ProfileDefault);
    }
    ResolvedRouteValue::new(
        selected_station_name.to_string(),
        RouteValueSource::RuntimeFallback,
    )
}

fn build_route_decision_provenance(
    decided_at_ms: u64,
    session_binding: Option<&crate::state::SessionBinding>,
    session_override_config: Option<&str>,
    global_config_override: Option<&str>,
    override_model: Option<&str>,
    override_effort: Option<&str>,
    override_service_tier: Option<&str>,
    request_model: Option<&str>,
    effective_effort: Option<&str>,
    effective_service_tier: Option<&str>,
    selected: &SelectedUpstream,
    provider_id: Option<&str>,
) -> RouteDecisionProvenance {
    let mut effective_model = resolve_request_field_provenance(
        request_model,
        override_model,
        session_binding.and_then(|binding| binding.model.as_deref()),
    );
    if let Some(current) = effective_model.as_mut() {
        let mapped = model_routing::effective_model(
            &selected.upstream.model_mapping,
            current.value.as_str(),
        );
        if mapped != current.value {
            *current = ResolvedRouteValue::new(mapped, RouteValueSource::StationMapping);
        }
    }

    RouteDecisionProvenance {
        decided_at_ms,
        binding_profile_name: session_binding.and_then(|binding| binding.profile_name.clone()),
        binding_continuity_mode: session_binding.map(|binding| binding.continuity_mode),
        effective_model,
        effective_reasoning_effort: resolve_request_field_provenance(
            effective_effort,
            override_effort,
            session_binding.and_then(|binding| binding.reasoning_effort.as_deref()),
        ),
        effective_service_tier: resolve_request_field_provenance(
            effective_service_tier,
            override_service_tier,
            session_binding.and_then(|binding| binding.service_tier.as_deref()),
        ),
        effective_station: Some(resolve_station_provenance(
            selected.station_name.as_str(),
            session_override_config,
            global_config_override,
            session_binding.and_then(|binding| binding.station_name.as_deref()),
        )),
        effective_upstream_base_url: Some(ResolvedRouteValue::new(
            selected.upstream.base_url.clone(),
            RouteValueSource::RuntimeFallback,
        )),
        provider_id: trim_non_empty(provider_id),
    }
}

pub(super) async fn record_passive_upstream_success(
    state: &ProxyState,
    service_name: &str,
    station_name: &str,
    base_url: &str,
    status_code: u16,
) {
    state
        .record_passive_upstream_success(
            service_name,
            station_name,
            base_url,
            Some(status_code),
            now_ms(),
        )
        .await;
}

pub(super) async fn record_passive_upstream_failure(
    state: &ProxyState,
    service_name: &str,
    station_name: &str,
    base_url: &str,
    status_code: Option<u16>,
    error_class: Option<&str>,
    error: Option<String>,
) {
    if class_is_health_neutral(error_class) {
        return;
    }
    state
        .record_passive_upstream_failure(
            service_name,
            station_name,
            base_url,
            status_code,
            error_class.map(ToOwned::to_owned),
            error,
            now_ms(),
        )
        .await;
}

async fn effective_default_profile_name(
    state: &ProxyState,
    service_name: &str,
    mgr: &ServiceConfigManager,
) -> Option<String> {
    if let Some(name) = state
        .get_runtime_default_profile_override(service_name)
        .await
        && mgr.profile(name.as_str()).is_some()
    {
        return Some(name);
    }
    mgr.default_profile_ref().map(|(name, _)| name.to_string())
}

fn configured_active_station_name(mgr: &ServiceConfigManager) -> Option<String> {
    mgr.active
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

fn effective_active_station_name(mgr: &ServiceConfigManager) -> Option<String> {
    mgr.active_station().map(|cfg| cfg.name.clone())
}

#[derive(Default)]
struct JsonFileCache {
    last_check_at: Option<Instant>,
    last_path: Option<std::path::PathBuf>,
    last_mtime: Option<std::time::SystemTime>,
    value: Option<serde_json::Value>,
}

fn cached_json_file_value(
    cache: &'static OnceLock<Mutex<JsonFileCache>>,
    path: std::path::PathBuf,
) -> Option<serde_json::Value> {
    let cache = cache.get_or_init(|| Mutex::new(JsonFileCache::default()));
    let now = Instant::now();
    let mut state = match cache.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let path_changed = state.last_path.as_ref() != Some(&path);
    let should_check = path_changed
        || state
            .last_check_at
            .map(|last| now.saturating_duration_since(last) >= AUTH_FILE_CACHE_MIN_CHECK_INTERVAL)
            .unwrap_or(true);
    if !should_check {
        return state.value.clone();
    }

    let mtime = std::fs::metadata(&path)
        .ok()
        .and_then(|metadata| metadata.modified().ok());
    if !path_changed && mtime == state.last_mtime {
        state.last_check_at = Some(now);
        return state.value.clone();
    }

    let value = read_json_file(&path);
    state.last_check_at = Some(now);
    state.last_path = Some(path);
    state.last_mtime = mtime;
    state.value = value.clone();
    value
}

fn format_reqwest_error_for_retry_chain(e: &reqwest::Error) -> String {
    use std::error::Error as _;

    let mut parts: Vec<String> = Vec::new();
    let first = e.to_string();
    if !first.trim().is_empty() {
        parts.push(first);
    }

    let mut cur = e.source();
    for _ in 0..4 {
        let Some(src) = cur else { break };
        let msg = src.to_string();
        if !msg.trim().is_empty() && !parts.iter().any(|x| x == &msg) {
            parts.push(msg);
        }
        cur = src.source();
    }

    let mut flags: Vec<&'static str> = Vec::new();
    if e.is_timeout() {
        flags.push("timeout");
    }
    if e.is_connect() {
        flags.push("connect");
    }

    let mut out = if parts.is_empty() {
        "reqwest error".to_string()
    } else {
        parts.join(" | caused_by: ")
    };
    if !flags.is_empty() {
        out.push_str(" (flags: ");
        out.push_str(&flags.join(","));
        out.push(')');
    }
    out = out.replace(['\r', '\n'], " ");
    const MAX_LEN: usize = 360;
    if out.len() > MAX_LEN {
        out.truncate(MAX_LEN);
        out.push('…');
    }
    out
}

#[allow(dead_code)]
fn lb_state_snapshot_json(lb: &LoadBalancer) -> Option<serde_json::Value> {
    let map = match lb.states.lock() {
        Ok(m) => m,
        Err(e) => e.into_inner(),
    };
    let st = map.get(&lb.service.name)?;
    let now = std::time::Instant::now();
    let upstreams = (0..lb.service.upstreams.len())
        .map(|idx| {
            let cooldown_remaining_ms = st
                .cooldown_until
                .get(idx)
                .and_then(|x| *x)
                .map(|until| until.saturating_duration_since(now).as_millis() as u64)
                .filter(|&ms| ms > 0);
            serde_json::json!({
                "idx": idx,
                "failure_count": st.failure_counts.get(idx).copied(),
                "penalty_streak": st.penalty_streak.get(idx).copied(),
                "usage_exhausted": st.usage_exhausted.get(idx).copied(),
                "cooldown_remaining_ms": cooldown_remaining_ms,
            })
        })
        .collect::<Vec<_>>();
    Some(serde_json::json!({
        "last_good_index": st.last_good_index,
        "upstreams": upstreams,
    }))
}

fn read_json_file(path: &std::path::Path) -> Option<serde_json::Value> {
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    if text.trim().is_empty() {
        return None;
    }
    serde_json::from_str(&text).ok()
}

fn codex_auth_json_value(key: &str) -> Option<String> {
    static CACHE: OnceLock<Mutex<JsonFileCache>> = OnceLock::new();
    let v = cached_json_file_value(&CACHE, crate::config::codex_auth_path());
    let obj = v.as_ref()?.as_object()?;
    obj.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

fn claude_settings_env_value(key: &str) -> Option<String> {
    static CACHE: OnceLock<Mutex<JsonFileCache>> = OnceLock::new();
    let v = cached_json_file_value(&CACHE, crate::config::claude_settings_path());
    let obj = v.as_ref()?.as_object()?;
    let env_obj = obj.get("env")?.as_object()?;
    env_obj
        .get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

fn resolve_auth_token_with_source(
    service_name: &str,
    auth: &crate::config::UpstreamAuth,
    client_has_auth: bool,
) -> (Option<String>, String) {
    if let Some(token) = auth.auth_token.as_deref()
        && !token.trim().is_empty()
    {
        return (Some(token.to_string()), "inline".to_string());
    }

    if let Some(env_name) = auth.auth_token_env.as_deref()
        && !env_name.trim().is_empty()
    {
        if let Ok(v) = std::env::var(env_name)
            && !v.trim().is_empty()
        {
            return (Some(v), format!("env:{env_name}"));
        }

        let file_value = match service_name {
            "codex" => codex_auth_json_value(env_name),
            "claude" => claude_settings_env_value(env_name),
            _ => None,
        };
        if let Some(v) = file_value
            && !v.trim().is_empty()
        {
            let src = match service_name {
                "codex" => format!("codex_auth_json:{env_name}"),
                "claude" => format!("claude_settings_env:{env_name}"),
                _ => format!("file:{env_name}"),
            };
            return (Some(v), src);
        }

        if client_has_auth {
            return (None, format!("client_passthrough (missing_env:{env_name})"));
        }
        return (None, format!("missing_env:{env_name}"));
    }

    if client_has_auth {
        (None, "client_passthrough".to_string())
    } else {
        (None, "none".to_string())
    }
}

fn resolve_api_key_with_source(
    service_name: &str,
    auth: &crate::config::UpstreamAuth,
    client_has_x_api_key: bool,
) -> (Option<String>, String) {
    if let Some(key) = auth.api_key.as_deref()
        && !key.trim().is_empty()
    {
        return (Some(key.to_string()), "inline".to_string());
    }

    if let Some(env_name) = auth.api_key_env.as_deref()
        && !env_name.trim().is_empty()
    {
        if let Ok(v) = std::env::var(env_name)
            && !v.trim().is_empty()
        {
            return (Some(v), format!("env:{env_name}"));
        }

        let file_value = match service_name {
            "codex" => codex_auth_json_value(env_name),
            "claude" => claude_settings_env_value(env_name),
            _ => None,
        };
        if let Some(v) = file_value
            && !v.trim().is_empty()
        {
            let src = match service_name {
                "codex" => format!("codex_auth_json:{env_name}"),
                "claude" => format!("claude_settings_env:{env_name}"),
                _ => format!("file:{env_name}"),
            };
            return (Some(v), src);
        }

        if client_has_x_api_key {
            return (None, format!("client_passthrough (missing_env:{env_name})"));
        }
        return (None, format!("missing_env:{env_name}"));
    }

    if client_has_x_api_key {
        (None, "client_passthrough".to_string())
    } else {
        (None, "none".to_string())
    }
}

fn is_hop_by_hop_header(name_lower: &str) -> bool {
    matches!(
        name_lower,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn hop_by_hop_connection_tokens(headers: &HeaderMap) -> Vec<String> {
    let mut out = Vec::new();
    for value in headers.get_all("connection").iter() {
        let Ok(s) = value.to_str() else {
            continue;
        };
        for token in s.split(',').map(|t| t.trim()).filter(|t| !t.is_empty()) {
            out.push(token.to_ascii_lowercase());
        }
    }
    out
}

fn filter_request_headers(src: &HeaderMap) -> HeaderMap {
    let extra = hop_by_hop_connection_tokens(src);
    let mut out = HeaderMap::new();
    for (name, value) in src.iter() {
        let name_lower = name.as_str().to_ascii_lowercase();
        if name_lower == "host"
            || name_lower == "content-length"
            || is_hop_by_hop_header(&name_lower)
        {
            continue;
        }
        if extra.iter().any(|t| t == &name_lower) {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

fn filter_response_headers(src: &HeaderMap) -> HeaderMap {
    let extra = hop_by_hop_connection_tokens(src);
    let mut out = HeaderMap::new();
    for (name, value) in src.iter() {
        let name_lower = name.as_str().to_ascii_lowercase();
        // reqwest 可能会自动解压响应体；为避免 content-length/content-encoding 与实际 body 不一致，这里不透传它们。
        if is_hop_by_hop_header(&name_lower)
            || name_lower == "content-length"
            || name_lower == "content-encoding"
        {
            continue;
        }
        if extra.iter().any(|t| t == &name_lower) {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

fn header_map_to_entries(headers: &HeaderMap) -> Vec<HeaderEntry> {
    fn is_sensitive(name_lower: &str) -> bool {
        matches!(
            name_lower,
            "authorization"
                | "proxy-authorization"
                | "cookie"
                | "set-cookie"
                | "x-api-key"
                | "x-forwarded-api-key"
                | "x-goog-api-key"
        )
    }

    let mut out = Vec::new();
    for (name, value) in headers.iter() {
        let name_lower = name.as_str().to_ascii_lowercase();
        let v = if is_sensitive(name_lower.as_str()) {
            "[REDACTED]".to_string()
        } else {
            String::from_utf8_lossy(value.as_bytes()).into_owned()
        };
        out.push(HeaderEntry {
            name: name.as_str().to_string(),
            value: v,
        });
    }
    out
}

#[derive(Clone)]
struct HttpDebugBase {
    debug_max_body_bytes: usize,
    warn_max_body_bytes: usize,
    #[allow(dead_code)]
    request_body_len: usize,
    #[allow(dead_code)]
    upstream_request_body_len: usize,
    client_uri: String,
    target_url: String,
    client_headers: Vec<HeaderEntry>,
    upstream_request_headers: Vec<HeaderEntry>,
    auth_resolution: Option<AuthResolutionLog>,
    client_body_debug: Option<BodyPreview>,
    upstream_request_body_debug: Option<BodyPreview>,
    client_body_warn: Option<BodyPreview>,
    upstream_request_body_warn: Option<BodyPreview>,
}

fn warn_http_debug(status_code: u16, http_debug: &HttpDebugLog) {
    let max_chars = 2048usize;
    let Ok(mut json) = serde_json::to_string(http_debug) else {
        return;
    };
    if json.chars().count() > max_chars {
        json = json.chars().take(max_chars).collect::<String>() + "...[TRUNCATED_FOR_LOG]";
    }
    warn!("upstream non-2xx http_debug={json} status_code={status_code}");
}

/// Generic proxy service; currently used by both Codex and Claude.
#[derive(Clone)]
pub struct ProxyService {
    pub client: Client,
    config: Arc<RuntimeConfig>,
    pub service_name: &'static str,
    lb_states: Arc<Mutex<HashMap<String, LbState>>>,
    filter: RequestFilter,
    state: Arc<ProxyState>,
}

impl ProxyService {
    pub fn new(
        client: Client,
        config: Arc<ProxyConfig>,
        service_name: &'static str,
        lb_states: Arc<Mutex<HashMap<String, LbState>>>,
    ) -> Self {
        let state = ProxyState::new_with_lb_states(Some(lb_states.clone()));
        ProxyState::spawn_cleanup_task(state.clone());
        if !cfg!(test) {
            let state = state.clone();
            let log_path = crate::config::proxy_home_dir()
                .join("logs")
                .join("requests.jsonl");
            let mut base_url_to_provider_id = HashMap::new();
            let mgr = match service_name {
                "claude" => &config.claude,
                _ => &config.codex,
            };
            for svc in mgr.stations().values() {
                for up in &svc.upstreams {
                    if let Some(pid) = up.tags.get("provider_id") {
                        base_url_to_provider_id.insert(up.base_url.clone(), pid.clone());
                    }
                }
            }
            tokio::spawn(async move {
                let _ = state
                    .replay_usage_from_requests_log(service_name, log_path, base_url_to_provider_id)
                    .await;
            });
        }
        Self {
            client,
            config: Arc::new(RuntimeConfig::new(config)),
            service_name,
            lb_states,
            filter: RequestFilter::new(),
            state,
        }
    }

    fn service_manager<'a>(&self, cfg: &'a ProxyConfig) -> &'a ServiceConfigManager {
        match self.service_name {
            "codex" => &cfg.codex,
            "claude" => &cfg.claude,
            _ => &cfg.codex,
        }
    }

    async fn ensure_default_session_binding(
        &self,
        mgr: &ServiceConfigManager,
        session_id: &str,
        now_ms: u64,
    ) -> Option<crate::state::SessionBinding> {
        if let Some(binding) = self.state.get_session_binding(session_id).await {
            self.state.touch_session_binding(session_id, now_ms).await;
            return Some(binding);
        }

        let profile_name =
            effective_default_profile_name(self.state.as_ref(), self.service_name, mgr).await?;
        let profile = crate::config::resolve_service_profile(mgr, profile_name.as_str()).ok()?;
        let binding = crate::state::SessionBinding {
            session_id: session_id.to_string(),
            profile_name: Some(profile_name),
            station_name: profile.station.clone(),
            model: profile.model.clone(),
            reasoning_effort: profile.reasoning_effort.clone(),
            service_tier: profile.service_tier.clone(),
            continuity_mode: crate::state::SessionContinuityMode::DefaultProfile,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            last_seen_ms: now_ms,
        };
        self.state.set_session_binding(binding.clone()).await;
        Some(binding)
    }

    async fn pinned_config(
        &self,
        mgr: &ServiceConfigManager,
        session_id: Option<&str>,
    ) -> Option<(String, &'static str)> {
        if let Some(sid) = session_id
            && let Some(name) = self.state.get_session_station_override(sid).await
            && !name.trim().is_empty()
        {
            return Some((name, "session"));
        }
        if let Some(name) = self.state.get_global_station_override().await
            && !name.trim().is_empty()
        {
            return Some((name, "global"));
        }
        if let Some(sid) = session_id
            && let Some(binding) = self.state.get_session_binding(sid).await
            && let Some(name) = binding.station_name
            && !name.trim().is_empty()
            && mgr.contains_station(name.as_str())
        {
            return Some((name, "profile_default"));
        }
        None
    }

    async fn lbs_for_request(
        &self,
        cfg: &ProxyConfig,
        session_id: Option<&str>,
    ) -> Vec<LoadBalancer> {
        let mgr = self.service_manager(cfg);
        let meta_overrides = self
            .state
            .get_station_meta_overrides(self.service_name)
            .await;
        let state_overrides = self
            .state
            .get_station_runtime_state_overrides(self.service_name)
            .await;
        if let Some((name, source)) = self.pinned_config(mgr, session_id).await {
            let runtime_state = effective_runtime_config_state(&state_overrides, name.as_str());
            if !runtime_state_allows_pinned_routing(runtime_state) {
                log_retry_trace(serde_json::json!({
                    "event": "lbs_for_request",
                    "service": self.service_name,
                    "session_id": session_id,
                    "mode": "pinned_blocked_breaker_open",
                    "pinned_source": source,
                    "pinned_name": name,
                    "runtime_state": "breaker_open",
                    "active_station": mgr.active.as_deref(),
                    "station_count": mgr.station_count(),
                }));
                return Vec::new();
            }
            if let Some(svc) = mgr.station(&name).or_else(|| mgr.active_station()).cloned() {
                log_retry_trace(serde_json::json!({
                    "event": "lbs_for_request",
                    "service": self.service_name,
                    "session_id": session_id,
                    "mode": "pinned",
                    "pinned_source": source,
                    "pinned_name": name,
                    "runtime_state": format!("{runtime_state:?}").to_ascii_lowercase(),
                    "selected_station": svc.name,
                    "selected_level": svc.level.clamp(1, 10),
                    "selected_upstreams": svc.upstreams.len(),
                    "active_station": mgr.active.as_deref(),
                    "station_count": mgr.station_count(),
                }));
                return vec![LoadBalancer::new(Arc::new(svc), self.lb_states.clone())];
            }
            log_retry_trace(serde_json::json!({
                "event": "lbs_for_request",
                "service": self.service_name,
                "session_id": session_id,
                "mode": "pinned",
                "pinned_source": source,
                "pinned_name": name,
                "selected_station": null,
                "active_station": mgr.active.as_deref(),
                "station_count": mgr.station_count(),
                "note": "pinned_station_not_found",
            }));
            return Vec::new();
        }

        let active_name = mgr.active.as_deref();
        let mut configs = mgr
            .stations()
            .iter()
            .filter(|(name, svc)| {
                let (enabled_ovr, _) = meta_overrides
                    .get(name.as_str())
                    .copied()
                    .unwrap_or((None, None));
                let enabled = enabled_ovr.unwrap_or(svc.enabled);
                let runtime_state = effective_runtime_config_state(&state_overrides, name.as_str());
                !svc.upstreams.is_empty()
                    && runtime_state_allows_general_routing(runtime_state)
                    && (enabled || active_name.is_some_and(|n| n == name.as_str()))
            })
            .collect::<Vec<_>>();

        let has_multi_level = {
            let mut levels = configs
                .iter()
                .map(|(name, svc)| {
                    let (_, level_ovr) = meta_overrides
                        .get(name.as_str())
                        .copied()
                        .unwrap_or((None, None));
                    level_ovr.unwrap_or(svc.level).clamp(1, 10)
                })
                .collect::<Vec<_>>();
            levels.sort_unstable();
            levels.dedup();
            levels.len() > 1
        };

        if !has_multi_level {
            let eligible_details = || {
                configs
                    .iter()
                    .map(|(name, svc)| {
                        let (_, level_ovr) = meta_overrides
                            .get(name.as_str())
                            .copied()
                            .unwrap_or((None, None));
                        serde_json::json!({
                            "name": (*name).clone(),
                            "level": level_ovr.unwrap_or(svc.level).clamp(1, 10),
                            "enabled": svc.enabled,
                            "runtime_state": format!(
                                "{:?}",
                                effective_runtime_config_state(&state_overrides, name.as_str())
                            )
                            .to_ascii_lowercase(),
                            "upstreams": svc.upstreams.len(),
                        })
                    })
                    .collect::<Vec<_>>()
            };

            let mut ordered = configs
                .iter()
                .map(|(name, svc)| ((*name).clone(), (*svc).clone()))
                .collect::<Vec<_>>();
            ordered.sort_by(|(a, _), (b, _)| a.cmp(b));
            if let Some(active) = active_name
                && let Some(pos) = ordered.iter().position(|(n, _)| n == active)
            {
                let item = ordered.remove(pos);
                ordered.insert(0, item);
            }

            let lbs = ordered
                .into_iter()
                .map(|(_, svc)| LoadBalancer::new(Arc::new(svc), self.lb_states.clone()))
                .collect::<Vec<_>>();
            if !lbs.is_empty() {
                log_retry_trace(serde_json::json!({
                    "event": "lbs_for_request",
                    "service": self.service_name,
                    "session_id": session_id,
                    "mode": "single_level_multi",
                    "active_station": active_name,
                    "selected_stations": lbs.iter().map(|lb| lb.service.name.clone()).collect::<Vec<_>>(),
                    "eligible_stations": configs.iter().map(|(n, _)| (*n).clone()).collect::<Vec<_>>(),
                    "eligible_details": eligible_details(),
                    "eligible_count": configs.len(),
                }));
                return lbs;
            }

            if let Some(svc) = mgr
                .active_station()
                .filter(|svc| {
                    runtime_state_allows_general_routing(effective_runtime_config_state(
                        &state_overrides,
                        svc.name.as_str(),
                    ))
                })
                .cloned()
            {
                log_retry_trace(serde_json::json!({
                    "event": "lbs_for_request",
                    "service": self.service_name,
                    "session_id": session_id,
                    "mode": "single_level_fallback_active_station",
                    "active_station": active_name,
                    "selected_station": svc.name,
                    "selected_level": svc.level.clamp(1, 10),
                    "selected_upstreams": svc.upstreams.len(),
                    "eligible_stations": configs.iter().map(|(n, _)| (*n).clone()).collect::<Vec<_>>(),
                    "eligible_details": eligible_details(),
                    "eligible_count": configs.len(),
                }));
                return vec![LoadBalancer::new(Arc::new(svc), self.lb_states.clone())];
            }

            log_retry_trace(serde_json::json!({
                "event": "lbs_for_request",
                "service": self.service_name,
                "session_id": session_id,
                "mode": "single_level_empty",
                "active_station": active_name,
                "eligible_stations": configs.iter().map(|(n, _)| (*n).clone()).collect::<Vec<_>>(),
                "eligible_details": eligible_details(),
                "eligible_count": configs.len(),
            }));
            return Vec::new();
        }

        configs.sort_by(|(a_name, a), (b_name, b)| {
            let a_level = meta_overrides
                .get(a_name.as_str())
                .and_then(|(_, l)| *l)
                .unwrap_or(a.level)
                .clamp(1, 10);
            let b_level = meta_overrides
                .get(b_name.as_str())
                .and_then(|(_, l)| *l)
                .unwrap_or(b.level)
                .clamp(1, 10);
            let a_active = active_name.is_some_and(|n| n == a_name.as_str());
            let b_active = active_name.is_some_and(|n| n == b_name.as_str());
            a_level
                .cmp(&b_level)
                .then_with(|| b_active.cmp(&a_active))
                .then_with(|| a_name.cmp(b_name))
        });

        let lbs = configs
            .into_iter()
            .map(|(_, svc)| LoadBalancer::new(Arc::new(svc.clone()), self.lb_states.clone()))
            .collect::<Vec<_>>();
        if !lbs.is_empty() {
            log_retry_trace(serde_json::json!({
                "event": "lbs_for_request",
                "service": self.service_name,
                "session_id": session_id,
                "mode": "multi_level",
                "active_station": active_name,
                "eligible_stations": lbs.iter().map(|lb| serde_json::json!({
                    "name": lb.service.name,
                    "level": lb.service.level.clamp(1, 10),
                    "upstreams": lb.service.upstreams.len(),
                })).collect::<Vec<_>>(),
                "eligible_count": lbs.len(),
            }));
            return lbs;
        }

        if let Some(svc) = mgr
            .active_station()
            .filter(|svc| {
                runtime_state_allows_general_routing(effective_runtime_config_state(
                    &state_overrides,
                    svc.name.as_str(),
                ))
            })
            .cloned()
        {
            log_retry_trace(serde_json::json!({
                "event": "lbs_for_request",
                "service": self.service_name,
                "session_id": session_id,
                "mode": "multi_level_fallback_active_station",
                "active_station": active_name,
                "selected_station": svc.name,
                "selected_level": svc.level.clamp(1, 10),
                "selected_upstreams": svc.upstreams.len(),
            }));
            return vec![LoadBalancer::new(Arc::new(svc), self.lb_states.clone())];
        }
        log_retry_trace(serde_json::json!({
            "event": "lbs_for_request",
            "service": self.service_name,
            "session_id": session_id,
            "mode": "multi_level_empty",
            "active_station": active_name,
        }));
        Vec::new()
    }

    fn build_target(
        &self,
        upstream: &SelectedUpstream,
        uri: &Uri,
    ) -> Result<(reqwest::Url, HeaderMap)> {
        let base = upstream.upstream.base_url.trim_end_matches('/').to_string();

        let base_url = reqwest::Url::parse(&base)
            .map_err(|e| anyhow!("invalid upstream base_url {base}: {e}"))?;
        let base_path = base_url.path().trim_end_matches('/').to_string();

        let mut path = uri.path().to_string();
        if !base_path.is_empty()
            && base_path != "/"
            && (path == base_path || path.starts_with(&format!("{base_path}/")))
        {
            // If the incoming request path already contains the base_url path prefix,
            // strip it to avoid double-prefixing (e.g. base_url=/v1 and request=/v1/responses).
            let rest = &path[base_path.len()..];
            path = if rest.is_empty() {
                "/".to_string()
            } else {
                rest.to_string()
            };
            if !path.starts_with('/') {
                path = format!("/{path}");
            }
        }
        let path_and_query = if let Some(q) = uri.query() {
            format!("{path}?{q}")
        } else {
            path
        };

        let full = format!("{base}{path_and_query}");
        let url =
            reqwest::Url::parse(&full).map_err(|e| anyhow!("invalid upstream url {full}: {e}"))?;

        // ensure query preserved (Url::parse already includes it)
        let headers = HeaderMap::new();
        Ok((url, headers))
    }

    pub fn state_handle(&self) -> Arc<ProxyState> {
        self.state.clone()
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn extract_session_id(headers: &HeaderMap) -> Option<String> {
    header_str(headers, "session_id")
        .or_else(|| header_str(headers, "conversation_id"))
        .map(|s| s.to_string())
}

fn normalize_client_identity_value(value: &str, max_chars: usize) -> Option<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out = trimmed.to_string();
    if out.chars().count() > max_chars {
        out = out.chars().take(max_chars).collect::<String>();
    }
    Some(out)
}

fn extract_client_name(headers: &HeaderMap) -> Option<String> {
    header_str(headers, CLIENT_NAME_HEADER)
        .and_then(|value| normalize_client_identity_value(value, 80))
        .or_else(|| {
            header_str(headers, "user-agent")
                .and_then(|value| normalize_client_identity_value(value, 120))
        })
}

fn extract_client_addr(extensions: &axum::http::Extensions) -> Option<String> {
    extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip().to_string())
        .and_then(|value| normalize_client_identity_value(value.as_str(), 64))
}

fn extract_reasoning_effort_from_request_body(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("reasoning")
        .and_then(|r| r.get("effort"))
        .and_then(|e| e.as_str())
        .map(|s| s.to_string())
}

fn extract_model_from_request_body(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("model")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
}

fn extract_service_tier_from_request_body(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("service_tier")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
}

fn extract_service_tier_from_response_value(value: &serde_json::Value) -> Option<String> {
    value
        .get("service_tier")
        .and_then(|v| v.as_str())
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("service_tier"))
                .and_then(|v| v.as_str())
        })
        .map(|s| s.to_string())
}

pub(super) fn extract_service_tier_from_response_body(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    extract_service_tier_from_response_value(&value)
}

pub(super) fn scan_service_tier_from_sse_bytes_incremental(
    data: &[u8],
    scan_pos: &mut usize,
    last: &mut Option<String>,
) {
    let mut i = (*scan_pos).min(data.len());

    while i < data.len() {
        let Some(rel_end) = data[i..].iter().position(|b| *b == b'\n') else {
            break;
        };
        let end = i + rel_end;
        let mut line = &data[i..end];
        i = end.saturating_add(1);

        if line.ends_with(b"\r") {
            line = &line[..line.len().saturating_sub(1)];
        }
        if line.is_empty() {
            continue;
        }

        const DATA_PREFIX: &[u8] = b"data:";
        if !line.starts_with(DATA_PREFIX) {
            continue;
        }
        let mut payload = &line[DATA_PREFIX.len()..];
        while !payload.is_empty() && payload[0].is_ascii_whitespace() {
            payload = &payload[1..];
        }
        if payload.is_empty() || payload == b"[DONE]" {
            continue;
        }

        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload)
            && let Some(service_tier) = extract_service_tier_from_response_value(&value)
        {
            *last = Some(service_tier);
        }
    }

    *scan_pos = i;
}

fn apply_reasoning_effort_override(body: &[u8], effort: &str) -> Option<Vec<u8>> {
    let mut v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let reasoning = v.get_mut("reasoning").and_then(|r| r.as_object_mut());
    if let Some(obj) = reasoning {
        obj.insert(
            "effort".to_string(),
            serde_json::Value::String(effort.to_string()),
        );
    } else {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "effort".to_string(),
            serde_json::Value::String(effort.to_string()),
        );
        v.as_object_mut()?
            .insert("reasoning".to_string(), serde_json::Value::Object(obj));
    }
    serde_json::to_vec(&v).ok()
}

fn apply_model_override(body: &[u8], model: &str) -> Option<Vec<u8>> {
    let mut v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.as_object_mut()?.insert(
        "model".to_string(),
        serde_json::Value::String(model.to_string()),
    );
    serde_json::to_vec(&v).ok()
}

fn apply_service_tier_override(body: &[u8], service_tier: &str) -> Option<Vec<u8>> {
    let mut v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.as_object_mut()?.insert(
        "service_tier".to_string(),
        serde_json::Value::String(service_tier.to_string()),
    );
    serde_json::to_vec(&v).ok()
}

#[instrument(skip_all, fields(service = %proxy.service_name))]
pub async fn handle_proxy(
    proxy: ProxyService,
    req: Request<Body>,
) -> Result<Response<Body>, (StatusCode, String)> {
    let start = Instant::now();
    let started_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let (parts, body) = req.into_parts();
    let client_addr = extract_client_addr(&parts.extensions);
    let uri = parts.uri;
    let method = parts.method;
    let client_headers = parts.headers;
    let client_headers_entries_cache: OnceLock<Vec<HeaderEntry>> = OnceLock::new();

    let session_id = extract_session_id(&client_headers);
    let client_name = extract_client_name(&client_headers);

    proxy.config.maybe_reload_from_disk().await;
    let cfg_snapshot = proxy.config.snapshot().await;
    let mgr = proxy.service_manager(cfg_snapshot.as_ref());
    let session_binding = if let Some(id) = session_id.as_deref() {
        proxy
            .ensure_default_session_binding(mgr, id, started_at_ms)
            .await
    } else {
        None
    };
    let lbs = proxy
        .lbs_for_request(cfg_snapshot.as_ref(), session_id.as_deref())
        .await;
    if lbs.is_empty() {
        let dur = start.elapsed().as_millis() as u64;
        let status = StatusCode::BAD_GATEWAY;
        let client_headers_entries = client_headers_entries_cache
            .get_or_init(|| header_map_to_entries(&client_headers))
            .clone();
        let http_debug = if should_include_http_warn(status.as_u16()) {
            Some(HttpDebugLog {
                request_body_len: None,
                upstream_request_body_len: None,
                upstream_headers_ms: None,
                upstream_first_chunk_ms: None,
                upstream_body_read_ms: None,
                upstream_error_class: Some("no_routable_station".to_string()),
                upstream_error_hint: Some(
                    "未找到任何可用的上游站点（active_station 未设置，或目标站点没有可用 upstream）。"
                        .to_string(),
                ),
                upstream_cf_ray: None,
                client_uri: uri.to_string(),
                target_url: "-".to_string(),
                client_headers: client_headers_entries,
                upstream_request_headers: Vec::new(),
                auth_resolution: None,
                client_body: None,
                upstream_request_body: None,
                upstream_response_headers: None,
                upstream_response_body: None,
                upstream_error: Some("no routable station".to_string()),
            })
        } else {
            None
        };
        log_request_with_debug(
            proxy.service_name,
            method.as_str(),
            uri.path(),
            status.as_u16(),
            dur,
            None,
            "-",
            None,
            "-",
            session_id.clone(),
            None,
            None,
            ServiceTierLog::default(),
            None,
            None,
            http_debug,
        );
        return Err((status, "no routable station".to_string()));
    }
    let client_content_type = client_headers
        .get("content-type")
        .and_then(|v| v.to_str().ok());

    // Detect streaming (SSE).
    let is_stream = client_headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.contains("text/event-stream"))
        .unwrap_or(false);

    let path = uri.path();
    let is_responses_path = path.ends_with("/responses");
    let is_user_turn = method == Method::POST && is_responses_path;
    let is_codex_service = proxy.service_name == "codex";

    let cwd = if let Some(id) = session_id.as_deref() {
        proxy.state.resolve_session_cwd(id).await
    } else {
        None
    };
    if let Some(id) = session_id.as_deref() {
        proxy.state.touch_session_override(id, started_at_ms).await;
        proxy
            .state
            .touch_session_station_override(id, started_at_ms)
            .await;
        proxy
            .state
            .touch_session_model_override(id, started_at_ms)
            .await;
        proxy
            .state
            .touch_session_service_tier_override(id, started_at_ms)
            .await;
        proxy.state.touch_session_binding(id, started_at_ms).await;
    }
    let session_override_config = if let Some(id) = session_id.as_deref() {
        proxy.state.get_session_station_override(id).await
    } else {
        None
    };
    let global_station_override = proxy.state.get_global_station_override().await;

    // Read request body and apply filters.
    let raw_body = match to_bytes(body, 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            let dur = start.elapsed().as_millis() as u64;
            let status = StatusCode::BAD_REQUEST;
            let err_str = e.to_string();
            let client_headers_entries = client_headers_entries_cache
                .get_or_init(|| header_map_to_entries(&client_headers))
                .clone();
            let http_debug = if should_include_http_warn(status.as_u16()) {
                Some(HttpDebugLog {
                    request_body_len: None,
                    upstream_request_body_len: None,
                    upstream_headers_ms: None,
                    upstream_first_chunk_ms: None,
                    upstream_body_read_ms: None,
                    upstream_error_class: Some("client_body_read_error".to_string()),
                    upstream_error_hint: Some(
                        "读取客户端请求 body 失败（可能超过大小限制或连接中断）。".to_string(),
                    ),
                    upstream_cf_ray: None,
                    client_uri: uri.to_string(),
                    target_url: "-".to_string(),
                    client_headers: client_headers_entries,
                    upstream_request_headers: Vec::new(),
                    auth_resolution: None,
                    client_body: None,
                    upstream_request_body: None,
                    upstream_response_headers: None,
                    upstream_response_body: None,
                    upstream_error: Some(err_str.clone()),
                })
            } else {
                None
            };
            log_request_with_debug(
                proxy.service_name,
                method.as_str(),
                uri.path(),
                status.as_u16(),
                dur,
                None,
                "-",
                None,
                "-",
                session_id.clone(),
                cwd.clone(),
                None,
                ServiceTierLog::default(),
                None,
                None,
                http_debug,
            );
            return Err((status, err_str));
        }
    };
    let original_effort = extract_reasoning_effort_from_request_body(&raw_body);
    let override_effort = if let Some(id) = session_id.as_deref() {
        proxy.state.get_session_effort_override(id).await
    } else {
        None
    };
    let binding_effort = session_binding
        .as_ref()
        .and_then(|binding| binding.reasoning_effort.as_deref());
    let mut body_for_upstream = match (override_effort.as_deref(), binding_effort) {
        (Some(effort), _) => Bytes::from(
            apply_reasoning_effort_override(&raw_body, effort)
                .unwrap_or_else(|| raw_body.as_ref().to_vec()),
        ),
        (None, Some(effort)) => Bytes::from(
            apply_reasoning_effort_override(&raw_body, effort)
                .unwrap_or_else(|| raw_body.as_ref().to_vec()),
        ),
        (None, None) => raw_body.clone(),
    };
    let effective_effort = extract_reasoning_effort_from_request_body(body_for_upstream.as_ref())
        .or(original_effort.clone());

    let override_model = if let Some(id) = session_id.as_deref() {
        proxy.state.get_session_model_override(id).await
    } else {
        None
    };
    let binding_model = session_binding
        .as_ref()
        .and_then(|binding| binding.model.as_deref());
    if let Some(model) = override_model.as_deref() {
        body_for_upstream = Bytes::from(
            apply_model_override(body_for_upstream.as_ref(), model)
                .unwrap_or_else(|| body_for_upstream.as_ref().to_vec()),
        );
    } else if let Some(model) = binding_model {
        body_for_upstream = Bytes::from(
            apply_model_override(body_for_upstream.as_ref(), model)
                .unwrap_or_else(|| body_for_upstream.as_ref().to_vec()),
        );
    }

    let original_service_tier = extract_service_tier_from_request_body(&raw_body);
    let override_service_tier = if let Some(id) = session_id.as_deref() {
        proxy.state.get_session_service_tier_override(id).await
    } else {
        None
    };
    let binding_service_tier = session_binding
        .as_ref()
        .and_then(|binding| binding.service_tier.as_deref());
    if let Some(service_tier) = override_service_tier.as_deref() {
        body_for_upstream = Bytes::from(
            apply_service_tier_override(body_for_upstream.as_ref(), service_tier)
                .unwrap_or_else(|| body_for_upstream.as_ref().to_vec()),
        );
    } else if let Some(service_tier) = binding_service_tier {
        body_for_upstream = Bytes::from(
            apply_service_tier_override(body_for_upstream.as_ref(), service_tier)
                .unwrap_or_else(|| body_for_upstream.as_ref().to_vec()),
        );
    }

    let request_model = extract_model_from_request_body(body_for_upstream.as_ref());
    let effective_service_tier = extract_service_tier_from_request_body(body_for_upstream.as_ref())
        .or(original_service_tier.clone());
    let base_service_tier = ServiceTierLog {
        requested: original_service_tier.clone(),
        effective: effective_service_tier.clone(),
        actual: None,
    };
    let request_body_len = raw_body.len();

    let debug_opt = http_debug_options();
    let warn_opt = http_warn_options();
    let debug_max = if debug_opt.enabled {
        debug_opt.max_body_bytes
    } else {
        0
    };
    let warn_max = if warn_opt.enabled {
        warn_opt.max_body_bytes
    } else {
        0
    };
    let request_body_previews = should_log_request_body_preview();
    let client_body_debug = if request_body_previews && debug_max > 0 {
        Some(make_body_preview(&raw_body, client_content_type, debug_max))
    } else {
        None
    };
    let client_body_warn = if request_body_previews && warn_max > 0 {
        Some(make_body_preview(&raw_body, client_content_type, warn_max))
    } else {
        None
    };

    let request_id = proxy
        .state
        .begin_request(
            proxy.service_name,
            method.as_str(),
            uri.path(),
            session_id.clone(),
            client_name.clone(),
            client_addr.clone(),
            cwd.clone(),
            request_model.clone(),
            effective_effort.clone(),
            effective_service_tier.clone(),
            started_at_ms,
        )
        .await;

    let retry_cfg = cfg_snapshot.retry.resolve();
    let plan = retry_plan(&retry_cfg);
    let upstream_opt = &plan.upstream;
    let provider_opt = &plan.provider;
    let cooldown_backoff = crate::lb::CooldownBackoff {
        factor: plan.cooldown_backoff_factor,
        max_secs: plan.cooldown_backoff_max_secs,
    };
    log_retry_trace(serde_json::json!({
        "event": "retry_options",
        "service": proxy.service_name,
        "request_id": request_id,
        "upstream": {
            "max_attempts": upstream_opt.max_attempts,
            "base_backoff_ms": upstream_opt.base_backoff_ms,
            "max_backoff_ms": upstream_opt.max_backoff_ms,
            "jitter_ms": upstream_opt.jitter_ms,
            "retry_status_ranges": upstream_opt.retry_status_ranges,
            "retry_error_classes": upstream_opt.retry_error_classes,
            "strategy": if upstream_opt.strategy == RetryStrategy::Failover { "failover" } else { "same_upstream" },
        },
        "provider": {
            "max_attempts": provider_opt.max_attempts,
            "base_backoff_ms": provider_opt.base_backoff_ms,
            "max_backoff_ms": provider_opt.max_backoff_ms,
            "jitter_ms": provider_opt.jitter_ms,
            "retry_status_ranges": provider_opt.retry_status_ranges,
            "retry_error_classes": provider_opt.retry_error_classes,
            "strategy": if provider_opt.strategy == RetryStrategy::Failover { "failover" } else { "same_upstream" },
        },
        "never_status_ranges": plan.never_status_ranges,
        "never_error_classes": plan.never_error_classes,
        "cloudflare_challenge_cooldown_secs": plan.cloudflare_challenge_cooldown_secs,
        "cloudflare_timeout_cooldown_secs": plan.cloudflare_timeout_cooldown_secs,
        "transport_cooldown_secs": plan.transport_cooldown_secs,
        "cooldown_backoff_factor": plan.cooldown_backoff_factor,
        "cooldown_backoff_max_secs": plan.cooldown_backoff_max_secs,
        "allow_cross_station_before_first_output": plan.allow_cross_station_before_first_output,
    }));
    let total_upstreams = lbs
        .iter()
        .map(|lb| lb.service.upstreams.len())
        .sum::<usize>();
    let mut avoid: HashMap<String, HashSet<usize>> = HashMap::new();
    let mut upstream_chain: Vec<String> = Vec::new();
    let mut avoided_total: usize = 0;

    // --- Two-layer retry model ---
    //
    // Layer 1 (upstream): retry within the current provider/config (default: same_upstream).
    // Layer 2 (provider): after upstream retries are exhausted (or failure is not upstream-retryable),
    // fail over to another provider/config when eligible.
    //
    // Guardrails: never_on_status / never_on_class prevent amplifying obvious client-side mistakes.
    let mut tried_stations: HashSet<String> = HashSet::new();
    let strict_multi_config = lbs.len() > 1;
    let cross_station_failover_enabled = strict_multi_config
        && plan.allow_cross_station_before_first_output
        && provider_opt.strategy == RetryStrategy::Failover;
    let provider_attempt_limit = if cross_station_failover_enabled {
        provider_opt.max_attempts
    } else {
        1
    };
    let mut global_attempt: u32 = 0;
    let mut last_err: Option<(StatusCode, String)> = None;

    for provider_attempt in 0..provider_attempt_limit {
        // Pick the next station in the precomputed order, skipping ones we've already tried.
        let mut provider_lb: Option<LoadBalancer> = None;
        for lb in &lbs {
            if tried_stations.contains(&lb.service.name) {
                continue;
            }
            provider_lb = Some(lb.clone());
            break;
        }
        let Some(lb) = provider_lb else {
            break;
        };
        let station_name = lb.service.name.clone();

        // Try all upstreams under this station first. Cross-station failover is only
        // considered after the current station has no remaining eligible upstreams.
        'upstreams: loop {
            let avoid_set = avoid.entry(station_name.clone()).or_default();
            let upstream_total = lb.service.upstreams.len();
            if upstream_total > 0 && avoid_set.len() >= upstream_total {
                log_same_station_failover_trace(
                    proxy.service_name,
                    request_id,
                    station_name.as_str(),
                    upstream_total,
                    avoid_set,
                    true,
                );
                break 'upstreams;
            }

            // Select an eligible upstream inside this station (skip unsupported models).
            let selected = loop {
                let upstream_total = lb.service.upstreams.len();
                if upstream_total > 0 && avoid_set.len() >= upstream_total {
                    break None;
                }
                let next = {
                    let avoid_ref: &HashSet<usize> = &*avoid_set;
                    if strict_multi_config {
                        lb.select_upstream_avoiding_strict(avoid_ref)
                    } else {
                        lb.select_upstream_avoiding(avoid_ref)
                    }
                };
                let Some(selected) = next else {
                    break None;
                };

                if let Some(ref requested_model) = request_model {
                    let supported = model_routing::is_model_supported(
                        &selected.upstream.supported_models,
                        &selected.upstream.model_mapping,
                        requested_model,
                    );
                    if !supported {
                        upstream_chain.push(format!(
                            "{}:{} (idx={}) skipped_unsupported_model={}",
                            selected.station_name,
                            selected.upstream.base_url,
                            selected.index,
                            requested_model
                        ));
                        if avoid_set.insert(selected.index) {
                            avoided_total = avoided_total.saturating_add(1);
                        }
                        continue;
                    }
                }

                break Some(selected);
            };

            let Some(selected) = selected else {
                break 'upstreams;
            };

            let mut model_note = "-".to_string();
            let mut body_for_selected = body_for_upstream.clone();
            if let Some(ref requested_model) = request_model {
                let effective_model = model_routing::effective_model(
                    &selected.upstream.model_mapping,
                    requested_model,
                );
                if effective_model != *requested_model {
                    if let Some(modified) =
                        apply_model_override(body_for_upstream.as_ref(), effective_model.as_str())
                    {
                        body_for_selected = Bytes::from(modified);
                    }
                    model_note = format!("{requested_model}->{effective_model}");
                } else {
                    model_note = requested_model.clone();
                }
            }
            let provider_id = selected.upstream.tags.get("provider_id").cloned();
            let route_decision = build_route_decision_provenance(
                now_ms(),
                session_binding.as_ref(),
                session_override_config.as_deref(),
                global_station_override.as_deref(),
                override_model.as_deref(),
                override_effort.as_deref(),
                override_service_tier.as_deref(),
                request_model.as_deref(),
                effective_effort.as_deref(),
                effective_service_tier.as_deref(),
                &selected,
                provider_id.as_deref(),
            );

            let filtered_body = proxy.filter.apply_bytes(body_for_selected);
            let upstream_request_body_len = filtered_body.len();
            let upstream_request_body_debug = if request_body_previews && debug_max > 0 {
                Some(make_body_preview(
                    &filtered_body,
                    client_content_type,
                    debug_max,
                ))
            } else {
                None
            };
            let upstream_request_body_warn = if request_body_previews && warn_max > 0 {
                Some(make_body_preview(
                    &filtered_body,
                    client_content_type,
                    warn_max,
                ))
            } else {
                None
            };

            // Layer 1: retry the same upstream a small number of times.
            for upstream_attempt in 0..upstream_opt.max_attempts {
                global_attempt = global_attempt.saturating_add(1);
                let mut avoid_for_station = avoid_set.iter().copied().collect::<Vec<_>>();
                avoid_for_station.sort_unstable();

                log_retry_trace(serde_json::json!({
                    "event": "attempt_select",
                    "service": proxy.service_name,
                    "request_id": request_id,
                    "attempt": global_attempt,
                    "provider_attempt": provider_attempt + 1,
                    "upstream_attempt": upstream_attempt + 1,
                    "provider_max_attempts": provider_opt.max_attempts,
                    "upstream_max_attempts": upstream_opt.max_attempts,
                    "station_name": selected.station_name.as_str(),
                    "upstream_index": selected.index,
                    "upstream_base_url": selected.upstream.base_url.as_str(),
                    "provider_id": provider_id.as_deref(),
                    "avoid_for_station": avoid_for_station,
                    "avoided_total": avoided_total,
                    "total_upstreams": total_upstreams,
                    "model": model_note.as_str(),
                }));

                let target_url = match proxy.build_target(&selected, &uri) {
                    Ok((url, _headers)) => url,
                    Err(e) => {
                        let err_str = e.to_string();
                        upstream_chain.push(format!(
                            "{}:{} (idx={}) target_build_error={} model={}",
                            selected.station_name,
                            selected.upstream.base_url,
                            selected.index,
                            err_str,
                            model_note.as_str()
                        ));
                        record_passive_upstream_failure(
                            proxy.state.as_ref(),
                            proxy.service_name,
                            &selected.station_name,
                            &selected.upstream.base_url,
                            Some(StatusCode::BAD_GATEWAY.as_u16()),
                            Some("target_build_error"),
                            Some(err_str.clone()),
                        )
                        .await;
                        if avoid_set.insert(selected.index) {
                            avoided_total = avoided_total.saturating_add(1);
                        }
                        last_err = Some((StatusCode::BAD_GATEWAY, err_str));
                        break;
                    }
                };

                // copy headers, stripping host/content-length and hop-by-hop.
                // auth headers:
                // - if upstream config provides a token/key, override client values;
                // - otherwise, preserve client Authorization / X-API-Key (required for requires_openai_auth=true providers).
                let mut headers = filter_request_headers(&client_headers);
                let client_has_auth = headers.contains_key("authorization");
                let (token, _token_src) = resolve_auth_token_with_source(
                    proxy.service_name,
                    &selected.upstream.auth,
                    client_has_auth,
                );
                if let Some(token) = token
                    && let Ok(v) = HeaderValue::from_str(&format!("Bearer {token}"))
                {
                    headers.insert(HeaderName::from_static("authorization"), v);
                }

                let client_has_x_api_key = headers.contains_key("x-api-key");
                let (api_key, _api_key_src) = resolve_api_key_with_source(
                    proxy.service_name,
                    &selected.upstream.auth,
                    client_has_x_api_key,
                );
                if let Some(key) = api_key
                    && let Ok(v) = HeaderValue::from_str(&key)
                {
                    headers.insert(HeaderName::from_static("x-api-key"), v);
                }

                let upstream_request_headers = headers.clone();
                proxy
                    .state
                    .update_request_route(
                        request_id,
                        selected.station_name.clone(),
                        provider_id.clone(),
                        selected.upstream.base_url.clone(),
                        Some(route_decision.clone()),
                    )
                    .await;

                let debug_base = if debug_max > 0 || warn_max > 0 {
                    Some(HttpDebugBase {
                        debug_max_body_bytes: debug_max,
                        warn_max_body_bytes: warn_max,
                        request_body_len,
                        upstream_request_body_len,
                        client_uri: uri.to_string(),
                        target_url: target_url.to_string(),
                        client_headers: client_headers_entries_cache
                            .get_or_init(|| header_map_to_entries(&client_headers))
                            .clone(),
                        upstream_request_headers: header_map_to_entries(&upstream_request_headers),
                        auth_resolution: None,
                        client_body_debug: client_body_debug.clone(),
                        upstream_request_body_debug: upstream_request_body_debug.clone(),
                        client_body_warn: client_body_warn.clone(),
                        upstream_request_body_warn: upstream_request_body_warn.clone(),
                    })
                } else {
                    None
                };

                let builder = proxy
                    .client
                    .request(method.clone(), target_url.clone())
                    .headers(headers)
                    .body(filtered_body.clone());

                let upstream_start = Instant::now();
                let resp = match builder.send().await {
                    Ok(r) => r,
                    Err(e) => {
                        let err_str = format_reqwest_error_for_retry_chain(&e);
                        upstream_chain.push(format!(
                            "{}:{} (idx={}) transport_error={} model={}",
                            selected.station_name,
                            selected.upstream.base_url,
                            selected.index,
                            err_str,
                            model_note.as_str()
                        ));
                        // Upstream-layer retry only for classified transient transport errors.
                        let can_retry_upstream = upstream_attempt + 1 < upstream_opt.max_attempts
                            && should_retry_class(upstream_opt, Some("upstream_transport_error"));
                        if can_retry_upstream {
                            backoff_sleep(upstream_opt, upstream_attempt).await;
                            continue;
                        }

                        lb.penalize_with_backoff(
                            selected.index,
                            plan.transport_cooldown_secs,
                            "upstream_transport_error",
                            cooldown_backoff,
                        );
                        record_passive_upstream_failure(
                            proxy.state.as_ref(),
                            proxy.service_name,
                            &selected.station_name,
                            &selected.upstream.base_url,
                            Some(StatusCode::BAD_GATEWAY.as_u16()),
                            Some("upstream_transport_error"),
                            Some(err_str.clone()),
                        )
                        .await;
                        if avoid_set.insert(selected.index) {
                            avoided_total = avoided_total.saturating_add(1);
                        }
                        last_err = Some((StatusCode::BAD_GATEWAY, err_str));
                        break;
                    }
                };

                let upstream_headers_ms = upstream_start.elapsed().as_millis() as u64;
                let status = resp.status();
                let success = status.is_success();
                let resp_headers = resp.headers().clone();
                let resp_headers_filtered = filter_response_headers(&resp_headers);

                if is_stream && success {
                    lb.record_result_with_backoff(
                        selected.index,
                        true,
                        crate::lb::COOLDOWN_SECS,
                        cooldown_backoff,
                    );
                    let retry = retry_info_for_chain(&upstream_chain);
                    return Ok(build_sse_success_response(
                        &proxy,
                        lb.clone(),
                        selected.clone(),
                        resp,
                        SseSuccessMeta {
                            status,
                            resp_headers,
                            resp_headers_filtered,
                            start,
                            started_at_ms,
                            upstream_start,
                            upstream_headers_ms,
                            request_body_len,
                            upstream_request_body_len,
                            debug_base,
                            retry,
                            session_id: session_id.clone(),
                            cwd: cwd.clone(),
                            effective_effort: effective_effort.clone(),
                            service_tier: base_service_tier.clone(),
                            request_id,
                            is_user_turn,
                            is_codex_service,
                            transport_cooldown_secs: plan.transport_cooldown_secs,
                            cooldown_backoff,
                            method: method.clone(),
                            path: uri.path().to_string(),
                        },
                    )
                    .await);
                }

                let bytes = match resp.bytes().await {
                    Ok(b) => b,
                    Err(e) => {
                        let err_str = format_reqwest_error_for_retry_chain(&e);
                        upstream_chain.push(format!(
                            "{}:{} (idx={}) body_read_error={} model={}",
                            selected.station_name,
                            selected.upstream.base_url,
                            selected.index,
                            err_str,
                            model_note.as_str()
                        ));
                        let can_retry_upstream = upstream_attempt + 1 < upstream_opt.max_attempts
                            && should_retry_class(upstream_opt, Some("upstream_transport_error"));
                        if can_retry_upstream {
                            backoff_sleep(upstream_opt, upstream_attempt).await;
                            continue;
                        }
                        lb.penalize_with_backoff(
                            selected.index,
                            plan.transport_cooldown_secs,
                            "upstream_body_read_error",
                            cooldown_backoff,
                        );
                        record_passive_upstream_failure(
                            proxy.state.as_ref(),
                            proxy.service_name,
                            &selected.station_name,
                            &selected.upstream.base_url,
                            Some(StatusCode::BAD_GATEWAY.as_u16()),
                            Some("upstream_body_read_error"),
                            Some(err_str.clone()),
                        )
                        .await;
                        if avoid_set.insert(selected.index) {
                            avoided_total = avoided_total.saturating_add(1);
                        }
                        last_err = Some((StatusCode::BAD_GATEWAY, err_str));
                        break;
                    }
                };

                let _upstream_body_read_ms = upstream_start.elapsed().as_millis() as u64;
                let dur = start.elapsed().as_millis() as u64;

                let status_code = status.as_u16();
                let (cls, _hint, _cf_ray) =
                    classify_upstream_response(status_code, &resp_headers, bytes.as_ref());
                let never_retry = should_never_retry(&plan, status_code, cls.as_deref());
                let observed_service_tier = extract_service_tier_from_response_body(bytes.as_ref());

                upstream_chain.push(format!(
                    "{} (idx={}) status={} class={} model={}",
                    selected.upstream.base_url,
                    selected.index,
                    status_code,
                    cls.as_deref().unwrap_or("-"),
                    model_note.as_str()
                ));

                if success {
                    lb.record_result_with_backoff(
                        selected.index,
                        true,
                        crate::lb::COOLDOWN_SECS,
                        cooldown_backoff,
                    );
                    record_passive_upstream_success(
                        proxy.state.as_ref(),
                        proxy.service_name,
                        &selected.station_name,
                        &selected.upstream.base_url,
                        status_code,
                    )
                    .await;

                    let usage = extract_usage_from_bytes(&bytes);
                    let usage_for_log = usage.clone();
                    let retry = retry_info_for_chain(&upstream_chain);
                    let retry_for_log = retry.clone();
                    let service_tier_for_log = ServiceTierLog {
                        actual: observed_service_tier.clone(),
                        ..base_service_tier.clone()
                    };
                    proxy
                        .state
                        .finish_request(crate::state::FinishRequestParams {
                            id: request_id,
                            status_code,
                            duration_ms: dur,
                            ended_at_ms: started_at_ms + dur,
                            observed_service_tier: observed_service_tier.clone(),
                            usage,
                            retry,
                            ttfb_ms: Some(upstream_headers_ms),
                        })
                        .await;

                    log_request_with_debug(
                        proxy.service_name,
                        method.as_str(),
                        uri.path(),
                        status_code,
                        dur,
                        Some(upstream_headers_ms),
                        &selected.station_name,
                        provider_id.clone(),
                        &selected.upstream.base_url,
                        session_id.clone(),
                        cwd.clone(),
                        effective_effort.clone(),
                        service_tier_for_log,
                        usage_for_log,
                        retry_for_log,
                        None,
                    );

                    let mut builder = Response::builder().status(status);
                    for (name, value) in resp_headers_filtered.iter() {
                        builder = builder.header(name, value);
                    }
                    return Ok(builder.body(Body::from(bytes)).unwrap());
                }

                if never_retry {
                    if !class_is_health_neutral(cls.as_deref()) {
                        lb.record_result_with_backoff(
                            selected.index,
                            false,
                            crate::lb::COOLDOWN_SECS,
                            cooldown_backoff,
                        );
                    }
                    record_passive_upstream_failure(
                        proxy.state.as_ref(),
                        proxy.service_name,
                        &selected.station_name,
                        &selected.upstream.base_url,
                        Some(status_code),
                        cls.as_deref(),
                        Some(String::from_utf8_lossy(bytes.as_ref()).to_string()),
                    )
                    .await;

                    let retry = retry_info_for_chain(&upstream_chain);
                    let retry_for_log = retry.clone();
                    let service_tier_for_log = ServiceTierLog {
                        actual: observed_service_tier.clone(),
                        ..base_service_tier.clone()
                    };
                    proxy
                        .state
                        .finish_request(crate::state::FinishRequestParams {
                            id: request_id,
                            status_code,
                            duration_ms: dur,
                            ended_at_ms: started_at_ms + dur,
                            observed_service_tier: observed_service_tier.clone(),
                            usage: None,
                            retry,
                            ttfb_ms: Some(upstream_headers_ms),
                        })
                        .await;

                    log_request_with_debug(
                        proxy.service_name,
                        method.as_str(),
                        uri.path(),
                        status_code,
                        dur,
                        Some(upstream_headers_ms),
                        &selected.station_name,
                        provider_id.clone(),
                        &selected.upstream.base_url,
                        session_id.clone(),
                        cwd.clone(),
                        effective_effort.clone(),
                        service_tier_for_log,
                        None,
                        retry_for_log,
                        None,
                    );

                    let mut builder = Response::builder().status(status);
                    for (name, value) in resp_headers_filtered.iter() {
                        builder = builder.header(name, value);
                    }
                    return Ok(builder.body(Body::from(bytes)).unwrap());
                }

                let upstream_retryable = should_retry_status(upstream_opt, status_code)
                    || should_retry_class(upstream_opt, cls.as_deref());
                let can_retry_upstream =
                    upstream_retryable && upstream_attempt + 1 < upstream_opt.max_attempts;
                if can_retry_upstream {
                    retry_sleep(upstream_opt, upstream_attempt, &resp_headers).await;
                    continue;
                }

                // Upstream-layer exhausted; decide whether to fail over (first within config, then to another config).
                let provider_retryable = should_retry_status(provider_opt, status_code)
                    || should_retry_class(provider_opt, cls.as_deref());
                if provider_retryable {
                    if !class_is_health_neutral(cls.as_deref()) {
                        lb.penalize_with_backoff(
                            selected.index,
                            plan.transport_cooldown_secs,
                            &format!("status_{}", status_code),
                            cooldown_backoff,
                        );
                    }
                    record_passive_upstream_failure(
                        proxy.state.as_ref(),
                        proxy.service_name,
                        &selected.station_name,
                        &selected.upstream.base_url,
                        Some(status_code),
                        cls.as_deref(),
                        Some(String::from_utf8_lossy(bytes.as_ref()).to_string()),
                    )
                    .await;
                    last_err = Some((status, String::from_utf8_lossy(bytes.as_ref()).to_string()));

                    if avoid_set.insert(selected.index) {
                        avoided_total = avoided_total.saturating_add(1);
                    }
                    break;
                }

                // Not retryable for provider failover either: return the error as-is.
                let retry = retry_info_for_chain(&upstream_chain);
                let retry_for_log = retry.clone();
                record_passive_upstream_failure(
                    proxy.state.as_ref(),
                    proxy.service_name,
                    &selected.station_name,
                    &selected.upstream.base_url,
                    Some(status_code),
                    cls.as_deref(),
                    Some(String::from_utf8_lossy(bytes.as_ref()).to_string()),
                )
                .await;
                proxy
                    .state
                    .finish_request(crate::state::FinishRequestParams {
                        id: request_id,
                        status_code,
                        duration_ms: dur,
                        ended_at_ms: started_at_ms + dur,
                        observed_service_tier: observed_service_tier.clone(),
                        usage: None,
                        retry,
                        ttfb_ms: Some(upstream_headers_ms),
                    })
                    .await;

                let service_tier_for_log = ServiceTierLog {
                    actual: observed_service_tier,
                    ..base_service_tier.clone()
                };
                log_request_with_debug(
                    proxy.service_name,
                    method.as_str(),
                    uri.path(),
                    status_code,
                    dur,
                    Some(upstream_headers_ms),
                    &selected.station_name,
                    provider_id.clone(),
                    &selected.upstream.base_url,
                    session_id.clone(),
                    cwd.clone(),
                    effective_effort.clone(),
                    service_tier_for_log,
                    None,
                    retry_for_log,
                    None,
                );

                let mut builder = Response::builder().status(status);
                for (name, value) in resp_headers_filtered.iter() {
                    builder = builder.header(name, value);
                }
                return Ok(builder.body(Body::from(bytes)).unwrap());
            }

            // If we don't have any more upstreams under this station, move to next station;
            // otherwise, continue selecting another upstream under the same station.
            let upstream_total = lb.service.upstreams.len();
            if upstream_total > 0 && avoid_set.len() >= upstream_total {
                log_same_station_failover_trace(
                    proxy.service_name,
                    request_id,
                    station_name.as_str(),
                    upstream_total,
                    avoid_set,
                    true,
                );
                break 'upstreams;
            }
            if !avoid_set.is_empty() {
                log_same_station_failover_trace(
                    proxy.service_name,
                    request_id,
                    station_name.as_str(),
                    upstream_total,
                    avoid_set,
                    false,
                );
            }
            continue 'upstreams;
        }

        tried_stations.insert(station_name.clone());

        if strict_multi_config
            && provider_attempt == 0
            && !cross_station_failover_enabled
            && provider_opt.max_attempts > 1
        {
            log_retry_trace(serde_json::json!({
                "event": "cross_station_failover_blocked",
                "service": proxy.service_name,
                "request_id": request_id,
                "station_name": station_name,
                "provider_strategy": if provider_opt.strategy == RetryStrategy::Failover { "failover" } else { "same_upstream" },
                "configured_provider_max_attempts": provider_opt.max_attempts,
                "effective_provider_max_attempts": provider_attempt_limit,
                "allow_cross_station_before_first_output": plan.allow_cross_station_before_first_output,
            }));
        }

        // Provider layer: optional backoff between providers (usually 0).
        if provider_opt.base_backoff_ms > 0 && provider_attempt + 1 < provider_attempt_limit {
            backoff_sleep(provider_opt, provider_attempt).await;
        }
    }

    // If we reach here, all provider attempts are exhausted.
    if let Some((status, msg)) = last_err {
        let dur = start.elapsed().as_millis() as u64;
        log_request_with_debug(
            proxy.service_name,
            method.as_str(),
            uri.path(),
            status.as_u16(),
            dur,
            None,
            "-",
            None,
            "-",
            session_id.clone(),
            cwd.clone(),
            effective_effort.clone(),
            base_service_tier.clone(),
            None,
            retry_info_for_chain(&upstream_chain),
            None,
        );
        let retry = retry_info_for_chain(&upstream_chain);
        proxy
            .state
            .finish_request(crate::state::FinishRequestParams {
                id: request_id,
                status_code: status.as_u16(),
                duration_ms: dur,
                ended_at_ms: started_at_ms + dur,
                observed_service_tier: None,
                usage: None,
                retry,
                ttfb_ms: None,
            })
            .await;
        return Err((status, msg));
    }

    return Err((
        StatusCode::BAD_GATEWAY,
        "no upstreams available".to_string(),
    ));

    #[cfg(any())]
    {
        for attempt_index in 0..retry_opt.max_attempts {
            let avoided_total = avoid.values().map(|s| s.len()).sum::<usize>();
            if total_upstreams > 0 && avoided_total >= total_upstreams {
                upstream_chain.push(format!("all_upstreams_avoided total={total_upstreams}"));
                break;
            }

            let strict_multi_config = lbs.len() > 1;

            let mut chosen: Option<(LoadBalancer, SelectedUpstream)> = None;
            for lb in &lbs {
                let cfg_name = lb.service.name.clone();
                let avoid_set = avoid.entry(cfg_name.clone()).or_default();
                loop {
                    let upstream_total = lb.service.upstreams.len();
                    if upstream_total > 0 && avoid_set.len() >= upstream_total {
                        break;
                    }
                    let next = {
                        let avoid_ref: &HashSet<usize> = &*avoid_set;
                        if strict_multi_config {
                            lb.select_upstream_avoiding_strict(avoid_ref)
                        } else {
                            lb.select_upstream_avoiding(avoid_ref)
                        }
                    };
                    let Some(selected) = next else {
                        break;
                    };

                    if let Some(ref requested_model) = request_model {
                        let supported = model_routing::is_model_supported(
                            &selected.upstream.supported_models,
                            &selected.upstream.model_mapping,
                            requested_model,
                        );
                        if !supported {
                            upstream_chain.push(format!(
                                "{}:{} (idx={}) skipped_unsupported_model={}",
                                selected.station_name,
                                selected.upstream.base_url,
                                selected.index,
                                requested_model
                            ));
                            avoid_set.insert(selected.index);
                            continue;
                        }
                    }

                    chosen = Some((lb.clone(), selected));
                    break;
                }
                if chosen.is_some() {
                    break;
                }
            }

            // When we have multiple station candidates, prefer skipping stations that are fully cooled down.
            // However, if *all* stations are cooled/unavailable, fall back to the original "always pick one"
            // behavior to avoid a hard outage.
            if chosen.is_none() && strict_multi_config {
                for lb in &lbs {
                    let station_name = lb.service.name.clone();
                    let avoid_set = avoid.entry(station_name.clone()).or_default();
                    let upstream_total = lb.service.upstreams.len();
                    if upstream_total > 0 && avoid_set.len() >= upstream_total {
                        continue;
                    }
                    let avoid_ref: &HashSet<usize> = &*avoid_set;
                    if let Some(selected) = lb.select_upstream_avoiding(avoid_ref) {
                        chosen = Some((lb.clone(), selected));
                        break;
                    }
                }
            }

            let Some((lb, selected)) = chosen else {
                log_retry_trace(serde_json::json!({
                    "event": "attempt_no_upstream",
                    "service": proxy.service_name,
                    "request_id": request_id,
                    "attempt": attempt_index + 1,
                    "max_attempts": retry_opt.max_attempts,
                    "avoid": avoid.iter().map(|(k, v)| serde_json::json!({
                        "station": k,
                        "indices": v.iter().copied().collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                    "total_upstreams": total_upstreams,
                    "model": request_model.clone(),
                }));
                let dur = start.elapsed().as_millis() as u64;
                let status = if request_model.is_some() {
                    StatusCode::NOT_FOUND
                } else {
                    StatusCode::BAD_GATEWAY
                };
                log_request_with_debug(
                    proxy.service_name,
                    method.as_str(),
                    uri.path(),
                    status.as_u16(),
                    dur,
                    None,
                    "-",
                    None,
                    "-",
                    session_id.clone(),
                    cwd.clone(),
                    effective_effort.clone(),
                    None,
                    retry_info_for_chain(&upstream_chain),
                    None,
                );
                let retry = retry_info_for_chain(&upstream_chain);
                proxy
                    .state
                    .finish_request(crate::state::FinishRequestParams {
                        id: request_id,
                        status_code: status.as_u16(),
                        duration_ms: dur,
                        ended_at_ms: started_at_ms + dur,
                        usage: None,
                        retry,
                        ttfb_ms: None,
                    })
                    .await;
                if let Some(model) = request_model.as_deref() {
                    return Err((
                        status,
                        format!("no upstreams support requested model '{model}'"),
                    ));
                }
                return Err((status, "no upstreams available".to_string()));
            };

            let selected_station_name = selected.station_name.clone();
            let selected_upstream_index = selected.index;
            let selected_upstream_base_url = selected.upstream.base_url.clone();
            let provider_id = selected.upstream.tags.get("provider_id").cloned();
            log_retry_trace(serde_json::json!({
                "event": "attempt_select",
                "service": proxy.service_name,
                "request_id": request_id,
                "attempt": attempt_index + 1,
                "max_attempts": retry_opt.max_attempts,
                "strategy": if retry_failover { "failover" } else { "same_upstream" },
                "station_name": selected_station_name,
                "upstream_index": selected_upstream_index,
                "upstream_base_url": selected_upstream_base_url,
                "provider_id": provider_id.clone(),
                "model": request_model.clone(),
                "lb_state": lb_state_snapshot_json(&lb),
                "avoid_for_station": avoid.get(&selected.station_name).map(|s| s.iter().copied().collect::<Vec<_>>()),
                "avoided_total": avoid.values().map(|s| s.len()).sum::<usize>(),
                "total_upstreams": total_upstreams,
            }));

            let mut model_note = "-".to_string();
            let mut body_for_selected = body_for_upstream.clone();
            if let Some(ref requested_model) = request_model {
                let effective_model = model_routing::effective_model(
                    &selected.upstream.model_mapping,
                    requested_model,
                );
                if effective_model != *requested_model {
                    if let Some(modified) =
                        apply_model_override(body_for_upstream.as_ref(), effective_model.as_str())
                    {
                        body_for_selected = Bytes::from(modified);
                    }
                    model_note = format!("{requested_model}->{effective_model}");
                } else {
                    model_note = requested_model.clone();
                }
            }
            let route_decision = build_route_decision_provenance(
                now_ms(),
                session_binding.as_ref(),
                session_override_config.as_deref(),
                global_config_override.as_deref(),
                override_model.as_deref(),
                override_effort.as_deref(),
                override_service_tier.as_deref(),
                request_model.as_deref(),
                effective_effort.as_deref(),
                effective_service_tier.as_deref(),
                &selected,
                provider_id.as_deref(),
            );

            let filtered_body = proxy.filter.apply_bytes(body_for_selected);
            let upstream_request_body_len = filtered_body.len();
            let upstream_request_body_debug = if request_body_previews && debug_max > 0 {
                Some(make_body_preview(
                    &filtered_body,
                    client_content_type,
                    debug_max,
                ))
            } else {
                None
            };
            let upstream_request_body_warn = if request_body_previews && warn_max > 0 {
                Some(make_body_preview(
                    &filtered_body,
                    client_content_type,
                    warn_max,
                ))
            } else {
                None
            };

            let target_url = match proxy.build_target(&selected, &uri) {
                Ok((url, _headers)) => url,
                Err(e) => {
                    lb.record_result_with_backoff(
                        selected.index,
                        false,
                        crate::lb::COOLDOWN_SECS,
                        cooldown_backoff,
                    );
                    let err_str = e.to_string();
                    upstream_chain.push(format!(
                        "{}:{} (idx={}) target_build_error={} model={}",
                        selected.station_name,
                        selected.upstream.base_url,
                        selected.index,
                        err_str,
                        model_note.as_str()
                    ));
                    record_passive_upstream_failure(
                        proxy.state.as_ref(),
                        proxy.service_name,
                        &selected.station_name,
                        &selected.upstream.base_url,
                        Some(StatusCode::BAD_GATEWAY.as_u16()),
                        Some("target_build_error"),
                        Some(err_str.clone()),
                    )
                    .await;
                    avoid
                        .entry(selected.station_name.clone())
                        .or_default()
                        .insert(selected.index);

                    let can_retry = attempt_index + 1 < retry_opt.max_attempts;
                    if can_retry {
                        backoff_sleep(&retry_opt, attempt_index).await;
                        continue;
                    }

                    let dur = start.elapsed().as_millis() as u64;
                    let status = StatusCode::BAD_GATEWAY;
                    let client_headers_entries = client_headers_entries_cache
                        .get_or_init(|| header_map_to_entries(&client_headers))
                        .clone();
                    let http_debug = if should_include_http_warn(status.as_u16()) {
                        Some(HttpDebugLog {
                            request_body_len: Some(request_body_len),
                            upstream_request_body_len: Some(upstream_request_body_len),
                            upstream_headers_ms: None,
                            upstream_first_chunk_ms: None,
                            upstream_body_read_ms: None,
                            upstream_error_class: Some("target_build_error".to_string()),
                            upstream_error_hint: Some(
                                "构造上游 target_url 失败（通常是 base_url 配置错误）。"
                                    .to_string(),
                            ),
                            upstream_cf_ray: None,
                            client_uri: uri.to_string(),
                            target_url: "-".to_string(),
                            client_headers: client_headers_entries,
                            upstream_request_headers: Vec::new(),
                            auth_resolution: None,
                            client_body: client_body_warn.clone(),
                            upstream_request_body: upstream_request_body_warn.clone(),
                            upstream_response_headers: None,
                            upstream_response_body: None,
                            upstream_error: Some(err_str.clone()),
                        })
                    } else {
                        None
                    };
                    log_request_with_debug(
                        proxy.service_name,
                        method.as_str(),
                        uri.path(),
                        status.as_u16(),
                        dur,
                        None,
                        &selected.station_name,
                        selected.upstream.tags.get("provider_id").cloned(),
                        &selected.upstream.base_url,
                        session_id.clone(),
                        cwd.clone(),
                        effective_effort.clone(),
                        None,
                        retry_info_for_chain(&upstream_chain),
                        http_debug,
                    );
                    let retry = retry_info_for_chain(&upstream_chain);
                    proxy
                        .state
                        .finish_request(crate::state::FinishRequestParams {
                            id: request_id,
                            status_code: status.as_u16(),
                            duration_ms: dur,
                            ended_at_ms: started_at_ms + dur,
                            usage: None,
                            retry,
                            ttfb_ms: None,
                        })
                        .await;
                    return Err((status, err_str));
                }
            };

            // copy headers, stripping host/content-length and hop-by-hop.
            // auth headers:
            // - if upstream config provides a token/key, override client values;
            // - otherwise, preserve client Authorization / X-API-Key (required for requires_openai_auth=true providers).
            let mut headers = filter_request_headers(&client_headers);
            let client_has_auth = headers.contains_key("authorization");
            let (token, token_src) = resolve_auth_token_with_source(
                proxy.service_name,
                &selected.upstream.auth,
                client_has_auth,
            );
            if let Some(token) = token
                && let Ok(v) = HeaderValue::from_str(&format!("Bearer {token}"))
            {
                headers.insert(HeaderName::from_static("authorization"), v);
            }

            let client_has_x_api_key = headers.contains_key("x-api-key");
            let (api_key, api_key_src) = resolve_api_key_with_source(
                proxy.service_name,
                &selected.upstream.auth,
                client_has_x_api_key,
            );
            if let Some(key) = api_key
                && let Ok(v) = HeaderValue::from_str(&key)
            {
                headers.insert(HeaderName::from_static("x-api-key"), v);
            }

            let upstream_request_headers = headers.clone();
            proxy
                .state
                .update_request_route(
                    request_id,
                    selected.station_name.clone(),
                    provider_id.clone(),
                    selected.upstream.base_url.clone(),
                    Some(route_decision.clone()),
                )
                .await;
            let auth_resolution = AuthResolutionLog {
                authorization: Some(token_src),
                x_api_key: Some(api_key_src),
            };

            let debug_base = if debug_max > 0 || warn_max > 0 {
                Some(HttpDebugBase {
                    debug_max_body_bytes: debug_max,
                    warn_max_body_bytes: warn_max,
                    request_body_len,
                    upstream_request_body_len,
                    client_uri: uri.to_string(),
                    target_url: target_url.to_string(),
                    client_headers: client_headers_entries_cache
                        .get_or_init(|| header_map_to_entries(&client_headers))
                        .clone(),
                    upstream_request_headers: header_map_to_entries(&upstream_request_headers),
                    auth_resolution: Some(auth_resolution),
                    client_body_debug: client_body_debug.clone(),
                    upstream_request_body_debug: upstream_request_body_debug.clone(),
                    client_body_warn: client_body_warn.clone(),
                    upstream_request_body_warn: upstream_request_body_warn.clone(),
                })
            } else {
                None
            };

            // 详细转发日志仅在 debug 级别输出，避免刷屏。
            tracing::debug!(
                "forwarding {} {} to {} ({})",
                method,
                uri.path(),
                target_url,
                selected.station_name
            );

            let builder = proxy
                .client
                .request(method.clone(), target_url.clone())
                .headers(headers)
                .body(filtered_body.clone());

            let upstream_start = Instant::now();
            let resp = match builder.send().await {
                Ok(r) => r,
                Err(e) => {
                    log_retry_trace(serde_json::json!({
                        "event": "attempt_transport_error",
                        "service": proxy.service_name,
                        "request_id": request_id,
                        "attempt": attempt_index + 1,
                        "max_attempts": retry_opt.max_attempts,
                        "strategy": if retry_failover { "failover" } else { "same_upstream" },
                        "station_name": selected.station_name.as_str(),
                        "upstream_index": selected.index,
                        "upstream_base_url": selected.upstream.base_url.as_str(),
                        "provider_id": provider_id.as_deref(),
                        "error": format_reqwest_error_for_retry_chain(&e),
                    }));
                    if retry_failover {
                        lb.record_result_with_backoff(
                            selected.index,
                            false,
                            crate::lb::COOLDOWN_SECS,
                            cooldown_backoff,
                        );
                    }
                    let err_str = format_reqwest_error_for_retry_chain(&e);
                    upstream_chain.push(format!(
                        "{}:{} (idx={}) transport_error={} model={}",
                        selected.station_name,
                        selected.upstream.base_url,
                        selected.index,
                        err_str,
                        model_note.as_str()
                    ));
                    let can_retry = attempt_index + 1 < retry_opt.max_attempts
                        && should_retry_class(&retry_opt, Some("upstream_transport_error"));
                    if can_retry {
                        if retry_failover {
                            lb.penalize_with_backoff(
                                selected.index,
                                retry_opt.transport_cooldown_secs,
                                "upstream_transport_error",
                                cooldown_backoff,
                            );
                            record_passive_upstream_failure(
                                proxy.state.as_ref(),
                                proxy.service_name,
                                &selected.station_name,
                                &selected.upstream.base_url,
                                Some(StatusCode::BAD_GATEWAY.as_u16()),
                                Some("upstream_transport_error"),
                                Some(err_str.clone()),
                            )
                            .await;
                            avoid
                                .entry(selected.station_name.clone())
                                .or_default()
                                .insert(selected.index);
                        }
                        backoff_sleep(&retry_opt, attempt_index).await;
                        continue;
                    }

                    // Even when we have no remaining in-request retries, mark this upstream as cooled down
                    // so external retries (e.g. Codex request_max_retries) can fail over to another upstream.
                    if should_retry_class(&retry_opt, Some("upstream_transport_error")) {
                        lb.penalize_with_backoff(
                            selected.index,
                            retry_opt.transport_cooldown_secs,
                            "upstream_transport_error_final",
                            cooldown_backoff,
                        );
                    }
                    record_passive_upstream_failure(
                        proxy.state.as_ref(),
                        proxy.service_name,
                        &selected.station_name,
                        &selected.upstream.base_url,
                        Some(StatusCode::BAD_GATEWAY.as_u16()),
                        Some("upstream_transport_error_final"),
                        Some(err_str.clone()),
                    )
                    .await;

                    let dur = start.elapsed().as_millis() as u64;
                    let status_code = StatusCode::BAD_GATEWAY.as_u16();
                    let upstream_headers_ms = upstream_start.elapsed().as_millis() as u64;
                    let retry = retry_info_for_chain(&upstream_chain);
                    let http_debug_warn = debug_base.as_ref().and_then(|b| {
                    if b.warn_max_body_bytes == 0 {
                        return None;
                    }
                    Some(HttpDebugLog {
                        request_body_len: Some(b.request_body_len),
                        upstream_request_body_len: Some(b.upstream_request_body_len),
                        upstream_headers_ms: Some(upstream_headers_ms),
                        upstream_first_chunk_ms: None,
                        upstream_body_read_ms: None,
                        upstream_error_class: Some("upstream_transport_error".to_string()),
                        upstream_error_hint: Some(
                            "上游连接/发送请求失败（reqwest 错误）；请检查网络、DNS、TLS、代理设置或上游可用性。".to_string(),
                        ),
                        upstream_cf_ray: None,
                        client_uri: b.client_uri.clone(),
                        target_url: b.target_url.clone(),
                        client_headers: b.client_headers.clone(),
                        upstream_request_headers: b.upstream_request_headers.clone(),
                        auth_resolution: b.auth_resolution.clone(),
                        client_body: b.client_body_warn.clone(),
                        upstream_request_body: b.upstream_request_body_warn.clone(),
                        upstream_response_headers: None,
                        upstream_response_body: None,
                        upstream_error: Some(err_str.clone()),
                    })
                });
                    if should_include_http_warn(status_code)
                        && let Some(h) = http_debug_warn.as_ref()
                    {
                        warn_http_debug(status_code, h);
                    }
                    let http_debug = if should_include_http_debug(status_code) {
                        debug_base.as_ref().and_then(|b| {
                        if b.debug_max_body_bytes == 0 {
                            return None;
                        }
                        Some(HttpDebugLog {
                            request_body_len: Some(b.request_body_len),
                            upstream_request_body_len: Some(b.upstream_request_body_len),
                            upstream_headers_ms: Some(upstream_headers_ms),
                            upstream_first_chunk_ms: None,
                            upstream_body_read_ms: None,
                            upstream_error_class: Some("upstream_transport_error".to_string()),
                            upstream_error_hint: Some(
                                "上游连接/发送请求失败（reqwest 错误）；请检查网络、DNS、TLS、代理设置或上游可用性。".to_string(),
                            ),
                            upstream_cf_ray: None,
                            client_uri: b.client_uri.clone(),
                            target_url: b.target_url.clone(),
                            client_headers: b.client_headers.clone(),
                            upstream_request_headers: b.upstream_request_headers.clone(),
                            auth_resolution: b.auth_resolution.clone(),
                            client_body: b.client_body_debug.clone(),
                            upstream_request_body: b.upstream_request_body_debug.clone(),
                            upstream_response_headers: None,
                            upstream_response_body: None,
                            upstream_error: Some(err_str.clone()),
                        })
                    })
                    } else if should_include_http_warn(status_code) {
                        http_debug_warn.clone()
                    } else {
                        None
                    };
                    log_request_with_debug(
                        proxy.service_name,
                        method.as_str(),
                        uri.path(),
                        status_code,
                        dur,
                        None,
                        &selected.station_name,
                        selected.upstream.tags.get("provider_id").cloned(),
                        &selected.upstream.base_url,
                        session_id.clone(),
                        cwd.clone(),
                        effective_effort.clone(),
                        None,
                        retry.clone(),
                        http_debug,
                    );
                    proxy
                        .state
                        .finish_request(crate::state::FinishRequestParams {
                            id: request_id,
                            status_code,
                            duration_ms: dur,
                            ended_at_ms: started_at_ms + dur,
                            usage: None,
                            retry,
                            ttfb_ms: None,
                        })
                        .await;
                    return Err((StatusCode::BAD_GATEWAY, e.to_string()));
                }
            };

            let upstream_headers_ms = upstream_start.elapsed().as_millis() as u64;
            let status = resp.status();
            let success = status.is_success();
            let resp_headers = resp.headers().clone();
            let resp_headers_filtered = filter_response_headers(&resp_headers);

            // 对用户对话轮次输出更有信息量的 info 日志（仅最终返回时打印，避免重试期间刷屏）。

            if is_stream && success {
                lb.record_result_with_backoff(
                    selected.index,
                    true,
                    crate::lb::COOLDOWN_SECS,
                    cooldown_backoff,
                );
                upstream_chain.push(format!(
                    "{} (idx={}) status={} model={}",
                    selected.upstream.base_url,
                    selected.index,
                    status.as_u16(),
                    model_note.as_str()
                ));
                let retry = retry_info_for_chain(&upstream_chain);

                return Ok(build_sse_success_response(
                    &proxy,
                    lb.clone(),
                    selected,
                    resp,
                    SseSuccessMeta {
                        status,
                        resp_headers,
                        resp_headers_filtered,
                        start,
                        started_at_ms,
                        upstream_start,
                        upstream_headers_ms,
                        request_body_len,
                        upstream_request_body_len,
                        debug_base,
                        retry,
                        session_id: session_id.clone(),
                        cwd: cwd.clone(),
                        effective_effort: effective_effort.clone(),
                        service_tier: base_service_tier.clone(),
                        request_id,
                        is_user_turn,
                        is_codex_service,
                        transport_cooldown_secs: retry_opt.transport_cooldown_secs,
                        cooldown_backoff,
                        method: method.clone(),
                        path: uri.path().to_string(),
                    },
                )
                .await);
            } else {
                let bytes = match resp.bytes().await {
                    Ok(b) => b,
                    Err(e) => {
                        log_retry_trace(serde_json::json!({
                            "event": "attempt_body_read_error",
                            "service": proxy.service_name,
                            "request_id": request_id,
                            "attempt": attempt_index + 1,
                            "max_attempts": retry_opt.max_attempts,
                            "strategy": if retry_failover { "failover" } else { "same_upstream" },
                            "station_name": selected.station_name.as_str(),
                            "upstream_index": selected.index,
                            "upstream_base_url": selected.upstream.base_url.as_str(),
                            "provider_id": provider_id.as_deref(),
                            "error": format_reqwest_error_for_retry_chain(&e),
                        }));
                        if retry_failover {
                            lb.record_result_with_backoff(
                                selected.index,
                                false,
                                crate::lb::COOLDOWN_SECS,
                                cooldown_backoff,
                            );
                        }
                        let err_str = format_reqwest_error_for_retry_chain(&e);
                        upstream_chain.push(format!(
                            "{}:{} (idx={}) body_read_error={} model={}",
                            selected.station_name,
                            selected.upstream.base_url,
                            selected.index,
                            err_str,
                            model_note.as_str()
                        ));
                        let can_retry = attempt_index + 1 < retry_opt.max_attempts
                            && should_retry_class(&retry_opt, Some("upstream_transport_error"));
                        if can_retry {
                            if retry_failover {
                                lb.penalize_with_backoff(
                                    selected.index,
                                    retry_opt.transport_cooldown_secs,
                                    "upstream_body_read_error",
                                    cooldown_backoff,
                                );
                                record_passive_upstream_failure(
                                    proxy.state.as_ref(),
                                    proxy.service_name,
                                    &selected.station_name,
                                    &selected.upstream.base_url,
                                    Some(StatusCode::BAD_GATEWAY.as_u16()),
                                    Some("upstream_body_read_error"),
                                    Some(err_str.clone()),
                                )
                                .await;
                                avoid
                                    .entry(selected.station_name.clone())
                                    .or_default()
                                    .insert(selected.index);
                            }
                            backoff_sleep(&retry_opt, attempt_index).await;
                            continue;
                        }

                        // Same reasoning as transport errors: without in-request retries, external retries
                        // should not get stuck repeatedly selecting the same broken upstream.
                        if should_retry_class(&retry_opt, Some("upstream_transport_error")) {
                            lb.penalize_with_backoff(
                                selected.index,
                                retry_opt.transport_cooldown_secs,
                                "upstream_body_read_error_final",
                                cooldown_backoff,
                            );
                        }
                        record_passive_upstream_failure(
                            proxy.state.as_ref(),
                            proxy.service_name,
                            &selected.station_name,
                            &selected.upstream.base_url,
                            Some(StatusCode::BAD_GATEWAY.as_u16()),
                            Some("upstream_body_read_error_final"),
                            Some(err_str.clone()),
                        )
                        .await;

                        let dur = start.elapsed().as_millis() as u64;
                        let status = StatusCode::BAD_GATEWAY;
                        let http_debug = if should_include_http_warn(status.as_u16())
                            && let Some(b) = debug_base.as_ref()
                        {
                            Some(HttpDebugLog {
                            request_body_len: Some(b.request_body_len),
                            upstream_request_body_len: Some(b.upstream_request_body_len),
                            upstream_headers_ms: Some(upstream_headers_ms),
                            upstream_first_chunk_ms: None,
                            upstream_body_read_ms: None,
                            upstream_error_class: Some("upstream_transport_error".to_string()),
                            upstream_error_hint: Some(
                                "读取上游响应 body 失败（连接中断/解码错误等）；可视为传输错误。"
                                    .to_string(),
                            ),
                            upstream_cf_ray: None,
                            client_uri: b.client_uri.clone(),
                            target_url: b.target_url.clone(),
                            client_headers: b.client_headers.clone(),
                            upstream_request_headers: b.upstream_request_headers.clone(),
                            auth_resolution: b.auth_resolution.clone(),
                            client_body: b.client_body_warn.clone(),
                            upstream_request_body: b.upstream_request_body_warn.clone(),
                            upstream_response_headers: Some(header_map_to_entries(&resp_headers)),
                            upstream_response_body: None,
                            upstream_error: Some(err_str.clone()),
                        })
                        } else {
                            None
                        };
                        log_request_with_debug(
                            proxy.service_name,
                            method.as_str(),
                            uri.path(),
                            status.as_u16(),
                            dur,
                            Some(upstream_headers_ms),
                            &selected.station_name,
                            selected.upstream.tags.get("provider_id").cloned(),
                            &selected.upstream.base_url,
                            session_id.clone(),
                            cwd.clone(),
                            effective_effort.clone(),
                            None,
                            retry_info_for_chain(&upstream_chain),
                            http_debug,
                        );
                        let retry = retry_info_for_chain(&upstream_chain);
                        proxy
                            .state
                            .finish_request(crate::state::FinishRequestParams {
                                id: request_id,
                                status_code: status.as_u16(),
                                duration_ms: dur,
                                ended_at_ms: started_at_ms + dur,
                                usage: None,
                                retry,
                                ttfb_ms: Some(upstream_headers_ms),
                            })
                            .await;
                        return Err((status, err_str));
                    }
                };
                let upstream_body_read_ms = upstream_start.elapsed().as_millis() as u64;
                let dur = start.elapsed().as_millis() as u64;
                let usage = extract_usage_from_bytes(&bytes);
                let status_code = status.as_u16();
                let (cls, hint, cf_ray) =
                    classify_upstream_response(status_code, &resp_headers, bytes.as_ref());
                let never_retry = should_never_retry_status(&retry_opt, status_code)
                    || should_never_retry_class(&retry_opt, cls.as_deref());

                upstream_chain.push(format!(
                    "{} (idx={}) status={} class={} model={}",
                    selected.upstream.base_url,
                    selected.index,
                    status_code,
                    cls.as_deref().unwrap_or("-"),
                    model_note.as_str()
                ));

                // If this looks like a transient / retryable upstream failure, but we have no remaining
                // in-request retries, proactively cool down this upstream so the next external retry
                // (Codex/app-level) can fail over to other upstreams.
                if !success
                    && attempt_index + 1 >= retry_opt.max_attempts
                    && !never_retry
                    && !class_is_health_neutral(cls.as_deref())
                    && (should_retry_status(&retry_opt, status_code)
                        || should_retry_class(&retry_opt, cls.as_deref()))
                {
                    log_retry_trace(serde_json::json!({
                        "event": "attempt_final_retryable_failure",
                        "service": proxy.service_name,
                        "request_id": request_id,
                        "attempt": attempt_index + 1,
                        "max_attempts": retry_opt.max_attempts,
                        "status_code": status_code,
                        "class": cls.as_deref(),
                        "hint": hint.as_deref(),
                        "cf_ray": cf_ray.as_deref(),
                        "station_name": selected.station_name.as_str(),
                        "upstream_index": selected.index,
                        "upstream_base_url": selected.upstream.base_url.as_str(),
                        "provider_id": provider_id.as_deref(),
                        "should_retry_status": should_retry_status(&retry_opt, status_code),
                        "should_retry_class": should_retry_class(&retry_opt, cls.as_deref()),
                        "never_retry_status": should_never_retry_status(&retry_opt, status_code),
                        "never_retry_class": should_never_retry_class(&retry_opt, cls.as_deref()),
                    }));
                    match cls.as_deref() {
                        Some("cloudflare_challenge") => lb.penalize_with_backoff(
                            selected.index,
                            retry_opt.cloudflare_challenge_cooldown_secs,
                            "cloudflare_challenge_final",
                            cooldown_backoff,
                        ),
                        Some("cloudflare_timeout") => lb.penalize_with_backoff(
                            selected.index,
                            retry_opt.cloudflare_timeout_cooldown_secs,
                            "cloudflare_timeout_final",
                            cooldown_backoff,
                        ),
                        _ if status_code >= 400 => lb.penalize_with_backoff(
                            selected.index,
                            retry_opt.transport_cooldown_secs,
                            &format!("status_{}_final", status_code),
                            cooldown_backoff,
                        ),
                        _ => {}
                    }
                }

                let retryable = !status.is_success()
                    && attempt_index + 1 < retry_opt.max_attempts
                    && !never_retry
                    && (should_retry_status(&retry_opt, status_code)
                        || should_retry_class(&retry_opt, cls.as_deref()));
                if retryable {
                    log_retry_trace(serde_json::json!({
                        "event": "attempt_retryable_failure",
                        "service": proxy.service_name,
                        "request_id": request_id,
                        "attempt": attempt_index + 1,
                        "next_attempt": attempt_index + 2,
                        "max_attempts": retry_opt.max_attempts,
                        "status_code": status_code,
                        "class": cls.as_deref(),
                        "hint": hint.as_deref(),
                        "cf_ray": cf_ray.as_deref(),
                        "station_name": selected.station_name.as_str(),
                        "upstream_index": selected.index,
                        "upstream_base_url": selected.upstream.base_url.as_str(),
                        "provider_id": provider_id.as_deref(),
                        "strategy": if retry_failover { "failover" } else { "same_upstream" },
                        "retry_after": resp_headers.get("retry-after").and_then(|v| v.to_str().ok()),
                        "should_retry_status": should_retry_status(&retry_opt, status_code),
                        "should_retry_class": should_retry_class(&retry_opt, cls.as_deref()),
                        "never_retry_status": should_never_retry_status(&retry_opt, status_code),
                        "never_retry_class": should_never_retry_class(&retry_opt, cls.as_deref()),
                    }));
                    let cls_s = cls.as_deref().unwrap_or("-");
                    info!(
                        "retrying after non-2xx status {} (class={}) for {} {} (station: {}, mode={}, next_attempt={}/{})",
                        status_code,
                        cls_s,
                        method,
                        uri.path(),
                        selected.station_name,
                        if retry_failover {
                            "failover"
                        } else {
                            "same_upstream"
                        },
                        attempt_index + 2,
                        retry_opt.max_attempts
                    );
                    if retry_failover {
                        // Treat retryable 5xx / WAF-like responses as upstream failures for LB tracking.
                        if (status_code >= 500 || cls.is_some())
                            && !class_is_health_neutral(cls.as_deref())
                        {
                            lb.record_result_with_backoff(
                                selected.index,
                                false,
                                crate::lb::COOLDOWN_SECS,
                                cooldown_backoff,
                            );
                        }
                        if !class_is_health_neutral(cls.as_deref()) {
                            match cls.as_deref() {
                                Some("cloudflare_challenge") => lb.penalize_with_backoff(
                                    selected.index,
                                    retry_opt.cloudflare_challenge_cooldown_secs,
                                    "cloudflare_challenge",
                                    cooldown_backoff,
                                ),
                                Some("cloudflare_timeout") => lb.penalize_with_backoff(
                                    selected.index,
                                    retry_opt.cloudflare_timeout_cooldown_secs,
                                    "cloudflare_timeout",
                                    cooldown_backoff,
                                ),
                                _ if status_code >= 400 => lb.penalize_with_backoff(
                                    selected.index,
                                    retry_opt.transport_cooldown_secs,
                                    &format!("status_{}", status_code),
                                    cooldown_backoff,
                                ),
                                _ => {}
                            }
                        }
                        avoid
                            .entry(selected.station_name.clone())
                            .or_default()
                            .insert(selected.index);
                        record_passive_upstream_failure(
                            proxy.state.as_ref(),
                            proxy.service_name,
                            &selected.station_name,
                            &selected.upstream.base_url,
                            Some(status_code),
                            cls.as_deref(),
                            Some(String::from_utf8_lossy(bytes.as_ref()).to_string()),
                        )
                        .await;
                    }
                    let skip_sleep = status_code >= 400 && status_code < 500 && status_code != 429;
                    if !skip_sleep {
                        retry_sleep(&retry_opt, attempt_index, &resp_headers).await;
                    }
                    continue;
                }

                // Update LB state (final attempt):
                // - 2xx => success
                // - transport / 5xx / classified WAF failures => failure
                // - generic 3xx/4xx => neutral (do not mark upstream good/bad to avoid sticky routing to a failing upstream,
                //   and also avoid penalizing upstreams for client-side mistakes).
                if success {
                    lb.record_result_with_backoff(
                        selected.index,
                        true,
                        crate::lb::COOLDOWN_SECS,
                        cooldown_backoff,
                    );
                    record_passive_upstream_success(
                        proxy.state.as_ref(),
                        proxy.service_name,
                        &selected.station_name,
                        &selected.upstream.base_url,
                        status_code,
                    )
                    .await;
                } else if (status_code >= 500 || cls.is_some())
                    && !class_is_health_neutral(cls.as_deref())
                {
                    lb.record_result_with_backoff(
                        selected.index,
                        false,
                        crate::lb::COOLDOWN_SECS,
                        cooldown_backoff,
                    );
                    record_passive_upstream_failure(
                        proxy.state.as_ref(),
                        proxy.service_name,
                        &selected.station_name,
                        &selected.upstream.base_url,
                        Some(status_code),
                        cls.as_deref(),
                        Some(String::from_utf8_lossy(bytes.as_ref()).to_string()),
                    )
                    .await;
                }

                let retry = retry_info_for_chain(&upstream_chain);

                if is_user_turn {
                    let provider_id = selected
                        .upstream
                        .tags
                        .get("provider_id")
                        .map(|s| s.as_str())
                        .unwrap_or("-");
                    info!(
                        "user turn {} {} using station '{}' upstream[{}] provider_id='{}' base_url='{}'",
                        method,
                        uri.path(),
                        selected.station_name,
                        selected.index,
                        provider_id,
                        selected.upstream.base_url
                    );
                }

                let http_debug_warn = if should_include_http_warn(status_code)
                    && let Some(b) = debug_base.as_ref()
                {
                    let max = b.warn_max_body_bytes;
                    let resp_ct = resp_headers
                        .get("content-type")
                        .and_then(|v| v.to_str().ok());
                    Some(HttpDebugLog {
                        request_body_len: Some(b.request_body_len),
                        upstream_request_body_len: Some(b.upstream_request_body_len),
                        upstream_headers_ms: Some(upstream_headers_ms),
                        upstream_first_chunk_ms: None,
                        upstream_body_read_ms: Some(upstream_body_read_ms),
                        upstream_error_class: cls.clone(),
                        upstream_error_hint: hint.clone(),
                        upstream_cf_ray: cf_ray.clone(),
                        client_uri: b.client_uri.clone(),
                        target_url: b.target_url.clone(),
                        client_headers: b.client_headers.clone(),
                        upstream_request_headers: b.upstream_request_headers.clone(),
                        auth_resolution: b.auth_resolution.clone(),
                        client_body: b.client_body_warn.clone(),
                        upstream_request_body: b.upstream_request_body_warn.clone(),
                        upstream_response_headers: Some(header_map_to_entries(&resp_headers)),
                        upstream_response_body: Some(make_body_preview(
                            bytes.as_ref(),
                            resp_ct,
                            max,
                        )),
                        upstream_error: None,
                    })
                } else {
                    None
                };

                if !status.is_success() {
                    if let Some(h) = http_debug_warn.as_ref() {
                        warn_http_debug(status_code, h);
                    } else {
                        let cls_s = cls.as_deref().unwrap_or("-");
                        let cf_ray_s = cf_ray.as_deref().unwrap_or("-");
                        warn!(
                            "upstream returned non-2xx status {} (class={}, cf_ray={}) for {} {} (station: {}); set CODEX_HELPER_HTTP_WARN=0 to disable preview logs (or CODEX_HELPER_HTTP_DEBUG=1 for full debug)",
                            status_code,
                            cls_s,
                            cf_ray_s,
                            method,
                            uri.path(),
                            selected.station_name
                        );
                    }
                }

                let http_debug = if should_include_http_debug(status_code) {
                    debug_base.map(|b| {
                        let max = b.debug_max_body_bytes;
                        let resp_ct = resp_headers
                            .get("content-type")
                            .and_then(|v| v.to_str().ok());
                        HttpDebugLog {
                            request_body_len: Some(b.request_body_len),
                            upstream_request_body_len: Some(b.upstream_request_body_len),
                            upstream_headers_ms: Some(upstream_headers_ms),
                            upstream_first_chunk_ms: None,
                            upstream_body_read_ms: Some(upstream_body_read_ms),
                            upstream_error_class: cls,
                            upstream_error_hint: hint,
                            upstream_cf_ray: cf_ray,
                            client_uri: b.client_uri,
                            target_url: b.target_url,
                            client_headers: b.client_headers,
                            upstream_request_headers: b.upstream_request_headers,
                            auth_resolution: b.auth_resolution,
                            client_body: b.client_body_debug,
                            upstream_request_body: b.upstream_request_body_debug,
                            upstream_response_headers: Some(header_map_to_entries(&resp_headers)),
                            upstream_response_body: Some(make_body_preview(
                                bytes.as_ref(),
                                resp_ct,
                                max,
                            )),
                            upstream_error: None,
                        }
                    })
                } else if should_include_http_warn(status_code) {
                    http_debug_warn.clone()
                } else {
                    None
                };

                log_request_with_debug(
                    proxy.service_name,
                    method.as_str(),
                    uri.path(),
                    status_code,
                    dur,
                    Some(upstream_headers_ms),
                    &selected.station_name,
                    selected.upstream.tags.get("provider_id").cloned(),
                    &selected.upstream.base_url,
                    session_id.clone(),
                    cwd.clone(),
                    effective_effort.clone(),
                    usage.clone(),
                    retry.clone(),
                    http_debug,
                );
                proxy
                    .state
                    .finish_request(crate::state::FinishRequestParams {
                        id: request_id,
                        status_code,
                        duration_ms: dur,
                        ended_at_ms: started_at_ms + dur,
                        usage: usage.clone(),
                        retry,
                        ttfb_ms: Some(upstream_headers_ms),
                    })
                    .await;

                // Poll usage once after a user request finishes (e.g. packycode), used to drive auto-switching.
                if is_user_turn && is_codex_service {
                    usage_providers::poll_for_codex_upstream(
                        cfg_snapshot.clone(),
                        proxy.lb_states.clone(),
                        &selected.station_name,
                        selected.index,
                    )
                    .await;
                }

                let mut builder = Response::builder().status(status);
                for (name, value) in resp_headers_filtered.iter() {
                    builder = builder.header(name, value);
                }
                return Ok(builder.body(Body::from(bytes)).unwrap());
            }
        }

        let dur = start.elapsed().as_millis() as u64;
        let status = StatusCode::BAD_GATEWAY;
        let http_debug = if should_include_http_warn(status.as_u16()) {
            let client_headers_entries = client_headers_entries_cache
                .get_or_init(|| header_map_to_entries(&client_headers))
                .clone();
            Some(HttpDebugLog {
                request_body_len: Some(request_body_len),
                upstream_request_body_len: None,
                upstream_headers_ms: None,
                upstream_first_chunk_ms: None,
                upstream_body_read_ms: None,
                upstream_error_class: Some("retry_exhausted".to_string()),
                upstream_error_hint: Some("所有重试尝试均未能返回可用响应。".to_string()),
                upstream_cf_ray: None,
                client_uri: uri.to_string(),
                target_url: "-".to_string(),
                client_headers: client_headers_entries,
                upstream_request_headers: Vec::new(),
                auth_resolution: None,
                client_body: client_body_warn.clone(),
                upstream_request_body: None,
                upstream_response_headers: None,
                upstream_response_body: None,
                upstream_error: Some(format!(
                    "retry attempts exhausted; chain={:?}",
                    upstream_chain
                )),
            })
        } else {
            None
        };
        log_request_with_debug(
            proxy.service_name,
            method.as_str(),
            uri.path(),
            status.as_u16(),
            dur,
            None,
            "-",
            None,
            "-",
            session_id.clone(),
            cwd.clone(),
            effective_effort.clone(),
            None,
            retry_info_for_chain(&upstream_chain),
            http_debug,
        );
        let retry = retry_info_for_chain(&upstream_chain);
        proxy
            .state
            .finish_request(crate::state::FinishRequestParams {
                id: request_id,
                status_code: status.as_u16(),
                duration_ms: dur,
                ended_at_ms: started_at_ms + dur,
                usage: None,
                retry,
                ttfb_ms: None,
            })
            .await;
        Err((status, "retry attempts exhausted".to_string()))
    }
}

pub fn router(proxy: ProxyService) -> Router {
    // In axum 0.8, wildcard segments use `/{*path}` (equivalent to `/*path` from axum 0.7).
    #[derive(serde::Deserialize)]
    struct SessionReasoningEffortOverrideRequest {
        session_id: String,
        #[serde(default, alias = "effort")]
        reasoning_effort: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct SessionStationOverrideRequest {
        session_id: String,
        #[serde(default)]
        station_name: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct SessionModelOverrideRequest {
        session_id: String,
        model: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct SessionServiceTierOverrideRequest {
        session_id: String,
        service_tier: Option<String>,
    }

    #[derive(Debug, Clone, Copy, serde::Deserialize, PartialEq, Eq, Hash)]
    #[serde(rename_all = "snake_case")]
    enum SessionOverrideDimension {
        Model,
        ReasoningEffort,
        StationName,
        ServiceTier,
        All,
    }

    #[derive(serde::Deserialize)]
    struct SessionManualOverridesPatchRequest {
        session_id: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default, alias = "effort")]
        reasoning_effort: Option<String>,
        #[serde(default)]
        station_name: Option<String>,
        #[serde(default)]
        service_tier: Option<String>,
        #[serde(default)]
        clear: Vec<SessionOverrideDimension>,
    }

    #[derive(serde::Deserialize)]
    struct SessionProfileApplyRequest {
        session_id: String,
        profile_name: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct SessionOverrideResetRequest {
        session_id: String,
    }

    #[derive(serde::Serialize)]
    struct SessionOverridePrecedence {
        request_fields_apply_order: Vec<&'static str>,
        station_apply_order: Vec<&'static str>,
    }

    #[derive(serde::Serialize)]
    struct SessionManualOverridesListResponse {
        precedence: SessionOverridePrecedence,
        sessions: std::collections::HashMap<String, crate::state::SessionManualOverrides>,
    }

    #[derive(serde::Serialize)]
    struct SessionManualOverridesResponse {
        session_id: String,
        overrides: crate::state::SessionManualOverrides,
        precedence: SessionOverridePrecedence,
    }

    #[derive(serde::Deserialize)]
    struct DefaultProfileRequest {
        profile_name: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct PersistedProfileUpsertRequest {
        #[serde(default)]
        extends: Option<String>,
        #[serde(default, alias = "config")]
        station: Option<String>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        reasoning_effort: Option<String>,
        #[serde(default)]
        service_tier: Option<String>,
    }

    fn require_session_id(session_id: &str) -> Result<(), (StatusCode, String)> {
        if session_id.trim().is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "session_id is required".to_string(),
            ));
        }
        Ok(())
    }

    fn normalize_session_override_value(
        field_name: &str,
        value: Option<String>,
    ) -> Result<Option<String>, (StatusCode, String)> {
        match value {
            Some(value) => {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    Err((StatusCode::BAD_REQUEST, format!("{field_name} is empty")))
                } else {
                    Ok(Some(trimmed.to_string()))
                }
            }
            None => Ok(None),
        }
    }

    fn session_override_precedence() -> SessionOverridePrecedence {
        SessionOverridePrecedence {
            request_fields_apply_order: vec![
                "session_override",
                "profile_default",
                "request_payload",
                "station_mapping",
                "runtime_fallback",
            ],
            station_apply_order: vec![
                "session_override",
                "global_station_override",
                "profile_default",
                "runtime_fallback",
            ],
        }
    }

    fn default_persisted_station_enabled() -> bool {
        true
    }

    fn default_persisted_station_level() -> u8 {
        1
    }

    #[derive(serde::Deserialize)]
    struct PersistedStationUpdateRequest {
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        level: Option<u8>,
    }

    #[derive(serde::Deserialize)]
    struct PersistedStationSpecUpsertRequest {
        #[serde(default)]
        alias: Option<String>,
        #[serde(default = "default_persisted_station_enabled")]
        enabled: bool,
        #[serde(default = "default_persisted_station_level")]
        level: u8,
        #[serde(default)]
        members: Vec<crate::config::GroupMemberRefV2>,
    }

    #[derive(serde::Deserialize)]
    struct PersistedProviderEndpointSpecUpsertRequest {
        name: String,
        base_url: String,
        #[serde(default = "default_persisted_station_enabled")]
        enabled: bool,
    }

    #[derive(serde::Deserialize)]
    struct PersistedProviderSpecUpsertRequest {
        #[serde(default)]
        alias: Option<String>,
        #[serde(default = "default_persisted_station_enabled")]
        enabled: bool,
        #[serde(default)]
        auth_token_env: Option<String>,
        #[serde(default)]
        api_key_env: Option<String>,
        #[serde(default)]
        endpoints: Vec<PersistedProviderEndpointSpecUpsertRequest>,
    }

    #[derive(serde::Deserialize)]
    struct PersistedStationActiveRequest {
        #[serde(default)]
        station_name: Option<String>,
    }

    impl PersistedStationActiveRequest {
        fn station_name(&self) -> Result<Option<String>, (StatusCode, String)> {
            Ok(self
                .station_name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(ToOwned::to_owned))
        }
    }

    #[derive(serde::Deserialize)]
    struct GlobalStationOverrideRequest {
        #[serde(default)]
        station_name: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct StationRuntimeMetaRequest {
        #[serde(default)]
        station_name: Option<String>,
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        level: Option<u8>,
        #[serde(default)]
        clear_enabled: bool,
        #[serde(default)]
        clear_level: bool,
        #[serde(default)]
        runtime_state: Option<RuntimeConfigState>,
        #[serde(default)]
        clear_runtime_state: bool,
    }

    impl StationRuntimeMetaRequest {
        fn target_name(&self) -> Result<&str, (StatusCode, String)> {
            self.station_name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .ok_or((
                    StatusCode::BAD_REQUEST,
                    "station_name is required".to_string(),
                ))
        }
    }

    #[derive(serde::Serialize)]
    struct ProfilesResponse {
        default_profile: Option<String>,
        configured_default_profile: Option<String>,
        profiles: Vec<crate::dashboard_core::ControlProfileOption>,
    }

    fn sanitize_profile_name(profile_name: &str) -> Result<String, (StatusCode, String)> {
        let profile_name = profile_name.trim();
        if profile_name.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "profile name is required".to_string(),
            ));
        }
        Ok(profile_name.to_string())
    }

    fn sanitize_station_name(station_name: &str) -> Result<String, (StatusCode, String)> {
        let station_name = station_name.trim();
        if station_name.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "station name is required".to_string(),
            ));
        }
        Ok(station_name.to_string())
    }

    fn sanitize_provider_name(provider_name: &str) -> Result<String, (StatusCode, String)> {
        let provider_name = provider_name.trim();
        if provider_name.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "provider name is required".to_string(),
            ));
        }
        Ok(provider_name.to_string())
    }

    fn sanitize_profile_request(
        payload: PersistedProfileUpsertRequest,
    ) -> crate::config::ServiceControlProfile {
        fn normalize(value: Option<String>) -> Option<String> {
            value
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        }

        crate::config::ServiceControlProfile {
            extends: normalize(payload.extends),
            station: normalize(payload.station),
            model: normalize(payload.model),
            reasoning_effort: normalize(payload.reasoning_effort),
            service_tier: normalize(payload.service_tier),
        }
    }

    fn normalize_optional_config_string(value: Option<String>) -> Option<String> {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }

    fn sanitize_station_spec_request(
        payload: PersistedStationSpecUpsertRequest,
    ) -> Result<crate::config::PersistedStationSpec, (StatusCode, String)> {
        let mut members = Vec::new();
        for member in payload.members {
            let provider = member.provider.trim();
            if provider.is_empty() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "station member provider is required".to_string(),
                ));
            }

            let mut endpoint_names = member
                .endpoint_names
                .into_iter()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>();
            endpoint_names.dedup();

            members.push(crate::config::GroupMemberRefV2 {
                provider: provider.to_string(),
                endpoint_names,
                preferred: member.preferred,
            });
        }

        Ok(crate::config::PersistedStationSpec {
            name: String::new(),
            alias: normalize_optional_config_string(payload.alias),
            enabled: payload.enabled,
            level: payload.level.clamp(1, 10),
            members,
        })
    }

    fn sanitize_provider_spec_request(
        payload: PersistedProviderSpecUpsertRequest,
    ) -> Result<crate::config::PersistedProviderSpec, (StatusCode, String)> {
        let mut endpoints = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for endpoint in payload.endpoints {
            let endpoint_name = endpoint.name.trim();
            if endpoint_name.is_empty() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "provider endpoint name is required".to_string(),
                ));
            }
            let base_url = endpoint.base_url.trim();
            if base_url.is_empty() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("provider endpoint '{}' base_url is required", endpoint_name),
                ));
            }
            if !seen.insert(endpoint_name.to_string()) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("duplicate provider endpoint '{}'", endpoint_name),
                ));
            }

            endpoints.push(crate::config::PersistedProviderEndpointSpec {
                name: endpoint_name.to_string(),
                base_url: base_url.to_string(),
                enabled: endpoint.enabled,
            });
        }

        Ok(crate::config::PersistedProviderSpec {
            name: String::new(),
            alias: normalize_optional_config_string(payload.alias),
            enabled: payload.enabled,
            auth_token_env: normalize_optional_config_string(payload.auth_token_env),
            api_key_env: normalize_optional_config_string(payload.api_key_env),
            endpoints,
        })
    }

    fn merge_persisted_provider_spec(
        existing: Option<&crate::config::ProviderConfigV2>,
        provider: &crate::config::PersistedProviderSpec,
    ) -> crate::config::ProviderConfigV2 {
        let mut auth = existing
            .map(|provider| provider.auth.clone())
            .unwrap_or_default();
        auth.auth_token_env = provider.auth_token_env.clone();
        auth.api_key_env = provider.api_key_env.clone();

        crate::config::ProviderConfigV2 {
            alias: provider.alias.clone(),
            enabled: provider.enabled,
            auth,
            tags: existing
                .map(|provider| provider.tags.clone())
                .unwrap_or_default(),
            supported_models: existing
                .map(|provider| provider.supported_models.clone())
                .unwrap_or_default(),
            model_mapping: existing
                .map(|provider| provider.model_mapping.clone())
                .unwrap_or_default(),
            endpoints: provider
                .endpoints
                .iter()
                .map(|endpoint| {
                    let existing_endpoint = existing
                        .and_then(|provider| provider.endpoints.get(endpoint.name.as_str()));
                    (
                        endpoint.name.clone(),
                        crate::config::ProviderEndpointV2 {
                            base_url: endpoint.base_url.clone(),
                            enabled: endpoint.enabled,
                            tags: existing_endpoint
                                .map(|endpoint| endpoint.tags.clone())
                                .unwrap_or_default(),
                            supported_models: existing_endpoint
                                .map(|endpoint| endpoint.supported_models.clone())
                                .unwrap_or_default(),
                            model_mapping: existing_endpoint
                                .map(|endpoint| endpoint.model_mapping.clone())
                                .unwrap_or_default(),
                        },
                    )
                })
                .collect(),
        }
    }

    fn service_view_v2<'a>(
        cfg: &'a crate::config::ProxyConfigV2,
        service_name: &str,
    ) -> &'a crate::config::ServiceViewV2 {
        match service_name {
            "claude" => &cfg.claude,
            _ => &cfg.codex,
        }
    }

    fn service_view_v2_mut<'a>(
        cfg: &'a mut crate::config::ProxyConfigV2,
        service_name: &str,
    ) -> &'a mut crate::config::ServiceViewV2 {
        match service_name {
            "claude" => &mut cfg.claude,
            _ => &mut cfg.codex,
        }
    }

    fn validate_station_members_for_view(
        service_name: &str,
        station_name: &str,
        view: &crate::config::ServiceViewV2,
        members: &[crate::config::GroupMemberRefV2],
    ) -> Result<(), (StatusCode, String)> {
        for member in members {
            let provider = view
                .providers
                .get(member.provider.as_str())
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        format!(
                            "[{service_name}] station '{}' references missing provider '{}'",
                            station_name, member.provider
                        ),
                    )
                })?;

            for endpoint_name in &member.endpoint_names {
                if !provider.endpoints.contains_key(endpoint_name.as_str()) {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        format!(
                            "[{service_name}] station '{}' references missing endpoint '{}.{}'",
                            station_name, member.provider, endpoint_name
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    async fn make_profiles_response(proxy: &ProxyService) -> ProfilesResponse {
        let cfg = proxy.config.snapshot().await;
        let mgr = match proxy.service_name {
            "claude" => &cfg.claude,
            _ => &cfg.codex,
        };
        let default_profile =
            effective_default_profile_name(proxy.state.as_ref(), proxy.service_name, mgr).await;
        ProfilesResponse {
            default_profile: default_profile.clone(),
            configured_default_profile: mgr.default_profile.clone(),
            profiles: build_profile_options_from_mgr(mgr, default_profile.as_deref()),
        }
    }

    #[derive(serde::Serialize)]
    struct RuntimeConfigStatus {
        config_path: String,
        loaded_at_ms: u64,
        source_mtime_ms: Option<u64>,
        retry: crate::config::ResolvedRetryConfig,
    }

    #[derive(serde::Serialize)]
    struct RetryConfigResponse {
        configured: crate::config::RetryConfig,
        resolved: crate::config::ResolvedRetryConfig,
    }

    #[derive(serde::Serialize)]
    struct ReloadResult {
        reloaded: bool,
        status: RuntimeConfigStatus,
    }

    fn host_local_session_history_available() -> bool {
        let sessions_dir = crate::config::codex_sessions_dir();
        std::fs::metadata(sessions_dir)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false)
    }

    async fn runtime_config_status(
        proxy: ProxyService,
    ) -> Result<Json<RuntimeConfigStatus>, (StatusCode, String)> {
        let cfg = proxy.config.snapshot().await;
        Ok(Json(RuntimeConfigStatus {
            config_path: crate::config::config_file_path().display().to_string(),
            loaded_at_ms: proxy.config.last_loaded_at_ms(),
            source_mtime_ms: proxy.config.last_mtime_ms().await,
            retry: cfg.retry.resolve(),
        }))
    }

    async fn get_retry_config(
        proxy: ProxyService,
    ) -> Result<Json<RetryConfigResponse>, (StatusCode, String)> {
        let cfg = proxy.config.snapshot().await;
        Ok(Json(RetryConfigResponse {
            configured: cfg.retry.clone(),
            resolved: cfg.retry.resolve(),
        }))
    }

    async fn set_retry_config(
        proxy: ProxyService,
        Json(payload): Json<crate::config::RetryConfig>,
    ) -> Result<Json<RetryConfigResponse>, (StatusCode, String)> {
        let cfg_snapshot = proxy.config.snapshot().await;
        let mut cfg = cfg_snapshot.as_ref().clone();
        cfg.retry = payload;

        save_proxy_config_and_reload(&proxy, cfg).await?;
        let cfg = proxy.config.snapshot().await;
        Ok(Json(RetryConfigResponse {
            configured: cfg.retry.clone(),
            resolved: cfg.retry.resolve(),
        }))
    }

    async fn reload_runtime_config(
        proxy: ProxyService,
    ) -> Result<Json<ReloadResult>, (StatusCode, String)> {
        let changed = proxy
            .config
            .force_reload_from_disk()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let cfg = proxy.config.snapshot().await;
        Ok(Json(ReloadResult {
            reloaded: changed,
            status: RuntimeConfigStatus {
                config_path: crate::config::config_file_path().display().to_string(),
                loaded_at_ms: proxy.config.last_loaded_at_ms(),
                source_mtime_ms: proxy.config.last_mtime_ms().await,
                retry: cfg.retry.resolve(),
            },
        }))
    }

    async fn set_session_reasoning_effort_override(
        proxy: ProxyService,
        Json(payload): Json<SessionReasoningEffortOverrideRequest>,
    ) -> Result<StatusCode, (StatusCode, String)> {
        require_session_id(payload.session_id.as_str())?;
        let reasoning_effort =
            normalize_session_override_value("reasoning_effort", payload.reasoning_effort)?;
        if let Some(reasoning_effort) = reasoning_effort {
            proxy
                .state
                .set_session_reasoning_effort_override(
                    payload.session_id,
                    reasoning_effort,
                    now_ms(),
                )
                .await;
        } else {
            proxy
                .state
                .clear_session_reasoning_effort_override(payload.session_id.as_str())
                .await;
        }
        Ok(StatusCode::NO_CONTENT)
    }

    async fn list_session_reasoning_effort_overrides(
        proxy: ProxyService,
    ) -> Result<Json<std::collections::HashMap<String, String>>, (StatusCode, String)> {
        let map = proxy.state.list_session_reasoning_effort_overrides().await;
        Ok(Json(map))
    }

    async fn list_session_manual_overrides(
        proxy: ProxyService,
    ) -> Result<Json<SessionManualOverridesListResponse>, (StatusCode, String)> {
        let sessions = proxy.state.list_session_manual_overrides().await;
        Ok(Json(SessionManualOverridesListResponse {
            precedence: session_override_precedence(),
            sessions,
        }))
    }

    async fn apply_session_manual_overrides(
        proxy: ProxyService,
        Json(payload): Json<SessionManualOverridesPatchRequest>,
    ) -> Result<Json<SessionManualOverridesResponse>, (StatusCode, String)> {
        require_session_id(payload.session_id.as_str())?;
        let model = normalize_session_override_value("model", payload.model)?;
        let reasoning_effort =
            normalize_session_override_value("reasoning_effort", payload.reasoning_effort)?;
        let station_name = normalize_session_override_value("station_name", payload.station_name)?;
        let service_tier = normalize_session_override_value("service_tier", payload.service_tier)?;
        let clear: HashSet<_> = payload.clear.into_iter().collect();
        if model.is_none()
            && reasoning_effort.is_none()
            && station_name.is_none()
            && service_tier.is_none()
            && clear.is_empty()
        {
            return Err((
                StatusCode::BAD_REQUEST,
                "expected at least one override value or clear target".to_string(),
            ));
        }

        let session_id = payload.session_id;
        if clear.contains(&SessionOverrideDimension::All) {
            proxy
                .state
                .clear_session_manual_overrides(session_id.as_str())
                .await;
        } else {
            if clear.contains(&SessionOverrideDimension::Model) {
                proxy
                    .state
                    .clear_session_model_override(session_id.as_str())
                    .await;
            }
            if clear.contains(&SessionOverrideDimension::ReasoningEffort) {
                proxy
                    .state
                    .clear_session_reasoning_effort_override(session_id.as_str())
                    .await;
            }
            if clear.contains(&SessionOverrideDimension::StationName) {
                proxy
                    .state
                    .clear_session_station_override(session_id.as_str())
                    .await;
            }
            if clear.contains(&SessionOverrideDimension::ServiceTier) {
                proxy
                    .state
                    .clear_session_service_tier_override(session_id.as_str())
                    .await;
            }
        }

        if let Some(model) = model {
            proxy
                .state
                .set_session_model_override(session_id.clone(), model, now_ms())
                .await;
        }
        if let Some(reasoning_effort) = reasoning_effort {
            proxy
                .state
                .set_session_reasoning_effort_override(
                    session_id.clone(),
                    reasoning_effort,
                    now_ms(),
                )
                .await;
        }
        if let Some(station_name) = station_name {
            proxy
                .state
                .set_session_station_override(session_id.clone(), station_name, now_ms())
                .await;
        }
        if let Some(service_tier) = service_tier {
            proxy
                .state
                .set_session_service_tier_override(session_id.clone(), service_tier, now_ms())
                .await;
        }

        let overrides = proxy
            .state
            .get_session_manual_overrides(session_id.as_str())
            .await;
        Ok(Json(SessionManualOverridesResponse {
            session_id,
            overrides,
            precedence: session_override_precedence(),
        }))
    }

    async fn set_session_station_override(
        proxy: ProxyService,
        Json(payload): Json<SessionStationOverrideRequest>,
    ) -> Result<StatusCode, (StatusCode, String)> {
        require_session_id(payload.session_id.as_str())?;
        let station_name = normalize_session_override_value("station_name", payload.station_name)?;
        if let Some(station_name) = station_name {
            proxy
                .state
                .set_session_station_override(
                    payload.session_id,
                    station_name,
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0),
                )
                .await;
        } else {
            proxy
                .state
                .clear_session_station_override(payload.session_id.as_str())
                .await;
        }
        Ok(StatusCode::NO_CONTENT)
    }

    async fn list_session_station_overrides(
        proxy: ProxyService,
    ) -> Result<Json<std::collections::HashMap<String, String>>, (StatusCode, String)> {
        let map = proxy.state.list_session_station_overrides().await;
        Ok(Json(map))
    }

    async fn set_session_model_override(
        proxy: ProxyService,
        Json(payload): Json<SessionModelOverrideRequest>,
    ) -> Result<StatusCode, (StatusCode, String)> {
        require_session_id(payload.session_id.as_str())?;
        let model = normalize_session_override_value("model", payload.model)?;
        if let Some(model) = model {
            proxy
                .state
                .set_session_model_override(payload.session_id, model, now_ms())
                .await;
        } else {
            proxy
                .state
                .clear_session_model_override(payload.session_id.as_str())
                .await;
        }
        Ok(StatusCode::NO_CONTENT)
    }

    async fn list_session_model_overrides(
        proxy: ProxyService,
    ) -> Result<Json<std::collections::HashMap<String, String>>, (StatusCode, String)> {
        let map = proxy.state.list_session_model_overrides().await;
        Ok(Json(map))
    }

    async fn set_session_service_tier_override(
        proxy: ProxyService,
        Json(payload): Json<SessionServiceTierOverrideRequest>,
    ) -> Result<StatusCode, (StatusCode, String)> {
        require_session_id(payload.session_id.as_str())?;
        let service_tier = normalize_session_override_value("service_tier", payload.service_tier)?;
        if let Some(service_tier) = service_tier {
            proxy
                .state
                .set_session_service_tier_override(payload.session_id, service_tier, now_ms())
                .await;
        } else {
            proxy
                .state
                .clear_session_service_tier_override(payload.session_id.as_str())
                .await;
        }
        Ok(StatusCode::NO_CONTENT)
    }

    async fn list_session_service_tier_overrides(
        proxy: ProxyService,
    ) -> Result<Json<std::collections::HashMap<String, String>>, (StatusCode, String)> {
        let map = proxy.state.list_session_service_tier_overrides().await;
        Ok(Json(map))
    }

    async fn reset_session_manual_overrides(
        proxy: ProxyService,
        Json(payload): Json<SessionOverrideResetRequest>,
    ) -> Result<StatusCode, (StatusCode, String)> {
        require_session_id(payload.session_id.as_str())?;
        proxy
            .state
            .clear_session_manual_overrides(payload.session_id.as_str())
            .await;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn list_profiles(
        proxy: ProxyService,
    ) -> Result<Json<ProfilesResponse>, (StatusCode, String)> {
        Ok(Json(make_profiles_response(&proxy).await))
    }

    async fn save_proxy_config_and_reload(
        proxy: &ProxyService,
        cfg: crate::config::ProxyConfig,
    ) -> Result<(), (StatusCode, String)> {
        crate::config::save_config(&cfg)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        proxy
            .config
            .force_reload_from_disk()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(())
    }

    async fn save_profiles_config_and_reload(
        proxy: &ProxyService,
        cfg: crate::config::ProxyConfig,
    ) -> Result<ProfilesResponse, (StatusCode, String)> {
        save_proxy_config_and_reload(proxy, cfg).await?;
        Ok(make_profiles_response(proxy).await)
    }

    async fn load_persisted_config_v2() -> Result<crate::config::ProxyConfigV2, (StatusCode, String)>
    {
        let path = crate::config::config_file_path();
        if path.exists()
            && path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
        {
            let text = tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            let version = toml::from_str::<toml::Value>(&text)
                .ok()
                .and_then(|value| value.get("version").and_then(|v| v.as_integer()))
                .map(|value| value as u32);
            if version == Some(2) {
                return toml::from_str::<crate::config::ProxyConfigV2>(&text)
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
            }
        }

        let runtime = crate::config::load_config()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        crate::config::compact_v2_config(&crate::config::migrate_legacy_to_v2(&runtime))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
    }

    async fn save_proxy_config_v2_and_reload(
        proxy: &ProxyService,
        cfg: crate::config::ProxyConfigV2,
    ) -> Result<(), (StatusCode, String)> {
        crate::config::save_config_v2(&cfg)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        proxy
            .config
            .force_reload_from_disk()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(())
    }

    async fn list_persisted_station_specs(
        proxy: ProxyService,
    ) -> Result<Json<crate::config::PersistedStationsCatalog>, (StatusCode, String)> {
        let cfg = load_persisted_config_v2().await?;
        Ok(Json(crate::config::build_persisted_station_catalog(
            service_view_v2(&cfg, proxy.service_name),
        )))
    }

    async fn list_persisted_provider_specs(
        proxy: ProxyService,
    ) -> Result<Json<crate::config::PersistedProvidersCatalog>, (StatusCode, String)> {
        let cfg = load_persisted_config_v2().await?;
        Ok(Json(crate::config::build_persisted_provider_catalog(
            service_view_v2(&cfg, proxy.service_name),
        )))
    }

    async fn upsert_persisted_profile(
        proxy: ProxyService,
        Path(profile_name): Path<String>,
        Json(payload): Json<PersistedProfileUpsertRequest>,
    ) -> Result<Json<ProfilesResponse>, (StatusCode, String)> {
        let profile_name = sanitize_profile_name(profile_name.as_str())?;
        let profile = sanitize_profile_request(payload);

        let cfg_snapshot = proxy.config.snapshot().await;
        let mut cfg = cfg_snapshot.as_ref().clone();
        let mgr = match proxy.service_name {
            "claude" => &mut cfg.claude,
            _ => &mut cfg.codex,
        };

        mgr.profiles.insert(profile_name.clone(), profile);
        let resolved = crate::config::resolve_service_profile(mgr, profile_name.as_str())
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        crate::config::validate_profile_station_compatibility(
            proxy.service_name,
            mgr,
            profile_name.as_str(),
            &resolved,
        )
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

        Ok(Json(save_profiles_config_and_reload(&proxy, cfg).await?))
    }

    async fn delete_persisted_profile(
        proxy: ProxyService,
        Path(profile_name): Path<String>,
    ) -> Result<Json<ProfilesResponse>, (StatusCode, String)> {
        let profile_name = sanitize_profile_name(profile_name.as_str())?;

        let cfg_snapshot = proxy.config.snapshot().await;
        let mut cfg = cfg_snapshot.as_ref().clone();
        let mgr = match proxy.service_name {
            "claude" => &mut cfg.claude,
            _ => &mut cfg.codex,
        };

        let referencing_profiles = mgr
            .profiles
            .iter()
            .filter_map(|(name, profile)| {
                (profile.extends.as_deref() == Some(profile_name.as_str())).then_some(name.clone())
            })
            .collect::<Vec<_>>();
        if !referencing_profiles.is_empty() {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "profile '{}' is extended by profiles: {}",
                    profile_name,
                    referencing_profiles.join(", ")
                ),
            ));
        }

        if mgr.profiles.remove(profile_name.as_str()).is_none() {
            return Err((
                StatusCode::NOT_FOUND,
                format!("profile '{}' not found", profile_name),
            ));
        }
        if mgr.default_profile.as_deref() == Some(profile_name.as_str()) {
            mgr.default_profile = None;
        }

        save_profiles_config_and_reload(&proxy, cfg).await?;
        if proxy
            .state
            .get_runtime_default_profile_override(proxy.service_name)
            .await
            .as_deref()
            == Some(profile_name.as_str())
        {
            proxy
                .state
                .clear_runtime_default_profile_override(proxy.service_name)
                .await;
        }

        Ok(Json(make_profiles_response(&proxy).await))
    }

    async fn update_persisted_station(
        proxy: ProxyService,
        Path(station_name): Path<String>,
        Json(payload): Json<PersistedStationUpdateRequest>,
    ) -> Result<StatusCode, (StatusCode, String)> {
        let station_name = sanitize_station_name(station_name.as_str())?;
        if payload.enabled.is_none() && payload.level.is_none() {
            return Err((
                StatusCode::BAD_REQUEST,
                "at least one persisted station field must be provided".to_string(),
            ));
        }

        let cfg_snapshot = proxy.config.snapshot().await;
        let mut cfg = cfg_snapshot.as_ref().clone();
        let mgr = match proxy.service_name {
            "claude" => &mut cfg.claude,
            _ => &mut cfg.codex,
        };
        let Some(station) = mgr.station_mut(station_name.as_str()) else {
            return Err((
                StatusCode::NOT_FOUND,
                format!("station '{}' not found", station_name),
            ));
        };
        if let Some(enabled) = payload.enabled {
            station.enabled = enabled;
        }
        if let Some(level) = payload.level {
            station.level = level.clamp(1, 10);
        }

        save_proxy_config_and_reload(&proxy, cfg).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn set_persisted_active_station(
        proxy: ProxyService,
        Json(payload): Json<PersistedStationActiveRequest>,
    ) -> Result<StatusCode, (StatusCode, String)> {
        let station_name = payload.station_name()?;

        let cfg_snapshot = proxy.config.snapshot().await;
        let mut cfg = cfg_snapshot.as_ref().clone();
        let mgr = match proxy.service_name {
            "claude" => &mut cfg.claude,
            _ => &mut cfg.codex,
        };
        if let Some(station_name) = station_name.as_deref()
            && !mgr.contains_station(station_name)
        {
            return Err((
                StatusCode::NOT_FOUND,
                format!("station '{}' not found", station_name),
            ));
        }
        mgr.active = station_name;

        save_proxy_config_and_reload(&proxy, cfg).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn upsert_persisted_station_spec(
        proxy: ProxyService,
        Path(station_name): Path<String>,
        Json(payload): Json<PersistedStationSpecUpsertRequest>,
    ) -> Result<StatusCode, (StatusCode, String)> {
        let station_name = sanitize_station_name(station_name.as_str())?;
        let mut station = sanitize_station_spec_request(payload)?;
        station.name = station_name.clone();

        let mut cfg = load_persisted_config_v2().await?;
        let view = service_view_v2_mut(&mut cfg, proxy.service_name);
        validate_station_members_for_view(
            proxy.service_name,
            station_name.as_str(),
            view,
            &station.members,
        )?;
        view.groups.insert(
            station_name.clone(),
            crate::config::GroupConfigV2 {
                alias: station.alias.clone(),
                enabled: station.enabled,
                level: station.level.clamp(1, 10),
                members: station.members.clone(),
            },
        );

        crate::config::compile_v2_to_runtime(&cfg)
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        save_proxy_config_v2_and_reload(&proxy, cfg).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn delete_persisted_station_spec(
        proxy: ProxyService,
        Path(station_name): Path<String>,
    ) -> Result<StatusCode, (StatusCode, String)> {
        let station_name = sanitize_station_name(station_name.as_str())?;
        let mut cfg = load_persisted_config_v2().await?;
        let view = service_view_v2_mut(&mut cfg, proxy.service_name);

        let referencing_profiles = view
            .profiles
            .iter()
            .filter_map(|(profile_name, profile)| {
                (profile.station.as_deref() == Some(station_name.as_str()))
                    .then_some(profile_name.clone())
            })
            .collect::<Vec<_>>();
        if !referencing_profiles.is_empty() {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "station '{}' is referenced by profiles: {}",
                    station_name,
                    referencing_profiles.join(", ")
                ),
            ));
        }

        if view.groups.remove(station_name.as_str()).is_none() {
            return Err((
                StatusCode::NOT_FOUND,
                format!("station '{}' not found", station_name),
            ));
        }
        if view.active_group.as_deref() == Some(station_name.as_str()) {
            view.active_group = None;
        }

        crate::config::compile_v2_to_runtime(&cfg)
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        save_proxy_config_v2_and_reload(&proxy, cfg).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn upsert_persisted_provider_spec(
        proxy: ProxyService,
        Path(provider_name): Path<String>,
        Json(payload): Json<PersistedProviderSpecUpsertRequest>,
    ) -> Result<StatusCode, (StatusCode, String)> {
        let provider_name = sanitize_provider_name(provider_name.as_str())?;
        let mut provider = sanitize_provider_spec_request(payload)?;
        provider.name = provider_name.clone();

        let mut cfg = load_persisted_config_v2().await?;
        let view = service_view_v2_mut(&mut cfg, proxy.service_name);
        let existing_provider = view.providers.get(provider_name.as_str()).cloned();
        view.providers.insert(
            provider_name,
            merge_persisted_provider_spec(existing_provider.as_ref(), &provider),
        );

        crate::config::compile_v2_to_runtime(&cfg)
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        save_proxy_config_v2_and_reload(&proxy, cfg).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn delete_persisted_provider_spec(
        proxy: ProxyService,
        Path(provider_name): Path<String>,
    ) -> Result<StatusCode, (StatusCode, String)> {
        let provider_name = sanitize_provider_name(provider_name.as_str())?;
        let mut cfg = load_persisted_config_v2().await?;
        let view = service_view_v2_mut(&mut cfg, proxy.service_name);

        let referencing_stations = view
            .groups
            .iter()
            .filter_map(|(station_name, station)| {
                station
                    .members
                    .iter()
                    .any(|member| member.provider == provider_name)
                    .then_some(station_name.clone())
            })
            .collect::<Vec<_>>();
        if !referencing_stations.is_empty() {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "provider '{}' is referenced by stations: {}",
                    provider_name,
                    referencing_stations.join(", ")
                ),
            ));
        }

        if view.providers.remove(provider_name.as_str()).is_none() {
            return Err((
                StatusCode::NOT_FOUND,
                format!("provider '{}' not found", provider_name),
            ));
        }

        crate::config::compile_v2_to_runtime(&cfg)
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        save_proxy_config_v2_and_reload(&proxy, cfg).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn apply_station_runtime_meta(
        proxy: ProxyService,
        Json(payload): Json<StationRuntimeMetaRequest>,
    ) -> Result<StatusCode, (StatusCode, String)> {
        let station_name = payload.target_name()?.to_string();

        if payload.enabled.is_none()
            && payload.level.is_none()
            && !payload.clear_enabled
            && !payload.clear_level
            && payload.runtime_state.is_none()
            && !payload.clear_runtime_state
        {
            return Err((
                StatusCode::BAD_REQUEST,
                "at least one runtime station action must be provided".to_string(),
            ));
        }

        let cfg = proxy.config.snapshot().await;
        let mgr = match proxy.service_name {
            "claude" => &cfg.claude,
            _ => &cfg.codex,
        };
        if !mgr.contains_station(station_name.as_str()) {
            return Err((
                StatusCode::NOT_FOUND,
                format!("station '{}' not found", station_name),
            ));
        }

        let now = now_ms();
        if payload.clear_enabled {
            proxy
                .state
                .clear_station_enabled_override(proxy.service_name, station_name.as_str())
                .await;
        } else if let Some(enabled) = payload.enabled {
            proxy
                .state
                .set_station_enabled_override(
                    proxy.service_name,
                    station_name.clone(),
                    enabled,
                    now,
                )
                .await;
        }

        if payload.clear_level {
            proxy
                .state
                .clear_station_level_override(proxy.service_name, station_name.as_str())
                .await;
        } else if let Some(level) = payload.level {
            proxy
                .state
                .set_station_level_override(
                    proxy.service_name,
                    station_name.clone(),
                    level.clamp(1, 10),
                    now,
                )
                .await;
        }

        if payload.clear_runtime_state {
            proxy
                .state
                .clear_station_runtime_state_override(proxy.service_name, station_name.as_str())
                .await;
        } else if let Some(runtime_state) = payload.runtime_state {
            proxy
                .state
                .set_station_runtime_state_override(
                    proxy.service_name,
                    station_name.clone(),
                    runtime_state,
                    now,
                )
                .await;
        }

        Ok(StatusCode::NO_CONTENT)
    }

    async fn set_persisted_default_profile(
        proxy: ProxyService,
        Json(payload): Json<DefaultProfileRequest>,
    ) -> Result<Json<ProfilesResponse>, (StatusCode, String)> {
        let profile_name = payload
            .profile_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string());

        let cfg_snapshot = proxy.config.snapshot().await;
        let mut cfg = cfg_snapshot.as_ref().clone();
        let mgr = match proxy.service_name {
            "claude" => &mut cfg.claude,
            _ => &mut cfg.codex,
        };

        if let Some(profile_name) = profile_name.as_deref() {
            if mgr.profile(profile_name).is_none() {
                return Err((
                    StatusCode::NOT_FOUND,
                    format!("profile '{}' not found", profile_name),
                ));
            }
            let resolved = crate::config::resolve_service_profile(mgr, profile_name)
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            crate::config::validate_profile_station_compatibility(
                proxy.service_name,
                mgr,
                profile_name,
                &resolved,
            )
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        }
        mgr.default_profile = profile_name;

        Ok(Json(save_profiles_config_and_reload(&proxy, cfg).await?))
    }

    async fn set_default_profile(
        proxy: ProxyService,
        Json(payload): Json<DefaultProfileRequest>,
    ) -> Result<StatusCode, (StatusCode, String)> {
        let profile_name = payload
            .profile_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string());

        if let Some(profile_name) = profile_name {
            let cfg = proxy.config.snapshot().await;
            let mgr = match proxy.service_name {
                "claude" => &cfg.claude,
                _ => &cfg.codex,
            };
            if mgr.profile(profile_name.as_str()).is_none() {
                return Err((
                    StatusCode::NOT_FOUND,
                    format!("profile '{}' not found", profile_name),
                ));
            }
            let resolved = crate::config::resolve_service_profile(mgr, profile_name.as_str())
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            crate::config::validate_profile_station_compatibility(
                proxy.service_name,
                mgr,
                profile_name.as_str(),
                &resolved,
            )
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            proxy
                .state
                .set_runtime_default_profile_override(
                    proxy.service_name.to_string(),
                    profile_name,
                    now_ms(),
                )
                .await;
        } else {
            proxy
                .state
                .clear_runtime_default_profile_override(proxy.service_name)
                .await;
        }

        Ok(StatusCode::NO_CONTENT)
    }

    async fn apply_session_profile(
        proxy: ProxyService,
        Json(payload): Json<SessionProfileApplyRequest>,
    ) -> Result<StatusCode, (StatusCode, String)> {
        if payload.session_id.trim().is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "session_id is required".to_string(),
            ));
        }
        let profile_name = payload
            .profile_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string());

        if profile_name.is_none() {
            proxy
                .state
                .clear_session_binding(payload.session_id.as_str())
                .await;
            return Ok(StatusCode::NO_CONTENT);
        }

        let cfg = proxy.config.snapshot().await;
        let mgr = match proxy.service_name {
            "claude" => &cfg.claude,
            _ => &cfg.codex,
        };
        let profile_name = profile_name.expect("profile_name checked above");
        if mgr.profile(profile_name.as_str()).is_none() {
            return Err((
                StatusCode::NOT_FOUND,
                format!("profile '{}' not found", profile_name),
            ));
        }

        let now = now_ms();
        if let Err(err) = proxy
            .state
            .apply_session_profile_binding(
                proxy.service_name,
                mgr,
                payload.session_id,
                profile_name,
                now,
            )
            .await
        {
            return Err((StatusCode::BAD_REQUEST, err.to_string()));
        }
        Ok(StatusCode::NO_CONTENT)
    }

    async fn get_global_station_override(
        proxy: ProxyService,
    ) -> Result<Json<Option<String>>, (StatusCode, String)> {
        Ok(Json(proxy.state.get_global_station_override().await))
    }

    async fn set_global_station_override(
        proxy: ProxyService,
        Json(payload): Json<GlobalStationOverrideRequest>,
    ) -> Result<StatusCode, (StatusCode, String)> {
        if let Some(station_name) = payload.station_name {
            if station_name.trim().is_empty() {
                return Err((StatusCode::BAD_REQUEST, "station_name is empty".to_string()));
            }
            proxy
                .state
                .set_global_station_override(
                    station_name,
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0),
                )
                .await;
        } else {
            proxy.state.clear_global_station_override().await;
        }
        Ok(StatusCode::NO_CONTENT)
    }

    async fn list_active_requests(
        proxy: ProxyService,
    ) -> Result<Json<Vec<ActiveRequest>>, (StatusCode, String)> {
        let vec = proxy.state.list_active_requests().await;
        Ok(Json(vec))
    }

    async fn list_session_stats(
        proxy: ProxyService,
    ) -> Result<
        Json<std::collections::HashMap<String, crate::state::SessionStats>>,
        (StatusCode, String),
    > {
        let map = proxy.state.list_session_stats().await;
        Ok(Json(map))
    }

    async fn load_session_identity_cards(
        proxy: &ProxyService,
    ) -> Vec<crate::state::SessionIdentityCard> {
        let mut cards = proxy
            .state
            .list_session_identity_cards_with_host_transcripts(2_000)
            .await;
        let cfg = proxy.config.snapshot().await;
        let mgr = match proxy.service_name {
            "claude" => &cfg.claude,
            _ => &cfg.codex,
        };
        crate::state::enrich_session_identity_cards_with_runtime(&mut cards, mgr);
        cards
    }

    async fn list_session_identity_cards(
        proxy: ProxyService,
    ) -> Result<Json<Vec<crate::state::SessionIdentityCard>>, (StatusCode, String)> {
        Ok(Json(load_session_identity_cards(&proxy).await))
    }

    async fn get_session_identity_card(
        proxy: ProxyService,
        Path(session_id): Path<String>,
    ) -> Result<Json<crate::state::SessionIdentityCard>, (StatusCode, String)> {
        require_session_id(session_id.as_str())?;
        let cards = load_session_identity_cards(&proxy).await;
        cards
            .into_iter()
            .find(|card| card.session_id.as_deref() == Some(session_id.as_str()))
            .map(Json)
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    format!("session '{}' not found", session_id),
                )
            })
    }

    #[derive(serde::Deserialize)]
    struct RecentQuery {
        limit: Option<usize>,
    }

    async fn list_recent_finished(
        proxy: ProxyService,
        Query(q): Query<RecentQuery>,
    ) -> Result<Json<Vec<FinishedRequest>>, (StatusCode, String)> {
        let limit = q.limit.unwrap_or(50).clamp(1, 200);
        let vec = proxy.state.list_recent_finished(limit).await;
        Ok(Json(vec))
    }

    async fn api_capabilities(
        proxy: ProxyService,
    ) -> Result<Json<ApiV1Capabilities>, (StatusCode, String)> {
        let host_local_history = host_local_session_history_available();
        Ok(Json(ApiV1Capabilities {
            api_version: 1,
            service_name: proxy.service_name.to_string(),
            endpoints: vec![
                "/__codex_helper/api/v1/capabilities",
                "/__codex_helper/api/v1/snapshot",
                "/__codex_helper/api/v1/sessions",
                "/__codex_helper/api/v1/sessions/{session_id}",
                "/__codex_helper/api/v1/status/active",
                "/__codex_helper/api/v1/status/recent",
                "/__codex_helper/api/v1/status/session-stats",
                "/__codex_helper/api/v1/status/health-checks",
                "/__codex_helper/api/v1/status/station-health",
                "/__codex_helper/api/v1/runtime/status",
                "/__codex_helper/api/v1/runtime/reload",
                "/__codex_helper/api/v1/retry/config",
                "/__codex_helper/api/v1/stations",
                "/__codex_helper/api/v1/stations/runtime",
                "/__codex_helper/api/v1/stations/config-active",
                "/__codex_helper/api/v1/stations/probe",
                "/__codex_helper/api/v1/stations/{name}",
                "/__codex_helper/api/v1/stations/specs",
                "/__codex_helper/api/v1/stations/specs/{name}",
                "/__codex_helper/api/v1/providers/specs",
                "/__codex_helper/api/v1/providers/specs/{name}",
                "/__codex_helper/api/v1/profiles",
                "/__codex_helper/api/v1/profiles/default",
                "/__codex_helper/api/v1/profiles/default/persisted",
                "/__codex_helper/api/v1/profiles/{name}",
                "/__codex_helper/api/v1/overrides/session",
                "/__codex_helper/api/v1/overrides/session/profile",
                "/__codex_helper/api/v1/overrides/session/model",
                "/__codex_helper/api/v1/overrides/session/effort",
                "/__codex_helper/api/v1/overrides/session/station",
                "/__codex_helper/api/v1/overrides/session/service-tier",
                "/__codex_helper/api/v1/overrides/session/reset",
                "/__codex_helper/api/v1/overrides/global-station",
                "/__codex_helper/api/v1/healthcheck/start",
                "/__codex_helper/api/v1/healthcheck/cancel",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            shared_capabilities: SharedControlPlaneCapabilities {
                session_observability: true,
                request_history: true,
            },
            host_local_capabilities: HostLocalControlPlaneCapabilities {
                session_history: host_local_history,
                transcript_read: host_local_history,
                cwd_enrichment: host_local_history,
            },
            remote_admin_access: admin_access_capabilities(),
        }))
    }

    #[derive(serde::Deserialize)]
    struct SnapshotQuery {
        recent_limit: Option<usize>,
        stats_days: Option<usize>,
    }

    async fn api_v1_snapshot(
        proxy: ProxyService,
        Query(q): Query<SnapshotQuery>,
    ) -> Result<Json<crate::dashboard_core::ApiV1Snapshot>, (StatusCode, String)> {
        let recent_limit = q.recent_limit.unwrap_or(200).clamp(1, 2_000);
        let stats_days = q.stats_days.unwrap_or(21).clamp(1, 365);

        let cfg = proxy.config.snapshot().await;
        let mgr = match proxy.service_name {
            "claude" => &cfg.claude,
            _ => &cfg.codex,
        };
        let meta_overrides = proxy
            .state
            .get_station_meta_overrides(proxy.service_name)
            .await;
        let state_overrides = proxy
            .state
            .get_station_runtime_state_overrides(proxy.service_name)
            .await;
        let stations = crate::dashboard_core::build_station_options_from_mgr(
            mgr,
            &meta_overrides,
            &state_overrides,
        );
        let configured_active_station = configured_active_station_name(mgr);
        let effective_active_station = effective_active_station_name(mgr);
        let default_profile =
            effective_default_profile_name(proxy.state.as_ref(), proxy.service_name, mgr).await;

        let mut snapshot = crate::dashboard_core::build_dashboard_snapshot(
            &proxy.state,
            proxy.service_name,
            recent_limit,
            stats_days,
        )
        .await;
        crate::state::enrich_session_identity_cards_with_runtime(&mut snapshot.session_cards, mgr);

        Ok(Json(crate::dashboard_core::ApiV1Snapshot {
            api_version: 1,
            service_name: proxy.service_name.to_string(),
            runtime_loaded_at_ms: Some(proxy.config.last_loaded_at_ms()),
            runtime_source_mtime_ms: proxy.config.last_mtime_ms().await,
            stations,
            configured_active_station,
            effective_active_station,
            default_profile: default_profile.clone(),
            profiles: build_profile_options_from_mgr(mgr, default_profile.as_deref()),
            snapshot,
        }))
    }

    async fn list_health_checks(
        proxy: ProxyService,
    ) -> Result<
        Json<std::collections::HashMap<String, crate::state::HealthCheckStatus>>,
        (StatusCode, String),
    > {
        let map = proxy.state.list_health_checks(proxy.service_name).await;
        Ok(Json(map))
    }

    async fn list_station_health(
        proxy: ProxyService,
    ) -> Result<
        Json<std::collections::HashMap<String, crate::state::StationHealth>>,
        (StatusCode, String),
    > {
        let map = proxy.state.get_station_health(proxy.service_name).await;
        Ok(Json(map))
    }

    #[derive(Debug, serde::Deserialize)]
    struct HealthCheckAction {
        #[serde(default)]
        all: bool,
        #[serde(default)]
        station_names: Vec<String>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct StationProbeRequest {
        #[serde(default)]
        station_name: Option<String>,
    }

    impl StationProbeRequest {
        fn station_name(&self) -> Result<String, (StatusCode, String)> {
            self.station_name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(ToOwned::to_owned)
                .ok_or((
                    StatusCode::BAD_REQUEST,
                    "station_name is required".to_string(),
                ))
        }
    }

    #[derive(Debug, serde::Serialize)]
    struct HealthCheckActionResult {
        started: Vec<String>,
        already_running: Vec<String>,
        missing: Vec<String>,
        cancel_requested: Vec<String>,
        not_running: Vec<String>,
    }

    async fn spawn_health_checks_for_targets(
        proxy: &ProxyService,
        targets: Vec<(String, Vec<crate::config::UpstreamConfig>)>,
    ) -> HealthCheckActionResult {
        let mut started = Vec::new();
        let mut already_running = Vec::new();
        for (name, upstreams) in targets {
            let now = now_ms();
            if !proxy
                .state
                .try_begin_station_health_check(proxy.service_name, &name, upstreams.len(), now)
                .await
            {
                already_running.push(name);
                continue;
            }

            proxy
                .state
                .record_station_health(
                    proxy.service_name,
                    name.clone(),
                    crate::state::StationHealth {
                        checked_at_ms: now,
                        upstreams: Vec::new(),
                    },
                )
                .await;

            let state = proxy.state.clone();
            let service_name = proxy.service_name;
            let station_name = name.clone();
            tokio::spawn(async move {
                crate::healthcheck::run_health_check_for_station(
                    state,
                    service_name,
                    station_name,
                    upstreams,
                )
                .await;
            });
            started.push(name);
        }

        HealthCheckActionResult {
            started,
            already_running,
            missing: Vec::new(),
            cancel_requested: Vec::new(),
            not_running: Vec::new(),
        }
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    async fn start_health_checks(
        proxy: ProxyService,
        Json(payload): Json<HealthCheckAction>,
    ) -> Result<Json<HealthCheckActionResult>, (StatusCode, String)> {
        let cfg = proxy.config.snapshot().await;
        let mgr = match proxy.service_name {
            "claude" => &cfg.claude,
            _ => &cfg.codex,
        };

        let mut targets = if payload.all {
            mgr.stations().keys().cloned().collect::<Vec<_>>()
        } else {
            payload.station_names
        };
        targets.retain(|s| !s.trim().is_empty());
        targets.sort();
        targets.dedup();
        if targets.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "expected { all: true } or non-empty station_names".to_string(),
            ));
        }

        let mut missing = Vec::new();
        let mut resolved_targets = Vec::new();
        for name in targets {
            let Some(svc) = mgr.station(&name) else {
                missing.push(name);
                continue;
            };
            resolved_targets.push((name, svc.upstreams.clone()));
        }

        let mut result = spawn_health_checks_for_targets(&proxy, resolved_targets).await;
        result.missing = missing;
        Ok(Json(result))
    }

    async fn probe_station(
        proxy: ProxyService,
        Json(payload): Json<StationProbeRequest>,
    ) -> Result<Json<HealthCheckActionResult>, (StatusCode, String)> {
        let cfg = proxy.config.snapshot().await;
        let mgr = match proxy.service_name {
            "claude" => &cfg.claude,
            _ => &cfg.codex,
        };

        let station_name = payload.station_name()?;
        let Some(station) = mgr.station(&station_name) else {
            return Err((
                StatusCode::NOT_FOUND,
                format!("station '{}' not found", station_name),
            ));
        };

        let result = spawn_health_checks_for_targets(
            &proxy,
            vec![(station_name, station.upstreams.clone())],
        )
        .await;
        Ok(Json(result))
    }

    async fn cancel_health_checks(
        proxy: ProxyService,
        Json(payload): Json<HealthCheckAction>,
    ) -> Result<Json<HealthCheckActionResult>, (StatusCode, String)> {
        let cfg = proxy.config.snapshot().await;
        let mgr = match proxy.service_name {
            "claude" => &cfg.claude,
            _ => &cfg.codex,
        };

        let mut targets = if payload.all {
            mgr.stations().keys().cloned().collect::<Vec<_>>()
        } else {
            payload.station_names
        };
        targets.retain(|s| !s.trim().is_empty());
        targets.sort();
        targets.dedup();
        if targets.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "expected { all: true } or non-empty station_names".to_string(),
            ));
        }

        let now = now_ms();
        let mut cancel_requested = Vec::new();
        let mut not_running = Vec::new();
        let mut missing = Vec::new();
        for name in targets {
            if !mgr.contains_station(&name) {
                missing.push(name);
                continue;
            }
            let ok = proxy
                .state
                .request_cancel_station_health_check(proxy.service_name, &name, now)
                .await;
            if ok {
                cancel_requested.push(name);
            } else {
                not_running.push(name);
            }
        }

        Ok(Json(HealthCheckActionResult {
            started: Vec::new(),
            already_running: Vec::new(),
            missing,
            cancel_requested,
            not_running,
        }))
    }

    async fn list_stations(
        proxy: ProxyService,
    ) -> Result<Json<Vec<crate::dashboard_core::StationOption>>, (StatusCode, String)> {
        let cfg = proxy.config.snapshot().await;
        let mgr = match proxy.service_name {
            "claude" => &cfg.claude,
            _ => &cfg.codex,
        };
        let meta_overrides = proxy
            .state
            .get_station_meta_overrides(proxy.service_name)
            .await;
        let state_overrides = proxy
            .state
            .get_station_runtime_state_overrides(proxy.service_name)
            .await;
        Ok(Json(crate::dashboard_core::build_station_options_from_mgr(
            mgr,
            &meta_overrides,
            &state_overrides,
        )))
    }

    let admin_access = AdminAccessConfig::from_env();

    let p2 = proxy.clone();
    let p8 = proxy.clone();
    let p9 = proxy.clone();
    let p10 = proxy.clone();
    let p11 = proxy.clone();
    let p12 = proxy.clone();
    let p13 = proxy.clone();
    let p15 = proxy.clone();
    let p16 = proxy.clone();
    let p17 = proxy.clone();
    let p18 = proxy.clone();
    let p19 = proxy.clone();
    let p20 = proxy.clone();
    let p21 = proxy.clone();
    let p22 = proxy.clone();
    let p23 = proxy.clone();
    let p24 = proxy.clone();
    let p25 = proxy.clone();
    let p26 = proxy.clone();
    let p27 = proxy.clone();
    let p28 = proxy.clone();
    let p29 = proxy.clone();
    let p30 = proxy.clone();
    let p31 = proxy.clone();
    let p32 = proxy.clone();
    let p33 = proxy.clone();
    let p35 = proxy.clone();
    let p36 = proxy.clone();
    let p37 = proxy.clone();
    let p38 = proxy.clone();
    let p39 = proxy.clone();
    let p40 = proxy.clone();
    let p41 = proxy.clone();
    let p42 = proxy.clone();
    let p43 = proxy.clone();
    let p44 = proxy.clone();
    let p45 = proxy.clone();
    let p46 = proxy.clone();
    let p47 = proxy.clone();
    let p48 = proxy.clone();
    let p49 = proxy.clone();
    let p50 = proxy.clone();
    let p51 = proxy.clone();
    let p52 = proxy.clone();
    let p53 = proxy.clone();
    let p56 = proxy.clone();

    let admin_routes = Router::new()
        // Versioned API (v1): attach-friendly, safe-by-default (no secrets).
        .route(
            "/__codex_helper/api/v1/capabilities",
            get(move || api_capabilities(p8.clone())),
        )
        .route(
            "/__codex_helper/api/v1/snapshot",
            get(move |q| api_v1_snapshot(p25.clone(), q)),
        )
        .route(
            "/__codex_helper/api/v1/sessions",
            get(move || list_session_identity_cards(p26.clone())),
        )
        .route(
            "/__codex_helper/api/v1/sessions/{session_id}",
            get(move |session_id| get_session_identity_card(p56.clone(), session_id)),
        )
        .route(
            "/__codex_helper/api/v1/status/active",
            get(move || list_active_requests(p9.clone())),
        )
        .route(
            "/__codex_helper/api/v1/status/recent",
            get(move |q| list_recent_finished(p10.clone(), q)),
        )
        .route(
            "/__codex_helper/api/v1/status/session-stats",
            get(move || list_session_stats(p11.clone())),
        )
        .route(
            "/__codex_helper/api/v1/status/health-checks",
            get(move || list_health_checks(p21.clone())),
        )
        .route(
            "/__codex_helper/api/v1/status/station-health",
            get(move || list_station_health(p22.clone())),
        )
        .route(
            "/__codex_helper/api/v1/runtime/status",
            get(move || runtime_config_status(p12.clone())),
        )
        .route(
            "/__codex_helper/api/v1/runtime/reload",
            post(move || reload_runtime_config(p13.clone())),
        )
        .route(
            "/__codex_helper/api/v1/retry/config",
            get(move || get_retry_config(p43.clone()))
                .post(move |payload| set_retry_config(p44.clone(), payload)),
        )
        .route(
            "/__codex_helper/api/v1/stations",
            get(move || list_stations(p35.clone())),
        )
        .route(
            "/__codex_helper/api/v1/stations/runtime",
            post(move |payload| apply_station_runtime_meta(p36.clone(), payload)),
        )
        .route(
            "/__codex_helper/api/v1/stations/config-active",
            post(move |payload| set_persisted_active_station(p41.clone(), payload)),
        )
        .route(
            "/__codex_helper/api/v1/stations/probe",
            post(move |payload| probe_station(p51.clone(), payload)),
        )
        .route(
            "/__codex_helper/api/v1/stations/{name}",
            put(move |name, payload| update_persisted_station(p42.clone(), name, payload)),
        )
        .route(
            "/__codex_helper/api/v1/stations/specs",
            get(move || list_persisted_station_specs(p37.clone())),
        )
        .route(
            "/__codex_helper/api/v1/stations/specs/{name}",
            put(move |name, payload| upsert_persisted_station_spec(p45.clone(), name, payload))
                .delete(move |name| delete_persisted_station_spec(p46.clone(), name)),
        )
        .route(
            "/__codex_helper/api/v1/providers/specs",
            get(move || list_persisted_provider_specs(p47.clone())),
        )
        .route(
            "/__codex_helper/api/v1/providers/specs/{name}",
            put(move |name, payload| upsert_persisted_provider_spec(p48.clone(), name, payload))
                .delete(move |name| delete_persisted_provider_spec(p49.clone(), name)),
        )
        .route(
            "/__codex_helper/api/v1/profiles",
            get(move || list_profiles(p31.clone())),
        )
        .route(
            "/__codex_helper/api/v1/profiles/default",
            post(move |payload| set_default_profile(p33.clone(), payload)),
        )
        .route(
            "/__codex_helper/api/v1/profiles/default/persisted",
            post(move |payload| set_persisted_default_profile(p38.clone(), payload)),
        )
        .route(
            "/__codex_helper/api/v1/profiles/{name}",
            put(move |name, payload| upsert_persisted_profile(p39.clone(), name, payload))
                .delete(move |name| delete_persisted_profile(p40.clone(), name)),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session",
            get(move || list_session_manual_overrides(p52.clone()))
                .post(move |payload| apply_session_manual_overrides(p53.clone(), payload)),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/profile",
            post(move |payload| apply_session_profile(p32.clone(), payload)),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/model",
            get(move || list_session_model_overrides(p15.clone()))
                .post(move |payload| set_session_model_override(p16.clone(), payload)),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/effort",
            get(move || list_session_reasoning_effort_overrides(p17.clone()))
                .post(move |payload| set_session_reasoning_effort_override(p18.clone(), payload)),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/station",
            get(move || list_session_station_overrides(p19.clone()))
                .post(move |payload| set_session_station_override(p20.clone(), payload)),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/service-tier",
            get(move || list_session_service_tier_overrides(p23.clone()))
                .post(move |payload| set_session_service_tier_override(p24.clone(), payload)),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/reset",
            post(move |payload| reset_session_manual_overrides(p50.clone(), payload)),
        )
        .route(
            "/__codex_helper/api/v1/overrides/global-station",
            get(move || get_global_station_override(p27.clone()))
                .post(move |payload| set_global_station_override(p28.clone(), payload)),
        )
        .route(
            "/__codex_helper/api/v1/healthcheck/start",
            post(move |payload| start_health_checks(p29.clone(), payload)),
        )
        .route(
            "/__codex_helper/api/v1/healthcheck/cancel",
            post(move |payload| cancel_health_checks(p30.clone(), payload)),
        )
        .layer(middleware::from_fn_with_state(
            admin_access,
            require_admin_access,
        ));

    Router::new()
        .merge(admin_routes)
        .merge(proxy_only_router(p2))
}

pub fn proxy_only_router(proxy: ProxyService) -> Router {
    proxy_only_router_with_admin_base_url(proxy, None)
}

pub fn proxy_only_router_with_admin_base_url(
    proxy: ProxyService,
    admin_base_url: Option<String>,
) -> Router {
    let service_name = proxy.service_name;
    let discovery = admin_base_url.map(|admin_base_url| {
        Json(ProxyAdminDiscovery {
            api_version: 1,
            service_name,
            admin_base_url,
        })
    });

    let mut router = Router::new();
    if let Some(discovery) = discovery {
        router = router.route(
            "/.well-known/codex-helper-admin",
            get(move || {
                let discovery = discovery.clone();
                async move { discovery }
            }),
        );
    }

    router
        .route("/{*path}", any(move |req| handle_proxy(proxy.clone(), req)))
        .layer(middleware::from_fn(reject_admin_paths_from_proxy))
}

pub fn admin_listener_router(proxy: ProxyService) -> Router {
    router(proxy).layer(middleware::from_fn(require_admin_path_only))
}

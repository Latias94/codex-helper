use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use axum::Json;
use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, Uri};
use axum::middleware;
use axum::routing::{any, get, post, put};
use reqwest::Client;
use std::sync::OnceLock;
use tracing::instrument;

mod admin;
mod api_responses;
mod auth_resolution;
mod classify;
mod client_identity;
mod control_plane;
mod headers;
mod healthcheck_api;
mod http_debug;
mod passive_health;
mod persisted_config_api;
mod profile_defaults;
mod request_body;
mod retry;
mod route_provenance;
mod runtime_admin_api;
mod runtime_config;
mod session_overrides;
mod stations_api;
mod stream;
#[cfg(test)]
mod tests;

use crate::config::{ProxyConfig, RetryStrategy, ServiceConfigManager};
use crate::filter::RequestFilter;
use crate::lb::{LbState, LoadBalancer, SelectedUpstream};
use crate::logging::{
    HeaderEntry, HttpDebugLog, ServiceTierLog, http_debug_options, http_warn_options,
    log_request_with_debug, log_retry_trace, make_body_preview, now_ms, should_include_http_warn,
    should_log_request_body_preview,
};
use crate::model_routing;
use crate::state::{ProxyState, RuntimeConfigState};
use crate::usage::extract_usage_from_bytes;

use self::admin::{
    AdminAccessConfig, ProxyAdminDiscovery, reject_admin_paths_from_proxy, require_admin_access,
    require_admin_path_only,
};
pub use self::admin::{
    admin_base_url_from_proxy_base_url, admin_loopback_addr_for_proxy_port,
    admin_port_for_proxy_port, local_admin_base_url_for_proxy_port, local_proxy_base_url,
};
use self::auth_resolution::{resolve_api_key_with_source, resolve_auth_token_with_source};
use self::classify::{class_is_health_neutral, classify_upstream_response};
use self::client_identity::{extract_client_addr, extract_client_name, extract_session_id};
use self::control_plane::{
    api_capabilities, api_v1_snapshot, apply_session_profile, get_global_station_override,
    get_session_identity_card, list_active_requests, list_recent_finished,
    list_session_identity_cards, list_session_stats, set_default_profile,
    set_global_station_override,
};
use self::headers::{filter_request_headers, filter_response_headers};
use self::healthcheck_api::{
    cancel_health_checks, list_health_checks, list_station_health, probe_station,
    start_health_checks,
};
use self::http_debug::format_reqwest_error_for_retry_chain;
use self::passive_health::{record_passive_upstream_failure, record_passive_upstream_success};
use self::persisted_config_api::{
    delete_persisted_profile, delete_persisted_provider_spec, delete_persisted_station_spec,
    list_persisted_provider_specs, list_persisted_station_specs, set_persisted_active_station,
    set_persisted_default_profile, update_persisted_station, upsert_persisted_profile,
    upsert_persisted_provider_spec, upsert_persisted_station_spec,
};
use self::profile_defaults::effective_default_profile_name;
use self::request_body::{
    apply_model_override, apply_reasoning_effort_override, apply_service_tier_override,
    extract_model_from_request_body, extract_reasoning_effort_from_request_body,
    extract_service_tier_from_request_body, extract_service_tier_from_response_body,
};
use self::retry::{
    backoff_sleep, retry_info_for_chain, retry_plan, retry_sleep, should_never_retry,
    should_retry_class, should_retry_status,
};
use self::route_provenance::build_route_decision_provenance;
use self::runtime_admin_api::{
    get_retry_config, list_profiles, reload_runtime_config, runtime_config_status, set_retry_config,
};
use self::runtime_config::RuntimeConfig;
use self::session_overrides::{
    apply_session_manual_overrides, list_session_manual_overrides, list_session_model_overrides,
    list_session_reasoning_effort_overrides, list_session_service_tier_overrides,
    list_session_station_overrides, reset_session_manual_overrides, set_session_model_override,
    set_session_reasoning_effort_override, set_session_service_tier_override,
    set_session_station_override,
};
use self::stations_api::{apply_station_runtime_meta, list_stations};
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

#[allow(dead_code)]
fn lb_state_snapshot_json(lb: &LoadBalancer) -> Option<serde_json::Value> {
    passive_health::lb_state_snapshot_json(lb)
}

#[cfg(test)]
fn codex_auth_json_value(key: &str) -> Option<String> {
    auth_resolution::codex_auth_json_value(key)
}

#[cfg(test)]
fn claude_settings_env_value(key: &str) -> Option<String> {
    auth_resolution::claude_settings_env_value(key)
}

fn header_map_to_entries(headers: &HeaderMap) -> Vec<HeaderEntry> {
    headers::header_map_to_entries(headers)
}

type HttpDebugBase = http_debug::HttpDebugBase;

fn warn_http_debug(status_code: u16, http_debug: &HttpDebugLog) {
    http_debug::warn_http_debug(status_code, http_debug);
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

pub(super) fn scan_service_tier_from_sse_bytes_incremental(
    data: &[u8],
    scan_pos: &mut usize,
    last: &mut Option<String>,
) {
    request_body::scan_service_tier_from_sse_bytes_incremental(data, scan_pos, last);
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

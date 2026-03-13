use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{HeaderMap, Request, Response, StatusCode};
use reqwest::Client;
use tracing::instrument;

mod admin;
mod api_responses;
mod attempt_execution;
mod attempt_failures;
mod attempt_request;
mod attempt_response;
mod attempt_selection;
mod attempt_transport;
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
mod provider_execution;
mod provider_orchestration;
mod request_body;
mod request_context;
mod request_failures;
mod request_preparation;
mod request_routing;
mod response_finalization;
mod retry;
mod route_provenance;
mod router_setup;
mod runtime_admin_api;
mod runtime_config;
mod selected_upstream_request;
mod session_overrides;
mod stations_api;
mod stream;
mod target_builder;
#[cfg(test)]
mod tests;

use crate::config::{ProxyConfig, ServiceConfigManager};
use crate::filter::RequestFilter;
use crate::lb::{LbState, LoadBalancer};
use crate::logging::{HeaderEntry, HttpDebugLog, now_ms};
use crate::state::{ProxyState, RuntimeConfigState};

pub use self::admin::{
    admin_base_url_from_proxy_base_url, admin_loopback_addr_for_proxy_port,
    admin_port_for_proxy_port, local_admin_base_url_for_proxy_port, local_proxy_base_url,
};
use self::profile_defaults::effective_default_profile_name;
use self::provider_execution::{
    ExecuteProviderChainParams, ProviderExecutionOutcome, execute_provider_chain, log_retry_options,
};
use self::request_context::prepare_proxy_request;
use self::request_failures::{finish_failed_proxy_request, no_upstreams_available_error};
use self::retry::retry_info_for_chain;
pub use self::router_setup::{
    admin_listener_router, proxy_only_router, proxy_only_router_with_admin_base_url, router,
};
use self::runtime_config::RuntimeConfig;

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

    let prepared = prepare_proxy_request(&proxy, req, &start, started_at_ms).await?;
    log_retry_options(proxy.service_name, prepared.request_id, &prepared.plan);
    let provider_execution = execute_provider_chain(ExecuteProviderChainParams {
        proxy: &proxy,
        lbs: &prepared.lbs,
        method: &prepared.method,
        uri: &prepared.uri,
        client_headers: &prepared.client_headers,
        client_headers_entries_cache: &prepared.client_headers_entries_cache,
        client_uri: prepared.client_uri.as_str(),
        start: &start,
        started_at_ms,
        request_id: prepared.request_id,
        request_body_len: prepared.request_body_len,
        body_for_upstream: &prepared.body_for_upstream,
        request_model: prepared.request_model.as_deref(),
        session_binding: prepared.session_binding.as_ref(),
        session_override_config: prepared.session_override_config.as_deref(),
        global_station_override: prepared.global_station_override.as_deref(),
        override_model: prepared.override_model.as_deref(),
        override_effort: prepared.override_effort.as_deref(),
        override_service_tier: prepared.override_service_tier.as_deref(),
        effective_effort: prepared.effective_effort.as_deref(),
        effective_service_tier: prepared.effective_service_tier.as_deref(),
        base_service_tier: &prepared.base_service_tier,
        session_id: prepared.session_id.as_deref(),
        cwd: prepared.cwd.as_deref(),
        request_flavor: &prepared.request_flavor,
        request_body_previews: prepared.request_body_previews,
        debug_max: prepared.debug_max,
        warn_max: prepared.warn_max,
        client_body_debug: prepared.client_body_debug.as_ref(),
        client_body_warn: prepared.client_body_warn.as_ref(),
        plan: &prepared.plan,
        cooldown_backoff: prepared.cooldown_backoff,
    })
    .await;
    let (upstream_chain, last_err) = match provider_execution {
        ProviderExecutionOutcome::Return(response) => return Ok(response),
        ProviderExecutionOutcome::Exhausted(state) => (state.upstream_chain, state.last_err),
    };

    // If we reach here, all provider attempts are exhausted.
    if let Some((status, msg)) = last_err {
        let dur = start.elapsed().as_millis() as u64;
        let retry = retry_info_for_chain(&upstream_chain);
        return Err(finish_failed_proxy_request(
            &proxy,
            &prepared.method,
            prepared.uri.path(),
            prepared.request_id,
            status,
            msg,
            dur,
            started_at_ms,
            prepared.session_id.clone(),
            prepared.cwd.clone(),
            prepared.effective_effort.clone(),
            prepared.base_service_tier.clone(),
            retry,
        )
        .await);
    }

    return Err(no_upstreams_available_error());
}

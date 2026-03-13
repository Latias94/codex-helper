use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes, to_bytes};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode};
use reqwest::Client;
use std::sync::OnceLock;
use tracing::instrument;

mod admin;
mod api_responses;
mod attempt_failures;
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
mod request_failures;
mod request_preparation;
mod request_routing;
mod response_finalization;
mod retry;
mod route_provenance;
mod router_setup;
mod runtime_admin_api;
mod runtime_config;
mod session_overrides;
mod stations_api;
mod stream;
mod target_builder;
#[cfg(test)]
mod tests;

use crate::config::{ProxyConfig, RetryStrategy, ServiceConfigManager};
use crate::filter::RequestFilter;
use crate::lb::{LbState, LoadBalancer, SelectedUpstream};
use crate::logging::{
    HeaderEntry, HttpDebugLog, ServiceTierLog, http_debug_options, http_warn_options,
    log_retry_trace, now_ms, should_log_request_body_preview,
};
use crate::model_routing;
use crate::state::{ProxyState, RuntimeConfigState};
use crate::usage::extract_usage_from_bytes;

pub use self::admin::{
    admin_base_url_from_proxy_base_url, admin_loopback_addr_for_proxy_port,
    admin_port_for_proxy_port, local_admin_base_url_for_proxy_port, local_proxy_base_url,
};
use self::attempt_failures::apply_terminal_upstream_failure;
use self::auth_resolution::{resolve_api_key_with_source, resolve_auth_token_with_source};
use self::classify::{class_is_health_neutral, classify_upstream_response};
use self::client_identity::{extract_client_addr, extract_client_name, extract_session_id};
use self::headers::{filter_request_headers, filter_response_headers};
use self::http_debug::format_reqwest_error_for_retry_chain;
use self::passive_health::{record_passive_upstream_failure, record_passive_upstream_success};
use self::profile_defaults::effective_default_profile_name;
use self::request_body::{apply_model_override, extract_service_tier_from_response_body};
use self::request_failures::{
    finish_failed_proxy_request, log_client_body_read_error, log_no_routable_station,
    no_upstreams_available_error,
};
use self::request_preparation::{build_body_previews, detect_request_flavor, prepare_request_body};
use self::response_finalization::{
    FinalizeForwardResponseParams, finish_and_build_forward_response,
};
use self::retry::{
    backoff_sleep, retry_info_for_chain, retry_plan, retry_sleep, should_never_retry,
    should_retry_class, should_retry_status,
};
use self::route_provenance::build_route_decision_provenance;
pub use self::router_setup::{
    admin_listener_router, proxy_only_router, proxy_only_router_with_admin_base_url, router,
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

    let (parts, body) = req.into_parts();
    let client_addr = extract_client_addr(&parts.extensions);
    let uri = parts.uri;
    let client_uri = uri.to_string();
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
        let client_headers_entries = client_headers_entries_cache
            .get_or_init(|| header_map_to_entries(&client_headers))
            .clone();
        return Err(log_no_routable_station(
            &proxy,
            &method,
            uri.path(),
            client_uri.as_str(),
            session_id.clone(),
            client_headers_entries,
            dur,
        ));
    }
    let request_flavor =
        detect_request_flavor(proxy.service_name, &method, &client_headers, uri.path());

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
            let err_str = e.to_string();
            let client_headers_entries = client_headers_entries_cache
                .get_or_init(|| header_map_to_entries(&client_headers))
                .clone();
            return Err(log_client_body_read_error(
                &proxy,
                &method,
                uri.path(),
                client_uri.as_str(),
                session_id.clone(),
                cwd.clone(),
                client_headers_entries,
                dur,
                err_str,
            ));
        }
    };
    let override_effort = if let Some(id) = session_id.as_deref() {
        proxy.state.get_session_effort_override(id).await
    } else {
        None
    };
    let binding_effort = session_binding
        .as_ref()
        .and_then(|binding| binding.reasoning_effort.as_deref());
    let override_model = if let Some(id) = session_id.as_deref() {
        proxy.state.get_session_model_override(id).await
    } else {
        None
    };
    let binding_model = session_binding
        .as_ref()
        .and_then(|binding| binding.model.as_deref());
    let override_service_tier = if let Some(id) = session_id.as_deref() {
        proxy.state.get_session_service_tier_override(id).await
    } else {
        None
    };
    let binding_service_tier = session_binding
        .as_ref()
        .and_then(|binding| binding.service_tier.as_deref());
    let prepared_request = prepare_request_body(
        &raw_body,
        override_effort.as_deref(),
        binding_effort,
        override_model.as_deref(),
        binding_model,
        override_service_tier.as_deref(),
        binding_service_tier,
    );
    let body_for_upstream = prepared_request.body_for_upstream.clone();
    let request_model = prepared_request.request_model.clone();
    let effective_effort = prepared_request.effective_effort.clone();
    let effective_service_tier = prepared_request.base_service_tier.effective.clone();
    let base_service_tier = prepared_request.base_service_tier.clone();
    let request_body_len = prepared_request.request_body_len;

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
    let client_body_previews = build_body_previews(
        &raw_body,
        request_flavor.client_content_type.as_deref(),
        request_body_previews,
        debug_max,
        warn_max,
    );
    let client_body_debug = client_body_previews.debug.clone();
    let client_body_warn = client_body_previews.warn.clone();

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
            let upstream_body_previews = build_body_previews(
                &filtered_body,
                request_flavor.client_content_type.as_deref(),
                request_body_previews,
                debug_max,
                warn_max,
            );
            let upstream_request_body_debug = upstream_body_previews.debug.clone();
            let upstream_request_body_warn = upstream_body_previews.warn.clone();

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
                        apply_terminal_upstream_failure(
                            &proxy,
                            None,
                            &selected,
                            "target_build_error",
                            None,
                            plan.transport_cooldown_secs,
                            cooldown_backoff,
                            err_str,
                            avoid_set,
                            &mut avoided_total,
                            &mut last_err,
                        )
                        .await;
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

                        apply_terminal_upstream_failure(
                            &proxy,
                            Some(&lb),
                            &selected,
                            "upstream_transport_error",
                            Some("upstream_transport_error"),
                            plan.transport_cooldown_secs,
                            cooldown_backoff,
                            err_str,
                            avoid_set,
                            &mut avoided_total,
                            &mut last_err,
                        )
                        .await;
                        break;
                    }
                };

                let upstream_headers_ms = upstream_start.elapsed().as_millis() as u64;
                let status = resp.status();
                let success = status.is_success();
                let resp_headers = resp.headers().clone();
                let resp_headers_filtered = filter_response_headers(&resp_headers);

                if request_flavor.is_stream && success {
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
                            is_user_turn: request_flavor.is_user_turn,
                            is_codex_service: request_flavor.is_codex_service,
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
                        apply_terminal_upstream_failure(
                            &proxy,
                            Some(&lb),
                            &selected,
                            "upstream_body_read_error",
                            Some("upstream_body_read_error"),
                            plan.transport_cooldown_secs,
                            cooldown_backoff,
                            err_str,
                            avoid_set,
                            &mut avoided_total,
                            &mut last_err,
                        )
                        .await;
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
                    let retry = retry_info_for_chain(&upstream_chain);
                    let service_tier_for_log = ServiceTierLog {
                        actual: observed_service_tier.clone(),
                        ..base_service_tier.clone()
                    };
                    return Ok(finish_and_build_forward_response(
                        &proxy,
                        &method,
                        uri.path(),
                        FinalizeForwardResponseParams {
                            request_id,
                            status,
                            duration_ms: dur,
                            started_at_ms,
                            upstream_headers_ms,
                            station_name: selected.station_name.clone(),
                            provider_id: provider_id.clone(),
                            upstream_base_url: selected.upstream.base_url.clone(),
                            session_id: session_id.clone(),
                            cwd: cwd.clone(),
                            effective_effort: effective_effort.clone(),
                            service_tier: service_tier_for_log,
                            usage,
                            retry,
                            response_headers: resp_headers_filtered,
                            response_body: bytes,
                        },
                    )
                    .await);
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
                    let service_tier_for_log = ServiceTierLog {
                        actual: observed_service_tier.clone(),
                        ..base_service_tier.clone()
                    };
                    return Ok(finish_and_build_forward_response(
                        &proxy,
                        &method,
                        uri.path(),
                        FinalizeForwardResponseParams {
                            request_id,
                            status,
                            duration_ms: dur,
                            started_at_ms,
                            upstream_headers_ms,
                            station_name: selected.station_name.clone(),
                            provider_id: provider_id.clone(),
                            upstream_base_url: selected.upstream.base_url.clone(),
                            session_id: session_id.clone(),
                            cwd: cwd.clone(),
                            effective_effort: effective_effort.clone(),
                            service_tier: service_tier_for_log,
                            usage: None,
                            retry,
                            response_headers: resp_headers_filtered,
                            response_body: bytes,
                        },
                    )
                    .await);
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

                let service_tier_for_log = ServiceTierLog {
                    actual: observed_service_tier,
                    ..base_service_tier.clone()
                };
                return Ok(finish_and_build_forward_response(
                    &proxy,
                    &method,
                    uri.path(),
                    FinalizeForwardResponseParams {
                        request_id,
                        status,
                        duration_ms: dur,
                        started_at_ms,
                        upstream_headers_ms,
                        station_name: selected.station_name.clone(),
                        provider_id: provider_id.clone(),
                        upstream_base_url: selected.upstream.base_url.clone(),
                        session_id: session_id.clone(),
                        cwd: cwd.clone(),
                        effective_effort: effective_effort.clone(),
                        service_tier: service_tier_for_log,
                        usage: None,
                        retry,
                        response_headers: resp_headers_filtered,
                        response_body: bytes,
                    },
                )
                .await);
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
        let retry = retry_info_for_chain(&upstream_chain);
        return Err(finish_failed_proxy_request(
            &proxy,
            &method,
            uri.path(),
            request_id,
            status,
            msg,
            dur,
            started_at_ms,
            session_id.clone(),
            cwd.clone(),
            effective_effort.clone(),
            base_service_tier.clone(),
            retry,
        )
        .await);
    }

    return Err(no_upstreams_available_error());
}

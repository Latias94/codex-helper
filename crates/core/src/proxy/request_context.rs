use std::sync::OnceLock;
use std::time::Instant;

use axum::body::{Body, Bytes, to_bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, Uri};

use crate::lb::{CooldownBackoff, LoadBalancer};
use crate::logging::{BodyPreview, HeaderEntry, ServiceTierLog};
use crate::state::SessionBinding;

use super::ProxyService;
use super::client_identity::{extract_client_addr, extract_client_name, extract_session_id};
use super::headers::header_map_to_entries;
use super::request_failures::{log_client_body_read_error, log_no_routable_station};
use super::request_preparation::{
    RequestFlavor, build_body_previews, detect_request_flavor, prepare_request_body,
};
use super::retry::{RetryPlan, retry_plan};

pub(super) struct PreparedProxyRequest {
    pub(super) method: Method,
    pub(super) uri: Uri,
    pub(super) client_uri: String,
    pub(super) client_headers: HeaderMap,
    pub(super) client_headers_entries_cache: OnceLock<Vec<HeaderEntry>>,
    pub(super) session_id: Option<String>,
    pub(super) session_binding: Option<SessionBinding>,
    pub(super) lbs: Vec<LoadBalancer>,
    pub(super) cwd: Option<String>,
    pub(super) session_override_config: Option<String>,
    pub(super) global_station_override: Option<String>,
    pub(super) override_effort: Option<String>,
    pub(super) override_model: Option<String>,
    pub(super) override_service_tier: Option<String>,
    pub(super) body_for_upstream: Bytes,
    pub(super) request_model: Option<String>,
    pub(super) effective_effort: Option<String>,
    pub(super) effective_service_tier: Option<String>,
    pub(super) base_service_tier: ServiceTierLog,
    pub(super) request_body_len: usize,
    pub(super) request_flavor: RequestFlavor,
    pub(super) request_body_previews: bool,
    pub(super) debug_max: usize,
    pub(super) warn_max: usize,
    pub(super) client_body_debug: Option<BodyPreview>,
    pub(super) client_body_warn: Option<BodyPreview>,
    pub(super) request_id: u64,
    pub(super) plan: RetryPlan,
    pub(super) cooldown_backoff: CooldownBackoff,
}

pub(super) async fn prepare_proxy_request(
    proxy: &ProxyService,
    req: Request<Body>,
    start: &Instant,
    started_at_ms: u64,
) -> Result<PreparedProxyRequest, (StatusCode, String)> {
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
            proxy,
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
    let cwd = resolve_and_touch_session_state(proxy, session_id.as_deref(), started_at_ms).await;
    let session_override_config = if let Some(id) = session_id.as_deref() {
        proxy.state.get_session_station_override(id).await
    } else {
        None
    };
    let global_station_override = proxy.state.get_global_station_override().await;

    let raw_body = match to_bytes(body, 10 * 1024 * 1024).await {
        Ok(body) => body,
        Err(error) => {
            let dur = start.elapsed().as_millis() as u64;
            let client_headers_entries = client_headers_entries_cache
                .get_or_init(|| header_map_to_entries(&client_headers))
                .clone();
            return Err(log_client_body_read_error(
                super::request_failures::ClientBodyReadErrorParams {
                    proxy,
                    method: &method,
                    path: uri.path(),
                    client_uri: client_uri.as_str(),
                    session_id: session_id.clone(),
                    cwd: cwd.clone(),
                    client_headers: client_headers_entries,
                    duration_ms: dur,
                    error_message: error.to_string(),
                },
            ));
        }
    };

    let override_effort = if let Some(id) = session_id.as_deref() {
        proxy.state.get_session_effort_override(id).await
    } else {
        None
    };
    let override_model = if let Some(id) = session_id.as_deref() {
        proxy.state.get_session_model_override(id).await
    } else {
        None
    };
    let override_service_tier = if let Some(id) = session_id.as_deref() {
        proxy.state.get_session_service_tier_override(id).await
    } else {
        None
    };
    let binding_effort = session_binding
        .as_ref()
        .and_then(|binding| binding.reasoning_effort.as_deref());
    let binding_model = session_binding
        .as_ref()
        .and_then(|binding| binding.model.as_deref());
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

    let debug_opt = crate::logging::http_debug_options();
    let warn_opt = crate::logging::http_warn_options();
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
    let request_body_previews = crate::logging::should_log_request_body_preview();
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
            client_name,
            client_addr,
            cwd.clone(),
            request_model.clone(),
            effective_effort.clone(),
            effective_service_tier.clone(),
            started_at_ms,
        )
        .await;

    let plan = retry_plan(&cfg_snapshot.retry.resolve());
    let cooldown_backoff = CooldownBackoff {
        factor: plan.cooldown_backoff_factor,
        max_secs: plan.cooldown_backoff_max_secs,
    };

    Ok(PreparedProxyRequest {
        method,
        uri,
        client_uri,
        client_headers,
        client_headers_entries_cache,
        session_id,
        session_binding,
        lbs,
        cwd,
        session_override_config,
        global_station_override,
        override_effort,
        override_model,
        override_service_tier,
        body_for_upstream,
        request_model,
        effective_effort,
        effective_service_tier,
        base_service_tier,
        request_body_len,
        request_flavor,
        request_body_previews,
        debug_max,
        warn_max,
        client_body_debug,
        client_body_warn,
        request_id,
        plan,
        cooldown_backoff,
    })
}

async fn resolve_and_touch_session_state(
    proxy: &ProxyService,
    session_id: Option<&str>,
    started_at_ms: u64,
) -> Option<String> {
    let cwd = if let Some(id) = session_id {
        proxy.state.resolve_session_cwd(id).await
    } else {
        None
    };

    if let Some(id) = session_id {
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

    cwd
}

use std::sync::OnceLock;
use std::time::Instant;

use axum::body::{Body, Bytes, to_bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, Uri};

use crate::logging::{BodyPreview, HeaderEntry, ServiceTierLog};
use crate::state::{SessionBinding, SessionIdentitySource};

use super::ProxyService;
use super::client_identity::{
    extract_client_addr, extract_client_name, extract_session_identity,
    extract_session_identity_with_body_fallback,
};
use super::headers::header_map_to_entries;
use super::request_body::{codex_compact_request_requires_affinity, complete_codex_session_fields};
use super::request_encoding::normalize_request_content_encoding;
use super::request_failures::{
    NoRoutableStationParams, log_client_body_read_error, log_no_routable_station,
};
use super::request_preparation::{
    CommonRequestPreparationError, CommonRequestPreparationParams, RequestFlavor,
    codex_path_is_responses_compact, codex_path_is_responses_or_compact, detect_request_flavor,
    load_request_config_context, prepare_common_request, session_identity_source,
    session_identity_value,
};
use super::request_routing::RequestRouteSelection;
use super::retry::RetryPlan;

pub(super) struct PreparedProxyRequest {
    pub(super) method: Method,
    pub(super) uri: Uri,
    pub(super) client_uri: String,
    pub(super) client_headers: HeaderMap,
    pub(super) client_headers_entries_cache: OnceLock<Vec<HeaderEntry>>,
    pub(super) session_id: Option<String>,
    pub(super) session_identity_source: Option<SessionIdentitySource>,
    pub(super) session_binding: Option<SessionBinding>,
    pub(super) route_selection: RequestRouteSelection,
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
    pub(super) cooldown_backoff: crate::lb::CooldownBackoff,
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
    let mut client_headers = parts.headers;
    let client_headers_entries_cache: OnceLock<Vec<HeaderEntry>> = OnceLock::new();

    let header_session_identity = extract_session_identity(&client_headers);
    let client_name = extract_client_name(&client_headers);

    let config = load_request_config_context(proxy).await;
    let mut request_flavor = detect_request_flavor(
        proxy.service_name,
        &method,
        &client_headers,
        uri.path(),
        config.codex_patch_mode,
    );
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
                    session_id: session_identity_value(header_session_identity.as_ref()),
                    session_identity_source: session_identity_source(
                        header_session_identity.as_ref(),
                    ),
                    cwd: None,
                    client_headers: client_headers_entries,
                    duration_ms: dur,
                    error_message: error.to_string(),
                },
            ));
        }
    };
    let raw_body = match normalize_request_content_encoding(&mut client_headers, raw_body) {
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
                    session_id: session_identity_value(header_session_identity.as_ref()),
                    session_identity_source: session_identity_source(
                        header_session_identity.as_ref(),
                    ),
                    cwd: None,
                    client_headers: client_headers_entries,
                    duration_ms: dur,
                    error_message: error.to_string(),
                },
            ));
        }
    };
    let session_identity_hint =
        extract_session_identity_with_body_fallback(&client_headers, raw_body.as_ref());
    let raw_body = if request_flavor.is_codex_service
        && method == Method::POST
        && codex_path_is_responses_or_compact(uri.path())
    {
        complete_codex_session_fields(&mut client_headers, &raw_body).0
    } else {
        raw_body
    };
    if request_flavor.is_remote_compaction_v1_request {
        request_flavor.remote_compaction_requires_affinity =
            codex_compact_request_requires_affinity(raw_body.as_ref());
    }

    let request_body_previews = crate::logging::should_log_request_body_preview();
    let prepared = match prepare_common_request(CommonRequestPreparationParams {
        proxy,
        config: &config,
        method: &method,
        uri: &uri,
        client_headers: &client_headers,
        raw_body: &raw_body,
        compact_request: codex_path_is_responses_compact(uri.path()),
        session_identity_hint,
        client_name,
        client_addr,
        started_at_ms,
        client_content_type: request_flavor.client_content_type.as_deref(),
        request_body_previews,
    })
    .await
    {
        Ok(prepared) => prepared,
        Err(CommonRequestPreparationError::NoRoutableStation {
            session_id,
            session_identity_source,
        }) => {
            let dur = start.elapsed().as_millis() as u64;
            let client_headers_entries = client_headers_entries_cache
                .get_or_init(|| header_map_to_entries(&client_headers))
                .clone();
            return Err(log_no_routable_station(NoRoutableStationParams {
                proxy,
                method: &method,
                path: uri.path(),
                client_uri: client_uri.as_str(),
                session_id,
                session_identity_source,
                client_headers: client_headers_entries,
                duration_ms: dur,
            }));
        }
    };

    if let Some(lbs) = prepared.route_selection.legacy_lbs() {
        super::route_executor_shadow::maybe_log_route_executor_shadow_diff(
            proxy.service_name,
            prepared.request_id,
            lbs,
            prepared.request_model.as_deref(),
        );
    }

    Ok(PreparedProxyRequest {
        method,
        uri,
        client_uri,
        client_headers,
        client_headers_entries_cache,
        session_id: prepared.session_id,
        session_identity_source: prepared.session_identity_source,
        session_binding: prepared.session_binding,
        route_selection: prepared.route_selection,
        cwd: prepared.cwd,
        session_override_config: prepared.session_override_config,
        global_station_override: prepared.global_station_override,
        override_effort: prepared.override_effort,
        override_model: prepared.override_model,
        override_service_tier: prepared.override_service_tier,
        body_for_upstream: prepared.body_for_upstream,
        request_model: prepared.request_model,
        effective_effort: prepared.effective_effort,
        effective_service_tier: prepared.effective_service_tier,
        base_service_tier: prepared.base_service_tier,
        request_body_len: prepared.request_body_len,
        request_flavor,
        request_body_previews: prepared.request_body_previews,
        debug_max: prepared.debug_max,
        warn_max: prepared.warn_max,
        client_body_debug: prepared.client_body_debug,
        client_body_warn: prepared.client_body_warn,
        request_id: prepared.request_id,
        plan: prepared.plan,
        cooldown_backoff: prepared.cooldown_backoff,
    })
}

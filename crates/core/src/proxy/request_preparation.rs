use std::sync::Arc;

use axum::body::Bytes;
use axum::http::{HeaderMap, Method, Uri};

use crate::codex_integration::CodexPatchMode;
use crate::config::{ProxyConfig, ProxyConfigV4};
use crate::lb::CooldownBackoff;
use crate::logging::{BodyPreview, CodexBridgeLog, ServiceTierLog, make_body_preview};
use crate::routing_ir::RouteRequestContext;
use crate::state::{SessionBinding, SessionContinuityMode, SessionIdentitySource};

use super::ProxyService;
use super::client_identity::ClientSessionIdentity;
use super::request_body::{
    apply_model_override_value, apply_reasoning_effort_override_value,
    apply_service_tier_override_value, codex_compact_request_requires_affinity,
    extract_model_from_value, extract_reasoning_effort_from_value, extract_service_tier_from_value,
    normalize_codex_compact_request_value,
};
use super::request_routing::RequestRouteSelection;
use super::retry::{RetryPlan, retry_plan};

#[derive(Debug, Clone)]
pub(super) struct RequestFlavor {
    pub client_content_type: Option<String>,
    pub is_stream: bool,
    pub is_user_turn: bool,
    pub is_remote_compaction_v1_request: bool,
    pub remote_compaction_requires_affinity: bool,
    pub is_codex_service: bool,
    pub codex_client_patch_mode: CodexPatchMode,
    pub codex_bridge_log: Option<CodexBridgeLog>,
}

impl RequestFlavor {
    pub(super) fn with_remote_compaction_affinity_from_body(self, raw_body: &[u8]) -> Self {
        let remote_compaction_requires_affinity = self.is_remote_compaction_v1_request
            && codex_compact_request_requires_affinity(raw_body);
        Self {
            remote_compaction_requires_affinity,
            ..self
        }
    }

    pub(super) fn with_responses_stream_from_body(self, raw_body: &[u8]) -> Self {
        let body_requests_stream = self.is_codex_service
            && self.is_user_turn
            && crate::proxy::request_body::codex_responses_body_requests_stream(raw_body);
        Self {
            is_stream: self.is_stream || body_requests_stream,
            ..self
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct PreparedRequestBody {
    pub body_for_upstream: Bytes,
    pub request_model: Option<String>,
    pub effective_effort: Option<String>,
    pub base_service_tier: ServiceTierLog,
    pub request_body_len: usize,
}

#[derive(Debug, Clone, Default)]
pub(super) struct BodyPreviewSet {
    pub debug: Option<BodyPreview>,
    pub warn: Option<BodyPreview>,
}

#[derive(Debug, Clone)]
pub(super) struct RequestConfigContext {
    pub(super) cfg_snapshot: Arc<ProxyConfig>,
    pub(super) v4_snapshot: Option<Arc<ProxyConfigV4>>,
    pub(super) route_graph_config: bool,
    pub(super) codex_patch_mode: CodexPatchMode,
}

pub(super) struct CommonPreparedRequest {
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
    pub(super) request_body_previews: bool,
    pub(super) debug_max: usize,
    pub(super) warn_max: usize,
    pub(super) client_body_debug: Option<BodyPreview>,
    pub(super) client_body_warn: Option<BodyPreview>,
    pub(super) request_id: u64,
    pub(super) plan: RetryPlan,
    pub(super) cooldown_backoff: CooldownBackoff,
}

#[derive(Debug, Clone)]
pub(super) enum CommonRequestPreparationError {
    NoRoutableStation {
        session_id: Option<String>,
        session_identity_source: Option<SessionIdentitySource>,
    },
}

pub(super) struct CommonRequestPreparationParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) config: &'a RequestConfigContext,
    pub(super) method: &'a Method,
    pub(super) uri: &'a Uri,
    pub(super) client_headers: &'a HeaderMap,
    pub(super) raw_body: &'a Bytes,
    pub(super) compact_request: bool,
    pub(super) session_identity_hint: Option<ClientSessionIdentity>,
    pub(super) client_name: Option<String>,
    pub(super) client_addr: Option<String>,
    pub(super) started_at_ms: u64,
    pub(super) client_content_type: Option<&'a str>,
    pub(super) request_body_previews: bool,
}

fn codex_patch_mode_for_proxy(proxy: &ProxyService) -> CodexPatchMode {
    if proxy.service_name != "codex" {
        return CodexPatchMode::Default;
    }
    crate::codex_integration::codex_switch_status()
        .ok()
        .and_then(|status| status.patch_mode)
        .unwrap_or(CodexPatchMode::Default)
}

pub(super) async fn load_request_config_context(proxy: &ProxyService) -> RequestConfigContext {
    let config_reloaded = proxy.config.maybe_reload_from_disk().await;
    let cfg_snapshot = proxy.config.snapshot().await;
    let v4_snapshot = proxy.config.v4_snapshot().await;
    let route_graph_config = v4_snapshot.is_some();
    let mgr = proxy.service_manager(cfg_snapshot.as_ref());
    if config_reloaded {
        proxy
            .state
            .prune_runtime_observability_for_service(proxy.service_name, mgr)
            .await;
    }

    RequestConfigContext {
        cfg_snapshot,
        v4_snapshot,
        route_graph_config,
        codex_patch_mode: codex_patch_mode_for_proxy(proxy),
    }
}

pub(super) async fn prepare_common_request(
    params: CommonRequestPreparationParams<'_>,
) -> Result<CommonPreparedRequest, CommonRequestPreparationError> {
    let CommonRequestPreparationParams {
        proxy,
        config,
        method,
        uri,
        client_headers,
        raw_body,
        compact_request,
        session_identity_hint,
        client_name,
        client_addr,
        started_at_ms,
        client_content_type,
        request_body_previews,
    } = params;
    let mgr = proxy.service_manager(config.cfg_snapshot.as_ref());
    let _ = client_headers;
    let session_identity = session_identity_hint;
    let session_id = session_identity_value(session_identity.as_ref());
    let session_identity_source = session_identity_source(session_identity.as_ref());
    let session_binding = if let Some(id) = session_id.as_deref() {
        proxy
            .ensure_default_session_binding(mgr, id, started_at_ms)
            .await
    } else {
        None
    };
    let cwd = resolve_and_touch_session_state(proxy, session_id.as_deref(), started_at_ms).await;
    let session_override_config = if !config.route_graph_config
        && let Some(id) = session_id.as_deref()
    {
        proxy.state.get_session_station_override(id).await
    } else {
        None
    };
    let global_station_override = if config.route_graph_config {
        None
    } else {
        proxy.state.get_global_station_override().await
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
    let binding_effort = binding_reasoning_effort_for_request(session_binding.as_ref());
    let binding_model = binding_model_for_request(session_binding.as_ref());
    let binding_service_tier = binding_service_tier_for_request(session_binding.as_ref());

    let prepared_request = prepare_request_body(PrepareRequestBodyParams {
        raw_body,
        compact_request,
        override_effort: override_effort.as_deref(),
        binding_effort,
        override_model: override_model.as_deref(),
        binding_model,
        override_service_tier: override_service_tier.as_deref(),
        binding_service_tier,
    });
    let body_for_upstream = prepared_request.body_for_upstream.clone();
    let request_model = prepared_request.request_model.clone();
    let effective_effort = prepared_request.effective_effort.clone();
    let effective_service_tier = prepared_request.base_service_tier.effective.clone();
    let base_service_tier = prepared_request.base_service_tier.clone();
    let request_body_len = prepared_request.request_body_len;

    let debug_opt = crate::logging::http_debug_options();
    let warn_opt = crate::logging::http_warn_options();
    let debug_max = if request_body_previews && debug_opt.enabled {
        debug_opt.max_body_bytes
    } else {
        0
    };
    let warn_max = if request_body_previews && warn_opt.enabled {
        warn_opt.max_body_bytes
    } else {
        0
    };
    let client_body_previews = build_body_previews(
        raw_body,
        client_content_type,
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
            session_identity_source,
            client_name,
            client_addr,
            cwd.clone(),
            request_model.clone(),
            effective_effort.clone(),
            effective_service_tier.clone(),
            started_at_ms,
        )
        .await;

    let plan = retry_plan(&config.cfg_snapshot.retry.resolve());
    let cooldown_backoff = CooldownBackoff {
        factor: plan.cooldown_backoff_factor,
        max_secs: plan.cooldown_backoff_max_secs,
    };

    let route_request = route_request_context(
        method,
        uri,
        client_headers,
        request_model.clone(),
        effective_effort.clone(),
        effective_service_tier.clone(),
    );
    let route_selection = proxy
        .lbs_for_request(
            config.cfg_snapshot.as_ref(),
            config.v4_snapshot.as_deref(),
            &route_request,
            session_id.as_deref(),
        )
        .await;
    if route_selection.is_empty() {
        return Err(CommonRequestPreparationError::NoRoutableStation {
            session_id,
            session_identity_source,
        });
    }

    Ok(CommonPreparedRequest {
        session_id,
        session_identity_source,
        session_binding,
        route_selection,
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

pub(super) fn detect_request_flavor(
    service_name: &str,
    method: &Method,
    headers: &HeaderMap,
    path: &str,
    codex_client_patch_mode: CodexPatchMode,
) -> RequestFlavor {
    let client_content_type = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    let is_stream = headers
        .get("accept")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_ascii_lowercase().contains("text/event-stream"))
        .unwrap_or(false);

    let is_responses_path = codex_path_is_responses(path);
    let is_remote_compaction_v1_request = codex_path_is_responses_compact(path);
    let is_user_turn = *method == Method::POST && is_responses_path;
    let is_codex_service = service_name == "codex";
    let codex_bridge_log = (is_codex_service
        && (!codex_client_patch_mode.is_default() || is_remote_compaction_v1_request))
        .then(|| CodexBridgeLog {
            patch_mode: codex_client_patch_mode.as_str().to_string(),
            remote_compaction_v1_request: is_remote_compaction_v1_request,
            responses_websocket_request: false,
            strips_client_auth: codex_client_patch_mode.strips_codex_client_auth(),
        });

    RequestFlavor {
        client_content_type,
        is_stream,
        is_user_turn,
        is_remote_compaction_v1_request,
        remote_compaction_requires_affinity: false,
        is_codex_service,
        codex_client_patch_mode,
        codex_bridge_log,
    }
}

pub(super) fn codex_path_is_responses(path: &str) -> bool {
    path.trim_end_matches('/').ends_with("/responses")
}

pub(super) fn codex_path_is_responses_compact(path: &str) -> bool {
    path.trim_end_matches('/').ends_with("/responses/compact")
}

pub(super) fn codex_path_is_responses_or_compact(path: &str) -> bool {
    codex_path_is_responses(path) || codex_path_is_responses_compact(path)
}

pub(super) fn session_identity_value(identity: Option<&ClientSessionIdentity>) -> Option<String> {
    identity.map(|identity| identity.value().to_string())
}

pub(super) fn session_identity_source(
    identity: Option<&ClientSessionIdentity>,
) -> Option<SessionIdentitySource> {
    identity.map(|identity| identity.source())
}

pub(super) fn route_request_context(
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    model: Option<String>,
    reasoning_effort: Option<String>,
    service_tier: Option<String>,
) -> RouteRequestContext {
    RouteRequestContext {
        model,
        service_tier,
        reasoning_effort,
        path: Some(uri.path().to_string()),
        method: Some(method.as_str().to_string()),
        headers: headers
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_string(), value.to_string()))
            })
            .collect(),
    }
}

fn binding_for_request_body_overrides(binding: Option<&SessionBinding>) -> Option<&SessionBinding> {
    let binding = binding?;
    if binding.continuity_mode != SessionContinuityMode::ManualProfile {
        return None;
    }
    Some(binding)
}

fn binding_reasoning_effort_for_request(binding: Option<&SessionBinding>) -> Option<&str> {
    binding_for_request_body_overrides(binding)
        .and_then(|binding| binding.reasoning_effort.as_deref())
}

fn binding_model_for_request(binding: Option<&SessionBinding>) -> Option<&str> {
    binding_for_request_body_overrides(binding).and_then(|binding| binding.model.as_deref())
}

fn binding_service_tier_for_request(binding: Option<&SessionBinding>) -> Option<&str> {
    let binding = binding_for_request_body_overrides(binding)?;
    normalize_profile_service_tier(binding.service_tier.as_deref())
}

fn normalize_profile_service_tier(value: Option<&str>) -> Option<&str> {
    let value = value?.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("auto") {
        None
    } else if value.eq_ignore_ascii_case("fast") {
        Some("priority")
    } else {
        Some(value)
    }
}

pub(super) async fn resolve_and_touch_session_state(
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
            .touch_session_route_target_override(id, started_at_ms)
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

pub(super) struct PrepareRequestBodyParams<'a> {
    raw_body: &'a Bytes,
    compact_request: bool,
    override_effort: Option<&'a str>,
    binding_effort: Option<&'a str>,
    override_model: Option<&'a str>,
    binding_model: Option<&'a str>,
    override_service_tier: Option<&'a str>,
    binding_service_tier: Option<&'a str>,
}

pub(super) fn prepare_request_body(params: PrepareRequestBodyParams<'_>) -> PreparedRequestBody {
    let PrepareRequestBodyParams {
        raw_body,
        compact_request,
        override_effort,
        binding_effort,
        override_model,
        binding_model,
        override_service_tier,
        binding_service_tier,
    } = params;
    let mut request_json = serde_json::from_slice::<serde_json::Value>(raw_body).ok();
    let original_effort = request_json
        .as_ref()
        .and_then(extract_reasoning_effort_from_value);
    let original_service_tier = request_json
        .as_ref()
        .and_then(extract_service_tier_from_value);

    let is_object_root = request_json
        .as_ref()
        .is_some_and(serde_json::Value::is_object);
    if is_object_root && let Some(value) = request_json.as_mut() {
        if let Some(effort) = override_effort.or(binding_effort) {
            apply_reasoning_effort_override_value(value, effort);
        }
        if let Some(model) = override_model.or(binding_model) {
            apply_model_override_value(value, model);
        }
        if let Some(service_tier) = override_service_tier.or(binding_service_tier) {
            apply_service_tier_override_value(value, service_tier);
        }
        if compact_request {
            normalize_codex_compact_request_value(value);
        }
    }

    let body_for_upstream = if is_object_root {
        request_json
            .as_ref()
            .and_then(|value| serde_json::to_vec(value).ok())
            .map(Bytes::from)
            .unwrap_or_else(|| raw_body.clone())
    } else {
        raw_body.clone()
    };

    let request_model = request_json.as_ref().and_then(extract_model_from_value);
    let effective_effort = request_json
        .as_ref()
        .and_then(extract_reasoning_effort_from_value)
        .or(original_effort);
    let effective_service_tier = request_json
        .as_ref()
        .and_then(extract_service_tier_from_value)
        .or(original_service_tier.clone());

    PreparedRequestBody {
        request_body_len: raw_body.len(),
        request_model,
        effective_effort,
        base_service_tier: ServiceTierLog {
            requested: original_service_tier,
            effective: effective_service_tier,
            actual: None,
        },
        body_for_upstream,
    }
}

pub(super) fn build_body_previews(
    body: &[u8],
    content_type: Option<&str>,
    previews_enabled: bool,
    debug_max: usize,
    warn_max: usize,
) -> BodyPreviewSet {
    if !previews_enabled {
        return BodyPreviewSet::default();
    }

    BodyPreviewSet {
        debug: (debug_max > 0).then(|| make_body_preview(body, content_type, debug_max)),
        warn: (warn_max > 0).then(|| make_body_preview(body, content_type, warn_max)),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use axum::http::{HeaderMap, HeaderValue};

    use super::*;
    use crate::config::{
        ProxyConfig, ServiceConfig, ServiceControlProfile, UpstreamAuth, UpstreamConfig,
    };
    use crate::lb::LbState;

    #[test]
    fn detect_request_flavor_reads_stream_and_turn_shape() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "accept",
            HeaderValue::from_static("Text/Event-Stream, application/json"),
        );
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        let flavor = detect_request_flavor(
            "codex",
            &Method::POST,
            &headers,
            "/v1/responses",
            CodexPatchMode::Default,
        );

        assert_eq!(
            flavor.client_content_type.as_deref(),
            Some("application/json")
        );
        assert!(flavor.is_stream);
        assert!(flavor.is_user_turn);
        assert!(!flavor.is_remote_compaction_v1_request);
        assert!(flavor.is_codex_service);
    }

    #[test]
    fn detect_request_flavor_marks_codex_bridge_compact_request() {
        let headers = HeaderMap::new();

        let flavor = detect_request_flavor(
            "codex",
            &Method::POST,
            &headers,
            "/v1/responses/compact",
            CodexPatchMode::OfficialImagegenBridge,
        );

        assert!(!flavor.is_user_turn);
        assert!(flavor.is_remote_compaction_v1_request);
        let bridge = flavor.codex_bridge_log.expect("bridge log");
        assert_eq!(bridge.patch_mode, "official-imagegen-bridge");
        assert!(bridge.remote_compaction_v1_request);
        assert!(bridge.strips_client_auth);
    }

    #[test]
    fn detect_request_flavor_marks_trailing_slash_compact_request() {
        let headers = HeaderMap::new();

        let flavor = detect_request_flavor(
            "codex",
            &Method::POST,
            &headers,
            "/v1/responses/compact/",
            CodexPatchMode::OfficialRelayBridge,
        );

        assert!(flavor.is_remote_compaction_v1_request);
        assert!(
            flavor
                .codex_bridge_log
                .expect("bridge log")
                .remote_compaction_v1_request
        );
    }

    #[test]
    fn request_flavor_finalizes_remote_compaction_affinity_from_body() {
        let headers = HeaderMap::new();

        let flavor = detect_request_flavor(
            "codex",
            &Method::POST,
            &headers,
            "/v1/responses/compact",
            CodexPatchMode::Default,
        )
        .with_remote_compaction_affinity_from_body(
            br#"{"input":[{"type":"reasoning","encrypted_content":"state"}]}"#,
        );

        assert!(flavor.remote_compaction_requires_affinity);
    }

    #[test]
    fn prepare_request_body_prefers_manual_overrides_over_binding_defaults() {
        let raw_body = Bytes::from_static(
            br#"{"model":"gpt-5","service_tier":"priority","reasoning":{"effort":"low"}}"#,
        );

        let prepared = prepare_request_body(PrepareRequestBodyParams {
            raw_body: &raw_body,
            compact_request: false,
            override_effort: Some("high"),
            binding_effort: Some("medium"),
            override_model: Some("gpt-5.4"),
            binding_model: Some("gpt-5-mini"),
            override_service_tier: Some("flex"),
            binding_service_tier: Some("default"),
        });

        assert_eq!(prepared.request_model.as_deref(), Some("gpt-5.4"));
        assert_eq!(prepared.effective_effort.as_deref(), Some("high"));
        assert_eq!(
            prepared.base_service_tier.requested.as_deref(),
            Some("priority")
        );
        assert_eq!(
            prepared.base_service_tier.effective.as_deref(),
            Some("flex")
        );
        assert_eq!(prepared.request_body_len, raw_body.len());
    }

    #[test]
    fn default_profile_binding_fields_are_not_request_overrides() {
        let binding = SessionBinding {
            session_id: "sid-fast".to_string(),
            profile_name: Some("daily".to_string()),
            station_name: Some("test".to_string()),
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("low".to_string()),
            service_tier: Some("default".to_string()),
            continuity_mode: SessionContinuityMode::DefaultProfile,
            created_at_ms: 1,
            updated_at_ms: 1,
            last_seen_ms: 1,
        };

        assert_eq!(binding_model_for_request(Some(&binding)), None);
        assert_eq!(binding_reasoning_effort_for_request(Some(&binding)), None);
        assert_eq!(binding_service_tier_for_request(Some(&binding)), None);
    }

    #[test]
    fn manual_profile_binding_fields_are_request_overrides() {
        let binding = SessionBinding {
            session_id: "sid-fast".to_string(),
            profile_name: Some("fast".to_string()),
            station_name: Some("test".to_string()),
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("low".to_string()),
            service_tier: Some("fast".to_string()),
            continuity_mode: SessionContinuityMode::ManualProfile,
            created_at_ms: 1,
            updated_at_ms: 1,
            last_seen_ms: 1,
        };

        assert_eq!(binding_model_for_request(Some(&binding)), Some("gpt-5.4"));
        assert_eq!(
            binding_reasoning_effort_for_request(Some(&binding)),
            Some("low")
        );
        assert_eq!(
            binding_service_tier_for_request(Some(&binding)),
            Some("priority")
        );
    }

    #[test]
    fn build_body_previews_respects_enable_flag_and_limits() {
        let previews = build_body_previews(
            br#"{"input":"hello"}"#,
            Some("application/json"),
            true,
            32,
            16,
        );
        assert!(previews.debug.is_some());
        assert!(previews.warn.is_some());

        let disabled = build_body_previews(
            br#"{"input":"hello"}"#,
            Some("application/json"),
            false,
            32,
            16,
        );
        assert!(disabled.debug.is_none());
        assert!(disabled.warn.is_none());
    }

    fn test_proxy_with_active_station() -> ProxyService {
        let mut cfg = ProxyConfig::default();
        cfg.codex.active = Some("test".to_string());
        cfg.codex.default_profile = Some("default".to_string());
        cfg.codex.profiles.insert(
            "default".to_string(),
            ServiceControlProfile {
                model: Some("gpt-5.4".to_string()),
                ..ServiceControlProfile::default()
            },
        );
        cfg.codex.configs.insert(
            "test".to_string(),
            ServiceConfig {
                name: "test".to_string(),
                alias: None,
                enabled: true,
                level: 1,
                upstreams: vec![UpstreamConfig {
                    base_url: "https://example.com/v1".to_string(),
                    auth: UpstreamAuth::default(),
                    tags: HashMap::new(),
                    supported_models: HashMap::new(),
                    model_mapping: HashMap::new(),
                }],
            },
        );
        ProxyService::new(
            reqwest::Client::new(),
            Arc::new(cfg),
            "codex",
            Arc::new(Mutex::new(HashMap::<String, LbState>::new())),
        )
    }

    #[tokio::test]
    async fn prepare_common_request_tracks_prompt_cache_identity_without_default_profile_patch() {
        let proxy = test_proxy_with_active_station();
        let config = load_request_config_context(&proxy).await;
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let method = Method::POST;
        let uri = "/v1/responses".parse::<Uri>().expect("uri");
        let raw_body =
            Bytes::from_static(br#"{"model":"gpt-5","prompt_cache_key":"pcache-shared"}"#);

        let prepared = prepare_common_request(CommonRequestPreparationParams {
            proxy: &proxy,
            config: &config,
            method: &method,
            uri: &uri,
            client_headers: &headers,
            raw_body: &raw_body,
            compact_request: false,
            session_identity_hint:
                super::super::client_identity::extract_session_identity_with_body_fallback(
                    &headers,
                    raw_body.as_ref(),
                ),
            client_name: Some("test-client".to_string()),
            client_addr: None,
            started_at_ms: 2,
            client_content_type: Some("application/json"),
            request_body_previews: false,
        })
        .await
        .expect("prepared");

        assert_eq!(prepared.session_id.as_deref(), Some("pcache-shared"));
        assert_eq!(
            prepared.session_identity_source,
            Some(SessionIdentitySource::PromptCacheKey)
        );
        assert_eq!(prepared.request_model.as_deref(), Some("gpt-5"));
        assert_eq!(
            prepared
                .session_binding
                .as_ref()
                .and_then(|binding| binding.profile_name.as_deref()),
            Some("default")
        );
        assert_eq!(
            prepared
                .session_binding
                .as_ref()
                .map(|binding| binding.continuity_mode),
            Some(SessionContinuityMode::DefaultProfile)
        );
        assert!(String::from_utf8_lossy(prepared.body_for_upstream.as_ref()).contains("\"gpt-5\""));
        assert!(
            !String::from_utf8_lossy(prepared.body_for_upstream.as_ref()).contains("\"gpt-5.4\"")
        );
        assert!(!prepared.request_body_previews);
        assert!(!prepared.route_selection.is_empty());

        let active = proxy.state.list_active_requests().await;
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].session_id.as_deref(), Some("pcache-shared"));
        assert_eq!(
            active[0].session_identity_source,
            Some(SessionIdentitySource::PromptCacheKey)
        );
    }
}

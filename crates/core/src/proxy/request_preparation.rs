use std::sync::Arc;

use axum::body::Bytes;
use axum::http::{HeaderMap, Method, Uri};

use crate::endpoint_health::CooldownBackoff;
use crate::logging::{BodyPreview, CodexBridgeLog, ServiceTierLog, make_body_preview};
use crate::routing_ir::RouteRequestContext;
use crate::state::{SessionBinding, SessionContinuityMode, SessionIdentitySource};

use super::ProxyService;
use super::client_identity::ClientSessionIdentity;
use super::request_body::{
    ReasoningOrchestrationIntent, RequestDialect, apply_model_override_value,
    apply_reasoning_effort_override_value, apply_service_tier_override_value,
    extract_model_from_value, extract_reasoning_effort_from_value, extract_service_tier_from_value,
    normalize_codex_compact_request_value, remove_reasoning_effort_value,
};
use super::request_continuity::{
    RequestContinuityClassificationInput, RequestTransport, classify_request_continuity,
};
use super::retry::{RetryPlan, retry_plan};
use super::runtime_config::{CapturedRoutePlan, RuntimeSnapshot};

#[derive(Debug, Clone)]
pub(super) struct RequestFlavor {
    pub client_content_type: Option<String>,
    pub is_stream: bool,
    pub is_user_turn: bool,
    pub is_remote_compaction_v1_request: bool,
    pub is_remote_compaction_v2_request: bool,
    pub remote_compaction_requires_affinity: bool,
    pub is_codex_service: bool,
    pub codex_bridge_log: Option<CodexBridgeLog>,
}

impl RequestFlavor {
    pub(super) fn is_remote_compaction_request(&self) -> bool {
        self.is_remote_compaction_v1_request || self.is_remote_compaction_v2_request
    }

    pub(super) fn with_remote_compaction_context_from_body(mut self, raw_body: &[u8]) -> Self {
        let continuity = classify_request_continuity(RequestContinuityClassificationInput {
            transport: RequestTransport::Http,
            is_codex_service: self.is_codex_service,
            is_user_turn: self.is_user_turn,
            is_remote_compaction_v1_request: self.is_remote_compaction_v1_request,
            raw_body,
        });
        self.is_remote_compaction_v1_request = continuity.is_remote_compaction_v1_request;
        self.is_remote_compaction_v2_request = continuity.is_remote_compaction_v2_request;
        self.remote_compaction_requires_affinity = continuity.remote_compaction_requires_affinity;
        if continuity.is_remote_compaction_v2_request {
            let bridge = self.codex_bridge_log.get_or_insert(CodexBridgeLog {
                patch_mode: "request-dialect".to_string(),
                remote_compaction_v1_request: false,
                remote_compaction_v2_request: false,
                responses_websocket_request: false,
                strips_client_auth: false,
            });
            bridge.remote_compaction_v2_request = true;
        }
        self
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
    pub requested_model: Option<String>,
    pub effective_effort: Option<String>,
    pub deferred_reasoning_intent: Option<ReasoningOrchestrationIntent>,
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
    pub(super) runtime_snapshot: Arc<RuntimeSnapshot>,
}

pub(super) struct CommonPreparedRequest {
    pub(super) session_id: Option<String>,
    pub(super) session_identity_source: Option<SessionIdentitySource>,
    pub(super) session_binding: Option<SessionBinding>,
    pub(super) route_plan: Option<CapturedRoutePlan>,
    pub(super) cwd: Option<String>,
    pub(super) body_for_upstream: Bytes,
    pub(super) request_dialect: RequestDialect,
    pub(super) request_model: Option<String>,
    pub(super) effective_effort: Option<String>,
    pub(super) deferred_reasoning_intent: Option<ReasoningOrchestrationIntent>,
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
    LifecycleStoreUnavailable {
        message: String,
    },
    NoRoutableCandidate {
        request_id: u64,
        session_id: Option<String>,
        session_identity_source: Option<SessionIdentitySource>,
        cwd: Option<String>,
        effective_effort: Option<String>,
        service_tier: ServiceTierLog,
    },
}

pub(super) struct CommonRequestPreparationParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) config: &'a RequestConfigContext,
    pub(super) method: &'a Method,
    pub(super) uri: &'a Uri,
    pub(super) client_headers: &'a HeaderMap,
    pub(super) raw_body: &'a Bytes,
    pub(super) request_dialect: RequestDialect,
    pub(super) session_identity_hint: Option<ClientSessionIdentity>,
    pub(super) client_name: Option<String>,
    pub(super) client_addr: Option<String>,
    pub(super) started_at_ms: u64,
    pub(super) client_content_type: Option<&'a str>,
    pub(super) request_body_previews: bool,
}

pub(super) async fn load_request_config_context(proxy: &ProxyService) -> RequestConfigContext {
    let config_reloaded = proxy.config.maybe_reload_from_disk().await;
    let runtime_snapshot = proxy.config.capture().await;
    if config_reloaded {
        super::control_plane_service::prune_runtime_observability_after_reload(proxy).await;
    }

    RequestConfigContext { runtime_snapshot }
}

pub(super) async fn prepare_common_request(
    params: CommonRequestPreparationParams<'_>,
) -> Result<CommonPreparedRequest, CommonRequestPreparationError> {
    prepare_common_request_inner(params, true).await
}

pub(super) async fn prepare_common_request_without_route_plan(
    params: CommonRequestPreparationParams<'_>,
) -> Result<CommonPreparedRequest, CommonRequestPreparationError> {
    prepare_common_request_inner(params, false).await
}

async fn prepare_common_request_inner(
    params: CommonRequestPreparationParams<'_>,
    select_route: bool,
) -> Result<CommonPreparedRequest, CommonRequestPreparationError> {
    let CommonRequestPreparationParams {
        proxy,
        config,
        method,
        uri,
        client_headers,
        raw_body,
        request_dialect,
        session_identity_hint,
        client_name,
        client_addr,
        started_at_ms,
        client_content_type,
        request_body_previews,
    } = params;
    let config_snapshot = config.runtime_snapshot.config();
    let view = super::control_plane_service::service_route_config(
        config_snapshot.as_ref(),
        proxy.service_name,
    );
    let _ = client_headers;
    let session_identity = session_identity_hint;
    let session_id = session_identity_value(session_identity.as_ref());
    let session_identity_source = session_identity_source(session_identity.as_ref());
    let session_binding = if let Some(id) = session_id.as_deref() {
        proxy
            .ensure_default_session_binding(view, id, started_at_ms)
            .await
    } else {
        None
    };
    touch_session_state(proxy, session_id.as_deref(), started_at_ms).await;
    let cwd = None;

    let binding_effort = binding_reasoning_effort_for_request(session_binding.as_ref());
    let binding_model = binding_model_for_request(session_binding.as_ref());
    let binding_service_tier = binding_service_tier_for_request(session_binding.as_ref());
    let prepared_request = prepare_request_body(PrepareRequestBodyParams {
        raw_body,
        dialect: request_dialect,
        binding_effort,
        binding_model,
        binding_service_tier,
    });
    let body_for_upstream = prepared_request.body_for_upstream.clone();
    let request_model = prepared_request.request_model.clone();
    let effective_effort = prepared_request.effective_effort.clone();
    let deferred_reasoning_intent = prepared_request.deferred_reasoning_intent;
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
        .try_begin_request(
            proxy.service_name,
            method.as_str(),
            uri.path(),
            session_id.clone(),
            session_identity_source,
            client_name,
            client_addr,
            cwd.clone(),
            request_model.clone(),
            prepared_request.requested_model.clone(),
            effective_effort.clone(),
            effective_service_tier.clone(),
            base_service_tier.requested.clone(),
            config.runtime_snapshot.provider_catalog(),
            config.runtime_snapshot.operator_pricing_catalog(),
            config.runtime_snapshot.revision(),
            config.runtime_snapshot.digest().to_string(),
            config.runtime_snapshot.provider_policy().policy_revision,
            started_at_ms,
        )
        .await
        .map_err(
            |error| CommonRequestPreparationError::LifecycleStoreUnavailable {
                message: error.to_string(),
            },
        )?;

    let plan = retry_plan(&config_snapshot.retry.resolve());
    let cooldown_backoff = CooldownBackoff {
        factor: plan.cooldown_backoff_factor,
        max_secs: plan.cooldown_backoff_max_secs,
    };

    let route_plan = if select_route {
        let route_request = route_request_context(
            method,
            uri,
            client_headers,
            request_model.clone(),
            effective_effort.clone(),
            effective_service_tier.clone(),
        );
        match config
            .runtime_snapshot
            .capture_route_plan(proxy.service_name, &route_request)
        {
            Ok(route_plan) => route_plan,
            Err(error) => {
                crate::logging::log_control_trace_event(serde_json::json!({
                    "event": "route_plan_selection_failed",
                    "service": proxy.service_name,
                    "runtime_revision": config.runtime_snapshot.revision(),
                    "error": error.to_string(),
                }));
                None
            }
        }
    } else {
        None
    };
    if select_route && route_plan.as_ref().is_none_or(CapturedRoutePlan::is_empty) {
        return Err(CommonRequestPreparationError::NoRoutableCandidate {
            request_id,
            session_id,
            session_identity_source,
            cwd,
            effective_effort,
            service_tier: base_service_tier,
        });
    }

    Ok(CommonPreparedRequest {
        session_id,
        session_identity_source,
        session_binding,
        route_plan,
        cwd,
        body_for_upstream,
        request_dialect,
        request_model,
        effective_effort,
        deferred_reasoning_intent,
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
    let codex_bridge_log =
        (is_codex_service && is_remote_compaction_v1_request).then(|| CodexBridgeLog {
            patch_mode: "request-dialect".to_string(),
            remote_compaction_v1_request: is_remote_compaction_v1_request,
            remote_compaction_v2_request: false,
            responses_websocket_request: false,
            strips_client_auth: false,
        });

    RequestFlavor {
        client_content_type,
        is_stream,
        is_user_turn,
        is_remote_compaction_v1_request,
        is_remote_compaction_v2_request: false,
        remote_compaction_requires_affinity: false,
        is_codex_service,
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

pub(super) async fn touch_session_state(
    proxy: &ProxyService,
    session_id: Option<&str>,
    started_at_ms: u64,
) {
    if let Some(id) = session_id {
        proxy.state.touch_session_binding(id, started_at_ms).await;
    }
}

pub(super) struct PrepareRequestBodyParams<'a> {
    raw_body: &'a Bytes,
    dialect: RequestDialect,
    binding_effort: Option<&'a str>,
    binding_model: Option<&'a str>,
    binding_service_tier: Option<&'a str>,
}

pub(super) fn prepare_request_body(params: PrepareRequestBodyParams<'_>) -> PreparedRequestBody {
    let PrepareRequestBodyParams {
        raw_body,
        dialect,
        binding_effort,
        binding_model,
        binding_service_tier,
    } = params;
    let mut request_json = serde_json::from_slice::<serde_json::Value>(raw_body).ok();
    let original_effort = request_json
        .as_ref()
        .and_then(|value| extract_reasoning_effort_from_value(value, dialect));
    let original_model = request_json.as_ref().and_then(extract_model_from_value);
    let original_service_tier = request_json
        .as_ref()
        .and_then(extract_service_tier_from_value);

    let is_object_root = request_json
        .as_ref()
        .is_some_and(serde_json::Value::is_object);
    let selected_effort = if dialect.supports_reasoning_effort() {
        binding_effort
    } else {
        None
    };
    let requested_effort = selected_effort
        .map(str::to_owned)
        .or_else(|| original_effort.clone());
    let deferred_reasoning_intent = requested_effort
        .as_deref()
        .filter(|effort| effort.eq_ignore_ascii_case("ultra"))
        .map(|_| ReasoningOrchestrationIntent::Ultra);
    let effective_effort = if deferred_reasoning_intent.is_none() {
        requested_effort
    } else {
        None
    };
    if is_object_root && let Some(value) = request_json.as_mut() {
        if deferred_reasoning_intent.is_some() {
            remove_reasoning_effort_value(value, dialect);
        } else if let Some(effort) = selected_effort {
            apply_reasoning_effort_override_value(value, dialect, effort);
        }
        if let Some(model) = binding_model {
            apply_model_override_value(value, model);
        }
        if let Some(service_tier) = binding_service_tier {
            apply_service_tier_override_value(value, service_tier);
        }
        if dialect == RequestDialect::ResponsesCompact {
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
    let effective_service_tier = request_json
        .as_ref()
        .and_then(extract_service_tier_from_value)
        .or(original_service_tier.clone());

    PreparedRequestBody {
        request_body_len: raw_body.len(),
        request_model,
        requested_model: original_model,
        effective_effort,
        deferred_reasoning_intent,
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
    use std::path::PathBuf;
    use std::sync::Arc;

    use axum::http::{HeaderMap, HeaderValue};

    use super::*;
    use crate::config::{
        HelperConfig, ProviderConfig, RouteGraphConfig, ServiceControlProfile, ServiceRouteConfig,
    };

    struct ScopedCodexHome {
        previous: Option<std::ffi::OsString>,
        path: PathBuf,
    }

    impl ScopedCodexHome {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "codex-helper-no-host-session-read-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&path).expect("create temporary Codex home");
            let previous = std::env::var_os("CODEX_HOME");
            unsafe {
                std::env::set_var("CODEX_HOME", &path);
            }
            Self { previous, path }
        }
    }

    impl Drop for ScopedCodexHome {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(previous) => unsafe {
                    std::env::set_var("CODEX_HOME", previous);
                },
                None => unsafe {
                    std::env::remove_var("CODEX_HOME");
                },
            }
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn detect_request_flavor_reads_stream_and_turn_shape() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "accept",
            HeaderValue::from_static("Text/Event-Stream, application/json"),
        );
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        let flavor = detect_request_flavor("codex", &Method::POST, &headers, "/v1/responses");

        assert_eq!(
            flavor.client_content_type.as_deref(),
            Some("application/json")
        );
        assert!(flavor.is_stream);
        assert!(flavor.is_user_turn);
        assert!(!flavor.is_remote_compaction_v1_request);
        assert!(!flavor.is_remote_compaction_v2_request);
        assert!(!flavor.is_remote_compaction_request());
        assert!(flavor.is_codex_service);
    }

    #[test]
    fn detect_request_flavor_marks_codex_bridge_compact_request() {
        let headers = HeaderMap::new();

        let flavor =
            detect_request_flavor("codex", &Method::POST, &headers, "/v1/responses/compact");

        assert!(!flavor.is_user_turn);
        assert!(flavor.is_remote_compaction_v1_request);
        let bridge = flavor.codex_bridge_log.expect("bridge log");
        assert_eq!(bridge.patch_mode, "request-dialect");
        assert!(bridge.remote_compaction_v1_request);
        assert!(!bridge.remote_compaction_v2_request);
        assert!(!bridge.strips_client_auth);
    }

    #[test]
    fn detect_request_flavor_marks_trailing_slash_compact_request() {
        let headers = HeaderMap::new();

        let flavor =
            detect_request_flavor("codex", &Method::POST, &headers, "/v1/responses/compact/");

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

        let flavor =
            detect_request_flavor("codex", &Method::POST, &headers, "/v1/responses/compact")
                .with_remote_compaction_context_from_body(
                    br#"{"input":[{"type":"reasoning","encrypted_content":"state"}]}"#,
                );

        assert!(flavor.remote_compaction_requires_affinity);
    }

    #[test]
    fn request_flavor_finalizes_remote_compaction_v2_from_body() {
        let headers = HeaderMap::new();

        let flavor = detect_request_flavor("codex", &Method::POST, &headers, "/v1/responses")
            .with_remote_compaction_context_from_body(
                br#"{"input":[{"type":"message"},{"type":"compaction_trigger"}],"stream":true}"#,
            );

        assert!(flavor.is_user_turn);
        assert!(!flavor.is_remote_compaction_v1_request);
        assert!(flavor.is_remote_compaction_v2_request);
        assert!(flavor.is_remote_compaction_request());
        assert!(flavor.remote_compaction_requires_affinity);
        let bridge = flavor.codex_bridge_log.expect("bridge log");
        assert_eq!(bridge.patch_mode, "request-dialect");
        assert!(!bridge.remote_compaction_v1_request);
        assert!(bridge.remote_compaction_v2_request);
        assert!(!bridge.strips_client_auth);
    }

    #[test]
    fn prepare_request_body_applies_manual_profile_binding_defaults() {
        let raw_body = Bytes::from_static(
            br#"{"model":"gpt-5","service_tier":"priority","reasoning":{"effort":"low"}}"#,
        );

        let prepared = prepare_request_body(PrepareRequestBodyParams {
            raw_body: &raw_body,
            dialect: RequestDialect::ResponsesHttp,
            binding_effort: Some("high"),
            binding_model: Some("gpt-5.4"),
            binding_service_tier: Some("flex"),
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
    fn responses_profile_effort_preserves_pro_mode_and_future_fields() {
        let raw_body = Bytes::from_static(
            br#"{"model":"gpt-5.6-sol","parallel_tool_calls":false,"reasoning":{"effort":"low","mode":"pro","future_mode":"deliberate"},"future_request_field":{"enabled":true}}"#,
        );

        let prepared = prepare_request_body(PrepareRequestBodyParams {
            raw_body: &raw_body,
            dialect: RequestDialect::ResponsesHttp,
            binding_effort: Some("high"),
            binding_model: None,
            binding_service_tier: None,
        });

        let value: serde_json::Value =
            serde_json::from_slice(prepared.body_for_upstream.as_ref()).expect("json body");
        assert_eq!(value["reasoning"]["effort"].as_str(), Some("high"));
        assert_eq!(value["reasoning"]["mode"].as_str(), Some("pro"));
        assert_eq!(
            value["reasoning"]["future_mode"].as_str(),
            Some("deliberate")
        );
        assert_eq!(value["parallel_tool_calls"].as_bool(), Some(false));
        assert_eq!(
            value["future_request_field"]["enabled"].as_bool(),
            Some(true)
        );
        assert_eq!(prepared.effective_effort.as_deref(), Some("high"));
    }

    #[test]
    fn chat_profile_effort_uses_only_reasoning_effort() {
        let raw_body = Bytes::from_static(
            br#"{"model":"gpt-5.6-terra","messages":[],"parallel_tool_calls":false,"reasoning_effort":"low","future_request_field":{"future_reasoning":"preserve"}}"#,
        );

        let prepared = prepare_request_body(PrepareRequestBodyParams {
            raw_body: &raw_body,
            dialect: RequestDialect::ChatCompletions,
            binding_effort: Some("xhigh"),
            binding_model: None,
            binding_service_tier: None,
        });

        let value: serde_json::Value =
            serde_json::from_slice(prepared.body_for_upstream.as_ref()).expect("json body");
        assert_eq!(value["reasoning_effort"].as_str(), Some("xhigh"));
        assert!(value.get("reasoning").is_none());
        assert_eq!(value["parallel_tool_calls"].as_bool(), Some(false));
        assert_eq!(
            value["future_request_field"]["future_reasoning"].as_str(),
            Some("preserve")
        );
        assert_eq!(prepared.effective_effort.as_deref(), Some("xhigh"));
    }

    #[test]
    fn passthrough_dialect_does_not_apply_or_report_profile_effort() {
        let raw_body = Bytes::from_static(
            br#"{"model":"claude-sonnet","messages":[],"future_request_field":true}"#,
        );

        let prepared = prepare_request_body(PrepareRequestBodyParams {
            raw_body: &raw_body,
            dialect: RequestDialect::Passthrough,
            binding_effort: Some("high"),
            binding_model: None,
            binding_service_tier: None,
        });

        let value: serde_json::Value =
            serde_json::from_slice(prepared.body_for_upstream.as_ref()).expect("json body");
        assert!(value.get("reasoning").is_none());
        assert!(value.get("reasoning_effort").is_none());
        assert_eq!(value["future_request_field"].as_bool(), Some(true));
        assert_eq!(prepared.effective_effort, None);
    }

    #[test]
    fn ultra_is_deferred_as_orchestration_intent_without_global_mapping() {
        let raw_body = Bytes::from_static(
            br#"{"model":"gpt-5.6-sol","reasoning":{"effort":"low","mode":"pro"}}"#,
        );

        let prepared = prepare_request_body(PrepareRequestBodyParams {
            raw_body: &raw_body,
            dialect: RequestDialect::ResponsesHttp,
            binding_effort: Some("ultra"),
            binding_model: None,
            binding_service_tier: None,
        });

        let value: serde_json::Value =
            serde_json::from_slice(prepared.body_for_upstream.as_ref()).expect("json body");
        assert!(value["reasoning"].get("effort").is_none());
        assert_eq!(value["reasoning"]["mode"].as_str(), Some("pro"));
        assert_eq!(prepared.effective_effort, None);
        assert_eq!(
            prepared.deferred_reasoning_intent,
            Some(ReasoningOrchestrationIntent::Ultra)
        );

        let raw_body = Bytes::from_static(
            br#"{"model":"gpt-5.6-sol","reasoning":{"effort":"ultra","mode":"pro"}}"#,
        );
        let prepared = prepare_request_body(PrepareRequestBodyParams {
            raw_body: &raw_body,
            dialect: RequestDialect::ResponsesHttp,
            binding_effort: None,
            binding_model: None,
            binding_service_tier: None,
        });
        let value: serde_json::Value =
            serde_json::from_slice(prepared.body_for_upstream.as_ref()).expect("json body");
        assert!(value["reasoning"].get("effort").is_none());
        assert_eq!(value["reasoning"]["mode"].as_str(), Some("pro"));
        assert_eq!(prepared.effective_effort, None);
        assert_eq!(
            prepared.deferred_reasoning_intent,
            Some(ReasoningOrchestrationIntent::Ultra)
        );
    }

    #[test]
    fn prepare_request_body_preserves_unknown_tool_fields_without_client_preset() {
        let raw_body = Bytes::from_static(
            br#"{"model":"gpt-5","tools":[{"type":"image_generation","output_format":"png"}],"tool_choice":{"type":"image_generation"}}"#,
        );

        let prepared = prepare_request_body(PrepareRequestBodyParams {
            raw_body: &raw_body,
            dialect: RequestDialect::ResponsesHttp,
            binding_effort: None,
            binding_model: None,
            binding_service_tier: None,
        });

        let value: serde_json::Value =
            serde_json::from_slice(prepared.body_for_upstream.as_ref()).expect("json body");
        let tools = value
            .get("tools")
            .and_then(serde_json::Value::as_array)
            .expect("top-level tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].get("type").and_then(serde_json::Value::as_str),
            Some("image_generation")
        );
        assert_eq!(
            value
                .get("tool_choice")
                .and_then(|choice| choice.get("type"))
                .and_then(serde_json::Value::as_str),
            Some("image_generation")
        );
    }

    #[test]
    fn default_profile_binding_fields_are_not_request_overrides() {
        let binding = SessionBinding {
            session_id: "sid-fast".to_string(),
            profile_name: Some("daily".to_string()),
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

    fn test_proxy_with_active_route() -> ProxyService {
        let cfg = HelperConfig {
            codex: ServiceRouteConfig {
                default_profile: Some("default".to_string()),
                profiles: std::collections::BTreeMap::from([(
                    "default".to_string(),
                    ServiceControlProfile {
                        model: Some("gpt-5.4".to_string()),
                        ..ServiceControlProfile::default()
                    },
                )]),
                providers: std::collections::BTreeMap::from([(
                    "test".to_string(),
                    ProviderConfig {
                        base_url: Some("https://example.com/v1".to_string()),
                        ..ProviderConfig::default()
                    },
                )]),
                routing: Some(RouteGraphConfig::ordered_failover(vec!["test".to_string()])),
            },
            ..HelperConfig::default()
        };
        ProxyService::new(reqwest::Client::new(), Arc::new(cfg), "codex")
    }

    #[tokio::test]
    async fn proxy_request_preparation_does_not_read_host_local_session_history() {
        let codex_home = ScopedCodexHome::new();
        let session_id = "019f57be-9892-7081-b735-c0a524d2a127";
        let session_dir = codex_home.path.join("sessions/2026/07/13");
        std::fs::create_dir_all(&session_dir).expect("create fake session directory");
        std::fs::write(
            session_dir.join(format!(
                "rollout-2026-07-13T00-00-00-{session_id}.jsonl"
            )),
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"cwd\":\"/host/private/project\"}}}}\n"
            ),
        )
        .expect("write fake Codex session");
        let proxy = test_proxy_with_active_route();
        let config = load_request_config_context(&proxy).await;
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let method = Method::POST;
        let uri = "/v1/responses".parse::<Uri>().expect("uri");
        let raw_body = Bytes::from(
            serde_json::json!({
                "model": "gpt-5",
                "prompt_cache_key": session_id,
            })
            .to_string(),
        );

        let prepared = prepare_common_request_without_route_plan(CommonRequestPreparationParams {
            proxy: &proxy,
            config: &config,
            method: &method,
            uri: &uri,
            client_headers: &headers,
            raw_body: &raw_body,
            request_dialect: RequestDialect::ResponsesHttp,
            session_identity_hint:
                super::super::client_identity::extract_session_identity_with_body_fallback(
                    &headers,
                    raw_body.as_ref(),
                ),
            client_name: None,
            client_addr: None,
            started_at_ms: 1,
            client_content_type: Some("application/json"),
            request_body_previews: false,
        })
        .await
        .expect("prepare request");

        assert_eq!(prepared.session_id.as_deref(), Some(session_id));
        assert_eq!(prepared.cwd, None);
    }

    #[tokio::test]
    async fn prepare_common_request_tracks_prompt_cache_identity_without_default_profile_patch() {
        let proxy = test_proxy_with_active_route();
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
            request_dialect: RequestDialect::ResponsesHttp,
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
        let route_plan = prepared.route_plan.as_ref().expect("route plan");
        assert!(!route_plan.is_empty());

        let active = proxy.state.list_active_requests().await;
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].session_id.as_deref(), Some("pcache-shared"));
        assert_eq!(
            active[0].session_identity_source,
            Some(SessionIdentitySource::PromptCacheKey)
        );
    }

    #[tokio::test]
    async fn prepare_common_request_preserves_unknown_tools_without_client_preset() {
        let proxy = test_proxy_with_active_route();
        let config = load_request_config_context(&proxy).await;
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let method = Method::POST;
        let uri = "/v1/responses".parse::<Uri>().expect("uri");
        let raw_body = Bytes::from_static(
            br#"{"model":"gpt-5","prompt_cache_key":"image-contract","tools":[{"type":"image_generation","output_format":"png"}],"tool_choice":{"type":"image_generation"}}"#,
        );

        let prepared = prepare_common_request(CommonRequestPreparationParams {
            proxy: &proxy,
            config: &config,
            method: &method,
            uri: &uri,
            client_headers: &headers,
            raw_body: &raw_body,
            request_dialect: RequestDialect::ResponsesHttp,
            session_identity_hint:
                super::super::client_identity::extract_session_identity_with_body_fallback(
                    &headers,
                    raw_body.as_ref(),
                ),
            client_name: Some("test-client".to_string()),
            client_addr: None,
            started_at_ms: 3,
            client_content_type: Some("application/json"),
            request_body_previews: false,
        })
        .await
        .expect("prepared");

        let value: serde_json::Value =
            serde_json::from_slice(prepared.body_for_upstream.as_ref()).expect("json body");
        let tools = value
            .get("tools")
            .and_then(serde_json::Value::as_array)
            .expect("tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].get("type").and_then(serde_json::Value::as_str),
            Some("image_generation")
        );
        assert_eq!(
            value
                .get("tool_choice")
                .and_then(|choice| choice.get("type"))
                .and_then(serde_json::Value::as_str),
            Some("image_generation")
        );
    }
}

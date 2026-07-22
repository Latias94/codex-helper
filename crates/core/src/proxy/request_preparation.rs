use std::sync::Arc;

use axum::body::Bytes;
use axum::http::{HeaderMap, Method, Uri};

use crate::config::{CODEX_CLIENT_RUNTIME_PATCH_HEADER, CodexClientRuntimePatch};
use crate::endpoint_health::{CooldownBackoff, RouteCapability};
use crate::logging::{BodyPreview, CodexBridgeLog, ServiceTierLog, make_body_preview};
use crate::routing_ir::RouteRequestContext;
use crate::runtime_store::RequestAccountingScope;
use crate::state::{
    SessionBinding, SessionContinuityMode, SessionIdentitySource, SessionRouteControlGuard,
};

use super::ProxyService;
use super::client_identity::ClientSessionIdentity;
use super::request_body::{
    ReasoningOrchestrationIntent, RequestDialect, apply_model_override_value,
    apply_reasoning_effort_override_value, apply_service_tier_override_value,
    extract_model_from_value, extract_reasoning_effort_from_value, extract_service_tier_from_value,
    normalize_codex_compact_request_value, remove_hosted_image_generation_tools_value,
    remove_reasoning_effort_value,
};
use super::request_continuity::{
    RequestContinuityClassificationInput, RequestTransport, classify_request_continuity,
};
use super::retry::{RetryPlan, retry_plan};
use super::runtime_config::{CapturedRoutePlan, RuntimeSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SharedRouteStateImpact {
    RouteFacing,
    RequestLocalOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestReplayPolicy {
    RouteFacing,
    SafeRead,
    NeverAfterDispatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StreamTerminalPolicy {
    ProtocolEvent,
    EndOfBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointSurface {
    Inference,
    RemoteCompaction,
    ModelCatalog,
    RequestLocalResource,
    Unknown,
}

#[derive(Debug, Clone, Copy)]
enum EndpointSurfaceMatcher {
    ExactSuffix(&'static str),
    SegmentFamily(&'static str),
}

#[derive(Debug, Clone, Copy)]
struct EndpointSurfaceRule {
    matcher: EndpointSurfaceMatcher,
    surface: EndpointSurface,
}

const ENDPOINT_SURFACE_CATALOG: &[EndpointSurfaceRule] = &[
    EndpointSurfaceRule {
        matcher: EndpointSurfaceMatcher::ExactSuffix("/responses/compact"),
        surface: EndpointSurface::RemoteCompaction,
    },
    EndpointSurfaceRule {
        matcher: EndpointSurfaceMatcher::ExactSuffix("/responses"),
        surface: EndpointSurface::Inference,
    },
    EndpointSurfaceRule {
        matcher: EndpointSurfaceMatcher::ExactSuffix("/chat/completions"),
        surface: EndpointSurface::Inference,
    },
    EndpointSurfaceRule {
        matcher: EndpointSurfaceMatcher::ExactSuffix("/messages"),
        surface: EndpointSurface::Inference,
    },
    EndpointSurfaceRule {
        matcher: EndpointSurfaceMatcher::SegmentFamily("models"),
        surface: EndpointSurface::ModelCatalog,
    },
    EndpointSurfaceRule {
        matcher: EndpointSurfaceMatcher::SegmentFamily("files"),
        surface: EndpointSurface::RequestLocalResource,
    },
    EndpointSurfaceRule {
        matcher: EndpointSurfaceMatcher::SegmentFamily("uploads"),
        surface: EndpointSurface::RequestLocalResource,
    },
    EndpointSurfaceRule {
        matcher: EndpointSurfaceMatcher::SegmentFamily("batches"),
        surface: EndpointSurface::RequestLocalResource,
    },
    EndpointSurfaceRule {
        matcher: EndpointSurfaceMatcher::SegmentFamily("containers"),
        surface: EndpointSurface::RequestLocalResource,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestOrigin {
    Client,
    ImagesCompatibility,
}

impl SharedRouteStateImpact {
    pub(super) fn allows_shared_updates(self) -> bool {
        self == Self::RouteFacing
    }
}

impl RequestReplayPolicy {
    pub(super) fn allows_after_dispatch(self) -> bool {
        self != Self::NeverAfterDispatch
    }

    pub(super) fn allows_credential_failover(self) -> bool {
        self == Self::SafeRead
    }
}

impl EndpointSurfaceMatcher {
    fn matches(self, normalized_path: &str) -> bool {
        match self {
            Self::ExactSuffix(suffix) => normalized_path.ends_with(suffix),
            Self::SegmentFamily(family) => {
                normalized_path.split('/').any(|segment| segment == family)
            }
        }
    }
}

impl EndpointSurface {
    fn shared_route_state_impact(self) -> SharedRouteStateImpact {
        match self {
            Self::Inference | Self::RemoteCompaction => SharedRouteStateImpact::RouteFacing,
            Self::ModelCatalog | Self::RequestLocalResource | Self::Unknown => {
                SharedRouteStateImpact::RequestLocalOnly
            }
        }
    }

    fn route_capability(self) -> RouteCapability {
        match self {
            Self::Inference => RouteCapability::Inference,
            Self::RemoteCompaction => RouteCapability::RemoteCompaction,
            // Request-local surfaces never project or update shared health. ModelCatalog is an
            // inert capability value for the concrete field carried through an HTTP attempt.
            Self::ModelCatalog | Self::RequestLocalResource | Self::Unknown => {
                RouteCapability::ModelCatalog
            }
        }
    }

    fn terminal_accounting(self) -> RequestAccountingScope {
        match self {
            Self::Inference | Self::RemoteCompaction => RequestAccountingScope::Economic,
            Self::ModelCatalog | Self::RequestLocalResource | Self::Unknown => {
                RequestAccountingScope::NonEconomic
            }
        }
    }

    fn stream_terminal_policy(self) -> StreamTerminalPolicy {
        match self {
            Self::Inference | Self::RemoteCompaction => StreamTerminalPolicy::ProtocolEvent,
            Self::ModelCatalog | Self::RequestLocalResource | Self::Unknown => {
                StreamTerminalPolicy::EndOfBody
            }
        }
    }

    fn replay_policy(self, method: &Method) -> RequestReplayPolicy {
        if self.shared_route_state_impact().allows_shared_updates() {
            RequestReplayPolicy::RouteFacing
        } else if method == Method::GET || method == Method::HEAD {
            RequestReplayPolicy::SafeRead
        } else {
            RequestReplayPolicy::NeverAfterDispatch
        }
    }
}

fn endpoint_surface(method: &Method, path: &str) -> EndpointSurface {
    let normalized_path = path.trim_end_matches('/');
    let surface = ENDPOINT_SURFACE_CATALOG
        .iter()
        .find(|rule| {
            matches!(rule.matcher, EndpointSurfaceMatcher::SegmentFamily(_))
                && rule.matcher.matches(normalized_path)
        })
        .or_else(|| {
            ENDPOINT_SURFACE_CATALOG
                .iter()
                .find(|rule| rule.matcher.matches(normalized_path))
        })
        .map(|rule| rule.surface)
        .unwrap_or(EndpointSurface::Unknown);

    // Responses WebSocket GET upgrades are handled by the dedicated router before this HTTP
    // forwarding path. Other methods must not create inference or compaction route evidence.
    if *method != Method::POST
        && matches!(
            surface,
            EndpointSurface::Inference | EndpointSurface::RemoteCompaction
        )
    {
        EndpointSurface::Unknown
    } else {
        surface
    }
}

#[derive(Debug, Clone)]
pub(super) struct RequestFlavor {
    pub client_content_type: Option<String>,
    pub is_stream: bool,
    pub is_user_turn: bool,
    pub is_remote_compaction_v1_request: bool,
    pub is_remote_compaction_v2_request: bool,
    pub remote_v2_downgrade_enabled: bool,
    pub remote_compaction_requires_affinity: bool,
    pub is_codex_service: bool,
    pub shared_route_state_impact: SharedRouteStateImpact,
    pub terminal_accounting: RequestAccountingScope,
    pub route_capability: RouteCapability,
    pub stream_terminal_policy: StreamTerminalPolicy,
    pub replay_policy: RequestReplayPolicy,
    pub codex_bridge_log: Option<CodexBridgeLog>,
}

impl RequestFlavor {
    pub(super) fn is_remote_compaction_request(&self) -> bool {
        self.is_remote_compaction_v1_request || self.is_remote_compaction_v2_request
    }

    pub(super) fn with_remote_v2_downgrade_enabled(mut self, enabled: bool) -> Self {
        self.remote_v2_downgrade_enabled = enabled;
        self
    }

    pub(super) fn route_state_session_id<'a>(
        &self,
        session_id: Option<&'a str>,
    ) -> Option<&'a str> {
        if self.shared_route_state_impact.allows_shared_updates() {
            session_id
        } else {
            None
        }
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
        if self.is_remote_compaction_request() {
            self.route_capability = RouteCapability::RemoteCompaction;
        }
        if continuity.is_remote_compaction_v2_request {
            let bridge = self.codex_bridge_log.get_or_insert(CodexBridgeLog {
                patch_mode: "request-dialect".to_string(),
                remote_compaction_v1_request: false,
                remote_compaction_v2_request: false,
                downgraded_to_responses_compact: false,
                responses_websocket_request: false,
                strips_client_auth: false,
            });
            bridge.remote_compaction_v2_request = true;
        }
        self
    }

    pub(super) fn with_hosted_image_generation(mut self, enabled: bool) -> Self {
        if enabled {
            self.route_capability = RouteCapability::HostedImageGeneration;
        }
        self
    }

    pub(super) fn transient_health_capability(&self) -> Option<RouteCapability> {
        self.shared_route_state_impact
            .allows_shared_updates()
            .then_some(self.route_capability)
    }

    pub(super) fn allows_request_body_transforms(&self) -> bool {
        self.shared_route_state_impact.allows_shared_updates()
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

pub(super) struct RequestConfigContext {
    pub(super) runtime_snapshot: Arc<RuntimeSnapshot>,
    session_id: Option<String>,
    session_identity_source: Option<SessionIdentitySource>,
    session_route_control: Option<SessionRouteControlGuard>,
}

pub(super) struct CommonPreparedRequest {
    pub(super) session_id: Option<String>,
    pub(super) session_identity_source: Option<SessionIdentitySource>,
    pub(super) session_binding: Option<SessionBinding>,
    pub(super) route_plan: Option<CapturedRoutePlan>,
    pub(super) cwd: Option<String>,
    pub(super) body_for_upstream: Bytes,
    pub(super) request_dialect: RequestDialect,
    pub(super) translate_openai_models: bool,
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
    pub(super) request_origin: RequestOrigin,
    pub(super) client_name: Option<String>,
    pub(super) client_addr: Option<String>,
    pub(super) started_at_ms: u64,
    pub(super) client_content_type: Option<&'a str>,
    pub(super) request_body_previews: bool,
}

pub(super) async fn load_request_config_context(
    proxy: &ProxyService,
    session_identity: Option<&ClientSessionIdentity>,
) -> RequestConfigContext {
    let session_id = session_identity_value(session_identity);
    let session_identity_source = session_identity_source(session_identity);
    let session_route_control = match session_id.as_deref() {
        Some(session_id) => Some(proxy.state.lock_session_route_control(session_id).await),
        None => None,
    };
    // Capturing after the per-session guard is the request's MVCC
    // linearization point relative to affinity control mutations.
    let runtime_snapshot = proxy.config.capture().await;

    RequestConfigContext {
        runtime_snapshot,
        session_id,
        session_identity_source,
        session_route_control,
    }
}

pub(super) async fn prepare_common_request(
    params: CommonRequestPreparationParams<'_>,
) -> Result<CommonPreparedRequest, CommonRequestPreparationError> {
    prepare_common_request_inner(params, true).await
}

pub(super) async fn prepare_http_request(
    params: CommonRequestPreparationParams<'_>,
    client_body: &Bytes,
) -> Result<CommonPreparedRequest, CommonRequestPreparationError> {
    let client_content_type = params.client_content_type;
    let request_body_previews = params.request_body_previews;
    let mut prepared = prepare_common_request_inner(params, true).await?;
    let client_body_previews = build_body_previews(
        client_body,
        client_content_type,
        request_body_previews,
        prepared.debug_max,
        prepared.warn_max,
    );

    prepared.request_body_len = client_body.len();
    prepared.client_body_debug = client_body_previews.debug;
    prepared.client_body_warn = client_body_previews.warn;
    Ok(prepared)
}

#[cfg(test)]
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
        request_origin,
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
    let client_runtime_patch = client_runtime_patch(client_headers);
    let session_id = config.session_id.clone();
    let session_identity_source = config.session_identity_source;
    let session_binding = if let Some(id) = session_id.as_deref() {
        proxy
            .ensure_default_session_binding(config.runtime_snapshot.as_ref(), id, started_at_ms)
            .await
    } else {
        None
    };
    touch_session_state(proxy, session_id.as_deref(), started_at_ms).await;
    let cwd = None;

    let binding_effort = binding_reasoning_effort_for_request(session_binding.as_ref());
    let binding_model = binding_model_for_request(session_binding.as_ref());
    let binding_service_tier = binding_service_tier_for_request(session_binding.as_ref());
    let filter_hosted_image_generation_tools = proxy.service_name == "codex"
        && request_origin == RequestOrigin::Client
        && request_dialect.supports_hosted_image_generation_tools()
        && client_runtime_patch
            .map(|patch| patch.hosted_image_generation)
            .or_else(|| {
                view.client_patch
                    .as_ref()
                    .map(|patch| patch.hosted_image_generation)
            })
            .is_some_and(|mode| mode.filters_client_image_requests());
    let translate_openai_models = proxy.service_name == "codex"
        && client_runtime_patch
            .map(|patch| patch.translate_models)
            .or_else(|| {
                view.client_patch
                    .as_ref()
                    .map(|patch| patch.translate_models)
            })
            .unwrap_or(false);
    let body_transforms_allowed = request_origin == RequestOrigin::ImagesCompatibility
        || request_dialect == RequestDialect::ResponsesWebSocket
        || endpoint_surface(method, uri.path())
            .shared_route_state_impact()
            .allows_shared_updates();
    let prepared_request = if body_transforms_allowed {
        prepare_request_body(PrepareRequestBodyParams {
            raw_body,
            dialect: request_dialect,
            binding_effort,
            binding_model,
            binding_service_tier,
            filter_hosted_image_generation_tools,
        })
    } else {
        inspect_passthrough_request_body(raw_body, request_dialect)
    };
    let body_for_upstream = prepared_request.body_for_upstream.clone();
    let request_model = prepared_request.request_model.clone();
    let effective_effort = prepared_request.effective_effort.clone();
    let deferred_reasoning_intent = prepared_request.deferred_reasoning_intent;
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
        .try_begin_request_with_session_route_control(
            config.session_route_control.as_ref(),
            proxy.service_name,
            method.as_str(),
            uri.path(),
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
        translate_openai_models,
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

fn client_runtime_patch(headers: &HeaderMap) -> Option<CodexClientRuntimePatch> {
    let mut values = headers.get_all(CODEX_CLIENT_RUNTIME_PATCH_HEADER).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    value
        .to_str()
        .ok()
        .and_then(CodexClientRuntimePatch::decode)
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

    let is_codex_service = service_name == "codex";
    let endpoint_surface = endpoint_surface(method, path);
    let shared_route_state_impact = endpoint_surface.shared_route_state_impact();
    let is_stream = headers
        .get("accept")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_ascii_lowercase().contains("text/event-stream"))
        .unwrap_or(false);

    let is_responses_path = codex_path_is_responses(path);
    let is_remote_compaction_v1_request =
        *method == Method::POST && codex_path_is_responses_compact(path);
    let is_user_turn = *method == Method::POST && is_responses_path;
    let codex_bridge_log =
        (is_codex_service && is_remote_compaction_v1_request).then(|| CodexBridgeLog {
            patch_mode: "request-dialect".to_string(),
            remote_compaction_v1_request: is_remote_compaction_v1_request,
            remote_compaction_v2_request: false,
            downgraded_to_responses_compact: false,
            responses_websocket_request: false,
            strips_client_auth: false,
        });

    RequestFlavor {
        client_content_type,
        is_stream,
        is_user_turn,
        is_remote_compaction_v1_request,
        is_remote_compaction_v2_request: false,
        remote_v2_downgrade_enabled: false,
        remote_compaction_requires_affinity: false,
        is_codex_service,
        shared_route_state_impact,
        terminal_accounting: endpoint_surface.terminal_accounting(),
        route_capability: endpoint_surface.route_capability(),
        stream_terminal_policy: endpoint_surface.stream_terminal_policy(),
        replay_policy: endpoint_surface.replay_policy(method),
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
    identity.and_then(|identity| {
        let value = identity.value().trim();
        (!value.is_empty()).then(|| value.to_string())
    })
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
    filter_hosted_image_generation_tools: bool,
}

pub(super) fn prepare_request_body(params: PrepareRequestBodyParams<'_>) -> PreparedRequestBody {
    let PrepareRequestBodyParams {
        raw_body,
        dialect,
        binding_effort,
        binding_model,
        binding_service_tier,
        filter_hosted_image_generation_tools,
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
        if filter_hosted_image_generation_tools {
            remove_hosted_image_generation_tools_value(value);
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

fn inspect_passthrough_request_body(
    raw_body: &Bytes,
    dialect: RequestDialect,
) -> PreparedRequestBody {
    let request_json = serde_json::from_slice::<serde_json::Value>(raw_body).ok();
    let request_model = request_json.as_ref().and_then(extract_model_from_value);
    let effective_effort = request_json
        .as_ref()
        .and_then(|value| extract_reasoning_effort_from_value(value, dialect));
    let service_tier = request_json
        .as_ref()
        .and_then(extract_service_tier_from_value);

    PreparedRequestBody {
        body_for_upstream: raw_body.clone(),
        request_model: request_model.clone(),
        requested_model: request_model,
        effective_effort,
        deferred_reasoning_intent: None,
        base_service_tier: ServiceTierLog {
            requested: service_tier.clone(),
            effective: service_tier,
            actual: None,
        },
        request_body_len: raw_body.len(),
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
    use std::time::Duration;

    use axum::http::{HeaderMap, HeaderValue};

    use super::*;
    use crate::config::{
        CodexClientPatchConfig, CodexHostedImageGenerationMode, HelperConfig, LoadedConfig,
        ProviderConfig, RouteGraphConfig, ServiceControlProfile, ServiceRouteConfig,
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
        assert_eq!(
            flavor.shared_route_state_impact,
            SharedRouteStateImpact::RouteFacing
        );
        assert_eq!(flavor.route_capability, RouteCapability::Inference);
        assert_eq!(
            flavor.transient_health_capability(),
            Some(RouteCapability::Inference)
        );
    }

    #[test]
    fn resource_endpoint_catalog_is_request_local_for_all_services_and_methods() {
        let mut headers = HeaderMap::new();
        headers.insert("accept", HeaderValue::from_static("text/event-stream"));

        for (service, method, path, expected_surface) in [
            (
                "codex",
                Method::GET,
                "/models",
                EndpointSurface::ModelCatalog,
            ),
            (
                "codex",
                Method::POST,
                "/v1/models/",
                EndpointSurface::ModelCatalog,
            ),
            (
                "claude",
                Method::GET,
                "/backend-api/codex/models/gpt-5.6-sol",
                EndpointSurface::ModelCatalog,
            ),
            (
                "codex",
                Method::GET,
                "/files",
                EndpointSurface::RequestLocalResource,
            ),
            (
                "codex",
                Method::DELETE,
                "/v1/files/file-1",
                EndpointSurface::RequestLocalResource,
            ),
            (
                "codex",
                Method::POST,
                "/v1/files/responses",
                EndpointSurface::RequestLocalResource,
            ),
            (
                "codex",
                Method::POST,
                "/v1/models/messages",
                EndpointSurface::ModelCatalog,
            ),
            (
                "codex",
                Method::POST,
                "/v1/uploads/upload-1/complete",
                EndpointSurface::RequestLocalResource,
            ),
            (
                "codex",
                Method::POST,
                "/v1/batches/batch-1/cancel",
                EndpointSurface::RequestLocalResource,
            ),
            (
                "codex",
                Method::GET,
                "/v1/containers/container-1/files",
                EndpointSurface::RequestLocalResource,
            ),
        ] {
            assert_eq!(endpoint_surface(&method, path), expected_surface, "{path}");
            let flavor = detect_request_flavor(service, &method, &headers, path);
            assert_eq!(
                flavor.shared_route_state_impact,
                SharedRouteStateImpact::RequestLocalOnly,
                "{path}"
            );
            assert!(flavor.is_stream, "{path}");
            assert_eq!(
                flavor.stream_terminal_policy,
                StreamTerminalPolicy::EndOfBody,
                "{path}"
            );
            assert_eq!(flavor.route_state_session_id(Some("session-a")), None);
            assert_eq!(flavor.transient_health_capability(), None, "{path}");
            assert_eq!(
                flavor.terminal_accounting,
                RequestAccountingScope::NonEconomic,
                "{path}"
            );
        }
    }

    #[test]
    fn endpoint_replay_policy_distinguishes_safe_reads_from_mutations() {
        for (method, path) in [
            (Method::GET, "/v1/models"),
            (Method::HEAD, "/v1/files/file-1"),
            (Method::GET, "/v1/future-resource"),
        ] {
            let flavor = detect_request_flavor("codex", &method, &HeaderMap::new(), path);
            assert_eq!(
                flavor.replay_policy,
                RequestReplayPolicy::SafeRead,
                "{path}"
            );
        }

        for (method, path) in [
            (Method::POST, "/v1/files"),
            (Method::DELETE, "/v1/files/file-1"),
            (Method::POST, "/v1/future-resource"),
        ] {
            let flavor = detect_request_flavor("codex", &method, &HeaderMap::new(), path);
            assert_eq!(
                flavor.replay_policy,
                RequestReplayPolicy::NeverAfterDispatch,
                "{path}"
            );
        }

        let inference =
            detect_request_flavor("codex", &Method::POST, &HeaderMap::new(), "/v1/responses");
        assert_eq!(inference.replay_policy, RequestReplayPolicy::RouteFacing);
    }

    #[test]
    fn conversation_endpoint_catalog_keeps_existing_capabilities() {
        for (service, method, path, capability) in [
            (
                "codex",
                Method::POST,
                "/v1/responses",
                RouteCapability::Inference,
            ),
            (
                "codex",
                Method::POST,
                "/v1/chat/completions/",
                RouteCapability::Inference,
            ),
            (
                "claude",
                Method::POST,
                "/v1/messages",
                RouteCapability::Inference,
            ),
            (
                "codex",
                Method::POST,
                "/v1/responses/compact",
                RouteCapability::RemoteCompaction,
            ),
        ] {
            let flavor = detect_request_flavor(service, &method, &HeaderMap::new(), path);
            assert_eq!(
                flavor.shared_route_state_impact,
                SharedRouteStateImpact::RouteFacing,
                "{service} {method} {path}"
            );
            assert_eq!(flavor.route_capability, capability, "{path}");
            assert_eq!(
                flavor.stream_terminal_policy,
                StreamTerminalPolicy::ProtocolEvent,
                "{path}"
            );
            assert_eq!(flavor.transient_health_capability(), Some(capability));
            assert_eq!(
                flavor.terminal_accounting,
                RequestAccountingScope::Economic,
                "{path}"
            );
            assert_eq!(
                flavor.route_state_session_id(Some("session-a")),
                Some("session-a")
            );
        }
    }

    #[test]
    fn non_post_conversation_endpoints_are_request_local() {
        for (method, path) in [
            (Method::GET, "/v1/responses"),
            (Method::DELETE, "/v1/chat/completions"),
            (Method::GET, "/v1/messages"),
            (Method::PUT, "/v1/responses/compact"),
        ] {
            assert_eq!(endpoint_surface(&method, path), EndpointSurface::Unknown);
            let flavor = detect_request_flavor("codex", &method, &HeaderMap::new(), path);
            assert_eq!(
                flavor.shared_route_state_impact,
                SharedRouteStateImpact::RequestLocalOnly,
                "{method} {path}"
            );
            assert_eq!(flavor.route_state_session_id(Some("session-a")), None);
            assert_eq!(
                flavor.transient_health_capability(),
                None,
                "{method} {path}"
            );
            assert!(!flavor.is_remote_compaction_request(), "{method} {path}");
            assert_eq!(
                flavor.terminal_accounting,
                RequestAccountingScope::NonEconomic,
                "{method} {path}"
            );
        }
    }

    #[test]
    fn unknown_endpoint_is_request_local_instead_of_inference_scoped() {
        for path in ["/", "/v1/future-resource", "/vendor/custom/action/"] {
            assert_eq!(
                endpoint_surface(&Method::POST, path),
                EndpointSurface::Unknown,
                "{path}"
            );
            let flavor = detect_request_flavor("codex", &Method::POST, &HeaderMap::new(), path);
            assert_eq!(
                flavor.shared_route_state_impact,
                SharedRouteStateImpact::RequestLocalOnly,
                "{path}"
            );
            assert_eq!(flavor.route_state_session_id(Some("session-a")), None);
            assert_eq!(flavor.transient_health_capability(), None, "{path}");
            assert_eq!(
                flavor.terminal_accounting,
                RequestAccountingScope::NonEconomic,
                "{path}"
            );
        }
    }

    #[test]
    fn hosted_image_bridge_keeps_its_dedicated_capability() {
        let flavor =
            detect_request_flavor("codex", &Method::POST, &HeaderMap::new(), "/v1/responses")
                .with_hosted_image_generation(true);

        assert_eq!(
            flavor.shared_route_state_impact,
            SharedRouteStateImpact::RouteFacing
        );
        assert_eq!(
            flavor.transient_health_capability(),
            Some(RouteCapability::HostedImageGeneration)
        );
    }

    #[test]
    fn detect_request_flavor_marks_codex_bridge_compact_request() {
        let headers = HeaderMap::new();

        let flavor =
            detect_request_flavor("codex", &Method::POST, &headers, "/v1/responses/compact");

        assert!(!flavor.is_user_turn);
        assert!(flavor.is_remote_compaction_v1_request);
        assert_eq!(
            flavor.transient_health_capability(),
            Some(RouteCapability::RemoteCompaction)
        );
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
        assert_eq!(
            flavor.transient_health_capability(),
            Some(RouteCapability::RemoteCompaction)
        );
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
            filter_hosted_image_generation_tools: false,
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
            filter_hosted_image_generation_tools: false,
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
            filter_hosted_image_generation_tools: false,
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
            filter_hosted_image_generation_tools: false,
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
            filter_hosted_image_generation_tools: false,
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
            filter_hosted_image_generation_tools: false,
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
            filter_hosted_image_generation_tools: false,
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
    fn prepare_request_body_filters_hosted_image_generation_when_requested() {
        let raw_body = Bytes::from_static(
            br#"{"model":"gpt-5","input":[{"type":"additional_tools","tools":[{"type":"image_generation","output_format":"png"},{"type":"function","name":"nested"}]}],"tools":[{"type":"image_generation","output_format":"png"},{"type":"function","name":"shell"}],"tool_choice":{"type":"image_generation"}}"#,
        );

        let prepared = prepare_request_body(PrepareRequestBodyParams {
            raw_body: &raw_body,
            dialect: RequestDialect::ResponsesHttp,
            binding_effort: None,
            binding_model: None,
            binding_service_tier: None,
            filter_hosted_image_generation_tools: true,
        });

        let value: serde_json::Value =
            serde_json::from_slice(prepared.body_for_upstream.as_ref()).expect("json body");
        assert_eq!(value["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["tools"][0]["name"].as_str(), Some("shell"));
        assert_eq!(value["input"][0]["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            value["input"][0]["tools"][0]["name"].as_str(),
            Some("nested")
        );
        assert_eq!(value["tool_choice"].as_str(), Some("auto"));
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

    fn test_config_with_active_route(provider_id: &str) -> HelperConfig {
        HelperConfig {
            codex: ServiceRouteConfig {
                client_patch: None,
                compaction: None,
                default_profile: Some("default".to_string()),
                profiles: std::collections::BTreeMap::from([(
                    "default".to_string(),
                    ServiceControlProfile {
                        model: Some("gpt-5.4".to_string()),
                        ..ServiceControlProfile::default()
                    },
                )]),
                providers: std::collections::BTreeMap::from([(
                    provider_id.to_string(),
                    ProviderConfig {
                        base_url: Some("https://example.com/v1".to_string()),
                        ..ProviderConfig::default()
                    },
                )]),
                routing: Some(RouteGraphConfig::ordered_failover(vec![
                    provider_id.to_string(),
                ])),
            },
            ..HelperConfig::default()
        }
    }

    fn test_proxy_with_active_route() -> ProxyService {
        ProxyService::new(
            reqwest::Client::new(),
            Arc::new(test_config_with_active_route("test")),
            "codex",
        )
    }

    fn test_proxy_with_hosted_image_generation_disabled() -> ProxyService {
        let mut config = test_config_with_active_route("test");
        config.codex.client_patch = Some(CodexClientPatchConfig {
            translate_models: true,
            hosted_image_generation: CodexHostedImageGenerationMode::Disabled,
            ..CodexClientPatchConfig::default()
        });
        ProxyService::new(reqwest::Client::new(), Arc::new(config), "codex")
    }

    #[tokio::test]
    async fn request_context_holds_the_canonical_session_guard_through_snapshot_capture() {
        let proxy = test_proxy_with_active_route();
        let state = Arc::clone(&proxy.state);
        let held = state.lock_session_route_control("session-guarded").await;
        let waiting = state
            .signal_next_session_route_control_lock_wait_for_test()
            .await;
        let mut headers = HeaderMap::new();
        headers.insert(
            "session-id",
            HeaderValue::from_static("  session-guarded  "),
        );
        let session_identity = super::super::client_identity::extract_session_identity(&headers);
        let task = tokio::spawn({
            let proxy = proxy.clone();
            async move { load_request_config_context(&proxy, session_identity.as_ref()).await }
        });

        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("request should reach the held session guard")
            .expect("request wait signal should remain connected");
        assert!(
            !task.is_finished(),
            "snapshot capture must wait behind control"
        );

        drop(held);
        let context = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("request context should resume")
            .expect("request context task should join");
        assert_eq!(context.session_id.as_deref(), Some("session-guarded"));
        assert!(
            state
                .try_lock_session_route_control("  session-guarded ")
                .await
                .is_none(),
            "request context must retain its typed guard"
        );

        drop(context);
        assert!(
            state
                .try_lock_session_route_control("session-guarded")
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn waiting_request_captures_runtime_snapshot_after_guard_and_reload() {
        let proxy = test_proxy_with_active_route();
        let before = proxy.config.capture().await;
        let state = Arc::clone(&proxy.state);
        let held = tokio::time::timeout(
            Duration::from_secs(1),
            state.lock_session_route_control("session-reload-order"),
        )
        .await
        .expect("control mutation should acquire the session guard");
        let waiting = state
            .signal_next_session_route_control_lock_wait_for_test()
            .await;
        let mut headers = HeaderMap::new();
        headers.insert(
            "session-id",
            HeaderValue::from_static("session-reload-order"),
        );
        let session_identity = super::super::client_identity::extract_session_identity(&headers);
        let request = tokio::spawn({
            let proxy = proxy.clone();
            async move { load_request_config_context(&proxy, session_identity.as_ref()).await }
        });
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("request should reach the held session guard")
            .expect("request wait signal should remain connected");

        let changed = tokio::time::timeout(
            Duration::from_secs(1),
            proxy.config.reload_with_source(|| async {
                Ok((
                    LoadedConfig {
                        source: test_config_with_active_route("reloaded"),
                    },
                    None,
                ))
            }),
        )
        .await
        .expect("reload should not deadlock with a session guard")
        .expect("reload runtime snapshot");
        assert!(changed);
        let after_reload = proxy.config.capture().await;
        assert!(after_reload.revision() > before.revision());

        drop(held);
        let context = tokio::time::timeout(Duration::from_secs(1), request)
            .await
            .expect("request should resume after control mutation")
            .expect("request context task should join");
        assert_eq!(context.runtime_snapshot.revision(), after_reload.revision());
        let candidate = context
            .runtime_snapshot
            .route_graph("codex")
            .expect("codex route graph")
            .handshake_plan()
            .candidates
            .into_iter()
            .next()
            .expect("reloaded route candidate");
        assert_eq!(candidate.provider_id, "reloaded");
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
        let session_identity =
            super::super::client_identity::extract_session_identity_with_body_fallback(
                &headers,
                raw_body.as_ref(),
            );
        let config = load_request_config_context(&proxy, session_identity.as_ref()).await;

        let prepared = prepare_common_request_without_route_plan(CommonRequestPreparationParams {
            proxy: &proxy,
            config: &config,
            method: &method,
            uri: &uri,
            client_headers: &headers,
            raw_body: &raw_body,
            request_dialect: RequestDialect::ResponsesHttp,
            request_origin: RequestOrigin::Client,
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
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let method = Method::POST;
        let uri = "/v1/responses".parse::<Uri>().expect("uri");
        let raw_body =
            Bytes::from_static(br#"{"model":"gpt-5","prompt_cache_key":"pcache-shared"}"#);
        let session_identity =
            super::super::client_identity::extract_session_identity_with_body_fallback(
                &headers,
                raw_body.as_ref(),
            );
        let config = load_request_config_context(&proxy, session_identity.as_ref()).await;

        let prepared = prepare_common_request(CommonRequestPreparationParams {
            proxy: &proxy,
            config: &config,
            method: &method,
            uri: &uri,
            client_headers: &headers,
            raw_body: &raw_body,
            request_dialect: RequestDialect::ResponsesHttp,
            request_origin: RequestOrigin::Client,
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
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let method = Method::POST;
        let uri = "/v1/responses".parse::<Uri>().expect("uri");
        let raw_body = Bytes::from_static(
            br#"{"model":"gpt-5","prompt_cache_key":"image-contract","tools":[{"type":"image_generation","output_format":"png"}],"tool_choice":{"type":"image_generation"}}"#,
        );
        let session_identity =
            super::super::client_identity::extract_session_identity_with_body_fallback(
                &headers,
                raw_body.as_ref(),
            );
        let config = load_request_config_context(&proxy, session_identity.as_ref()).await;

        let prepared = prepare_common_request(CommonRequestPreparationParams {
            proxy: &proxy,
            config: &config,
            method: &method,
            uri: &uri,
            client_headers: &headers,
            raw_body: &raw_body,
            request_dialect: RequestDialect::ResponsesHttp,
            request_origin: RequestOrigin::Client,
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

    #[tokio::test]
    async fn prepare_common_request_filters_from_captured_client_patch_snapshot() {
        let proxy = test_proxy_with_hosted_image_generation_disabled();
        let config = load_request_config_context(&proxy, None).await;
        let captured_revision = config.runtime_snapshot.revision();
        proxy
            .config
            .reload_with_source(|| async {
                Ok((
                    LoadedConfig {
                        source: test_config_with_active_route("test"),
                    },
                    None,
                ))
            })
            .await
            .expect("reload runtime config");
        assert!(proxy.config.capture().await.revision() > captured_revision);

        let headers = HeaderMap::new();
        let method = Method::POST;
        let uri = "/v1/responses".parse::<Uri>().expect("uri");
        let raw_body = Bytes::from_static(
            br#"{"model":"gpt-5","tools":[{"type":"image_generation"},{"type":"function","name":"shell"}],"tool_choice":{"type":"image_generation"}}"#,
        );
        let prepared = prepare_common_request_without_route_plan(CommonRequestPreparationParams {
            proxy: &proxy,
            config: &config,
            method: &method,
            uri: &uri,
            client_headers: &headers,
            raw_body: &raw_body,
            request_dialect: RequestDialect::ResponsesHttp,
            request_origin: RequestOrigin::Client,
            client_name: None,
            client_addr: None,
            started_at_ms: 4,
            client_content_type: Some("application/json"),
            request_body_previews: false,
        })
        .await
        .expect("prepared");

        let value: serde_json::Value =
            serde_json::from_slice(prepared.body_for_upstream.as_ref()).expect("json body");
        assert_eq!(value["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["tools"][0]["name"].as_str(), Some("shell"));
        assert_eq!(value["tool_choice"].as_str(), Some("auto"));
        assert!(prepared.translate_openai_models);
    }

    #[tokio::test]
    async fn prepare_common_request_filters_websocket_response_create() {
        let proxy = test_proxy_with_hosted_image_generation_disabled();
        let config = load_request_config_context(&proxy, None).await;
        let headers = HeaderMap::new();
        let method = Method::GET;
        let uri = "/v1/responses".parse::<Uri>().expect("uri");
        let raw_body = Bytes::from_static(
            br#"{"type":"response.create","model":"gpt-5","tools":[{"type":"image_generation"},{"type":"function","name":"shell"}]}"#,
        );

        let prepared = prepare_common_request_without_route_plan(CommonRequestPreparationParams {
            proxy: &proxy,
            config: &config,
            method: &method,
            uri: &uri,
            client_headers: &headers,
            raw_body: &raw_body,
            request_dialect: RequestDialect::ResponsesWebSocket,
            request_origin: RequestOrigin::Client,
            client_name: None,
            client_addr: None,
            started_at_ms: 5,
            client_content_type: Some("application/json"),
            request_body_previews: false,
        })
        .await
        .expect("prepared");

        let value: serde_json::Value =
            serde_json::from_slice(prepared.body_for_upstream.as_ref()).expect("json body");
        assert_eq!(value["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["tools"][0]["name"].as_str(), Some("shell"));
    }

    #[tokio::test]
    async fn client_runtime_marker_overrides_captured_patch_in_both_directions() {
        let method = Method::POST;
        let uri = "/v1/responses".parse::<Uri>().expect("uri");
        let raw_body = Bytes::from_static(
            br#"{"model":"gpt-5","tools":[{"type":"image_generation"},{"type":"function","name":"shell"}]}"#,
        );

        for (proxy, marker, expected_tools, expected_translation) in [
            (
                test_proxy_with_hosted_image_generation_disabled(),
                "v1;models=0;hosted=enabled",
                2,
                false,
            ),
            (
                test_proxy_with_active_route(),
                "v1;models=1;hosted=disabled",
                1,
                true,
            ),
        ] {
            let config = load_request_config_context(&proxy, None).await;
            let mut headers = HeaderMap::new();
            headers.insert(
                CODEX_CLIENT_RUNTIME_PATCH_HEADER,
                HeaderValue::from_str(marker).expect("runtime marker header"),
            );
            let prepared =
                prepare_common_request_without_route_plan(CommonRequestPreparationParams {
                    proxy: &proxy,
                    config: &config,
                    method: &method,
                    uri: &uri,
                    client_headers: &headers,
                    raw_body: &raw_body,
                    request_dialect: RequestDialect::ResponsesHttp,
                    request_origin: RequestOrigin::Client,
                    client_name: None,
                    client_addr: None,
                    started_at_ms: 7,
                    client_content_type: Some("application/json"),
                    request_body_previews: false,
                })
                .await
                .expect("prepare marker override");
            let value: serde_json::Value =
                serde_json::from_slice(prepared.body_for_upstream.as_ref())
                    .expect("parse prepared body");

            assert_eq!(
                value["tools"].as_array().map(Vec::len),
                Some(expected_tools)
            );
            assert_eq!(prepared.translate_openai_models, expected_translation);
        }
    }

    #[test]
    fn client_runtime_marker_rejects_duplicate_or_malformed_headers() {
        let mut headers = HeaderMap::new();
        headers.append(
            CODEX_CLIENT_RUNTIME_PATCH_HEADER,
            HeaderValue::from_static("v1;models=1;hosted=disabled"),
        );
        headers.append(
            CODEX_CLIENT_RUNTIME_PATCH_HEADER,
            HeaderValue::from_static("v1;models=0;hosted=enabled"),
        );
        assert_eq!(client_runtime_patch(&headers), None);

        headers.clear();
        headers.insert(
            CODEX_CLIENT_RUNTIME_PATCH_HEADER,
            HeaderValue::from_static("v2;models=1;hosted=disabled"),
        );
        assert_eq!(client_runtime_patch(&headers), None);
    }

    #[tokio::test]
    async fn prepare_common_request_preserves_images_compatibility_contract() {
        let proxy = test_proxy_with_hosted_image_generation_disabled();
        let config = load_request_config_context(&proxy, None).await;
        let headers = HeaderMap::new();
        let method = Method::POST;
        let uri = "/v1/responses".parse::<Uri>().expect("uri");
        let raw_body = Bytes::from_static(
            br#"{"model":"gpt-5","tools":[{"type":"image_generation","output_format":"png"}],"tool_choice":{"type":"image_generation"}}"#,
        );

        let prepared = prepare_common_request_without_route_plan(CommonRequestPreparationParams {
            proxy: &proxy,
            config: &config,
            method: &method,
            uri: &uri,
            client_headers: &headers,
            raw_body: &raw_body,
            request_dialect: RequestDialect::ResponsesHttp,
            request_origin: RequestOrigin::ImagesCompatibility,
            client_name: None,
            client_addr: None,
            started_at_ms: 6,
            client_content_type: Some("application/json"),
            request_body_previews: false,
        })
        .await
        .expect("prepared");

        let value: serde_json::Value =
            serde_json::from_slice(prepared.body_for_upstream.as_ref()).expect("json body");
        assert_eq!(value["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["tools"][0]["type"].as_str(), Some("image_generation"));
        assert_eq!(
            value["tool_choice"]["type"].as_str(),
            Some("image_generation")
        );
    }
}

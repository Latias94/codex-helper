use std::sync::Arc;

use reqwest::Client;

mod admin;
mod admin_api_error;
mod api_responses;
mod attempt_execution;
mod attempt_failures;
mod attempt_health;
mod attempt_request;
mod attempt_response;
mod attempt_transport;
mod classify;
mod client_identity;
mod codex_failure;
mod codex_relay_capabilities;
mod codex_relay_evidence;
mod codex_relay_live_smoke;
mod codex_relay_probe;
mod codex_relay_target;
mod concurrency_limits;
mod control_plane;
mod control_plane_manifest;
mod control_plane_routes;
mod control_plane_service;
mod entrypoint;
mod failure_summary;
mod headers;
mod http_debug;
mod local_operator_routes;
mod models_compat;
mod openai_images;
mod profile_defaults;
mod provider_evidence;
mod provider_execution;
mod providers_api;
mod reasoning_guard;
mod request_body;
mod request_context;
mod request_continuity;
mod request_encoding;
mod request_failures;
mod request_observer;
mod request_preparation;
mod response_entity;
mod response_finalization;
mod response_fixer;
mod response_semantics;
mod responses_websocket;
mod retry;
mod route_affinity;
mod route_attempts;
mod route_provenance;
mod route_target_selection;
mod route_unavailability;
mod router_setup;
mod routing_control;
mod runtime_admin_api;
mod runtime_config;
mod selected_upstream_request;
mod service_core;
mod session_affinity_control;
mod session_binding_control;
mod settings_control;
mod stream;
mod target_builder;
#[cfg(test)]
mod tests;

use crate::filter::RequestFilter;
use crate::state::{ProviderBalanceSnapshot, ProxyState};
use crate::usage_providers::UsageProviderRefreshSummary;

pub use self::admin::{
    admin_base_url_from_proxy_base_url, admin_loopback_addr_for_proxy_port,
    admin_port_for_proxy_port, local_admin_base_url_for_proxy_port, local_proxy_base_url,
};
pub use self::api_responses::ProfilesResponse;
pub use self::codex_relay_capabilities::{
    CodexRelayCapabilitiesObserved, CodexRelayCapabilitiesRequest, CodexRelayCapabilitiesResponse,
    CodexRelayCapabilityMismatch, CodexRelayContinuityDiagnostics,
    CodexRelayContinuityDomainSummary, CodexRelayProviderContract,
};
pub use self::codex_relay_evidence::{
    CodexRelayEvidenceEntry, CodexRelayEvidenceFilters, CodexRelayEvidenceKind,
    codex_relay_evidence_path, read_recent_codex_relay_evidence,
};
pub use self::codex_relay_live_smoke::{
    CODEX_RELAY_LIVE_SMOKE_ACK, CodexRelayLiveSmokeCase, CodexRelayLiveSmokeConfidence,
    CodexRelayLiveSmokeOutcome, CodexRelayLiveSmokeRequest, CodexRelayLiveSmokeResponse,
    CodexRelayLiveSmokeResult, CodexRelayLiveSmokeSideEffect,
};
pub(crate) use self::codex_relay_probe::CodexRelayProbeClient;
pub use self::codex_relay_probe::{
    CodexRelayProbeConfidence, CodexRelayProbeKind, CodexRelayProbeResult,
    CodexRelayProbeSideEffect, CodexRelayProbeSpec, CodexRelayProbeSupport,
    classify_codex_relay_probe_response,
};
use self::concurrency_limits::ConcurrencyLimiter;
pub(crate) use self::control_plane_manifest::{
    LOCAL_V1_BALANCE_REFRESH, LOCAL_V1_CREDENTIAL_REFRESH, LOCAL_V1_DEFAULT_PROFILE_MUTATION,
    LOCAL_V1_OPERATOR_SESSION, LOCAL_V1_RELAY_CAPABILITIES, LOCAL_V1_RELAY_LIVE_SMOKE,
    LOCAL_V1_ROUTING_MUTATION, LOCAL_V1_RUNTIME_RELOAD, LOCAL_V1_SERVICE_RUNTIME_READ,
    LOCAL_V1_SESSION_AFFINITY_MUTATION, LOCAL_V1_SESSION_BINDING_MUTATION,
    LOCAL_V1_SESSION_METADATA_READ,
};
pub(crate) use self::entrypoint::handle_proxy;
pub(crate) use self::local_operator_routes::{
    LOCAL_OPERATOR_NONCE_HEADER, LOCAL_OPERATOR_SESSION_HEADER, LOCAL_OPERATOR_SIGNATURE_HEADER,
    LOCAL_OPERATOR_TIMESTAMP_HEADER,
};
pub use self::response_entity::upstream_http_client_builder;
#[cfg(test)]
pub(crate) use self::router_setup::router;
pub(crate) use self::router_setup::{admin_listener_router, proxy_only_router};
pub use self::routing_control::{
    OperatorEndpointMode, OperatorRoutingCommand, OperatorRoutingMutationRequest,
    OperatorRoutingMutationResponse, OperatorRoutingMutationStatus,
};
use self::runtime_config::RuntimeConfig;
pub use self::session_affinity_control::{
    OperatorSessionAffinityCommand, OperatorSessionAffinityMutationRequest,
    OperatorSessionAffinityMutationResponse, OperatorSessionAffinityMutationStatus,
};
pub use self::session_binding_control::{
    OperatorSessionBindingCommand, OperatorSessionBindingMutationRequest,
    OperatorSessionBindingMutationResponse, OperatorSessionBindingMutationStatus,
};
pub use self::settings_control::{
    EffectiveDefaultProfileSource, OperatorDefaultProfileMutationRequest,
    OperatorDefaultProfileMutationResponse, OperatorDefaultProfileMutationStatus,
    OperatorDefaultProfileScope, OperatorRuntimeReloadRequest, OperatorRuntimeReloadResponse,
    RuntimeDefaultProfileControlSnapshot,
};

pub const ADMIN_TOKEN_ENV_VAR: &str = "CODEX_HELPER_ADMIN_TOKEN";
pub const ADMIN_TOKEN_HEADER: &str = "x-codex-helper-admin-token";
pub const CLIENT_NAME_HEADER: &str = "x-codex-helper-client-name";
pub const ADMIN_PORT_OFFSET: u16 = 1000;

#[cfg(test)]
fn claude_settings_env_value(key: &str) -> Option<String> {
    crate::auth_resolution::claude_settings_env_value(key)
}

/// Generic proxy service; currently used by both Codex and Claude.
#[derive(Clone)]
pub struct ProxyService {
    pub(crate) client: Client,
    config: Arc<RuntimeConfig>,
    pub service_name: &'static str,
    concurrency_limiter: Arc<ConcurrencyLimiter>,
    filter: RequestFilter,
    state: Arc<ProxyState>,
    service_install_generation: Option<crate::service_target::ServiceInstallGeneration>,
    service_runtime_identity: Option<crate::service_target::ServiceRuntimeIdentity>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderBalanceRefreshResponse {
    pub service_name: String,
    pub refresh: UsageProviderRefreshSummary,
    pub provider_balances: Vec<ProviderBalanceSnapshot>,
}

#[derive(Debug, Clone)]
pub struct ProxyControlError {
    status: axum::http::StatusCode,
    message: String,
}

impl ProxyControlError {
    pub fn new(status: axum::http::StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub fn status(&self) -> axum::http::StatusCode {
        self.status
    }

    pub fn message(&self) -> &str {
        self.message.as_str()
    }

    pub fn into_http_error(self) -> (axum::http::StatusCode, String) {
        (self.status, self.message)
    }
}

impl std::fmt::Display for ProxyControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "status={}, {}", self.status, self.message)
    }
}

impl std::error::Error for ProxyControlError {}

impl From<(axum::http::StatusCode, String)> for ProxyControlError {
    fn from((status, message): (axum::http::StatusCode, String)) -> Self {
        Self::new(status, message)
    }
}

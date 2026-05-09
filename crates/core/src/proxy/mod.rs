use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::Client;

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
mod control_plane_manifest;
mod control_plane_routes;
mod control_plane_service;
mod entrypoint;
mod failure_summary;
mod headers;
mod healthcheck_api;
mod http_debug;
mod passive_health;
mod persisted_registry_api;
mod profile_defaults;
mod provider_execution;
mod provider_orchestration;
mod providers_api;
mod request_body;
mod request_context;
mod request_failures;
mod request_preparation;
mod request_routing;
mod response_finalization;
mod retry;
mod route_attempts;
mod route_provenance;
mod router_setup;
mod routing_plan;
mod runtime_admin_api;
mod runtime_config;
mod selected_upstream_request;
mod service_core;
mod session_overrides;
mod stations_api;
mod stream;
mod target_builder;
#[cfg(test)]
mod tests;

use crate::filter::RequestFilter;
use crate::lb::LbState;
use crate::state::ProxyState;

pub use self::admin::{
    admin_base_url_from_proxy_base_url, admin_loopback_addr_for_proxy_port,
    admin_port_for_proxy_port, local_admin_base_url_for_proxy_port, local_proxy_base_url,
};
pub use self::entrypoint::handle_proxy;
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

#[cfg(test)]
fn codex_auth_json_value(key: &str) -> Option<String> {
    auth_resolution::codex_auth_json_value(key)
}

#[cfg(test)]
fn claude_settings_env_value(key: &str) -> Option<String> {
    auth_resolution::claude_settings_env_value(key)
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

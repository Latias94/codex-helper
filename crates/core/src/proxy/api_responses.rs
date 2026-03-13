use crate::config::ProxyConfig;
use crate::dashboard_core::{ControlProfileOption, build_profile_options_from_mgr};

use super::ProxyService;
use super::profile_defaults::effective_default_profile_name;

#[derive(serde::Serialize)]
pub(super) struct ProfilesResponse {
    default_profile: Option<String>,
    configured_default_profile: Option<String>,
    profiles: Vec<ControlProfileOption>,
}

#[derive(serde::Serialize)]
pub(super) struct RuntimeConfigStatus {
    config_path: String,
    loaded_at_ms: u64,
    source_mtime_ms: Option<u64>,
    retry: crate::config::ResolvedRetryConfig,
}

#[derive(serde::Serialize)]
pub(super) struct RetryConfigResponse {
    configured: crate::config::RetryConfig,
    resolved: crate::config::ResolvedRetryConfig,
}

#[derive(serde::Serialize)]
pub(super) struct ReloadResult {
    reloaded: bool,
    status: RuntimeConfigStatus,
}

pub(super) async fn make_profiles_response(proxy: &ProxyService) -> ProfilesResponse {
    let cfg = proxy.config.snapshot().await;
    let mgr = proxy.service_manager(cfg.as_ref());
    let default_profile =
        effective_default_profile_name(proxy.state.as_ref(), proxy.service_name, mgr).await;
    ProfilesResponse {
        default_profile: default_profile.clone(),
        configured_default_profile: mgr.default_profile.clone(),
        profiles: build_profile_options_from_mgr(mgr, default_profile.as_deref()),
    }
}

pub(super) async fn build_runtime_config_status(proxy: &ProxyService) -> RuntimeConfigStatus {
    let cfg = proxy.config.snapshot().await;
    RuntimeConfigStatus {
        config_path: crate::config::config_file_path().display().to_string(),
        loaded_at_ms: proxy.config.last_loaded_at_ms(),
        source_mtime_ms: proxy.config.last_mtime_ms().await,
        retry: cfg.retry.resolve(),
    }
}

pub(super) fn build_retry_config_response(cfg: &ProxyConfig) -> RetryConfigResponse {
    RetryConfigResponse {
        configured: cfg.retry.clone(),
        resolved: cfg.retry.resolve(),
    }
}

pub(super) fn build_reload_result(reloaded: bool, status: RuntimeConfigStatus) -> ReloadResult {
    ReloadResult { reloaded, status }
}

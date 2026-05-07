use crate::config::{ProxyConfig, resolve_service_profile};
use crate::dashboard_core::{
    ApiV1OperatorSummary, ControlPlaneSurfaceCapabilities, ControlProfileOption,
    HostLocalControlPlaneCapabilities, OperatorProfileSummary, OperatorRetrySummary,
    OperatorRuntimeSummary, OperatorSummaryCounts, RemoteAdminAccessCapabilities,
    SharedControlPlaneCapabilities, build_operator_health_summary, build_profile_options_from_mgr,
    build_provider_options_from_view, build_station_options_from_mgr,
    summarize_recent_retry_observations,
};
use crate::state::{SessionIdentityCardBuildInputs, build_session_identity_cards_from_parts};

use super::ProxyService;
use super::control_plane_manifest::api_v1_operator_summary_links;
use super::control_plane_service::{load_persisted_proxy_settings_v2, service_view_v2};
use super::profile_defaults::{
    configured_active_station_name, effective_active_station_name, effective_default_profile_name,
};

#[derive(serde::Serialize)]
pub(super) struct ProfilesResponse {
    default_profile: Option<String>,
    configured_default_profile: Option<String>,
    profiles: Vec<ControlProfileOption>,
}

#[derive(serde::Serialize)]
pub(super) struct RuntimeStatusResponse {
    runtime_source_path: String,
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
    status: RuntimeStatusResponse,
}

pub(super) async fn build_operator_summary(
    proxy: &ProxyService,
    surface_capabilities: ControlPlaneSurfaceCapabilities,
    shared_capabilities: SharedControlPlaneCapabilities,
    host_local_capabilities: HostLocalControlPlaneCapabilities,
    remote_admin_access: RemoteAdminAccessCapabilities,
) -> ApiV1OperatorSummary {
    let cfg = proxy.config.snapshot().await;
    let mgr = proxy.service_manager(cfg.as_ref());
    let configured_active_station = configured_active_station_name(mgr);
    let effective_active_station = effective_active_station_name(mgr);
    let configured_default_profile = mgr.default_profile.clone();
    let configured_retry = cfg.retry.clone();
    let resolved_retry = configured_retry.resolve();
    let loaded_at_ms = proxy.config.last_loaded_at_ms();
    let source_mtime_ms = proxy.config.last_mtime_ms().await;
    let (
        active,
        recent,
        global_station_override,
        session_model,
        session_station,
        session_effort,
        session_service_tier,
        session_bindings,
        session_stats,
        default_profile,
        station_meta_overrides,
        station_state_overrides,
        provider_upstream_overrides,
        station_health,
        health_checks,
        lb_view,
    ) = tokio::join!(
        proxy.state.list_active_requests(),
        proxy.state.list_recent_finished(200),
        proxy.state.get_global_station_override(),
        proxy.state.list_session_model_overrides(),
        proxy.state.list_session_station_overrides(),
        proxy.state.list_session_effort_overrides(),
        proxy.state.list_session_service_tier_overrides(),
        proxy.state.list_session_bindings(),
        proxy.state.list_session_stats(),
        effective_default_profile_name(proxy.state.as_ref(), proxy.service_name, mgr),
        proxy.state.get_station_meta_overrides(proxy.service_name),
        proxy
            .state
            .get_station_runtime_state_overrides(proxy.service_name),
        proxy.state.get_upstream_meta_overrides(proxy.service_name),
        proxy.state.get_station_health(proxy.service_name),
        proxy.state.list_health_checks(proxy.service_name),
        proxy.state.get_lb_view(),
    );
    let session_cards = build_session_identity_cards_from_parts(SessionIdentityCardBuildInputs {
        active: &active,
        recent: &recent,
        overrides: &session_effort,
        station_overrides: &session_station,
        model_overrides: &session_model,
        service_tier_overrides: &session_service_tier,
        bindings: &session_bindings,
        global_station_override: global_station_override.as_deref(),
        stats: &session_stats,
    });
    let default_profile_summary = default_profile.as_deref().and_then(|profile_name| {
        resolve_service_profile(mgr, profile_name)
            .ok()
            .map(|profile| OperatorProfileSummary {
                name: profile_name.to_string(),
                station: profile.station,
                model: profile.model,
                reasoning_effort: profile.reasoning_effort,
                service_tier: profile.service_tier.clone(),
                fast_mode: profile.service_tier.as_deref() == Some("priority"),
            })
    });
    let stations =
        build_station_options_from_mgr(mgr, &station_meta_overrides, &station_state_overrides);
    let profiles = build_profile_options_from_mgr(mgr, default_profile.as_deref());
    let health =
        build_operator_health_summary(&stations, &station_health, &health_checks, &lb_view);
    let providers = load_persisted_proxy_settings_v2()
        .await
        .ok()
        .map(|persisted_cfg| {
            build_provider_options_from_view(
                service_view_v2(&persisted_cfg, proxy.service_name),
                &provider_upstream_overrides,
            )
        })
        .unwrap_or_default();
    let retry_observations = summarize_recent_retry_observations(&recent);

    ApiV1OperatorSummary {
        api_version: 1,
        service_name: proxy.service_name.to_string(),
        runtime: OperatorRuntimeSummary {
            runtime_loaded_at_ms: Some(loaded_at_ms),
            runtime_source_mtime_ms: source_mtime_ms,
            configured_active_station,
            effective_active_station,
            global_station_override,
            configured_default_profile,
            default_profile,
            default_profile_summary,
        },
        counts: OperatorSummaryCounts {
            active_requests: active.len(),
            recent_requests: recent.len(),
            sessions: session_cards.len(),
            stations: mgr.stations().len(),
            profiles: mgr.profiles.len(),
            providers: providers.len(),
        },
        retry: OperatorRetrySummary {
            configured_profile: configured_retry.profile,
            supports_write: surface_capabilities.retry_config,
            upstream_max_attempts: resolved_retry.upstream.max_attempts,
            provider_max_attempts: resolved_retry.provider.max_attempts,
            allow_cross_station_before_first_output: resolved_retry
                .allow_cross_station_before_first_output,
            recent_retried_requests: retry_observations.recent_retried_requests,
            recent_cross_station_failovers: retry_observations.recent_cross_station_failovers,
            recent_same_station_retries: retry_observations.recent_same_station_retries,
            recent_fast_mode_requests: retry_observations.recent_fast_mode_requests,
        },
        health: Some(health),
        session_cards,
        stations: stations.clone(),
        profiles,
        providers,
        links: Some(api_v1_operator_summary_links()),
        surface_capabilities,
        shared_capabilities,
        host_local_capabilities,
        remote_admin_access,
    }
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

pub(super) async fn build_runtime_status_response(proxy: &ProxyService) -> RuntimeStatusResponse {
    let cfg = proxy.config.snapshot().await;
    let runtime_source_path = crate::config::config_file_path().display().to_string();
    RuntimeStatusResponse {
        runtime_source_path: runtime_source_path.clone(),
        config_path: runtime_source_path,
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

pub(super) fn build_reload_result(reloaded: bool, status: RuntimeStatusResponse) -> ReloadResult {
    ReloadResult { reloaded, status }
}

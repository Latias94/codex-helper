use axum::Json;
use axum::extract::Query;
use axum::http::StatusCode;

use crate::dashboard_core::{
    ApiV1Capabilities, ApiV1OperatorSummary, ApiV1Snapshot, HostLocalControlPlaneCapabilities,
    SharedControlPlaneCapabilities, build_profile_options_from_mgr,
};

use super::super::ProxyService;
use super::super::admin::admin_access_capabilities;
use super::super::api_responses::build_operator_summary;
use super::super::control_plane_manifest::{api_v1_endpoint_paths, api_v1_surface_capabilities};
use super::super::profile_defaults::{
    configured_active_station_name, effective_active_station_name, effective_default_profile_name,
};
use super::{SnapshotQuery, host_local_session_history_available};

async fn api_v1_surface_capabilities_for_proxy(
    proxy: &ProxyService,
) -> crate::dashboard_core::ControlPlaneSurfaceCapabilities {
    let mut surface = api_v1_surface_capabilities();
    let cfg = proxy.config.snapshot().await;
    if cfg.version == Some(3) {
        surface.station_persisted_settings = false;
        surface.station_specs = false;
    }
    surface
}

pub(in crate::proxy) async fn api_capabilities(
    proxy: ProxyService,
) -> Result<Json<ApiV1Capabilities>, (StatusCode, String)> {
    let host_local_history = host_local_session_history_available();
    let surface_capabilities = api_v1_surface_capabilities_for_proxy(&proxy).await;
    Ok(Json(ApiV1Capabilities {
        api_version: 1,
        service_name: proxy.service_name.to_string(),
        endpoints: api_v1_endpoint_paths(),
        surface_capabilities,
        shared_capabilities: SharedControlPlaneCapabilities {
            session_observability: true,
            request_history: true,
        },
        host_local_capabilities: HostLocalControlPlaneCapabilities {
            session_history: host_local_history,
            transcript_read: host_local_history,
            cwd_enrichment: host_local_history,
        },
        remote_admin_access: admin_access_capabilities(),
    }))
}

pub(in crate::proxy) async fn api_v1_snapshot(
    proxy: ProxyService,
    Query(query): Query<SnapshotQuery>,
) -> Result<Json<ApiV1Snapshot>, (StatusCode, String)> {
    let recent_limit = query.recent_limit.unwrap_or(200).clamp(1, 2_000);
    let stats_days = query.stats_days.unwrap_or(21).clamp(1, 365);

    let cfg = proxy.config.snapshot().await;
    let mgr = proxy.service_manager(cfg.as_ref());
    let meta_overrides = proxy
        .state
        .get_station_meta_overrides(proxy.service_name)
        .await;
    let state_overrides = proxy
        .state
        .get_station_runtime_state_overrides(proxy.service_name)
        .await;
    let stations = crate::dashboard_core::build_station_options_from_mgr(
        mgr,
        &meta_overrides,
        &state_overrides,
    );
    let configured_active_station = configured_active_station_name(mgr);
    let effective_active_station = effective_active_station_name(mgr);
    let default_profile =
        effective_default_profile_name(proxy.state.as_ref(), proxy.service_name, mgr).await;

    let mut snapshot = crate::dashboard_core::build_dashboard_snapshot(
        &proxy.state,
        proxy.service_name,
        recent_limit,
        stats_days,
    )
    .await;
    crate::state::enrich_session_identity_cards_with_runtime(&mut snapshot.session_cards, mgr);

    Ok(Json(ApiV1Snapshot {
        api_version: 1,
        service_name: proxy.service_name.to_string(),
        runtime_loaded_at_ms: Some(proxy.config.last_loaded_at_ms()),
        runtime_source_mtime_ms: proxy.config.last_mtime_ms().await,
        stations,
        configured_active_station,
        effective_active_station,
        default_profile: default_profile.clone(),
        profiles: build_profile_options_from_mgr(mgr, default_profile.as_deref()),
        snapshot,
    }))
}

pub(in crate::proxy) async fn api_operator_summary(
    proxy: ProxyService,
) -> Result<Json<ApiV1OperatorSummary>, (StatusCode, String)> {
    let surface_capabilities = api_v1_surface_capabilities_for_proxy(&proxy).await;
    let host_local_history = host_local_session_history_available();
    let shared_capabilities = SharedControlPlaneCapabilities {
        session_observability: true,
        request_history: true,
    };
    let host_local_capabilities = HostLocalControlPlaneCapabilities {
        session_history: host_local_history,
        transcript_read: host_local_history,
        cwd_enrichment: host_local_history,
    };
    let remote_admin_access = admin_access_capabilities();
    Ok(Json(
        build_operator_summary(
            &proxy,
            surface_capabilities,
            shared_capabilities,
            host_local_capabilities,
            remote_admin_access,
        )
        .await,
    ))
}

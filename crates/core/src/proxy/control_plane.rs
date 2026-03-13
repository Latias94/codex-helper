use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;

use crate::dashboard_core::{
    ApiV1Capabilities, HostLocalControlPlaneCapabilities, SharedControlPlaneCapabilities,
    build_profile_options_from_mgr,
};
use crate::logging::now_ms;
use crate::state::{ActiveRequest, FinishedRequest};

use super::ProxyService;
use super::admin::admin_access_capabilities;
use super::profile_defaults::{
    configured_active_station_name, effective_active_station_name, effective_default_profile_name,
};

#[derive(serde::Deserialize)]
pub(super) struct SessionProfileApplyRequest {
    session_id: String,
    profile_name: Option<String>,
}

#[derive(serde::Deserialize)]
pub(super) struct DefaultProfileRequest {
    profile_name: Option<String>,
}

#[derive(serde::Deserialize)]
pub(super) struct GlobalStationOverrideRequest {
    #[serde(default)]
    station_name: Option<String>,
}

#[derive(serde::Deserialize)]
pub(super) struct RecentQuery {
    limit: Option<usize>,
}

#[derive(serde::Deserialize)]
pub(super) struct SnapshotQuery {
    recent_limit: Option<usize>,
    stats_days: Option<usize>,
}

fn require_session_id(session_id: &str) -> Result<(), (StatusCode, String)> {
    if session_id.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "session_id is required".to_string(),
        ));
    }
    Ok(())
}

fn host_local_session_history_available() -> bool {
    let sessions_dir = crate::config::codex_sessions_dir();
    std::fs::metadata(sessions_dir)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

pub(super) async fn set_default_profile(
    proxy: ProxyService,
    Json(payload): Json<DefaultProfileRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let profile_name = payload
        .profile_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    if let Some(profile_name) = profile_name {
        let cfg = proxy.config.snapshot().await;
        let mgr = proxy.service_manager(cfg.as_ref());
        if mgr.profile(profile_name.as_str()).is_none() {
            return Err((
                StatusCode::NOT_FOUND,
                format!("profile '{}' not found", profile_name),
            ));
        }
        let resolved = crate::config::resolve_service_profile(mgr, profile_name.as_str())
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        crate::config::validate_profile_station_compatibility(
            proxy.service_name,
            mgr,
            profile_name.as_str(),
            &resolved,
        )
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        proxy
            .state
            .set_runtime_default_profile_override(
                proxy.service_name.to_string(),
                profile_name,
                now_ms(),
            )
            .await;
    } else {
        proxy
            .state
            .clear_runtime_default_profile_override(proxy.service_name)
            .await;
    }

    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn apply_session_profile(
    proxy: ProxyService,
    Json(payload): Json<SessionProfileApplyRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_session_id(payload.session_id.as_str())?;
    let profile_name = payload
        .profile_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    if profile_name.is_none() {
        proxy
            .state
            .clear_session_binding(payload.session_id.as_str())
            .await;
        return Ok(StatusCode::NO_CONTENT);
    }

    let cfg = proxy.config.snapshot().await;
    let mgr = proxy.service_manager(cfg.as_ref());
    let profile_name = profile_name.expect("profile_name checked above");
    if mgr.profile(profile_name.as_str()).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("profile '{}' not found", profile_name),
        ));
    }

    if let Err(err) = proxy
        .state
        .apply_session_profile_binding(
            proxy.service_name,
            mgr,
            payload.session_id,
            profile_name,
            now_ms(),
        )
        .await
    {
        return Err((StatusCode::BAD_REQUEST, err.to_string()));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn get_global_station_override(
    proxy: ProxyService,
) -> Result<Json<Option<String>>, (StatusCode, String)> {
    Ok(Json(proxy.state.get_global_station_override().await))
}

pub(super) async fn set_global_station_override(
    proxy: ProxyService,
    Json(payload): Json<GlobalStationOverrideRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if let Some(station_name) = payload.station_name {
        if station_name.trim().is_empty() {
            return Err((StatusCode::BAD_REQUEST, "station_name is empty".to_string()));
        }
        proxy
            .state
            .set_global_station_override(station_name, now_ms())
            .await;
    } else {
        proxy.state.clear_global_station_override().await;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn list_active_requests(
    proxy: ProxyService,
) -> Result<Json<Vec<ActiveRequest>>, (StatusCode, String)> {
    let vec = proxy.state.list_active_requests().await;
    Ok(Json(vec))
}

pub(super) async fn list_session_stats(
    proxy: ProxyService,
) -> Result<Json<HashMap<String, crate::state::SessionStats>>, (StatusCode, String)> {
    let map = proxy.state.list_session_stats().await;
    Ok(Json(map))
}

async fn load_session_identity_cards(
    proxy: &ProxyService,
) -> Vec<crate::state::SessionIdentityCard> {
    let mut cards = proxy
        .state
        .list_session_identity_cards_with_host_transcripts(2_000)
        .await;
    let cfg = proxy.config.snapshot().await;
    let mgr = proxy.service_manager(cfg.as_ref());
    crate::state::enrich_session_identity_cards_with_runtime(&mut cards, mgr);
    cards
}

pub(super) async fn list_session_identity_cards(
    proxy: ProxyService,
) -> Result<Json<Vec<crate::state::SessionIdentityCard>>, (StatusCode, String)> {
    Ok(Json(load_session_identity_cards(&proxy).await))
}

pub(super) async fn get_session_identity_card(
    proxy: ProxyService,
    Path(session_id): Path<String>,
) -> Result<Json<crate::state::SessionIdentityCard>, (StatusCode, String)> {
    require_session_id(session_id.as_str())?;
    let cards = load_session_identity_cards(&proxy).await;
    cards
        .into_iter()
        .find(|card| card.session_id.as_deref() == Some(session_id.as_str()))
        .map(Json)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("session '{}' not found", session_id),
            )
        })
}

pub(super) async fn list_recent_finished(
    proxy: ProxyService,
    Query(q): Query<RecentQuery>,
) -> Result<Json<Vec<FinishedRequest>>, (StatusCode, String)> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let vec = proxy.state.list_recent_finished(limit).await;
    Ok(Json(vec))
}

pub(super) async fn api_capabilities(
    proxy: ProxyService,
) -> Result<Json<ApiV1Capabilities>, (StatusCode, String)> {
    let host_local_history = host_local_session_history_available();
    Ok(Json(ApiV1Capabilities {
        api_version: 1,
        service_name: proxy.service_name.to_string(),
        endpoints: vec![
            "/__codex_helper/api/v1/capabilities",
            "/__codex_helper/api/v1/snapshot",
            "/__codex_helper/api/v1/sessions",
            "/__codex_helper/api/v1/sessions/{session_id}",
            "/__codex_helper/api/v1/status/active",
            "/__codex_helper/api/v1/status/recent",
            "/__codex_helper/api/v1/status/session-stats",
            "/__codex_helper/api/v1/status/health-checks",
            "/__codex_helper/api/v1/status/station-health",
            "/__codex_helper/api/v1/runtime/status",
            "/__codex_helper/api/v1/runtime/reload",
            "/__codex_helper/api/v1/retry/config",
            "/__codex_helper/api/v1/stations",
            "/__codex_helper/api/v1/stations/runtime",
            "/__codex_helper/api/v1/stations/config-active",
            "/__codex_helper/api/v1/stations/probe",
            "/__codex_helper/api/v1/stations/{name}",
            "/__codex_helper/api/v1/stations/specs",
            "/__codex_helper/api/v1/stations/specs/{name}",
            "/__codex_helper/api/v1/providers/specs",
            "/__codex_helper/api/v1/providers/specs/{name}",
            "/__codex_helper/api/v1/profiles",
            "/__codex_helper/api/v1/profiles/default",
            "/__codex_helper/api/v1/profiles/default/persisted",
            "/__codex_helper/api/v1/profiles/{name}",
            "/__codex_helper/api/v1/overrides/session",
            "/__codex_helper/api/v1/overrides/session/profile",
            "/__codex_helper/api/v1/overrides/session/model",
            "/__codex_helper/api/v1/overrides/session/effort",
            "/__codex_helper/api/v1/overrides/session/station",
            "/__codex_helper/api/v1/overrides/session/service-tier",
            "/__codex_helper/api/v1/overrides/session/reset",
            "/__codex_helper/api/v1/overrides/global-station",
            "/__codex_helper/api/v1/healthcheck/start",
            "/__codex_helper/api/v1/healthcheck/cancel",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
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

pub(super) async fn api_v1_snapshot(
    proxy: ProxyService,
    Query(q): Query<SnapshotQuery>,
) -> Result<Json<crate::dashboard_core::ApiV1Snapshot>, (StatusCode, String)> {
    let recent_limit = q.recent_limit.unwrap_or(200).clamp(1, 2_000);
    let stats_days = q.stats_days.unwrap_or(21).clamp(1, 365);

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

    Ok(Json(crate::dashboard_core::ApiV1Snapshot {
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

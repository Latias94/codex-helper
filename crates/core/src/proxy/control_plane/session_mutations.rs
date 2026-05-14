use axum::Json;
use axum::http::StatusCode;

use crate::config::is_supported_route_graph_config_version;
use crate::logging::now_ms;

use super::super::ProxyService;
use super::{
    DefaultProfileRequest, GlobalStationOverrideRequest, SessionProfileApplyRequest,
    require_session_id,
};

pub(in crate::proxy) async fn set_default_profile(
    proxy: ProxyService,
    Json(payload): Json<DefaultProfileRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    proxy
        .set_runtime_default_profile(payload.profile_name)
        .await
        .map_err(super::super::ProxyControlError::into_http_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(in crate::proxy) async fn apply_session_profile(
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

pub(in crate::proxy) async fn get_global_station_override(
    proxy: ProxyService,
) -> Result<Json<Option<String>>, (StatusCode, String)> {
    Ok(Json(proxy.state.get_global_station_override().await))
}

pub(in crate::proxy) async fn set_global_station_override(
    proxy: ProxyService,
    Json(payload): Json<GlobalStationOverrideRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if let Some(station_name) = payload.station_name {
        if station_name.trim().is_empty() {
            return Err((StatusCode::BAD_REQUEST, "station_name is empty".to_string()));
        }
        let cfg = proxy.config.snapshot().await;
        if cfg
            .version
            .is_some_and(is_supported_route_graph_config_version)
        {
            return Err((
                StatusCode::BAD_REQUEST,
                "route graph configs do not support global station overrides; use routing/provider endpoint controls instead"
                    .to_string(),
            ));
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

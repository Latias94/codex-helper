use axum::Json;
use axum::http::StatusCode;

use crate::logging::now_ms;
use crate::state::RuntimeConfigState;

use super::ProxyService;

#[derive(serde::Deserialize)]
pub(super) struct StationRuntimeMetaRequest {
    #[serde(default)]
    station_name: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    level: Option<u8>,
    #[serde(default)]
    clear_enabled: bool,
    #[serde(default)]
    clear_level: bool,
    #[serde(default)]
    runtime_state: Option<RuntimeConfigState>,
    #[serde(default)]
    clear_runtime_state: bool,
}

impl StationRuntimeMetaRequest {
    fn target_name(&self) -> Result<&str, (StatusCode, String)> {
        self.station_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or((
                StatusCode::BAD_REQUEST,
                "station_name is required".to_string(),
            ))
    }
}

pub(super) async fn apply_station_runtime_meta(
    proxy: ProxyService,
    Json(payload): Json<StationRuntimeMetaRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let station_name = payload.target_name()?.to_string();

    if payload.enabled.is_none()
        && payload.level.is_none()
        && !payload.clear_enabled
        && !payload.clear_level
        && payload.runtime_state.is_none()
        && !payload.clear_runtime_state
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "at least one runtime station action must be provided".to_string(),
        ));
    }

    let cfg = proxy.config.snapshot().await;
    let mgr = proxy.service_manager(cfg.as_ref());
    if !mgr.contains_station(station_name.as_str()) {
        return Err((
            StatusCode::NOT_FOUND,
            format!("station '{}' not found", station_name),
        ));
    }

    let now = now_ms();
    if payload.clear_enabled {
        proxy
            .state
            .clear_station_enabled_override(proxy.service_name, station_name.as_str())
            .await;
    } else if let Some(enabled) = payload.enabled {
        proxy
            .state
            .set_station_enabled_override(proxy.service_name, station_name.clone(), enabled, now)
            .await;
    }

    if payload.clear_level {
        proxy
            .state
            .clear_station_level_override(proxy.service_name, station_name.as_str())
            .await;
    } else if let Some(level) = payload.level {
        proxy
            .state
            .set_station_level_override(
                proxy.service_name,
                station_name.clone(),
                level.clamp(1, 10),
                now,
            )
            .await;
    }

    if payload.clear_runtime_state {
        proxy
            .state
            .clear_station_runtime_state_override(proxy.service_name, station_name.as_str())
            .await;
    } else if let Some(runtime_state) = payload.runtime_state {
        proxy
            .state
            .set_station_runtime_state_override(
                proxy.service_name,
                station_name.clone(),
                runtime_state,
                now,
            )
            .await;
    }

    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn list_stations(
    proxy: ProxyService,
) -> Result<Json<Vec<crate::dashboard_core::StationOption>>, (StatusCode, String)> {
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
    Ok(Json(crate::dashboard_core::build_station_options_from_mgr(
        mgr,
        &meta_overrides,
        &state_overrides,
    )))
}

use std::collections::HashMap;

use axum::Json;
use axum::http::StatusCode;

use crate::logging::now_ms;

use super::ProxyService;

#[derive(Debug, serde::Deserialize)]
pub(super) struct HealthCheckAction {
    #[serde(default)]
    all: bool,
    #[serde(default)]
    station_names: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
pub(super) struct StationProbeRequest {
    #[serde(default)]
    station_name: Option<String>,
}

impl StationProbeRequest {
    fn station_name(&self) -> Result<String, (StatusCode, String)> {
        self.station_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .ok_or((
                StatusCode::BAD_REQUEST,
                "station_name is required".to_string(),
            ))
    }
}

#[derive(Debug, serde::Serialize)]
pub(super) struct HealthCheckActionResult {
    started: Vec<String>,
    already_running: Vec<String>,
    missing: Vec<String>,
    cancel_requested: Vec<String>,
    not_running: Vec<String>,
}

pub(super) async fn list_health_checks(
    proxy: ProxyService,
) -> Result<Json<HashMap<String, crate::state::HealthCheckStatus>>, (StatusCode, String)> {
    let map = proxy.state.list_health_checks(proxy.service_name).await;
    Ok(Json(map))
}

pub(super) async fn list_station_health(
    proxy: ProxyService,
) -> Result<Json<HashMap<String, crate::state::StationHealth>>, (StatusCode, String)> {
    let map = proxy.state.get_station_health(proxy.service_name).await;
    Ok(Json(map))
}

async fn spawn_health_checks_for_targets(
    proxy: &ProxyService,
    targets: Vec<(String, Vec<crate::config::UpstreamConfig>)>,
) -> HealthCheckActionResult {
    let mut started = Vec::new();
    let mut already_running = Vec::new();
    for (name, upstreams) in targets {
        let now = now_ms();
        if !proxy
            .state
            .try_begin_station_health_check(proxy.service_name, &name, upstreams.len(), now)
            .await
        {
            already_running.push(name);
            continue;
        }

        proxy
            .state
            .record_station_health(
                proxy.service_name,
                name.clone(),
                crate::state::StationHealth {
                    checked_at_ms: now,
                    upstreams: Vec::new(),
                },
            )
            .await;

        let state = proxy.state.clone();
        let service_name = proxy.service_name;
        let station_name = name.clone();
        tokio::spawn(async move {
            crate::healthcheck::run_health_check_for_station(
                state,
                service_name,
                station_name,
                upstreams,
            )
            .await;
        });
        started.push(name);
    }

    HealthCheckActionResult {
        started,
        already_running,
        missing: Vec::new(),
        cancel_requested: Vec::new(),
        not_running: Vec::new(),
    }
}

pub(super) async fn start_health_checks(
    proxy: ProxyService,
    Json(payload): Json<HealthCheckAction>,
) -> Result<Json<HealthCheckActionResult>, (StatusCode, String)> {
    let cfg = proxy.config.snapshot().await;
    let mgr = proxy.service_manager(cfg.as_ref());

    let mut targets = if payload.all {
        mgr.stations().keys().cloned().collect::<Vec<_>>()
    } else {
        payload.station_names
    };
    targets.retain(|s| !s.trim().is_empty());
    targets.sort();
    targets.dedup();
    if targets.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "expected { all: true } or non-empty station_names".to_string(),
        ));
    }

    let mut missing = Vec::new();
    let mut resolved_targets = Vec::new();
    for name in targets {
        let Some(svc) = mgr.station(&name) else {
            missing.push(name);
            continue;
        };
        resolved_targets.push((name, svc.upstreams.clone()));
    }

    let mut result = spawn_health_checks_for_targets(&proxy, resolved_targets).await;
    result.missing = missing;
    Ok(Json(result))
}

pub(super) async fn probe_station(
    proxy: ProxyService,
    Json(payload): Json<StationProbeRequest>,
) -> Result<Json<HealthCheckActionResult>, (StatusCode, String)> {
    let cfg = proxy.config.snapshot().await;
    let mgr = proxy.service_manager(cfg.as_ref());

    let station_name = payload.station_name()?;
    let Some(station) = mgr.station(&station_name) else {
        return Err((
            StatusCode::NOT_FOUND,
            format!("station '{}' not found", station_name),
        ));
    };

    let result =
        spawn_health_checks_for_targets(&proxy, vec![(station_name, station.upstreams.clone())])
            .await;
    Ok(Json(result))
}

pub(super) async fn cancel_health_checks(
    proxy: ProxyService,
    Json(payload): Json<HealthCheckAction>,
) -> Result<Json<HealthCheckActionResult>, (StatusCode, String)> {
    let cfg = proxy.config.snapshot().await;
    let mgr = proxy.service_manager(cfg.as_ref());

    let mut targets = if payload.all {
        mgr.stations().keys().cloned().collect::<Vec<_>>()
    } else {
        payload.station_names
    };
    targets.retain(|s| !s.trim().is_empty());
    targets.sort();
    targets.dedup();
    if targets.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "expected { all: true } or non-empty station_names".to_string(),
        ));
    }

    let now = now_ms();
    let mut cancel_requested = Vec::new();
    let mut not_running = Vec::new();
    let mut missing = Vec::new();
    for name in targets {
        if !mgr.contains_station(&name) {
            missing.push(name);
            continue;
        }
        let ok = proxy
            .state
            .request_cancel_station_health_check(proxy.service_name, &name, now)
            .await;
        if ok {
            cancel_requested.push(name);
        } else {
            not_running.push(name);
        }
    }

    Ok(Json(HealthCheckActionResult {
        started: Vec::new(),
        already_running: Vec::new(),
        missing,
        cancel_requested,
        not_running,
    }))
}

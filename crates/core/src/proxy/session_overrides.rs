use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::http::StatusCode;

use super::ProxyService;

#[derive(serde::Deserialize)]
pub(super) struct SessionReasoningEffortOverrideRequest {
    session_id: String,
    #[serde(default, alias = "effort")]
    reasoning_effort: Option<String>,
}

#[derive(serde::Deserialize)]
pub(super) struct SessionStationOverrideRequest {
    session_id: String,
    #[serde(default)]
    station_name: Option<String>,
}

#[derive(serde::Deserialize)]
pub(super) struct SessionModelOverrideRequest {
    session_id: String,
    model: Option<String>,
}

#[derive(serde::Deserialize)]
pub(super) struct SessionServiceTierOverrideRequest {
    session_id: String,
    service_tier: Option<String>,
}

#[derive(Debug, Clone, Copy, serde::Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub(super) enum SessionOverrideDimension {
    Model,
    ReasoningEffort,
    StationName,
    ServiceTier,
    All,
}

#[derive(serde::Deserialize)]
pub(super) struct SessionManualOverridesPatchRequest {
    session_id: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default, alias = "effort")]
    reasoning_effort: Option<String>,
    #[serde(default)]
    station_name: Option<String>,
    #[serde(default)]
    service_tier: Option<String>,
    #[serde(default)]
    clear: Vec<SessionOverrideDimension>,
}

#[derive(serde::Deserialize)]
pub(super) struct SessionOverrideResetRequest {
    session_id: String,
}

#[derive(serde::Serialize)]
pub(super) struct SessionOverridePrecedence {
    request_fields_apply_order: Vec<&'static str>,
    station_apply_order: Vec<&'static str>,
}

#[derive(serde::Serialize)]
pub(super) struct SessionManualOverridesListResponse {
    precedence: SessionOverridePrecedence,
    sessions: HashMap<String, crate::state::SessionManualOverrides>,
}

#[derive(serde::Serialize)]
pub(super) struct SessionManualOverridesResponse {
    session_id: String,
    overrides: crate::state::SessionManualOverrides,
    precedence: SessionOverridePrecedence,
}

fn normalize_session_override_value(
    field_name: &str,
    value: Option<String>,
) -> Result<Option<String>, (StatusCode, String)> {
    match value {
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Err((StatusCode::BAD_REQUEST, format!("{field_name} is empty")))
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        None => Ok(None),
    }
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

fn session_override_precedence() -> SessionOverridePrecedence {
    SessionOverridePrecedence {
        request_fields_apply_order: vec![
            "session_override",
            "profile_default",
            "request_payload",
            "station_mapping",
            "runtime_fallback",
        ],
        station_apply_order: vec![
            "session_override",
            "global_station_override",
            "profile_default",
            "runtime_fallback",
        ],
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(super) async fn set_session_reasoning_effort_override(
    proxy: ProxyService,
    Json(payload): Json<SessionReasoningEffortOverrideRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_session_id(payload.session_id.as_str())?;
    let reasoning_effort =
        normalize_session_override_value("reasoning_effort", payload.reasoning_effort)?;
    if let Some(reasoning_effort) = reasoning_effort {
        proxy
            .state
            .set_session_reasoning_effort_override(payload.session_id, reasoning_effort, now_ms())
            .await;
    } else {
        proxy
            .state
            .clear_session_reasoning_effort_override(payload.session_id.as_str())
            .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn list_session_reasoning_effort_overrides(
    proxy: ProxyService,
) -> Result<Json<HashMap<String, String>>, (StatusCode, String)> {
    let map = proxy.state.list_session_reasoning_effort_overrides().await;
    Ok(Json(map))
}

pub(super) async fn list_session_manual_overrides(
    proxy: ProxyService,
) -> Result<Json<SessionManualOverridesListResponse>, (StatusCode, String)> {
    let sessions = proxy.state.list_session_manual_overrides().await;
    Ok(Json(SessionManualOverridesListResponse {
        precedence: session_override_precedence(),
        sessions,
    }))
}

pub(super) async fn apply_session_manual_overrides(
    proxy: ProxyService,
    Json(payload): Json<SessionManualOverridesPatchRequest>,
) -> Result<Json<SessionManualOverridesResponse>, (StatusCode, String)> {
    require_session_id(payload.session_id.as_str())?;
    let model = normalize_session_override_value("model", payload.model)?;
    let reasoning_effort =
        normalize_session_override_value("reasoning_effort", payload.reasoning_effort)?;
    let station_name = normalize_session_override_value("station_name", payload.station_name)?;
    let service_tier = normalize_session_override_value("service_tier", payload.service_tier)?;
    let clear: HashSet<_> = payload.clear.into_iter().collect();
    if model.is_none()
        && reasoning_effort.is_none()
        && station_name.is_none()
        && service_tier.is_none()
        && clear.is_empty()
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "expected at least one override value or clear target".to_string(),
        ));
    }

    let session_id = payload.session_id;
    if clear.contains(&SessionOverrideDimension::All) {
        proxy
            .state
            .clear_session_manual_overrides(session_id.as_str())
            .await;
    } else {
        if clear.contains(&SessionOverrideDimension::Model) {
            proxy
                .state
                .clear_session_model_override(session_id.as_str())
                .await;
        }
        if clear.contains(&SessionOverrideDimension::ReasoningEffort) {
            proxy
                .state
                .clear_session_reasoning_effort_override(session_id.as_str())
                .await;
        }
        if clear.contains(&SessionOverrideDimension::StationName) {
            proxy
                .state
                .clear_session_station_override(session_id.as_str())
                .await;
        }
        if clear.contains(&SessionOverrideDimension::ServiceTier) {
            proxy
                .state
                .clear_session_service_tier_override(session_id.as_str())
                .await;
        }
    }

    if let Some(model) = model {
        proxy
            .state
            .set_session_model_override(session_id.clone(), model, now_ms())
            .await;
    }
    if let Some(reasoning_effort) = reasoning_effort {
        proxy
            .state
            .set_session_reasoning_effort_override(session_id.clone(), reasoning_effort, now_ms())
            .await;
    }
    if let Some(station_name) = station_name {
        proxy
            .state
            .set_session_station_override(session_id.clone(), station_name, now_ms())
            .await;
    }
    if let Some(service_tier) = service_tier {
        proxy
            .state
            .set_session_service_tier_override(session_id.clone(), service_tier, now_ms())
            .await;
    }

    let overrides = proxy
        .state
        .get_session_manual_overrides(session_id.as_str())
        .await;
    Ok(Json(SessionManualOverridesResponse {
        session_id,
        overrides,
        precedence: session_override_precedence(),
    }))
}

pub(super) async fn set_session_station_override(
    proxy: ProxyService,
    Json(payload): Json<SessionStationOverrideRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_session_id(payload.session_id.as_str())?;
    let station_name = normalize_session_override_value("station_name", payload.station_name)?;
    if let Some(station_name) = station_name {
        proxy
            .state
            .set_session_station_override(payload.session_id, station_name, now_ms())
            .await;
    } else {
        proxy
            .state
            .clear_session_station_override(payload.session_id.as_str())
            .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn list_session_station_overrides(
    proxy: ProxyService,
) -> Result<Json<HashMap<String, String>>, (StatusCode, String)> {
    let map = proxy.state.list_session_station_overrides().await;
    Ok(Json(map))
}

pub(super) async fn set_session_model_override(
    proxy: ProxyService,
    Json(payload): Json<SessionModelOverrideRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_session_id(payload.session_id.as_str())?;
    let model = normalize_session_override_value("model", payload.model)?;
    if let Some(model) = model {
        proxy
            .state
            .set_session_model_override(payload.session_id, model, now_ms())
            .await;
    } else {
        proxy
            .state
            .clear_session_model_override(payload.session_id.as_str())
            .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn list_session_model_overrides(
    proxy: ProxyService,
) -> Result<Json<HashMap<String, String>>, (StatusCode, String)> {
    let map = proxy.state.list_session_model_overrides().await;
    Ok(Json(map))
}

pub(super) async fn set_session_service_tier_override(
    proxy: ProxyService,
    Json(payload): Json<SessionServiceTierOverrideRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_session_id(payload.session_id.as_str())?;
    let service_tier = normalize_session_override_value("service_tier", payload.service_tier)?;
    if let Some(service_tier) = service_tier {
        proxy
            .state
            .set_session_service_tier_override(payload.session_id, service_tier, now_ms())
            .await;
    } else {
        proxy
            .state
            .clear_session_service_tier_override(payload.session_id.as_str())
            .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn list_session_service_tier_overrides(
    proxy: ProxyService,
) -> Result<Json<HashMap<String, String>>, (StatusCode, String)> {
    let map = proxy.state.list_session_service_tier_overrides().await;
    Ok(Json(map))
}

pub(super) async fn reset_session_manual_overrides(
    proxy: ProxyService,
    Json(payload): Json<SessionOverrideResetRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_session_id(payload.session_id.as_str())?;
    proxy
        .state
        .clear_session_manual_overrides(payload.session_id.as_str())
        .await;
    Ok(StatusCode::NO_CONTENT)
}

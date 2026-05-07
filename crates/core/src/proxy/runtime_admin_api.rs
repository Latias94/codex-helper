use axum::Json;
use axum::extract::Query;
use axum::http::StatusCode;

use super::ProxyService;
use super::api_responses::{
    ProfilesResponse, ReloadResult, RetryConfigResponse, RuntimeStatusResponse,
    build_reload_result, build_retry_config_response, build_runtime_status_response,
    make_profiles_response,
};
use super::control_plane_service::save_runtime_proxy_settings_and_reload;

#[derive(serde::Deserialize)]
pub(super) struct ControlTraceQuery {
    limit: Option<usize>,
}

pub(super) async fn runtime_status(
    proxy: ProxyService,
) -> Result<Json<RuntimeStatusResponse>, (StatusCode, String)> {
    Ok(Json(build_runtime_status_response(&proxy).await))
}

pub(super) async fn get_retry_config(
    proxy: ProxyService,
) -> Result<Json<RetryConfigResponse>, (StatusCode, String)> {
    let cfg = proxy.config.snapshot().await;
    Ok(Json(build_retry_config_response(cfg.as_ref())))
}

pub(super) async fn get_pricing_catalog(
    _proxy: ProxyService,
) -> Result<Json<crate::pricing::ModelPriceCatalogSnapshot>, (StatusCode, String)> {
    Ok(Json(crate::pricing::operator_model_price_catalog_snapshot()))
}

pub(super) async fn set_retry_config(
    proxy: ProxyService,
    Json(payload): Json<crate::config::RetryConfig>,
) -> Result<Json<RetryConfigResponse>, (StatusCode, String)> {
    let cfg_snapshot = proxy.config.snapshot().await;
    let mut cfg = cfg_snapshot.as_ref().clone();
    cfg.retry = payload;

    save_runtime_proxy_settings_and_reload(&proxy, cfg).await?;
    let cfg = proxy.config.snapshot().await;
    Ok(Json(build_retry_config_response(cfg.as_ref())))
}

pub(super) async fn reload_runtime_config(
    proxy: ProxyService,
) -> Result<Json<ReloadResult>, (StatusCode, String)> {
    let changed = proxy
        .config
        .force_reload_from_disk()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let status = build_runtime_status_response(&proxy).await;
    Ok(Json(build_reload_result(changed, status)))
}

pub(super) async fn list_profiles(
    proxy: ProxyService,
) -> Result<Json<ProfilesResponse>, (StatusCode, String)> {
    Ok(Json(make_profiles_response(&proxy).await))
}

pub(super) async fn get_control_trace(
    _proxy: ProxyService,
    Query(q): Query<ControlTraceQuery>,
) -> Result<Json<Vec<crate::logging::ControlTraceLogEntry>>, (StatusCode, String)> {
    let limit = q.limit.unwrap_or(80).clamp(20, 400);
    crate::logging::read_recent_control_trace_entries(limit)
        .map(Json)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

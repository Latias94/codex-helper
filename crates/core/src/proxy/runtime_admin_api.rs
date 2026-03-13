use axum::Json;
use axum::http::StatusCode;

use super::ProxyService;
use super::api_responses::{
    ProfilesResponse, ReloadResult, RetryConfigResponse, RuntimeConfigStatus, build_reload_result,
    build_retry_config_response, build_runtime_config_status, make_profiles_response,
};

async fn save_proxy_config_and_reload(
    proxy: &ProxyService,
    cfg: crate::config::ProxyConfig,
) -> Result<(), (StatusCode, String)> {
    crate::config::save_config(&cfg)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    proxy
        .config
        .force_reload_from_disk()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(())
}

pub(super) async fn runtime_config_status(
    proxy: ProxyService,
) -> Result<Json<RuntimeConfigStatus>, (StatusCode, String)> {
    Ok(Json(build_runtime_config_status(&proxy).await))
}

pub(super) async fn get_retry_config(
    proxy: ProxyService,
) -> Result<Json<RetryConfigResponse>, (StatusCode, String)> {
    let cfg = proxy.config.snapshot().await;
    Ok(Json(build_retry_config_response(cfg.as_ref())))
}

pub(super) async fn set_retry_config(
    proxy: ProxyService,
    Json(payload): Json<crate::config::RetryConfig>,
) -> Result<Json<RetryConfigResponse>, (StatusCode, String)> {
    let cfg_snapshot = proxy.config.snapshot().await;
    let mut cfg = cfg_snapshot.as_ref().clone();
    cfg.retry = payload;

    save_proxy_config_and_reload(&proxy, cfg).await?;
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
    let status = build_runtime_config_status(&proxy).await;
    Ok(Json(build_reload_result(changed, status)))
}

pub(super) async fn list_profiles(
    proxy: ProxyService,
) -> Result<Json<ProfilesResponse>, (StatusCode, String)> {
    Ok(Json(make_profiles_response(&proxy).await))
}

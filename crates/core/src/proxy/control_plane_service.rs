use axum::http::StatusCode;

use crate::config::{ProxyConfig, ProxyConfigV2, ServiceConfigManager, ServiceViewV2};

use super::ProxyService;
use super::api_responses::{ProfilesResponse, make_profiles_response};

pub(super) fn runtime_service_manager_mut<'a>(
    cfg: &'a mut ProxyConfig,
    service_name: &str,
) -> &'a mut ServiceConfigManager {
    match service_name {
        "claude" => &mut cfg.claude,
        _ => &mut cfg.codex,
    }
}

pub(super) fn service_view_v2<'a>(cfg: &'a ProxyConfigV2, service_name: &str) -> &'a ServiceViewV2 {
    match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    }
}

pub(super) fn service_view_v2_mut<'a>(
    cfg: &'a mut ProxyConfigV2,
    service_name: &str,
) -> &'a mut ServiceViewV2 {
    match service_name {
        "claude" => &mut cfg.claude,
        _ => &mut cfg.codex,
    }
}

pub(super) async fn save_runtime_config_and_reload(
    proxy: &ProxyService,
    cfg: ProxyConfig,
) -> Result<(), (StatusCode, String)> {
    crate::config::save_config(&cfg)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    proxy
        .config
        .force_reload_from_disk()
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(())
}

pub(super) async fn save_runtime_profiles_config_and_reload(
    proxy: &ProxyService,
    cfg: ProxyConfig,
) -> Result<ProfilesResponse, (StatusCode, String)> {
    save_runtime_config_and_reload(proxy, cfg).await?;
    Ok(make_profiles_response(proxy).await)
}

pub(super) async fn load_persisted_config_v2() -> Result<ProxyConfigV2, (StatusCode, String)> {
    let path = crate::config::config_file_path();
    if path.exists()
        && path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
    {
        let text = tokio::fs::read_to_string(&path)
            .await
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
        let version = toml::from_str::<toml::Value>(&text)
            .ok()
            .and_then(|value| {
                value
                    .get("version")
                    .and_then(|version| version.as_integer())
            })
            .map(|value| value as u32);
        if version == Some(2) {
            return toml::from_str::<ProxyConfigV2>(&text)
                .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()));
        }
    }

    let runtime = crate::config::load_config()
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    crate::config::compact_v2_config(&crate::config::migrate_legacy_to_v2(&runtime))
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

pub(super) async fn save_proxy_config_v2_and_reload(
    proxy: &ProxyService,
    cfg: ProxyConfigV2,
) -> Result<(), (StatusCode, String)> {
    crate::config::save_config_v2(&cfg)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    proxy
        .config
        .force_reload_from_disk()
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(())
}

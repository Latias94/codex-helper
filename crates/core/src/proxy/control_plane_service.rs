use axum::http::StatusCode;

use crate::config::{
    ProxyConfig, ProxyConfigV2, ProxyConfigV3, ServiceConfigManager, ServiceViewV2, ServiceViewV3,
};

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

pub(super) fn service_view_v3<'a>(cfg: &'a ProxyConfigV3, service_name: &str) -> &'a ServiceViewV3 {
    match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    }
}

pub(super) fn service_view_v3_mut<'a>(
    cfg: &'a mut ProxyConfigV3,
    service_name: &str,
) -> &'a mut ServiceViewV3 {
    match service_name {
        "claude" => &mut cfg.claude,
        _ => &mut cfg.codex,
    }
}

pub(super) enum PersistedProxySettingsDocument {
    V2(ProxyConfigV2),
    V3(ProxyConfigV3),
}

pub(super) async fn prune_runtime_observability_after_reload(proxy: &ProxyService) {
    let cfg = proxy.config.snapshot().await;
    let mgr = proxy.service_manager(cfg.as_ref());
    proxy
        .state
        .prune_runtime_observability_for_service(proxy.service_name, mgr)
        .await;
}

pub(super) async fn save_runtime_proxy_settings_and_reload(
    proxy: &ProxyService,
    cfg: ProxyConfig,
) -> Result<(), (StatusCode, String)> {
    crate::config::save_config(&cfg)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let changed = proxy
        .config
        .force_reload_from_disk()
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    if changed {
        prune_runtime_observability_after_reload(proxy).await;
    }
    Ok(())
}

pub(super) async fn save_runtime_profile_settings_and_reload(
    proxy: &ProxyService,
    cfg: ProxyConfig,
) -> Result<ProfilesResponse, (StatusCode, String)> {
    save_runtime_proxy_settings_and_reload(proxy, cfg).await?;
    Ok(make_profiles_response(proxy).await)
}

fn toml_schema_version_or_shape(text: &str) -> Option<u32> {
    let value = toml::from_str::<toml::Value>(text).ok()?;
    if let Some(version) = value
        .get("version")
        .and_then(|version| version.as_integer())
        .map(|value| value as u32)
    {
        return Some(version);
    }

    let has_routing = ["codex", "claude"].iter().any(|service| {
        value
            .get(*service)
            .and_then(|service| service.get("routing"))
            .is_some()
    });
    if has_routing { Some(3) } else { None }
}

pub(super) async fn load_persisted_proxy_settings_document()
-> Result<PersistedProxySettingsDocument, (StatusCode, String)> {
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
        match toml_schema_version_or_shape(&text) {
            Some(3) => {
                let cfg = toml::from_str::<ProxyConfigV3>(&text)
                    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
                crate::config::compile_v3_to_runtime(&cfg)
                    .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
                return Ok(PersistedProxySettingsDocument::V3(cfg));
            }
            Some(2) => {
                let cfg = toml::from_str::<ProxyConfigV2>(&text)
                    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
                crate::config::compile_v2_to_runtime(&cfg)
                    .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
                return Ok(PersistedProxySettingsDocument::V2(cfg));
            }
            _ => {}
        }
    }

    let runtime = crate::config::load_config()
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let cfg = crate::config::compact_v2_config(&crate::config::migrate_legacy_to_v2(&runtime))
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(PersistedProxySettingsDocument::V2(cfg))
}

pub(super) async fn load_persisted_proxy_settings_v2() -> Result<ProxyConfigV2, (StatusCode, String)>
{
    match load_persisted_proxy_settings_document().await? {
        PersistedProxySettingsDocument::V2(cfg) => Ok(cfg),
        PersistedProxySettingsDocument::V3(cfg) => {
            let v2 = crate::config::compile_v3_to_v2(&cfg)
                .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
            crate::config::compact_v2_config(&v2)
                .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
        }
    }
}

pub(super) async fn save_persisted_proxy_settings_v2_and_reload(
    proxy: &ProxyService,
    cfg: ProxyConfigV2,
) -> Result<(), (StatusCode, String)> {
    let runtime = crate::config::compile_v2_to_runtime(&cfg)
        .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
    save_runtime_proxy_settings_and_reload(proxy, runtime).await
}

pub(super) async fn save_persisted_proxy_settings_document_and_reload(
    proxy: &ProxyService,
    document: PersistedProxySettingsDocument,
) -> Result<(), (StatusCode, String)> {
    match document {
        PersistedProxySettingsDocument::V2(cfg) => {
            let runtime = crate::config::compile_v2_to_runtime(&cfg)
                .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
            save_runtime_proxy_settings_and_reload(proxy, runtime).await?;
        }
        PersistedProxySettingsDocument::V3(cfg) => {
            crate::config::save_config_v3(&cfg)
                .await
                .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
            let changed = proxy
                .config
                .force_reload_from_disk()
                .await
                .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
            if changed {
                prune_runtime_observability_after_reload(proxy).await;
            }
        }
    }
    Ok(())
}

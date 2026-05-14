use axum::http::StatusCode;

use crate::config::{
    ProxyConfig, ProxyConfigV2, ProxyConfigV4, ServiceConfigManager, ServiceViewV2, ServiceViewV4,
    is_supported_route_graph_config_version,
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

pub(super) fn service_view_v4<'a>(cfg: &'a ProxyConfigV4, service_name: &str) -> &'a ServiceViewV4 {
    match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    }
}

pub(super) fn service_view_v4_mut<'a>(
    cfg: &'a mut ProxyConfigV4,
    service_name: &str,
) -> &'a mut ServiceViewV4 {
    match service_name {
        "claude" => &mut cfg.claude,
        _ => &mut cfg.codex,
    }
}

pub(super) enum PersistedProxySettingsDocument {
    V2(Box<ProxyConfigV2>),
    V4(Box<ProxyConfigV4>),
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

    let has_v4_routing = ["codex", "claude"].iter().any(|service| {
        value
            .get(*service)
            .and_then(|service| service.get("routing"))
            .and_then(|routing| routing.get("entry").or_else(|| routing.get("routes")))
            .is_some()
    });
    if has_v4_routing {
        Some(4)
    } else {
        let has_legacy_routing = ["codex", "claude"].iter().any(|service| {
            value
                .get(*service)
                .and_then(|service| service.get("routing"))
                .is_some()
        });
        if has_legacy_routing { Some(3) } else { None }
    }
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
            Some(version) if is_supported_route_graph_config_version(version) => {
                let mut cfg = toml::from_str::<ProxyConfigV4>(&text)
                    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
                cfg.sync_routing_compat_from_graph();
                crate::config::compile_v4_to_runtime(&cfg)
                    .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
                return Ok(PersistedProxySettingsDocument::V4(Box::new(cfg)));
            }
            Some(3) => {
                let legacy = toml::from_str::<crate::config::legacy::ProxyConfigV3Legacy>(&text)
                    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
                let migrated = crate::config::legacy::migrate_v3_legacy_to_v4(&legacy)
                    .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
                let mut cfg = migrated.config;
                cfg.sync_routing_compat_from_graph();
                crate::config::compile_v4_to_runtime(&cfg)
                    .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
                return Ok(PersistedProxySettingsDocument::V4(Box::new(cfg)));
            }
            Some(2) => {
                let cfg = toml::from_str::<ProxyConfigV2>(&text)
                    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
                crate::config::compile_v2_to_runtime(&cfg)
                    .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
                return Ok(PersistedProxySettingsDocument::V2(Box::new(cfg)));
            }
            _ => {}
        }
    }

    let runtime = crate::config::load_config()
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let cfg = crate::config::compact_v2_config(&crate::config::migrate_legacy_to_v2(&runtime))
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(PersistedProxySettingsDocument::V2(Box::new(cfg)))
}

pub(super) async fn load_persisted_proxy_settings_v2() -> Result<ProxyConfigV2, (StatusCode, String)>
{
    match load_persisted_proxy_settings_document().await? {
        PersistedProxySettingsDocument::V2(cfg) => Ok(*cfg),
        PersistedProxySettingsDocument::V4(cfg) => {
            let v2 = crate::config::compile_v4_to_v2(&cfg)
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
        PersistedProxySettingsDocument::V4(cfg) => {
            crate::config::save_config_v4(&cfg)
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

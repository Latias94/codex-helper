use super::*;

pub(super) fn parse_proxy_settings_document(
    text: &str,
) -> anyhow::Result<ProxySettingsWorkingDocument> {
    if let Ok(value) = toml::from_str::<toml::Value>(text) {
        let version = value
            .get("version")
            .and_then(|v| v.as_integer())
            .map(|v| v as u32)
            .or_else(|| {
                let has_routing = ["codex", "claude"].iter().any(|service| {
                    value
                        .get(*service)
                        .and_then(|service| service.get("routing"))
                        .is_some()
                });
                if has_routing { Some(3) } else { None }
            });
        if version == Some(3) {
            let cfg = toml::from_str::<crate::config::ProxyConfigV3>(text)?;
            crate::config::compile_v3_to_runtime(&cfg)?;
            return Ok(ProxySettingsWorkingDocument::V3(cfg));
        }
        if version == Some(2) {
            let cfg = toml::from_str::<crate::config::ProxyConfigV2>(text)?;
            crate::config::compile_v2_to_runtime(&cfg)?;
            return Ok(ProxySettingsWorkingDocument::V2(cfg));
        }

        if let Ok(cfg) = toml::from_str::<crate::config::ProxyConfig>(text) {
            return Ok(ProxySettingsWorkingDocument::Legacy(cfg));
        }
    }

    let v = serde_json::from_str::<crate::config::ProxyConfig>(text)?;
    Ok(ProxySettingsWorkingDocument::Legacy(v))
}

pub(super) fn save_proxy_settings_document(
    rt: &tokio::runtime::Runtime,
    doc: &ProxySettingsWorkingDocument,
) -> anyhow::Result<()> {
    match doc {
        ProxySettingsWorkingDocument::Legacy(cfg) => {
            rt.block_on(crate::config::save_config(cfg))?
        }
        ProxySettingsWorkingDocument::V2(cfg) => {
            rt.block_on(crate::config::save_config_v2(cfg))?;
        }
        ProxySettingsWorkingDocument::V3(cfg) => {
            rt.block_on(crate::config::save_config_v3(cfg))?;
        }
    }
    Ok(())
}

pub(super) fn sync_codex_auth_into_settings_document(
    doc: &mut ProxySettingsWorkingDocument,
    options: crate::config::SyncCodexAuthFromCodexOptions,
) -> anyhow::Result<crate::config::SyncCodexAuthFromCodexReport> {
    match doc {
        ProxySettingsWorkingDocument::Legacy(cfg) => {
            crate::config::sync_codex_auth_from_codex_cli(cfg, options)
        }
        ProxySettingsWorkingDocument::V2(cfg) => {
            let mut runtime = crate::config::compile_v2_to_runtime(cfg)?;
            let report = crate::config::sync_codex_auth_from_codex_cli(&mut runtime, options)?;
            *cfg =
                crate::config::compact_v2_config(&crate::config::migrate_legacy_to_v2(&runtime))?;
            Ok(report)
        }
        ProxySettingsWorkingDocument::V3(cfg) => {
            let mut runtime = crate::config::compile_v3_to_runtime(cfg)?;
            let report = crate::config::sync_codex_auth_from_codex_cli(&mut runtime, options)?;
            *cfg = crate::config::migrate_legacy_to_v3(&runtime)?;
            Ok(report)
        }
    }
}

pub(super) fn working_legacy_proxy_settings(
    view: &ProxySettingsViewState,
) -> Option<&crate::config::ProxyConfig> {
    match view.working.as_ref()? {
        ProxySettingsWorkingDocument::Legacy(cfg) => Some(cfg),
        ProxySettingsWorkingDocument::V2(_) | ProxySettingsWorkingDocument::V3(_) => None,
    }
}

pub(super) fn working_legacy_proxy_settings_mut(
    view: &mut ProxySettingsViewState,
) -> Option<&mut crate::config::ProxyConfig> {
    match view.working.as_mut()? {
        ProxySettingsWorkingDocument::Legacy(cfg) => Some(cfg),
        ProxySettingsWorkingDocument::V2(_) | ProxySettingsWorkingDocument::V3(_) => None,
    }
}

use super::*;

pub(super) fn parse_proxy_config_document(text: &str) -> anyhow::Result<ConfigWorkingDocument> {
    if let Ok(value) = toml::from_str::<toml::Value>(text) {
        let version = value
            .get("version")
            .and_then(|v| v.as_integer())
            .map(|v| v as u32);
        if version == Some(2) {
            let cfg = toml::from_str::<crate::config::ProxyConfigV2>(text)?;
            crate::config::compile_v2_to_runtime(&cfg)?;
            return Ok(ConfigWorkingDocument::V2(cfg));
        }

        if let Ok(cfg) = toml::from_str::<crate::config::ProxyConfig>(text) {
            return Ok(ConfigWorkingDocument::Legacy(cfg));
        }
    }

    let v = serde_json::from_str::<crate::config::ProxyConfig>(text)?;
    Ok(ConfigWorkingDocument::Legacy(v))
}

pub(super) fn save_proxy_config_document(
    rt: &tokio::runtime::Runtime,
    doc: &ConfigWorkingDocument,
) -> anyhow::Result<()> {
    match doc {
        ConfigWorkingDocument::Legacy(cfg) => rt.block_on(crate::config::save_config(cfg))?,
        ConfigWorkingDocument::V2(cfg) => {
            rt.block_on(crate::config::save_config_v2(cfg))?;
        }
    }
    Ok(())
}

pub(super) fn sync_codex_auth_into_document(
    doc: &mut ConfigWorkingDocument,
    options: crate::config::SyncCodexAuthFromCodexOptions,
) -> anyhow::Result<crate::config::SyncCodexAuthFromCodexReport> {
    match doc {
        ConfigWorkingDocument::Legacy(cfg) => {
            crate::config::sync_codex_auth_from_codex_cli(cfg, options)
        }
        ConfigWorkingDocument::V2(cfg) => {
            let mut runtime = crate::config::compile_v2_to_runtime(cfg)?;
            let report = crate::config::sync_codex_auth_from_codex_cli(&mut runtime, options)?;
            *cfg =
                crate::config::compact_v2_config(&crate::config::migrate_legacy_to_v2(&runtime))?;
            Ok(report)
        }
    }
}

pub(super) fn working_legacy_config(view: &ConfigViewState) -> Option<&crate::config::ProxyConfig> {
    match view.working.as_ref()? {
        ConfigWorkingDocument::Legacy(cfg) => Some(cfg),
        ConfigWorkingDocument::V2(_) => None,
    }
}

pub(super) fn working_legacy_config_mut(
    view: &mut ConfigViewState,
) -> Option<&mut crate::config::ProxyConfig> {
    match view.working.as_mut()? {
        ConfigWorkingDocument::Legacy(cfg) => Some(cfg),
        ConfigWorkingDocument::V2(_) => None,
    }
}

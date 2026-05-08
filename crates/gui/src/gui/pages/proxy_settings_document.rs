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
    }

    anyhow::bail!(
        "GUI settings editor only supports v3 routing-first TOML config; run `codex-helper config migrate --write --yes` first"
    );
}

pub(super) fn save_proxy_settings_document(
    rt: &tokio::runtime::Runtime,
    doc: &ProxySettingsWorkingDocument,
) -> anyhow::Result<()> {
    match doc {
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
        ProxySettingsWorkingDocument::V3(cfg) => {
            let mut runtime = crate::config::compile_v3_to_runtime(cfg)?;
            let report = crate::config::sync_codex_auth_from_codex_cli(&mut runtime, options)?;
            *cfg = crate::config::migrate_legacy_to_v3(&runtime)?;
            Ok(report)
        }
    }
}

use super::view_state::ProxySettingsSaveLoad;
use super::*;
use std::sync::mpsc::TryRecvError;

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

async fn save_proxy_settings_document_async(
    doc: ProxySettingsWorkingDocument,
) -> anyhow::Result<String> {
    let _saved_path = match doc {
        ProxySettingsWorkingDocument::V3(cfg) => crate::config::save_config_v3(&cfg).await?,
    };
    let path = crate::config::config_file_path();
    Ok(tokio::fs::read_to_string(path).await?)
}

pub(super) fn start_proxy_settings_save(
    ctx: &mut PageCtx<'_>,
    doc: ProxySettingsWorkingDocument,
    message: String,
    reload_runtime: bool,
) {
    if let Some(load) = ctx.view.proxy_settings.save_load.take() {
        load.join.abort();
    }
    ctx.view.proxy_settings.save_seq = ctx.view.proxy_settings.save_seq.saturating_add(1);
    let seq = ctx.view.proxy_settings.save_seq;
    let (tx, rx) = std::sync::mpsc::channel();
    let join = ctx.rt.spawn(async move {
        let result = save_proxy_settings_document_async(doc).await;
        let _ = tx.send((seq, result));
    });
    ctx.view.proxy_settings.save_load = Some(ProxySettingsSaveLoad {
        seq,
        message,
        reload_runtime,
        rx,
        join,
    });
}

pub(super) fn poll_proxy_settings_save(ctx: &mut PageCtx<'_>) {
    let Some(load) = ctx.view.proxy_settings.save_load.as_mut() else {
        return;
    };
    match load.rx.try_recv() {
        Ok((seq, result)) => {
            if seq != load.seq {
                ctx.view.proxy_settings.save_load = None;
                return;
            }

            let message = load.message.clone();
            let reload_runtime = load.reload_runtime;
            ctx.view.proxy_settings.save_load = None;
            match result {
                Ok(text) => {
                    *ctx.proxy_settings_text = text.clone();
                    match parse_proxy_settings_document(&text) {
                        Ok(doc) => {
                            ctx.view.proxy_settings.working = Some(doc);
                            ctx.view.proxy_settings.load_error = None;
                        }
                        Err(err) => {
                            ctx.view.proxy_settings.working = None;
                            ctx.view.proxy_settings.load_error =
                                Some(format!("parse failed: {err}"));
                            *ctx.last_error = Some(format!("re-read parse failed: {err}"));
                            *ctx.last_info = Some(message);
                            return;
                        }
                    }

                    if reload_runtime
                        && matches!(
                            ctx.proxy.kind(),
                            ProxyModeKind::Running | ProxyModeKind::Attached
                        )
                        && let Err(err) = ctx.proxy.reload_runtime_config(ctx.rt)
                    {
                        *ctx.last_error = Some(format!("reload runtime failed: {err}"));
                        *ctx.last_info = Some(message);
                        return;
                    }

                    *ctx.last_info = Some(message);
                    *ctx.last_error = None;
                }
                Err(err) => {
                    *ctx.last_error = Some(format!("save failed: {err}"));
                }
            }
        }
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => {
            ctx.view.proxy_settings.save_load = None;
        }
    }
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

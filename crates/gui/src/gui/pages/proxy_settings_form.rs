use super::proxy_settings_document::start_proxy_settings_save;
use super::proxy_settings_v3_editors::{
    render_v3_provider_editor, render_v3_routing_editor, routing_policy_label,
};
use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.label(pick(
        ctx.lang,
        "表单视图可编辑常用 v3 provider；复杂 routing、endpoint、模型映射仍可在“原始”视图中精确编辑。",
        "Form view edits common v3 providers; use Raw for advanced routing, endpoints, and model mappings.",
    ));

    let mut needs_load = ctx.view.proxy_settings.working.is_none();
    if let Some(err) = ctx.view.proxy_settings.load_error.as_deref() {
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
        needs_load = true;
    }

    ui.horizontal(|ui| {
        if ui
            .button(pick(ctx.lang, "从磁盘加载", "Load from disk"))
            .clicked()
        {
            needs_load = true;
        }

        if ui
            .button(pick(ctx.lang, "重载代理运行态", "Reload proxy runtime"))
            .clicked()
        {
            if let Err(e) = ctx.proxy.reload_runtime_config(ctx.rt) {
                *ctx.last_error = Some(format!("reload runtime failed: {e}"));
            } else {
                *ctx.last_info = Some(pick(ctx.lang, "已重载", "Reloaded").to_string());
            }
        }

        if ui
            .button(pick(ctx.lang, "从 Codex 导入", "Import from Codex"))
            .clicked()
        {
            ctx.view.proxy_settings.import_codex.open = true;
            ctx.view.proxy_settings.import_codex.last_error = None;
            ctx.view.proxy_settings.import_codex.preview = None;
        }
    });
    if ctx.view.proxy_settings.save_load.is_some() {
        ui.label(pick(ctx.lang, "正在保存设置...", "Saving settings..."));
    }

    if needs_load {
        reload_working_document_from_disk(ctx);
    }

    let mut do_preview = false;
    let mut do_apply = false;
    render_import_modal(ui, ctx, &mut do_preview, &mut do_apply);

    if do_preview || do_apply {
        let options = crate::config::SyncCodexAuthFromCodexOptions {
            add_missing: ctx.view.proxy_settings.import_codex.add_missing,
            set_active: ctx.view.proxy_settings.import_codex.set_active,
            force: ctx.view.proxy_settings.import_codex.force,
        };
        let mut target = if let Some(cfg) = ctx.view.proxy_settings.working.as_ref() {
            Some(cfg.clone())
        } else {
            match std::fs::read_to_string(ctx.proxy_settings_path) {
                Ok(text) => match parse_proxy_settings_document(&text) {
                    Ok(doc) => Some(doc),
                    Err(err) => {
                        ctx.view.proxy_settings.import_codex.last_error =
                            Some(format!("parse settings failed: {err}"));
                        None
                    }
                },
                Err(err) => {
                    ctx.view.proxy_settings.import_codex.last_error =
                        Some(format!("read settings failed: {err}"));
                    None
                }
            }
        };

        if let Some(ref mut doc) = target {
            match sync_codex_auth_into_settings_document(doc, options) {
                Ok(report) => {
                    let summary = format!(
                        "updated={} added={} active_set={}",
                        report.updated, report.added, report.active_set
                    );
                    ctx.view.proxy_settings.import_codex.preview = Some(report);
                    ctx.view.proxy_settings.import_codex.last_error = None;
                    *ctx.last_info =
                        Some(pick(ctx.lang, "已生成预览", "Preview ready").to_string());

                    if do_apply {
                        start_proxy_settings_save(
                            ctx,
                            doc.clone(),
                            format!(
                                "{}: {}",
                                pick(ctx.lang, "已导入并保存", "Imported & saved"),
                                summary
                            ),
                            true,
                        );
                        ctx.view.proxy_settings.import_codex.last_error = None;
                    }
                }
                Err(err) => {
                    ctx.view.proxy_settings.import_codex.preview = None;
                    ctx.view.proxy_settings.import_codex.last_error = Some(err.to_string());
                }
            }
        } else {
            ctx.view.proxy_settings.import_codex.preview = None;
        }
    }

    ui.separator();
    let lang = ctx.lang;
    let mut editor_action = None;
    match ctx.view.proxy_settings.working.as_mut() {
        Some(ProxySettingsWorkingDocument::V3(cfg)) => {
            render_v3_summary(ui, lang, cfg);
            ui.separator();
            editor_action = render_v3_provider_editor(
                ui,
                lang,
                cfg,
                &mut ctx.view.proxy_settings.provider_editor,
            );
            if editor_action.is_none() {
                ui.separator();
                let routing_service = ctx.view.proxy_settings.provider_editor.service;
                editor_action = render_v3_routing_editor(
                    ui,
                    lang,
                    cfg,
                    routing_service,
                    &mut ctx.view.proxy_settings.routing_editor,
                );
            }
        }
        None => {
            ui.label(pick(
                ctx.lang,
                "未加载设置。你可以切到“原始”视图，或者先从磁盘加载。",
                "Settings not loaded. Switch to Raw view, or load from disk first.",
            ));
        }
    }

    if let Some(action) = editor_action {
        match action {
            Ok(message) => persist_current_working_document(ctx, message),
            Err(err) => {
                *ctx.last_error = Some(err);
                *ctx.last_info = None;
            }
        }
    }
}

fn reload_working_document_from_disk(ctx: &mut PageCtx<'_>) {
    match std::fs::read_to_string(ctx.proxy_settings_path) {
        Ok(text) => {
            *ctx.proxy_settings_text = text.clone();
            match parse_proxy_settings_document(&text) {
                Ok(doc) => {
                    ctx.view.proxy_settings.working = Some(doc);
                    ctx.view.proxy_settings.load_error = None;
                }
                Err(err) => {
                    ctx.view.proxy_settings.working = None;
                    ctx.view.proxy_settings.load_error = Some(format!("parse failed: {err}"));
                }
            }
        }
        Err(err) => {
            ctx.view.proxy_settings.working = None;
            ctx.view.proxy_settings.load_error = Some(format!("read settings failed: {err}"));
        }
    }
}

fn persist_current_working_document(ctx: &mut PageCtx<'_>, message: String) {
    let Some(doc) = ctx.view.proxy_settings.working.as_ref() else {
        *ctx.last_error = Some("no loaded settings document to save".to_string());
        return;
    };
    start_proxy_settings_save(ctx, doc.clone(), message, true);
}

fn render_import_modal(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    do_preview: &mut bool,
    do_apply: &mut bool,
) {
    if !ctx.view.proxy_settings.import_codex.open {
        return;
    }

    let mut open = true;
    let mut close_clicked = false;
    egui::Window::new(pick(
        ctx.lang,
        "从 Codex 导入（providers / env_key）",
        "Import from Codex (providers / env_key)",
    ))
    .collapsible(false)
    .resizable(false)
    .open(&mut open)
    .show(ui.ctx(), |ui| {
        ui.label(pick(
            ctx.lang,
            "读取 ~/.codex/config.toml 与 ~/.codex/auth.json，同步 providers 的 base_url/env_key（只写入 env var 名，不写入密钥）。",
            "Reads ~/.codex/config.toml and ~/.codex/auth.json, syncing providers' base_url/env_key (writes only env var names, no secrets).",
        ));
        ui.add_space(6.0);

        ui.checkbox(
            &mut ctx.view.proxy_settings.import_codex.add_missing,
            pick(ctx.lang, "添加缺失的 provider", "Add missing providers"),
        );
        ui.checkbox(
            &mut ctx.view.proxy_settings.import_codex.set_active,
            pick(
                ctx.lang,
                "同步 active 为 Codex 当前 model_provider",
                "Set active to Codex model_provider",
            ),
        );
        ui.checkbox(
            &mut ctx.view.proxy_settings.import_codex.force,
            pick(ctx.lang, "强制覆盖（谨慎）", "Force overwrite (careful)"),
        );
        if ctx.view.proxy_settings.import_codex.force {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(
                    ctx.lang,
                    "强制覆盖可能会覆盖非 Codex 来源的上游配置，请确认。",
                    "Force overwrite may override non-Codex upstreams. Use with care.",
                ),
            );
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(pick(ctx.lang, "预览", "Preview")).clicked() {
                *do_preview = true;
            }
            if ui.button(pick(ctx.lang, "应用并保存", "Apply & save")).clicked() {
                *do_apply = true;
            }
            if ui.button(pick(ctx.lang, "关闭", "Close")).clicked() {
                close_clicked = true;
            }
        });

        if let Some(err) = ctx.view.proxy_settings.import_codex.last_error.as_deref() {
            ui.add_space(6.0);
            ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
        }

        if let Some(report) = ctx.view.proxy_settings.import_codex.preview.as_ref() {
            ui.add_space(6.0);
            ui.label(format!(
                "{}: updated={} added={} active_set={}",
                pick(ctx.lang, "预览结果", "Preview"),
                report.updated,
                report.added,
                report.active_set
            ));
            if !report.warnings.is_empty() {
                ui.add_space(4.0);
                ui.label(pick(ctx.lang, "警告：", "Warnings:"));
                for warning in report.warnings.iter().take(12) {
                    ui.colored_label(egui::Color32::from_rgb(200, 120, 40), warning);
                }
                if report.warnings.len() > 12 {
                    ui.label(format!("… +{} more", report.warnings.len() - 12));
                }
            }
        }
    });
    if close_clicked {
        open = false;
    }
    ctx.view.proxy_settings.import_codex.open = open;
}

fn render_v3_summary(ui: &mut egui::Ui, lang: Language, cfg: &crate::config::ProxyConfigV3) {
    ui.label(pick(
        lang,
        "当前文件是 v3 routing-first 配置。",
        "The current file uses the v3 routing-first schema.",
    ));
    ui.small(pick(
        lang,
        "Provider 只定义身份、地址、鉴权和标签；实际选择顺序由 routing 决定。",
        "Providers define identity, endpoint, auth, and tags; routing decides selection order.",
    ));
    ui.add_space(6.0);

    render_service_summary(ui, lang, "codex", &cfg.codex);
    ui.add_space(4.0);
    render_service_summary(ui, lang, "claude", &cfg.claude);
}

fn render_service_summary(
    ui: &mut egui::Ui,
    lang: Language,
    service_name: &str,
    service: &crate::config::ServiceViewV3,
) {
    let provider_count = service.providers.len();
    let endpoint_count = service
        .providers
        .values()
        .map(|provider| provider.endpoints.len())
        .sum::<usize>();
    let profile_count = service.profiles.len();
    let routing = service.routing.as_ref();
    let routing_policy = routing
        .map(|routing| routing_policy_label(routing.policy))
        .unwrap_or("<none>");
    let routing_order = routing.map(|routing| routing.order.len()).unwrap_or(0);
    let default_profile = service.default_profile.as_deref().unwrap_or("<auto>");

    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(format!("{service_name}:"));
            ui.label(format!(
                "{} {}",
                pick(lang, "providers", "providers"),
                provider_count
            ));
            ui.label(format!(
                "{} {}",
                pick(lang, "profiles", "profiles"),
                profile_count
            ));
            ui.label(format!(
                "{} {}",
                pick(lang, "endpoints", "endpoints"),
                endpoint_count
            ));
        });
        ui.small(format!(
            "{}: {}  {}: {}  {}: {}",
            pick(lang, "default_profile", "default_profile"),
            default_profile,
            pick(lang, "routing", "routing"),
            routing_policy,
            pick(lang, "order", "order"),
            routing_order
        ));
        if let Some(routing) = routing {
            if let Some(target) = routing.target.as_deref() {
                ui.small(format!("{}: {target}", pick(lang, "target", "target")));
            }
            ui.small(format!(
                "{}: {}",
                pick(lang, "on_exhausted", "on_exhausted"),
                format!("{:?}", routing.on_exhausted).to_ascii_lowercase()
            ));
        }
    });
}

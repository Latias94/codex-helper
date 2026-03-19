use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.label(pick(
        ctx.lang,
        "表单视图：优先做常用代理设置（active / enabled / level）。复杂字段仍建议用“原始”视图。",
        "Form view focuses on common proxy settings (active / enabled / level). Use Raw view for advanced edits.",
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

    if needs_load {
        match std::fs::read_to_string(ctx.proxy_settings_path) {
            Ok(t) => match parse_proxy_settings_document(&t) {
                Ok(cfg) => {
                    ctx.view.proxy_settings.working = Some(cfg);
                    ctx.view.proxy_settings.load_error = None;
                }
                Err(e) => {
                    ctx.view.proxy_settings.working = None;
                    ctx.view.proxy_settings.load_error = Some(format!("parse failed: {e}"));
                }
            },
            Err(e) => {
                ctx.view.proxy_settings.working = None;
                ctx.view.proxy_settings.load_error = Some(format!("read settings failed: {e}"));
            }
        }
    }

    // Modal: import/sync providers from Codex CLI.
    let mut do_preview = false;
    let mut do_apply = false;
    if ctx.view.proxy_settings.import_codex.open {
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
                "读取 ~/.codex/config.toml 与 ~/.codex/auth.json，同步 providers 的 base_url/env_key（仅写入 env var 名，不写入密钥）。",
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
                    do_preview = true;
                }
                if ui.button(pick(ctx.lang, "应用并保存", "Apply & save")).clicked() {
                    do_apply = true;
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
                    for w in report.warnings.iter().take(12) {
                        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), w);
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

    if do_preview {
        let options = crate::config::SyncCodexAuthFromCodexOptions {
            add_missing: ctx.view.proxy_settings.import_codex.add_missing,
            set_active: ctx.view.proxy_settings.import_codex.set_active,
            force: ctx.view.proxy_settings.import_codex.force,
        };

        let tmp_opt = if let Some(cfg) = ctx.view.proxy_settings.working.as_ref() {
            Some(cfg.clone())
        } else {
            match std::fs::read_to_string(ctx.proxy_settings_path) {
                Ok(t) => match parse_proxy_settings_document(&t) {
                    Ok(cfg) => Some(cfg),
                    Err(e) => {
                        ctx.view.proxy_settings.import_codex.last_error =
                            Some(format!("parse settings failed: {e}"));
                        None
                    }
                },
                Err(e) => {
                    ctx.view.proxy_settings.import_codex.last_error =
                        Some(format!("read settings failed: {e}"));
                    None
                }
            }
        };

        if let Some(mut tmp) = tmp_opt {
            match sync_codex_auth_into_settings_document(&mut tmp, options) {
                Ok(report) => {
                    ctx.view.proxy_settings.import_codex.preview = Some(report);
                    ctx.view.proxy_settings.import_codex.last_error = None;
                    *ctx.last_info =
                        Some(pick(ctx.lang, "已生成预览", "Preview ready").to_string());
                }
                Err(e) => {
                    ctx.view.proxy_settings.import_codex.preview = None;
                    ctx.view.proxy_settings.import_codex.last_error = Some(e.to_string());
                }
            }
        } else {
            ctx.view.proxy_settings.import_codex.preview = None;
        }
    }

    if do_apply {
        let options = crate::config::SyncCodexAuthFromCodexOptions {
            add_missing: ctx.view.proxy_settings.import_codex.add_missing,
            set_active: ctx.view.proxy_settings.import_codex.set_active,
            force: ctx.view.proxy_settings.import_codex.force,
        };

        let mut can_apply = true;
        if ctx.view.proxy_settings.working.is_none() {
            match std::fs::read_to_string(ctx.proxy_settings_path) {
                Ok(t) => match parse_proxy_settings_document(&t) {
                    Ok(cfg) => {
                        ctx.view.proxy_settings.working = Some(cfg);
                        ctx.view.proxy_settings.load_error = None;
                    }
                    Err(e) => {
                        ctx.view.proxy_settings.import_codex.last_error =
                            Some(format!("parse settings failed: {e}"));
                        can_apply = false;
                    }
                },
                Err(e) => {
                    ctx.view.proxy_settings.import_codex.last_error =
                        Some(format!("read settings failed: {e}"));
                    can_apply = false;
                }
            }
        }

        let report = if can_apply {
            match sync_codex_auth_into_settings_document(
                ctx.view
                    .proxy_settings
                    .working
                    .as_mut()
                    .expect("loaded above"),
                options,
            ) {
                Ok(r) => Some(r),
                Err(e) => {
                    ctx.view.proxy_settings.import_codex.last_error = Some(e.to_string());
                    ctx.view.proxy_settings.import_codex.preview = None;
                    None
                }
            }
        } else {
            None
        };

        if let Some(report) = report {
            let summary = format!(
                "updated={} added={} active_set={}",
                report.updated, report.added, report.active_set
            );

            let save_res = {
                let cfg = ctx
                    .view
                    .proxy_settings
                    .working
                    .as_ref()
                    .expect("checked above");
                save_proxy_settings_document(ctx.rt, cfg)
            };

            match save_res {
                Ok(()) => {
                    let new_path = crate::config::config_file_path();
                    if let Ok(t) = std::fs::read_to_string(&new_path) {
                        *ctx.proxy_settings_text = t;
                    }
                    if let Ok(t) = std::fs::read_to_string(&new_path)
                        && let Ok(parsed) = parse_proxy_settings_document(&t)
                    {
                        ctx.view.proxy_settings.working = Some(parsed);
                    }

                    if matches!(
                        ctx.proxy.kind(),
                        ProxyModeKind::Running | ProxyModeKind::Attached
                    ) && let Err(e) = ctx.proxy.reload_runtime_config(ctx.rt)
                    {
                        *ctx.last_error = Some(format!("reload runtime failed: {e}"));
                    }

                    ctx.view.proxy_settings.import_codex.preview = Some(report);
                    ctx.view.proxy_settings.import_codex.last_error = None;
                    *ctx.last_info = Some(format!(
                        "{}: {summary}",
                        pick(ctx.lang, "已导入并保存", "Imported & saved")
                    ));
                }
                Err(e) => {
                    ctx.view.proxy_settings.import_codex.preview = Some(report);
                    ctx.view.proxy_settings.import_codex.last_error =
                        Some(format!("save failed: {e}"));
                    *ctx.last_error = Some(format!("save failed: {e}"));
                }
            }
        }
    }

    if ctx.view.proxy_settings.working.is_none() {
        ui.add_space(6.0);
        ui.label(pick(
            ctx.lang,
            "未加载设置。你可以切换到“原始”视图，或点击“从磁盘加载”。",
            "Settings not loaded. Switch to Raw view, or click Load from disk.",
        ));
        return;
    }

    if matches!(
        ctx.view.proxy_settings.working.as_ref(),
        Some(ProxySettingsWorkingDocument::V2(_))
    ) {
        config_v2::render(ui, ctx);
        return;
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "服务", "Service"));
        let mut svc = ctx.view.proxy_settings.service;
        egui::ComboBox::from_id_salt("config_form_service")
            .selected_text(match svc {
                crate::config::ServiceKind::Codex => "codex",
                crate::config::ServiceKind::Claude => "claude",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut svc, crate::config::ServiceKind::Codex, "codex");
                ui.selectable_value(&mut svc, crate::config::ServiceKind::Claude, "claude");
            });
        ctx.view.proxy_settings.service = svc;
    });

    let (active_name, active_fallback, names) = {
        let cfg = working_legacy_proxy_settings(&ctx.view.proxy_settings).expect("legacy branch");
        let mgr = match ctx.view.proxy_settings.service {
            crate::config::ServiceKind::Claude => &cfg.claude,
            crate::config::ServiceKind::Codex => &cfg.codex,
        };
        let mut v = mgr.stations().keys().cloned().collect::<Vec<_>>();
        v.sort_by(|a, b| {
            let la = mgr.station(a).map(|c| c.level).unwrap_or(1);
            let lb = mgr.station(b).map(|c| c.level).unwrap_or(1);
            la.cmp(&lb).then_with(|| a.cmp(b))
        });
        (
            mgr.active.clone(),
            mgr.active_station().map(|c| c.name.clone()),
            v,
        )
    };

    if names.is_empty() {
        ui.add_space(6.0);
        ui.label(pick(
            ctx.lang,
            "该服务下没有任何 station。请先在“原始”视图或文件中添加。",
            "No stations found for this service. Add one via Raw view or by editing the file.",
        ));
        return;
    }

    if ctx
        .view
        .proxy_settings
        .selected_name
        .as_ref()
        .is_none_or(|n| !names.iter().any(|x| x == n))
    {
        ctx.view.proxy_settings.selected_name = names.first().cloned();
    }

    let selected_service_kind = ctx.view.proxy_settings.service;
    let mut selected_name = ctx.view.proxy_settings.selected_name.clone();
    let mut action_set_active: Option<String> = None;
    let mut action_clear_active = false;
    let mut action_probe_selected: Option<String> = None;
    let mut action_health_start: Option<(bool, Vec<String>)> = None;
    let mut action_health_cancel: Option<(bool, Vec<String>)> = None;
    let mut action_save_apply = false;

    {
        let cfg =
            working_legacy_proxy_settings_mut(&mut ctx.view.proxy_settings).expect("legacy branch");
        ui.columns(2, |cols| {
            cols[0].heading(pick(ctx.lang, "站点列表", "Stations"));
            cols[0].add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt("config_configs_scroll")
                .max_height(520.0)
                .show(&mut cols[0], |ui| {
                    for name in names.iter() {
                        let is_active = active_name.as_deref() == Some(name.as_str());
                        let is_fallback_active = active_name.is_none()
                            && active_fallback.as_deref() == Some(name.as_str());
                        let is_selected = selected_name.as_deref() == Some(name.as_str());

                        let svc = match selected_service_kind {
                            crate::config::ServiceKind::Claude => cfg.claude.station(name),
                            crate::config::ServiceKind::Codex => cfg.codex.station(name),
                        };

                        let (enabled, level, alias, upstreams) = svc
                            .map(|s| {
                                (
                                    s.enabled,
                                    s.level.clamp(1, 10),
                                    s.alias.as_deref().unwrap_or(""),
                                    s.upstreams.len(),
                                )
                            })
                            .unwrap_or((false, 1, "", 0));

                        let mut label = format!("L{level} {name}");
                        if !alias.trim().is_empty() {
                            label.push_str(&format!(" ({alias})"));
                        }
                        label.push_str(&format!("  up={upstreams}"));
                        if !enabled {
                            label.push_str("  [off]");
                        }
                        if is_active {
                            label = format!("★ {label}");
                        } else if is_fallback_active {
                            label = format!("◇ {label}");
                        }

                        if ui.selectable_label(is_selected, label).clicked() {
                            selected_name = Some(name.clone());
                        }
                    }
                });

            cols[1].heading(pick(ctx.lang, "详情", "Details"));
            cols[1].add_space(4.0);

            let Some(name) = selected_name.clone() else {
                cols[1].label(pick(ctx.lang, "未选择站点。", "No station selected."));
                return;
            };

            let mgr = match selected_service_kind {
                crate::config::ServiceKind::Claude => &mut cfg.claude,
                crate::config::ServiceKind::Codex => &mut cfg.codex,
            };
            let active_label = mgr
                .active
                .clone()
                .unwrap_or_else(|| pick(ctx.lang, "<自动>", "<auto>").to_string());
            let effective_label = mgr
                .active_station()
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "-".to_string());

            cols[1].label(format!("active: {active_label}"));
            cols[1].label(format!(
                "{}: {effective_label}",
                pick(ctx.lang, "生效配置", "Effective")
            ));
            cols[1].add_space(6.0);

            let Some(svc) = mgr.station_mut(&name) else {
                cols[1].label(pick(
                    ctx.lang,
                    "设置不存在（可能已被删除）。",
                    "Settings missing.",
                ));
                return;
            };

            cols[1].label(format!("name: {}", svc.name));
            cols[1].label(format!("alias: {}", svc.alias.as_deref().unwrap_or("-")));
            cols[1].label(format!("upstreams: {}", svc.upstreams.len()));
            cols[1].add_space(6.0);

            cols[1].horizontal(|ui| {
                ui.checkbox(&mut svc.enabled, pick(ctx.lang, "启用", "Enabled"));
                ui.label(pick(ctx.lang, "等级", "Level"));
                ui.add(egui::DragValue::new(&mut svc.level).range(1..=10));
            });

            cols[1].add_space(8.0);
            cols[1].separator();
            cols[1].label(pick(ctx.lang, "健康检查", "Health check"));

            let selected_service = match selected_service_kind {
                crate::config::ServiceKind::Claude => "claude",
                crate::config::ServiceKind::Codex => "codex",
            };

            let (runtime_service, supports_v1, cfg_health, hc_status): (
                Option<String>,
                bool,
                Option<StationHealth>,
                Option<HealthCheckStatus>,
            ) = match ctx.proxy.kind() {
                ProxyModeKind::Running => {
                    if let Some(r) = ctx.proxy.running() {
                        let state = r.state.clone();
                        let (health, checks) = ctx.rt.block_on(async {
                            tokio::join!(
                                state.get_station_health(r.service_name),
                                state.list_health_checks(r.service_name)
                            )
                        });
                        (
                            Some(r.service_name.to_string()),
                            true,
                            health.get(&name).cloned(),
                            checks.get(&name).cloned(),
                        )
                    } else {
                        (None, false, None, None)
                    }
                }
                ProxyModeKind::Attached => {
                    if let Some(att) = ctx.proxy.attached() {
                        (
                            att.service_name.clone(),
                            att.api_version == Some(1),
                            att.station_health.get(&name).cloned(),
                            att.health_checks.get(&name).cloned(),
                        )
                    } else {
                        (None, false, None, None)
                    }
                }
                _ => (None, false, None, None),
            };

            if runtime_service.is_none() {
                cols[1].label(pick(
                    ctx.lang,
                    "代理未运行/未附着，无法执行健康检查。",
                    "Proxy is not running/attached; health check disabled.",
                ));
            } else if !supports_v1 {
                cols[1].label(pick(
                    ctx.lang,
                    "附着代理未启用 API v1：健康检查不可用。",
                    "Attached proxy has no API v1: health check disabled.",
                ));
            } else if runtime_service.as_deref() != Some(selected_service) {
                cols[1].label(pick(
                    ctx.lang,
                    "当前代理服务与所选服务不一致：健康检查已禁用。",
                    "Runtime service differs from selected service: health check disabled.",
                ));
            } else {
                if let Some(st) = hc_status.as_ref() {
                    cols[1].label(format!(
                        "status: {}/{} ok={} err={} cancel={} done={}",
                        st.completed, st.total, st.ok, st.err, st.cancel_requested, st.done
                    ));
                    if let Some(e) = st.last_error.as_deref() {
                        cols[1].colored_label(egui::Color32::from_rgb(200, 120, 40), e);
                    }
                } else {
                    cols[1].label(pick(ctx.lang, "(无状态)", "(no status)"));
                }

                cols[1].horizontal(|ui| {
                    if ui
                        .button(pick(ctx.lang, "探测当前", "Probe selected"))
                        .clicked()
                    {
                        action_probe_selected = Some(name.clone());
                    }
                    if ui
                        .button(pick(ctx.lang, "取消当前", "Cancel selected"))
                        .clicked()
                    {
                        action_health_cancel = Some((false, vec![name.clone()]));
                    }
                    if ui.button(pick(ctx.lang, "检查全部", "Check all")).clicked() {
                        action_health_start = Some((true, Vec::new()));
                    }
                    if ui
                        .button(pick(ctx.lang, "取消全部", "Cancel all"))
                        .clicked()
                    {
                        action_health_cancel = Some((true, Vec::new()));
                    }
                });

                if let Some(h) = cfg_health.as_ref() {
                    cols[1].add_space(6.0);
                    cols[1].label(format!(
                        "{}: {}  upstreams={}",
                        pick(ctx.lang, "最近检查", "Last checked"),
                        h.checked_at_ms,
                        h.upstreams.len()
                    ));
                    egui::ScrollArea::vertical()
                        .id_salt("station_health_upstreams_scroll")
                        .max_height(160.0)
                        .show(&mut cols[1], |ui| {
                            let max = 12usize;
                            for up in h.upstreams.iter().rev().take(max) {
                                let ok = up.ok.map(|v| if v { "ok" } else { "err" }).unwrap_or("-");
                                let sc = up
                                    .status_code
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| "-".to_string());
                                let lat = up
                                    .latency_ms
                                    .map(|v| format!("{v}ms"))
                                    .unwrap_or_else(|| "-".to_string());
                                let err = up
                                    .error
                                    .as_deref()
                                    .map(|e| shorten(e, 60))
                                    .unwrap_or_else(|| "-".to_string());
                                ui.label(format!(
                                    "{ok} {sc} {lat}  {}  {err}",
                                    shorten_middle(&up.base_url, 48)
                                ));
                            }
                            if h.upstreams.len() > max {
                                ui.label(format!("… +{} more", h.upstreams.len() - max));
                            }
                        });
                }
            }

            cols[1].add_space(6.0);
            cols[1].horizontal(|ui| {
                if ui
                    .button(pick(ctx.lang, "设为 active", "Set active"))
                    .clicked()
                {
                    action_set_active = Some(name.clone());
                }

                if ui
                    .button(pick(ctx.lang, "清除 active", "Clear active"))
                    .clicked()
                {
                    action_clear_active = true;
                }

                if ui
                    .button(pick(ctx.lang, "保存并应用", "Save & apply"))
                    .clicked()
                {
                    action_save_apply = true;
                }
            });
        });
    }

    ctx.view.proxy_settings.selected_name = selected_name;

    if let Some(name) = action_set_active {
        let selected_service_kind = ctx.view.proxy_settings.service;
        let cfg =
            working_legacy_proxy_settings_mut(&mut ctx.view.proxy_settings).expect("legacy branch");
        let mgr = match selected_service_kind {
            crate::config::ServiceKind::Claude => &mut cfg.claude,
            crate::config::ServiceKind::Codex => &mut cfg.codex,
        };
        mgr.active = Some(name);
        *ctx.last_info = Some(pick(ctx.lang, "已设置 active", "Active set").to_string());
    }

    if action_clear_active {
        let selected_service_kind = ctx.view.proxy_settings.service;
        let cfg =
            working_legacy_proxy_settings_mut(&mut ctx.view.proxy_settings).expect("legacy branch");
        let mgr = match selected_service_kind {
            crate::config::ServiceKind::Claude => &mut cfg.claude,
            crate::config::ServiceKind::Codex => &mut cfg.codex,
        };
        mgr.active = None;
        *ctx.last_info = Some(pick(ctx.lang, "已清除 active", "Active cleared").to_string());
    }

    if let Some((all, names)) = action_health_start {
        if let Err(e) = ctx.proxy.start_health_checks(ctx.rt, all, names) {
            *ctx.last_error = Some(format!("health check start failed: {e}"));
        } else {
            *ctx.last_info =
                Some(pick(ctx.lang, "已开始健康检查", "Health check started").to_string());
        }
    }

    if let Some(name) = action_probe_selected {
        if let Err(e) = ctx.proxy.probe_station(ctx.rt, name) {
            *ctx.last_error = Some(format!("station probe failed: {e}"));
        } else {
            *ctx.last_info = Some(pick(ctx.lang, "已开始探测", "Probe started").to_string());
        }
    }

    if let Some((all, names)) = action_health_cancel {
        if let Err(e) = ctx.proxy.cancel_health_checks(ctx.rt, all, names) {
            *ctx.last_error = Some(format!("health check cancel failed: {e}"));
        } else {
            *ctx.last_info = Some(pick(ctx.lang, "已请求取消", "Cancel requested").to_string());
        }
    }

    if action_save_apply {
        let save_res = {
            let cfg = ctx
                .view
                .proxy_settings
                .working
                .as_ref()
                .expect("checked above");
            save_proxy_settings_document(ctx.rt, cfg)
        };
        match save_res {
            Ok(()) => {
                let new_path = crate::config::config_file_path();
                if let Ok(t) = std::fs::read_to_string(&new_path) {
                    *ctx.proxy_settings_text = t;
                }
                if let Ok(t) = std::fs::read_to_string(&new_path)
                    && let Ok(parsed) = parse_proxy_settings_document(&t)
                {
                    ctx.view.proxy_settings.working = Some(parsed);
                }

                if matches!(
                    ctx.proxy.kind(),
                    ProxyModeKind::Running | ProxyModeKind::Attached
                ) && let Err(e) = ctx.proxy.reload_runtime_config(ctx.rt)
                {
                    *ctx.last_error = Some(format!("reload runtime failed: {e}"));
                }

                *ctx.last_info = Some(pick(ctx.lang, "已保存", "Saved").to_string());
                *ctx.last_error = None;
            }
            Err(e) => {
                *ctx.last_error = Some(format!("save failed: {e}"));
            }
        }
    }
}

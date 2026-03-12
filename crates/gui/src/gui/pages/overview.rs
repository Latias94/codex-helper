use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "总览", "Overview"));

    ui.separator();

    let mut action_scan_local_proxies = false;
    let mut action_attach_discovered: Option<DiscoveredProxy> = None;

    // Sync defaults from GUI config (so Settings changes take effect without restart).
    // Avoid overriding the UI state while running/attached.
    if matches!(ctx.proxy.kind(), ProxyModeKind::Stopped) {
        ctx.proxy
            .set_defaults(ctx.gui_cfg.proxy.default_port, ctx.gui_cfg.service_kind());
    }

    ui.group(|ui| {
        ui.heading(pick(ctx.lang, "连接与路由", "Connection & routing"));

        let kind = ctx.proxy.kind();
        let status_text = match kind {
            ProxyModeKind::Running => pick(ctx.lang, "运行中", "Running"),
            ProxyModeKind::Attached => pick(ctx.lang, "已附着", "Attached"),
            ProxyModeKind::Starting => pick(ctx.lang, "启动中", "Starting"),
            ProxyModeKind::Stopped => pick(ctx.lang, "未运行", "Stopped"),
        };
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "状态", "Status"),
            status_text
        ));

        if let Some(s) = ctx.proxy.snapshot() {
            if let Some(base) = s.base_url.as_deref() {
                ui.label(format!("{}: {base}", pick(ctx.lang, "地址", "Base URL")));
            }
            if let Some(svc) = s.service_name.as_deref() {
                ui.label(format!("{}: {svc}", pick(ctx.lang, "服务", "Service")));
            }
            if let Some(port) = s.port {
                ui.label(format!("{}: {port}", pick(ctx.lang, "端口", "Port")));
            }
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "API", "API"),
                if s.supports_v1 { "v1" } else { "legacy" }
            ));
        }

        let can_edit = matches!(kind, ProxyModeKind::Stopped);
        ui.horizontal(|ui| {
            ui.label(pick(ctx.lang, "服务", "Service"));
            ui.add_enabled_ui(can_edit, |ui| {
                let mut svc = ctx.proxy.desired_service();
                egui::ComboBox::from_id_salt("proxy_service")
                    .selected_text(match svc {
                        crate::config::ServiceKind::Codex => "codex",
                        crate::config::ServiceKind::Claude => "claude",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut svc, crate::config::ServiceKind::Codex, "codex");
                        ui.selectable_value(&mut svc, crate::config::ServiceKind::Claude, "claude");
                    });
                if svc != ctx.proxy.desired_service() {
                    ctx.proxy.set_desired_service(svc);
                    ctx.gui_cfg.set_service_kind(svc);
                    if let Err(e) = ctx.gui_cfg.save() {
                        *ctx.last_error = Some(format!("save gui config failed: {e}"));
                    }
                }
            });

            ui.add_space(12.0);
            ui.label(pick(ctx.lang, "端口", "Port"));
            ui.add_enabled_ui(can_edit, |ui| {
                let mut port = ctx.proxy.desired_port();
                ui.add(egui::DragValue::new(&mut port).range(1..=65535));
                if port != ctx.proxy.desired_port() {
                    ctx.proxy.set_desired_port(port);
                    ctx.gui_cfg.proxy.default_port = port;
                    if let Err(e) = ctx.gui_cfg.save() {
                        *ctx.last_error = Some(format!("save gui config failed: {e}"));
                    }
                }
            });

            if !can_edit {
                ui.colored_label(
                    egui::Color32::from_rgb(120, 120, 120),
                    pick(ctx.lang, "（停止后可修改）", "(stop to edit)"),
                );
            }
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            match kind {
                ProxyModeKind::Stopped => {
                    if ui
                        .button(pick(ctx.lang, "启动代理", "Start proxy"))
                        .clicked()
                    {
                        let action = PortInUseAction::parse(&ctx.gui_cfg.attach.on_port_in_use);
                        ctx.proxy.request_start_or_prompt(
                            ctx.rt,
                            action,
                            ctx.gui_cfg.attach.remember_choice,
                        );

                        if let Some(e) = ctx.proxy.last_start_error() {
                            *ctx.last_error = Some(e.to_string());
                        }
                    }
                }
                ProxyModeKind::Running => {
                    if ui
                        .button(pick(ctx.lang, "停止代理", "Stop proxy"))
                        .clicked()
                    {
                        if let Err(e) = ctx.proxy.stop(ctx.rt) {
                            *ctx.last_error = Some(format!("stop failed: {e}"));
                        } else {
                            *ctx.last_info = Some(pick(ctx.lang, "已停止", "Stopped").to_string());
                        }
                    }
                }
                ProxyModeKind::Attached => {
                    if ui.button(pick(ctx.lang, "取消附着", "Detach")).clicked() {
                        ctx.proxy.clear_port_in_use_modal();
                        ctx.proxy.detach();
                        *ctx.last_info = Some(pick(ctx.lang, "已取消附着", "Detached").to_string());
                    }
                }
                ProxyModeKind::Starting => {
                    ui.spinner();
                }
            }

            if matches!(kind, ProxyModeKind::Running | ProxyModeKind::Attached)
                && ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked()
            {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
            }

            if matches!(kind, ProxyModeKind::Running | ProxyModeKind::Attached)
                && ui
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
                .button(pick(ctx.lang, "扫描 3210-3220", "Scan 3210-3220"))
                .clicked()
            {
                action_scan_local_proxies = true;
            }
            if let Some(t) = ctx.proxy.last_discovery_scan() {
                ui.label(format!(
                    "{}: {}s",
                    pick(ctx.lang, "上次扫描", "Last scan"),
                    t.elapsed().as_secs()
                ));
            }
        });

        ui.add_space(6.0);
        ui.collapsing(
            pick(
                ctx.lang,
                "附着到已运行的代理",
                "Attach to an existing proxy",
            ),
            |ui| {
                if !matches!(kind, ProxyModeKind::Stopped) {
                    ui.colored_label(
                        egui::Color32::from_rgb(120, 120, 120),
                        pick(
                            ctx.lang,
                            "提示：请先停止/取消附着，再切换到其他代理。",
                            "Tip: stop/detach first before switching to another proxy.",
                        ),
                    );
                }

                ui.horizontal(|ui| {
                    ui.label(pick(ctx.lang, "端口", "Port"));
                    let mut attach_port = ctx
                        .gui_cfg
                        .attach
                        .last_port
                        .unwrap_or(ctx.gui_cfg.proxy.default_port);
                    ui.add(egui::DragValue::new(&mut attach_port).range(1..=65535));
                    if Some(attach_port) != ctx.gui_cfg.attach.last_port {
                        ctx.gui_cfg.attach.last_port = Some(attach_port);
                        if let Err(e) = ctx.gui_cfg.save() {
                            *ctx.last_error = Some(format!("save gui config failed: {e}"));
                        }
                    }

                    if ui
                        .add_enabled(
                            matches!(kind, ProxyModeKind::Stopped),
                            egui::Button::new(pick(ctx.lang, "附着", "Attach")),
                        )
                        .clicked()
                    {
                        ctx.proxy.request_attach(attach_port);
                        ctx.gui_cfg.attach.last_port = Some(attach_port);
                        if let Err(e) = ctx.gui_cfg.save() {
                            *ctx.last_error = Some(format!("save gui config failed: {e}"));
                        } else {
                            *ctx.last_info =
                                Some(pick(ctx.lang, "正在附着…", "Attaching...").into());
                        }
                    }
                });

                let discovered = ctx.proxy.discovered_proxies().to_vec();
                if discovered.is_empty() {
                    ui.label(pick(ctx.lang, "（未发现可用代理）", "(no proxies found)"));
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("overview_discovered_proxies_scroll")
                        .max_height(180.0)
                        .show(ui, |ui| {
                            egui::Grid::new("discovered_proxies_grid")
                                .striped(true)
                                .show(ui, |ui| {
                                    ui.label(pick(ctx.lang, "端口", "Port"));
                                    ui.label(pick(ctx.lang, "服务", "Service"));
                                    ui.label(pick(ctx.lang, "API", "API"));
                                    ui.label(pick(ctx.lang, "状态", "Status"));
                                    ui.end_row();

                                    let now = now_ms();
                                    for p in discovered {
                                        let mut hover = format!("base_url: {}", p.base_url);
                                        if !p.endpoints.is_empty() {
                                            hover.push_str(&format!(
                                                "\nendpoints: {}",
                                                p.endpoints.len()
                                            ));
                                        }
                                        if let Some(ms) = p.runtime_loaded_at_ms {
                                            hover.push_str(&format!(
                                                "\nruntime_loaded: {}",
                                                format_age(now, Some(ms))
                                            ));
                                        }
                                        ui.label(p.port.to_string()).on_hover_text(hover);
                                        ui.label(
                                            p.service_name.as_deref().unwrap_or_else(|| {
                                                pick(ctx.lang, "未知", "unknown")
                                            }),
                                        );
                                        ui.label(match p.api_version {
                                            Some(v) => format!("v{v}"),
                                            None => "-".to_string(),
                                        });
                                        ui.vertical(|ui| {
                                            if let Some(err) = p.last_error.as_deref() {
                                                ui.label(err);
                                            } else {
                                                ui.label(pick(ctx.lang, "可用", "OK"));
                                            }
                                            if let Some(warning) = remote_local_only_warning_message(
                                                p.admin_base_url.as_str(),
                                                &p.host_local_capabilities,
                                                ctx.lang,
                                                &[
                                                    pick(ctx.lang, "cwd", "cwd"),
                                                    pick(ctx.lang, "transcript", "transcript"),
                                                    pick(ctx.lang, "resume", "resume"),
                                                ],
                                            ) {
                                                ui.small(pick(
                                                    ctx.lang,
                                                    "远端附着：本机禁用 host-local 功能",
                                                    "Remote attach: no host-local access here",
                                                ))
                                                .on_hover_text(warning);
                                            }
                                            if let Some(label) = remote_admin_access_short_label(
                                                p.admin_base_url.as_str(),
                                                &p.remote_admin_access,
                                                ctx.lang,
                                            ) {
                                                let color = if p.remote_admin_access.remote_enabled
                                                    && remote_admin_token_present()
                                                {
                                                    egui::Color32::from_rgb(60, 160, 90)
                                                } else {
                                                    egui::Color32::from_rgb(200, 120, 40)
                                                };
                                                let response = ui.colored_label(color, label);
                                                if let Some(message) = remote_admin_access_message(
                                                    p.admin_base_url.as_str(),
                                                    &p.remote_admin_access,
                                                    ctx.lang,
                                                ) {
                                                    response.on_hover_text(message);
                                                }
                                            }
                                        });

                                        if ui
                                            .add_enabled(
                                                matches!(kind, ProxyModeKind::Stopped),
                                                egui::Button::new(pick(ctx.lang, "附着", "Attach")),
                                            )
                                            .clicked()
                                        {
                                            action_attach_discovered = Some(p.clone());
                                        }
                                        ui.end_row();
                                    }
                                });
                        });
                }
            },
        );

        ui.add_space(8.0);
        ui.separator();
        stations::render_profile_management_entrypoint(ui, ctx);

        render_overview_station_summary(ui, ctx);
    });

    match ctx.proxy.kind() {
        ProxyModeKind::Stopped => {
            ui.add_space(8.0);
            ui.label(pick(
                ctx.lang,
                "提示：可在上方“连接与路由”面板启动或附着到代理。",
                "Tip: use the panel above to start or attach to a proxy.",
            ));
        }
        ProxyModeKind::Starting => {
            ui.label(pick(ctx.lang, "正在启动…", "Starting..."));
        }
        ProxyModeKind::Running => {
            if let Some(r) = ctx.proxy.running() {
                ui.label(format!(
                    "{}: 127.0.0.1:{} ({})",
                    pick(ctx.lang, "运行中", "Running"),
                    r.port,
                    r.service_name
                ));
                if let Some(err) = r.last_error.as_deref() {
                    ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
                }

                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "活跃请求", "Active requests"),
                    r.active.len()
                ));
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "最近请求(<=200)", "Recent (<=200)"),
                    r.recent.len()
                ));
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "全局覆盖(Pinned)", "Global override (pinned)"),
                    r.global_station_override
                        .as_deref()
                        .unwrap_or_else(|| pick(ctx.lang, "<自动>", "<auto>"))
                ));

                let active_name = match r.service_name {
                    "claude" => r.cfg.claude.active.clone(),
                    _ => r.cfg.codex.active.clone(),
                };
                let active_fallback = match r.service_name {
                    "claude" => r.cfg.claude.active_config().map(|c| c.name.clone()),
                    _ => r.cfg.codex.active_config().map(|c| c.name.clone()),
                };
                let active_display = active_name
                    .clone()
                    .or(active_fallback.clone())
                    .unwrap_or_else(|| "-".to_string());
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "当前站点(active)", "Active station"),
                    active_display
                ));

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(120, 120, 120),
                        pick(
                            ctx.lang,
                            "默认 active_station / global pin / drain / breaker 已移到 Stations 页集中操作。",
                            "Default active_station / global pin / drain / breaker now live in the Stations page.",
                        ),
                    );
                    if ui
                        .button(pick(ctx.lang, "打开 Stations 页", "Open Stations page"))
                        .clicked()
                    {
                        ctx.view.requested_page = Some(Page::Stations);
                    }
                });

                let warnings =
                    crate::config::model_routing_warnings(r.cfg.as_ref(), r.service_name);
                if !warnings.is_empty() {
                    ui.add_space(4.0);
                    ui.label(pick(
                        ctx.lang,
                        "模型路由配置警告（建议处理）：",
                        "Model routing warnings (recommended to fix):",
                    ));
                    egui::ScrollArea::vertical()
                        .id_salt("overview_model_routing_warnings_scroll")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for w in warnings {
                                ui.colored_label(egui::Color32::from_rgb(200, 120, 40), w);
                            }
                        });
                }
            }
        }
        ProxyModeKind::Attached => {
            if let Some(att) = ctx.proxy.attached() {
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "已附着", "Attached"),
                    att.base_url
                ));
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "活跃请求", "Active requests"),
                    att.active.len()
                ));
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "最近请求(<=200)", "Recent (<=200)"),
                    att.recent.len()
                ));
                if let Some(v) = att.api_version {
                    ui.label(format!(
                        "{}: v{}",
                        pick(ctx.lang, "API 版本", "API version"),
                        v
                    ));
                }
                if let Some(svc) = att.service_name.as_deref() {
                    ui.label(format!("{}: {svc}", pick(ctx.lang, "服务", "Service")));
                }
                if let Some(ms) = att.runtime_loaded_at_ms {
                    ui.label(format!(
                        "{}: {}",
                        pick(ctx.lang, "运行态配置 loaded_at_ms", "runtime loaded_at_ms"),
                        ms
                    ));
                }
                if let Some(ms) = att.runtime_source_mtime_ms {
                    ui.label(format!(
                        "{}: {}",
                        pick(ctx.lang, "运行态配置 mtime_ms", "runtime mtime_ms"),
                        ms
                    ));
                }
                if let Some(err) = att.last_error.as_deref() {
                    ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
                }
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "全局覆盖(Pinned)", "Global override (pinned)"),
                    att.global_station_override
                        .as_deref()
                        .unwrap_or_else(|| pick(ctx.lang, "<自动>", "<auto>"))
                ));
                ui.colored_label(
                    egui::Color32::from_rgb(120, 120, 120),
                    pick(
                        ctx.lang,
                        "提示：附着模式下不会改你的本机配置文件，但如果远端代理支持 API v1 扩展，上方的运行时控制仍可直接作用于该代理进程。",
                        "Tip: attached mode won't change your local config file, but runtime controls above can still act on the remote proxy process when supported.",
                    ),
                );
                if let Some(warning) = remote_local_only_warning_message(
                    att.admin_base_url.as_str(),
                    &att.host_local_capabilities,
                    ctx.lang,
                    &[
                        pick(ctx.lang, "cwd", "cwd"),
                        pick(ctx.lang, "transcript", "transcript"),
                        pick(ctx.lang, "resume", "resume"),
                        pick(ctx.lang, "open file", "open file"),
                    ],
                ) {
                    ui.colored_label(egui::Color32::from_rgb(200, 120, 40), warning);
                }
                if let Some(label) = remote_admin_access_short_label(
                    att.admin_base_url.as_str(),
                    &att.remote_admin_access,
                    ctx.lang,
                ) {
                    let color =
                        if att.remote_admin_access.remote_enabled && remote_admin_token_present() {
                            egui::Color32::from_rgb(60, 160, 90)
                        } else {
                            egui::Color32::from_rgb(200, 120, 40)
                        };
                    let response = ui.colored_label(color, label);
                    let remote_admin_message = remote_admin_access_message(
                        att.admin_base_url.as_str(),
                        &att.remote_admin_access,
                        ctx.lang,
                    );
                    if let Some(message) = remote_admin_message.clone() {
                        response.on_hover_text(message.clone());
                        if !att.remote_admin_access.remote_enabled || !remote_admin_token_present()
                        {
                            ui.colored_label(egui::Color32::from_rgb(200, 120, 40), message);
                        }
                    }
                }
            }
        }
    }

    if action_scan_local_proxies {
        if let Err(e) = ctx.proxy.scan_local_proxies(ctx.rt, 3210..=3220) {
            *ctx.last_error = Some(format!("scan failed: {e}"));
        } else if ctx.proxy.discovered_proxies().is_empty() {
            *ctx.last_info =
                Some(pick(ctx.lang, "扫描完成：未发现代理", "Scan done: none found").to_string());
        } else {
            *ctx.last_info =
                Some(pick(ctx.lang, "扫描完成：已列出可用代理", "Scan done").to_string());
        }
    }

    if let Some(proxy) = action_attach_discovered {
        let warning = remote_local_only_warning_message(
            proxy.admin_base_url.as_str(),
            &proxy.host_local_capabilities,
            ctx.lang,
            &[
                pick(ctx.lang, "cwd", "cwd"),
                pick(ctx.lang, "transcript", "transcript"),
                pick(ctx.lang, "resume", "resume"),
                pick(ctx.lang, "open file", "open file"),
            ],
        );
        let admin_message = remote_admin_access_message(
            proxy.admin_base_url.as_str(),
            &proxy.remote_admin_access,
            ctx.lang,
        );
        ctx.proxy
            .request_attach_with_admin_base(proxy.port, Some(proxy.admin_base_url.clone()));
        ctx.gui_cfg.attach.last_port = Some(proxy.port);
        if let Err(e) = ctx.gui_cfg.save() {
            *ctx.last_error = Some(format!("save gui config failed: {e}"));
        } else {
            *ctx.last_info = Some(merge_info_message(
                pick(ctx.lang, "正在附着…", "Attaching...").to_string(),
                [warning, admin_message].into_iter().flatten(),
            ));
        }
    }

    // Port-in-use modal (only shown when action is Ask).
    if ctx.proxy.show_port_in_use_modal() {
        let mut open = true;
        egui::Window::new(pick(ctx.lang, "端口已被占用", "Port is in use"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                let port = ctx.proxy.desired_port();
                ui.label(format!(
                    "{}: 127.0.0.1:{}",
                    pick(ctx.lang, "监听端口冲突", "Bind conflict"),
                    port
                ));
                ui.add_space(8.0);

                let mut remember = ctx.proxy.port_in_use_modal_remember();
                ui.checkbox(
                    &mut remember,
                    pick(
                        ctx.lang,
                        "记住我的选择（下次不再弹窗）",
                        "Remember my choice",
                    ),
                );
                ctx.proxy.set_port_in_use_modal_remember(remember);

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(pick(ctx.lang, "附着到现有代理", "Attach"))
                        .clicked()
                    {
                        if remember {
                            ctx.gui_cfg.attach.remember_choice = true;
                            ctx.gui_cfg.attach.on_port_in_use =
                                PortInUseAction::Attach.as_str().to_string();
                            let _ = ctx.gui_cfg.save();
                        }
                        ctx.proxy.confirm_port_in_use_attach();
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(pick(ctx.lang, "换端口启动", "Start on another port"));
                    let mut p = ctx
                        .proxy
                        .port_in_use_modal_suggested_port()
                        .unwrap_or(port.saturating_add(1));
                    ui.add(egui::DragValue::new(&mut p).range(1..=65535));
                    ctx.proxy.set_port_in_use_modal_new_port(p);
                    if ui.button(pick(ctx.lang, "启动", "Start")).clicked() {
                        if remember {
                            ctx.gui_cfg.attach.remember_choice = true;
                            ctx.gui_cfg.attach.on_port_in_use =
                                PortInUseAction::StartNewPort.as_str().to_string();
                            let _ = ctx.gui_cfg.save();
                        }
                        ctx.proxy.confirm_port_in_use_new_port(ctx.rt);
                    }
                });

                ui.horizontal(|ui| {
                    if ui.button(pick(ctx.lang, "退出", "Exit")).clicked() {
                        if remember {
                            ctx.gui_cfg.attach.remember_choice = true;
                            ctx.gui_cfg.attach.on_port_in_use =
                                PortInUseAction::Exit.as_str().to_string();
                            let _ = ctx.gui_cfg.save();
                        }
                        ctx.proxy.confirm_port_in_use_exit();
                    }
                });
            });

        if !open {
            ctx.proxy.clear_port_in_use_modal();
        }
    }
}

fn render_overview_station_summary(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let Some(snapshot) = ctx.proxy.snapshot() else {
        return;
    };
    if snapshot.stations.is_empty() {
        return;
    }

    let runtime_maps = runtime_station_maps(ctx.proxy);
    let override_count = snapshot
        .stations
        .iter()
        .filter(|cfg| {
            cfg.runtime_enabled_override.is_some()
                || cfg.runtime_level_override.is_some()
                || cfg.runtime_state_override.is_some()
        })
        .count();
    let health_count = runtime_maps.station_health.len();
    let active_station = current_runtime_active_station(ctx.proxy);

    ui.add_space(8.0);
    ui.separator();
    ui.label(pick(ctx.lang, "站点控制摘要", "Stations summary"));
    ui.horizontal(|ui| {
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "站点数", "Stations"),
            snapshot.stations.len()
        ));
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "健康记录", "Health records"),
            health_count
        ));
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "运行时覆盖", "Runtime overrides"),
            override_count
        ));
        if ui
            .button(pick(ctx.lang, "打开 Stations 页", "Open Stations page"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Stations);
        }
    });
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "全局站点覆盖", "Global pinned station"),
        snapshot
            .global_station_override
            .as_deref()
            .unwrap_or_else(|| pick(ctx.lang, "<自动>", "<auto>"))
    ));
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "当前 active_station", "Current active_station"),
        active_station.as_deref().unwrap_or_else(|| pick(
            ctx.lang,
            "<未知/仅本机可见>",
            "<unknown/local-only>"
        ))
    ));
    ui.colored_label(
        egui::Color32::from_rgb(120, 120, 120),
        pick(
            ctx.lang,
            "更细的 quick switch、drain、breaker、健康检查已经移到单独的 Stations 页。",
            "Detailed quick switch, drain, breaker, and health controls now live in the dedicated Stations page.",
        ),
    );
}


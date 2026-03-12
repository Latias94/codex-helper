use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "快速设置", "Setup"));
    ui.label(pick(
        ctx.lang,
        "目标：让 Codex/Claude 走本地 codex-helper 代理（常驻后台），并完成基础配置。",
        "Goal: route Codex/Claude through the local codex-helper proxy (resident) and complete basic setup.",
    ));
    ui.label(pick(
        ctx.lang,
        "推荐顺序：先 1) 配置，再 2) 启动/附着代理，最后 3) 切换客户端。如果你已在 TUI 启动代理，请在第 2 步使用“扫描并附着”。",
        "Recommended order: 1) config, 2) start/attach proxy, 3) switch client. If you already started the proxy in TUI, use “Scan & attach” in step 2.",
    ));
    ui.separator();

    // Step 1: proxy config
    let cfg_path = ctx.proxy_config_path.to_path_buf();
    let cfg_exists = cfg_path.exists() && !ctx.proxy_config_text.trim().is_empty();

    ui.group(|ui| {
        ui.heading(pick(
            ctx.lang,
            "1) 生成/导入配置",
            "1) Create/import config",
        ));
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "配置文件", "Config file"),
            cfg_path.display()
        ));

        if cfg_exists {
            ui.colored_label(
                egui::Color32::from_rgb(60, 160, 90),
                pick(ctx.lang, "已就绪", "Ready"),
            );
            if ui
                .button(pick(ctx.lang, "打开配置文件", "Open config file"))
                .clicked()
                && let Err(e) = open_in_file_manager(&cfg_path, true)
            {
                *ctx.last_error = Some(format!("open config failed: {e}"));
            }
            if ui
                .button(pick(ctx.lang, "前往配置页", "Go to Config page"))
                .clicked()
            {
                ctx.view.requested_page = Some(Page::Config);
            }
        } else {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(
                    ctx.lang,
                    "未检测到有效配置（建议先创建）",
                    "Config not found (create one first)",
                ),
            );
            ui.checkbox(
                &mut ctx.view.setup.import_codex_on_init,
                pick(
                    ctx.lang,
                    "自动从 ~/.codex/config.toml + auth.json 导入 Codex upstream",
                    "Auto-import Codex upstreams from ~/.codex/config.toml + auth.json",
                ),
            );

            if ui
                .button(pick(ctx.lang, "创建 config.toml", "Create config.toml"))
                .clicked()
            {
                match ctx.rt.block_on(crate::config::init_config_toml(
                    false,
                    ctx.view.setup.import_codex_on_init,
                )) {
                    Ok(path) => {
                        *ctx.last_info = Some(format!(
                            "{}: {}",
                            pick(ctx.lang, "已写入配置", "Wrote config"),
                            path.display()
                        ));
                        *ctx.proxy_config_text =
                            std::fs::read_to_string(ctx.proxy_config_path).unwrap_or_default();
                    }
                    Err(e) => *ctx.last_error = Some(format!("init config failed: {e}")),
                }
            }
        }
    });

    ui.add_space(10.0);

    let mut action_scan_local_proxies = false;
    let mut action_attach_discovered: Option<DiscoveredProxy> = None;

    // Step 2: start proxy
    ui.group(|ui| {
        ui.heading(pick(ctx.lang, "2) 启动本地代理", "2) Start local proxy"));

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

        ui.horizontal(|ui| {
            ui.label(pick(ctx.lang, "服务", "Service"));
            let mut svc = ctx.proxy.desired_service();
            egui::ComboBox::from_id_salt("setup_service")
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

            ui.add_space(12.0);
            ui.label(pick(ctx.lang, "端口", "Port"));
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

        ui.horizontal(|ui| {
            let can_start = matches!(ctx.proxy.kind(), ProxyModeKind::Stopped);
            if ui
                .add_enabled(
                    can_start,
                    egui::Button::new(pick(ctx.lang, "启动代理", "Start proxy")),
                )
                .clicked()
            {
                let action = PortInUseAction::parse(
                    &ctx.gui_cfg.attach.on_port_in_use,
                );
                ctx.proxy.request_start_or_prompt(
                    ctx.rt,
                    action,
                    ctx.gui_cfg.attach.remember_choice,
                );
            }

            let can_stop = matches!(
                ctx.proxy.kind(),
                ProxyModeKind::Running | ProxyModeKind::Attached
            );
            if ui
                .add_enabled(
                    can_stop,
                    egui::Button::new(pick(ctx.lang, "停止代理", "Stop proxy")),
                )
                .clicked()
            {
                if let Err(e) = ctx.proxy.stop(ctx.rt) {
                    *ctx.last_error = Some(format!("stop failed: {e}"));
                } else {
                    *ctx.last_info =
                        Some(pick(ctx.lang, "已停止代理", "Proxy stopped").to_string());
                }
            }
        });

        ui.add_space(8.0);
        ui.separator();
        ui.label(pick(
            ctx.lang,
            "已运行代理？（例如：你已在 TUI 中启动）",
            "Already running? (e.g. started from TUI)",
        ));
        ui.horizontal(|ui| {
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

        let discovered = ctx.proxy.discovered_proxies().to_vec();
        if discovered.is_empty() {
            ui.label(pick(ctx.lang, "（未发现可用代理）", "(no proxies found)"));
        } else {
            egui::ScrollArea::vertical()
                .id_salt("setup_discovered_proxies_scroll")
                .max_height(160.0)
                .show(ui, |ui| {
                    egui::Grid::new("setup_discovered_proxies_grid")
                        .striped(true)
                        .show(ui, |ui| {
                            ui.label(pick(ctx.lang, "端口", "Port"));
                            ui.label(pick(ctx.lang, "服务", "Service"));
                            ui.label(pick(ctx.lang, "API", "API"));
                            ui.label(pick(ctx.lang, "状态", "Status"));
                            ui.end_row();

                            for p in discovered {
                                ui.label(p.port.to_string());
                                ui.label(
                                    p.service_name
                                        .as_deref()
                                        .unwrap_or_else(|| pick(ctx.lang, "未知", "unknown")),
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

                                if ui.button(pick(ctx.lang, "附着", "Attach")).clicked() {
                                    action_attach_discovered = Some(p.clone());
                                }
                                ui.end_row();
                            }
                        });
                });
        }
    });

    ui.add_space(10.0);

    // Step 3: switch client
    ui.group(|ui| {
        ui.heading(pick(ctx.lang, "3) 让客户端走本地代理", "3) Point client to local proxy"));

        let svc = ctx.proxy.desired_service();
        let port = ctx
            .proxy
            .snapshot()
            .and_then(|s| s.port)
            .unwrap_or(ctx.proxy.desired_port());

        match svc {
            crate::config::ServiceKind::Claude => {
                let st = crate::codex_integration::claude_switch_status();
                match st {
                    Ok(st) => {
                        ui.label(format!(
                            "{}: {}",
                            pick(ctx.lang, "Claude settings", "Claude settings"),
                            st.settings_path.display()
                        ));
                        ui.label(format!(
                            "{}: {}",
                            pick(ctx.lang, "当前 ANTHROPIC_BASE_URL", "Current ANTHROPIC_BASE_URL"),
                            st.base_url.as_deref().unwrap_or("-")
                        ));
                        if st.enabled {
                            ui.colored_label(
                                egui::Color32::from_rgb(60, 160, 90),
                                pick(ctx.lang, "已启用（本地代理）", "Enabled (local proxy)"),
                            );
                            if !st.has_backup {
                                ui.colored_label(
                                    egui::Color32::from_rgb(200, 120, 40),
                                    pick(
                                        ctx.lang,
                                        "提示：当前已指向本地代理但未找到备份文件；请勿重复 switch on，否则备份可能覆盖原始配置。",
                                        "Tip: enabled but no backup found; avoid repeated switch on (backup may not represent the original config).",
                                    ),
                                );
                            }
                        } else {
                            ui.colored_label(
                                egui::Color32::from_rgb(200, 120, 40),
                                pick(ctx.lang, "未启用", "Not enabled"),
                            );
                        }

                        ui.horizontal(|ui| {
                            let enable_label = match ctx.lang {
                                Language::Zh => format!("启用（端口 {port}）"),
                                Language::En => format!("Enable (port {port})"),
                            };
                            if ui
                                .add_enabled(
                                    !st.enabled,
                                    egui::Button::new(enable_label),
                                )
                                .clicked()
                            {
                                match crate::codex_integration::claude_switch_on(port) {
                                    Ok(()) => {
                                        *ctx.last_info = Some(pick(
                                            ctx.lang,
                                            "已更新 Claude settings 指向本地代理",
                                            "Updated Claude settings to local proxy",
                                        )
                                        .to_string());
                                    }
                                    Err(e) => *ctx.last_error = Some(format!("switch on failed: {e}")),
                                }
                            }

                            if ui
                                .add_enabled(
                                    st.has_backup,
                                    egui::Button::new(pick(ctx.lang, "恢复（从备份）", "Restore (from backup)")),
                                )
                                .clicked()
                            {
                                match crate::codex_integration::claude_switch_off() {
                                    Ok(()) => {
                                        *ctx.last_info = Some(pick(
                                            ctx.lang,
                                            "已从备份恢复 Claude settings",
                                            "Restored Claude settings from backup",
                                        )
                                        .to_string());
                                    }
                                    Err(e) => *ctx.last_error = Some(format!("switch off failed: {e}")),
                                }
                            }
                        });
                    }
                    Err(e) => *ctx.last_error = Some(format!("read claude switch status failed: {e}")),
                }
            }
            _ => {
                let st = crate::codex_integration::codex_switch_status();
                match st {
                    Ok(st) => {
                        ui.label(pick(
                            ctx.lang,
                            "Codex 将通过 ~/.codex/config.toml 的 model_provider 指向本地代理。",
                            "Codex will route through ~/.codex/config.toml (model_provider).",
                        ));
                        ui.label(format!(
                            "{}: {}",
                            pick(ctx.lang, "当前 model_provider", "Current model_provider"),
                            st.model_provider.as_deref().unwrap_or("-")
                        ));
                        ui.label(format!(
                            "{}: {}",
                            pick(ctx.lang, "当前 base_url", "Current base_url"),
                            st.base_url.as_deref().unwrap_or("-")
                        ));
                        if st.enabled {
                            ui.colored_label(
                                egui::Color32::from_rgb(60, 160, 90),
                                pick(ctx.lang, "已启用（本地代理）", "Enabled (local proxy)"),
                            );
                            if !st.has_backup {
                                ui.colored_label(
                                    egui::Color32::from_rgb(200, 120, 40),
                                    pick(
                                        ctx.lang,
                                        "提示：当前已指向本地代理但未找到备份文件；请勿重复 switch on，否则备份可能覆盖原始配置。",
                                        "Tip: enabled but no backup found; avoid repeated switch on (backup may not represent the original config).",
                                    ),
                                );
                            }
                        } else {
                            ui.colored_label(
                                egui::Color32::from_rgb(200, 120, 40),
                                pick(ctx.lang, "未启用", "Not enabled"),
                            );
                        }

                        ui.horizontal(|ui| {
                            let enable_label = match ctx.lang {
                                Language::Zh => format!("启用（端口 {port}）"),
                                Language::En => format!("Enable (port {port})"),
                            };
                            if ui
                                .add_enabled(
                                    !st.enabled,
                                    egui::Button::new(enable_label),
                                )
                                .clicked()
                            {
                                match crate::codex_integration::switch_on(port) {
                                    Ok(()) => {
                                        *ctx.last_info = Some(pick(
                                            ctx.lang,
                                            "已更新 ~/.codex/config.toml 指向本地代理",
                                            "Updated ~/.codex/config.toml to local proxy",
                                        )
                                        .to_string());
                                    }
                                    Err(e) => *ctx.last_error = Some(format!("switch on failed: {e}")),
                                }
                            }

                            if ui
                                .add_enabled(
                                    st.has_backup,
                                    egui::Button::new(pick(ctx.lang, "恢复（从备份）", "Restore (from backup)")),
                                )
                                .clicked()
                            {
                                match crate::codex_integration::switch_off() {
                                    Ok(()) => {
                                        *ctx.last_info = Some(pick(
                                            ctx.lang,
                                            "已从备份恢复 ~/.codex/config.toml",
                                            "Restored ~/.codex/config.toml from backup",
                                        )
                                        .to_string());
                                    }
                                    Err(e) => *ctx.last_error = Some(format!("switch off failed: {e}")),
                                }
                            }
                        });

                        if !st.has_backup {
                            ui.colored_label(
                                egui::Color32::from_rgb(200, 120, 40),
                                pick(
                                    ctx.lang,
                                    "提示：未检测到备份文件（首次 switch on 时会自动创建备份）。",
                                    "Tip: no backup detected (a backup is created on first switch on).",
                                ),
                            );
                        }
                    }
                    Err(e) => *ctx.last_error = Some(format!("read codex switch status failed: {e}")),
                }
            }
        }
    });

    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if ui
            .button(pick(ctx.lang, "我已完成，前往总览", "Done, go to Overview"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Overview);
        }
    });

    if action_scan_local_proxies {
        if let Err(e) = ctx.proxy.scan_local_proxies(ctx.rt, 3210..=3220) {
            *ctx.last_error = Some(format!("scan failed: {e}"));
        } else if ctx.proxy.discovered_proxies().is_empty() {
            *ctx.last_info =
                Some(pick(ctx.lang, "扫描完成：未发现代理", "Scan done: none found").to_string());
        } else {
            *ctx.last_info = Some(
                pick(
                    ctx.lang,
                    "扫描完成：请选择一个代理进行附着",
                    "Scan done: pick a proxy to attach",
                )
                .to_string(),
            );
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
        ctx.proxy.set_desired_port(proxy.port);
        ctx.gui_cfg.attach.last_port = Some(proxy.port);
        ctx.gui_cfg.proxy.default_port = proxy.port;
        if let Err(e) = ctx.gui_cfg.save() {
            *ctx.last_error = Some(format!("save gui config failed: {e}"));
        } else {
            *ctx.last_info = Some(merge_info_message(
                pick(ctx.lang, "已附着到代理。", "Attached.").to_string(),
                [warning, admin_message].into_iter().flatten(),
            ));
        }
    }
}


use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "站点", "Stations"));
    ui.label(pick(
        ctx.lang,
        "面向 operator 的运行态站点面板：在这里集中查看站点能力、健康、熔断/冷却状态，并执行 quick switch 与运行时控制。",
        "Operator-focused runtime station panel: inspect station capabilities, health, breaker/cooldown state, and perform quick switch plus runtime control here.",
    ));

    let Some(snapshot) = ctx.proxy.snapshot() else {
        ui.add_space(8.0);
        ui.label(pick(
            ctx.lang,
            "当前没有运行中的本地代理，也没有附着到远端代理。请先在“总览”页启动或附着。",
            "No running or attached proxy is available. Start or attach one from Overview first.",
        ));
        if ui
            .button(pick(ctx.lang, "前往总览", "Go to Overview"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Overview);
        }
        return;
    };

    if snapshot.stations.is_empty() {
        ui.add_space(8.0);
        ui.label(pick(
            ctx.lang,
            "当前运行态没有可见站点。你可以先去“配置”页或原始配置文件里定义 station/provider。",
            "No stations are visible in the current runtime. Define stations/providers in Config first.",
        ));
        ui.horizontal(|ui| {
            if ui
                .button(pick(ctx.lang, "前往配置页", "Open Config page"))
                .clicked()
            {
                ctx.view.requested_page = Some(Page::Config);
            }
            if ui
                .button(pick(ctx.lang, "返回总览", "Back to Overview"))
                .clicked()
            {
                ctx.view.requested_page = Some(Page::Overview);
            }
        });
        return;
    }

    let runtime_maps = runtime_station_maps(ctx.proxy);
    let active_station = current_runtime_active_station(ctx.proxy);
    let configured_active_station = snapshot.configured_active_station.clone();
    let effective_active_station = snapshot
        .effective_active_station
        .clone()
        .or(active_station.clone());
    let supports_persisted_station_config = snapshot.supports_persisted_station_config;
    let mut stations = snapshot.stations.clone();
    stations.sort_by(|a, b| {
        a.level
            .clamp(1, 10)
            .cmp(&b.level.clamp(1, 10))
            .then_with(|| a.name.cmp(&b.name))
    });

    let search_query = ctx.view.stations.search.trim().to_ascii_lowercase();
    let enabled_only = ctx.view.stations.enabled_only;
    let overrides_only = ctx.view.stations.overrides_only;
    let filtered = stations
        .into_iter()
        .filter(|cfg| {
            if enabled_only && !cfg.enabled {
                return false;
            }
            if overrides_only
                && cfg.runtime_enabled_override.is_none()
                && cfg.runtime_level_override.is_none()
                && cfg.runtime_state_override.is_none()
            {
                return false;
            }
            if search_query.is_empty() {
                return true;
            }
            let alias = cfg.alias.as_deref().unwrap_or("");
            let capability = format_runtime_config_capability_label(ctx.lang, &cfg.capabilities);
            let haystack = format!(
                "{} {} {} {}",
                cfg.name.to_ascii_lowercase(),
                alias.to_ascii_lowercase(),
                format_runtime_station_health_status(
                    runtime_maps.station_health.get(cfg.name.as_str()),
                    runtime_maps.health_checks.get(cfg.name.as_str())
                )
                .to_ascii_lowercase(),
                capability.to_ascii_lowercase(),
            );
            haystack.contains(search_query.as_str())
        })
        .collect::<Vec<_>>();

    if ctx
        .view
        .stations
        .selected_name
        .as_ref()
        .is_none_or(|name| !filtered.iter().any(|cfg| cfg.name == *name))
    {
        ctx.view.stations.selected_name = filtered.first().map(|cfg| cfg.name.clone());
    }
    let mut selected_name = ctx.view.stations.selected_name.clone();

    ui.add_space(8.0);
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "模式", "Mode"),
                match snapshot.kind {
                    ProxyModeKind::Running => pick(ctx.lang, "本地运行", "Running"),
                    ProxyModeKind::Attached => pick(ctx.lang, "远端附着", "Attached"),
                    ProxyModeKind::Starting => pick(ctx.lang, "启动中", "Starting"),
                    ProxyModeKind::Stopped => pick(ctx.lang, "停止", "Stopped"),
                }
            ));
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "服务", "Service"),
                snapshot
                    .service_name
                    .as_deref()
                    .unwrap_or_else(|| pick(ctx.lang, "-", "-"))
            ));
            if let Some(base_url) = snapshot.base_url.as_deref() {
                ui.label(format!("base: {}", shorten_middle(base_url, 56)));
            }
        });
        ui.horizontal(|ui| {
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
                pick(ctx.lang, "配置 active_station", "Configured active_station"),
                configured_active_station
                    .as_deref()
                    .unwrap_or_else(|| pick(ctx.lang, "<无>", "<none>"))
            ));
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "生效站点", "Effective station"),
                effective_active_station
                    .as_deref()
                    .unwrap_or_else(|| pick(ctx.lang, "<未知/仅本机可见>", "<unknown/local-only>"))
            ));
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "配置 default_profile", "Configured default_profile"),
                snapshot
                    .configured_default_profile
                    .as_deref()
                    .or(snapshot.default_profile.as_deref())
                    .unwrap_or_else(|| pick(ctx.lang, "<无>", "<none>"))
            ));
            if snapshot
                .configured_default_profile
                .as_deref()
                != snapshot.default_profile.as_deref()
            {
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "生效 default_profile", "Effective default_profile"),
                    snapshot
                        .default_profile
                        .as_deref()
                        .unwrap_or_else(|| pick(ctx.lang, "<无>", "<none>"))
                ));
            }
        });
        ui.horizontal(|ui| {
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "持久化站点配置", "Persisted station config"),
                if supports_persisted_station_config {
                    pick(ctx.lang, "可用", "available")
                } else {
                    pick(ctx.lang, "不可用", "unavailable")
                }
            ));
            if matches!(snapshot.kind, ProxyModeKind::Attached) {
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "远端写回", "Remote write-back"),
                    if supports_persisted_station_config {
                        pick(ctx.lang, "已启用", "enabled")
                    } else {
                        pick(ctx.lang, "未提供", "not exposed")
                    }
                ));
            }
        });
        ui.horizontal(|ui| {
            if ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
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
                .button(pick(ctx.lang, "打开配置页", "Open Config page"))
                .clicked()
            {
                ctx.view.requested_page = Some(Page::Config);
            }
            if ui
                .button(pick(ctx.lang, "回到总览", "Back to Overview"))
                .clicked()
            {
                ctx.view.requested_page = Some(Page::Overview);
            }
        });
        ui.colored_label(
            egui::Color32::from_rgb(120, 120, 120),
            if matches!(snapshot.kind, ProxyModeKind::Attached) {
                if supports_persisted_station_config {
                    pick(
                        ctx.lang,
                        "附着模式下，global pin / runtime 覆盖会直接作用到远端代理；下面的“配置控制”也会直接写回远端代理配置，不会改动本机文件。",
                        "In attached mode, global pin and runtime overrides act on the remote proxy directly; the persisted config controls below also write back to the remote proxy rather than this device's local file.",
                    )
                } else {
                    pick(
                        ctx.lang,
                        "附着模式下，global pin / runtime 覆盖会直接作用到远端代理；当前附着目标还没有暴露 persisted station config API，因此只能做运行时控制。",
                        "In attached mode, global pin and runtime overrides act on the remote proxy directly; this attached target does not expose persisted station config APIs yet, so only runtime controls are available.",
                    )
                }
            } else {
                pick(
                    ctx.lang,
                    "这里的 global pin 是运行时覆盖；“配置控制”会通过本地 control-plane 写回配置文件并刷新运行态。",
                    "Global pin here is runtime-only; the persisted config controls write through the local control plane and refresh the runtime.",
                )
            },
        );
        if matches!(snapshot.kind, ProxyModeKind::Attached)
            && let Some(base_url) = snapshot.base_url.as_deref()
            && let Some(label) = remote_admin_access_short_label(
                base_url,
                &snapshot.remote_admin_access,
                ctx.lang,
            )
        {
            let color = if snapshot.remote_admin_access.remote_enabled
                && remote_admin_token_present()
            {
                egui::Color32::from_rgb(60, 160, 90)
            } else {
                egui::Color32::from_rgb(200, 120, 40)
            };
            let response = ui.colored_label(color, label);
            if let Some(message) =
                remote_admin_access_message(base_url, &snapshot.remote_admin_access, ctx.lang)
            {
                response.on_hover_text(message.clone());
                if !snapshot.remote_admin_access.remote_enabled || !remote_admin_token_present() {
                    ui.colored_label(egui::Color32::from_rgb(200, 120, 40), message);
                }
            }
        }
    });

    ui.add_space(8.0);
    render_retry_panel(ui, ctx, &snapshot);

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "搜索", "Search"));
        ui.add_sized(
            [320.0, 20.0],
            egui::TextEdit::singleline(&mut ctx.view.stations.search).hint_text(pick(
                ctx.lang,
                "按 station / alias / health / capability 过滤…",
                "Filter by station / alias / health / capability...",
            )),
        );
        ui.checkbox(
            &mut ctx.view.stations.enabled_only,
            pick(ctx.lang, "仅启用", "Enabled only"),
        );
        ui.checkbox(
            &mut ctx.view.stations.overrides_only,
            pick(ctx.lang, "仅运行时覆盖", "Overrides only"),
        );
        if ui.button(pick(ctx.lang, "清空", "Clear")).clicked() {
            ctx.view.stations.search.clear();
            ctx.view.stations.enabled_only = false;
            ctx.view.stations.overrides_only = false;
        }
    });

    ui.add_space(6.0);
    ui.columns(2, |cols| {
        cols[0].heading(pick(ctx.lang, "站点列表", "Stations"));
        cols[0].add_space(4.0);
        if filtered.is_empty() {
            cols[0].label(pick(
                ctx.lang,
                "筛选后没有匹配站点。",
                "No stations matched the current filters.",
            ));
        } else {
            egui::ScrollArea::vertical()
                .id_salt("stations_page_list_scroll")
                .max_height(560.0)
                .show(&mut cols[0], |ui| {
                    for cfg in filtered.iter() {
                        let is_selected = selected_name.as_deref() == Some(cfg.name.as_str());
                        let is_active = active_station.as_deref() == Some(cfg.name.as_str());
                        let is_pinned =
                            snapshot.global_station_override.as_deref() == Some(cfg.name.as_str());
                        let health_label = format_runtime_station_health_status(
                            runtime_maps.station_health.get(cfg.name.as_str()),
                            runtime_maps.health_checks.get(cfg.name.as_str()),
                        );
                        let breaker_label =
                            format_runtime_lb_summary(runtime_maps.lb_view.get(cfg.name.as_str()));

                        let mut label = format!("L{} {}", cfg.level.clamp(1, 10), cfg.name);
                        if let Some(alias) = cfg.alias.as_deref()
                            && !alias.trim().is_empty()
                        {
                            label.push_str(&format!(" ({alias})"));
                        }
                        if is_active {
                            label = format!("★ {label}");
                        } else if is_pinned {
                            label = format!("◆ {label}");
                        }
                        if !cfg.enabled {
                            label.push_str("  [off]");
                        }

                        let capability_hover =
                            runtime_config_capability_hover_text(ctx.lang, &cfg.capabilities);
                        let hover = format!(
                            "health: {health_label}\nbreaker: {breaker_label}\n{}\nsource: {}",
                            capability_hover,
                            format_runtime_station_source(ctx.lang, cfg)
                        );
                        if ui
                            .selectable_label(is_selected, label)
                            .on_hover_text(hover)
                            .clicked()
                        {
                            selected_name = Some(cfg.name.clone());
                        }
                        ui.small(format!(
                            "{}  |  {}",
                            health_label,
                            format_runtime_config_capability_label(ctx.lang, &cfg.capabilities)
                        ));
                        ui.add_space(4.0);
                    }
                });
        }

        cols[1].heading(pick(ctx.lang, "站点详情", "Station details"));
        cols[1].add_space(4.0);

        let Some(name) = selected_name.clone() else {
            cols[1].label(pick(ctx.lang, "未选择站点。", "No station selected."));
            return;
        };
        let Some(cfg) = filtered.iter().find(|cfg| cfg.name == name).cloned() else {
            cols[1].label(pick(
                ctx.lang,
                "当前选中站点不在筛选结果中。",
                "The selected station is not visible under the current filters.",
            ));
            return;
        };

        let health = runtime_maps.station_health.get(cfg.name.as_str());
        let health_status = runtime_maps.health_checks.get(cfg.name.as_str());
        let lb = runtime_maps.lb_view.get(cfg.name.as_str());
        let referencing_profiles = snapshot
            .profiles
            .iter()
            .filter(|profile| profile.station.as_deref() == Some(cfg.name.as_str()))
            .map(|profile| format_profile_display(profile.name.as_str(), Some(profile)))
            .collect::<Vec<_>>();

        cols[1].label(format!("name: {}", cfg.name));
        cols[1].label(format!(
            "alias: {}",
            cfg.alias
                .as_deref()
                .unwrap_or_else(|| pick(ctx.lang, "-", "-"))
        ));
        cols[1].label(format!(
            "{}: {}",
            pick(ctx.lang, "路由角色", "Routing role"),
            if effective_active_station.as_deref() == Some(cfg.name.as_str()) {
                pick(ctx.lang, "当前 active_station", "current active_station")
            } else if snapshot.global_station_override.as_deref() == Some(cfg.name.as_str()) {
                pick(ctx.lang, "当前 global pin", "current global pin")
            } else {
                pick(ctx.lang, "普通候选", "normal candidate")
            }
        ));
        if configured_active_station.as_deref() == Some(cfg.name.as_str())
            && effective_active_station.as_deref() != Some(cfg.name.as_str())
        {
            cols[1].small(pick(
                ctx.lang,
                "该站点是配置 active_station，但当前生效路由已被 fallback / pin / runtime 状态改变。",
                "This station is the configured active_station, but the effective route currently differs because of fallback, pin, or runtime state.",
            ));
        }
        cols[1].label(format!(
            "enabled: {}  (configured: {})",
            cfg.enabled, cfg.configured_enabled
        ));
        cols[1].label(format!(
            "level: L{}  (configured: L{})",
            cfg.level.clamp(1, 10),
            cfg.configured_level.clamp(1, 10)
        ));
        cols[1].label(format!(
            "state: {}",
            runtime_config_state_label(ctx.lang, cfg.runtime_state)
        ));
        cols[1].label(format!(
            "source: {}",
            format_runtime_station_source(ctx.lang, &cfg)
        ));
        cols[1].label(format!(
            "health: {}",
            format_runtime_station_health_status(health, health_status)
        ));
        cols[1].label(format!("breaker: {}", format_runtime_lb_summary(lb)));
        cols[1].label(format!(
            "{}: {}",
            pick(ctx.lang, "Profiles", "Profiles"),
            if referencing_profiles.is_empty() {
                pick(ctx.lang, "<无>", "<none>").to_string()
            } else {
                referencing_profiles.join(", ")
            }
        ));
        cols[1]
            .small(format_runtime_config_capability_label(
                ctx.lang,
                &cfg.capabilities,
            ))
            .on_hover_text(runtime_config_capability_hover_text(
                ctx.lang,
                &cfg.capabilities,
            ));

        if cfg.capabilities.model_catalog_kind == ModelCatalogKind::Declared
            && !cfg.capabilities.supported_models.is_empty()
        {
            let preview = cfg
                .capabilities
                .supported_models
                .iter()
                .take(12)
                .cloned()
                .collect::<Vec<_>>();
            let suffix = if cfg.capabilities.supported_models.len() > preview.len() {
                format!(
                    " … +{}",
                    cfg.capabilities.supported_models.len() - preview.len()
                )
            } else {
                String::new()
            };
            cols[1].small(format!("models: {}{suffix}", preview.join(", ")));
        }

        cols[1].add_space(8.0);
        cols[1].separator();
        cols[1].label(pick(
            ctx.lang,
            "Quick switch（运行时）",
            "Quick switch (runtime)",
        ));
        cols[1].separator();
        cols[1].horizontal(|ui| {
            if ui
                .add_enabled(
                    snapshot.supports_v1,
                    egui::Button::new(pick(ctx.lang, "Pin 当前站点", "Pin selected station")),
                )
                .clicked()
            {
                match ctx
                    .proxy
                    .apply_global_station_override(ctx.rt, Some(cfg.name.clone()))
                {
                    Ok(()) => {
                        ctx.proxy
                            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                        *ctx.last_info = Some(
                            pick(ctx.lang, "已应用全局站点覆盖", "Global station pin applied")
                                .to_string(),
                        );
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("apply global override failed: {e}"));
                    }
                }
            }
            if ui
                .add_enabled(
                    snapshot.supports_v1 && snapshot.global_station_override.is_some(),
                    egui::Button::new(pick(ctx.lang, "清除 global pin", "Clear global pin")),
                )
                .clicked()
            {
                match ctx.proxy.apply_global_station_override(ctx.rt, None) {
                    Ok(()) => {
                        ctx.proxy
                            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                        *ctx.last_info = Some(
                            pick(ctx.lang, "已清除全局覆盖", "Global pin cleared").to_string(),
                        );
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("clear global override failed: {e}"));
                    }
                }
            }
        });
        cols[1].small(pick(
            ctx.lang,
            "这里的 pin 只影响当前代理运行态，不修改配置文件。",
            "Pins here only affect the current proxy runtime and do not rewrite persisted config.",
        ));

        cols[1].add_space(8.0);
        cols[1].separator();
        cols[1].label(pick(ctx.lang, "配置控制", "Persisted config"));
        if supports_persisted_station_config {
            cols[1].horizontal(|ui| {
                if ui
                    .button(pick(
                        ctx.lang,
                        "设为配置 active_station",
                        "Set configured active_station",
                    ))
                    .clicked()
                {
                    match ctx
                        .proxy
                        .set_persisted_active_station(ctx.rt, Some(cfg.name.clone()))
                    {
                        Ok(()) => {
                            ctx.proxy
                                .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                            refresh_config_editor_from_disk_if_running(ctx);
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已更新配置 active_station",
                                    "Configured active_station updated",
                                )
                                .to_string(),
                            );
                            *ctx.last_error = None;
                        }
                        Err(e) => {
                            *ctx.last_error =
                                Some(format!("set persisted active station failed: {e}"));
                        }
                    }
                }
                if ui
                    .add_enabled(
                        configured_active_station.is_some(),
                        egui::Button::new(pick(
                            ctx.lang,
                            "清除配置 active_station",
                            "Clear configured active_station",
                        )),
                    )
                    .clicked()
                {
                    match ctx.proxy.set_persisted_active_station(ctx.rt, None) {
                        Ok(()) => {
                            ctx.proxy
                                .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                            refresh_config_editor_from_disk_if_running(ctx);
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已清除配置 active_station",
                                    "Configured active_station cleared",
                                )
                                .to_string(),
                            );
                            *ctx.last_error = None;
                        }
                        Err(e) => {
                            *ctx.last_error =
                                Some(format!("clear persisted active station failed: {e}"));
                        }
                    }
                }
            });

            let mut persisted_enabled = cfg.configured_enabled;
            let mut persisted_level = cfg.configured_level.clamp(1, 10);
            cols[1].horizontal(|ui| {
                ui.checkbox(
                    &mut persisted_enabled,
                    pick(ctx.lang, "配置启用", "Configured enabled"),
                );
                ui.label(pick(ctx.lang, "配置等级", "Configured level"));
                egui::ComboBox::from_id_salt(("stations_persisted_level", cfg.name.as_str()))
                    .selected_text(persisted_level.to_string())
                    .show_ui(ui, |ui| {
                        for candidate in 1u8..=10 {
                            ui.selectable_value(
                                &mut persisted_level,
                                candidate,
                                candidate.to_string(),
                            );
                        }
                    });
            });
            if persisted_enabled != cfg.configured_enabled
                || persisted_level != cfg.configured_level.clamp(1, 10)
            {
                match ctx.proxy.update_persisted_station(
                    ctx.rt,
                    cfg.name.clone(),
                    persisted_enabled,
                    persisted_level,
                ) {
                    Ok(()) => {
                        ctx.proxy
                            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                        refresh_config_editor_from_disk_if_running(ctx);
                        *ctx.last_info = Some(
                            pick(
                                ctx.lang,
                                "已写回站点配置字段",
                                "Persisted station fields updated",
                            )
                            .to_string(),
                        );
                        *ctx.last_error = None;
                    }
                    Err(e) => {
                        *ctx.last_error =
                            Some(format!("update persisted station fields failed: {e}"));
                    }
                }
            }
            cols[1].small(if matches!(snapshot.kind, ProxyModeKind::Attached) {
                pick(
                    ctx.lang,
                    "这里直接写回附着代理的配置，不依赖本机文件。",
                    "These controls write back to the attached proxy's config directly and do not rely on this device's local file.",
                )
            } else {
                pick(
                    ctx.lang,
                    "这里通过本地 control-plane 写回配置文件，并与运行态保持同步。",
                    "These controls write back through the local control plane and keep runtime in sync.",
                )
            });
        } else {
            cols[1].colored_label(
                egui::Color32::from_rgb(120, 120, 120),
                pick(
                    ctx.lang,
                    "当前目标没有暴露 persisted station config API，因此这里只能查看配置态，不能直接修改。",
                    "This target does not expose persisted station config APIs yet, so persisted fields are view-only here.",
                ),
            );
        }

        cols[1].add_space(8.0);
        cols[1].separator();
        cols[1].label(pick(ctx.lang, "运行时控制", "Runtime control"));
        if snapshot.supports_station_runtime_override {
            let mut runtime_state = cfg.runtime_state;
            cols[1].horizontal(|ui| {
                ui.label(pick(ctx.lang, "状态", "State"));
                egui::ComboBox::from_id_salt(("stations_runtime_state", cfg.name.as_str()))
                    .selected_text(runtime_config_state_label(ctx.lang, runtime_state))
                    .show_ui(ui, |ui| {
                        for candidate in [
                            RuntimeConfigState::Normal,
                            RuntimeConfigState::Draining,
                            RuntimeConfigState::BreakerOpen,
                            RuntimeConfigState::HalfOpen,
                        ] {
                            ui.selectable_value(
                                &mut runtime_state,
                                candidate,
                                runtime_config_state_label(ctx.lang, candidate),
                            );
                        }
                    });
                if runtime_state != cfg.runtime_state {
                    match ctx.proxy.set_runtime_station_meta(
                        ctx.rt,
                        cfg.name.clone(),
                        None,
                        None,
                        Some(Some(runtime_state)),
                    ) {
                        Ok(()) => {
                            ctx.proxy
                                .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已应用站点运行时状态",
                                    "Runtime station state updated",
                                )
                                .to_string(),
                            );
                        }
                        Err(e) => {
                            *ctx.last_error = Some(format!("apply runtime state failed: {e}"));
                        }
                    }
                }
            });

            cols[1].horizontal(|ui| {
                let mut enabled = cfg.enabled;
                if ui
                    .checkbox(&mut enabled, pick(ctx.lang, "启用", "Enabled"))
                    .changed()
                {
                    match ctx.proxy.set_runtime_station_meta(
                        ctx.rt,
                        cfg.name.clone(),
                        Some(Some(enabled)),
                        None,
                        None,
                    ) {
                        Ok(()) => {
                            ctx.proxy
                                .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已应用站点运行时开关",
                                    "Runtime station enabled updated",
                                )
                                .to_string(),
                            );
                        }
                        Err(e) => {
                            *ctx.last_error = Some(format!("apply runtime enabled failed: {e}"));
                        }
                    }
                }

                let mut level = cfg.level.clamp(1, 10);
                ui.label(pick(ctx.lang, "等级", "Level"));
                egui::ComboBox::from_id_salt(("stations_runtime_level", cfg.name.as_str()))
                    .selected_text(level.to_string())
                    .show_ui(ui, |ui| {
                        for candidate in 1u8..=10 {
                            ui.selectable_value(&mut level, candidate, candidate.to_string());
                        }
                    });
                if level != cfg.level {
                    match ctx.proxy.set_runtime_station_meta(
                        ctx.rt,
                        cfg.name.clone(),
                        None,
                        Some(Some(level)),
                        None,
                    ) {
                        Ok(()) => {
                            ctx.proxy
                                .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已应用站点运行时等级",
                                    "Runtime station level updated",
                                )
                                .to_string(),
                            );
                        }
                        Err(e) => {
                            *ctx.last_error = Some(format!("apply runtime level failed: {e}"));
                        }
                    }
                }
            });

            let has_override = cfg.runtime_enabled_override.is_some()
                || cfg.runtime_level_override.is_some()
                || cfg.runtime_state_override.is_some();
            if cols[1]
                .add_enabled(
                    has_override,
                    egui::Button::new(pick(ctx.lang, "清除运行时覆盖", "Clear runtime override")),
                )
                .clicked()
            {
                match ctx.proxy.set_runtime_station_meta(
                    ctx.rt,
                    cfg.name.clone(),
                    cfg.runtime_enabled_override.map(|_| None),
                    cfg.runtime_level_override.map(|_| None),
                    cfg.runtime_state_override.map(|_| None),
                ) {
                    Ok(()) => {
                        ctx.proxy
                            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                        *ctx.last_info = Some(
                            pick(
                                ctx.lang,
                                "已清除站点运行时覆盖",
                                "Runtime station override cleared",
                            )
                            .to_string(),
                        );
                    }
                    Err(e) => {
                        *ctx.last_error =
                            Some(format!("clear runtime station override failed: {e}"));
                    }
                }
            }
        } else {
            cols[1].colored_label(
                egui::Color32::from_rgb(120, 120, 120),
                pick(
                    ctx.lang,
                    "当前代理不支持运行时站点控制；此区域只读。",
                    "This proxy does not support runtime station control; this area is read-only.",
                ),
            );
        }

        cols[1].add_space(8.0);
        cols[1].separator();
        cols[1].label(pick(ctx.lang, "健康检查", "Health check"));
        if let Some(status) = health_status {
            cols[1].label(format!(
                "status: {}/{} ok={} err={} cancel={} done={}",
                status.completed,
                status.total,
                status.ok,
                status.err,
                status.cancel_requested,
                status.done
            ));
            if let Some(err) = status.last_error.as_deref() {
                cols[1].colored_label(egui::Color32::from_rgb(200, 120, 40), err);
            }
        } else {
            cols[1].label(pick(ctx.lang, "(无状态)", "(no status)"));
        }
        cols[1].horizontal(|ui| {
            if ui
                .button(pick(ctx.lang, "探测当前", "Probe selected"))
                .clicked()
            {
                match ctx.proxy.probe_station(ctx.rt, cfg.name.clone()) {
                    Ok(()) => {
                        ctx.proxy
                            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已开始探测", "Probe started").to_string());
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("station probe failed: {e}"));
                    }
                }
            }
            if ui
                .button(pick(ctx.lang, "取消当前", "Cancel selected"))
                .clicked()
            {
                match ctx
                    .proxy
                    .cancel_health_checks(ctx.rt, false, vec![cfg.name.clone()])
                {
                    Ok(()) => {
                        ctx.proxy
                            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已请求取消", "Cancel requested").to_string());
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("health check cancel failed: {e}"));
                    }
                }
            }
            if ui.button(pick(ctx.lang, "检查全部", "Check all")).clicked() {
                match ctx.proxy.start_health_checks(ctx.rt, true, Vec::new()) {
                    Ok(()) => {
                        ctx.proxy
                            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                        *ctx.last_info = Some(
                            pick(ctx.lang, "已开始健康检查", "Health check started").to_string(),
                        );
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("health check start failed: {e}"));
                    }
                }
            }
            if ui
                .button(pick(ctx.lang, "取消全部", "Cancel all"))
                .clicked()
            {
                match ctx.proxy.cancel_health_checks(ctx.rt, true, Vec::new()) {
                    Ok(()) => {
                        ctx.proxy
                            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已请求取消", "Cancel requested").to_string());
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("health check cancel failed: {e}"));
                    }
                }
            }
        });

        if let Some(health) = health {
            cols[1].add_space(6.0);
            cols[1].label(format!(
                "{}: {}  upstreams={}",
                pick(ctx.lang, "最近检查", "Last checked"),
                health.checked_at_ms,
                health.upstreams.len()
            ));
            egui::ScrollArea::vertical()
                .id_salt(("stations_health_upstreams_scroll", cfg.name.as_str()))
                .max_height(140.0)
                .show(&mut cols[1], |ui| {
                    let max = 12usize;
                    for up in health.upstreams.iter().rev().take(max) {
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
                            shorten_middle(&up.base_url, 52)
                        ));
                    }
                    if health.upstreams.len() > max {
                        ui.label(format!("… +{} more", health.upstreams.len() - max));
                    }
                });
        }

        cols[1].add_space(8.0);
        cols[1].separator();
        cols[1].label(pick(ctx.lang, "熔断/冷却细节", "Breaker/cooldown details"));
        if let Some(lb) = lb {
            if lb.upstreams.is_empty() {
                cols[1].label(pick(ctx.lang, "(无上游状态)", "(no upstream state)"));
            } else {
                egui::ScrollArea::vertical()
                    .id_salt(("stations_lb_scroll", cfg.name.as_str()))
                    .max_height(120.0)
                    .show(&mut cols[1], |ui| {
                        for (idx, upstream) in lb.upstreams.iter().enumerate() {
                            let cooldown = upstream
                                .cooldown_remaining_secs
                                .map(|secs| format!("{secs}s"))
                                .unwrap_or_else(|| "-".to_string());
                            ui.label(format!(
                                "#{} fail={} cooldown={} quota_exhausted={}",
                                idx, upstream.failure_count, cooldown, upstream.usage_exhausted
                            ));
                        }
                        if let Some(last_good_index) = lb.last_good_index {
                            ui.small(format!("last_good_index={last_good_index}"));
                        }
                    });
            }
        } else {
            cols[1].label(pick(ctx.lang, "(无熔断数据)", "(no breaker data)"));
        }
    });

    ctx.view.stations.selected_name = selected_name;
}


pub(super) fn render_profile_management_entrypoint(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.label(pick(ctx.lang, "控制 profiles", "Control profiles"));
    ui.colored_label(
        egui::Color32::from_rgb(120, 120, 120),
        pick(
            ctx.lang,
            "旧版 GUI routing preset 已停用。现在统一使用代理配置里的 [codex.profiles.*]；默认 profile 在“配置”页管理，单会话覆盖在“会话”页管理。",
            "Legacy GUI routing presets are retired. Use [codex.profiles.*] in proxy config instead; manage default profiles in Config and per-session overrides in Sessions.",
        ),
    );
    ui.horizontal(|ui| {
        if ui
            .button(pick(ctx.lang, "前往配置页", "Open Config page"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Config);
        }
        if ui
            .button(pick(ctx.lang, "前往会话页", "Open Sessions page"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Sessions);
        }
    });
}

pub(super) fn render_retry_panel(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
) {
    if snapshot.supports_retry_config_api {
        let configured_retry = snapshot.configured_retry.clone().unwrap_or_default();
        sync_stations_retry_editor(&mut ctx.view.stations.retry_editor, &configured_retry);
    }

    ui.group(|ui| {
        ui.heading(pick(ctx.lang, "Retry / Failover", "Retry / Failover"));
        ui.label(pick(
            ctx.lang,
            "这里管理全局的 retry profile 与冷却/熔断惩罚；它影响整个代理的路由行为，不是单个 station 的局部设置。",
            "Manage the global retry profile plus cooldown/breaker penalties here; it affects whole-proxy routing behavior rather than a single station.",
        ));

        if snapshot.supports_retry_config_api {
            {
                let editor = &mut ctx.view.stations.retry_editor;
                ui.horizontal(|ui| {
                    ui.label(pick(ctx.lang, "Retry profile", "Retry profile"));
                    egui::ComboBox::from_id_salt("stations_retry_profile")
                        .selected_text(retry_profile_display_text(
                            ctx.lang,
                            retry_profile_name_from_value(editor.profile.as_str()),
                        ))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut editor.profile,
                                String::new(),
                                retry_profile_display_text(ctx.lang, None),
                            );
                            for profile in [
                                RetryProfileName::Balanced,
                                RetryProfileName::SameUpstream,
                                RetryProfileName::AggressiveFailover,
                                RetryProfileName::CostPrimary,
                            ] {
                                ui.selectable_value(
                                    &mut editor.profile,
                                    retry_profile_name_value(profile).to_string(),
                                    retry_profile_display_text(ctx.lang, Some(profile)),
                                );
                            }
                        });
                });

                ui.horizontal(|ui| {
                    ui.label("cf challenge");
                    ui.add_sized(
                        [72.0, 22.0],
                        egui::TextEdit::singleline(&mut editor.cloudflare_challenge_cooldown_secs),
                    );
                    ui.label("cf timeout");
                    ui.add_sized(
                        [72.0, 22.0],
                        egui::TextEdit::singleline(&mut editor.cloudflare_timeout_cooldown_secs),
                    );
                    ui.label("transport");
                    ui.add_sized(
                        [72.0, 22.0],
                        egui::TextEdit::singleline(&mut editor.transport_cooldown_secs),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("backoff factor");
                    ui.add_sized(
                        [72.0, 22.0],
                        egui::TextEdit::singleline(&mut editor.cooldown_backoff_factor),
                    );
                    ui.label("backoff max");
                    ui.add_sized(
                        [72.0, 22.0],
                        egui::TextEdit::singleline(&mut editor.cooldown_backoff_max_secs),
                    );
                });
            }

            ui.horizontal(|ui| {
                if ui
                    .button(pick(ctx.lang, "写回 retry 配置", "Apply persisted retry config"))
                    .clicked()
                {
                    let base_retry = snapshot.configured_retry.as_ref().cloned().unwrap_or_default();
                    match build_retry_config_from_editor(&ctx.view.stations.retry_editor, &base_retry)
                    {
                        Ok(retry) => match ctx.proxy.set_persisted_retry_config(ctx.rt, retry) {
                            Ok(()) => {
                                ctx.proxy.refresh_current_if_due(
                                    ctx.rt,
                                    std::time::Duration::from_secs(0),
                                );
                                refresh_config_editor_from_disk_if_running(ctx);
                                *ctx.last_info = Some(
                                    pick(
                                        ctx.lang,
                                        "已写回 retry/failover 配置",
                                        "Persisted retry/failover config updated",
                                    )
                                    .to_string(),
                                );
                                *ctx.last_error = None;
                            }
                            Err(e) => {
                                *ctx.last_error =
                                    Some(format!("set persisted retry config failed: {e}"));
                            }
                        },
                        Err(e) => {
                            *ctx.last_error = Some(format!("invalid retry config: {e}"));
                        }
                    }
                }

                if ui
                    .button(pick(ctx.lang, "恢复 balanced 表单", "Reset form to balanced"))
                    .clicked()
                {
                    load_stations_retry_editor_fields(
                        &mut ctx.view.stations.retry_editor,
                        &RetryConfig::default(),
                    );
                }
            });

            ui.small(if matches!(snapshot.kind, ProxyModeKind::Attached) {
                pick(
                    ctx.lang,
                    "附着模式下，这里直接写回远端代理暴露的 retry config API，不依赖本机文件。",
                    "In attached mode, this writes directly to the remote proxy's retry config API instead of any local file on this device.",
                )
            } else {
                pick(
                    ctx.lang,
                    "本地运行模式下，这里通过 control-plane 写回配置文件并触发 reload。",
                    "In local running mode, this writes through the control plane to persisted config and reloads the runtime.",
                )
            });
        } else {
            ui.colored_label(
                egui::Color32::from_rgb(120, 120, 120),
                if matches!(snapshot.kind, ProxyModeKind::Attached) {
                    pick(
                        ctx.lang,
                        "当前附着目标没有暴露 retry config API，因此这里只能查看 resolved policy，不能直接写回。",
                        "This attached target does not expose retry config APIs, so only the resolved policy is visible here.",
                    )
                } else {
                    pick(
                        ctx.lang,
                        "当前运行态没有可写 retry config API；下面仅展示 resolved policy。",
                        "No writable retry config API is available for the current runtime; only the resolved policy is shown below.",
                    )
                },
            );
        }

        ui.add_space(6.0);
        ui.separator();
        ui.label(pick(ctx.lang, "Resolved policy", "Resolved policy"));
        if let Some(retry) = snapshot.resolved_retry.as_ref() {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "upstream: {} / attempts={}",
                    retry_strategy_label(retry.upstream.strategy),
                    retry.upstream.max_attempts
                ));
                ui.label(format!(
                    "provider: {} / attempts={}",
                    retry_strategy_label(retry.provider.strategy),
                    retry.provider.max_attempts
                ));
            });
            ui.horizontal(|ui| {
                ui.label(format!(
                    "cf challenge={}s",
                    retry.cloudflare_challenge_cooldown_secs
                ));
                ui.label(format!(
                    "cf timeout={}s",
                    retry.cloudflare_timeout_cooldown_secs
                ));
                ui.label(format!("transport={}s", retry.transport_cooldown_secs));
            });
            ui.horizontal(|ui| {
                ui.label(format!(
                    "backoff factor={}",
                    retry.cooldown_backoff_factor
                ));
                ui.label(format!(
                    "backoff max={}s",
                    retry.cooldown_backoff_max_secs
                ));
            });
            ui.small(format!(
                "upstream backoff={}..{} ms  provider backoff={}..{} ms",
                retry.upstream.backoff_ms,
                retry.upstream.backoff_max_ms,
                retry.provider.backoff_ms,
                retry.provider.backoff_max_ms
            ));
            ui.small(pick(
                ctx.lang,
                "同站点 failover 规则：优先在当前 station 内尝试其他 eligible upstream，只有当前 station 耗尽后才会考虑下一个 station。",
                "Same-station failover rule: exhaust other eligible upstreams inside the current station before considering the next station.",
            ));
        } else {
            ui.label(pick(
                ctx.lang,
                "当前还没有可见的 resolved retry policy。",
                "No resolved retry policy is visible for the current runtime yet.",
            ));
        }
    });
}

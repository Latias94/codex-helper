use super::config_v2::context::ConfigV2RenderContext;
use super::view_state::ConfigV2Section;
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlSurfaceMode {
    DirectLocal,
    DirectRemote,
    LocalDraft,
    Unavailable,
}

pub(super) fn render_config_v2_workspace_header(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    render_ctx: &ConfigV2RenderContext,
) {
    let lang = ctx.lang;
    let proxy_kind = ctx.proxy.kind();
    let current_section = ctx.view.config.v2_section;
    let station_count = render_ctx.station_display_names.len();
    let provider_count = render_ctx.provider_display_names.len();
    let profile_count = if render_ctx.profile_control_plane_enabled {
        render_ctx.profile_control_plane_catalog.len()
    } else {
        render_ctx.profile_catalog.len()
    };
    let mode_label = match proxy_kind {
        ProxyModeKind::Attached => pick(lang, "附着代理", "Attached proxy"),
        ProxyModeKind::Running => pick(lang, "本机运行", "Local runtime"),
        ProxyModeKind::Starting => pick(lang, "启动中", "Starting"),
        ProxyModeKind::Stopped => pick(lang, "本地文件", "Local file"),
    };
    let focus_hint = match current_section {
        ConfigV2Section::Stations => pick(
            lang,
            "适合调整默认路由、成员组合、健康检查与 active station。",
            "Best for routing topology, health checks, and active station control.",
        ),
        ConfigV2Section::Providers => pick(
            lang,
            "适合管理中转来源、认证引用、endpoint 集合和后续故障切换基础。",
            "Best for relay sources, auth references, endpoint sets, and future failover basics.",
        ),
        ConfigV2Section::Profiles => pick(
            lang,
            "最适合日常切换 fast mode、模型、reasoning_effort 与 service_tier。",
            "Best for daily switching of fast mode, model, reasoning_effort, and service_tier.",
        ),
    };
    let scope_label = control_scope_label(lang, proxy_kind, render_ctx);
    let scope_hint = control_scope_hint(lang, proxy_kind, render_ctx);
    let station_value = render_ctx
        .effective_active_name
        .as_deref()
        .or(render_ctx.configured_active_name.as_deref())
        .unwrap_or_else(|| pick(lang, "<自动>", "<auto>"));
    let station_hint = station_summary_hint(lang, render_ctx, station_count);
    let profile_value = render_ctx
        .station_default_profile
        .as_deref()
        .unwrap_or_else(|| pick(lang, "<无默认>", "<no default>"));
    let profile_hint = profile_summary_hint(lang, proxy_kind, render_ctx, profile_count);
    let provider_hint = provider_summary_hint(lang, proxy_kind, render_ctx, provider_count);

    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading(pick(lang, "Control Deck", "Control Deck"));
                ui.small(focus_hint);
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(mode_label)
                        .strong()
                        .color(egui::Color32::from_rgb(76, 114, 176)),
                );
            });
        });

        ui.add_space(6.0);
        ui.columns(4, |cols| {
            render_config_v2_summary_card(
                &mut cols[0],
                pick(lang, "Scope", "Scope"),
                scope_label,
                &scope_hint,
            );
            render_config_v2_summary_card(
                &mut cols[1],
                pick(lang, "Active station", "Active station"),
                station_value.to_string(),
                &station_hint,
            );
            render_config_v2_summary_card(
                &mut cols[2],
                pick(lang, "Default profile", "Default profile"),
                profile_value.to_string(),
                &profile_hint,
            );
            render_config_v2_summary_card(
                &mut cols[3],
                pick(lang, "Providers", "Providers"),
                provider_count.to_string(),
                &provider_hint,
            );
        });

        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            render_surface_chip(
                ui,
                lang,
                pick(lang, "站点运行态", "Station runtime"),
                control_surface_mode(
                    proxy_kind,
                    render_ctx.station_control_plane_enabled,
                    !render_ctx.attached_mode,
                ),
            );
            render_surface_chip(
                ui,
                lang,
                pick(lang, "站点结构", "Station registry"),
                control_surface_mode(
                    proxy_kind,
                    render_ctx.station_structure_control_plane_enabled,
                    render_ctx.station_structure_edit_enabled,
                ),
            );
            render_surface_chip(
                ui,
                lang,
                pick(lang, "Provider 目录", "Provider registry"),
                control_surface_mode(
                    proxy_kind,
                    render_ctx.provider_structure_control_plane_enabled,
                    render_ctx.provider_structure_edit_enabled,
                ),
            );
            render_surface_chip(
                ui,
                lang,
                pick(lang, "Profile 目录", "Profile registry"),
                control_surface_mode(
                    proxy_kind,
                    render_ctx.profile_control_plane_enabled,
                    !render_ctx.attached_mode,
                ),
            );
        });

        ui.add_space(8.0);
        render_control_deck_actions(ui, ctx, proxy_kind, render_ctx);

        ui.add_space(8.0);
        render_control_deck_focus_targets(ui, ctx, render_ctx);

        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            let section = &mut ctx.view.config.v2_section;
            ui.selectable_value(
                section,
                ConfigV2Section::Profiles,
                format!("{} · {}", pick(lang, "Profiles", "Profiles"), profile_count),
            );
            ui.selectable_value(
                section,
                ConfigV2Section::Stations,
                format!("{} · {}", pick(lang, "Stations", "Stations"), station_count),
            );
            ui.selectable_value(
                section,
                ConfigV2Section::Providers,
                format!(
                    "{} · {}",
                    pick(lang, "Providers", "Providers"),
                    provider_count
                ),
            );
        });
    });
}

fn render_config_v2_summary_card(ui: &mut egui::Ui, title: &str, value: String, hint: &str) {
    ui.group(|ui| {
        ui.small(title);
        ui.heading(value);
        ui.small(hint);
    });
}

fn render_control_deck_actions(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    proxy_kind: ProxyModeKind,
    render_ctx: &ConfigV2RenderContext,
) {
    let can_reload_runtime = matches!(proxy_kind, ProxyModeKind::Running | ProxyModeKind::Attached);
    let can_refresh_runtime = can_reload_runtime;
    let runtime_target = match proxy_kind {
        ProxyModeKind::Attached => pick(
            ctx.lang,
            "当前动作会直接作用于附着代理。",
            "Actions target the attached proxy directly.",
        ),
        ProxyModeKind::Running => pick(
            ctx.lang,
            "当前动作会直接作用于本机运行中的代理。",
            "Actions target the locally running proxy directly.",
        ),
        ProxyModeKind::Starting => pick(
            ctx.lang,
            "代理正在启动，暂时只能切换工作台或编辑本地文稿。",
            "The proxy is starting; use deck navigation or edit the local draft for now.",
        ),
        ProxyModeKind::Stopped => pick(
            ctx.lang,
            "当前没有活动代理，页头动作会导航或编辑本地文稿，不会刷新运行态。",
            "There is no active proxy; deck actions navigate or edit the local draft only.",
        ),
    };

    ui.group(|ui| {
        ui.horizontal_wrapped(|ui| {
            ui.small(format!(
                "{}: {}",
                pick(ctx.lang, "快捷入口", "Quick jump"),
                runtime_target
            ));
        });

        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button(pick(ctx.lang, "总览", "Overview")).clicked() {
                ctx.view.requested_page = Some(Page::Overview);
            }
            if ui.button(pick(ctx.lang, "站点台", "Stations")).clicked() {
                ctx.view.requested_page = Some(Page::Stations);
            }
            if ui.button(pick(ctx.lang, "会话台", "Sessions")).clicked() {
                ctx.view.requested_page = Some(Page::Sessions);
            }
            if ui.button(pick(ctx.lang, "请求台", "Requests")).clicked() {
                ctx.view.requested_page = Some(Page::Requests);
            }

            ui.separator();

            if ui
                .add_enabled(
                    can_refresh_runtime,
                    egui::Button::new(pick(ctx.lang, "刷新运行态", "Refresh runtime")),
                )
                .clicked()
            {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                *ctx.last_info = Some(
                    pick(
                        ctx.lang,
                        "已请求刷新当前运行态",
                        "Requested runtime refresh",
                    )
                    .to_string(),
                );
            }

            if ui
                .add_enabled(
                    can_reload_runtime,
                    egui::Button::new(pick(ctx.lang, "重载代理", "Reload proxy")),
                )
                .clicked()
            {
                if let Err(error) = ctx.proxy.reload_runtime_config(ctx.rt) {
                    *ctx.last_error = Some(format!("reload runtime failed: {error}"));
                } else {
                    *ctx.last_info = Some(
                        pick(
                            ctx.lang,
                            "已重载当前代理运行态",
                            "Reloaded current proxy runtime",
                        )
                        .to_string(),
                    );
                }
            }

            if !render_ctx.profile_control_plane_enabled
                && !render_ctx.station_control_plane_enabled
                && ui
                    .button(pick(ctx.lang, "回到设置", "Open Setup"))
                    .clicked()
            {
                ctx.view.requested_page = Some(Page::Setup);
            }
        });
    });
}

fn render_control_deck_focus_targets(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    render_ctx: &ConfigV2RenderContext,
) {
    let active_station_name = render_ctx
        .effective_active_name
        .clone()
        .or_else(|| render_ctx.configured_active_name.clone());
    let default_profile_name = render_ctx.station_default_profile.clone();
    let focus_provider_name = focus_provider_name(render_ctx);
    let resolved_default_profile = default_profile_name
        .as_deref()
        .and_then(|name| resolve_profile_for_deck(render_ctx, name));

    ui.group(|ui| {
        ui.small(pick(
            ctx.lang,
            "聚焦当前控制目标",
            "Focus current control target",
        ));
        ui.horizontal_wrapped(|ui| {
            if let Some(station_name) = active_station_name.as_deref()
                && ui
                    .button(format!(
                        "{}: {}",
                        pick(ctx.lang, "聚焦站点", "Focus station"),
                        station_name
                    ))
                    .clicked()
            {
                ctx.view.config.v2_section = ConfigV2Section::Stations;
                ctx.view.config.selected_name = Some(station_name.to_string());
            }

            if let Some(profile_name) = default_profile_name.as_deref()
                && ui
                    .button(format!(
                        "{}: {}",
                        pick(ctx.lang, "聚焦默认 profile", "Focus default profile"),
                        profile_name
                    ))
                    .clicked()
            {
                ctx.view.config.v2_section = ConfigV2Section::Profiles;
                ctx.view.config.selected_profile_name = Some(profile_name.to_string());
            }

            if let Some(provider_name) = focus_provider_name.as_deref()
                && ui
                    .button(format!(
                        "{}: {}",
                        pick(ctx.lang, "聚焦 provider", "Focus provider"),
                        provider_name
                    ))
                    .clicked()
            {
                ctx.view.config.v2_section = ConfigV2Section::Providers;
                ctx.view.config.selected_provider_name = Some(provider_name.to_string());
            }
        });

        if let Some((profile_name, profile)) = resolved_default_profile {
            ui.add_space(6.0);
            ui.small(format!(
                "{}: {}",
                pick(
                    ctx.lang,
                    "当前默认 profile 摘要",
                    "Current default profile summary"
                ),
                profile_name
            ));
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "station={}",
                    session_profile_target_value(profile.station.as_deref(), ctx.lang)
                ));
                ui.label(format!(
                    "model={}",
                    session_profile_target_value(profile.model.as_deref(), ctx.lang)
                ));
                ui.label(format!(
                    "reasoning={}",
                    session_profile_target_value(profile.reasoning_effort.as_deref(), ctx.lang)
                ));
                ui.label(format!(
                    "service_tier={}",
                    format_service_tier_display(profile.service_tier.as_deref(), ctx.lang, "auto")
                ));
            });
        }
    });
}

fn control_surface_mode(
    proxy_kind: ProxyModeKind,
    direct_available: bool,
    local_edit_available: bool,
) -> ControlSurfaceMode {
    if direct_available {
        match proxy_kind {
            ProxyModeKind::Attached => ControlSurfaceMode::DirectRemote,
            ProxyModeKind::Running | ProxyModeKind::Starting | ProxyModeKind::Stopped => {
                ControlSurfaceMode::DirectLocal
            }
        }
    } else if local_edit_available {
        ControlSurfaceMode::LocalDraft
    } else {
        ControlSurfaceMode::Unavailable
    }
}

fn control_scope_label(
    lang: Language,
    proxy_kind: ProxyModeKind,
    render_ctx: &ConfigV2RenderContext,
) -> String {
    match control_surface_mode(
        proxy_kind,
        render_ctx.profile_control_plane_enabled || render_ctx.station_control_plane_enabled,
        !render_ctx.attached_mode,
    ) {
        ControlSurfaceMode::DirectRemote => pick(lang, "直写远端", "Remote control-plane").into(),
        ControlSurfaceMode::DirectLocal => pick(lang, "直写本机", "Local control-plane").into(),
        ControlSurfaceMode::LocalDraft => pick(lang, "本地文稿", "Local config draft").into(),
        ControlSurfaceMode::Unavailable => pick(lang, "远端受限", "Remote-limited").into(),
    }
}

fn control_scope_hint(
    lang: Language,
    proxy_kind: ProxyModeKind,
    render_ctx: &ConfigV2RenderContext,
) -> String {
    match control_surface_mode(
        proxy_kind,
        render_ctx.profile_control_plane_enabled || render_ctx.station_control_plane_enabled,
        !render_ctx.attached_mode,
    ) {
        ControlSurfaceMode::DirectRemote => format!(
            "{} · {}",
            render_ctx.selected_service,
            pick(
                lang,
                "变更会直接写到附着代理并刷新远端运行态",
                "Writes go straight to the attached proxy runtime",
            )
        ),
        ControlSurfaceMode::DirectLocal => format!(
            "{} · {}",
            render_ctx.selected_service,
            pick(
                lang,
                "变更通过本机 control-plane 落盘并刷新运行态",
                "Writes go through the local control plane and refresh runtime",
            )
        ),
        ControlSurfaceMode::LocalDraft => format!(
            "{} · {}",
            render_ctx.selected_service,
            pick(
                lang,
                "当前编辑本机配置文档，需重新运行或附着后生效",
                "Editing the local config document; run or attach to apply",
            )
        ),
        ControlSurfaceMode::Unavailable => format!(
            "{} · {}",
            render_ctx.selected_service,
            pick(
                lang,
                "当前附着目标未暴露对应写接口，只能查看或切回本机文稿",
                "The attached target does not expose write APIs for this surface",
            )
        ),
    }
}

fn station_summary_hint(
    lang: Language,
    render_ctx: &ConfigV2RenderContext,
    station_count: usize,
) -> String {
    let configured = render_ctx
        .configured_active_name
        .as_deref()
        .unwrap_or_else(|| pick(lang, "<无>", "<none>"));
    let effective = render_ctx
        .effective_active_name
        .as_deref()
        .unwrap_or_else(|| pick(lang, "<自动>", "<auto>"));
    format!(
        "{} {} · {} {} · {} {}",
        pick(lang, "配置", "cfg"),
        configured,
        pick(lang, "生效", "eff"),
        effective,
        pick(lang, "总数", "total"),
        station_count
    )
}

fn profile_summary_hint(
    lang: Language,
    proxy_kind: ProxyModeKind,
    render_ctx: &ConfigV2RenderContext,
    profile_count: usize,
) -> String {
    let surface = surface_mode_short_label(
        lang,
        control_surface_mode(
            proxy_kind,
            render_ctx.profile_control_plane_enabled,
            !render_ctx.attached_mode,
        ),
    );
    format!(
        "{} · {} {}",
        surface,
        pick(lang, "模板", "profiles"),
        profile_count
    )
}

fn provider_summary_hint(
    lang: Language,
    proxy_kind: ProxyModeKind,
    render_ctx: &ConfigV2RenderContext,
    provider_count: usize,
) -> String {
    let surface = surface_mode_short_label(
        lang,
        control_surface_mode(
            proxy_kind,
            render_ctx.provider_structure_control_plane_enabled,
            render_ctx.provider_structure_edit_enabled,
        ),
    );
    let lead = render_ctx
        .provider_display_names
        .first()
        .cloned()
        .unwrap_or_else(|| pick(lang, "未配置", "empty").to_string());
    format!(
        "{} · {} {} · {}",
        surface,
        pick(lang, "总数", "total"),
        provider_count,
        lead
    )
}

fn render_surface_chip(ui: &mut egui::Ui, lang: Language, title: &str, mode: ControlSurfaceMode) {
    let (status, color) = surface_mode_chip(lang, mode);
    let text = format!("{title}: {status}");
    ui.label(egui::RichText::new(text).color(color));
}

fn resolve_profile_for_deck(
    render_ctx: &ConfigV2RenderContext,
    profile_name: &str,
) -> Option<(String, crate::config::ServiceControlProfile)> {
    let profile = crate::config::resolve_service_profile_from_catalog(
        current_profile_catalog(render_ctx),
        profile_name,
    )
    .ok()
    .or_else(|| {
        current_profile_catalog(render_ctx)
            .get(profile_name)
            .cloned()
    })?;
    Some((profile_name.to_string(), profile))
}

fn current_profile_catalog(
    render_ctx: &ConfigV2RenderContext,
) -> &BTreeMap<String, crate::config::ServiceControlProfile> {
    if render_ctx.profile_control_plane_enabled {
        &render_ctx.profile_control_plane_catalog
    } else {
        &render_ctx.profile_catalog
    }
}

fn focus_provider_name(render_ctx: &ConfigV2RenderContext) -> Option<String> {
    let active_station_name = render_ctx
        .effective_active_name
        .as_deref()
        .or(render_ctx.configured_active_name.as_deref());

    if let Some(station_name) = active_station_name
        && let Some(station_specs) = render_ctx.preview_station_specs()
        && let Some(station) = station_specs.get(station_name)
        && let Some(member) = station.members.first()
    {
        return Some(member.provider.clone());
    }

    render_ctx.provider_display_names.first().cloned()
}

fn surface_mode_short_label(lang: Language, mode: ControlSurfaceMode) -> &'static str {
    match mode {
        ControlSurfaceMode::DirectLocal => pick(lang, "本机直写", "local-direct"),
        ControlSurfaceMode::DirectRemote => pick(lang, "远端直写", "remote-direct"),
        ControlSurfaceMode::LocalDraft => pick(lang, "本地文稿", "local-draft"),
        ControlSurfaceMode::Unavailable => pick(lang, "不可用", "unavailable"),
    }
}

fn surface_mode_chip(lang: Language, mode: ControlSurfaceMode) -> (&'static str, egui::Color32) {
    match mode {
        ControlSurfaceMode::DirectLocal => (
            pick(lang, "本机直写", "local-direct"),
            egui::Color32::from_rgb(63, 120, 191),
        ),
        ControlSurfaceMode::DirectRemote => (
            pick(lang, "远端直写", "remote-direct"),
            egui::Color32::from_rgb(54, 153, 94),
        ),
        ControlSurfaceMode::LocalDraft => (
            pick(lang, "本地文稿", "local-draft"),
            egui::Color32::from_rgb(196, 140, 70),
        ),
        ControlSurfaceMode::Unavailable => (
            pick(lang, "不可用", "unavailable"),
            egui::Color32::from_rgb(160, 84, 84),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_surface_mode_prefers_direct_over_local_draft() {
        assert_eq!(
            control_surface_mode(ProxyModeKind::Attached, true, true),
            ControlSurfaceMode::DirectRemote
        );
        assert_eq!(
            control_surface_mode(ProxyModeKind::Running, true, true),
            ControlSurfaceMode::DirectLocal
        );
    }

    #[test]
    fn control_surface_mode_falls_back_to_local_draft_when_editable() {
        assert_eq!(
            control_surface_mode(ProxyModeKind::Attached, false, true),
            ControlSurfaceMode::LocalDraft
        );
    }

    #[test]
    fn control_surface_mode_marks_unavailable_when_no_surface_exists() {
        assert_eq!(
            control_surface_mode(ProxyModeKind::Attached, false, false),
            ControlSurfaceMode::Unavailable
        );
    }
}

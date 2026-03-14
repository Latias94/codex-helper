use super::config_v2::context::ConfigV2RenderContext;
use super::view_state::ConfigV2Section;
use super::*;

pub(super) fn render_config_v2_workspace_header(
    ui: &mut egui::Ui,
    lang: Language,
    proxy_kind: ProxyModeKind,
    render_ctx: &ConfigV2RenderContext,
    section: &mut ConfigV2Section,
) {
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
    let focus_hint = match section {
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
                pick(lang, "Stations", "Stations"),
                station_count.to_string(),
                render_ctx
                    .effective_active_name
                    .as_deref()
                    .unwrap_or_else(|| pick(lang, "未设 active", "no active")),
            );
            render_config_v2_summary_card(
                &mut cols[1],
                pick(lang, "Providers", "Providers"),
                provider_count.to_string(),
                render_ctx
                    .provider_display_names
                    .first()
                    .map(String::as_str)
                    .unwrap_or_else(|| pick(lang, "未配置", "empty")),
            );
            render_config_v2_summary_card(
                &mut cols[2],
                pick(lang, "Profiles", "Profiles"),
                profile_count.to_string(),
                render_ctx
                    .station_default_profile
                    .as_deref()
                    .unwrap_or_else(|| pick(lang, "无默认", "no default")),
            );
            render_config_v2_summary_card(
                &mut cols[3],
                pick(lang, "Service", "Service"),
                render_ctx.selected_service.to_string(),
                match section {
                    ConfigV2Section::Stations => pick(lang, "当前聚焦: 路由", "Focus: routing"),
                    ConfigV2Section::Providers => pick(lang, "当前聚焦: 来源", "Focus: providers"),
                    ConfigV2Section::Profiles => pick(lang, "当前聚焦: 策略", "Focus: profiles"),
                },
            );
        });

        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
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

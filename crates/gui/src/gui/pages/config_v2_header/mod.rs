use super::config_v2::context::ProxySettingsRenderContext;
use super::view_state::ProxySettingsSection;
use super::*;

mod actions;
mod focus_targets;
mod runtime_card;
mod surface_mode;

use actions::render_control_deck_actions;
use focus_targets::render_control_deck_focus_targets;
use runtime_card::{proxy_mode_label, render_control_deck_runtime_card};
use surface_mode::{
    control_scope_hint, control_scope_label, control_surface_mode, profile_summary_hint,
    provider_summary_hint, render_surface_chip, station_summary_hint,
};

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
    render_ctx: &ProxySettingsRenderContext,
) {
    let lang = ctx.lang;
    let proxy_kind = ctx.proxy.kind();
    let current_section = ctx.view.proxy_settings.section;
    let station_count = render_ctx.station_display_names.len();
    let provider_count = render_ctx.provider_display_names.len();
    let profile_count = if render_ctx.profile_control_plane_enabled {
        render_ctx.profile_control_plane_catalog.len()
    } else {
        render_ctx.profile_catalog.len()
    };
    let mode_label = proxy_mode_label(lang, proxy_kind);
    let focus_hint = match current_section {
        ProxySettingsSection::Stations => pick(
            lang,
            "适合调整默认路由、成员组合、健康检查与 active station。",
            "Best for routing topology, health checks, and active station control.",
        ),
        ProxySettingsSection::Providers => pick(
            lang,
            "适合管理中转来源、认证引用、endpoint 集合和后续故障切换基础。",
            "Best for relay sources, auth references, endpoint sets, and future failover basics.",
        ),
        ProxySettingsSection::Profiles => pick(
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
        render_control_deck_runtime_card(ui, ctx, proxy_kind, render_ctx);

        ui.add_space(8.0);
        render_control_deck_actions(ui, ctx, proxy_kind, render_ctx);

        ui.add_space(8.0);
        render_control_deck_focus_targets(ui, ctx, render_ctx);

        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            let section = &mut ctx.view.proxy_settings.section;
            ui.selectable_value(
                section,
                ProxySettingsSection::Profiles,
                format!("{} · {}", pick(lang, "Profiles", "Profiles"), profile_count),
            );
            ui.selectable_value(
                section,
                ProxySettingsSection::Stations,
                format!("{} · {}", pick(lang, "Stations", "Stations"), station_count),
            );
            ui.selectable_value(
                section,
                ProxySettingsSection::Providers,
                format!(
                    "{} · {}",
                    pick(lang, "Providers", "Providers"),
                    provider_count
                ),
            );
        });
    });
}

pub(super) fn render_config_v2_summary_card(
    ui: &mut egui::Ui,
    title: &str,
    value: String,
    hint: &str,
) {
    ui.group(|ui| {
        ui.small(title);
        ui.heading(value);
        ui.small(hint);
    });
}

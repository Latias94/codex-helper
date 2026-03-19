use super::sessions_controller::{
    apply_session_page_actions, build_runtime_station_catalog, build_session_render_data,
    build_session_rows_for_snapshot, resolve_session_preview_catalogs,
    sync_default_profile_selection, sync_session_editor_from_selection,
};
use super::sessions_split_view::render_sessions_split_view;
use super::sessions_toolbar::{render_default_profile_section, render_session_filter_controls};
use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "会话", "Sessions"));

    let Some(snapshot) = ctx.proxy.snapshot() else {
        ui.separator();
        ui.label(pick(
            ctx.lang,
            "当前未运行代理，也未附着到现有代理。请在“总览”里启动或附着后再查看会话。",
            "No proxy is running or attached. Start or attach on Overview to view sessions.",
        ));
        return;
    };
    let host_local_session_features = host_local_session_features_available(ctx.proxy);
    let mut force_refresh = false;

    let profiles = snapshot.profiles.clone();
    let default_profile = snapshot.default_profile.clone();
    let global_station_override = snapshot.global_station_override.clone();
    let configured_active_station = snapshot.configured_active_station.clone();
    let effective_active_station = snapshot.effective_active_station.clone();

    let runtime_station_catalog = build_runtime_station_catalog(&snapshot);
    let session_preview_service_name =
        snapshot
            .service_name
            .as_deref()
            .unwrap_or(match ctx.view.proxy_settings.service {
                crate::config::ServiceKind::Claude => "claude",
                crate::config::ServiceKind::Codex => "codex",
            });
    let session_preview_catalogs =
        resolve_session_preview_catalogs(ctx, session_preview_service_name);
    let session_preview_station_specs = session_preview_catalogs
        .as_ref()
        .map(|(stations, _)| stations);
    let session_preview_provider_catalog = session_preview_catalogs
        .as_ref()
        .map(|(_, providers)| providers);
    let session_preview_runtime_station_catalog = Some(&runtime_station_catalog);

    sync_default_profile_selection(
        &mut ctx.view.sessions,
        default_profile.as_deref(),
        &profiles,
    );

    if let Some(err) = snapshot.last_error.as_deref() {
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
        ui.add_space(4.0);
    }

    if remote_attached_proxy_active(ctx.proxy) {
        ui.colored_label(
            egui::Color32::from_rgb(200, 120, 40),
            pick(
                ctx.lang,
                "当前附着的是远端代理：共享的 session 控制仍可用，但 cwd / transcript 这类 host-local 入口已按远端模式收敛。",
                "A remote proxy is attached: shared session controls remain available, but host-local entries such as cwd/transcript are gated for remote safety.",
            ),
        );
        ui.add_space(4.0);
    }

    force_refresh |=
        render_default_profile_section(ui, ctx, &snapshot, &profiles, default_profile.as_deref());
    render_session_filter_controls(ui, ctx);

    let (has_session_cards, rows) = build_session_rows_for_snapshot(&snapshot);
    let render_data = build_session_render_data(&mut ctx.view.sessions, rows);
    sync_session_editor_from_selection(
        &mut ctx.view.sessions,
        render_data.selected_row(),
        &profiles,
        default_profile.as_deref(),
    );

    let actions = render_sessions_split_view(
        ui,
        ctx,
        &snapshot,
        &render_data,
        &profiles,
        global_station_override.as_deref(),
        configured_active_station.as_deref(),
        effective_active_station.as_deref(),
        session_preview_station_specs,
        session_preview_provider_catalog,
        session_preview_runtime_station_catalog,
        has_session_cards,
        host_local_session_features,
    );
    force_refresh |=
        apply_session_page_actions(ctx, actions, default_profile.as_deref(), &profiles);

    if force_refresh {
        ctx.proxy
            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
    }
}

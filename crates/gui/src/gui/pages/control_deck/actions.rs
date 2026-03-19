use super::*;

pub(super) fn render_control_deck_actions(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    proxy_kind: ProxyModeKind,
    render_ctx: &ProxySettingsRenderContext,
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
            if ui.button(pick(ctx.lang, "统计台", "Stats")).clicked() {
                ctx.view.requested_page = Some(Page::Stats);
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

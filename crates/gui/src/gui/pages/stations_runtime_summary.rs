use super::*;

pub(super) fn render_stations_runtime_summary(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
    configured_active_station: Option<&str>,
    effective_active_station: Option<&str>,
) {
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
            let global_runtime_override = if snapshot.supports_global_route_target_override {
                (
                    pick(ctx.lang, "全局 route target", "Global route target"),
                    snapshot.global_route_target_override.as_deref(),
                )
            } else {
                (
                    pick(ctx.lang, "全局站点覆盖", "Global pinned station"),
                    snapshot.global_station_override.as_deref(),
                )
            };
            ui.label(format!(
                "{}: {}",
                global_runtime_override.0,
                global_runtime_override
                    .1
                    .unwrap_or_else(|| pick(ctx.lang, "<自动>", "<auto>"))
            ));
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "配置 active_station", "Configured active_station"),
                configured_active_station.unwrap_or_else(|| pick(ctx.lang, "<无>", "<none>"))
            ));
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "生效站点", "Effective station"),
                effective_active_station
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
            if ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
            }
            if ui
                .button(pick(ctx.lang, "重载代理运行态", "Reload proxy runtime"))
                .clicked()
            {
                if let Err(error) = ctx.proxy.reload_runtime_config(ctx.rt) {
                    *ctx.last_error = Some(format!("reload runtime failed: {error}"));
                } else {
                    *ctx.last_info = Some(pick(ctx.lang, "已重载", "Reloaded").to_string());
                }
            }
            if ui
                .button(pick(ctx.lang, "打开代理设置页", "Open Proxy Settings"))
                .clicked()
            {
                ctx.view.requested_page = Some(Page::ProxySettings);
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
                pick(
                    ctx.lang,
                    "附着模式下，route target / 运行时覆盖会直接作用到远端代理；这里不再提供旧的持久化站点写回入口。",
                    "In attached mode, route targets and runtime overrides act on the remote proxy directly; the old persisted station write-back entrypoint is no longer shown here.",
                )
            } else {
                pick(
                    ctx.lang,
                    "这里的 route target / global pin 是运行时覆盖；配置文件编辑请走代理设置页的原始视图。",
                    "Route targets / global pins here are runtime-only; edit the config file through the Proxy Settings raw view.",
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
}

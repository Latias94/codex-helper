use super::*;

pub(super) fn render_stations_runtime_summary(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
    configured_active_station: Option<&str>,
    effective_active_station: Option<&str>,
    supports_persisted_station_settings: bool,
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
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "持久化站点设置", "Persisted station settings"),
                if supports_persisted_station_settings {
                    pick(ctx.lang, "可用", "available")
                } else {
                    pick(ctx.lang, "不可用", "unavailable")
                }
            ));
            if matches!(snapshot.kind, ProxyModeKind::Attached) {
                ui.label(format!(
                    "{}: {}",
                    pick(ctx.lang, "远端写回", "Remote write-back"),
                    if supports_persisted_station_settings {
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
                if supports_persisted_station_settings {
                    pick(
                        ctx.lang,
                        "附着模式下，global pin / runtime 覆盖会直接作用到远端代理；下面的“持久化站点设置”也会直接写回远端代理的持久化状态，不会改动本机文件。",
                        "In attached mode, global pin and runtime overrides act on the remote proxy directly; the persisted station settings below also write back to the remote proxy's persisted state rather than this device's local file.",
                    )
                } else {
                    pick(
                        ctx.lang,
                        "附着模式下，global pin / runtime 覆盖会直接作用到远端代理；当前附着目标还没有暴露持久化站点设置 API，因此只能做运行时控制。",
                        "In attached mode, global pin and runtime overrides act on the remote proxy directly; this attached target does not expose persisted station settings APIs yet, so only runtime controls are available.",
                    )
                }
            } else {
                pick(
                    ctx.lang,
                    "这里的 global pin 是运行时覆盖；“持久化站点设置”会通过本地 control-plane 写回配置文件并刷新运行态。",
                    "Global pin here is runtime-only; the persisted station settings write through the local control plane and refresh the runtime.",
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

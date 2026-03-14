use super::*;

pub(super) fn render_attached_proxy_summary(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let Some(attached) = ctx.proxy.attached() else {
        return;
    };

    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "已附着", "Attached"),
        attached.base_url
    ));
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "活跃请求", "Active requests"),
        attached.active.len()
    ));
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "最近请求(<=200)", "Recent (<=200)"),
        attached.recent.len()
    ));
    if let Some(version) = attached.api_version {
        ui.label(format!(
            "{}: v{}",
            pick(ctx.lang, "API 版本", "API version"),
            version
        ));
    }
    if let Some(service) = attached.service_name.as_deref() {
        ui.label(format!("{}: {service}", pick(ctx.lang, "服务", "Service")));
    }
    if let Some(loaded_at_ms) = attached.runtime_loaded_at_ms {
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "运行态配置 loaded_at_ms", "runtime loaded_at_ms"),
            loaded_at_ms
        ));
    }
    if let Some(mtime_ms) = attached.runtime_source_mtime_ms {
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "运行态配置 mtime_ms", "runtime mtime_ms"),
            mtime_ms
        ));
    }
    if let Some(error) = attached.last_error.as_deref() {
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), error);
    }
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "全局覆盖(Pinned)", "Global override (pinned)"),
        attached
            .global_station_override
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
        attached.admin_base_url.as_str(),
        &attached.host_local_capabilities,
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
        attached.admin_base_url.as_str(),
        &attached.remote_admin_access,
        ctx.lang,
    ) {
        let color = if attached.remote_admin_access.remote_enabled && remote_admin_token_present() {
            egui::Color32::from_rgb(60, 160, 90)
        } else {
            egui::Color32::from_rgb(200, 120, 40)
        };
        let response = ui.colored_label(color, label);
        let remote_admin_message = remote_admin_access_message(
            attached.admin_base_url.as_str(),
            &attached.remote_admin_access,
            ctx.lang,
        );
        if let Some(message) = remote_admin_message.clone() {
            response.on_hover_text(message.clone());
            if !attached.remote_admin_access.remote_enabled || !remote_admin_token_present() {
                ui.colored_label(egui::Color32::from_rgb(200, 120, 40), message);
            }
        }
    }
}

use super::*;

#[derive(Debug, Default)]
pub(super) struct ProxyDiscoveryActions {
    pub scan_local_proxies: bool,
    pub attach_discovered: Option<DiscoveredProxy>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ProxyDiscoveryListOptions<'a> {
    pub scroll_id: &'a str,
    pub grid_id: &'a str,
    pub max_height: f32,
    pub empty_text: &'a str,
    pub attach_enabled: bool,
    pub show_port_hover_details: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ProxyDiscoveryApplyOptions<'a> {
    pub scan_done_none: &'a str,
    pub scan_done_found: &'a str,
    pub attach_success: &'a str,
    pub sync_desired_port: bool,
    pub sync_default_port: bool,
}

pub(super) fn render_proxy_discovery_list(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    options: ProxyDiscoveryListOptions<'_>,
    actions: &mut ProxyDiscoveryActions,
) {
    let discovered = ctx.proxy.discovered_proxies().to_vec();
    if discovered.is_empty() {
        ui.label(options.empty_text);
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt(options.scroll_id)
        .max_height(options.max_height)
        .show(ui, |ui| {
            egui::Grid::new(options.grid_id)
                .striped(true)
                .show(ui, |ui| {
                    ui.label(pick(ctx.lang, "端口", "Port"));
                    ui.label(pick(ctx.lang, "服务", "Service"));
                    ui.label(pick(ctx.lang, "API", "API"));
                    ui.label(pick(ctx.lang, "状态", "Status"));
                    ui.end_row();

                    let now = now_ms();
                    for proxy in discovered {
                        render_proxy_discovery_row(
                            ui,
                            ctx,
                            &proxy,
                            options.attach_enabled,
                            options.show_port_hover_details,
                            now,
                            actions,
                        );
                    }
                });
        });
}

pub(super) fn apply_proxy_discovery_actions(
    ctx: &mut PageCtx<'_>,
    actions: ProxyDiscoveryActions,
    options: ProxyDiscoveryApplyOptions<'_>,
) {
    if actions.scan_local_proxies {
        if let Err(e) = ctx.proxy.scan_local_proxies(ctx.rt, 3210..=3220) {
            *ctx.last_error = Some(format!("scan failed: {e}"));
        } else if ctx.proxy.discovered_proxies().is_empty() {
            *ctx.last_info = Some(options.scan_done_none.to_string());
        } else {
            *ctx.last_info = Some(options.scan_done_found.to_string());
        }
    }

    if let Some(proxy) = actions.attach_discovered {
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
        if options.sync_desired_port {
            ctx.proxy.set_desired_port(proxy.port);
        }
        ctx.gui_cfg.attach.last_port = Some(proxy.port);
        if options.sync_default_port {
            ctx.gui_cfg.proxy.default_port = proxy.port;
        }
        if let Err(e) = ctx.gui_cfg.save() {
            *ctx.last_error = Some(format!("save gui config failed: {e}"));
        } else {
            *ctx.last_info = Some(merge_info_message(
                options.attach_success.to_string(),
                [warning, admin_message].into_iter().flatten(),
            ));
        }
    }
}

fn render_proxy_discovery_row(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    proxy: &DiscoveredProxy,
    attach_enabled: bool,
    show_port_hover_details: bool,
    now: u64,
    actions: &mut ProxyDiscoveryActions,
) {
    if show_port_hover_details {
        let mut hover = format!("base_url: {}", proxy.base_url);
        if !proxy.endpoints.is_empty() {
            hover.push_str(&format!("\nendpoints: {}", proxy.endpoints.len()));
        }
        if let Some(ms) = proxy.runtime_loaded_at_ms {
            hover.push_str(&format!("\nruntime_loaded: {}", format_age(now, Some(ms))));
        }
        ui.label(proxy.port.to_string()).on_hover_text(hover);
    } else {
        ui.label(proxy.port.to_string());
    }

    ui.label(
        proxy
            .service_name
            .as_deref()
            .unwrap_or_else(|| pick(ctx.lang, "未知", "unknown")),
    );
    ui.label(match proxy.api_version {
        Some(v) => format!("v{v}"),
        None => "-".to_string(),
    });
    ui.vertical(|ui| {
        if let Some(err) = proxy.last_error.as_deref() {
            ui.label(err);
        } else {
            ui.label(pick(ctx.lang, "可用", "OK"));
        }
        if let Some(warning) = remote_local_only_warning_message(
            proxy.admin_base_url.as_str(),
            &proxy.host_local_capabilities,
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
            proxy.admin_base_url.as_str(),
            &proxy.remote_admin_access,
            ctx.lang,
        ) {
            let color = if proxy.remote_admin_access.remote_enabled && remote_admin_token_present()
            {
                egui::Color32::from_rgb(60, 160, 90)
            } else {
                egui::Color32::from_rgb(200, 120, 40)
            };
            let response = ui.colored_label(color, label);
            if let Some(message) = remote_admin_access_message(
                proxy.admin_base_url.as_str(),
                &proxy.remote_admin_access,
                ctx.lang,
            ) {
                response.on_hover_text(message);
            }
        }
    });

    if ui
        .add_enabled(
            attach_enabled,
            egui::Button::new(pick(ctx.lang, "附着", "Attach")),
        )
        .clicked()
    {
        actions.attach_discovered = Some(proxy.clone());
    }
    ui.end_row();
}

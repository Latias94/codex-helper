use super::proxy_discovery::{ProxyDiscoveryApplyOptions, apply_proxy_discovery_actions};
use super::setup_client_step::render_setup_client_step;
use super::setup_config_step::render_setup_config_step;
use super::setup_proxy_step::render_setup_proxy_step;
use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "快速设置", "Setup"));
    ui.label(pick(
        ctx.lang,
        "目标：让 Codex/Claude 走本地 codex-helper 代理（常驻后台），并完成基础配置。",
        "Goal: route Codex/Claude through the local codex-helper proxy (resident) and complete basic setup.",
    ));
    ui.label(pick(
        ctx.lang,
        "推荐顺序：先 1) 配置，再 2) 启动/附着代理，最后 3) 切换客户端。如果你已在 TUI 启动代理，请在第 2 步使用“扫描并附着”。",
        "Recommended order: 1) config, 2) start/attach proxy, 3) switch client. If you already started the proxy in TUI, use “Scan & attach” in step 2.",
    ));
    ui.separator();

    render_setup_config_step(ui, ctx);
    ui.add_space(10.0);

    let discovery_actions = render_setup_proxy_step(ui, ctx);
    ui.add_space(10.0);

    render_setup_client_step(ui, ctx);

    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if ui
            .button(pick(ctx.lang, "我已完成，前往总览", "Done, go to Overview"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Overview);
        }
    });

    apply_proxy_discovery_actions(
        ctx,
        discovery_actions,
        ProxyDiscoveryApplyOptions {
            scan_done_none: pick(ctx.lang, "扫描完成：未发现代理", "Scan done: none found"),
            scan_done_found: pick(
                ctx.lang,
                "扫描完成：请选择一个代理进行附着",
                "Scan done: pick a proxy to attach",
            ),
            attach_success: pick(ctx.lang, "已附着到代理。", "Attached."),
            sync_desired_port: true,
            sync_default_port: true,
        },
    );
}

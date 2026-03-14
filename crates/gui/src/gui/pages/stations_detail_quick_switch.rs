use super::stations_detail_controls::refresh_runtime_snapshot;
use super::*;

pub(super) fn render_station_quick_switch_section(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    cfg: &StationOption,
    snapshot: &GuiRuntimeSnapshot,
) {
    ui.label(pick(
        ctx.lang,
        "Quick switch（运行时）",
        "Quick switch (runtime)",
    ));
    ui.separator();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                snapshot.supports_v1,
                egui::Button::new(pick(ctx.lang, "Pin 当前站点", "Pin selected station")),
            )
            .clicked()
        {
            match ctx
                .proxy
                .apply_global_station_override(ctx.rt, Some(cfg.name.clone()))
            {
                Ok(()) => {
                    refresh_runtime_snapshot(ctx);
                    *ctx.last_info = Some(
                        pick(ctx.lang, "已应用全局站点覆盖", "Global station pin applied")
                            .to_string(),
                    );
                }
                Err(error) => {
                    *ctx.last_error = Some(format!("apply global override failed: {error}"));
                }
            }
        }
        if ui
            .add_enabled(
                snapshot.supports_v1 && snapshot.global_station_override.is_some(),
                egui::Button::new(pick(ctx.lang, "清除 global pin", "Clear global pin")),
            )
            .clicked()
        {
            match ctx.proxy.apply_global_station_override(ctx.rt, None) {
                Ok(()) => {
                    refresh_runtime_snapshot(ctx);
                    *ctx.last_info =
                        Some(pick(ctx.lang, "已清除全局覆盖", "Global pin cleared").to_string());
                }
                Err(error) => {
                    *ctx.last_error = Some(format!("clear global override failed: {error}"));
                }
            }
        }
    });
    ui.small(pick(
        ctx.lang,
        "这里的 pin 只影响当前代理运行态，不修改配置文件。",
        "Pins here only affect the current proxy runtime and do not rewrite persisted config.",
    ));
}

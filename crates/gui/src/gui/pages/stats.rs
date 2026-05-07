use super::stats_balance::render_balance_overview;
use super::stats_control_trace::render_control_trace_panel;
use super::stats_pricing::render_pricing_catalog;
use super::stats_summary::render_stats_summary;
use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "统计", "Stats"));
    render_stats_summary(ui, ctx);
    ui.add_space(10.0);
    render_balance_overview(ui, ctx);
    ui.add_space(10.0);
    render_pricing_catalog(ui, ctx);
    ui.add_space(10.0);
    ui.separator();
    render_control_trace_panel(ui, ctx);
}

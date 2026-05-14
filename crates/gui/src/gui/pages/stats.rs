use super::stats_balance::render_balance_overview;
use super::stats_control_trace::render_control_trace_panel;
use super::stats_pricing::render_pricing_catalog;
use super::stats_request_ledger::render_request_ledger_summary_panel;
use super::stats_summary::render_stats_summary;
use super::*;
use crate::gui::proxy_control::{GuiRuntimeSnapshot, ProviderBalanceRefreshStatus};
use crate::usage_balance::{UsageBalanceBuildInput, UsageBalanceRefreshInput, UsageBalanceView};

pub(super) fn build_usage_balance_view(
    snapshot: &GuiRuntimeSnapshot,
    refresh: &ProviderBalanceRefreshStatus,
) -> UsageBalanceView {
    UsageBalanceView::build(UsageBalanceBuildInput {
        service_name: snapshot.service_name.as_deref().unwrap_or("codex"),
        window_days: 0,
        generated_at_ms: now_ms(),
        usage_rollup: &snapshot.usage_rollup,
        provider_balances: &snapshot.provider_balances,
        recent: &snapshot.recent,
        routing_explain: snapshot.routing_explain.as_ref(),
        refresh: UsageBalanceRefreshInput {
            refreshing: refresh.refreshing,
            last_message: refresh.last_message.clone(),
            last_error: refresh.last_error.clone(),
        },
    })
}

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let snapshot = ctx.proxy.snapshot();
    let refresh_status = ctx.proxy.provider_balance_refresh_status().clone();
    let usage_balance = snapshot
        .as_ref()
        .map(|snapshot| build_usage_balance_view(snapshot, &refresh_status));

    ui.heading(pick(ctx.lang, "用量 / 余额", "Usage / Balance"));
    render_stats_summary(ui, ctx, snapshot.as_ref(), usage_balance.as_ref());
    ui.add_space(10.0);
    render_balance_overview(ui, ctx, snapshot.as_ref(), usage_balance.as_ref());
    ui.add_space(10.0);
    render_request_ledger_summary_panel(ui, ctx);
    ui.add_space(10.0);
    render_pricing_catalog(ui, ctx);
    ui.add_space(10.0);
    ui.separator();
    render_control_trace_panel(ui, ctx);
}

use eframe::egui;

use super::super::i18n::pick;
use super::history_all_by_date::render_history_all_by_date;
use super::history_controller::{
    apply_pending_metadata_filter, history_refresh_needed, poll_history_refresh_loader,
    poll_tail_transcript_search_loader, stabilize_history_selection,
};
use super::history_main_view::render_history_content;
use super::history_state::{HistoryDataSource, HistoryScope};
use super::history_toolbar::{
    handle_history_shortcuts, render_history_controls, render_history_header,
};
use super::history_transcript_runtime::poll_transcript_loader;
use super::{PageCtx, remote_attached_proxy_active};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResolvedHistoryLayout {
    Horizontal,
    Vertical,
}

fn resolve_history_layout(layout_mode: &str, available_width: f32) -> ResolvedHistoryLayout {
    match layout_mode.trim().to_ascii_lowercase().as_str() {
        "horizontal" | "h" => ResolvedHistoryLayout::Horizontal,
        "vertical" | "v" => ResolvedHistoryLayout::Vertical,
        "auto" | "" => {
            if available_width < 980.0 {
                ResolvedHistoryLayout::Vertical
            } else {
                ResolvedHistoryLayout::Horizontal
            }
        }
        _ => {
            if available_width < 980.0 {
                ResolvedHistoryLayout::Vertical
            } else {
                ResolvedHistoryLayout::Horizontal
            }
        }
    }
}

fn render_observed_fallback_notice(ui: &mut egui::Ui, ctx: &PageCtx<'_>) {
    ui.add_space(4.0);
    ui.group(|ui| {
        ui.colored_label(
            egui::Color32::from_rgb(200, 120, 40),
            pick(
                ctx.lang,
                "当前显示的是共享观测会话摘要，不是本机 ~/.codex/sessions 文件列表。",
                "The current list is built from shared observed sessions, not this device's ~/.codex/sessions files.",
            ),
        );
        ui.small(pick(
            ctx.lang,
            "可用：筛选、选择、查看 route 摘要。不可用：transcript、resume、open file 这类 host-local 文件动作。",
            "Available: filtering, selection, and route summary browsing. Unavailable: transcript, resume, and open-file actions that require host-local files.",
        ));
    });
}

pub(super) fn render_history(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    poll_transcript_loader(ctx);
    poll_tail_transcript_search_loader(ctx);
    let remote_attached = remote_attached_proxy_active(ctx.proxy);
    let (shared_observed_history_available, attached_host_local_history_advertised) = ctx
        .proxy
        .snapshot()
        .map(|snapshot| {
            (
                snapshot.shared_capabilities.session_observability,
                snapshot.host_local_capabilities.session_history,
            )
        })
        .unwrap_or((false, false));
    poll_history_refresh_loader(ctx);

    render_history_header(
        ui,
        ctx,
        remote_attached,
        shared_observed_history_available,
        attached_host_local_history_advertised,
    );

    let mut refresh_requested = history_refresh_needed(&ctx.view.history, remote_attached);

    ui.add_space(6.0);
    handle_history_shortcuts(ui, ctx);
    render_history_controls(
        ui,
        ctx,
        shared_observed_history_available,
        &mut refresh_requested,
    );

    if ctx.view.history.scope == HistoryScope::AllByDate {
        let layout =
            resolve_history_layout(ctx.view.history.layout_mode.as_str(), ui.available_width());
        render_history_all_by_date(ui, ctx, layout);
        return;
    }

    apply_pending_metadata_filter(ctx);

    if ctx.view.history.data_source == HistoryDataSource::ObservedFallback {
        render_observed_fallback_notice(ui, ctx);
    }

    if let Some(err) = ctx.view.history.last_error.as_deref() {
        ui.add_space(4.0);
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
    }
    if ctx.view.history.refresh_load.is_some() {
        ui.add_space(4.0);
        ui.label(pick(
            ctx.lang,
            "正在刷新会话列表...",
            "Refreshing sessions...",
        ));
    } else if ctx.view.history.tail_search_load.is_some() {
        ui.add_space(4.0);
        ui.label(pick(
            ctx.lang,
            "正在搜索对话尾部...",
            "Searching transcript tails...",
        ));
    }

    if ctx.view.history.sessions.is_empty() {
        ui.add_space(8.0);
        if ctx.view.history.refresh_load.is_none() && ctx.view.history.tail_search_load.is_none() {
            ui.label(pick(
                ctx.lang,
                "暂无会话。点击“刷新”加载。",
                "No sessions loaded. Click Refresh.",
            ));
        }
        return;
    }

    stabilize_history_selection(&mut ctx.view.history);

    let layout =
        resolve_history_layout(ctx.view.history.layout_mode.as_str(), ui.available_width());
    render_history_content(ui, ctx, layout);
}

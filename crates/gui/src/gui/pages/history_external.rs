use eframe::egui;

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExternalHistoryOrigin {
    Sessions,
    Requests,
}

#[derive(Debug, Clone)]
pub(super) struct ExternalHistoryFocus {
    pub(super) summary: SessionSummary,
    pub(super) origin: ExternalHistoryOrigin,
}

pub(super) fn merge_external_focus_session(
    list: &mut Vec<SessionSummary>,
    focus: &ExternalHistoryFocus,
) {
    if let Some(existing) = list
        .iter_mut()
        .find(|summary| summary.id == focus.summary.id)
    {
        let prefer_focus = matches!(focus.summary.source, SessionSummarySource::LocalFile)
            && !matches!(existing.source, SessionSummarySource::LocalFile);
        if prefer_focus {
            *existing = focus.summary.clone();
            return;
        }

        if existing.cwd.is_none() {
            existing.cwd = focus.summary.cwd.clone();
        }
        if existing.first_user_message.is_none() {
            existing.first_user_message = focus.summary.first_user_message.clone();
        }
        if existing.sort_hint_ms.is_none() {
            existing.sort_hint_ms = focus.summary.sort_hint_ms;
        }
        if existing.updated_at.is_none() {
            existing.updated_at = focus.summary.updated_at.clone();
        }
        if existing.last_response_at.is_none() {
            existing.last_response_at = focus.summary.last_response_at.clone();
        }
        if existing.path.as_os_str().is_empty() && !focus.summary.path.as_os_str().is_empty() {
            existing.path = focus.summary.path.clone();
        }
        return;
    }

    list.insert(0, focus.summary.clone());
}

pub(super) fn ensure_external_focus_visible(state: &mut super::history::HistoryViewState) {
    let Some(focus) = state.external_focus.as_ref() else {
        return;
    };
    if state.selected_id.as_deref() != Some(focus.summary.id.as_str()) {
        return;
    }
    if !state
        .sessions
        .iter()
        .any(|summary| summary.id == focus.summary.id)
    {
        state.sessions.insert(0, focus.summary.clone());
    }
}

pub(super) fn prepare_select_session_from_external(
    state: &mut super::history::HistoryViewState,
    summary: SessionSummary,
    origin: ExternalHistoryOrigin,
) {
    let sid = summary.id.clone();
    state.scope = super::history::HistoryScope::GlobalRecent;
    state.query.clear();
    state.applied_scope = super::history::HistoryScope::GlobalRecent;
    state.applied_query.clear();
    state.search_transcript_applied = None;
    state.external_focus = Some(ExternalHistoryFocus { summary, origin });
    if let Some(focus) = state.external_focus.as_ref() {
        merge_external_focus_session(&mut state.sessions_all, focus);
        merge_external_focus_session(&mut state.sessions, focus);
    }
    state.selected_idx = 0;
    state.selected_id = Some(sid.clone());
    ensure_external_focus_visible(state);
    state.loaded_at_ms = None;
    state.loaded_for = None;
    state.auto_load_transcript = true;
    super::history::cancel_transcript_load(state);
    state.transcript_raw_messages.clear();
    state.transcript_messages.clear();
    state.transcript_error = None;
    state.transcript_scroll_to_msg_idx = None;
    state.transcript_plain_key = None;
    state.transcript_plain_text.clear();
    state.transcript_selected_msg_idx = 0;
}

fn history_summary_source_label(source: SessionSummarySource, lang: Language) -> &'static str {
    match source {
        SessionSummarySource::LocalFile => {
            pick(lang, "本地 transcript 文件", "Local transcript file")
        }
        SessionSummarySource::ObservedOnly => pick(lang, "共享观测摘要", "Shared observed summary"),
    }
}

fn external_history_origin_label(origin: ExternalHistoryOrigin, lang: Language) -> &'static str {
    match origin {
        ExternalHistoryOrigin::Sessions => pick(lang, "来自 Sessions", "Opened from Sessions"),
        ExternalHistoryOrigin::Requests => pick(lang, "来自 Requests", "Opened from Requests"),
    }
}

pub(super) fn render_history_selection_context(
    ui: &mut egui::Ui,
    lang: Language,
    state: &super::history::HistoryViewState,
    summary: &SessionSummary,
) {
    let color = match summary.source {
        SessionSummarySource::LocalFile => egui::Color32::from_rgb(60, 160, 90),
        SessionSummarySource::ObservedOnly => egui::Color32::from_rgb(200, 120, 40),
    };
    let focus_origin = state
        .external_focus
        .as_ref()
        .filter(|focus| focus.summary.id == summary.id)
        .map(|focus| focus.origin);

    ui.group(|ui| {
        if let Some(origin) = focus_origin {
            ui.small(format!(
                "{}: {}",
                pick(lang, "入口", "Entry"),
                external_history_origin_label(origin, lang)
            ));
        }
        ui.colored_label(
            color,
            format!(
                "{}: {}",
                pick(lang, "来源", "Source"),
                history_summary_source_label(summary.source, lang)
            ),
        );
        match summary.source {
            SessionSummarySource::LocalFile => {
                ui.small(pick(
                    lang,
                    "当前条目映射到这台设备可读取的本地 session 文件；resume、open file 和 transcript 动作都可用。",
                    "This item maps to a local session file readable on this device; resume, open-file, and transcript actions are available.",
                ));
                if !summary.path.as_os_str().is_empty() {
                    ui.small(format!("file: {}", summary.path.display()));
                }
            }
            SessionSummarySource::ObservedOnly => {
                ui.small(pick(
                    lang,
                    "当前条目只带共享观测摘要；可以浏览 session 标识和路由线索，但不能假设这台设备有对应 transcript 文件。",
                    "This item carries shared observed metadata only; you can inspect session identity and routing clues, but this device cannot assume a matching transcript file exists.",
                ));
            }
        }
    });
}

pub(super) fn render_open_in_sessions_button(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    selected_id: &str,
) {
    if ui
        .button(pick(ctx.lang, "在 Sessions 查看", "Open in Sessions"))
        .clicked()
    {
        super::focus_session_in_sessions(&mut ctx.view.sessions, selected_id.to_string());
        ctx.view.requested_page = Some(Page::Sessions);
        *ctx.last_info = Some(
            pick(
                ctx.lang,
                "已切到 Sessions 并定位到当前 session",
                "Opened in Sessions and focused the current session",
            )
            .to_string(),
        );
    }
}

pub(super) fn render_open_in_requests_button(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    selected_id: &str,
) {
    if ui
        .button(pick(ctx.lang, "在 Requests 查看", "Open in Requests"))
        .clicked()
    {
        super::focus_session_in_sessions(&mut ctx.view.sessions, selected_id.to_string());
        super::prepare_select_requests_for_session(&mut ctx.view.requests, selected_id.to_string());
        ctx.view.requested_page = Some(Page::Requests);
        *ctx.last_info = Some(
            pick(
                ctx.lang,
                "已切到 Requests 并限定到当前 session",
                "Opened in Requests and scoped to the current session",
            )
            .to_string(),
        );
    }
}

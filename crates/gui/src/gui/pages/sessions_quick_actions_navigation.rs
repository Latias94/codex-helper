use super::*;

pub(super) fn render_open_requests_action(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    row: &SessionRow,
) {
    let can_open_requests = row.session_id.is_some();
    let mut open_requests = ui.add_enabled(
        can_open_requests,
        egui::Button::new(pick(ctx.lang, "在 Requests 查看", "Open in Requests")),
    );
    if row.session_id.is_none() {
        open_requests = open_requests.on_disabled_hover_text(pick(
            ctx.lang,
            "当前会话没有 session_id。",
            "The current session has no session_id.",
        ));
    }
    if open_requests.clicked() {
        let Some(sid) = row.session_id.clone() else {
            return;
        };
        prepare_select_requests_for_session(&mut ctx.view.requests, sid);
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

pub(super) fn render_open_history_action(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    row: &SessionRow,
    host_local_session_features: bool,
) {
    let can_open_history = row.session_id.is_some();
    let mut open_history = ui.add_enabled(
        can_open_history,
        egui::Button::new(pick(ctx.lang, "在 History 查看", "Open in History")),
    );
    if row.session_id.is_none() {
        open_history = open_history.on_disabled_hover_text(pick(
            ctx.lang,
            "当前会话没有 session_id。",
            "The current session has no session_id.",
        ));
    }
    if open_history.clicked() {
        if row.session_id.is_none() {
            return;
        }
        start_open_session_row_in_history(ctx, row.clone(), host_local_session_features);
    }
}

pub(super) fn render_open_transcript_action(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    row: &SessionRow,
    host_local_session_features: bool,
) {
    let can_open_transcript = row.session_id.is_some() && host_local_session_features;
    let mut open_transcript = ui.add_enabled(
        can_open_transcript,
        egui::Button::new(pick(ctx.lang, "打开对话记录", "Open transcript")),
    );
    if row.session_id.is_none() {
        open_transcript = open_transcript.on_disabled_hover_text(pick(
            ctx.lang,
            "当前会话没有 session_id。",
            "The current session has no session_id.",
        ));
    } else if !host_local_session_features {
        open_transcript = open_transcript.on_disabled_hover_text(pick(
            ctx.lang,
            "当前附着的是远端代理；GUI 无法假设这台设备能直接读取远端 host 的 ~/.codex/sessions。",
            "A remote proxy is attached; the GUI cannot assume this device can directly read the remote host's ~/.codex/sessions.",
        ));
    }
    if open_transcript.clicked() {
        if row.session_id.is_none() {
            return;
        }
        start_open_session_row_transcript(ctx, row.clone());
    }
}

use super::*;

fn render_request_session_actions(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    sid: &str,
    request: &FinishedRequest,
) {
    if ui
        .button(pick(ctx.lang, "限定到此 session", "Focus this session"))
        .clicked()
    {
        prepare_select_requests_for_session(&mut ctx.view.requests, sid.to_string());
    }

    if ui
        .button(pick(ctx.lang, "在 Sessions 查看", "Open in Sessions"))
        .clicked()
    {
        focus_session_in_sessions(&mut ctx.view.sessions, sid.to_string());
        prepare_select_requests_for_session(&mut ctx.view.requests, sid.to_string());
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

    if ui
        .button(pick(ctx.lang, "在 History 查看", "Open in History"))
        .clicked()
    {
        start_open_request_in_history(ctx, request.clone());
    }
}

pub(super) fn render_request_header(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    request: &FinishedRequest,
) {
    ui.horizontal_wrapped(|ui| {
        ui.small(format!("id: {}", request.id));
        ui.small(format!(
            "{}: {}",
            pick(ctx.lang, "结束于", "Ended"),
            format_age(now_ms(), Some(request.ended_at_ms))
        ));
        if let Some(sid) = request.session_id.as_deref() {
            ui.small(format!("session: {sid}"));
        }
    });

    if let Some(cwd) = request.cwd.as_deref() {
        ui.small(format!("cwd: {cwd}"));
    }

    if let Some(sid) = request.session_id.as_deref() {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            render_request_session_actions(ui, ctx, sid, request);
        });
    }
}

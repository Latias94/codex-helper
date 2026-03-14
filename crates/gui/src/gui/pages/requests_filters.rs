use super::*;

pub(super) fn selected_requests_session_id(ctx: &PageCtx<'_>) -> Option<String> {
    ctx.view
        .requests
        .focused_session_id
        .clone()
        .or_else(|| ctx.view.sessions.selected_session_id.clone())
}

pub(super) fn render_requests_filters(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    selected_sid_ref: Option<&str>,
) {
    ui.horizontal(|ui| {
        ui.checkbox(
            &mut ctx.view.requests.scope_session,
            pick(ctx.lang, "跟随所选会话", "Scope to selected session"),
        );
        ui.checkbox(
            &mut ctx.view.requests.errors_only,
            pick(ctx.lang, "仅错误", "Errors only"),
        );
        if ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
            ctx.proxy
                .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
        }
    });

    if ctx.view.requests.scope_session {
        ui.horizontal_wrapped(|ui| {
            if let Some(sid) = selected_sid_ref {
                ui.small(format!("session: {sid}"));
                if ctx.view.requests.focused_session_id.is_some() {
                    ui.small(pick(ctx.lang, "（显式聚焦）", "(explicit focus)"));
                    if ui
                        .button(pick(ctx.lang, "改为跟随 Sessions", "Follow Sessions instead"))
                        .clicked()
                    {
                        ctx.view.requests.focused_session_id = None;
                    }
                } else {
                    ui.small(pick(ctx.lang, "（跟随 Sessions）", "(following Sessions)"));
                }
            } else {
                ui.small(pick(
                    ctx.lang,
                    "当前没有可用于限定的 session_id；显示全部请求。",
                    "No session_id is available for scoping right now; all requests remain visible.",
                ));
            }
        });
    }
}

pub(super) fn filtered_recent_requests<'a>(
    recent: &'a [FinishedRequest],
    ctx: &PageCtx<'_>,
    selected_sid_ref: Option<&str>,
) -> Vec<&'a FinishedRequest> {
    recent
        .iter()
        .filter(|request| {
            if ctx.view.requests.errors_only && request.status_code < 400 {
                return false;
            }
            if ctx.view.requests.scope_session {
                match (selected_sid_ref, request.session_id.as_deref()) {
                    (Some(sid), Some(request_sid)) => sid == request_sid,
                    (Some(_), None) => false,
                    (None, _) => true,
                }
            } else {
                true
            }
        })
        .take(600)
        .collect()
}

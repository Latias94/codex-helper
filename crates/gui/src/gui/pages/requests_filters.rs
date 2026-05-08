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
    let local_ledger_available = matches!(ctx.proxy.kind(), ProxyModeKind::Running);
    if !local_ledger_available {
        ctx.view.requests.include_local_ledger = false;
    }

    ui.horizontal(|ui| {
        ui.checkbox(
            &mut ctx.view.requests.scope_session,
            pick(ctx.lang, "跟随所选会话", "Scope to selected session"),
        );
        ui.checkbox(
            &mut ctx.view.requests.errors_only,
            pick(ctx.lang, "仅错误", "Errors only"),
        );
        ui.add_enabled_ui(local_ledger_available, |ui| {
            let changed = ui
                .checkbox(
                    &mut ctx.view.requests.include_local_ledger,
                    pick(ctx.lang, "本地日志", "Local log"),
                )
                .changed();
            if changed && ctx.view.requests.include_local_ledger {
                refresh_local_request_ledger(ctx);
            }
        });
        if ctx.view.requests.include_local_ledger {
            ui.label(pick(ctx.lang, "条数", "Limit"));
            let limit_response = ui.add(
                egui::DragValue::new(&mut ctx.view.requests.local_ledger_limit)
                    .range(20..=5000)
                    .speed(20),
            );
            if limit_response.changed() {
                ctx.view.requests.local_ledger_limit =
                    ctx.view.requests.local_ledger_limit.clamp(20, 5000);
            }
            if ui.button(pick(ctx.lang, "载入日志", "Load log")).clicked() {
                refresh_local_request_ledger(ctx);
            }
        }
        if ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
            ctx.proxy
                .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
            if ctx.view.requests.include_local_ledger {
                refresh_local_request_ledger(ctx);
            }
        }
    });

    if ctx.view.requests.include_local_ledger {
        render_local_ledger_status(ui, ctx);
    }

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

pub(super) fn ensure_local_request_ledger_loaded(ctx: &mut PageCtx<'_>) {
    if !ctx.view.requests.include_local_ledger {
        return;
    }
    if ctx.view.requests.local_ledger_loaded_at_ms.is_none()
        && ctx.view.requests.local_ledger_last_error.is_none()
    {
        refresh_local_request_ledger(ctx);
    }
}

fn refresh_local_request_ledger(ctx: &mut PageCtx<'_>) {
    let limit = ctx.view.requests.local_ledger_limit.clamp(20, 5000);
    ctx.view.requests.local_ledger_limit = limit;
    let path = crate::request_ledger::request_log_path();
    match crate::request_ledger::tail_finished_requests_from_log(&path, limit) {
        Ok(records) => {
            ctx.view.requests.local_ledger_records = records;
            ctx.view.requests.local_ledger_loaded_limit = limit;
            ctx.view.requests.local_ledger_loaded_at_ms = Some(now_ms());
            ctx.view.requests.local_ledger_last_error = None;
            ctx.view.requests.selected_idx = 0;
        }
        Err(err) => {
            ctx.view.requests.local_ledger_records.clear();
            ctx.view.requests.local_ledger_loaded_limit = limit;
            ctx.view.requests.local_ledger_loaded_at_ms = Some(now_ms());
            ctx.view.requests.local_ledger_last_error = Some(err.to_string());
            ctx.view.requests.selected_idx = 0;
        }
    }
}

fn render_local_ledger_status(ui: &mut egui::Ui, ctx: &PageCtx<'_>) {
    ui.horizontal_wrapped(|ui| {
        let path = crate::request_ledger::request_log_path();
        if let Some(error) = ctx.view.requests.local_ledger_last_error.as_deref() {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                format!(
                    "{}: {error}",
                    pick(ctx.lang, "本地日志错误", "Local log error")
                ),
            );
            return;
        }
        ui.small(format!(
            "{}: {} / {}  {}: {}",
            pick(ctx.lang, "本地日志", "Local log"),
            ctx.view.requests.local_ledger_records.len(),
            ctx.view.requests.local_ledger_loaded_limit,
            pick(ctx.lang, "路径", "path"),
            path.display()
        ));
    });
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

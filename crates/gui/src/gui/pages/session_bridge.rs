use super::view_state::HistoryOpenLoad;
use super::*;
use std::sync::mpsc::TryRecvError;

pub(super) fn session_history_bridge_summary(row: &SessionRow, lang: Language) -> String {
    let mut parts = vec![super::session_effective_route_inline_summary(row, lang)];
    if let Some(profile) = row.binding_profile_name.as_deref() {
        parts.push(format!("profile={profile}"));
    }
    if let Some(client) = super::format_observed_client_identity(
        row.last_client_name.as_deref(),
        row.last_client_addr.as_deref(),
    ) {
        parts.push(format!("client={client}"));
    }
    if let Some(status) = row.last_status {
        parts.push(format!("status={status}"));
    }
    if row.active_count > 0 {
        parts.push(format!("active={}", row.active_count));
    }
    format!(
        "{}: {}",
        pick(lang, "来自 Sessions", "From Sessions"),
        parts.join(", ")
    )
}

pub(super) fn session_history_summary_from_row(
    row: &SessionRow,
    path: Option<std::path::PathBuf>,
    lang: Language,
) -> Option<SessionSummary> {
    let sid = row.session_id.clone()?;
    let sort_hint_ms = row.last_ended_at_ms.or(row.active_started_at_ms_min);
    let updated_at = sort_hint_ms.map(|ms| format_age(now_ms(), Some(ms)));
    let turns = row.turns_total.unwrap_or(0).min(usize::MAX as u64) as usize;
    let source = if path.is_some() {
        SessionSummarySource::LocalFile
    } else {
        SessionSummarySource::ObservedOnly
    };
    Some(SessionSummary {
        id: sid,
        path: path.unwrap_or_default(),
        cwd: row.cwd.clone(),
        created_at: None,
        updated_at: updated_at.clone(),
        last_response_at: updated_at,
        user_turns: turns,
        assistant_turns: turns,
        rounds: turns,
        first_user_message: Some(session_history_bridge_summary(row, lang)),
        source,
        sort_hint_ms,
    })
}

pub(super) fn host_transcript_path_from_session_row(
    row: &SessionRow,
) -> Option<std::path::PathBuf> {
    row.host_local_transcript_path
        .as_deref()
        .map(std::path::PathBuf::from)
}

pub(super) fn request_history_bridge_summary(request: &FinishedRequest, lang: Language) -> String {
    let mut parts = vec![
        format!(
            "station={}",
            request.station_name.as_deref().unwrap_or("auto")
        ),
        format!("model={}", request.model.as_deref().unwrap_or("auto")),
        format!("tier={}", request.service_tier.as_deref().unwrap_or("auto")),
    ];
    if let Some(provider) = request.provider_id.as_deref() {
        parts.push(format!("provider={provider}"));
    }
    if let Some(client) = super::format_observed_client_identity(
        request.client_name.as_deref(),
        request.client_addr.as_deref(),
    ) {
        parts.push(format!("client={client}"));
    }
    parts.push(format!("status={}", request.status_code));
    parts.push(format!("path={}", request.path));
    format!(
        "{}: {}",
        pick(lang, "来自 Requests", "From Requests"),
        parts.join(", ")
    )
}

pub(super) fn request_history_summary_from_request(
    request: &FinishedRequest,
    path: Option<std::path::PathBuf>,
    lang: Language,
) -> Option<SessionSummary> {
    let sid = request.session_id.clone()?;
    let updated_at = Some(format_age(now_ms(), Some(request.ended_at_ms)));
    let turns = 1usize;
    let source = if path.is_some() {
        SessionSummarySource::LocalFile
    } else {
        SessionSummarySource::ObservedOnly
    };
    Some(SessionSummary {
        id: sid,
        path: path.unwrap_or_default(),
        cwd: request.cwd.clone(),
        created_at: None,
        updated_at: updated_at.clone(),
        last_response_at: updated_at,
        user_turns: turns,
        assistant_turns: turns,
        rounds: turns,
        first_user_message: Some(request_history_bridge_summary(request, lang)),
        source,
        sort_hint_ms: Some(request.ended_at_ms),
    })
}

fn cancel_history_open_load(view: &mut ViewState) {
    if let Some(load) = view.history_open_load.take() {
        load.join.abort();
    }
}

fn history_open_info(lang: Language, summary: &SessionSummary) -> String {
    if matches!(summary.source, SessionSummarySource::LocalFile) {
        pick(
            lang,
            "已切到 History（本地 transcript）",
            "Opened in History (local transcript)",
        )
    } else {
        pick(
            lang,
            "已切到 History（共享观测摘要）",
            "Opened in History (observed summary)",
        )
    }
    .to_string()
}

pub(super) fn poll_history_open_loader(ctx: &mut PageCtx<'_>) {
    let Some(load) = ctx.view.history_open_load.as_mut() else {
        return;
    };
    match load.rx.try_recv() {
        Ok((seq, res)) => {
            if seq != load.seq {
                ctx.view.history_open_load = None;
                return;
            }

            let origin = load.origin;
            let require_local = load.require_local;
            ctx.view.history_open_load = None;
            match res {
                Ok(Some(summary)) => {
                    if require_local && !matches!(summary.source, SessionSummarySource::LocalFile) {
                        *ctx.last_error = Some(
                            pick(
                                ctx.lang,
                                "未找到该 session_id 的本地 Codex 会话文件（~/.codex/sessions）。",
                                "No local Codex session file found for this session_id (~/.codex/sessions).",
                            )
                            .to_string(),
                        );
                        return;
                    }
                    let info = history_open_info(ctx.lang, &summary);
                    super::history::prepare_select_session_from_external(
                        &mut ctx.view.history,
                        summary,
                        origin,
                    );
                    ctx.view.requested_page = Some(Page::History);
                    *ctx.last_info = Some(info);
                    *ctx.last_error = None;
                }
                Ok(None) => {
                    *ctx.last_error = Some(
                        pick(
                            ctx.lang,
                            "无法为该 session 构建 History 入口。",
                            "Could not build a History entry for this session.",
                        )
                        .to_string(),
                    );
                }
                Err(error) => {
                    *ctx.last_error = Some(format!("find session file failed: {error}"));
                }
            }
        }
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => {
            ctx.view.history_open_load = None;
        }
    }
}

fn start_history_open_load(
    ctx: &mut PageCtx<'_>,
    origin: super::history_external::ExternalHistoryOrigin,
    require_local: bool,
    rx: std::sync::mpsc::Receiver<(u64, anyhow::Result<Option<SessionSummary>>)>,
    join: tokio::task::JoinHandle<()>,
    seq: u64,
) {
    cancel_history_open_load(ctx.view);
    ctx.view.history_open_load = Some(HistoryOpenLoad {
        seq,
        origin,
        require_local,
        rx,
        join,
    });
    *ctx.last_info = Some(
        pick(
            ctx.lang,
            "正在定位 History 会话...",
            "Locating History session...",
        )
        .to_string(),
    );
    *ctx.last_error = None;
}

pub(super) fn start_open_request_in_history(ctx: &mut PageCtx<'_>, request: FinishedRequest) {
    let Some(sid) = request.session_id.clone() else {
        return;
    };
    cancel_history_open_load(ctx.view);
    ctx.view.history_open_seq = ctx.view.history_open_seq.saturating_add(1);
    let seq = ctx.view.history_open_seq;
    let lang = ctx.lang;
    let (tx, rx) = std::sync::mpsc::channel();
    let join = ctx.rt.spawn(async move {
        let result = crate::sessions::find_codex_session_file_by_id(&sid)
            .await
            .map(|path| request_history_summary_from_request(&request, path, lang));
        let _ = tx.send((seq, result));
    });
    start_history_open_load(
        ctx,
        super::history_external::ExternalHistoryOrigin::Requests,
        false,
        rx,
        join,
        seq,
    );
}

pub(super) fn start_open_session_row_in_history(
    ctx: &mut PageCtx<'_>,
    row: SessionRow,
    host_local_session_features: bool,
) {
    let Some(sid) = row.session_id.clone() else {
        return;
    };
    cancel_history_open_load(ctx.view);
    ctx.view.history_open_seq = ctx.view.history_open_seq.saturating_add(1);
    let seq = ctx.view.history_open_seq;
    let lang = ctx.lang;
    let (tx, rx) = std::sync::mpsc::channel();
    let join = ctx.rt.spawn(async move {
        let result = if host_local_session_features {
            Ok(host_transcript_path_from_session_row(&row))
        } else {
            crate::sessions::find_codex_session_file_by_id(&sid).await
        }
        .map(|path| session_history_summary_from_row(&row, path, lang));
        let _ = tx.send((seq, result));
    });
    start_history_open_load(
        ctx,
        super::history_external::ExternalHistoryOrigin::Sessions,
        false,
        rx,
        join,
        seq,
    );
}

pub(super) fn start_open_session_row_transcript(ctx: &mut PageCtx<'_>, row: SessionRow) {
    let Some(sid) = row.session_id.clone() else {
        return;
    };
    if let Some(path) = host_transcript_path_from_session_row(&row) {
        if let Some(summary) = session_history_summary_from_row(&row, Some(path), ctx.lang) {
            super::history::prepare_select_session_from_external(
                &mut ctx.view.history,
                summary,
                super::history_external::ExternalHistoryOrigin::Sessions,
            );
            ctx.view.requested_page = Some(Page::History);
        }
        return;
    }

    cancel_history_open_load(ctx.view);
    ctx.view.history_open_seq = ctx.view.history_open_seq.saturating_add(1);
    let seq = ctx.view.history_open_seq;
    let lang = ctx.lang;
    let (tx, rx) = std::sync::mpsc::channel();
    let join = ctx.rt.spawn(async move {
        let result = crate::sessions::find_codex_session_file_by_id(&sid)
            .await
            .map(|path| session_history_summary_from_row(&row, path, lang));
        let _ = tx.send((seq, result));
    });
    start_history_open_load(
        ctx,
        super::history_external::ExternalHistoryOrigin::Sessions,
        true,
        rx,
        join,
        seq,
    );
}

pub(super) fn focus_session_in_sessions(state: &mut SessionsViewState, sid: String) {
    state.active_only = false;
    state.errors_only = false;
    state.overrides_only = false;
    state.search = sid.clone();
    state.selected_session_id = Some(sid);
    state.selected_idx = 0;
}

pub(super) fn prepare_select_requests_for_session(state: &mut RequestsViewState, sid: String) {
    state.errors_only = false;
    state.scope_session = true;
    state.focused_session_id = Some(sid);
    state.selected_idx = 0;
}

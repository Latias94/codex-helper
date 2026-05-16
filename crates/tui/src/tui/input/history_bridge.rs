use std::path::PathBuf;

use crate::sessions::{SessionSummary, SessionSummarySource};
use crate::state::FinishedRequest;
use crate::tui::model::{
    SessionRow, Snapshot, codex_recent_window_threshold_ms, format_age, now_ms,
    request_matches_page_filters, request_page_focus_session_id,
};
use crate::tui::state::{CodexHistoryExternalFocusOrigin, RecentCodexRow, UiState};
use crate::tui::types::{Focus, Page};

pub(super) fn selected_request_page_request<'a>(
    snapshot: &'a Snapshot,
    ui: &UiState,
) -> Option<&'a FinishedRequest> {
    let focused_sid = request_page_focus_session_id(
        snapshot,
        ui.focused_request_session_id.as_deref(),
        ui.selected_session_idx,
    );

    snapshot
        .recent
        .iter()
        .filter(|request| {
            request_matches_page_filters(
                request,
                ui.request_page_errors_only,
                ui.request_page_scope_session,
                focused_sid.as_deref(),
            )
        })
        .nth(ui.selected_request_page_idx)
}

pub(super) fn selected_dashboard_request<'a>(
    snapshot: &'a Snapshot,
    ui: &UiState,
) -> Option<&'a FinishedRequest> {
    let selected_sid = snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|row| row.session_id.as_deref());

    snapshot
        .recent
        .iter()
        .filter(
            |request| match (selected_sid, request.session_id.as_deref()) {
                (Some(sid), Some(request_sid)) => sid == request_sid,
                (Some(_), None) => false,
                (None, _) => true,
            },
        )
        .take(60)
        .nth(ui.selected_request_idx)
}

pub(super) fn selected_recent_row(ui: &UiState) -> Option<RecentCodexRow> {
    let now = now_ms();
    let threshold_ms = codex_recent_window_threshold_ms(now, ui.codex_recent_window_idx);
    ui.codex_recent_rows
        .iter()
        .filter(|row| row.mtime_ms >= threshold_ms)
        .nth(ui.codex_recent_selected_idx)
        .cloned()
}

fn session_history_bridge_summary(row: &SessionRow) -> String {
    let mut parts = vec![
        format!(
            "station={}",
            row.effective_station
                .as_ref()
                .map(|value| value.value.as_str())
                .or(row.last_station_name.as_deref())
                .unwrap_or("auto")
        ),
        format!(
            "model={}",
            row.effective_model
                .as_ref()
                .map(|value| value.value.as_str())
                .or(row.last_model.as_deref())
                .unwrap_or("auto")
        ),
        format!(
            "tier={}",
            row.effective_service_tier
                .as_ref()
                .map(|value| value.value.as_str())
                .or(row.last_service_tier.as_deref())
                .unwrap_or("auto")
        ),
    ];
    if let Some(provider) = row.last_provider_id.as_deref() {
        parts.push(format!("provider={provider}"));
    }
    if let Some(status) = row.last_status {
        parts.push(format!("status={status}"));
    }
    format!("From Sessions: {}", parts.join(", "))
}

fn request_history_bridge_summary(request: &FinishedRequest) -> String {
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
    parts.push(format!("status={}", request.status_code));
    parts.push(format!("path={}", request.path));
    format!("From Requests: {}", parts.join(", "))
}

pub(super) fn session_history_summary_from_row(
    row: &SessionRow,
    path: Option<PathBuf>,
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
        first_user_message: Some(session_history_bridge_summary(row)),
        source,
        sort_hint_ms,
    })
}

pub(super) fn host_transcript_path_from_row(row: &SessionRow) -> Option<PathBuf> {
    row.host_local_transcript_path.as_deref().map(PathBuf::from)
}

fn recent_history_bridge_summary(row: &RecentCodexRow) -> String {
    let mut parts = vec![format!("root={}", row.root)];
    if let Some(branch) = row.branch.as_deref() {
        parts.push(format!("branch={branch}"));
    }
    if let Some(cwd) = row.cwd.as_deref() {
        parts.push(format!("cwd={cwd}"));
    }
    format!("From Recent: {}", parts.join(", "))
}

pub(super) fn recent_history_summary_from_row(
    row: &RecentCodexRow,
    path: Option<PathBuf>,
) -> SessionSummary {
    let updated_at = Some(format_age(now_ms(), Some(row.mtime_ms)));
    let source = if path.is_some() {
        SessionSummarySource::LocalFile
    } else {
        SessionSummarySource::ObservedOnly
    };
    SessionSummary {
        id: row.session_id.clone(),
        path: path.unwrap_or_default(),
        cwd: row.cwd.clone(),
        created_at: None,
        updated_at: updated_at.clone(),
        last_response_at: updated_at,
        user_turns: 0,
        assistant_turns: 0,
        rounds: 0,
        first_user_message: Some(recent_history_bridge_summary(row)),
        source,
        sort_hint_ms: Some(row.mtime_ms),
    }
}

pub(super) fn request_history_summary_from_request(
    request: &FinishedRequest,
    path: Option<PathBuf>,
) -> Option<SessionSummary> {
    let sid = request.session_id.clone()?;
    let updated_at = Some(format_age(now_ms(), Some(request.ended_at_ms)));
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
        user_turns: 1,
        assistant_turns: 1,
        rounds: 1,
        first_user_message: Some(request_history_bridge_summary(request)),
        source,
        sort_hint_ms: Some(request.ended_at_ms),
    })
}

pub(super) fn prepare_select_history_from_external(
    ui: &mut UiState,
    summary: SessionSummary,
    origin: CodexHistoryExternalFocusOrigin,
) {
    ui.page = Page::History;
    ui.focus = Focus::Sessions;
    ui.prepare_codex_history_external_focus(summary, origin);
    ui.needs_codex_history_refresh = true;
}

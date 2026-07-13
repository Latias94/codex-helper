use std::path::PathBuf;

use crate::dashboard_core::OperatorRequestSummary;
use crate::sessions::{SessionSummary, SessionSummarySource};
use crate::tui::model::{SessionRow, Snapshot, format_age, now_ms};
use crate::tui::state::{CodexHistoryExternalFocusOrigin, RecentCodexRow, UiState};
use crate::tui::types::{Focus, Page};

pub(super) fn selected_request_page_request<'a>(
    snapshot: &'a Snapshot,
    ui: &UiState,
) -> Option<&'a OperatorRequestSummary> {
    ui.request_page_filtered_indices(snapshot)
        .get(ui.selected_request_page_idx)
        .and_then(|idx| snapshot.recent.get(*idx))
}

pub(super) fn selected_dashboard_request<'a>(
    snapshot: &'a Snapshot,
    ui: &UiState,
) -> Option<&'a OperatorRequestSummary> {
    let selected_sid = snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|row| row.session_id.as_deref());

    snapshot
        .recent
        .iter()
        .filter(
            |request| match (selected_sid, request.session_key.as_deref()) {
                (Some(sid), Some(request_sid)) => sid == request_sid,
                (Some(_), None) => false,
                (None, _) => true,
            },
        )
        .take(60)
        .nth(ui.selected_request_idx)
}

pub(super) fn local_session_id_for_opaque_key<'a>(
    snapshot: &'a Snapshot,
    opaque_session_key: &str,
) -> Option<&'a str> {
    snapshot
        .rows
        .iter()
        .find(|row| row.session_id.as_deref() == Some(opaque_session_key))
        .and_then(SessionRow::local_command_session_id)
}

pub(super) fn selected_recent_row(ui: &UiState) -> Option<RecentCodexRow> {
    let now = now_ms();
    ui.codex_recent_visible_indices(now)
        .get(ui.codex_recent_selected_idx)
        .and_then(|idx| ui.codex_recent_rows.get(*idx))
        .cloned()
}

fn session_history_bridge_summary(row: &SessionRow) -> String {
    let mut parts = vec![
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
    if let Some(provider) = row.observed_provider_id() {
        parts.push(format!("provider={provider}"));
    }
    if let Some(endpoint) = row.observed_endpoint_id() {
        parts.push(format!("endpoint={endpoint}"));
    }
    if let Some(status) = row.last_status {
        parts.push(format!("status={status}"));
    }
    format!("From Sessions: {}", parts.join(", "))
}

fn request_history_bridge_summary(request: &OperatorRequestSummary) -> String {
    let mut parts = vec![
        format!("model={}", request.model.as_deref().unwrap_or("auto")),
        format!("tier={}", request.service_tier.as_deref().unwrap_or("auto")),
    ];
    if let Some(provider) = request.provider_id.as_deref() {
        parts.push(format!("provider={provider}"));
    }
    if let Some(endpoint) = request.endpoint_id.as_deref() {
        parts.push(format!("endpoint={endpoint}"));
    }
    parts.push(format!("status={}", request.status_code));
    parts.push(format!("path={}", request.path));
    format!("From Requests: {}", parts.join(", "))
}

pub(super) fn session_history_summary_from_row(
    row: &SessionRow,
    path: Option<PathBuf>,
) -> Option<SessionSummary> {
    let sid = row.local_command_session_id()?.to_string();
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
    request: &OperatorRequestSummary,
    local_session_id: &str,
    path: Option<PathBuf>,
) -> SessionSummary {
    let updated_at = Some(format_age(now_ms(), Some(request.ended_at_ms)));
    let source = if path.is_some() {
        SessionSummarySource::LocalFile
    } else {
        SessionSummarySource::ObservedOnly
    };
    SessionSummary {
        id: local_session_id.to_string(),
        path: path.unwrap_or_default(),
        cwd: None,
        created_at: None,
        updated_at: updated_at.clone(),
        last_response_at: updated_at,
        user_turns: 1,
        assistant_turns: 1,
        rounds: 1,
        first_user_message: Some(request_history_bridge_summary(request)),
        source,
        sort_hint_ms: Some(request.ended_at_ms),
    }
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

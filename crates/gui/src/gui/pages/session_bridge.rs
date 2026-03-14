use super::*;

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

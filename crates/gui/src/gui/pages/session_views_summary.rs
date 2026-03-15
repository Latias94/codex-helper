use super::*;

pub(super) fn session_row_matches_query(row: &SessionRow, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    for value in [
        row.session_id.as_deref(),
        row.last_client_name.as_deref(),
        row.last_client_addr.as_deref(),
        row.cwd.as_deref(),
        row.last_model.as_deref(),
        row.last_service_tier.as_deref(),
        row.last_provider_id.as_deref(),
        row.last_station_name(),
        row.last_upstream_base_url.as_deref(),
        row.binding_profile_name.as_deref(),
        row.effective_model.as_ref().map(|v| v.value.as_str()),
        row.effective_reasoning_effort
            .as_ref()
            .map(|v| v.value.as_str()),
        row.effective_service_tier
            .as_ref()
            .map(|v| v.value.as_str()),
        row.effective_station().map(|v| v.value.as_str()),
        row.effective_upstream_base_url
            .as_ref()
            .map(|v| v.value.as_str()),
        row.override_model.as_deref(),
        row.override_effort.as_deref(),
        row.override_station_name(),
        row.override_service_tier.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if value.to_lowercase().contains(q) {
            return true;
        }
    }
    false
}

pub(super) fn session_effective_route_inline_summary(row: &SessionRow, lang: Language) -> String {
    format!(
        "station={}, model={}, reasoning={}, service_tier={}",
        session_route_preview_value(row.effective_station(), row.last_station_name(), lang),
        session_route_preview_value(
            row.effective_model.as_ref(),
            row.last_model.as_deref(),
            lang
        ),
        session_route_preview_value(
            row.effective_reasoning_effort.as_ref(),
            row.last_reasoning_effort.as_deref(),
            lang,
        ),
        session_route_preview_value(
            row.effective_service_tier.as_ref(),
            row.last_service_tier.as_deref(),
            lang,
        )
    )
}

pub(super) fn session_last_activity_summary(row: &SessionRow) -> String {
    let status = row
        .last_status
        .map(|status| status.to_string())
        .unwrap_or_else(|| "-".to_string());
    let duration = row
        .last_duration_ms
        .map(|duration| format!("{duration} ms"))
        .unwrap_or_else(|| "-".to_string());
    let last = format_age(now_ms(), row.last_ended_at_ms);
    format!("status={status}, duration={duration}, last={last}")
}

pub(super) fn session_list_control_label(row: &SessionRow) -> String {
    if let Some(profile_name) = row.binding_profile_name.as_deref() {
        return format!("pf:{}", shorten(profile_name, 10));
    }
    if let Some(station_name) = row.override_station_name() {
        return format!("pin:{}", shorten(station_name, 10));
    }
    let override_count = usize::from(row.override_model.is_some())
        + usize::from(row.override_effort.is_some())
        + usize::from(row.override_service_tier.is_some());
    if override_count > 0 {
        return format!("ovr:{override_count}");
    }
    if row.effective_station_source() == Some(RouteValueSource::GlobalOverride) {
        return "global".to_string();
    }
    "-".to_string()
}

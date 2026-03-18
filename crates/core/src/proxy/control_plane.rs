use axum::http::StatusCode;

mod capabilities;
mod session_mutations;
mod session_observability;

pub(super) use self::capabilities::{api_capabilities, api_operator_summary, api_v1_snapshot};
pub(super) use self::session_mutations::{
    apply_session_profile, get_global_station_override, set_default_profile,
    set_global_station_override,
};
pub(super) use self::session_observability::{
    get_session_identity_card, list_active_requests, list_recent_finished,
    list_session_identity_cards, list_session_stats,
};

#[derive(serde::Deserialize)]
pub(in crate::proxy) struct SessionProfileApplyRequest {
    session_id: String,
    profile_name: Option<String>,
}

#[derive(serde::Deserialize)]
pub(in crate::proxy) struct DefaultProfileRequest {
    profile_name: Option<String>,
}

#[derive(serde::Deserialize)]
pub(in crate::proxy) struct GlobalStationOverrideRequest {
    #[serde(default)]
    station_name: Option<String>,
}

#[derive(serde::Deserialize)]
pub(in crate::proxy) struct RecentQuery {
    limit: Option<usize>,
}

#[derive(serde::Deserialize)]
pub(in crate::proxy) struct SnapshotQuery {
    recent_limit: Option<usize>,
    stats_days: Option<usize>,
}

pub(super) fn require_session_id(session_id: &str) -> Result<(), (StatusCode, String)> {
    if session_id.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "session_id is required".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn host_local_session_history_available() -> bool {
    let sessions_dir = crate::config::codex_sessions_dir();
    std::fs::metadata(sessions_dir)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

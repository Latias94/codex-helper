use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::stream::{FuturesUnordered, StreamExt};
use reqwest::Url;
use tokio::sync::{OnceCell, Semaphore};

use crate::config::{
    UpstreamConfig,
    auth_sync::{SyncCodexAuthFromCodexOptions, sync_codex_auth_from_codex_cli},
    bootstrap::overwrite_codex_config_from_codex_cli_in_place,
    proxy_home_dir,
    storage::{load_config, save_config},
};
use crate::dashboard_core::{ControlProfileOption, build_model_options_from_mgr};
use crate::sessions::{
    SessionSummary, SessionSummarySource, find_codex_session_file_by_id, read_codex_session_meta,
    read_codex_session_transcript,
};
use crate::state::{FinishedRequest, ProxyState, StationHealth, UpstreamHealth};

use super::Language;
use super::model::{
    CODEX_RECENT_WINDOWS, ProviderOption, SessionRow, Snapshot, codex_recent_window_label,
    codex_recent_window_threshold_ms, filtered_request_page_len, filtered_requests_len,
    find_session_idx, format_age, now_ms, request_matches_page_filters,
    request_page_focus_session_id, session_row_has_any_override, short_sid,
};
use super::report::build_stats_report;
use super::state::{
    CodexHistoryExternalFocusOrigin, RecentCodexRow, UiState, adjust_table_selection,
};
use super::types::{EffortChoice, Focus, Overlay, Page, ServiceTierChoice, StatsFocus};

#[derive(Debug, serde::Deserialize)]
struct ProfileControlResponse {
    configured_default_profile: Option<String>,
    default_profile: Option<String>,
    profiles: Vec<ControlProfileOption>,
}

pub(in crate::tui) fn should_accept_key_event(event: &KeyEvent) -> bool {
    matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

pub(in crate::tui) async fn handle_key_event(
    state: Arc<ProxyState>,
    providers: &mut Vec<ProviderOption>,
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    if ui.overlay == Overlay::None && apply_page_shortcuts(ui, key.code) {
        return true;
    }

    match ui.overlay {
        Overlay::None => handle_key_normal(&state, providers, ui, snapshot, key).await,
        Overlay::Help => match key.code {
            KeyCode::Esc | KeyCode::Char('?') => {
                ui.overlay = Overlay::None;
                true
            }
            KeyCode::Char('L') => {
                toggle_language(ui).await;
                true
            }
            _ => false,
        },
        Overlay::SessionTranscript => match key.code {
            KeyCode::Esc | KeyCode::Char('t') => {
                ui.overlay = Overlay::None;
                true
            }
            KeyCode::Char('A') | KeyCode::Char('a') => {
                let Some(file) = ui.session_transcript_file.as_deref() else {
                    ui.toast = Some(("no transcript file loaded".to_string(), Instant::now()));
                    return true;
                };

                ui.session_transcript_tail = match ui.session_transcript_tail {
                    Some(_) => None,
                    None => Some(80),
                };
                ui.session_transcript_messages.clear();
                ui.session_transcript_scroll = u16::MAX;
                ui.session_transcript_error = None;

                let path = PathBuf::from(file);
                match read_codex_session_transcript(&path, ui.session_transcript_tail).await {
                    Ok(msgs) => {
                        ui.session_transcript_messages = msgs;
                        ui.toast = Some((
                            match ui.session_transcript_tail {
                                Some(n) => format!("transcript: loaded tail {n}"),
                                None => "transcript: loaded all".to_string(),
                            },
                            Instant::now(),
                        ));
                    }
                    Err(e) => {
                        ui.session_transcript_error = Some(e.to_string());
                        ui.toast =
                            Some((format!("transcript: reload failed: {e}"), Instant::now()));
                    }
                }
                true
            }
            KeyCode::Char('y') => {
                let text = format_session_transcript_text(ui);
                match try_copy_to_clipboard(&text) {
                    Ok(()) => {
                        ui.toast = Some((
                            "transcript: copied to clipboard".to_string(),
                            Instant::now(),
                        ))
                    }
                    Err(e) => {
                        ui.toast = Some((format!("transcript: copy failed: {e}"), Instant::now()))
                    }
                }
                true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                ui.session_transcript_scroll = ui.session_transcript_scroll.saturating_sub(1);
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                ui.session_transcript_scroll = ui.session_transcript_scroll.saturating_add(1);
                true
            }
            KeyCode::PageUp => {
                ui.session_transcript_scroll = ui.session_transcript_scroll.saturating_sub(10);
                true
            }
            KeyCode::PageDown => {
                ui.session_transcript_scroll = ui.session_transcript_scroll.saturating_add(10);
                true
            }
            KeyCode::Home | KeyCode::Char('g') => {
                ui.session_transcript_scroll = 0;
                true
            }
            KeyCode::End | KeyCode::Char('G') => {
                ui.session_transcript_scroll = u16::MAX;
                true
            }
            KeyCode::Char('L') => {
                toggle_language(ui).await;
                true
            }
            _ => false,
        },
        Overlay::StationInfo => match key.code {
            KeyCode::Esc | KeyCode::Char('i') => {
                ui.overlay = Overlay::None;
                true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                ui.station_info_scroll = ui.station_info_scroll.saturating_sub(1);
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                ui.station_info_scroll = ui.station_info_scroll.saturating_add(1);
                true
            }
            KeyCode::PageUp => {
                ui.station_info_scroll = ui.station_info_scroll.saturating_sub(10);
                true
            }
            KeyCode::PageDown => {
                ui.station_info_scroll = ui.station_info_scroll.saturating_add(10);
                true
            }
            KeyCode::Home | KeyCode::Char('g') => {
                ui.station_info_scroll = 0;
                true
            }
            KeyCode::End | KeyCode::Char('G') => {
                ui.station_info_scroll = u16::MAX;
                true
            }
            KeyCode::Char('L') => {
                toggle_language(ui).await;
                true
            }
            _ => false,
        },
        Overlay::EffortMenu => handle_key_effort_menu(&state, ui, snapshot, key).await,
        Overlay::ModelMenuSession => handle_key_model_menu(&state, ui, snapshot, key).await,
        Overlay::ModelInputSession => handle_key_model_input(&state, ui, snapshot, key).await,
        Overlay::ServiceTierMenuSession => {
            handle_key_service_tier_menu(&state, ui, snapshot, key).await
        }
        Overlay::ServiceTierInputSession => {
            handle_key_service_tier_input(&state, ui, snapshot, key).await
        }
        Overlay::ProfileMenuSession
        | Overlay::ProfileMenuDefaultRuntime
        | Overlay::ProfileMenuDefaultPersisted => {
            handle_key_profile_menu(&state, ui, snapshot, key).await
        }
        Overlay::ProviderMenuSession | Overlay::ProviderMenuGlobal => {
            handle_key_provider_menu(&state, providers, ui, snapshot, key).await
        }
    }
}

fn apply_page_shortcuts(ui: &mut UiState, code: KeyCode) -> bool {
    let page = match code {
        KeyCode::Char('1') => Some(Page::Dashboard),
        KeyCode::Char('2') => Some(Page::Stations),
        KeyCode::Char('3') => Some(Page::Sessions),
        KeyCode::Char('4') => Some(Page::Requests),
        KeyCode::Char('5') => Some(Page::Stats),
        KeyCode::Char('6') => Some(Page::Settings),
        KeyCode::Char('7') => Some(Page::History),
        KeyCode::Char('8') => Some(Page::Recent),
        _ => None,
    };
    if let Some(p) = page {
        ui.page = p;
        if ui.page == Page::Stations {
            ui.focus = Focus::Stations;
        } else if ui.page == Page::Requests {
            ui.focus = Focus::Requests;
        } else if ui.page == Page::Sessions
            || ui.page == Page::History
            || ui.page == Page::Recent
            || (ui.page == Page::Dashboard && ui.focus == Focus::Stations)
        {
            ui.focus = Focus::Sessions;
        }
        if ui.page == Page::History {
            ui.needs_codex_history_refresh = true;
            ui.sync_codex_history_selection();
        }
        if ui.page == Page::Recent {
            ui.needs_codex_recent_refresh = true;
            ui.codex_recent_selected_idx = 0;
            ui.codex_recent_selected_id = None;
            ui.codex_recent_table.select(None);
        }
        return true;
    }
    false
}

fn apply_selected_session(ui: &mut UiState, snapshot: &Snapshot, idx: usize) {
    ui.selected_session_idx = idx.min(snapshot.rows.len().saturating_sub(1));
    ui.selected_session_id = snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|r| r.session_id.clone());

    ui.sessions_table.select(if snapshot.rows.is_empty() {
        None
    } else {
        Some(ui.selected_session_idx)
    });

    ui.selected_request_idx = 0;
    let req_len = filtered_requests_len(snapshot, ui.selected_session_idx);
    ui.requests_table
        .select(if req_len == 0 { None } else { Some(0) });
}

pub(in crate::tui) async fn refresh_profile_control_state(ui: &mut UiState) -> anyhow::Result<()> {
    let response = reqwest::Client::new()
        .get(format!(
            "http://127.0.0.1:{}/__codex_helper/api/v1/profiles",
            ui.admin_port
        ))
        .timeout(Duration::from_millis(1200))
        .send()
        .await?
        .error_for_status()?
        .json::<ProfileControlResponse>()
        .await?;

    ui.configured_default_profile = response.configured_default_profile.clone();
    ui.effective_default_profile = response.default_profile.clone();
    ui.runtime_default_profile_override =
        if response.default_profile != response.configured_default_profile {
            response.default_profile.clone()
        } else {
            None
        };
    ui.profile_options = response.profiles;
    Ok(())
}

fn default_profile_menu_idx(
    profiles: &[ControlProfileOption],
    binding_profile_name: Option<&str>,
) -> usize {
    match binding_profile_name {
        Some(name) => profiles
            .iter()
            .position(|profile| profile.name == name)
            .map(|idx| idx + 1)
            .unwrap_or(0),
        None => usize::from(!profiles.is_empty()),
    }
}

fn runtime_default_profile_menu_idx(
    profiles: &[ControlProfileOption],
    runtime_default_profile_override: Option<&str>,
) -> usize {
    match runtime_default_profile_override {
        Some(name) => default_profile_menu_idx(profiles, Some(name)),
        None => 0,
    }
}

async fn apply_runtime_default_profile(
    ui: &UiState,
    profile_name: Option<String>,
) -> anyhow::Result<()> {
    reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/__codex_helper/api/v1/profiles/default",
            ui.admin_port
        ))
        .timeout(Duration::from_millis(1200))
        .json(&serde_json::json!({
            "profile_name": profile_name,
        }))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

async fn apply_persisted_default_profile(
    ui: &UiState,
    profile_name: Option<String>,
) -> anyhow::Result<()> {
    reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/__codex_helper/api/v1/profiles/default/persisted",
            ui.admin_port
        ))
        .timeout(Duration::from_millis(1200))
        .json(&serde_json::json!({
            "profile_name": profile_name,
        }))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

fn default_profile_label(value: Option<&str>, fallback: &str) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn profile_menu_max_idx(profiles: &[ControlProfileOption]) -> usize {
    profiles.len()
}

async fn load_model_options_for_service(service_name: &str) -> anyhow::Result<Vec<String>> {
    let cfg = load_config().await?;
    let mgr = match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    Ok(build_model_options_from_mgr(mgr))
}

fn selected_session_model_hint(snapshot: &Snapshot, ui: &UiState) -> Option<String> {
    snapshot.rows.get(ui.selected_session_idx).and_then(|row| {
        row.override_model
            .as_deref()
            .or(row
                .effective_model
                .as_ref()
                .map(|value| value.value.as_str()))
            .or(row.last_model.as_deref())
            .map(ToString::to_string)
    })
}

fn add_model_option_if_missing(options: &mut Vec<String>, model: Option<&str>) {
    let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) else {
        return;
    };
    if options.iter().all(|existing| existing != model) {
        options.push(model.to_string());
        options.sort();
    }
}

fn current_model_override(snapshot: &Snapshot, ui: &UiState) -> Option<String> {
    snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|row| row.override_model.clone())
}

fn selected_session_service_tier_hint(snapshot: &Snapshot, ui: &UiState) -> Option<String> {
    snapshot.rows.get(ui.selected_session_idx).and_then(|row| {
        row.override_service_tier
            .as_deref()
            .or(row
                .effective_service_tier
                .as_ref()
                .map(|value| value.value.as_str()))
            .or(row.last_service_tier.as_deref())
            .map(ToString::to_string)
    })
}

fn current_service_tier_override(snapshot: &Snapshot, ui: &UiState) -> Option<String> {
    snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|row| row.override_service_tier.clone())
}

fn focus_session_in_sessions(ui: &mut UiState, snapshot: &Snapshot, sid: &str) -> bool {
    let Some(idx) = find_session_idx(snapshot, sid) else {
        return false;
    };

    ui.sessions_page_active_only = false;
    ui.sessions_page_errors_only = false;
    ui.sessions_page_overrides_only = false;
    ui.selected_sessions_page_idx = 0;
    ui.page = Page::Sessions;
    ui.focus = Focus::Sessions;
    apply_selected_session(ui, snapshot, idx);
    true
}

fn prepare_select_requests_for_session(ui: &mut UiState, sid: String) {
    ui.page = Page::Requests;
    ui.focus = Focus::Requests;
    ui.request_page_errors_only = false;
    ui.request_page_scope_session = true;
    ui.focused_request_session_id = Some(sid);
    ui.selected_request_page_idx = 0;
}

fn clear_request_page_focus(ui: &mut UiState) {
    ui.focused_request_session_id = None;
    ui.selected_request_page_idx = 0;
}

fn selected_request_page_request<'a>(
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

fn selected_dashboard_request<'a>(
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

fn selected_recent_row(ui: &UiState) -> Option<RecentCodexRow> {
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

fn session_history_summary_from_row(
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

fn host_transcript_path_from_row(row: &SessionRow) -> Option<PathBuf> {
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

fn recent_history_summary_from_row(row: &RecentCodexRow, path: Option<PathBuf>) -> SessionSummary {
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

fn request_history_summary_from_request(
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

fn prepare_select_history_from_external(
    ui: &mut UiState,
    summary: SessionSummary,
    origin: CodexHistoryExternalFocusOrigin,
) {
    ui.page = Page::History;
    ui.focus = Focus::Sessions;
    ui.prepare_codex_history_external_focus(summary, origin);
    ui.needs_codex_history_refresh = true;
}

async fn apply_effort_override(state: &ProxyState, sid: String, effort: Option<String>) {
    let now = now_ms();
    if let Some(eff) = effort {
        state.set_session_effort_override(sid, eff, now).await;
    } else {
        state.clear_session_effort_override(&sid).await;
    }
}

async fn apply_model_override(state: &ProxyState, sid: String, model: Option<String>) {
    let now = now_ms();
    if let Some(model) = model {
        state.set_session_model_override(sid, model, now).await;
    } else {
        state.clear_session_model_override(&sid).await;
    }
}

async fn apply_service_tier_override(
    state: &ProxyState,
    sid: String,
    service_tier: Option<String>,
) {
    let now = now_ms();
    if let Some(service_tier) = service_tier {
        state
            .set_session_service_tier_override(sid, service_tier, now)
            .await;
    } else {
        state.clear_session_service_tier_override(&sid).await;
    }
}

async fn apply_session_profile(
    state: &ProxyState,
    service_name: &str,
    sid: String,
    profile_name: String,
) -> anyhow::Result<()> {
    let cfg = load_config().await?;
    let mgr = match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    state
        .apply_session_profile_binding(service_name, mgr, sid, profile_name, now_ms())
        .await
}

async fn apply_session_provider_override(state: &ProxyState, sid: String, cfg: Option<String>) {
    let now = now_ms();
    if let Some(cfg) = cfg {
        state.set_session_station_override(sid, cfg, now).await;
    } else {
        state.clear_session_station_override(&sid).await;
    }
}

async fn clear_session_manual_overrides(state: &ProxyState, sid: String) {
    state.clear_session_manual_overrides(&sid).await;
}

async fn apply_global_active_station(
    state: &ProxyState,
    providers: &mut Vec<ProviderOption>,
    ui: &mut UiState,
    snapshot: &Snapshot,
    station_name: Option<String>,
) -> anyhow::Result<()> {
    // Do not pin routing via runtime override; only persist the preferred station (active),
    // so failover across stations remains possible.
    state.clear_global_station_override().await;

    let mut cfg = load_config().await?;
    if ui.service_name == "codex" {
        let _ = sync_codex_auth_from_codex_cli(
            &mut cfg,
            SyncCodexAuthFromCodexOptions {
                add_missing: false,
                set_active: false,
                force: false,
            },
        );
    }

    let mgr = if ui.service_name == "claude" {
        &mut cfg.claude
    } else {
        &mut cfg.codex
    };
    if let Some(name) = station_name.as_deref()
        && !mgr.contains_station(name)
    {
        anyhow::bail!("unknown station: {name}");
    }
    mgr.active = station_name;

    save_config(&cfg).await?;

    // Best-effort: ask the running server to reload immediately.
    let url = format!(
        "http://127.0.0.1:{}/__codex_helper/api/v1/runtime/reload",
        ui.admin_port
    );
    let _ = reqwest::Client::new().post(&url).send().await;

    *providers = crate::tui::build_provider_options(&cfg, ui.service_name);
    ui.clamp_selection(snapshot, providers.len());
    Ok(())
}

async fn persist_station_meta(
    ui: &UiState,
    station_name: &str,
    enabled: Option<bool>,
    level: Option<u8>,
) -> anyhow::Result<()> {
    let mut cfg = load_config().await?;
    let mgr = if ui.service_name == "claude" {
        &mut cfg.claude
    } else {
        &mut cfg.codex
    };
    let Some(svc) = mgr.station_mut(station_name) else {
        anyhow::bail!("station '{station_name}' not found");
    };
    if let Some(enabled) = enabled {
        svc.enabled = enabled;
    }
    if let Some(level) = level {
        svc.level = level.clamp(1, 10);
    }
    save_config(&cfg).await?;
    Ok(())
}

async fn persist_ui_language(language: Language) -> anyhow::Result<()> {
    let mut cfg = load_config().await?;
    cfg.ui.language = Some(match language {
        Language::Zh => "zh".to_string(),
        Language::En => "en".to_string(),
    });
    save_config(&cfg).await?;
    Ok(())
}

fn language_name(language: Language) -> &'static str {
    match language {
        Language::Zh => "中文",
        Language::En => "English",
    }
}

async fn toggle_language(ui: &mut UiState) {
    let next = if ui.language == Language::En {
        Language::Zh
    } else {
        Language::En
    };
    ui.language = next;
    match persist_ui_language(next).await {
        Ok(()) => {
            ui.toast = Some((
                format!(
                    "{}{}{}",
                    crate::tui::i18n::pick(ui.language, "语言：", "language: "),
                    language_name(next),
                    crate::tui::i18n::pick(ui.language, "（已保存）", " (saved)")
                ),
                Instant::now(),
            ));
        }
        Err(err) => {
            let suffix = match ui.language {
                Language::Zh => format!("（保存失败：{err}）"),
                Language::En => format!(" (save failed: {err})"),
            };
            ui.toast = Some((
                format!(
                    "{}{}{}",
                    crate::tui::i18n::pick(ui.language, "语言：", "language: "),
                    language_name(next),
                    suffix
                ),
                Instant::now(),
            ));
        }
    }
}

fn shorten_err(err: &str, max: usize) -> String {
    if err.chars().count() <= max {
        return err.to_string();
    }
    err.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}

fn health_check_timeout() -> Duration {
    let ms = std::env::var("CODEX_HELPER_TUI_HEALTHCHECK_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(2_500)
        .clamp(300, 20_000);
    Duration::from_millis(ms)
}

fn health_check_upstream_concurrency() -> usize {
    std::env::var("CODEX_HELPER_TUI_HEALTHCHECK_UPSTREAM_CONCURRENCY")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(4)
        .min(32)
}

fn health_check_max_inflight_stations() -> usize {
    std::env::var("CODEX_HELPER_TUI_HEALTHCHECK_MAX_INFLIGHT")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(2)
        .min(16)
}

fn health_check_station_semaphore() -> &'static OnceCell<Arc<Semaphore>> {
    static SEM: OnceCell<Arc<Semaphore>> = OnceCell::const_new();
    &SEM
}

fn health_check_url(base_url: &str) -> anyhow::Result<Url> {
    let mut url = Url::parse(base_url)?;
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url.join("models")?)
}

async fn probe_upstream(client: &reqwest::Client, upstream: &UpstreamConfig) -> UpstreamHealth {
    let mut out = UpstreamHealth {
        base_url: upstream.base_url.clone(),
        ..UpstreamHealth::default()
    };

    let url = match health_check_url(&upstream.base_url) {
        Ok(u) => u,
        Err(e) => {
            out.ok = Some(false);
            out.error = Some(shorten_err(&format!("invalid base_url: {e}"), 140));
            return out;
        }
    };

    let start = Instant::now();
    let mut req = client.get(url).header("Accept", "application/json");
    if let Some(token) = upstream.auth.resolve_auth_token() {
        req = req.header("Authorization", format!("Bearer {}", token));
    } else if let Some(key) = upstream.auth.resolve_api_key() {
        req = req.header("X-API-Key", key);
    }

    match req.send().await {
        Ok(resp) => {
            out.latency_ms = Some(start.elapsed().as_millis() as u64);
            out.status_code = Some(resp.status().as_u16());
            out.ok = Some(resp.status().is_success());
            if !resp.status().is_success() {
                out.error = Some(shorten_err(&format!("HTTP {}", resp.status()), 140));
            }
        }
        Err(e) => {
            out.latency_ms = Some(start.elapsed().as_millis() as u64);
            out.ok = Some(false);
            out.error = Some(shorten_err(&e.to_string(), 140));
        }
    }
    out
}

async fn load_upstreams_for_station(
    service_name: &str,
    station_name: &str,
) -> anyhow::Result<Vec<UpstreamConfig>> {
    let cfg = load_config().await?;
    let mgr = if service_name == "claude" {
        &cfg.claude
    } else {
        &cfg.codex
    };
    let Some(svc) = mgr.station(station_name) else {
        anyhow::bail!("station '{station_name}' not found");
    };
    Ok(svc.upstreams.clone())
}

async fn run_health_check_for_station(
    state: Arc<ProxyState>,
    service_name: &'static str,
    station_name: String,
    upstreams: Vec<UpstreamConfig>,
) {
    let timeout = health_check_timeout();
    let client = match reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout)
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            let now = now_ms();
            state
                .record_station_health_check_result(
                    service_name,
                    &station_name,
                    now,
                    UpstreamHealth {
                        base_url: "<client>".to_string(),
                        ok: Some(false),
                        status_code: None,
                        latency_ms: None,
                        error: Some(shorten_err(&err.to_string(), 140)),
                        passive: None,
                    },
                )
                .await;
            state
                .finish_station_health_check(service_name, &station_name, now, false)
                .await;
            return;
        }
    };

    let upstream_conc = health_check_upstream_concurrency();
    let sem = Arc::new(Semaphore::new(upstream_conc));
    let mut futs = FuturesUnordered::new();
    for upstream in upstreams {
        let client = client.clone();
        let sem = Arc::clone(&sem);
        futs.push(async move {
            let _permit = sem.acquire().await;
            probe_upstream(&client, &upstream).await
        });
    }

    let mut canceled = false;
    while let Some(up) = futs.next().await {
        let now = now_ms();
        state
            .record_station_health_check_result(service_name, &station_name, now, up)
            .await;
        if state
            .is_station_health_check_cancel_requested(service_name, &station_name)
            .await
        {
            canceled = true;
            break;
        }
    }

    let now = now_ms();
    state
        .finish_station_health_check(service_name, &station_name, now, canceled)
        .await;
}

fn reports_dir() -> std::path::PathBuf {
    proxy_home_dir().join("reports")
}

fn write_report(report: &str, now_ms: u64) -> anyhow::Result<std::path::PathBuf> {
    let dir = reports_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("tui_stats_report.{now_ms}.txt"));
    std::fs::write(&path, report.as_bytes())?;
    Ok(path)
}

fn try_copy_to_clipboard(report: &str) -> anyhow::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    fn run(mut cmd: Command, report: &str) -> anyhow::Result<()> {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        {
            let Some(mut stdin) = child.stdin.take() else {
                anyhow::bail!("no stdin");
            };
            stdin.write_all(report.as_bytes())?;
        }
        let status = child.wait()?;
        if status.success() {
            Ok(())
        } else {
            anyhow::bail!("clipboard command failed")
        }
    }

    #[cfg(target_os = "macos")]
    {
        run(Command::new("pbcopy"), report)
    }

    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "clip"]);
        run(cmd, report)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Ok(()) = run(Command::new("wl-copy"), report) {
            return Ok(());
        }
        let mut cmd = Command::new("xclip");
        cmd.args(["-selection", "clipboard"]);
        run(cmd, report)
    }
}

fn format_session_transcript_text(ui: &UiState) -> String {
    let sid = ui.session_transcript_sid.as_deref().unwrap_or("-");
    let mode = match ui.session_transcript_tail {
        Some(n) => format!("tail {n}"),
        None => "all".to_string(),
    };

    let mut out = String::new();
    out.push_str(&format!("sid: {sid}\n"));
    out.push_str(&format!("mode: {mode}\n"));

    if let Some(meta) = ui.session_transcript_meta.as_ref() {
        out.push_str(&format!(
            "meta: id={} cwd={}\n",
            meta.id,
            meta.cwd.as_deref().unwrap_or("-")
        ));
    }
    if let Some(file) = ui.session_transcript_file.as_deref() {
        out.push_str(&format!("file: {file}\n"));
    }
    out.push('\n');

    for msg in ui.session_transcript_messages.iter() {
        let head = if let Some(ts) = msg.timestamp.as_deref() {
            format!("[{}] {}", ts, msg.role)
        } else {
            msg.role.clone()
        };
        out.push_str(&head);
        out.push('\n');
        out.push_str(msg.text.as_str());
        out.push_str("\n\n");
    }

    out
}

async fn open_session_transcript_from_path(
    ui: &mut UiState,
    sid: String,
    path: &Path,
    tail: Option<usize>,
) {
    ui.session_transcript_sid = Some(sid);
    ui.session_transcript_meta = None;
    ui.session_transcript_file = Some(path.to_string_lossy().to_string());
    ui.session_transcript_tail = tail;
    ui.session_transcript_messages.clear();
    ui.session_transcript_scroll = u16::MAX;
    ui.session_transcript_error = None;

    match read_codex_session_meta(path).await {
        Ok(meta) => ui.session_transcript_meta = meta,
        Err(e) => ui.session_transcript_error = Some(e.to_string()),
    }
    match read_codex_session_transcript(path, tail).await {
        Ok(msgs) => ui.session_transcript_messages = msgs,
        Err(e) => ui.session_transcript_error = Some(e.to_string()),
    }
    ui.overlay = Overlay::SessionTranscript;
}

async fn handle_key_normal(
    state: &Arc<ProxyState>,
    providers: &mut Vec<ProviderOption>,
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
        ui.should_exit = true;
        return true;
    }

    match key.code {
        KeyCode::Char('q') => {
            ui.should_exit = true;
            true
        }
        KeyCode::Char('L') => {
            toggle_language(ui).await;
            true
        }
        KeyCode::Char('?') => {
            ui.overlay = Overlay::Help;
            true
        }
        KeyCode::Char('O') if ui.page == Page::Settings => {
            if ui.service_name != "codex" {
                ui.toast = Some((
                    "overwrite-from-codex is only supported for Codex service".to_string(),
                    Instant::now(),
                ));
                return true;
            }

            let now = Instant::now();
            if let Some(prev) = ui.pending_overwrite_from_codex_confirm_at
                && now.duration_since(prev) <= Duration::from_secs(3)
            {
                ui.pending_overwrite_from_codex_confirm_at = None;
            } else {
                ui.pending_overwrite_from_codex_confirm_at = Some(now);
                ui.toast = Some((
                    crate::tui::i18n::pick(
                        ui.language,
                        "再次按 O 确认覆盖导入（3s 内）",
                        "Press O again to confirm overwrite (within 3s)",
                    )
                    .to_string(),
                    now,
                ));
                return true;
            }

            match load_config().await {
                Ok(mut cfg) => {
                    if let Err(err) = overwrite_codex_config_from_codex_cli_in_place(&mut cfg) {
                        ui.toast = Some((
                            format!("overwrite-from-codex failed: {err}"),
                            Instant::now(),
                        ));
                        return true;
                    }
                    if let Err(err) = save_config(&cfg).await {
                        ui.toast = Some((format!("save failed: {err}"), Instant::now()));
                        return true;
                    }

                    *providers = crate::tui::build_provider_options(&cfg, ui.service_name);
                    ui.clamp_selection(snapshot, providers.len());
                    let _ = refresh_profile_control_state(ui).await;
                    ui.toast = Some((
                        format!("overwrote stations from ~/.codex (n={})", providers.len()),
                        Instant::now(),
                    ));
                    true
                }
                Err(err) => {
                    ui.toast = Some((format!("load config failed: {err}"), Instant::now()));
                    true
                }
            }
        }
        KeyCode::Char('R') if ui.page == Page::Settings => {
            let now = Instant::now();
            let url = format!(
                "http://127.0.0.1:{}/__codex_helper/api/v1/runtime/reload",
                ui.admin_port
            );
            let res = async {
                let client = reqwest::Client::new();
                client
                    .post(&url)
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<serde_json::Value>()
                    .await
            }
            .await;
            match res {
                Ok(v) => {
                    let st = v.get("status");
                    ui.last_runtime_config_loaded_at_ms = st
                        .and_then(|x| x.get("loaded_at_ms"))
                        .and_then(|x| x.as_u64());
                    ui.last_runtime_config_source_mtime_ms = st
                        .and_then(|x| x.get("source_mtime_ms"))
                        .and_then(|x| x.as_u64());
                    ui.last_runtime_retry = st
                        .and_then(|x| x.get("retry"))
                        .and_then(|x| serde_json::from_value(x.clone()).ok());
                    ui.last_runtime_config_refresh_at = Some(now);
                    let _ = refresh_profile_control_state(ui).await;

                    let changed = v.get("reloaded").and_then(|x| x.as_bool()).unwrap_or(false);
                    ui.toast = Some((
                        crate::tui::i18n::pick(
                            ui.language,
                            format!(
                                "已重载配置（{}）",
                                if changed {
                                    "检测到变更"
                                } else {
                                    "无变更"
                                }
                            )
                            .as_str(),
                            format!(
                                "Config reloaded ({})",
                                if changed { "changed" } else { "no change" }
                            )
                            .as_str(),
                        )
                        .to_string(),
                        now,
                    ));
                    true
                }
                Err(err) => {
                    ui.toast = Some((format!("reload failed: {err}"), now));
                    true
                }
            }
        }
        KeyCode::Char('i') if ui.page == Page::Stations => {
            ui.overlay = Overlay::StationInfo;
            ui.station_info_scroll = 0;
            true
        }
        KeyCode::Tab => {
            if ui.page == Page::Dashboard {
                ui.focus = match ui.focus {
                    Focus::Sessions => Focus::Requests,
                    Focus::Requests => Focus::Sessions,
                    Focus::Stations => Focus::Sessions,
                };
            } else if ui.page == Page::Stations {
                ui.focus = Focus::Stations;
            } else if ui.page == Page::Stats {
                ui.stats_focus = match ui.stats_focus {
                    StatsFocus::Stations => StatsFocus::Providers,
                    StatsFocus::Providers => StatsFocus::Stations,
                };
                ui.toast = Some((
                    format!(
                        "stats focus: {}",
                        match ui.stats_focus {
                            StatsFocus::Stations => "stations",
                            StatsFocus::Providers => "providers",
                        }
                    ),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Stations => {
            if let Some(next) = adjust_table_selection(&mut ui.stations_table, -1, providers.len())
            {
                ui.selected_station_idx = next;
                return true;
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Stations => {
            if let Some(next) = adjust_table_selection(&mut ui.stations_table, 1, providers.len()) {
                ui.selected_station_idx = next;
                return true;
            }
            false
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Stats => {
            let (table, len) = match ui.stats_focus {
                StatsFocus::Stations => (
                    &mut ui.stats_stations_table,
                    snapshot.usage_rollup.by_config.len(),
                ),
                StatsFocus::Providers => (
                    &mut ui.stats_providers_table,
                    snapshot.usage_rollup.by_provider.len(),
                ),
            };
            if let Some(next) = adjust_table_selection(table, -1, len) {
                match ui.stats_focus {
                    StatsFocus::Stations => ui.selected_stats_station_idx = next,
                    StatsFocus::Providers => ui.selected_stats_provider_idx = next,
                }
                return true;
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Stats => {
            let (table, len) = match ui.stats_focus {
                StatsFocus::Stations => (
                    &mut ui.stats_stations_table,
                    snapshot.usage_rollup.by_config.len(),
                ),
                StatsFocus::Providers => (
                    &mut ui.stats_providers_table,
                    snapshot.usage_rollup.by_provider.len(),
                ),
            };
            if let Some(next) = adjust_table_selection(table, 1, len) {
                match ui.stats_focus {
                    StatsFocus::Stations => ui.selected_stats_station_idx = next,
                    StatsFocus::Providers => ui.selected_stats_provider_idx = next,
                }
                return true;
            }
            false
        }
        KeyCode::Char('d') if ui.page == Page::Stats => {
            let options = [7usize, 21usize, 60usize];
            let idx = options
                .iter()
                .position(|&n| n == ui.stats_days)
                .unwrap_or(1);
            let next = options[(idx + 1) % options.len()];
            ui.stats_days = next;
            ui.needs_snapshot_refresh = true;
            ui.toast = Some((format!("stats days: {next}"), Instant::now()));
            true
        }
        KeyCode::Char('e') if ui.page == Page::Stats => {
            ui.stats_errors_only = !ui.stats_errors_only;
            ui.toast = Some((
                format!("stats: errors_only={}", ui.stats_errors_only),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('y') if ui.page == Page::Stats => {
            let now = now_ms();
            let Some(report) = build_stats_report(ui, snapshot, now) else {
                ui.toast = Some(("stats report: no selection".to_string(), Instant::now()));
                return true;
            };
            let saved = write_report(&report, now);
            let copied = try_copy_to_clipboard(&report);

            match (saved, copied) {
                (Ok(path), Ok(())) => {
                    ui.toast = Some((
                        format!("stats report: copied + saved {}", path.display()),
                        Instant::now(),
                    ));
                }
                (Ok(path), Err(err)) => {
                    ui.toast = Some((
                        format!(
                            "stats report: saved {} (copy failed: {err})",
                            path.display()
                        ),
                        Instant::now(),
                    ));
                }
                (Err(err), Ok(())) => {
                    ui.toast = Some((
                        format!("stats report: copied (save failed: {err})"),
                        Instant::now(),
                    ));
                }
                (Err(err1), Err(err2)) => {
                    ui.toast = Some((
                        format!("stats report: copy failed: {err2} (save failed: {err1})"),
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Enter if ui.page == Page::Stations => {
            let Some(name) = providers
                .get(ui.selected_station_idx)
                .map(|p| p.name.clone())
            else {
                return true;
            };
            match apply_global_active_station(state, providers, ui, snapshot, Some(name.clone()))
                .await
            {
                Ok(()) => {
                    ui.toast = Some((format!("active station: {name}"), Instant::now()));
                }
                Err(err) => {
                    ui.toast = Some((format!("set active failed: {err}"), Instant::now()));
                }
            }
            true
        }
        KeyCode::Backspace | KeyCode::Delete if ui.page == Page::Stations => {
            match apply_global_active_station(state, providers, ui, snapshot, None).await {
                Ok(()) => {
                    ui.toast = Some(("active station: <auto>".to_string(), Instant::now()));
                }
                Err(err) => {
                    ui.toast = Some((format!("set active failed: {err}"), Instant::now()));
                }
            }
            true
        }
        KeyCode::Char('o') if ui.page == Page::Stations => {
            let Some(pvd) = providers.get(ui.selected_station_idx) else {
                return true;
            };
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                ui.toast = Some((
                    "session station override: <no session>".to_string(),
                    Instant::now(),
                ));
                return true;
            };
            apply_session_provider_override(state, sid, Some(pvd.name.clone())).await;
            ui.toast = Some((
                format!("session station override: {}", pvd.name),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('O') if ui.page == Page::Stations => {
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                ui.toast = Some((
                    "session station override: <no session>".to_string(),
                    Instant::now(),
                ));
                return true;
            };
            apply_session_provider_override(state, sid, None).await;
            ui.toast = Some((
                "session station override: <clear>".to_string(),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('t') if ui.page == Page::Stations => {
            let Some(pvd) = providers.get(ui.selected_station_idx) else {
                return true;
            };
            let (enabled_ovr, _) = snapshot
                .station_meta_overrides
                .get(pvd.name.as_str())
                .copied()
                .unwrap_or((None, None));
            let current = enabled_ovr.unwrap_or(pvd.enabled);
            let next = !current;
            let now = now_ms();
            state
                .set_station_enabled_override(ui.service_name, pvd.name.clone(), next, now)
                .await;

            if let Err(err) = persist_station_meta(ui, &pvd.name, Some(next), None).await {
                ui.toast = Some((format!("save failed: {err}"), Instant::now()));
            } else {
                ui.toast = Some((
                    format!(
                        "station {} enabled={}",
                        pvd.name,
                        if next { "true" } else { "false" }
                    ),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Char('+') | KeyCode::Char('=') if ui.page == Page::Stations => {
            let Some(pvd) = providers.get(ui.selected_station_idx) else {
                return true;
            };
            let (_, level_ovr) = snapshot
                .station_meta_overrides
                .get(pvd.name.as_str())
                .copied()
                .unwrap_or((None, None));
            let current = level_ovr.unwrap_or(pvd.level).clamp(1, 10);
            let next = (current + 1).min(10);
            let now = now_ms();
            state
                .set_station_level_override(ui.service_name, pvd.name.clone(), next, now)
                .await;
            if let Err(err) = persist_station_meta(ui, &pvd.name, None, Some(next)).await {
                ui.toast = Some((format!("save failed: {err}"), Instant::now()));
            } else {
                ui.toast = Some((format!("station {} level={next}", pvd.name), Instant::now()));
            }
            true
        }
        KeyCode::Char('-') if ui.page == Page::Stations => {
            let Some(pvd) = providers.get(ui.selected_station_idx) else {
                return true;
            };
            let (_, level_ovr) = snapshot
                .station_meta_overrides
                .get(pvd.name.as_str())
                .copied()
                .unwrap_or((None, None));
            let current = level_ovr.unwrap_or(pvd.level).clamp(1, 10);
            let next = current.saturating_sub(1).max(1);
            let now = now_ms();
            state
                .set_station_level_override(ui.service_name, pvd.name.clone(), next, now)
                .await;
            if let Err(err) = persist_station_meta(ui, &pvd.name, None, Some(next)).await {
                ui.toast = Some((format!("save failed: {err}"), Instant::now()));
            } else {
                ui.toast = Some((format!("station {} level={next}", pvd.name), Instant::now()));
            }
            true
        }
        KeyCode::Char('h') if ui.page == Page::Stations => {
            let Some(pvd) = providers.get(ui.selected_station_idx) else {
                return true;
            };
            let service_name = ui.service_name;
            let station_name = pvd.name.clone();

            let upstreams = match load_upstreams_for_station(service_name, &station_name).await {
                Ok(v) => v,
                Err(err) => {
                    ui.toast = Some((format!("health check load failed: {err}"), Instant::now()));
                    return true;
                }
            };

            let now = now_ms();
            if !state
                .try_begin_station_health_check(service_name, &station_name, upstreams.len(), now)
                .await
            {
                ui.toast = Some((
                    format!("health check already running: {station_name}"),
                    Instant::now(),
                ));
                return true;
            }

            state
                .record_station_health(
                    service_name,
                    station_name.clone(),
                    StationHealth {
                        checked_at_ms: now,
                        upstreams: Vec::new(),
                    },
                )
                .await;

            let state = Arc::clone(state);
            ui.toast = Some((
                format!("health check queued: {station_name}"),
                Instant::now(),
            ));
            let upstreams_for_task = upstreams;
            tokio::spawn(async move {
                let sem = health_check_station_semaphore()
                    .get_or_init(|| async {
                        Arc::new(Semaphore::new(health_check_max_inflight_stations()))
                    })
                    .await;
                let _permit = sem.clone().acquire_owned().await;
                run_health_check_for_station(state, service_name, station_name, upstreams_for_task)
                    .await;
            });
            true
        }
        KeyCode::Char('H') if ui.page == Page::Stations => {
            let service_name = ui.service_name;
            let stations = providers.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
            let state = Arc::clone(state);
            ui.toast = Some((
                format!("health check queued: {} stations", stations.len()),
                Instant::now(),
            ));
            tokio::spawn(async move {
                let sem = health_check_station_semaphore()
                    .get_or_init(|| async {
                        Arc::new(Semaphore::new(health_check_max_inflight_stations()))
                    })
                    .await
                    .clone();

                let cfg = match load_config().await {
                    Ok(c) => c,
                    Err(err) => {
                        let now = now_ms();
                        for station_name in stations {
                            state
                                .try_begin_station_health_check(service_name, &station_name, 1, now)
                                .await;
                            state
                                .record_station_health_check_result(
                                    service_name,
                                    &station_name,
                                    now,
                                    UpstreamHealth {
                                        base_url: "<load_config>".to_string(),
                                        ok: Some(false),
                                        status_code: None,
                                        latency_ms: None,
                                        error: Some(shorten_err(&err.to_string(), 140)),
                                        passive: None,
                                    },
                                )
                                .await;
                            state
                                .finish_station_health_check(
                                    service_name,
                                    &station_name,
                                    now,
                                    false,
                                )
                                .await;
                        }
                        return;
                    }
                };

                let mgr = if service_name == "claude" {
                    &cfg.claude
                } else {
                    &cfg.codex
                };
                for station_name in stations {
                    let Some(svc) = mgr.station(&station_name) else {
                        continue;
                    };
                    let upstreams = svc.upstreams.clone();
                    let now = now_ms();
                    if !state
                        .try_begin_station_health_check(
                            service_name,
                            &station_name,
                            upstreams.len(),
                            now,
                        )
                        .await
                    {
                        continue;
                    }
                    state
                        .record_station_health(
                            service_name,
                            station_name.clone(),
                            StationHealth {
                                checked_at_ms: now,
                                upstreams: Vec::new(),
                            },
                        )
                        .await;

                    let state = Arc::clone(&state);
                    let sem = sem.clone();
                    tokio::spawn(async move {
                        let _permit = sem.acquire_owned().await;
                        run_health_check_for_station(state, service_name, station_name, upstreams)
                            .await;
                    });

                    tokio::time::sleep(Duration::from_millis(40)).await;
                }
            });
            true
        }
        KeyCode::Char('c') if ui.page == Page::Stations => {
            let Some(pvd) = providers.get(ui.selected_station_idx) else {
                return true;
            };
            let now = now_ms();
            if state
                .request_cancel_station_health_check(ui.service_name, pvd.name.as_str(), now)
                .await
            {
                ui.toast = Some((
                    format!("health check cancel requested: {}", pvd.name),
                    Instant::now(),
                ));
            } else {
                ui.toast = Some((
                    format!("health check not running: {}", pvd.name),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Char('C') if ui.page == Page::Stations => {
            let now = now_ms();
            let mut count = 0usize;
            for p in providers {
                if state
                    .request_cancel_station_health_check(ui.service_name, p.name.as_str(), now)
                    .await
                {
                    count += 1;
                }
            }
            ui.toast = Some((
                format!("health check cancel requested: {count} stations"),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('a') if ui.page == Page::Sessions => {
            ui.sessions_page_active_only = !ui.sessions_page_active_only;
            ui.selected_sessions_page_idx = 0;
            ui.toast = Some((
                format!(
                    "sessions filter: active_only={}",
                    ui.sessions_page_active_only
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('e') if ui.page == Page::Sessions => {
            ui.sessions_page_errors_only = !ui.sessions_page_errors_only;
            ui.selected_sessions_page_idx = 0;
            ui.toast = Some((
                format!(
                    "sessions filter: errors_only={}",
                    ui.sessions_page_errors_only
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('v') if ui.page == Page::Sessions => {
            ui.sessions_page_overrides_only = !ui.sessions_page_overrides_only;
            ui.selected_sessions_page_idx = 0;
            ui.toast = Some((
                format!(
                    "sessions filter: overrides_only={}",
                    ui.sessions_page_overrides_only
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('r') if ui.page == Page::Sessions => {
            ui.sessions_page_active_only = false;
            ui.sessions_page_errors_only = false;
            ui.sessions_page_overrides_only = false;
            ui.selected_sessions_page_idx = 0;
            ui.toast = Some(("sessions filter: reset".to_string(), Instant::now()));
            true
        }
        KeyCode::Char('r') if ui.page == Page::History => {
            ui.needs_codex_history_refresh = true;
            ui.toast = Some((
                crate::tui::i18n::pick(ui.language, "history: 刷新中…", "history: refreshing…")
                    .to_string(),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('r') if ui.page == Page::Recent => {
            ui.needs_codex_recent_refresh = true;
            ui.toast = Some((
                crate::tui::i18n::pick(ui.language, "recent: 刷新中…", "recent: refreshing…")
                    .to_string(),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('b')
            if ui.focus == Focus::Sessions
                && matches!(ui.page, Page::Dashboard | Page::Sessions) =>
        {
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|row| row.session_id.as_deref())
            else {
                ui.toast = Some(("no session selected".to_string(), Instant::now()));
                return true;
            };

            match refresh_profile_control_state(ui).await {
                Ok(()) if ui.profile_options.is_empty() => {
                    ui.toast = Some((
                        crate::tui::i18n::pick(
                            ui.language,
                            "profile: 当前服务没有可用 profile",
                            "profile: no profiles configured for this service",
                        )
                        .to_string(),
                        Instant::now(),
                    ));
                }
                Ok(()) => {
                    let selected_profile = snapshot
                        .rows
                        .get(ui.selected_session_idx)
                        .and_then(|row| row.binding_profile_name.as_deref());
                    ui.profile_menu_idx =
                        default_profile_menu_idx(&ui.profile_options, selected_profile);
                    ui.overlay = Overlay::ProfileMenuSession;
                    ui.toast = Some((
                        format!("profile: manage binding for {}", short_sid(sid, 18)),
                        Instant::now(),
                    ));
                }
                Err(e) => {
                    ui.toast = Some((format!("profile: load failed: {e}"), Instant::now()));
                }
            }
            true
        }
        KeyCode::Char('p') if ui.page == Page::Settings => {
            match refresh_profile_control_state(ui).await {
                Ok(()) if ui.profile_options.is_empty() => {
                    ui.toast = Some((
                        crate::tui::i18n::pick(
                            ui.language,
                            "default profile: 当前服务没有可用 profile",
                            "default profile: no profiles configured for this service",
                        )
                        .to_string(),
                        Instant::now(),
                    ));
                }
                Ok(()) => {
                    ui.profile_menu_idx = default_profile_menu_idx(
                        &ui.profile_options,
                        ui.configured_default_profile.as_deref(),
                    );
                    ui.overlay = Overlay::ProfileMenuDefaultPersisted;
                    ui.toast = Some((
                        crate::tui::i18n::pick(
                            ui.language,
                            "default profile: 管理配置默认值",
                            "default profile: manage configured default",
                        )
                        .to_string(),
                        Instant::now(),
                    ));
                }
                Err(err) => {
                    ui.toast = Some((
                        format!("default profile load failed: {err}"),
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Char('P') if ui.page == Page::Settings => {
            match refresh_profile_control_state(ui).await {
                Ok(()) if ui.profile_options.is_empty() => {
                    ui.toast = Some((
                        crate::tui::i18n::pick(
                            ui.language,
                            "runtime default profile: 当前服务没有可用 profile",
                            "runtime default profile: no profiles configured for this service",
                        )
                        .to_string(),
                        Instant::now(),
                    ));
                }
                Ok(()) => {
                    ui.profile_menu_idx = runtime_default_profile_menu_idx(
                        &ui.profile_options,
                        ui.runtime_default_profile_override.as_deref(),
                    );
                    ui.overlay = Overlay::ProfileMenuDefaultRuntime;
                    ui.toast = Some((
                        crate::tui::i18n::pick(
                            ui.language,
                            "runtime default profile: 管理运行时默认值",
                            "runtime default profile: manage runtime default",
                        )
                        .to_string(),
                        Instant::now(),
                    ));
                }
                Err(err) => {
                    ui.toast = Some((
                        format!("runtime default profile load failed: {err}"),
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Char('M')
            if ui.focus == Focus::Sessions
                && matches!(ui.page, Page::Dashboard | Page::Sessions) =>
        {
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|row| row.session_id.as_deref())
            else {
                ui.toast = Some(("no session selected".to_string(), Instant::now()));
                return true;
            };

            match load_model_options_for_service(ui.service_name).await {
                Ok(mut models) => {
                    let current = selected_session_model_hint(snapshot, ui);
                    add_model_option_if_missing(&mut models, current.as_deref());
                    if models.is_empty() {
                        ui.toast = Some((
                            crate::tui::i18n::pick(
                                ui.language,
                                "model: 当前服务没有可用模型目录",
                                "model: no model catalog available for this service",
                            )
                            .to_string(),
                            Instant::now(),
                        ));
                        return true;
                    }

                    let current_override = snapshot
                        .rows
                        .get(ui.selected_session_idx)
                        .and_then(|row| row.override_model.as_deref())
                        .unwrap_or("");
                    ui.model_menu_idx = models
                        .iter()
                        .position(|model| model == current_override)
                        .map(|idx| idx + 1)
                        .unwrap_or(0);
                    ui.session_model_options = models;
                    ui.session_model_input =
                        current_model_override(snapshot, ui).unwrap_or_default();
                    ui.session_model_input_hint = selected_session_model_hint(snapshot, ui);
                    ui.overlay = Overlay::ModelMenuSession;
                    ui.toast = Some((
                        format!("model: select target for {}", short_sid(sid, 18)),
                        Instant::now(),
                    ));
                }
                Err(err) => {
                    ui.toast = Some((format!("model: load failed: {err}"), Instant::now()));
                }
            }
            true
        }
        KeyCode::Char('f')
            if ui.focus == Focus::Sessions
                && matches!(ui.page, Page::Dashboard | Page::Sessions) =>
        {
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|row| row.session_id.as_deref())
            else {
                ui.toast = Some(("no session selected".to_string(), Instant::now()));
                return true;
            };

            let current = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|row| row.override_service_tier.as_deref())
                .unwrap_or("");
            ui.service_tier_menu_idx = match current {
                "default" => 1,
                "priority" => 2,
                "flex" => 3,
                _ => 0,
            };
            ui.session_service_tier_input =
                current_service_tier_override(snapshot, ui).unwrap_or_default();
            ui.session_service_tier_input_hint = selected_session_service_tier_hint(snapshot, ui);
            ui.overlay = Overlay::ServiceTierMenuSession;
            ui.toast = Some((
                format!("service_tier: select target for {}", short_sid(sid, 18)),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('R')
            if ui.focus == Focus::Sessions
                && matches!(ui.page, Page::Dashboard | Page::Sessions) =>
        {
            let Some(row) = snapshot.rows.get(ui.selected_session_idx) else {
                ui.toast = Some(("no session selected".to_string(), Instant::now()));
                return true;
            };
            let Some(sid) = row.session_id.clone() else {
                ui.toast = Some(("no session selected".to_string(), Instant::now()));
                return true;
            };
            if !session_row_has_any_override(row) {
                ui.toast = Some((
                    "session overrides already clear".to_string(),
                    Instant::now(),
                ));
                return true;
            }

            clear_session_manual_overrides(state, sid).await;
            ui.needs_snapshot_refresh = true;
            ui.toast = Some(("session manual overrides reset".to_string(), Instant::now()));
            true
        }
        KeyCode::Char('[') if ui.page == Page::Recent => {
            if ui.codex_recent_window_idx == 0 {
                ui.codex_recent_window_idx = CODEX_RECENT_WINDOWS.len().saturating_sub(1);
            } else {
                ui.codex_recent_window_idx = ui.codex_recent_window_idx.saturating_sub(1);
            }
            ui.codex_recent_selected_idx = 0;
            ui.codex_recent_selected_id = None;
            ui.codex_recent_table.select(None);
            ui.toast = Some((
                format!(
                    "recent window: {}",
                    codex_recent_window_label(ui.codex_recent_window_idx)
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char(']') if ui.page == Page::Recent => {
            ui.codex_recent_window_idx =
                (ui.codex_recent_window_idx + 1) % CODEX_RECENT_WINDOWS.len().max(1);
            ui.codex_recent_selected_idx = 0;
            ui.codex_recent_selected_id = None;
            ui.codex_recent_table.select(None);
            ui.toast = Some((
                format!(
                    "recent window: {}",
                    codex_recent_window_label(ui.codex_recent_window_idx)
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('o') if ui.page == Page::Sessions => {
            let Some(sid) = ui.selected_session_id.clone() else {
                ui.toast = Some(("sessions: no session selected".to_string(), Instant::now()));
                return true;
            };
            let sid_label = short_sid(&sid, 18);
            prepare_select_requests_for_session(ui, sid);
            ui.toast = Some((
                format!("requests: focused session {sid_label}"),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('H') if ui.page == Page::Sessions => {
            let Some(row) = snapshot.rows.get(ui.selected_session_idx) else {
                ui.toast = Some(("sessions: no session selected".to_string(), Instant::now()));
                return true;
            };
            let Some(sid) = row.session_id.as_deref() else {
                ui.toast = Some((
                    "sessions: selected row has no session id".to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let path = if let Some(path) = host_transcript_path_from_row(row) {
                Some(path)
            } else {
                match find_codex_session_file_by_id(sid).await {
                    Ok(path) => path,
                    Err(e) => {
                        ui.toast = Some((
                            format!("history: resolve session file failed: {e}"),
                            Instant::now(),
                        ));
                        return true;
                    }
                }
            };
            let Some(summary) = session_history_summary_from_row(row, path) else {
                ui.toast = Some((
                    "history: failed to prepare session focus".to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let sid_label = short_sid(sid, 18);
            prepare_select_history_from_external(
                ui,
                summary,
                CodexHistoryExternalFocusOrigin::Sessions,
            );
            ui.toast = Some((
                format!("history: focused session {sid_label}"),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('O') if ui.page == Page::Dashboard && ui.focus == Focus::Sessions => {
            let Some(sid) = ui.selected_session_id.clone() else {
                ui.toast = Some(("dashboard: no session selected".to_string(), Instant::now()));
                return true;
            };
            let sid_label = short_sid(&sid, 18);
            prepare_select_requests_for_session(ui, sid);
            ui.toast = Some((
                format!("requests: focused session {sid_label}"),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('H') if ui.page == Page::Dashboard && ui.focus == Focus::Sessions => {
            let Some(row) = snapshot.rows.get(ui.selected_session_idx) else {
                ui.toast = Some(("dashboard: no session selected".to_string(), Instant::now()));
                return true;
            };
            let Some(sid) = row.session_id.as_deref() else {
                ui.toast = Some((
                    "dashboard: selected row has no session id".to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let path = if let Some(path) = host_transcript_path_from_row(row) {
                Some(path)
            } else {
                match find_codex_session_file_by_id(sid).await {
                    Ok(path) => path,
                    Err(e) => {
                        ui.toast = Some((
                            format!("history: resolve session file failed: {e}"),
                            Instant::now(),
                        ));
                        return true;
                    }
                }
            };
            let Some(summary) = session_history_summary_from_row(row, path) else {
                ui.toast = Some((
                    "history: failed to prepare session focus".to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let sid_label = short_sid(sid, 18);
            prepare_select_history_from_external(
                ui,
                summary,
                CodexHistoryExternalFocusOrigin::Sessions,
            );
            ui.toast = Some((
                format!("history: focused session {sid_label}"),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('t') if ui.page == Page::Sessions => {
            let Some(sid) = ui.selected_session_id.clone() else {
                ui.toast = Some(("no session selected".to_string(), Instant::now()));
                return true;
            };
            ui.session_transcript_sid = Some(sid.clone());

            let selected_row = snapshot.rows.get(ui.selected_session_idx);
            let resolved_path =
                if let Some(path) = selected_row.and_then(host_transcript_path_from_row) {
                    Ok(Some(path))
                } else {
                    find_codex_session_file_by_id(&sid).await
                };
            match resolved_path {
                Ok(Some(path)) => {
                    open_session_transcript_from_path(ui, sid, &path, Some(80)).await;
                }
                Ok(None) => {
                    ui.toast = Some((
                        "no Codex session file found for this session id".to_string(),
                        Instant::now(),
                    ));
                }
                Err(e) => {
                    ui.toast = Some((format!("failed to load transcript: {e}"), Instant::now()));
                }
            }
            true
        }
        KeyCode::Enter if ui.page == Page::Recent => {
            let Some(r) = selected_recent_row(ui) else {
                ui.toast = Some(("recent: no selection".to_string(), Instant::now()));
                return true;
            };
            let line = format!("{} {}", r.root, r.session_id);
            match try_copy_to_clipboard(&line) {
                Ok(()) => {
                    ui.toast = Some((
                        crate::tui::i18n::pick(
                            ui.language,
                            "recent: 已复制选中条目",
                            "recent: copied selected",
                        )
                        .to_string(),
                        Instant::now(),
                    ));
                }
                Err(e) => {
                    ui.toast = Some((format!("clipboard failed: {e}"), Instant::now()));
                }
            }
            true
        }
        KeyCode::Char('t') if ui.page == Page::Recent => {
            let Some(r) = selected_recent_row(ui) else {
                ui.toast = Some(("recent: no selection".to_string(), Instant::now()));
                return true;
            };
            let sid = r.session_id.clone();
            match find_codex_session_file_by_id(&sid).await {
                Ok(Some(path)) => {
                    open_session_transcript_from_path(ui, sid, &path, Some(80)).await;
                }
                Ok(None) => {
                    ui.toast = Some((
                        "recent: no local transcript file found for this session".to_string(),
                        Instant::now(),
                    ));
                }
                Err(e) => {
                    ui.toast = Some((
                        format!("recent: resolve session file failed: {e}"),
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Char('s') if ui.page == Page::Recent => {
            let Some(r) = selected_recent_row(ui) else {
                ui.toast = Some(("recent: no selection".to_string(), Instant::now()));
                return true;
            };
            if focus_session_in_sessions(ui, snapshot, r.session_id.as_str()) {
                ui.toast = Some((
                    format!("sessions: focused {}", short_sid(r.session_id.as_str(), 18)),
                    Instant::now(),
                ));
            } else {
                ui.toast = Some((
                    crate::tui::i18n::pick(
                        ui.language,
                        "sessions: 当前 runtime 未观测到这个 recent session",
                        "sessions: this recent session is not currently observed in runtime",
                    )
                    .to_string(),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Char('f') if ui.page == Page::Recent => {
            let Some(r) = selected_recent_row(ui) else {
                ui.toast = Some(("recent: no selection".to_string(), Instant::now()));
                return true;
            };
            let sid_label = short_sid(r.session_id.as_str(), 18);
            prepare_select_requests_for_session(ui, r.session_id);
            ui.toast = Some((
                format!("requests: focused session {sid_label}"),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('h') if ui.page == Page::Recent => {
            let Some(r) = selected_recent_row(ui) else {
                ui.toast = Some(("recent: no selection".to_string(), Instant::now()));
                return true;
            };
            let path = match find_codex_session_file_by_id(r.session_id.as_str()).await {
                Ok(path) => path,
                Err(e) => {
                    ui.toast = Some((
                        format!("history: resolve session file failed: {e}"),
                        Instant::now(),
                    ));
                    return true;
                }
            };
            let summary = recent_history_summary_from_row(&r, path);
            let sid_label = short_sid(r.session_id.as_str(), 18);
            prepare_select_history_from_external(
                ui,
                summary,
                CodexHistoryExternalFocusOrigin::Recent,
            );
            ui.toast = Some((
                format!("history: focused session {sid_label}"),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('y') if ui.page == Page::Recent => {
            let now = now_ms();
            let threshold_ms = codex_recent_window_threshold_ms(now, ui.codex_recent_window_idx);
            let mut out = String::new();
            for r in ui
                .codex_recent_rows
                .iter()
                .filter(|r| r.mtime_ms >= threshold_ms)
            {
                let root = r.root.trim();
                if root.is_empty() || root == "-" {
                    continue;
                }
                out.push_str(root);
                out.push(' ');
                out.push_str(r.session_id.as_str());
                out.push('\n');
            }
            if out.trim().is_empty() {
                ui.toast = Some(("recent: nothing to copy".to_string(), Instant::now()));
                return true;
            }
            match try_copy_to_clipboard(&out) {
                Ok(()) => {
                    ui.toast = Some((
                        crate::tui::i18n::pick(
                            ui.language,
                            "recent: 已复制可见列表",
                            "recent: copied visible list",
                        )
                        .to_string(),
                        Instant::now(),
                    ));
                }
                Err(e) => {
                    ui.toast = Some((format!("clipboard failed: {e}"), Instant::now()));
                }
            }
            true
        }
        KeyCode::Char('o') if ui.page == Page::Dashboard && ui.focus == Focus::Requests => {
            let Some(request) = selected_dashboard_request(snapshot, ui) else {
                ui.toast = Some(("dashboard: no request selected".to_string(), Instant::now()));
                return true;
            };
            let Some(sid) = request.session_id.as_deref() else {
                ui.toast = Some((
                    "dashboard: selected request has no session id".to_string(),
                    Instant::now(),
                ));
                return true;
            };
            if focus_session_in_sessions(ui, snapshot, sid) {
                ui.toast = Some((
                    format!("sessions: focused {}", short_sid(sid, 18)),
                    Instant::now(),
                ));
            } else {
                ui.toast = Some((
                    crate::tui::i18n::pick(
                        ui.language,
                        "sessions: 当前 runtime 未观测到这个 session",
                        "sessions: this session is not currently observed in runtime",
                    )
                    .to_string(),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Char('h') if ui.page == Page::Dashboard && ui.focus == Focus::Requests => {
            let Some(request) = selected_dashboard_request(snapshot, ui).cloned() else {
                ui.toast = Some(("dashboard: no request selected".to_string(), Instant::now()));
                return true;
            };
            let Some(sid) = request.session_id.as_deref() else {
                ui.toast = Some((
                    "dashboard: selected request has no session id".to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let path = match find_codex_session_file_by_id(sid).await {
                Ok(path) => path,
                Err(e) => {
                    ui.toast = Some((
                        format!("history: resolve session file failed: {e}"),
                        Instant::now(),
                    ));
                    return true;
                }
            };
            let Some(summary) = request_history_summary_from_request(&request, path) else {
                ui.toast = Some((
                    "history: failed to prepare request focus".to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let sid_label = short_sid(sid, 18);
            prepare_select_history_from_external(
                ui,
                summary,
                CodexHistoryExternalFocusOrigin::Requests,
            );
            ui.toast = Some((
                format!("history: focused session {sid_label}"),
                Instant::now(),
            ));
            true
        }
        KeyCode::Enter | KeyCode::Char('t') if ui.page == Page::History => {
            let Some(summary) = ui
                .codex_history_sessions
                .get(ui.selected_codex_history_idx)
                .cloned()
            else {
                ui.toast = Some(("history: no selection".to_string(), Instant::now()));
                return true;
            };
            if summary.path.as_os_str().is_empty() {
                ui.toast = Some((
                    crate::tui::i18n::pick(
                        ui.language,
                        "history: 当前选中项没有本地 transcript 文件",
                        "history: selected entry has no local transcript file",
                    )
                    .to_string(),
                    Instant::now(),
                ));
                return true;
            }
            open_session_transcript_from_path(ui, summary.id, &summary.path, Some(80)).await;
            true
        }
        KeyCode::Char('s') if ui.page == Page::History => {
            let Some(sid) = ui
                .codex_history_sessions
                .get(ui.selected_codex_history_idx)
                .map(|summary| summary.id.clone())
            else {
                ui.toast = Some(("history: no selection".to_string(), Instant::now()));
                return true;
            };
            if focus_session_in_sessions(ui, snapshot, sid.as_str()) {
                ui.toast = Some((
                    format!("sessions: focused {}", short_sid(sid.as_str(), 18)),
                    Instant::now(),
                ));
            } else {
                ui.toast = Some((
                    crate::tui::i18n::pick(
                        ui.language,
                        "sessions: 当前 runtime 未观测到这个 session",
                        "sessions: this session is not currently observed in runtime",
                    )
                    .to_string(),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Char('f') if ui.page == Page::History => {
            let Some(summary) = ui
                .codex_history_sessions
                .get(ui.selected_codex_history_idx)
                .cloned()
            else {
                ui.toast = Some(("history: no selection".to_string(), Instant::now()));
                return true;
            };
            let sid_label = short_sid(summary.id.as_str(), 18);
            prepare_select_requests_for_session(ui, summary.id);
            ui.toast = Some((
                format!("requests: focused session {sid_label}"),
                Instant::now(),
            ));
            true
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::History => {
            let len = ui.codex_history_sessions.len();
            if let Some(next) = adjust_table_selection(&mut ui.codex_history_table, -1, len) {
                ui.selected_codex_history_idx = next;
                ui.selected_codex_history_id = ui
                    .codex_history_sessions
                    .get(next)
                    .map(|summary| summary.id.clone());
                return true;
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::History => {
            let len = ui.codex_history_sessions.len();
            if let Some(next) = adjust_table_selection(&mut ui.codex_history_table, 1, len) {
                ui.selected_codex_history_idx = next;
                ui.selected_codex_history_id = ui
                    .codex_history_sessions
                    .get(next)
                    .map(|summary| summary.id.clone());
                return true;
            }
            false
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Recent => {
            let now = now_ms();
            let threshold_ms = codex_recent_window_threshold_ms(now, ui.codex_recent_window_idx);
            let len = ui
                .codex_recent_rows
                .iter()
                .filter(|r| r.mtime_ms >= threshold_ms)
                .count();
            if let Some(next) = adjust_table_selection(&mut ui.codex_recent_table, -1, len) {
                ui.codex_recent_selected_idx = next;
                ui.codex_recent_selected_id = ui
                    .codex_recent_rows
                    .iter()
                    .filter(|r| r.mtime_ms >= threshold_ms)
                    .nth(next)
                    .map(|r| r.session_id.clone());
                return true;
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Recent => {
            let now = now_ms();
            let threshold_ms = codex_recent_window_threshold_ms(now, ui.codex_recent_window_idx);
            let len = ui
                .codex_recent_rows
                .iter()
                .filter(|r| r.mtime_ms >= threshold_ms)
                .count();
            if let Some(next) = adjust_table_selection(&mut ui.codex_recent_table, 1, len) {
                ui.codex_recent_selected_idx = next;
                ui.codex_recent_selected_id = ui
                    .codex_recent_rows
                    .iter()
                    .filter(|r| r.mtime_ms >= threshold_ms)
                    .nth(next)
                    .map(|r| r.session_id.clone());
                return true;
            }
            false
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Sessions => {
            let filtered = snapshot
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| {
                    if ui.sessions_page_active_only && row.active_count == 0 {
                        return false;
                    }
                    if ui.sessions_page_errors_only && row.last_status.is_some_and(|s| s < 400) {
                        return false;
                    }
                    if ui.sessions_page_overrides_only && !session_row_has_any_override(row) {
                        return false;
                    }
                    true
                })
                .take(200)
                .map(|(idx, _)| idx)
                .collect::<Vec<_>>();

            let len = filtered.len();
            if let Some(next) = adjust_table_selection(&mut ui.sessions_page_table, -1, len) {
                ui.selected_sessions_page_idx = next;
                if let Some(&row_idx) = filtered.get(next) {
                    apply_selected_session(ui, snapshot, row_idx);
                }
                return true;
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Sessions => {
            let filtered = snapshot
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| {
                    if ui.sessions_page_active_only && row.active_count == 0 {
                        return false;
                    }
                    if ui.sessions_page_errors_only && row.last_status.is_some_and(|s| s < 400) {
                        return false;
                    }
                    if ui.sessions_page_overrides_only && !session_row_has_any_override(row) {
                        return false;
                    }
                    true
                })
                .take(200)
                .map(|(idx, _)| idx)
                .collect::<Vec<_>>();

            let len = filtered.len();
            if let Some(next) = adjust_table_selection(&mut ui.sessions_page_table, 1, len) {
                ui.selected_sessions_page_idx = next;
                if let Some(&row_idx) = filtered.get(next) {
                    apply_selected_session(ui, snapshot, row_idx);
                }
                return true;
            }
            false
        }
        KeyCode::Char('e') if ui.page == Page::Requests => {
            ui.request_page_errors_only = !ui.request_page_errors_only;
            ui.selected_request_page_idx = 0;
            ui.toast = Some((
                format!(
                    "requests filter: errors_only={}",
                    ui.request_page_errors_only
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('s') if ui.page == Page::Requests => {
            ui.request_page_scope_session = !ui.request_page_scope_session;
            if ui.request_page_scope_session && ui.focused_request_session_id.is_none() {
                ui.focused_request_session_id = snapshot
                    .rows
                    .get(ui.selected_session_idx)
                    .and_then(|row| row.session_id.clone());
            }
            ui.selected_request_page_idx = 0;
            ui.toast = Some((
                format!(
                    "requests scope: {}",
                    if ui.request_page_scope_session {
                        "selected session"
                    } else {
                        "all"
                    }
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('x') if ui.page == Page::Requests => {
            clear_request_page_focus(ui);
            ui.toast = Some((
                "requests: cleared explicit session focus".to_string(),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('o') if ui.page == Page::Requests => {
            let Some(request) = selected_request_page_request(snapshot, ui) else {
                ui.toast = Some(("requests: no selection".to_string(), Instant::now()));
                return true;
            };
            let Some(sid) = request.session_id.as_deref() else {
                ui.toast = Some((
                    "requests: selected request has no session id".to_string(),
                    Instant::now(),
                ));
                return true;
            };
            if focus_session_in_sessions(ui, snapshot, sid) {
                ui.toast = Some((
                    format!("sessions: focused {}", short_sid(sid, 18)),
                    Instant::now(),
                ));
            } else {
                ui.toast = Some((
                    crate::tui::i18n::pick(
                        ui.language,
                        "sessions: 当前 runtime 未观测到这个 session",
                        "sessions: this session is not currently observed in runtime",
                    )
                    .to_string(),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Char('h') if ui.page == Page::Requests => {
            let Some(request) = selected_request_page_request(snapshot, ui).cloned() else {
                ui.toast = Some(("requests: no selection".to_string(), Instant::now()));
                return true;
            };
            let Some(sid) = request.session_id.as_deref() else {
                ui.toast = Some((
                    "requests: selected request has no session id".to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let path = match find_codex_session_file_by_id(sid).await {
                Ok(path) => path,
                Err(e) => {
                    ui.toast = Some((
                        format!("history: resolve session file failed: {e}"),
                        Instant::now(),
                    ));
                    return true;
                }
            };
            let Some(summary) = request_history_summary_from_request(&request, path) else {
                ui.toast = Some((
                    "history: failed to prepare request focus".to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let sid_label = short_sid(sid, 18);
            prepare_select_history_from_external(
                ui,
                summary,
                CodexHistoryExternalFocusOrigin::Requests,
            );
            ui.toast = Some((
                format!("history: focused session {sid_label}"),
                Instant::now(),
            ));
            true
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Requests => {
            let filtered_len = filtered_request_page_len(
                snapshot,
                ui.focused_request_session_id.as_deref(),
                ui.selected_session_idx,
                ui.request_page_errors_only,
                ui.request_page_scope_session,
            );
            if let Some(next) = adjust_table_selection(&mut ui.request_page_table, -1, filtered_len)
            {
                ui.selected_request_page_idx = next;
                return true;
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Requests => {
            let filtered_len = filtered_request_page_len(
                snapshot,
                ui.focused_request_session_id.as_deref(),
                ui.selected_session_idx,
                ui.request_page_errors_only,
                ui.request_page_scope_session,
            );
            if let Some(next) = adjust_table_selection(&mut ui.request_page_table, 1, filtered_len)
            {
                ui.selected_request_page_idx = next;
                return true;
            }
            false
        }
        KeyCode::Up | KeyCode::Char('k') => match ui.focus {
            Focus::Sessions => {
                if let Some(next) =
                    adjust_table_selection(&mut ui.sessions_table, -1, snapshot.rows.len())
                {
                    apply_selected_session(ui, snapshot, next);
                    return true;
                }
                false
            }
            Focus::Requests => {
                let filtered_len = filtered_requests_len(snapshot, ui.selected_session_idx);
                if let Some(next) = adjust_table_selection(&mut ui.requests_table, -1, filtered_len)
                {
                    ui.selected_request_idx = next;
                    return true;
                }
                false
            }
            Focus::Stations => false,
        },
        KeyCode::Down | KeyCode::Char('j') => match ui.focus {
            Focus::Sessions => {
                if let Some(next) =
                    adjust_table_selection(&mut ui.sessions_table, 1, snapshot.rows.len())
                {
                    apply_selected_session(ui, snapshot, next);
                    return true;
                }
                false
            }
            Focus::Requests => {
                let filtered_len = filtered_requests_len(snapshot, ui.selected_session_idx);
                if let Some(next) = adjust_table_selection(&mut ui.requests_table, 1, filtered_len)
                {
                    ui.selected_request_idx = next;
                    return true;
                }
                false
            }
            Focus::Stations => false,
        },
        KeyCode::Enter => {
            if ui.focus != Focus::Sessions {
                return false;
            }
            if snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.as_deref())
                .is_none()
            {
                return false;
            }

            ui.overlay = Overlay::EffortMenu;
            let current = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.override_effort.as_deref())
                .unwrap_or("");
            ui.effort_menu_idx = match current {
                "low" => 1,
                "medium" => 2,
                "high" => 3,
                "xhigh" => 4,
                _ => 0,
            };
            true
        }
        KeyCode::Char('l') | KeyCode::Char('m') | KeyCode::Char('h') | KeyCode::Char('X') => {
            if ui.focus != Focus::Sessions {
                return false;
            }
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                return false;
            };
            let eff = match key.code {
                KeyCode::Char('l') => Some("low"),
                KeyCode::Char('m') => Some("medium"),
                KeyCode::Char('h') => Some("high"),
                KeyCode::Char('X') => Some("xhigh"),
                _ => None,
            }
            .map(|s| s.to_string());
            apply_effort_override(state, sid, eff.clone()).await;
            ui.toast = Some((
                format!("effort override: {}", eff.as_deref().unwrap_or("<clear>")),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('x') => {
            if ui.focus != Focus::Sessions {
                return false;
            }
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                return false;
            };
            apply_effort_override(state, sid, None).await;
            ui.toast = Some(("effort override cleared".to_string(), Instant::now()));
            true
        }
        KeyCode::Char('p') => {
            if ui.focus != Focus::Sessions {
                return false;
            }
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                return false;
            };
            let current = snapshot
                .station_overrides
                .get(&sid)
                .map(|s| s.as_str())
                .unwrap_or("");
            ui.provider_menu_idx = providers
                .iter()
                .position(|p| p.name == current)
                .map(|i| i + 1)
                .unwrap_or(0);
            ui.overlay = Overlay::ProviderMenuSession;
            true
        }
        KeyCode::Char('P') => {
            let current = snapshot
                .global_station_override
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| providers.iter().find(|p| p.active).map(|p| p.name.as_str()))
                .unwrap_or("");
            ui.provider_menu_idx = providers
                .iter()
                .position(|p| p.name == current)
                .map(|i| i + 1)
                .unwrap_or(0);
            ui.overlay = Overlay::ProviderMenuGlobal;
            true
        }
        _ => false,
    }
}

async fn handle_key_effort_menu(
    state: &ProxyState,
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.effort_menu_idx = ui.effort_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            ui.effort_menu_idx = (ui.effort_menu_idx + 1).min(4);
            true
        }
        KeyCode::Enter => {
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                ui.overlay = Overlay::None;
                return true;
            };
            let choice = match ui.effort_menu_idx {
                1 => EffortChoice::Low,
                2 => EffortChoice::Medium,
                3 => EffortChoice::High,
                4 => EffortChoice::XHigh,
                _ => EffortChoice::Clear,
            };
            apply_effort_override(state, sid, choice.value().map(|s| s.to_string())).await;
            ui.overlay = Overlay::None;
            ui.toast = Some((format!("effort set: {}", choice.label()), Instant::now()));
            true
        }
        _ => false,
    }
}

async fn handle_key_profile_menu(
    state: &ProxyState,
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.profile_menu_idx = ui.profile_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let max = profile_menu_max_idx(&ui.profile_options);
            ui.profile_menu_idx = (ui.profile_menu_idx + 1).min(max);
            true
        }
        KeyCode::Enter => {
            let chosen = if ui.profile_menu_idx == 0 {
                None
            } else {
                ui.profile_options
                    .get(ui.profile_menu_idx.saturating_sub(1))
                    .map(|profile| profile.name.clone())
            };

            match ui.overlay {
                Overlay::ProfileMenuSession => {
                    let Some(sid) = snapshot
                        .rows
                        .get(ui.selected_session_idx)
                        .and_then(|row| row.session_id.clone())
                    else {
                        ui.overlay = Overlay::None;
                        return true;
                    };

                    if let Some(profile_name) = chosen {
                        match apply_session_profile(
                            state,
                            ui.service_name,
                            sid,
                            profile_name.clone(),
                        )
                        .await
                        {
                            Ok(()) => {
                                ui.needs_snapshot_refresh = true;
                                ui.toast = Some((
                                    format!("profile applied: {profile_name}"),
                                    Instant::now(),
                                ));
                            }
                            Err(err) => {
                                ui.toast =
                                    Some((format!("profile apply failed: {err}"), Instant::now()));
                            }
                        }
                    } else {
                        state.clear_session_binding(&sid).await;
                        ui.needs_snapshot_refresh = true;
                        ui.toast = Some(("profile binding cleared".to_string(), Instant::now()));
                    }
                }
                Overlay::ProfileMenuDefaultRuntime => {
                    match apply_runtime_default_profile(ui, chosen.clone()).await {
                        Ok(()) => match refresh_profile_control_state(ui).await {
                            Ok(()) => {
                                ui.toast = Some((
                                    format!(
                                        "runtime default profile: {}",
                                        default_profile_label(
                                            ui.runtime_default_profile_override.as_deref(),
                                            "<configured fallback>",
                                        )
                                    ),
                                    Instant::now(),
                                ));
                            }
                            Err(err) => {
                                ui.toast = Some((
                                    format!("runtime default profile refresh failed: {err}"),
                                    Instant::now(),
                                ));
                            }
                        },
                        Err(err) => {
                            ui.toast = Some((
                                format!("runtime default profile apply failed: {err}"),
                                Instant::now(),
                            ));
                        }
                    }
                }
                Overlay::ProfileMenuDefaultPersisted => {
                    match apply_persisted_default_profile(ui, chosen.clone()).await {
                        Ok(()) => match refresh_profile_control_state(ui).await {
                            Ok(()) => {
                                ui.toast = Some((
                                    format!(
                                        "configured default profile: {}",
                                        default_profile_label(
                                            ui.configured_default_profile.as_deref(),
                                            "<none>",
                                        )
                                    ),
                                    Instant::now(),
                                ));
                            }
                            Err(err) => {
                                ui.toast = Some((
                                    format!("configured default profile refresh failed: {err}"),
                                    Instant::now(),
                                ));
                            }
                        },
                        Err(err) => {
                            ui.toast = Some((
                                format!("configured default profile apply failed: {err}"),
                                Instant::now(),
                            ));
                        }
                    }
                }
                _ => {}
            }
            ui.overlay = Overlay::None;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::default_profile_menu_idx;
    use crate::dashboard_core::ControlProfileOption;

    fn make_profile(name: &str) -> ControlProfileOption {
        ControlProfileOption {
            name: name.to_string(),
            extends: None,
            station: None,
            model: None,
            reasoning_effort: None,
            service_tier: None,
            fast_mode: false,
            is_default: false,
        }
    }

    #[test]
    fn default_profile_menu_idx_offsets_bound_profile_selection() {
        let profiles = vec![make_profile("balanced"), make_profile("fast")];

        assert_eq!(default_profile_menu_idx(&profiles, Some("fast")), 2);
    }

    #[test]
    fn default_profile_menu_idx_falls_back_to_clear_for_missing_binding() {
        let profiles = vec![make_profile("balanced"), make_profile("fast")];

        assert_eq!(default_profile_menu_idx(&profiles, Some("missing")), 0);
    }

    #[test]
    fn default_profile_menu_idx_prefers_first_profile_when_unbound() {
        let profiles = vec![make_profile("balanced"), make_profile("fast")];

        assert_eq!(default_profile_menu_idx(&profiles, None), 1);
        assert_eq!(default_profile_menu_idx(&[], None), 0);
    }
}

async fn handle_key_service_tier_menu(
    state: &ProxyState,
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.service_tier_menu_idx = ui.service_tier_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            ui.service_tier_menu_idx = (ui.service_tier_menu_idx + 1).min(4);
            true
        }
        KeyCode::Enter => {
            if ui.service_tier_menu_idx == 4 {
                ui.session_service_tier_input =
                    current_service_tier_override(snapshot, ui).unwrap_or_default();
                ui.session_service_tier_input_hint =
                    selected_session_service_tier_hint(snapshot, ui);
                ui.overlay = Overlay::ServiceTierInputSession;
                return true;
            }

            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|row| row.session_id.clone())
            else {
                ui.overlay = Overlay::None;
                return true;
            };
            let choice = match ui.service_tier_menu_idx {
                1 => ServiceTierChoice::Default,
                2 => ServiceTierChoice::Priority,
                3 => ServiceTierChoice::Flex,
                _ => ServiceTierChoice::Clear,
            };
            apply_service_tier_override(state, sid, choice.value().map(|s| s.to_string())).await;
            ui.overlay = Overlay::None;
            ui.toast = Some((
                format!("service_tier set: {}", choice.label()),
                Instant::now(),
            ));
            true
        }
        _ => false,
    }
}

async fn handle_key_service_tier_input(
    state: &ProxyState,
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::ServiceTierMenuSession;
            true
        }
        KeyCode::Enter => {
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|row| row.session_id.clone())
            else {
                ui.overlay = Overlay::None;
                return true;
            };
            let value = ui.session_service_tier_input.trim().to_string();
            let tier = if value.is_empty() { None } else { Some(value) };
            apply_service_tier_override(state, sid, tier.clone()).await;
            ui.overlay = Overlay::None;
            ui.toast = Some((
                format!("service_tier set: {}", tier.as_deref().unwrap_or("<clear>")),
                Instant::now(),
            ));
            true
        }
        KeyCode::Backspace => {
            ui.session_service_tier_input.pop();
            true
        }
        KeyCode::Delete => {
            ui.session_service_tier_input.clear();
            true
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            ui.session_service_tier_input.clear();
            true
        }
        KeyCode::Char(ch)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            ui.session_service_tier_input.push(ch);
            true
        }
        _ => false,
    }
}

async fn handle_key_model_menu(
    state: &ProxyState,
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.model_menu_idx = ui.model_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let max = ui.session_model_options.len() + 1;
            ui.model_menu_idx = (ui.model_menu_idx + 1).min(max);
            true
        }
        KeyCode::Enter => {
            if ui.model_menu_idx == ui.session_model_options.len() + 1 {
                ui.session_model_input = current_model_override(snapshot, ui).unwrap_or_default();
                ui.session_model_input_hint = selected_session_model_hint(snapshot, ui);
                ui.overlay = Overlay::ModelInputSession;
                return true;
            }

            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|row| row.session_id.clone())
            else {
                ui.overlay = Overlay::None;
                return true;
            };
            let model = if ui.model_menu_idx == 0 {
                None
            } else {
                ui.session_model_options.get(ui.model_menu_idx - 1).cloned()
            };
            apply_model_override(state, sid, model.clone()).await;
            ui.overlay = Overlay::None;
            ui.toast = Some((
                format!("model override: {}", model.as_deref().unwrap_or("<clear>")),
                Instant::now(),
            ));
            true
        }
        _ => false,
    }
}

async fn handle_key_model_input(
    state: &ProxyState,
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::ModelMenuSession;
            true
        }
        KeyCode::Enter => {
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|row| row.session_id.clone())
            else {
                ui.overlay = Overlay::None;
                return true;
            };
            let value = ui.session_model_input.trim().to_string();
            let model = if value.is_empty() { None } else { Some(value) };
            apply_model_override(state, sid, model.clone()).await;
            ui.overlay = Overlay::None;
            ui.toast = Some((
                format!("model override: {}", model.as_deref().unwrap_or("<clear>")),
                Instant::now(),
            ));
            true
        }
        KeyCode::Backspace => {
            ui.session_model_input.pop();
            true
        }
        KeyCode::Delete => {
            ui.session_model_input.clear();
            true
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            ui.session_model_input.clear();
            true
        }
        KeyCode::Char(ch)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            ui.session_model_input.push(ch);
            true
        }
        _ => false,
    }
}

async fn handle_key_provider_menu(
    state: &ProxyState,
    providers: &mut Vec<ProviderOption>,
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.provider_menu_idx = ui.provider_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let max = providers.len();
            ui.provider_menu_idx = (ui.provider_menu_idx + 1).min(max);
            true
        }
        KeyCode::Enter => {
            let idx = ui.provider_menu_idx;
            let chosen = if idx == 0 {
                None
            } else {
                providers.get(idx - 1).map(|p| p.name.clone())
            };

            match ui.overlay {
                Overlay::ProviderMenuGlobal => {
                    match apply_global_active_station(
                        state,
                        providers,
                        ui,
                        snapshot,
                        chosen.clone(),
                    )
                    .await
                    {
                        Ok(()) => {
                            let active = providers
                                .iter()
                                .find(|p| p.active)
                                .map(|p| p.name.as_str())
                                .unwrap_or("<auto>");
                            ui.toast = Some((format!("active station: {active}"), Instant::now()));
                        }
                        Err(err) => {
                            ui.toast = Some((format!("set active failed: {err}"), Instant::now()));
                        }
                    }
                }
                Overlay::ProviderMenuSession => {
                    let Some(sid) = snapshot
                        .rows
                        .get(ui.selected_session_idx)
                        .and_then(|r| r.session_id.clone())
                    else {
                        ui.overlay = Overlay::None;
                        return true;
                    };
                    apply_session_provider_override(state, sid, chosen.clone()).await;
                    ui.toast = Some((
                        format!(
                            "session station override: {}",
                            chosen.as_deref().unwrap_or("<clear>")
                        ),
                        Instant::now(),
                    ));
                }
                _ => {}
            }

            ui.overlay = Overlay::None;
            true
        }
        _ => false,
    }
}

use std::time::Instant;

use codex_helper_core::codex_switch::{self, CodexSwitchIntent, ValidatedCodexBaseUrl};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::proxy_home_dir;
use crate::proxy::{OperatorRoutingCommand, OperatorRoutingMutationRequest};
use crate::sessions::find_codex_session_file_by_id;
use crate::tui::Language;
use crate::tui::i18n::{self, msg};
use crate::tui::model::{
    CODEX_RECENT_WINDOWS, Snapshot, codex_recent_window_label, codex_recent_window_threshold_ms,
    filtered_requests_len, find_session_idx, now_ms, short_sid,
};
use crate::tui::report::build_stats_report;
use crate::tui::state::{
    CodexHistoryExternalFocusOrigin, FleetViewMode, UiState, adjust_table_selection,
};
use crate::tui::types::{Focus, Overlay, Page};

use super::KeyEventContext;
use super::history_bridge::{
    host_transcript_path_from_row, local_session_id_for_opaque_key,
    prepare_select_history_from_external, recent_history_summary_from_row,
    request_history_summary_from_request, selected_dashboard_request, selected_recent_row,
    selected_request_page_request, session_history_summary_from_row,
};
use super::transcript::open_session_transcript_from_path;
use crate::tui::operator_actions::{notify_read_only_operator_action, queue_balance_refresh};

pub(in crate::tui) fn codex_switch_intent_for_key(
    code: KeyCode,
    proxy_port: u16,
) -> Option<CodexSwitchIntent> {
    match code {
        KeyCode::Char('n') => Some(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(proxy_port),
        }),
        KeyCode::Char('o') => Some(CodexSwitchIntent::Off),
        _ => None,
    }
}

pub(in crate::tui) fn apply_codex_switch(ui: &mut UiState, intent: CodexSwitchIntent) {
    let action = if matches!(&intent, CodexSwitchIntent::On { .. }) {
        "on"
    } else {
        "off"
    };
    let message = match codex_switch::apply(intent) {
        Ok(outcome) => match ui.language {
            Language::Zh => format!(
                "Codex 本地 switch {action}：{}（phase={}）；重启已有 Codex app 后生效",
                outcome.change.as_str(),
                outcome.status.phase.as_str()
            ),
            Language::En => format!(
                "Codex local switch {action}: {} (phase={}); restart existing Codex apps to apply it",
                outcome.change.as_str(),
                outcome.status.phase.as_str()
            ),
        },
        Err(err) => match ui.language {
            Language::Zh => format!("Codex 本地 switch {action} 失败：{err}"),
            Language::En => format!("Codex local switch {action} failed: {err}"),
        },
    };
    ui.toast = Some((message, Instant::now()));
}

pub(super) fn apply_page_shortcuts(ui: &mut UiState, code: KeyCode) -> bool {
    let previous_page = ui.page;
    let page = match code {
        KeyCode::Char('1') => Some(Page::Dashboard),
        KeyCode::Char('2') => Some(Page::Routing),
        KeyCode::Char('3') => Some(Page::Sessions),
        KeyCode::Char('4') => Some(Page::Requests),
        KeyCode::Char('5') => Some(Page::Stats),
        KeyCode::Char('6') => Some(Page::ServiceStatus),
        KeyCode::Char('7') => Some(Page::Settings),
        KeyCode::Char('8') => Some(Page::History),
        KeyCode::Char('9') => Some(Page::Recent),
        KeyCode::Char('0') => Some(Page::Fleet),
        _ => None,
    };
    if let Some(p) = page {
        ui.page = p;
        if previous_page == Page::Stats || ui.page == Page::Stats || ui.page == Page::ServiceStatus
        {
            ui.needs_snapshot_refresh = true;
        }
        if ui.page == Page::Routing {
            ui.focus = Focus::Providers;
            if previous_page != Page::Routing {
                let _ = queue_balance_refresh(ui, false, false);
            }
        } else if ui.page == Page::Requests {
            ui.focus = Focus::Requests;
        } else if ui.page == Page::Sessions
            || ui.page == Page::History
            || ui.page == Page::Recent
            || (ui.page == Page::Dashboard && ui.focus == Focus::Providers)
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
        if ui.page == Page::Fleet {
            ui.needs_fleet_refresh = true;
            ui.sync_fleet_selection();
        }
        return true;
    }
    false
}

pub(in crate::tui) fn handle_routing_operator_key(
    ui: &mut UiState,
    snapshot: &Snapshot,
    code: KeyCode,
) -> bool {
    if ui.page != Page::Routing {
        return false;
    }
    match code {
        KeyCode::Char('g') => {
            let _ = queue_balance_refresh(ui, true, true);
            true
        }
        KeyCode::Enter | KeyCode::Char('m') => {
            if !ui.can_mutate_routing() {
                notify_read_only_operator_action(ui);
                return true;
            }
            let Some(routing) = snapshot.routing.as_ref() else {
                ui.toast = Some((
                    match ui.language {
                        Language::Zh => "当前 daemon 未提供可写路由摘要".to_string(),
                        Language::En => {
                            "the daemon did not provide a mutable routing summary".to_string()
                        }
                    },
                    Instant::now(),
                ));
                return true;
            };
            let Some(candidate) = ui.selected_routing_candidate(snapshot) else {
                ui.toast = Some((
                    match ui.language {
                        Language::Zh => "没有选中的候选端点".to_string(),
                        Language::En => "no endpoint candidate is selected".to_string(),
                    },
                    Instant::now(),
                ));
                return true;
            };
            let _ = routing;
            let _ = candidate;
            ui.routing_action_selected_idx = if code == KeyCode::Char('m') { 2 } else { 0 };
            ui.overlay = Overlay::RoutingActions;
            true
        }
        KeyCode::Char('a') | KeyCode::Backspace | KeyCode::Delete => {
            if !ui.can_mutate_routing() {
                notify_read_only_operator_action(ui);
                return true;
            }
            let Some(routing) = snapshot.routing.as_ref() else {
                ui.toast = Some((
                    match ui.language {
                        Language::Zh => "当前 daemon 未提供可写路由摘要".to_string(),
                        Language::En => {
                            "the daemon did not provide a mutable routing summary".to_string()
                        }
                    },
                    Instant::now(),
                ));
                return true;
            };
            if routing.new_session_preference.is_none() {
                ui.toast = Some((
                    match ui.language {
                        Language::Zh => "当前已经使用自动路由".to_string(),
                        Language::En => "automatic routing is already active".to_string(),
                    },
                    Instant::now(),
                ));
                return true;
            }
            let request = routing_mutation_request(
                routing,
                OperatorRoutingCommand::ClearNewSessionPreference,
            );
            ui.routing_confirmation = Some(request);
            ui.overlay = Overlay::RoutingConfirmation;
            true
        }
        _ => false,
    }
}

pub(in crate::tui) fn handle_session_affinity_operator_key(
    ui: &mut UiState,
    snapshot: &Snapshot,
    code: KeyCode,
) -> bool {
    if ui.page != Page::Sessions || code != KeyCode::Enter {
        return false;
    }
    if !ui.can_mutate_session_affinity() {
        notify_read_only_operator_action(ui);
        return true;
    }
    let Some(row) = snapshot.rows.get(ui.selected_session_idx) else {
        ui.toast = Some((
            match ui.language {
                Language::Zh => "没有选中的会话".to_string(),
                Language::En => "no session is selected".to_string(),
            },
            Instant::now(),
        ));
        return true;
    };
    if row.active_count > 0 {
        ui.toast = Some((
            match ui.language {
                Language::Zh => "会话仍有进行中的请求；只能在空闲后更改 affinity".to_string(),
                Language::En => {
                    "the session has an active request; affinity can only change when idle"
                        .to_string()
                }
            },
            Instant::now(),
        ));
        return true;
    }
    let Some(affinity) = row.route_affinity.as_ref() else {
        ui.toast = Some((
            match ui.language {
                Language::Zh => "会话尚无 affinity；下一次请求会自动选择路由".to_string(),
                Language::En => {
                    "the session has no affinity; its next request will be routed automatically"
                        .to_string()
                }
            },
            Instant::now(),
        ));
        return true;
    };
    if affinity.revision.trim().is_empty() {
        ui.toast = Some((
            match ui.language {
                Language::Zh => "daemon 未提供可写 affinity 控制元数据；已降级为只读".to_string(),
                Language::En => {
                    "the daemon did not provide mutable affinity control metadata; read-only mode"
                        .to_string()
                }
            },
            Instant::now(),
        ));
        return true;
    }
    let Some(routing) = snapshot.routing.as_ref() else {
        ui.toast = Some((
            match ui.language {
                Language::Zh => "当前 daemon 未提供可写路由摘要".to_string(),
                Language::En => "the daemon did not provide a mutable routing summary".to_string(),
            },
            Instant::now(),
        ));
        return true;
    };
    ui.session_affinity_action_selected_idx =
        if routing.entry_strategy == crate::config::RouteStrategy::Conditional {
            0
        } else {
            routing
                .candidates
                .iter()
                .position(|candidate| {
                    candidate.provider_id == affinity.provider_id
                        && candidate.endpoint_id == affinity.endpoint_id
                })
                .map(|index| index + 1)
                .unwrap_or_else(|| usize::from(!routing.candidates.is_empty()))
        };
    ui.overlay = Overlay::SessionAffinityActions;
    true
}

pub(in crate::tui) fn routing_mutation_request(
    routing: &crate::dashboard_core::OperatorRoutingSummary,
    command: OperatorRoutingCommand,
) -> OperatorRoutingMutationRequest {
    OperatorRoutingMutationRequest {
        expected_route_graph_key: routing.route_graph_key.clone(),
        expected_control_revision: routing.control_revision,
        expected_policy_revision: routing.provider_policy_revision,
        command,
    }
}

pub(in crate::tui) fn move_routing_page_selection(
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers_len: usize,
    delta: i32,
) -> bool {
    if snapshot.routing.is_some() {
        return ui.move_routing_selection(snapshot, delta);
    }
    let Some(next) = adjust_table_selection(&mut ui.providers_table, delta, providers_len) else {
        return false;
    };
    ui.selected_provider_idx = next;
    true
}

pub(in crate::tui) fn select_routing_page_edge(
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers_len: usize,
    last: bool,
) -> bool {
    if let Some(routing) = snapshot.routing.as_ref() {
        let index = if last {
            routing.candidates.len().saturating_sub(1)
        } else {
            0
        };
        return ui.select_routing_candidate_index(snapshot, index);
    }
    if providers_len == 0 {
        return false;
    }
    let index = if last {
        providers_len.saturating_sub(1)
    } else {
        0
    };
    ui.selected_provider_idx = index;
    ui.providers_table.select(Some(index));
    true
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

fn focus_session_in_sessions(ui: &mut UiState, snapshot: &Snapshot, sid: &str) -> bool {
    let Some(idx) = find_session_idx(snapshot, sid) else {
        return false;
    };

    ui.sessions_page_active_only = false;
    ui.sessions_page_errors_only = false;
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

fn move_fleet_selection(ui: &mut UiState, delta: i32) -> bool {
    let Some(snapshot) = ui.fleet_snapshot.as_ref() else {
        return false;
    };

    if ui.focus == Focus::Providers {
        if let Some(next) =
            adjust_table_selection(&mut ui.fleet_nodes_table, delta, snapshot.nodes.len())
        {
            ui.selected_fleet_node_idx = next;
            ui.selected_fleet_node_id = snapshot.nodes.get(next).map(|node| node.node_id.clone());
            ui.selected_fleet_unit_idx = 0;
            ui.selected_fleet_unit_id = snapshot
                .nodes
                .get(next)
                .and_then(|node| node.work_units.first())
                .map(|unit| unit.id.clone());
            let unit_len = snapshot
                .nodes
                .get(next)
                .map(|node| node.work_units.len())
                .unwrap_or(0);
            ui.fleet_units_table
                .select((unit_len > 0).then_some(ui.selected_fleet_unit_idx));
            *ui.fleet_units_table.offset_mut() = 0;
            return true;
        }
        return false;
    }

    let unit_len = snapshot
        .nodes
        .get(ui.selected_fleet_node_idx)
        .map(|node| node.work_units.len())
        .unwrap_or(0);
    if let Some(next) = adjust_table_selection(&mut ui.fleet_units_table, delta, unit_len) {
        ui.selected_fleet_unit_idx = next;
        ui.selected_fleet_unit_id = snapshot
            .nodes
            .get(ui.selected_fleet_node_idx)
            .and_then(|node| node.work_units.get(next))
            .map(|unit| unit.id.clone());
        return true;
    }
    false
}

pub(super) fn toggle_language(ui: &mut UiState) {
    let next = i18n::next_language(ui.language);
    ui.language = next;
    ui.toast = Some((
        i18n::format_language_changed(ui.language, next),
        Instant::now(),
    ));
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

pub(super) fn try_copy_to_clipboard(report: &str) -> anyhow::Result<()> {
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

pub(in crate::tui) fn export_selected_stats_report(ui: &mut UiState, snapshot: &Snapshot) -> bool {
    let now = now_ms();
    let Some(report) = build_stats_report(ui, snapshot, now) else {
        ui.toast = Some((
            match ui.language {
                Language::Zh => "stats report: 未选择条目",
                Language::En => "stats report: no selection",
            }
            .to_string(),
            Instant::now(),
        ));
        return true;
    };
    let saved = write_report(&report, now);
    let copied = try_copy_to_clipboard(&report);

    match (saved, copied) {
        (Ok(path), Ok(())) => {
            ui.toast = Some((
                match ui.language {
                    Language::Zh => format!("stats report: 已复制并保存 {}", path.display()),
                    Language::En => format!("stats report: copied + saved {}", path.display()),
                },
                Instant::now(),
            ));
        }
        (Ok(path), Err(err)) => {
            ui.toast = Some((
                match ui.language {
                    Language::Zh => {
                        format!("stats report: 已保存 {}（复制失败：{err}）", path.display())
                    }
                    Language::En => {
                        format!(
                            "stats report: saved {} (copy failed: {err})",
                            path.display()
                        )
                    }
                },
                Instant::now(),
            ));
        }
        (Err(err), Ok(())) => {
            ui.toast = Some((
                match ui.language {
                    Language::Zh => format!("stats report: 已复制（保存失败：{err}）"),
                    Language::En => format!("stats report: copied (save failed: {err})"),
                },
                Instant::now(),
            ));
        }
        (Err(err1), Err(err2)) => {
            ui.toast = Some((
                match ui.language {
                    Language::Zh => {
                        format!("stats report: 复制失败：{err2}（保存失败：{err1}）")
                    }
                    Language::En => {
                        format!("stats report: copy failed: {err2} (save failed: {err1})")
                    }
                },
                Instant::now(),
            ));
        }
    }
    true
}

pub(super) async fn handle_key_normal(ctx: KeyEventContext<'_>, key: KeyEvent) -> bool {
    let KeyEventContext {
        providers,
        ui,
        snapshot,
    } = ctx;

    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
        ui.should_exit = true;
        return true;
    }

    if ui.page == Page::Settings
        && ui.allows_local_codex_switch()
        && let Some(intent) = codex_switch_intent_for_key(key.code, ui.proxy_port)
    {
        apply_codex_switch(ui, intent);
        return true;
    }

    if handle_routing_operator_key(ui, snapshot, key.code) {
        return true;
    }
    if handle_session_affinity_operator_key(ui, snapshot, key.code) {
        return true;
    }

    match key.code {
        KeyCode::Char('q') => {
            ui.should_exit = true;
            true
        }
        KeyCode::Char('L') => {
            toggle_language(ui);
            true
        }
        KeyCode::Char('?') => {
            ui.overlay = Overlay::Help;
            true
        }
        KeyCode::Char('i') if ui.page == Page::Routing => {
            ui.sync_selected_provider_from_routing(snapshot, providers);
            super::open_provider_info(ui);
            true
        }
        KeyCode::Tab => {
            if ui.page == Page::Dashboard {
                ui.focus = match ui.focus {
                    Focus::Sessions => Focus::Requests,
                    Focus::Requests => Focus::Sessions,
                    Focus::Providers => Focus::Sessions,
                };
            } else if ui.page == Page::Routing {
                ui.focus = Focus::Providers;
            } else if ui.page == Page::Stats {
                ui.cycle_stats_focus();
                ui.toast = Some((
                    format!(
                        "{}: {}",
                        i18n::label(ui.language, "focus"),
                        ui.stats_focus_label()
                    ),
                    Instant::now(),
                ));
            } else if ui.page == Page::Fleet {
                ui.focus = match ui.focus {
                    Focus::Providers => Focus::Sessions,
                    Focus::Sessions | Focus::Requests => Focus::Providers,
                };
                ui.toast = Some((
                    format!(
                        "{}: {}",
                        i18n::label(ui.language, "focus"),
                        if ui.focus == Focus::Providers {
                            i18n::label(ui.language, "nodes")
                        } else {
                            i18n::label(ui.language, "work units")
                        }
                    ),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Char('r') if ui.page == Page::Fleet => {
            ui.needs_fleet_refresh = true;
            ui.toast = Some((
                i18n::label(ui.language, "fleet: refreshing").to_string(),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('r') if ui.page == Page::ServiceStatus => {
            ui.needs_snapshot_refresh = true;
            ui.toast = Some((
                i18n::label(ui.language, "service status: reading snapshot").to_string(),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('t') if ui.page == Page::Fleet => {
            ui.fleet_view_mode = match ui.fleet_view_mode {
                FleetViewMode::Tree => FleetViewMode::Flat,
                FleetViewMode::Flat => FleetViewMode::Tree,
            };
            ui.toast = Some((
                format!(
                    "{}: {}",
                    i18n::label(ui.language, "fleet view"),
                    match ui.fleet_view_mode {
                        FleetViewMode::Tree => i18n::label(ui.language, "tree"),
                        FleetViewMode::Flat => i18n::label(ui.language, "flat"),
                    }
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Fleet => move_fleet_selection(ui, -1),
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Fleet => move_fleet_selection(ui, 1),
        KeyCode::Char('p') if ui.page == Page::Routing => {
            if ui.select_preferred_routing_candidate(snapshot) {
                true
            } else {
                ui.toast = Some((
                    match ui.language {
                        Language::Zh => "当前没有可定位的新会话偏好".to_string(),
                        Language::En => {
                            "there is no preferred new-session target to locate".to_string()
                        }
                    },
                    Instant::now(),
                ));
                true
            }
        }
        KeyCode::PageUp if ui.page == Page::Routing => {
            let delta = -(ui.routing_candidates_visible_rows.min(i32::MAX as usize) as i32);
            move_routing_page_selection(ui, snapshot, providers.len(), delta)
        }
        KeyCode::PageDown if ui.page == Page::Routing => {
            let delta = ui.routing_candidates_visible_rows.min(i32::MAX as usize) as i32;
            move_routing_page_selection(ui, snapshot, providers.len(), delta)
        }
        KeyCode::Home if ui.page == Page::Routing => {
            select_routing_page_edge(ui, snapshot, providers.len(), false)
        }
        KeyCode::End if ui.page == Page::Routing => {
            select_routing_page_edge(ui, snapshot, providers.len(), true)
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Routing => {
            move_routing_page_selection(ui, snapshot, providers.len(), -1)
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Routing => {
            move_routing_page_selection(ui, snapshot, providers.len(), 1)
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Stats => {
            ui.move_stats_selection(snapshot, -1)
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Stats => {
            ui.move_stats_selection(snapshot, 1)
        }
        KeyCode::Char('g') if ui.page == Page::Stats => {
            ui.needs_snapshot_refresh = true;
            ui.toast = Some((
                match ui.language {
                    Language::Zh => "额度：正在刷新 operator read model",
                    Language::En => "quota: refreshing operator read model",
                }
                .to_string(),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('y') if ui.page == Page::Stats => export_selected_stats_report(ui, snapshot),
        KeyCode::Char('a') if ui.page == Page::Sessions => {
            ui.sessions_page_active_only = !ui.sessions_page_active_only;
            ui.selected_sessions_page_idx = 0;
            ui.toast = Some((
                format!(
                    "{}: {}={}",
                    i18n::label(ui.language, "sessions filter"),
                    i18n::label(ui.language, "active_only"),
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
                    "{}: {}={}",
                    i18n::label(ui.language, "sessions filter"),
                    i18n::label(ui.language, "errors_only"),
                    ui.sessions_page_errors_only
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('r') if ui.page == Page::Sessions => {
            ui.sessions_page_active_only = false;
            ui.sessions_page_errors_only = false;
            ui.selected_sessions_page_idx = 0;
            ui.toast = Some((
                i18n::label(ui.language, "sessions filter: reset").to_string(),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('r') if ui.page == Page::History => {
            ui.needs_codex_history_refresh = true;
            ui.toast = Some((
                i18n::text(ui.language, msg::HISTORY_REFRESHING).to_string(),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('r') if ui.page == Page::Recent => {
            ui.needs_codex_recent_refresh = true;
            ui.toast = Some((
                i18n::text(ui.language, msg::RECENT_REFRESHING).to_string(),
                Instant::now(),
            ));
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
                    "{}: {}",
                    i18n::label(ui.language, "recent window"),
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
                    "{}: {}",
                    i18n::label(ui.language, "recent window"),
                    codex_recent_window_label(ui.codex_recent_window_idx)
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('o') if ui.page == Page::Sessions => {
            let Some(sid) = ui.selected_session_id.clone() else {
                ui.toast = Some((
                    i18n::label(ui.language, "sessions: no session selected").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let sid_label = short_sid(&sid, 18);
            prepare_select_requests_for_session(ui, sid);
            ui.toast = Some((
                format!(
                    "{} {sid_label}",
                    i18n::label(ui.language, "requests: focused session")
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('H') if ui.page == Page::Sessions => {
            let Some(row) = snapshot.rows.get(ui.selected_session_idx) else {
                ui.toast = Some((
                    i18n::label(ui.language, "sessions: no session selected").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let Some(local_sid) = row.local_command_session_id() else {
                ui.toast = Some((
                    i18n::label(
                        ui.language,
                        "sessions: selected row has no local session id",
                    )
                    .to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let path = if let Some(path) = host_transcript_path_from_row(row) {
                Some(path)
            } else {
                match find_codex_session_file_by_id(local_sid).await {
                    Ok(path) => path,
                    Err(e) => {
                        ui.toast = Some((
                            format!(
                                "{}: {e}",
                                i18n::label(ui.language, "history: resolve session file failed")
                            ),
                            Instant::now(),
                        ));
                        return true;
                    }
                }
            };
            let Some(summary) = session_history_summary_from_row(row, path) else {
                ui.toast = Some((
                    i18n::label(ui.language, "history: failed to prepare session focus")
                        .to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let sid_label = short_sid(row.session_id.as_deref().unwrap_or(local_sid), 18);
            prepare_select_history_from_external(
                ui,
                summary,
                CodexHistoryExternalFocusOrigin::Sessions,
            );
            ui.toast = Some((
                format!(
                    "{} {sid_label}",
                    i18n::label(ui.language, "history: focused session")
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('O') if ui.page == Page::Dashboard && ui.focus == Focus::Sessions => {
            let Some(sid) = ui.selected_session_id.clone() else {
                ui.toast = Some((
                    i18n::label(ui.language, "dashboard: no session selected").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let sid_label = short_sid(&sid, 18);
            prepare_select_requests_for_session(ui, sid);
            ui.toast = Some((
                format!(
                    "{} {sid_label}",
                    i18n::label(ui.language, "requests: focused session")
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('H') if ui.page == Page::Dashboard && ui.focus == Focus::Sessions => {
            let Some(row) = snapshot.rows.get(ui.selected_session_idx) else {
                ui.toast = Some((
                    i18n::label(ui.language, "dashboard: no session selected").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let Some(local_sid) = row.local_command_session_id() else {
                ui.toast = Some((
                    i18n::label(
                        ui.language,
                        "dashboard: selected row has no local session id",
                    )
                    .to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let path = if let Some(path) = host_transcript_path_from_row(row) {
                Some(path)
            } else {
                match find_codex_session_file_by_id(local_sid).await {
                    Ok(path) => path,
                    Err(e) => {
                        ui.toast = Some((
                            format!(
                                "{}: {e}",
                                i18n::label(ui.language, "history: resolve session file failed")
                            ),
                            Instant::now(),
                        ));
                        return true;
                    }
                }
            };
            let Some(summary) = session_history_summary_from_row(row, path) else {
                ui.toast = Some((
                    i18n::label(ui.language, "history: failed to prepare session focus")
                        .to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let sid_label = short_sid(row.session_id.as_deref().unwrap_or(local_sid), 18);
            prepare_select_history_from_external(
                ui,
                summary,
                CodexHistoryExternalFocusOrigin::Sessions,
            );
            ui.toast = Some((
                format!(
                    "{} {sid_label}",
                    i18n::label(ui.language, "history: focused session")
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('t') if ui.page == Page::Sessions => {
            let Some(_opaque_sid) = ui.selected_session_id.as_deref() else {
                ui.toast = Some((
                    i18n::label(ui.language, "no session selected").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let Some(selected_row) = snapshot.rows.get(ui.selected_session_idx) else {
                return true;
            };
            let Some(local_sid) = selected_row.local_command_session_id().map(str::to_owned) else {
                ui.toast = Some((
                    i18n::label(ui.language, "no local session selected").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            ui.session_transcript_sid = Some(local_sid.clone());

            let resolved_path = if let Some(path) = host_transcript_path_from_row(selected_row) {
                Ok(Some(path))
            } else {
                find_codex_session_file_by_id(&local_sid).await
            };
            match resolved_path {
                Ok(Some(path)) => {
                    open_session_transcript_from_path(ui, local_sid, &path, Some(80)).await;
                }
                Ok(None) => {
                    ui.toast = Some((
                        i18n::label(
                            ui.language,
                            "no Codex session file found for this session id",
                        )
                        .to_string(),
                        Instant::now(),
                    ));
                }
                Err(e) => {
                    ui.toast = Some((
                        format!(
                            "{}: {e}",
                            i18n::label(ui.language, "failed to load transcript")
                        ),
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Enter if ui.page == Page::Recent => {
            let Some(r) = selected_recent_row(ui) else {
                ui.toast = Some((
                    i18n::label(ui.language, "recent: no selection").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let line = format!("{} {}", r.root, r.session_id);
            match try_copy_to_clipboard(&line) {
                Ok(()) => {
                    ui.toast = Some((
                        i18n::text(ui.language, msg::RECENT_COPIED_SELECTED).to_string(),
                        Instant::now(),
                    ));
                }
                Err(e) => {
                    ui.toast = Some((
                        format!("{}: {e}", i18n::label(ui.language, "clipboard failed")),
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Char('t') if ui.page == Page::Recent => {
            let Some(r) = selected_recent_row(ui) else {
                ui.toast = Some((
                    i18n::label(ui.language, "recent: no selection").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let sid = r.session_id.clone();
            match find_codex_session_file_by_id(&sid).await {
                Ok(Some(path)) => {
                    open_session_transcript_from_path(ui, sid, &path, Some(80)).await;
                }
                Ok(None) => {
                    ui.toast = Some((
                        i18n::label(
                            ui.language,
                            "recent: no local transcript file found for this session",
                        )
                        .to_string(),
                        Instant::now(),
                    ));
                }
                Err(e) => {
                    ui.toast = Some((
                        format!(
                            "{}: {e}",
                            i18n::label(ui.language, "recent: resolve session file failed")
                        ),
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Char('s') if ui.page == Page::Recent => {
            let Some(r) = selected_recent_row(ui) else {
                ui.toast = Some((
                    i18n::label(ui.language, "recent: no selection").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            if focus_session_in_sessions(ui, snapshot, r.session_id.as_str()) {
                ui.toast = Some((
                    format!(
                        "{} {}",
                        i18n::label(ui.language, "sessions: focused"),
                        short_sid(r.session_id.as_str(), 18)
                    ),
                    Instant::now(),
                ));
            } else {
                ui.toast = Some((
                    i18n::text(ui.language, msg::RECENT_SESSION_NOT_OBSERVED).to_string(),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Char('f') if ui.page == Page::Recent => {
            let Some(r) = selected_recent_row(ui) else {
                ui.toast = Some((
                    i18n::label(ui.language, "recent: no selection").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let sid_label = short_sid(r.session_id.as_str(), 18);
            prepare_select_requests_for_session(ui, r.session_id);
            ui.toast = Some((
                format!(
                    "{} {sid_label}",
                    i18n::label(ui.language, "requests: focused session")
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('h') if ui.page == Page::Recent => {
            let Some(r) = selected_recent_row(ui) else {
                ui.toast = Some((
                    i18n::label(ui.language, "recent: no selection").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let path = match find_codex_session_file_by_id(r.session_id.as_str()).await {
                Ok(path) => path,
                Err(e) => {
                    ui.toast = Some((
                        format!(
                            "{}: {e}",
                            i18n::label(ui.language, "history: resolve session file failed")
                        ),
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
                format!(
                    "{} {sid_label}",
                    i18n::label(ui.language, "history: focused session")
                ),
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
                ui.toast = Some((
                    i18n::label(ui.language, "recent: nothing to copy").to_string(),
                    Instant::now(),
                ));
                return true;
            }
            match try_copy_to_clipboard(&out) {
                Ok(()) => {
                    ui.toast = Some((
                        i18n::text(ui.language, msg::RECENT_COPIED_VISIBLE).to_string(),
                        Instant::now(),
                    ));
                }
                Err(e) => {
                    ui.toast = Some((
                        format!("{}: {e}", i18n::label(ui.language, "clipboard failed")),
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Char('o') if ui.page == Page::Dashboard && ui.focus == Focus::Requests => {
            let Some(request) = selected_dashboard_request(snapshot, ui) else {
                ui.toast = Some((
                    i18n::label(ui.language, "dashboard: no request selected").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let Some(opaque_sid) = request.session_key.as_deref() else {
                ui.toast = Some((
                    i18n::label(ui.language, "dashboard: selected request has no session id")
                        .to_string(),
                    Instant::now(),
                ));
                return true;
            };
            if focus_session_in_sessions(ui, snapshot, opaque_sid) {
                ui.toast = Some((
                    format!(
                        "{} {}",
                        i18n::label(ui.language, "sessions: focused"),
                        short_sid(opaque_sid, 18)
                    ),
                    Instant::now(),
                ));
            } else {
                ui.toast = Some((
                    i18n::text(ui.language, msg::SESSION_NOT_OBSERVED).to_string(),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Char('h') if ui.page == Page::Dashboard && ui.focus == Focus::Requests => {
            let Some(request) = selected_dashboard_request(snapshot, ui).cloned() else {
                ui.toast = Some((
                    i18n::label(ui.language, "dashboard: no request selected").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let Some(opaque_sid) = request.session_key.as_deref() else {
                ui.toast = Some((
                    i18n::label(ui.language, "dashboard: selected request has no session id")
                        .to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let Some(local_sid) = local_session_id_for_opaque_key(snapshot, opaque_sid) else {
                ui.toast = Some((
                    i18n::label(
                        ui.language,
                        "dashboard: selected request has no local session id",
                    )
                    .to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let path = match find_codex_session_file_by_id(local_sid).await {
                Ok(path) => path,
                Err(e) => {
                    ui.toast = Some((
                        format!(
                            "{}: {e}",
                            i18n::label(ui.language, "history: resolve session file failed")
                        ),
                        Instant::now(),
                    ));
                    return true;
                }
            };
            let summary = request_history_summary_from_request(&request, local_sid, path);
            let sid_label = short_sid(opaque_sid, 18);
            prepare_select_history_from_external(
                ui,
                summary,
                CodexHistoryExternalFocusOrigin::Requests,
            );
            ui.toast = Some((
                format!(
                    "{} {sid_label}",
                    i18n::label(ui.language, "history: focused session")
                ),
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
                ui.toast = Some((
                    i18n::label(ui.language, "history: no selection").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            if summary.path.as_os_str().is_empty() {
                ui.toast = Some((
                    i18n::text(ui.language, msg::HISTORY_NO_TRANSCRIPT_FILE).to_string(),
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
                ui.toast = Some((
                    i18n::label(ui.language, "history: no selection").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            if focus_session_in_sessions(ui, snapshot, sid.as_str()) {
                ui.toast = Some((
                    format!(
                        "{} {}",
                        i18n::label(ui.language, "sessions: focused"),
                        short_sid(sid.as_str(), 18)
                    ),
                    Instant::now(),
                ));
            } else {
                ui.toast = Some((
                    i18n::text(ui.language, msg::SESSION_NOT_OBSERVED).to_string(),
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
                ui.toast = Some((
                    i18n::label(ui.language, "history: no selection").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let sid_label = short_sid(summary.id.as_str(), 18);
            prepare_select_requests_for_session(ui, summary.id);
            ui.toast = Some((
                format!(
                    "{} {sid_label}",
                    i18n::label(ui.language, "requests: focused session")
                ),
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
            let visible = ui.codex_recent_visible_indices(now);
            let len = visible.len();
            if let Some(next) = adjust_table_selection(&mut ui.codex_recent_table, -1, len) {
                ui.codex_recent_selected_idx = next;
                ui.codex_recent_selected_id = ui
                    .codex_recent_rows
                    .get(visible[next])
                    .map(|r| r.session_id.clone());
                return true;
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Recent => {
            let now = now_ms();
            let visible = ui.codex_recent_visible_indices(now);
            let len = visible.len();
            if let Some(next) = adjust_table_selection(&mut ui.codex_recent_table, 1, len) {
                ui.codex_recent_selected_idx = next;
                ui.codex_recent_selected_id = ui
                    .codex_recent_rows
                    .get(visible[next])
                    .map(|r| r.session_id.clone());
                return true;
            }
            false
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Sessions => {
            let filtered = ui.filtered_sessions_page_indices(snapshot);

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
            let filtered = ui.filtered_sessions_page_indices(snapshot);

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
                    "{}: {}={}",
                    i18n::label(ui.language, "requests filter"),
                    i18n::label(ui.language, "errors_only"),
                    ui.request_page_errors_only
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('c') if ui.page == Page::Requests => {
            ui.request_page_control_filter = ui.request_page_control_filter.next();
            ui.selected_request_page_idx = 0;
            ui.toast = Some((
                format!(
                    "{}: {}={}",
                    i18n::label(ui.language, "requests filter"),
                    i18n::label(ui.language, "control"),
                    ui.request_page_control_filter.label(ui.language)
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
                    "{}: {}",
                    i18n::label(ui.language, "requests scope"),
                    if ui.request_page_scope_session {
                        i18n::label(ui.language, "selected session")
                    } else {
                        i18n::label(ui.language, "all")
                    }
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('x') if ui.page == Page::Requests => {
            clear_request_page_focus(ui);
            ui.toast = Some((
                i18n::label(ui.language, "requests: cleared explicit session focus").to_string(),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('o') if ui.page == Page::Requests => {
            let Some(request) = selected_request_page_request(snapshot, ui) else {
                ui.toast = Some((
                    i18n::label(ui.language, "requests: no selection").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let Some(opaque_sid) = request.session_key.as_deref() else {
                ui.toast = Some((
                    i18n::label(ui.language, "requests: selected request has no session id")
                        .to_string(),
                    Instant::now(),
                ));
                return true;
            };
            if focus_session_in_sessions(ui, snapshot, opaque_sid) {
                ui.toast = Some((
                    format!(
                        "{} {}",
                        i18n::label(ui.language, "sessions: focused"),
                        short_sid(opaque_sid, 18)
                    ),
                    Instant::now(),
                ));
            } else {
                ui.toast = Some((
                    i18n::text(ui.language, msg::SESSION_NOT_OBSERVED).to_string(),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Char('h') if ui.page == Page::Requests => {
            let Some(request) = selected_request_page_request(snapshot, ui).cloned() else {
                ui.toast = Some((
                    i18n::label(ui.language, "requests: no selection").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let Some(opaque_sid) = request.session_key.as_deref() else {
                ui.toast = Some((
                    i18n::label(ui.language, "requests: selected request has no session id")
                        .to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let Some(local_sid) = local_session_id_for_opaque_key(snapshot, opaque_sid) else {
                ui.toast = Some((
                    i18n::label(
                        ui.language,
                        "requests: selected request has no local session id",
                    )
                    .to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let path = match find_codex_session_file_by_id(local_sid).await {
                Ok(path) => path,
                Err(e) => {
                    ui.toast = Some((
                        format!(
                            "{}: {e}",
                            i18n::label(ui.language, "history: resolve session file failed")
                        ),
                        Instant::now(),
                    ));
                    return true;
                }
            };
            let summary = request_history_summary_from_request(&request, local_sid, path);
            let sid_label = short_sid(opaque_sid, 18);
            prepare_select_history_from_external(
                ui,
                summary,
                CodexHistoryExternalFocusOrigin::Requests,
            );
            ui.toast = Some((
                format!(
                    "{} {sid_label}",
                    i18n::label(ui.language, "history: focused session")
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Requests => {
            let filtered_len = ui.request_page_filtered_indices(snapshot).len();
            if let Some(next) = adjust_table_selection(&mut ui.request_page_table, -1, filtered_len)
            {
                ui.selected_request_page_idx = next;
                return true;
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Requests => {
            let filtered_len = ui.request_page_filtered_indices(snapshot).len();
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
            Focus::Providers => false,
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
            Focus::Providers => false,
        },
        _ => false,
    }
}

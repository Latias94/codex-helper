use std::time::Instant;

use codex_helper_core::codex_switch::{self, CodexSwitchIntent, ValidatedCodexBaseUrl};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::config::{CodexClientPatchConfig, CodexClientPreset, load_config, proxy_home_dir};
use crate::proxy::{
    OperatorRoutingCommand, OperatorRoutingMutationRequest, OperatorSessionBindingCommand,
    OperatorSessionBindingMutationRequest,
};
use crate::sessions::find_codex_session_file_by_id;
use crate::tui::Language;
use crate::tui::i18n::{self, msg};
use crate::tui::model::{
    CODEX_RECENT_WINDOWS, Snapshot, codex_recent_window_label, codex_recent_window_threshold_ms,
    filtered_requests_len, find_session_idx, now_ms, short_sid,
};
use crate::tui::report::build_stats_report;
use crate::tui::state::{
    CodexHistoryExternalFocusOrigin, FleetViewMode, SessionBindingEditContext, UiState,
    adjust_table_selection,
};
use crate::tui::types::{
    Focus, Overlay, Page, SessionBindingInputKind, SessionEffortChoice, SessionServiceTierChoice,
};

use super::KeyEventContext;
use super::history_bridge::{
    host_transcript_path_from_row, local_session_context_for_opaque_key,
    prepare_select_history_from_external, recent_history_summary_from_row,
    request_history_summary_from_request, selected_dashboard_request, selected_recent_row,
    selected_request_page_request, session_history_summary_from_row,
};
use super::transcript::open_session_transcript_from_path;
use crate::tui::operator_actions::{
    notify_read_only_operator_action, queue_balance_refresh, queue_relay_capabilities,
    queue_relay_live_smoke, queue_runtime_reload, queue_session_binding_mutation,
};
use crate::tui::settings_relay::{
    CodexRelayLiveSmokeDecision, CodexRelayLiveSmokeMode, infer_codex_relay_model,
};

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

pub(in crate::tui) fn codex_client_preset_for_key(code: KeyCode) -> Option<CodexClientPreset> {
    match code {
        KeyCode::Char('B') => Some(CodexClientPreset::ChatGptBridge),
        KeyCode::Char('I') => Some(CodexClientPreset::ImagegenBridge),
        KeyCode::Char('F') => Some(CodexClientPreset::OfficialRelay),
        KeyCode::Char('V') => Some(CodexClientPreset::OfficialImagegen),
        KeyCode::Char('D') => Some(CodexClientPreset::Default),
        _ => None,
    }
}

pub(in crate::tui) fn accepts_codex_switch_key(key: &KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
}

pub(in crate::tui) async fn apply_codex_switch(
    ui: &mut UiState,
    intent: CodexSwitchIntent,
    preset: Option<CodexClientPreset>,
) {
    let action = if matches!(&intent, CodexSwitchIntent::On { .. }) {
        "on"
    } else {
        "off"
    };
    let outcome = match &intent {
        CodexSwitchIntent::On { .. } => {
            let client_patch = match preset {
                Some(preset) => Ok(CodexClientPatchConfig {
                    preset,
                    ..CodexClientPatchConfig::default()
                }),
                None => load_config()
                    .await
                    .map(|config| config.codex.client_patch.unwrap_or_default())
                    .map_err(|error| {
                        format!("read [codex.client_patch] from helper config: {error}")
                    }),
            };
            match client_patch {
                Ok(client_patch) => codex_switch::apply_with_client_patch(intent, client_patch)
                    .map_err(|error| error.to_string()),
                Err(error) => Err(error),
            }
        }
        CodexSwitchIntent::Off => codex_switch::apply(intent).map_err(|error| error.to_string()),
    };
    let message = match outcome {
        Ok(outcome) => match ui.language {
            Language::Zh => format!(
                "Codex 本地 switch {action}：{}（phase={}，preset={}）；重启已有 Codex app 后生效",
                outcome.change.as_str(),
                outcome.status.phase.as_str(),
                outcome
                    .status
                    .client_patch
                    .map(|patch| patch.preset.as_str())
                    .unwrap_or("-")
            ),
            Language::En => format!(
                "Codex local switch {action}: {} (phase={}, preset={}); restart existing Codex apps to apply it",
                outcome.change.as_str(),
                outcome.status.phase.as_str(),
                outcome
                    .status
                    .client_patch
                    .map(|patch| patch.preset.as_str())
                    .unwrap_or("-")
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
        if previous_page != p {
            ui.help_scroll = 0;
        }
        if previous_page == Page::Stats || ui.page == Page::Stats || ui.page == Page::ServiceStatus
        {
            ui.needs_snapshot_refresh = true;
        }
        if ui.page == Page::Routing {
            ui.focus = Focus::Providers;
            if previous_page != Page::Routing {
                ui.routing_detail_focused = false;
                ui.routing_detail_scroll = 0;
                let _ = queue_balance_refresh(ui, false, false);
            }
        } else if ui.page == Page::Requests {
            ui.focus = Focus::Requests;
        } else if ui.page == Page::Dashboard && previous_page != Page::Dashboard {
            ui.dashboard_details_scroll = 0;
        } else if ui.page == Page::Sessions
            || ui.page == Page::History
            || ui.page == Page::Recent
            || (ui.page == Page::Dashboard && ui.focus == Focus::Providers)
        {
            ui.focus = Focus::Sessions;
        }
        if ui.page == Page::History {
            ui.needs_codex_history_refresh = true;
            ui.codex_history_details_scroll = 0;
            ui.sync_codex_history_selection();
        }
        if ui.page == Page::Recent {
            ui.needs_codex_recent_refresh = true;
            ui.codex_recent_details_scroll = 0;
            ui.sync_codex_recent_selection(now_ms());
        }
        if ui.page == Page::Fleet {
            ui.focus = Focus::Providers;
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
    if ui.page != Page::Sessions || code != KeyCode::Char('A') {
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

fn prepare_session_binding_edit(ui: &mut UiState, snapshot: &Snapshot) -> Option<usize> {
    if !ui.can_mutate_session_binding() {
        notify_read_only_operator_action(ui);
        return None;
    }
    let Some((index, row)) = snapshot
        .rows
        .get(ui.selected_session_idx)
        .map(|row| (ui.selected_session_idx, row))
    else {
        ui.toast = Some((
            match ui.language {
                Language::Zh => "没有选中的会话".to_string(),
                Language::En => "no session is selected".to_string(),
            },
            Instant::now(),
        ));
        return None;
    };
    let Some(session_key) = row.session_id.as_deref() else {
        ui.toast = Some((
            match ui.language {
                Language::Zh => "选中行没有可控制的会话标识".to_string(),
                Language::En => "the selected row has no controllable session identity".to_string(),
            },
            Instant::now(),
        ));
        return None;
    };
    if row.binding.revision.trim().is_empty() {
        ui.toast = Some((
            match ui.language {
                Language::Zh => "daemon 未提供会话绑定控制元数据；已降级为只读".to_string(),
                Language::En => {
                    "the daemon did not provide session binding control metadata; read-only mode"
                        .to_string()
                }
            },
            Instant::now(),
        ));
        return None;
    }
    ui.session_binding_edit = Some(SessionBindingEditContext {
        session_key: session_key.to_string(),
        expected_revision: row.binding.revision.clone(),
    });
    Some(index)
}

fn selected_session_binding_request(
    ui: &mut UiState,
    snapshot: &Snapshot,
    command: OperatorSessionBindingCommand,
) -> Option<OperatorSessionBindingMutationRequest> {
    prepare_session_binding_edit(ui, snapshot)?;
    let edit = ui.session_binding_edit.take()?;
    Some(OperatorSessionBindingMutationRequest {
        session_key: edit.session_key,
        expected_binding_revision: edit.expected_revision,
        command,
    })
}

fn session_binding_shortcuts_active(ui: &UiState) -> bool {
    ui.focus == Focus::Sessions && matches!(ui.page, Page::Dashboard | Page::Sessions)
}

fn open_session_profile_menu(ui: &mut UiState, snapshot: &Snapshot) -> bool {
    let Some(index) = prepare_session_binding_edit(ui, snapshot) else {
        return true;
    };
    let selected = snapshot.rows[index].binding.profile_name.as_deref();
    ui.capture_profile_menu_snapshot();
    ui.session_profile_menu_idx = selected
        .and_then(|name| {
            ui.profile_menu_options()
                .iter()
                .position(|profile| profile.name == name)
        })
        .map(|index| index + 1)
        .unwrap_or_else(|| {
            usize::from(selected.is_none() && !ui.profile_menu_options().is_empty())
        });
    ui.overlay = Overlay::SessionProfileMenu;
    true
}

fn open_session_model_menu(ui: &mut UiState, snapshot: &Snapshot) -> bool {
    let Some(index) = prepare_session_binding_edit(ui, snapshot) else {
        return true;
    };
    let row = &snapshot.rows[index];
    let mut models = ui
        .profile_options
        .iter()
        .filter_map(|profile| profile.model.clone())
        .collect::<Vec<_>>();
    for model in [
        row.binding.model.as_deref(),
        row.effective_model
            .as_ref()
            .map(|value| value.value.as_str()),
        row.last_model.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        let model = model.trim();
        if !model.is_empty() {
            models.push(model.to_string());
        }
    }
    models.sort();
    models.dedup();
    ui.session_model_menu_idx = row
        .binding
        .model
        .as_deref()
        .and_then(|model| models.iter().position(|candidate| candidate == model))
        .map(|index| index + 1)
        .unwrap_or(0);
    ui.session_model_options = models;
    ui.session_binding_input_kind = SessionBindingInputKind::Model;
    ui.session_binding_input = row.binding.model.clone().unwrap_or_default();
    ui.session_binding_input_hint = row
        .effective_model
        .as_ref()
        .map(|value| value.value.clone())
        .or_else(|| row.last_model.clone());
    ui.overlay = Overlay::SessionModelMenu;
    true
}

fn open_session_effort_menu(ui: &mut UiState, snapshot: &Snapshot) -> bool {
    let Some(index) = prepare_session_binding_edit(ui, snapshot) else {
        return true;
    };
    let current = snapshot.rows[index].binding.reasoning_effort.as_deref();
    ui.session_effort_menu_idx = SessionEffortChoice::ALL
        .iter()
        .position(|choice| choice.value() == current)
        .unwrap_or(0);
    ui.overlay = Overlay::SessionEffortMenu;
    true
}

fn open_session_service_tier_menu(ui: &mut UiState, snapshot: &Snapshot) -> bool {
    let Some(index) = prepare_session_binding_edit(ui, snapshot) else {
        return true;
    };
    let row = &snapshot.rows[index];
    let current = row.binding.service_tier.as_deref();
    ui.session_service_tier_menu_idx = SessionServiceTierChoice::ALL
        .iter()
        .position(|choice| match choice {
            SessionServiceTierChoice::Fast => current == Some("priority"),
            choice => choice.value() == current,
        })
        .unwrap_or(SessionServiceTierChoice::ALL.len());
    ui.session_binding_input_kind = SessionBindingInputKind::ServiceTier;
    ui.session_binding_input = row.binding.service_tier.clone().unwrap_or_default();
    ui.session_binding_input_hint = row
        .effective_service_tier
        .as_ref()
        .map(|value| value.value.clone())
        .or_else(|| row.last_service_tier.clone());
    ui.overlay = Overlay::SessionServiceTierMenu;
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
    ui.selected_request_id = None;
    ui.dashboard_details_scroll = 0;
    ui.sessions_details_scroll = 0;
    ui.sync_dashboard_request_selection(snapshot);
}

fn focus_session_in_sessions(ui: &mut UiState, snapshot: &Snapshot, sid: &str) -> bool {
    let Some(idx) = find_session_idx(snapshot, sid) else {
        return false;
    };

    ui.sessions_page_active_only = false;
    ui.sessions_page_errors_only = false;
    ui.sessions_page_overrides_only = false;
    ui.sessions_details_scroll = 0;
    ui.selected_sessions_page_idx = 0;
    ui.page = Page::Sessions;
    ui.focus = Focus::Sessions;
    apply_selected_session(ui, snapshot, idx);
    true
}

fn require_runtime_session_bridge(ui: &mut UiState) -> bool {
    if ui.can_bridge_runtime_sessions_to_local_codex() {
        return true;
    }

    let message = match (ui.language, ui.runtime_connection.is_remote_observer()) {
        (Language::Zh, true) => {
            "远程观察模式无法将观察端本机 Codex 会话匹配到远程 Sessions/Requests"
        }
        (Language::En, true) => {
            "remote observer mode cannot match observer-local Codex sessions to remote Sessions/Requests"
        }
        (Language::Zh, false) => "当前 TUI 无法将本机 Codex 会话匹配到运行时 Sessions/Requests",
        (Language::En, false) => {
            "this TUI cannot match local Codex sessions to runtime Sessions/Requests"
        }
    };
    ui.toast = Some((message.to_string(), Instant::now()));
    false
}

fn prepare_select_requests_for_session(ui: &mut UiState, snapshot: &Snapshot, sid: String) {
    ui.page = Page::Requests;
    ui.focus = Focus::Requests;
    ui.request_page_errors_only = false;
    ui.request_page_scope_session = true;
    ui.focused_request_session_id =
        Some(crate::tui::model::runtime_session_key(snapshot, &sid).unwrap_or(sid));
    ui.selected_request_page_idx = 0;
    ui.selected_request_page_id = None;
    ui.sync_request_page_selection(snapshot);
    ui.requests_details_scroll = 0;
}

fn clear_request_page_focus(ui: &mut UiState) {
    ui.focused_request_session_id = None;
    ui.selected_request_page_idx = 0;
    ui.selected_request_page_id = None;
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

    if accepts_codex_switch_key(&key)
        && ui.page == Page::Settings
        && ui.allows_local_codex_switch()
        && let Some(preset) = codex_client_preset_for_key(key.code)
    {
        apply_codex_switch(
            ui,
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(ui.proxy_port),
            },
            Some(preset),
        )
        .await;
        return true;
    }

    if accepts_codex_switch_key(&key)
        && ui.page == Page::Settings
        && ui.allows_local_codex_switch()
        && let Some(intent) = codex_switch_intent_for_key(key.code, ui.proxy_port)
    {
        apply_codex_switch(ui, intent, None).await;
        return true;
    }

    if handle_routing_operator_key(ui, snapshot, key.code) {
        return true;
    }
    if handle_session_affinity_operator_key(ui, snapshot, key.code) {
        return true;
    }

    if ui.page == Page::Settings {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                ui.settings_scroll = ui.settings_scroll.saturating_sub(1);
                return true;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                ui.settings_scroll = ui.settings_scroll.saturating_add(1);
                return true;
            }
            KeyCode::PageUp => {
                ui.settings_scroll = ui.settings_scroll.saturating_sub(8);
                return true;
            }
            KeyCode::PageDown => {
                ui.settings_scroll = ui.settings_scroll.saturating_add(8);
                return true;
            }
            KeyCode::Home => {
                ui.settings_scroll = 0;
                return true;
            }
            KeyCode::End => {
                ui.settings_scroll = u16::MAX;
                return true;
            }
            KeyCode::Char('R') => {
                queue_runtime_reload(ui);
                return true;
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                if !ui.can_mutate_default_profile() {
                    notify_read_only_operator_action(ui);
                    return true;
                }
                let selected = if key.code == KeyCode::Char('p') {
                    ui.configured_default_profile.clone()
                } else {
                    ui.runtime_default_profile_override.clone()
                };
                ui.capture_profile_menu_snapshot();
                ui.settings_profile_menu_idx = selected
                    .as_deref()
                    .and_then(|selected| {
                        ui.profile_menu_options()
                            .iter()
                            .position(|profile| profile.name == selected)
                    })
                    .map_or(0, |index| index + 1);
                ui.overlay = if key.code == KeyCode::Char('p') {
                    Overlay::ConfiguredDefaultProfileMenu
                } else {
                    Overlay::RuntimeDefaultProfileMenu
                };
                return true;
            }
            KeyCode::Char('C') => {
                if !ui.can_inspect_relay_capabilities() {
                    notify_read_only_operator_action(ui);
                    return true;
                }
                let model = infer_codex_relay_model(
                    snapshot,
                    ui.selected_session_idx,
                    ui.operator_read_model.as_ref(),
                );
                match ui
                    .codex_relay_diagnostics
                    .begin(ui.service_name, model, Instant::now())
                {
                    Ok(start) => {
                        queue_relay_capabilities(ui, start);
                    }
                    Err(error) => ui.toast = Some((error.to_string(), Instant::now())),
                }
                return true;
            }
            KeyCode::Char('X') | KeyCode::Char('Y') => {
                if !ui.can_run_relay_live_smoke() {
                    notify_read_only_operator_action(ui);
                    return true;
                }
                let model = infer_codex_relay_model(
                    snapshot,
                    ui.selected_session_idx,
                    ui.operator_read_model.as_ref(),
                );
                let mode = if key.code == KeyCode::Char('X') {
                    CodexRelayLiveSmokeMode::CompactOnly
                } else {
                    CodexRelayLiveSmokeMode::CompactAndImage
                };
                match ui.codex_relay_live_smoke.confirm_or_begin(
                    ui.service_name,
                    model,
                    mode,
                    Instant::now(),
                ) {
                    CodexRelayLiveSmokeDecision::ConfirmAgain { mode, .. } => {
                        ui.toast = Some((
                            match ui.language {
                                Language::Zh => {
                                    format!("再次按 {} 确认真实上游 smoke；会消耗余额", mode.key())
                                }
                                Language::En => format!(
                                    "press {} again to confirm the billable live smoke",
                                    mode.key()
                                ),
                            },
                            Instant::now(),
                        ));
                    }
                    CodexRelayLiveSmokeDecision::Started(start) => {
                        queue_relay_live_smoke(ui, start);
                    }
                    CodexRelayLiveSmokeDecision::Blocked(error) => {
                        ui.toast = Some((error.to_string(), Instant::now()));
                    }
                }
                return true;
            }
            _ => {}
        }
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
            ui.help_scroll = 0;
            true
        }
        KeyCode::Char('i') if ui.page == Page::Routing => {
            ui.sync_selected_provider_from_routing(snapshot, providers);
            super::open_provider_info(ui);
            true
        }
        KeyCode::Tab | KeyCode::BackTab => {
            if ui.page == Page::Dashboard {
                ui.focus = match ui.focus {
                    Focus::Sessions => Focus::Requests,
                    Focus::Requests => Focus::Sessions,
                    Focus::Providers => Focus::Sessions,
                };
            } else if ui.page == Page::Routing {
                ui.focus = Focus::Providers;
                if ui.routing_detail_available && snapshot.routing.is_some() {
                    ui.routing_detail_focused = !ui.routing_detail_focused;
                    ui.toast = Some((
                        match (ui.language, ui.routing_detail_focused) {
                            (Language::Zh, true) => "路由焦点：端点详情".to_string(),
                            (Language::Zh, false) => "路由焦点：候选端点".to_string(),
                            (Language::En, true) => "routing focus: endpoint details".to_string(),
                            (Language::En, false) => {
                                "routing focus: endpoint candidates".to_string()
                            }
                        },
                        Instant::now(),
                    ));
                } else {
                    ui.routing_detail_focused = false;
                    ui.routing_detail_scroll = 0;
                }
            } else if ui.page == Page::ServiceStatus {
                if ui.service_status_detail_available {
                    ui.service_status_detail_focused = !ui.service_status_detail_focused;
                    ui.toast = Some((
                        match (ui.language, ui.service_status_detail_focused) {
                            (Language::Zh, true) => "服务状态焦点：当前详情".to_string(),
                            (Language::Zh, false) => "服务状态焦点：探针列表".to_string(),
                            (Language::En, true) => {
                                "service status focus: selected details".to_string()
                            }
                            (Language::En, false) => "service status focus: probe list".to_string(),
                        },
                        Instant::now(),
                    ));
                } else {
                    ui.service_status_detail_focused = false;
                    ui.service_status_detail_scroll = 0;
                }
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
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::ServiceStatus => {
            if ui.service_status_detail_focused {
                ui.service_status_detail_scroll = ui.service_status_detail_scroll.saturating_sub(1);
                true
            } else {
                ui.move_service_status_selection(snapshot, -1)
            }
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::ServiceStatus => {
            if ui.service_status_detail_focused {
                ui.service_status_detail_scroll = ui.service_status_detail_scroll.saturating_add(1);
                true
            } else {
                ui.move_service_status_selection(snapshot, 1)
            }
        }
        KeyCode::PageUp if ui.page == Page::ServiceStatus => {
            if ui.service_status_detail_focused {
                ui.service_status_detail_scroll = ui.service_status_detail_scroll.saturating_sub(8);
                true
            } else {
                ui.move_service_status_selection(
                    snapshot,
                    -(ui.service_status_visible_rows.min(i32::MAX as usize) as i32),
                )
            }
        }
        KeyCode::PageDown if ui.page == Page::ServiceStatus => {
            if ui.service_status_detail_focused {
                ui.service_status_detail_scroll = ui.service_status_detail_scroll.saturating_add(8);
                true
            } else {
                ui.move_service_status_selection(
                    snapshot,
                    ui.service_status_visible_rows.min(i32::MAX as usize) as i32,
                )
            }
        }
        KeyCode::Home if ui.page == Page::ServiceStatus => {
            if ui.service_status_detail_focused {
                ui.service_status_detail_scroll = 0;
                true
            } else {
                ui.select_service_status_edge(snapshot, false)
            }
        }
        KeyCode::End if ui.page == Page::ServiceStatus => {
            if ui.service_status_detail_focused {
                ui.service_status_detail_scroll = u16::MAX;
                true
            } else {
                ui.select_service_status_edge(snapshot, true)
            }
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
            if ui.routing_detail_focused {
                ui.routing_detail_scroll = ui.routing_detail_scroll.saturating_sub(8);
                true
            } else {
                let delta = -(ui.routing_candidates_visible_rows.min(i32::MAX as usize) as i32);
                move_routing_page_selection(ui, snapshot, providers.len(), delta)
            }
        }
        KeyCode::PageDown if ui.page == Page::Routing => {
            if ui.routing_detail_focused {
                ui.routing_detail_scroll = ui.routing_detail_scroll.saturating_add(8);
                true
            } else {
                let delta = ui.routing_candidates_visible_rows.min(i32::MAX as usize) as i32;
                move_routing_page_selection(ui, snapshot, providers.len(), delta)
            }
        }
        KeyCode::Home if ui.page == Page::Routing => {
            if ui.routing_detail_focused {
                ui.routing_detail_scroll = 0;
                true
            } else {
                select_routing_page_edge(ui, snapshot, providers.len(), false)
            }
        }
        KeyCode::End if ui.page == Page::Routing => {
            if ui.routing_detail_focused {
                ui.routing_detail_scroll = u16::MAX;
                true
            } else {
                select_routing_page_edge(ui, snapshot, providers.len(), true)
            }
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Routing => {
            if ui.routing_detail_focused {
                ui.routing_detail_scroll = ui.routing_detail_scroll.saturating_sub(1);
                true
            } else {
                move_routing_page_selection(ui, snapshot, providers.len(), -1)
            }
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Routing => {
            if ui.routing_detail_focused {
                ui.routing_detail_scroll = ui.routing_detail_scroll.saturating_add(1);
                true
            } else {
                move_routing_page_selection(ui, snapshot, providers.len(), 1)
            }
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Stats => {
            ui.move_stats_selection(snapshot, -1)
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Stats => {
            ui.move_stats_selection(snapshot, 1)
        }
        KeyCode::Char('g') if ui.page == Page::Stats => {
            ui.needs_snapshot_refresh = true;
            if ui.can_refresh_provider_balances() {
                let _ = queue_balance_refresh(ui, true, true);
            } else {
                ui.toast = Some((
                    match ui.language {
                        Language::Zh => "额度：仅刷新观察快照；远程余额保持不变",
                        Language::En => {
                            "quota: refreshing snapshot-only view; upstream balances are unchanged"
                        }
                    }
                    .to_string(),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Char('y') if ui.page == Page::Stats => export_selected_stats_report(ui, snapshot),
        KeyCode::PageUp if ui.page == Page::Dashboard => {
            ui.dashboard_details_scroll = ui.dashboard_details_scroll.saturating_sub(8);
            true
        }
        KeyCode::PageDown if ui.page == Page::Dashboard => {
            ui.dashboard_details_scroll = ui.dashboard_details_scroll.saturating_add(8);
            true
        }
        KeyCode::Home if ui.page == Page::Dashboard => {
            ui.dashboard_details_scroll = 0;
            true
        }
        KeyCode::End if ui.page == Page::Dashboard => {
            ui.dashboard_details_scroll = u16::MAX;
            true
        }
        KeyCode::Char('a') if ui.page == Page::Sessions => {
            ui.sessions_page_active_only = !ui.sessions_page_active_only;
            ui.selected_sessions_page_idx = 0;
            ui.sessions_details_scroll = 0;
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
            ui.sessions_details_scroll = 0;
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
        KeyCode::Char('v') if ui.page == Page::Sessions => {
            ui.sessions_page_overrides_only = !ui.sessions_page_overrides_only;
            ui.selected_sessions_page_idx = 0;
            ui.sessions_details_scroll = 0;
            ui.toast = Some((
                format!(
                    "{}: {}={}",
                    i18n::label(ui.language, "sessions filter"),
                    i18n::label(ui.language, "overrides_only"),
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
            ui.sessions_details_scroll = 0;
            ui.toast = Some((
                i18n::label(ui.language, "sessions filter: reset").to_string(),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('b') if session_binding_shortcuts_active(ui) => {
            open_session_profile_menu(ui, snapshot)
        }
        KeyCode::Char('M') if session_binding_shortcuts_active(ui) => {
            open_session_model_menu(ui, snapshot)
        }
        KeyCode::Char('E') if session_binding_shortcuts_active(ui) => {
            open_session_effort_menu(ui, snapshot)
        }
        KeyCode::Enter if session_binding_shortcuts_active(ui) => {
            open_session_effort_menu(ui, snapshot)
        }
        KeyCode::Char('f') if session_binding_shortcuts_active(ui) => {
            open_session_service_tier_menu(ui, snapshot)
        }
        KeyCode::Char('l') | KeyCode::Char('m') | KeyCode::Char('h') | KeyCode::Char('X')
            if session_binding_shortcuts_active(ui) =>
        {
            let Some(reasoning_effort) = (match key.code {
                KeyCode::Char('l') => Some("low"),
                KeyCode::Char('m') => Some("medium"),
                KeyCode::Char('h') => Some("high"),
                KeyCode::Char('X') => Some("xhigh"),
                _ => None,
            }) else {
                return false;
            };
            if let Some(request) = selected_session_binding_request(
                ui,
                snapshot,
                OperatorSessionBindingCommand::SetReasoningEffort {
                    reasoning_effort: Some(reasoning_effort.to_string()),
                },
            ) {
                let _ = queue_session_binding_mutation(ui, request);
            }
            true
        }
        KeyCode::Char('R') if session_binding_shortcuts_active(ui) => {
            let has_manual_values = snapshot
                .rows
                .get(ui.selected_session_idx)
                .is_some_and(|row| row.binding.has_manual_values());
            if !has_manual_values {
                ui.toast = Some((
                    match ui.language {
                        Language::Zh => "会话手动控制已经为空".to_string(),
                        Language::En => "session manual controls are already clear".to_string(),
                    },
                    Instant::now(),
                ));
                return true;
            }
            if let Some(request) = selected_session_binding_request(
                ui,
                snapshot,
                OperatorSessionBindingCommand::ResetManualOverrides,
            ) {
                let _ = queue_session_binding_mutation(ui, request);
            }
            true
        }
        KeyCode::Char('x') if session_binding_shortcuts_active(ui) => {
            if let Some(request) = selected_session_binding_request(
                ui,
                snapshot,
                OperatorSessionBindingCommand::SetReasoningEffort {
                    reasoning_effort: None,
                },
            ) {
                let _ = queue_session_binding_mutation(ui, request);
            }
            true
        }
        KeyCode::Char('r') if ui.page == Page::History => {
            ui.needs_codex_history_refresh = true;
            ui.codex_history_details_scroll = 0;
            ui.toast = Some((
                i18n::text(ui.language, msg::HISTORY_REFRESHING).to_string(),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('r') if ui.page == Page::Recent => {
            ui.needs_codex_recent_refresh = true;
            ui.codex_recent_details_scroll = 0;
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
            ui.codex_recent_details_scroll = 0;
            ui.sync_codex_recent_selection(now_ms());
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
            ui.codex_recent_details_scroll = 0;
            ui.sync_codex_recent_selection(now_ms());
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
            prepare_select_requests_for_session(ui, snapshot, sid);
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
            prepare_select_requests_for_session(ui, snapshot, sid);
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
            if !require_runtime_session_bridge(ui) {
                return true;
            }
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
            if !require_runtime_session_bridge(ui) {
                return true;
            }
            let Some(r) = selected_recent_row(ui) else {
                ui.toast = Some((
                    i18n::label(ui.language, "recent: no selection").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let sid_label = short_sid(r.session_id.as_str(), 18);
            prepare_select_requests_for_session(ui, snapshot, r.session_id);
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
            let Some((local_sid, cwd)) = local_session_context_for_opaque_key(snapshot, opaque_sid)
            else {
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
            let path = match find_codex_session_file_by_id(&local_sid).await {
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
            let summary = request_history_summary_from_request(&request, &local_sid, cwd, path);
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
            if !require_runtime_session_bridge(ui) {
                return true;
            }
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
            if !require_runtime_session_bridge(ui) {
                return true;
            }
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
            prepare_select_requests_for_session(ui, snapshot, summary.id);
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
            let len = ui.codex_history_visible_len();
            if let Some(next) = adjust_table_selection(&mut ui.codex_history_table, -1, len) {
                ui.selected_codex_history_idx = next;
                ui.selected_codex_history_id = ui
                    .codex_history_sessions
                    .get(next)
                    .map(|summary| summary.id.clone());
                ui.codex_history_details_scroll = 0;
                return true;
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::History => {
            let len = ui.codex_history_visible_len();
            if let Some(next) = adjust_table_selection(&mut ui.codex_history_table, 1, len) {
                ui.selected_codex_history_idx = next;
                ui.selected_codex_history_id = ui
                    .codex_history_sessions
                    .get(next)
                    .map(|summary| summary.id.clone());
                ui.codex_history_details_scroll = 0;
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
                ui.codex_recent_details_scroll = 0;
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
                ui.codex_recent_details_scroll = 0;
                return true;
            }
            false
        }
        KeyCode::PageUp if ui.page == Page::History => {
            ui.codex_history_details_scroll = ui.codex_history_details_scroll.saturating_sub(8);
            true
        }
        KeyCode::PageDown if ui.page == Page::History => {
            ui.codex_history_details_scroll = ui.codex_history_details_scroll.saturating_add(8);
            true
        }
        KeyCode::Home if ui.page == Page::History => {
            ui.codex_history_details_scroll = 0;
            true
        }
        KeyCode::End if ui.page == Page::History => {
            ui.codex_history_details_scroll = u16::MAX;
            true
        }
        KeyCode::PageUp if ui.page == Page::Recent => {
            ui.codex_recent_details_scroll = ui.codex_recent_details_scroll.saturating_sub(8);
            true
        }
        KeyCode::PageDown if ui.page == Page::Recent => {
            ui.codex_recent_details_scroll = ui.codex_recent_details_scroll.saturating_add(8);
            true
        }
        KeyCode::Home if ui.page == Page::Recent => {
            ui.codex_recent_details_scroll = 0;
            true
        }
        KeyCode::End if ui.page == Page::Recent => {
            ui.codex_recent_details_scroll = u16::MAX;
            true
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
        KeyCode::PageUp if ui.page == Page::Sessions => {
            ui.sessions_details_scroll = ui.sessions_details_scroll.saturating_sub(8);
            true
        }
        KeyCode::PageDown if ui.page == Page::Sessions => {
            ui.sessions_details_scroll = ui.sessions_details_scroll.saturating_add(8);
            true
        }
        KeyCode::Home if ui.page == Page::Sessions => {
            ui.sessions_details_scroll = 0;
            true
        }
        KeyCode::End if ui.page == Page::Sessions => {
            ui.sessions_details_scroll = u16::MAX;
            true
        }
        KeyCode::Char('e') if ui.page == Page::Requests => {
            ui.request_page_errors_only = !ui.request_page_errors_only;
            ui.selected_request_page_idx = 0;
            ui.selected_request_page_id = None;
            ui.requests_details_scroll = 0;
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
            ui.selected_request_page_id = None;
            ui.requests_details_scroll = 0;
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
            ui.selected_request_page_id = None;
            ui.requests_details_scroll = 0;
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
            let Some((local_sid, cwd)) = local_session_context_for_opaque_key(snapshot, opaque_sid)
            else {
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
            let path = match find_codex_session_file_by_id(&local_sid).await {
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
            let summary = request_history_summary_from_request(&request, &local_sid, cwd, path);
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
                return ui.select_request_page_index(snapshot, next);
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Requests => {
            let filtered_len = ui.request_page_filtered_indices(snapshot).len();
            if let Some(next) = adjust_table_selection(&mut ui.request_page_table, 1, filtered_len)
            {
                return ui.select_request_page_index(snapshot, next);
            }
            false
        }
        KeyCode::PageUp if ui.page == Page::Requests => {
            ui.requests_details_scroll = ui.requests_details_scroll.saturating_sub(8);
            true
        }
        KeyCode::PageDown if ui.page == Page::Requests => {
            ui.requests_details_scroll = ui.requests_details_scroll.saturating_add(8);
            true
        }
        KeyCode::Home if ui.page == Page::Requests => {
            ui.requests_details_scroll = 0;
            true
        }
        KeyCode::End if ui.page == Page::Requests => {
            ui.requests_details_scroll = u16::MAX;
            true
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
                    return ui.select_dashboard_request_index(snapshot, next);
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
                    return ui.select_dashboard_request_index(snapshot, next);
                }
                false
            }
            Focus::Providers => false,
        },
        _ => false,
    }
}

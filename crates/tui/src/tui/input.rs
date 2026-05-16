mod balance;
mod health;
mod history_bridge;
mod profile;
mod routing;
mod routing_menu;
mod session_overrides;
mod transcript;

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::config::{
    bootstrap::overwrite_codex_config_from_codex_cli_in_place,
    proxy_home_dir,
    storage::{load_config, save_config},
};
use crate::proxy::ProxyService;
use crate::sessions::find_codex_session_file_by_id;
use crate::state::ProxyState;

use super::Language;
use super::i18n::{self, msg};
use super::model::{
    CODEX_RECENT_WINDOWS, ProviderOption, Snapshot, codex_recent_window_label,
    codex_recent_window_threshold_ms, filtered_request_page_len, filtered_requests_len,
    find_session_idx, now_ms, session_row_has_any_override, short_sid,
};
use super::report::build_stats_report;
use super::state::{CodexHistoryExternalFocusOrigin, UiState, adjust_table_selection};
use super::types::{Focus, Overlay, Page, StatsFocus};
pub(in crate::tui) use balance::{
    BalanceRefreshMode, BalanceRefreshOutcome, BalanceRefreshSender,
    request_provider_balance_refresh,
};
use health::{
    begin_station_health_check, load_upstreams_for_station, spawn_all_station_health_checks,
    spawn_station_health_check,
};
use history_bridge::{
    host_transcript_path_from_row, prepare_select_history_from_external,
    recent_history_summary_from_row, request_history_summary_from_request,
    selected_dashboard_request, selected_recent_row, selected_request_page_request,
    session_history_summary_from_row,
};
pub(in crate::tui) use profile::refresh_profile_control_state;
use profile::{
    default_profile_menu_idx, handle_key_profile_menu, runtime_default_profile_menu_idx,
};
use routing::{
    apply_global_route_target_pin, invalidate_route_target_preview, open_routing_editor,
    refresh_route_graph_balances,
};
use routing_menu::handle_key_routing_menu;
use session_overrides::{
    add_model_option_if_missing, apply_effort_override, current_model_override,
    current_service_tier_override, handle_key_effort_menu, handle_key_model_input,
    handle_key_model_menu, handle_key_service_tier_input, handle_key_service_tier_menu,
    load_model_options_for_service, selected_session_model_hint,
    selected_session_service_tier_hint,
};
use transcript::{handle_key_session_transcript, open_session_transcript_from_path};

pub(in crate::tui) use routing::refresh_routing_control_state;

#[cfg(test)]
use balance::should_request_provider_balance_refresh;
#[cfg(test)]
use routing_menu::{
    routing_entry_children, routing_entry_is_flat_provider_list,
    routing_spec_after_provider_enabled_change, routing_spec_with_order,
};

pub(in crate::tui) fn should_accept_key_event(event: &KeyEvent) -> bool {
    matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

pub(in crate::tui) async fn handle_key_event(
    state: Arc<ProxyState>,
    providers: &mut Vec<ProviderOption>,
    ui: &mut UiState,
    snapshot: &Snapshot,
    proxy: &ProxyService,
    balance_refresh_tx: BalanceRefreshSender,
    key: KeyEvent,
) -> bool {
    if ui.overlay == Overlay::None && apply_page_shortcuts(ui, key.code) {
        return true;
    }

    match ui.overlay {
        Overlay::None => {
            handle_key_normal(
                &state,
                providers,
                ui,
                snapshot,
                proxy,
                &balance_refresh_tx,
                key,
            )
            .await
        }
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
        Overlay::SessionTranscript => handle_key_session_transcript(ui, key).await,
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
            handle_key_profile_menu(&state, ui, snapshot, proxy, key).await
        }
        Overlay::ProviderMenuSession | Overlay::ProviderMenuGlobal => {
            handle_key_provider_menu(&state, providers, ui, snapshot, proxy, key).await
        }
        Overlay::RoutingMenu => {
            handle_key_routing_menu(providers, ui, snapshot, proxy, &balance_refresh_tx, key).await
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

async fn apply_session_provider_override(state: &ProxyState, sid: String, cfg: Option<String>) {
    let now = now_ms();
    if let Some(cfg) = cfg {
        state.set_session_station_override(sid, cfg, now).await;
    } else {
        state.clear_session_station_override(&sid).await;
    }
}

async fn apply_session_route_target_override(
    state: &ProxyState,
    sid: String,
    target: Option<String>,
) {
    let now = now_ms();
    if let Some(target) = target {
        state
            .set_session_route_target_override(sid, target, now)
            .await;
    } else {
        state.clear_session_route_target_override(&sid).await;
    }
}

async fn clear_session_manual_overrides(state: &ProxyState, sid: String) {
    state.clear_session_manual_overrides(&sid).await;
}

async fn apply_global_station_pin(
    state: &ProxyState,
    providers: &[ProviderOption],
    station_name: Option<String>,
) -> anyhow::Result<()> {
    if let Some(name) = station_name.as_deref() {
        if !providers.iter().any(|provider| provider.name == name) {
            anyhow::bail!("unknown station: {name}");
        }
        state
            .set_global_station_override(name.to_string(), now_ms())
            .await;
    } else {
        state.clear_global_station_override().await;
    }
    Ok(())
}

async fn persist_ui_language(language: Language) -> anyhow::Result<()> {
    let mut cfg = load_config().await?;
    cfg.ui.language = Some(i18n::storage_code(language).to_string());
    save_config(&cfg).await?;
    Ok(())
}

async fn toggle_language(ui: &mut UiState) {
    let next = i18n::next_language(ui.language);
    ui.language = next;
    match persist_ui_language(next).await {
        Ok(()) => {
            ui.toast = Some((
                i18n::format_language_saved(ui.language, next),
                Instant::now(),
            ));
        }
        Err(err) => {
            ui.toast = Some((
                i18n::format_language_save_failed(ui.language, next, &err),
                Instant::now(),
            ));
        }
    }
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

async fn handle_key_normal(
    state: &Arc<ProxyState>,
    providers: &mut Vec<ProviderOption>,
    ui: &mut UiState,
    snapshot: &Snapshot,
    proxy: &ProxyService,
    balance_refresh_tx: &BalanceRefreshSender,
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
                    i18n::label(
                        ui.language,
                        "overwrite-from-codex is only supported for Codex service",
                    )
                    .to_string(),
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
                    i18n::text(ui.language, msg::CONFIRM_OVERWRITE).to_string(),
                    now,
                ));
                return true;
            }

            match load_config().await {
                Ok(mut cfg) => {
                    if let Err(err) = overwrite_codex_config_from_codex_cli_in_place(&mut cfg) {
                        ui.toast = Some((
                            match ui.language {
                                Language::Zh => format!("overwrite-from-codex 失败：{err}"),
                                Language::En => format!("overwrite-from-codex failed: {err}"),
                            },
                            Instant::now(),
                        ));
                        return true;
                    }
                    if let Err(err) = save_config(&cfg).await {
                        ui.toast = Some((
                            match ui.language {
                                Language::Zh => format!("保存失败：{err}"),
                                Language::En => format!("save failed: {err}"),
                            },
                            Instant::now(),
                        ));
                        return true;
                    }

                    *providers = crate::tui::build_provider_options(&cfg, ui.service_name);
                    ui.clamp_selection(snapshot, providers.len());
                    let _ = refresh_profile_control_state(ui, proxy).await;
                    ui.toast = Some((
                        match ui.language {
                            Language::Zh => {
                                format!("已从 ~/.codex 覆盖导入站点（n={}）", providers.len())
                            }
                            Language::En => {
                                format!("overwrote stations from ~/.codex (n={})", providers.len())
                            }
                        },
                        Instant::now(),
                    ));
                    true
                }
                Err(err) => {
                    ui.toast = Some((
                        match ui.language {
                            Language::Zh => format!("加载配置失败：{err}"),
                            Language::En => format!("load config failed: {err}"),
                        },
                        Instant::now(),
                    ));
                    true
                }
            }
        }
        KeyCode::Char('R') if ui.page == Page::Settings => {
            let now = Instant::now();
            match proxy.reload_runtime_config().await {
                Ok(result) => {
                    ui.last_runtime_config_loaded_at_ms = Some(result.status.loaded_at_ms);
                    ui.last_runtime_config_source_mtime_ms = result.status.source_mtime_ms;
                    ui.last_runtime_retry = Some(result.status.retry);
                    ui.last_runtime_config_refresh_at = Some(now);
                    let _ = refresh_profile_control_state(ui, proxy).await;

                    ui.toast = Some((
                        i18n::format_config_reloaded(ui.language, result.reloaded),
                        now,
                    ));
                    true
                }
                Err(err) => {
                    ui.toast = Some((
                        match ui.language {
                            Language::Zh => format!("重载失败：{err}"),
                            Language::En => format!("reload failed: {err}"),
                        },
                        now,
                    ));
                    true
                }
            }
        }
        KeyCode::Char('i') if ui.page == Page::Stations => {
            if ui.uses_route_graph_routing() {
                open_routing_editor(
                    ui,
                    snapshot,
                    proxy,
                    i18n::label(ui.language, "routing: provider details/edit"),
                    balance_refresh_tx,
                )
                .await;
                return true;
            }
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
                ui.stats_provider_detail_scroll = 0;
                ui.toast = Some((
                    format!(
                        "{}: {}",
                        i18n::label(ui.language, "focus"),
                        match ui.stats_focus {
                            StatsFocus::Stations => i18n::label(ui.language, "station"),
                            StatsFocus::Providers => i18n::label(ui.language, "provider"),
                        }
                    ),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Stations => {
            let len = ui.station_page_rows_len(providers.len());
            if let Some(next) = adjust_table_selection(&mut ui.stations_table, -1, len) {
                ui.selected_station_idx = next;
                return true;
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Stations => {
            let len = ui.station_page_rows_len(providers.len());
            if let Some(next) = adjust_table_selection(&mut ui.stations_table, 1, len) {
                ui.selected_station_idx = next;
                return true;
            }
            false
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Stats => {
            match ui.stats_focus {
                StatsFocus::Stations => {
                    let len = snapshot.usage_rollup.by_config.len();
                    if let Some(next) =
                        adjust_table_selection(&mut ui.stats_stations_table, -1, len)
                    {
                        ui.selected_stats_station_idx = next;
                        return true;
                    }
                }
                StatsFocus::Providers => {
                    let len = ui.usage_balance_provider_rows_len(snapshot);
                    if let Some(next) =
                        adjust_table_selection(&mut ui.stats_providers_table, -1, len)
                    {
                        ui.selected_stats_provider_idx = next;
                        ui.stats_provider_detail_scroll = 0;
                        return true;
                    }
                }
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Stats => {
            match ui.stats_focus {
                StatsFocus::Stations => {
                    let len = snapshot.usage_rollup.by_config.len();
                    if let Some(next) = adjust_table_selection(&mut ui.stats_stations_table, 1, len)
                    {
                        ui.selected_stats_station_idx = next;
                        return true;
                    }
                }
                StatsFocus::Providers => {
                    let len = ui.usage_balance_provider_rows_len(snapshot);
                    if let Some(next) =
                        adjust_table_selection(&mut ui.stats_providers_table, 1, len)
                    {
                        ui.selected_stats_provider_idx = next;
                        ui.stats_provider_detail_scroll = 0;
                        return true;
                    }
                }
            }
            false
        }
        KeyCode::PageUp if ui.page == Page::Stats && ui.stats_focus == StatsFocus::Providers => {
            ui.stats_provider_detail_scroll = ui.stats_provider_detail_scroll.saturating_sub(5);
            true
        }
        KeyCode::PageDown if ui.page == Page::Stats && ui.stats_focus == StatsFocus::Providers => {
            ui.stats_provider_detail_scroll = ui.stats_provider_detail_scroll.saturating_add(5);
            true
        }
        KeyCode::Char('d') if ui.page == Page::Stats => {
            let options = [1usize, 7usize, 30usize, 0usize];
            let idx = options
                .iter()
                .position(|&n| n == ui.stats_days)
                .unwrap_or(1);
            let next = options[(idx + 1) % options.len()];
            ui.stats_days = next;
            ui.stats_provider_detail_scroll = 0;
            ui.needs_snapshot_refresh = true;
            let label = if next == 0 {
                i18n::label(ui.language, "loaded").to_string()
            } else if next == 1 {
                i18n::label(ui.language, "today").to_string()
            } else {
                format!("{next}d")
            };
            ui.toast = Some((
                format!("{}: {label}", i18n::label(ui.language, "window")),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('a') if ui.page == Page::Stats => {
            ui.stats_attention_only = !ui.stats_attention_only;
            ui.selected_stats_provider_idx = 0;
            ui.stats_provider_detail_scroll = 0;
            let len = ui.usage_balance_provider_rows_len(snapshot);
            ui.stats_providers_table
                .select((len > 0).then_some(ui.selected_stats_provider_idx));
            *ui.stats_providers_table.offset_mut() = 0;
            ui.toast = Some((
                format!(
                    "{}: {}={}",
                    i18n::label(ui.language, "Stats page"),
                    i18n::label(ui.language, "attention only"),
                    ui.stats_attention_only
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('e') if ui.page == Page::Stats => {
            ui.stats_errors_only = !ui.stats_errors_only;
            ui.toast = Some((
                format!(
                    "{}: {}={}",
                    i18n::label(ui.language, "Stats page"),
                    i18n::label(ui.language, "errors_only"),
                    ui.stats_errors_only
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('g') if ui.page == Page::Stats => {
            let balance_started = request_provider_balance_refresh(
                ui,
                snapshot,
                proxy,
                BalanceRefreshMode::Force,
                balance_refresh_tx,
            );
            ui.toast = Some((
                if balance_started {
                    match ui.language {
                        Language::Zh => "usage/balance: 余额刷新已开始",
                        Language::En => "usage/balance: balance refresh started",
                    }
                } else {
                    i18n::label(ui.language, "balance refresh already requested")
                }
                .to_string(),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('y') if ui.page == Page::Stats => {
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
                            Language::Zh => {
                                format!("stats report: 已复制并保存 {}", path.display())
                            }
                            Language::En => {
                                format!("stats report: copied + saved {}", path.display())
                            }
                        },
                        Instant::now(),
                    ));
                }
                (Ok(path), Err(err)) => {
                    ui.toast = Some((
                        match ui.language {
                            Language::Zh => {
                                format!(
                                    "stats report: 已保存 {}（复制失败：{err}）",
                                    path.display()
                                )
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
        KeyCode::Enter if ui.page == Page::Stations => {
            if ui.uses_route_graph_routing() {
                let Some(name) = ui.selected_route_graph_provider_name() else {
                    return true;
                };
                match apply_global_route_target_pin(state, providers, Some(name.clone())).await {
                    Ok(()) => {
                        invalidate_route_target_preview(ui);
                        ui.toast = Some((
                            match ui.language {
                                Language::Zh => format!("全局 route target：{name}"),
                                Language::En => format!("global route target: {name}"),
                            },
                            Instant::now(),
                        ));
                    }
                    Err(err) => {
                        ui.toast = Some((
                            match ui.language {
                                Language::Zh => format!("设置全局 route target 失败：{err}"),
                                Language::En => format!("set global route target failed: {err}"),
                            },
                            Instant::now(),
                        ));
                    }
                }
                return true;
            }
            let Some(name) = providers
                .get(ui.selected_station_idx)
                .map(|p| p.name.clone())
            else {
                return true;
            };
            match apply_global_station_pin(state, providers, Some(name.clone())).await {
                Ok(()) => {
                    ui.toast = Some((
                        match ui.language {
                            Language::Zh => format!("全局站点 pin：{name}"),
                            Language::En => format!("global station pin: {name}"),
                        },
                        Instant::now(),
                    ));
                }
                Err(err) => {
                    ui.toast = Some((
                        match ui.language {
                            Language::Zh => format!("设置全局 pin 失败：{err}"),
                            Language::En => format!("set global pin failed: {err}"),
                        },
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Char('r') if ui.page == Page::Stations => {
            open_routing_editor(
                ui,
                snapshot,
                proxy,
                i18n::label(ui.language, "routing: edit persisted policy/order"),
                balance_refresh_tx,
            )
            .await;
            true
        }
        KeyCode::Char('g') if ui.page == Page::Stations && ui.uses_route_graph_routing() => {
            refresh_route_graph_balances(ui, snapshot, proxy, balance_refresh_tx).await;
            true
        }
        KeyCode::Backspace | KeyCode::Delete if ui.page == Page::Stations => {
            if ui.uses_route_graph_routing() {
                match apply_global_route_target_pin(state, providers, None).await {
                    Ok(()) => {
                        invalidate_route_target_preview(ui);
                        ui.toast = Some((
                            match ui.language {
                                Language::Zh => "全局 route target：<auto>",
                                Language::En => "global route target: <auto>",
                            }
                            .to_string(),
                            Instant::now(),
                        ));
                    }
                    Err(err) => {
                        ui.toast = Some((
                            match ui.language {
                                Language::Zh => format!("清除全局 route target 失败：{err}"),
                                Language::En => format!("clear global route target failed: {err}"),
                            },
                            Instant::now(),
                        ));
                    }
                }
                return true;
            }
            match apply_global_station_pin(state, providers, None).await {
                Ok(()) => {
                    let message = match ui.language {
                        Language::Zh => "全局站点 pin：<auto>",
                        Language::En => "global station pin: <auto>",
                    };
                    ui.toast = Some((message.to_string(), Instant::now()));
                }
                Err(err) => {
                    ui.toast = Some((
                        match ui.language {
                            Language::Zh => format!("设置全局 pin 失败：{err}"),
                            Language::En => format!("set global pin failed: {err}"),
                        },
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Char('o') if ui.page == Page::Stations => {
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                let label = if ui.uses_route_graph_routing() {
                    "session route target: <no session>"
                } else {
                    "session station override: <no session>"
                };
                ui.toast = Some((i18n::label(ui.language, label).to_string(), Instant::now()));
                return true;
            };
            if ui.uses_route_graph_routing() {
                let Some(name) = ui.selected_route_graph_provider_name() else {
                    return true;
                };
                apply_session_route_target_override(state, sid, Some(name.clone())).await;
                invalidate_route_target_preview(ui);
                ui.toast = Some((
                    match ui.language {
                        Language::Zh => format!("会话 route target：{name}"),
                        Language::En => format!("session route target: {name}"),
                    },
                    Instant::now(),
                ));
                return true;
            }
            let Some(pvd) = providers.get(ui.selected_station_idx) else {
                return true;
            };
            apply_session_provider_override(state, sid, Some(pvd.name.clone())).await;
            ui.toast = Some((
                format!(
                    "{}: {}",
                    i18n::label(ui.language, "session station override"),
                    pvd.name
                ),
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
                let label = if ui.uses_route_graph_routing() {
                    "session route target: <no session>"
                } else {
                    "session station override: <no session>"
                };
                ui.toast = Some((i18n::label(ui.language, label).to_string(), Instant::now()));
                return true;
            };
            if ui.uses_route_graph_routing() {
                apply_session_route_target_override(state, sid.clone(), None).await;
                state.clear_session_station_override(&sid).await;
                invalidate_route_target_preview(ui);
                ui.toast = Some((
                    match ui.language {
                        Language::Zh => "会话 route target：<清除>",
                        Language::En => "session route target: <clear>",
                    }
                    .to_string(),
                    Instant::now(),
                ));
                return true;
            }
            apply_session_provider_override(state, sid, None).await;
            let message = i18n::label(ui.language, "session station override: <clear>");
            ui.toast = Some((message.to_string(), Instant::now()));
            true
        }
        KeyCode::Char('e')
        | KeyCode::Char('f')
        | KeyCode::Char('s')
        | KeyCode::Char('1')
        | KeyCode::Char('2')
        | KeyCode::Char('0')
        | KeyCode::Char('[')
        | KeyCode::Char(']')
        | KeyCode::Char('u')
        | KeyCode::Char('d')
            if ui.page == Page::Stations && ui.uses_route_graph_routing() =>
        {
            if ui.routing_spec.is_none()
                && let Err(err) = refresh_routing_control_state(ui, proxy).await
            {
                ui.toast = Some((
                    format!(
                        "{}: {err}",
                        i18n::label(ui.language, "routing: load failed")
                    ),
                    Instant::now(),
                ));
                return true;
            }
            ui.sync_routing_menu_with_station_selection();
            let handled =
                handle_key_routing_menu(providers, ui, snapshot, proxy, balance_refresh_tx, key)
                    .await;
            ui.sync_station_selection_with_routing_menu();
            handled
        }
        KeyCode::Char('h') if ui.page == Page::Stations => {
            if ui.uses_route_graph_routing() {
                ui.toast = Some((
                    i18n::label(ui.language, "routing: use g to refresh balances").to_string(),
                    Instant::now(),
                ));
                return true;
            }
            let Some(pvd) = providers.get(ui.selected_station_idx) else {
                return true;
            };
            let service_name = ui.service_name;
            let station_name = pvd.name.clone();

            let upstreams = match load_upstreams_for_station(service_name, &station_name).await {
                Ok(v) => v,
                Err(err) => {
                    ui.toast = Some((
                        format!(
                            "{}: {err}",
                            i18n::label(ui.language, "health check load failed")
                        ),
                        Instant::now(),
                    ));
                    return true;
                }
            };

            if !begin_station_health_check(
                state.as_ref(),
                service_name,
                &station_name,
                upstreams.len(),
            )
            .await
            {
                ui.toast = Some((
                    format!(
                        "{}: {station_name}",
                        i18n::label(ui.language, "health check already running")
                    ),
                    Instant::now(),
                ));
                return true;
            }

            ui.toast = Some((
                format!(
                    "{}: {station_name}",
                    i18n::label(ui.language, "health check queued")
                ),
                Instant::now(),
            ));
            spawn_station_health_check(Arc::clone(state), service_name, station_name, upstreams);
            true
        }
        KeyCode::Char('H') if ui.page == Page::Stations => {
            if ui.uses_route_graph_routing() {
                ui.toast = Some((
                    i18n::label(ui.language, "routing: use g to refresh balances").to_string(),
                    Instant::now(),
                ));
                return true;
            }
            let service_name = ui.service_name;
            let stations = providers.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
            ui.toast = Some((
                match ui.language {
                    Language::Zh => format!("健康检查已排队：{} 个站点", stations.len()),
                    Language::En => format!("health check queued: {} stations", stations.len()),
                },
                Instant::now(),
            ));
            spawn_all_station_health_checks(Arc::clone(state), service_name, stations);
            true
        }
        KeyCode::Char('c') if ui.page == Page::Stations => {
            if ui.uses_route_graph_routing() {
                ui.toast = Some((
                    i18n::label(ui.language, "routing: no health check is running").to_string(),
                    Instant::now(),
                ));
                return true;
            }
            let Some(pvd) = providers.get(ui.selected_station_idx) else {
                return true;
            };
            let now = now_ms();
            if state
                .request_cancel_station_health_check(ui.service_name, pvd.name.as_str(), now)
                .await
            {
                ui.toast = Some((
                    format!(
                        "{}: {}",
                        i18n::label(ui.language, "health check cancel requested"),
                        pvd.name
                    ),
                    Instant::now(),
                ));
            } else {
                ui.toast = Some((
                    format!(
                        "{}: {}",
                        i18n::label(ui.language, "health check not running"),
                        pvd.name
                    ),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Char('C') if ui.page == Page::Stations => {
            if ui.uses_route_graph_routing() {
                ui.toast = Some((
                    i18n::label(ui.language, "routing: no health check is running").to_string(),
                    Instant::now(),
                ));
                return true;
            }
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
                match ui.language {
                    Language::Zh => format!("已请求取消健康检查：{count} 个站点"),
                    Language::En => format!("health check cancel requested: {count} stations"),
                },
                Instant::now(),
            ));
            true
        }
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
        KeyCode::Char('v') if ui.page == Page::Sessions => {
            ui.sessions_page_overrides_only = !ui.sessions_page_overrides_only;
            ui.selected_sessions_page_idx = 0;
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
        KeyCode::Char('b')
            if ui.focus == Focus::Sessions
                && matches!(ui.page, Page::Dashboard | Page::Sessions) =>
        {
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|row| row.session_id.as_deref())
            else {
                ui.toast = Some((
                    i18n::label(ui.language, "no session selected").to_string(),
                    Instant::now(),
                ));
                return true;
            };

            match refresh_profile_control_state(ui, proxy).await {
                Ok(()) if ui.profile_options.is_empty() => {
                    ui.toast = Some((
                        i18n::text(ui.language, msg::PROFILE_NO_OPTIONS).to_string(),
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
                        match ui.language {
                            Language::Zh => {
                                format!("profile: 管理 {} 的绑定", short_sid(sid, 18))
                            }
                            Language::En => {
                                format!("profile: manage binding for {}", short_sid(sid, 18))
                            }
                        },
                        Instant::now(),
                    ));
                }
                Err(e) => {
                    ui.toast = Some((
                        match ui.language {
                            Language::Zh => format!("profile: 加载失败：{e}"),
                            Language::En => format!("profile: load failed: {e}"),
                        },
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Char('p') if ui.page == Page::Settings => {
            match refresh_profile_control_state(ui, proxy).await {
                Ok(()) if ui.profile_options.is_empty() => {
                    ui.toast = Some((
                        i18n::text(ui.language, msg::DEFAULT_PROFILE_NO_OPTIONS).to_string(),
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
                        i18n::text(ui.language, msg::DEFAULT_PROFILE_MANAGE_CONFIGURED).to_string(),
                        Instant::now(),
                    ));
                }
                Err(err) => {
                    ui.toast = Some((
                        match ui.language {
                            Language::Zh => format!("default profile 加载失败：{err}"),
                            Language::En => format!("default profile load failed: {err}"),
                        },
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Char('P') if ui.page == Page::Settings => {
            match refresh_profile_control_state(ui, proxy).await {
                Ok(()) if ui.profile_options.is_empty() => {
                    ui.toast = Some((
                        i18n::text(ui.language, msg::RUNTIME_DEFAULT_PROFILE_NO_OPTIONS)
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
                        i18n::text(ui.language, msg::RUNTIME_DEFAULT_PROFILE_MANAGE).to_string(),
                        Instant::now(),
                    ));
                }
                Err(err) => {
                    ui.toast = Some((
                        match ui.language {
                            Language::Zh => {
                                format!("runtime default profile 加载失败：{err}")
                            }
                            Language::En => {
                                format!("runtime default profile load failed: {err}")
                            }
                        },
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
                ui.toast = Some((
                    i18n::label(ui.language, "no session selected").to_string(),
                    Instant::now(),
                ));
                return true;
            };

            match load_model_options_for_service(ui.service_name).await {
                Ok(mut models) => {
                    let current = selected_session_model_hint(snapshot, ui);
                    add_model_option_if_missing(&mut models, current.as_deref());
                    if models.is_empty() {
                        ui.toast = Some((
                            i18n::text(ui.language, msg::MODEL_NO_CATALOG).to_string(),
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
                        match ui.language {
                            Language::Zh => format!("model: 为 {} 选择目标", short_sid(sid, 18)),
                            Language::En => {
                                format!("model: select target for {}", short_sid(sid, 18))
                            }
                        },
                        Instant::now(),
                    ));
                }
                Err(err) => {
                    ui.toast = Some((
                        match ui.language {
                            Language::Zh => format!("model: 加载失败：{err}"),
                            Language::En => format!("model: load failed: {err}"),
                        },
                        Instant::now(),
                    ));
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
                ui.toast = Some((
                    i18n::label(ui.language, "no session selected").to_string(),
                    Instant::now(),
                ));
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
                match ui.language {
                    Language::Zh => {
                        format!("service_tier: 为 {} 选择目标", short_sid(sid, 18))
                    }
                    Language::En => {
                        format!("service_tier: select target for {}", short_sid(sid, 18))
                    }
                },
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('R')
            if ui.focus == Focus::Sessions
                && matches!(ui.page, Page::Dashboard | Page::Sessions) =>
        {
            let Some(row) = snapshot.rows.get(ui.selected_session_idx) else {
                ui.toast = Some((
                    i18n::label(ui.language, "no session selected").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let Some(sid) = row.session_id.clone() else {
                ui.toast = Some((
                    i18n::label(ui.language, "no session selected").to_string(),
                    Instant::now(),
                ));
                return true;
            };
            if !session_row_has_any_override(row) {
                ui.toast = Some((
                    i18n::label(ui.language, "session overrides already clear").to_string(),
                    Instant::now(),
                ));
                return true;
            }

            clear_session_manual_overrides(state, sid).await;
            ui.needs_snapshot_refresh = true;
            if ui.uses_route_graph_routing() && row.override_route_target.is_some() {
                invalidate_route_target_preview(ui);
            }
            ui.toast = Some((
                i18n::label(ui.language, "session manual overrides reset").to_string(),
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
            let Some(sid) = row.session_id.as_deref() else {
                ui.toast = Some((
                    i18n::label(ui.language, "sessions: selected row has no session id")
                        .to_string(),
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
            let sid_label = short_sid(sid, 18);
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
            let Some(sid) = row.session_id.as_deref() else {
                ui.toast = Some((
                    i18n::label(ui.language, "dashboard: selected row has no session id")
                        .to_string(),
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
            let sid_label = short_sid(sid, 18);
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
            let Some(sid) = ui.selected_session_id.clone() else {
                ui.toast = Some((
                    i18n::label(ui.language, "no session selected").to_string(),
                    Instant::now(),
                ));
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
            let Some(sid) = request.session_id.as_deref() else {
                ui.toast = Some((
                    i18n::label(ui.language, "dashboard: selected request has no session id")
                        .to_string(),
                    Instant::now(),
                ));
                return true;
            };
            if focus_session_in_sessions(ui, snapshot, sid) {
                ui.toast = Some((
                    format!(
                        "{} {}",
                        i18n::label(ui.language, "sessions: focused"),
                        short_sid(sid, 18)
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
            let Some(sid) = request.session_id.as_deref() else {
                ui.toast = Some((
                    i18n::label(ui.language, "dashboard: selected request has no session id")
                        .to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let path = match find_codex_session_file_by_id(sid).await {
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
            let Some(summary) = request_history_summary_from_request(&request, path) else {
                ui.toast = Some((
                    i18n::label(ui.language, "history: failed to prepare request focus")
                        .to_string(),
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
                    "{}: {}={}",
                    i18n::label(ui.language, "requests filter"),
                    i18n::label(ui.language, "errors_only"),
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
            let Some(sid) = request.session_id.as_deref() else {
                ui.toast = Some((
                    i18n::label(ui.language, "requests: selected request has no session id")
                        .to_string(),
                    Instant::now(),
                ));
                return true;
            };
            if focus_session_in_sessions(ui, snapshot, sid) {
                ui.toast = Some((
                    format!(
                        "{} {}",
                        i18n::label(ui.language, "sessions: focused"),
                        short_sid(sid, 18)
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
            let Some(sid) = request.session_id.as_deref() else {
                ui.toast = Some((
                    i18n::label(ui.language, "requests: selected request has no session id")
                        .to_string(),
                    Instant::now(),
                ));
                return true;
            };
            let path = match find_codex_session_file_by_id(sid).await {
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
            let Some(summary) = request_history_summary_from_request(&request, path) else {
                ui.toast = Some((
                    i18n::label(ui.language, "history: failed to prepare request focus")
                        .to_string(),
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
                format!(
                    "{} {sid_label}",
                    i18n::label(ui.language, "history: focused session")
                ),
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
                format!(
                    "{}: {}",
                    i18n::label(ui.language, "effort override"),
                    eff.as_deref()
                        .unwrap_or_else(|| i18n::label(ui.language, "<clear>"))
                ),
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
            ui.toast = Some((
                i18n::label(ui.language, "effort override cleared").to_string(),
                Instant::now(),
            ));
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
            let current = if ui.uses_route_graph_routing() {
                snapshot
                    .route_target_overrides
                    .get(&sid)
                    .map(|s| s.as_str())
                    .unwrap_or("")
            } else {
                snapshot
                    .station_overrides
                    .get(&sid)
                    .map(|s| s.as_str())
                    .unwrap_or("")
            };
            ui.provider_menu_idx = providers
                .iter()
                .position(|p| p.name == current)
                .map(|i| i + 1)
                .unwrap_or(0);
            let balance_started = request_provider_balance_refresh(
                ui,
                snapshot,
                proxy,
                BalanceRefreshMode::Auto,
                balance_refresh_tx,
            );
            ui.overlay = Overlay::ProviderMenuSession;
            if balance_started {
                ui.toast = Some((
                    i18n::label(ui.language, "balance refresh started").to_string(),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Char('P') => {
            let current = if ui.uses_route_graph_routing() {
                snapshot
                    .global_route_target_override
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("")
            } else {
                snapshot
                    .global_station_override
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("")
            };
            ui.provider_menu_idx = providers
                .iter()
                .position(|p| p.name == current)
                .map(|i| i + 1)
                .unwrap_or(0);
            let balance_started = request_provider_balance_refresh(
                ui,
                snapshot,
                proxy,
                BalanceRefreshMode::Auto,
                balance_refresh_tx,
            );
            ui.overlay = Overlay::ProviderMenuGlobal;
            if balance_started {
                ui.toast = Some((
                    i18n::label(ui.language, "balance refresh started").to_string(),
                    Instant::now(),
                ));
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;

async fn handle_key_provider_menu(
    state: &ProxyState,
    providers: &mut [ProviderOption],
    ui: &mut UiState,
    snapshot: &Snapshot,
    _proxy: &ProxyService,
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
                    let result = if ui.uses_route_graph_routing() {
                        apply_global_route_target_pin(state, providers, chosen.clone()).await
                    } else {
                        apply_global_station_pin(state, providers, chosen.clone()).await
                    };
                    match result {
                        Ok(()) => {
                            let label = if ui.uses_route_graph_routing() {
                                invalidate_route_target_preview(ui);
                                "global route target"
                            } else {
                                "global station pin"
                            };
                            ui.toast = Some((
                                format!(
                                    "{}: {}",
                                    i18n::label(ui.language, label),
                                    chosen
                                        .as_deref()
                                        .unwrap_or_else(|| i18n::label(ui.language, "<auto>"))
                                ),
                                Instant::now(),
                            ));
                        }
                        Err(err) => {
                            let label = if ui.uses_route_graph_routing() {
                                "set global route target failed"
                            } else {
                                "set global pin failed"
                            };
                            ui.toast = Some((
                                format!("{}: {err}", i18n::label(ui.language, label)),
                                Instant::now(),
                            ));
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
                    if ui.uses_route_graph_routing() {
                        apply_session_route_target_override(state, sid, chosen.clone()).await;
                        invalidate_route_target_preview(ui);
                    } else {
                        apply_session_provider_override(state, sid, chosen.clone()).await;
                    }
                    let label = if ui.uses_route_graph_routing() {
                        "session route target"
                    } else {
                        "session station override"
                    };
                    ui.toast = Some((
                        format!(
                            "{}: {}",
                            i18n::label(ui.language, label),
                            chosen
                                .as_deref()
                                .unwrap_or_else(|| i18n::label(ui.language, "<clear>"))
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

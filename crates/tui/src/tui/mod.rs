mod i18n;
mod input;
mod model;
mod report;
mod state;
mod terminal;
mod types;
mod view;

pub use i18n::Language;
pub use i18n::{detect_system_language, parse_language, resolve_language_preference};
#[allow(unused_imports)]
pub use model::{ProviderOption, UpstreamSummary, build_provider_options};

use std::io;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use crossterm::ExecutableCommand;
use crossterm::event::{Event, EventStream};
use crossterm::terminal::LeaveAlternateScreen;
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::{mpsc, watch};

use crate::config::ProxyConfig;
use crate::config::storage::load_config;
use crate::state::ProxyState;

use self::model::{Palette, now_ms, refresh_snapshot};
use self::state::{RecentCodexRow, UiState, merge_codex_history_external_focus};
use self::terminal::TerminalGuard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderInvalidation {
    None,
    Redraw,
    FullClear,
}

fn request_redraw(invalidation: &mut RenderInvalidation) {
    if matches!(invalidation, RenderInvalidation::None) {
        *invalidation = RenderInvalidation::Redraw;
    }
}

fn request_full_clear(invalidation: &mut RenderInvalidation) {
    *invalidation = RenderInvalidation::FullClear;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RenderSurfaceKey {
    page: types::Page,
    overlay: types::Overlay,
    focus: types::Focus,
    stats_focus: types::StatsFocus,
    selected_station_idx: usize,
    selected_session_idx: usize,
    selected_request_idx: usize,
    selected_request_page_idx: usize,
    selected_sessions_page_idx: usize,
    selected_codex_history_idx: usize,
    codex_recent_selected_idx: usize,
    selected_stats_station_idx: usize,
    selected_stats_provider_idx: usize,
    effort_menu_idx: usize,
    model_menu_idx: usize,
    service_tier_menu_idx: usize,
    profile_menu_idx: usize,
    provider_menu_idx: usize,
    station_info_scroll: u16,
    session_transcript_scroll: u16,
}

#[derive(Debug)]
struct CodexHistoryRefreshResult {
    generation: u64,
    result: Result<Vec<crate::sessions::SessionSummary>, String>,
}

#[derive(Debug)]
struct CodexRecentRefreshResult {
    generation: u64,
    result: Result<CodexRecentRefreshPayload, String>,
}

#[derive(Debug)]
struct CodexRecentRefreshPayload {
    rows: Vec<RecentCodexRow>,
    branch_cache: std::collections::HashMap<String, Option<String>>,
}

impl RenderSurfaceKey {
    fn capture(ui: &UiState) -> Self {
        Self {
            page: ui.page,
            overlay: ui.overlay,
            focus: ui.focus,
            stats_focus: ui.stats_focus,
            selected_station_idx: ui.selected_station_idx,
            selected_session_idx: ui.selected_session_idx,
            selected_request_idx: ui.selected_request_idx,
            selected_request_page_idx: ui.selected_request_page_idx,
            selected_sessions_page_idx: ui.selected_sessions_page_idx,
            selected_codex_history_idx: ui.selected_codex_history_idx,
            codex_recent_selected_idx: ui.codex_recent_selected_idx,
            selected_stats_station_idx: ui.selected_stats_station_idx,
            selected_stats_provider_idx: ui.selected_stats_provider_idx,
            effort_menu_idx: ui.effort_menu_idx,
            model_menu_idx: ui.model_menu_idx,
            service_tier_menu_idx: ui.service_tier_menu_idx,
            profile_menu_idx: ui.profile_menu_idx,
            provider_menu_idx: ui.provider_menu_idx,
            station_info_scroll: ui.station_info_scroll,
            session_transcript_scroll: ui.session_transcript_scroll,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run_dashboard(
    state: Arc<ProxyState>,
    cfg: Arc<ProxyConfig>,
    service_name: &'static str,
    port: u16,
    admin_port: u16,
    providers: Vec<ProviderOption>,
    language: Language,
    shutdown: watch::Sender<bool>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let refresh_ms = std::env::var("CODEX_HELPER_TUI_REFRESH_MS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(500)
        .clamp(100, 5_000);

    let io_timeout = Duration::from_millis((refresh_ms / 2).clamp(50, 250));

    let mut term_guard = TerminalGuard::enter()?;
    let stdout = io::stdout();

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let mut ui = UiState {
        service_name,
        admin_port,
        language,
        refresh_ms,
        config_version: cfg.version,
        ..Default::default()
    };
    let _ = input::refresh_profile_control_state(&mut ui).await;
    let palette = Palette::default();

    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(refresh_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());

    let mut cfg = cfg;
    let mut snapshot = refresh_snapshot(&state, cfg.clone(), service_name, ui.stats_days).await;
    let mut providers = providers;
    ui.clamp_selection(&snapshot, ui.station_page_rows_len(providers.len()));
    let (balance_refresh_tx, mut balance_refresh_rx) =
        mpsc::unbounded_channel::<input::BalanceRefreshOutcome>();
    let (history_refresh_tx, mut history_refresh_rx) =
        mpsc::unbounded_channel::<CodexHistoryRefreshResult>();
    let (recent_refresh_tx, mut recent_refresh_rx) =
        mpsc::unbounded_channel::<CodexRecentRefreshResult>();

    let mut render_invalidation = RenderInvalidation::FullClear;
    let mut last_drawn_page = ui.page;
    loop {
        if render_invalidation != RenderInvalidation::None {
            if ui.page != last_drawn_page {
                // Defensive: some terminals occasionally leave stale cells when only a small
                // region changes (e.g., switching tabs). A full clear on page switch keeps the
                // UI visually consistent without clearing on every tick.
                request_full_clear(&mut render_invalidation);
                ui.reset_table_viewports();
                last_drawn_page = ui.page;
            }
            if matches!(render_invalidation, RenderInvalidation::FullClear) {
                terminal.clear()?;
            }
            terminal.draw(|f| {
                view::render_app(
                    f,
                    palette,
                    &mut ui,
                    &snapshot,
                    service_name,
                    port,
                    &providers,
                )
            })?;
            render_invalidation = RenderInvalidation::None;
        }

        if ui.should_exit || *shutdown_rx.borrow() {
            let _ = shutdown.send(true);
            break;
        }

        tokio::select! {
            _ = ticker.tick() => {
                let refresh = refresh_snapshot(&state, cfg.clone(), service_name, ui.stats_days);
                if let Ok(new_snapshot) = tokio::time::timeout(io_timeout, refresh).await {
                    snapshot = new_snapshot;
                    ui.clamp_selection(&snapshot, ui.station_page_rows_len(providers.len()));
                }
                if ui.page == crate::tui::types::Page::Settings
                    && ui
                        .last_runtime_config_refresh_at
                        .is_none_or(|t| t.elapsed() > Duration::from_secs(1))
                {
                    let url = format!(
                        "http://127.0.0.1:{}/__codex_helper/api/v1/runtime/status",
                        ui.admin_port
                    );
                    let fetch = async {
                        let client = reqwest::Client::new();
                        client
                            .get(&url)
                            .send()
                            .await?
                            .error_for_status()?
                            .json::<serde_json::Value>()
                            .await
                    };
                    if let Ok(Ok(v)) = tokio::time::timeout(io_timeout, fetch).await {
                        ui.last_runtime_config_loaded_at_ms =
                            v.get("loaded_at_ms").and_then(|x| x.as_u64());
                        ui.last_runtime_config_source_mtime_ms =
                            v.get("source_mtime_ms").and_then(|x| x.as_u64());
                        ui.last_runtime_retry = v
                            .get("retry")
                            .and_then(|x| serde_json::from_value(x.clone()).ok());
                    }
                    ui.last_runtime_config_refresh_at = Some(Instant::now());
                }
                if ui.uses_route_graph_routing()
                    && ui.page == crate::tui::types::Page::Stations
                    && ui
                        .last_routing_control_refresh_at
                        .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
                {
                    let refresh = input::refresh_routing_control_state(&mut ui);
                    match tokio::time::timeout(io_timeout, refresh).await {
                        Ok(Ok(())) => {
                            ui.clamp_selection(&snapshot, ui.station_page_rows_len(providers.len()));
                        }
                        Ok(Err(err)) => {
                            ui.last_routing_control_refresh_at = Some(Instant::now());
                            ui.toast =
                                Some((format!("routing refresh failed: {err}"), Instant::now()));
                        }
                        Err(_) => {
                            ui.last_routing_control_refresh_at = Some(Instant::now());
                        }
                    }
                }
                request_redraw(&mut render_invalidation);
            }
            maybe_balance_refresh = balance_refresh_rx.recv() => {
                if let Some(result) = maybe_balance_refresh {
                    if let Err(err) = result {
                        ui.toast = Some((format!("balance refresh failed: {err}"), Instant::now()));
                    }
                    let refresh = refresh_snapshot(&state, cfg.clone(), service_name, ui.stats_days);
                    if let Ok(new_snapshot) = tokio::time::timeout(io_timeout, refresh).await {
                        snapshot = new_snapshot;
                        ui.clamp_selection(&snapshot, ui.station_page_rows_len(providers.len()));
                    }
                    request_redraw(&mut render_invalidation);
                }
            }
            maybe_history_refresh = history_refresh_rx.recv() => {
                if let Some(result) = maybe_history_refresh
                    && apply_codex_history_refresh_result(&mut ui, result)
                {
                    request_redraw(&mut render_invalidation);
                }
            }
            maybe_recent_refresh = recent_refresh_rx.recv() => {
                if let Some(result) = maybe_recent_refresh
                    && apply_codex_recent_refresh_result(&mut ui, result)
                {
                    request_redraw(&mut render_invalidation);
                }
            }
            changed = shutdown_rx.changed() => {
                let _ = changed;
                ui.should_exit = true;
                let _ = shutdown.send(true);
                break;
            }
            _ = &mut ctrl_c => {
                ui.should_exit = true;
                let _ = shutdown.send(true);
                break;
            }
            maybe_event = events.next() => {
                let Some(Ok(event)) = maybe_event else { continue; };
                match event {
                    Event::Key(key) if input::should_accept_key_event(&key) => {
                        let before_surface = RenderSurfaceKey::capture(&ui);
                        if input::handle_key_event(
                            state.clone(),
                            &mut providers,
                            &mut ui,
                            &snapshot,
                            balance_refresh_tx.clone(),
                            key,
                        )
                        .await
                        {
                            if ui.needs_config_refresh {
                                match load_config().await {
                                    Ok(new_cfg) => {
                                        cfg = Arc::new(new_cfg);
                                        ui.config_version = cfg.version;
                                        providers = build_provider_options(&cfg, service_name);
                                        ui.clamp_selection(
                                            &snapshot,
                                            ui.station_page_rows_len(providers.len()),
                                        );
                                    }
                                    Err(err) => {
                                        ui.toast = Some((
                                            format!("config refresh failed: {err}"),
                                            Instant::now(),
                                        ));
                                    }
                                }
                                ui.needs_config_refresh = false;
                            }
                            if ui.needs_snapshot_refresh {
                                snapshot = refresh_snapshot(&state, cfg.clone(), service_name, ui.stats_days).await;
                                ui.clamp_selection(
                                    &snapshot,
                                    ui.station_page_rows_len(providers.len()),
                                );
                                ui.needs_snapshot_refresh = false;
                            }
                            if ui.needs_codex_history_refresh {
                                start_codex_history_refresh(&mut ui, history_refresh_tx.clone());
                                ui.needs_codex_history_refresh = false;
                            }
                            if ui.needs_codex_recent_refresh {
                                start_codex_recent_refresh(&mut ui, recent_refresh_tx.clone());
                                ui.needs_codex_recent_refresh = false;
                            }
                            let after_surface = RenderSurfaceKey::capture(&ui);
                            if before_surface != after_surface {
                                request_full_clear(&mut render_invalidation);
                            } else {
                                request_redraw(&mut render_invalidation);
                            }
                        }
                    }
                    Event::Resize(_, _) => {
                        ui.reset_table_viewports();
                        request_full_clear(&mut render_invalidation);
                    }
                    _ => {}
                }
            }
        }
    }

    terminal.show_cursor()?;
    crossterm::terminal::disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    term_guard.disarm();
    Ok(())
}

fn start_codex_history_refresh(
    ui: &mut UiState,
    tx: mpsc::UnboundedSender<CodexHistoryRefreshResult>,
) {
    ui.codex_history_refresh_generation = ui.codex_history_refresh_generation.wrapping_add(1);
    let generation = ui.codex_history_refresh_generation;
    ui.codex_history_loading = true;
    ui.codex_history_error = None;
    ui.toast = Some((
        i18n::text(ui.language, i18n::msg::HISTORY_REFRESHING).to_string(),
        Instant::now(),
    ));

    tokio::spawn(async move {
        let result = crate::sessions::find_codex_sessions_for_current_dir(200)
            .await
            .map_err(|err| err.to_string());
        let _ = tx.send(CodexHistoryRefreshResult { generation, result });
    });
}

fn apply_codex_history_refresh_result(ui: &mut UiState, result: CodexHistoryRefreshResult) -> bool {
    if result.generation != ui.codex_history_refresh_generation {
        return false;
    }

    ui.codex_history_loading = false;
    ui.codex_history_loaded_at_ms = Some(now_ms());
    match result.result {
        Ok(list) => {
            ui.codex_history_sessions = list;
            if let Some(focus) = ui.codex_history_external_focus.as_ref() {
                merge_codex_history_external_focus(&mut ui.codex_history_sessions, focus);
            }
            ui.codex_history_error = None;
            ui.sync_codex_history_selection();
            ui.toast = Some((
                i18n::format_history_loaded(ui.language, ui.codex_history_sessions.len()),
                Instant::now(),
            ));
        }
        Err(err) => {
            if let Some(focus) = ui.codex_history_external_focus.as_ref() {
                merge_codex_history_external_focus(&mut ui.codex_history_sessions, focus);
            }
            ui.codex_history_error = Some(err.clone());
            ui.sync_codex_history_selection();
            ui.toast = Some((
                i18n::format_history_load_failed(ui.language, &err),
                Instant::now(),
            ));
        }
    }
    true
}

fn start_codex_recent_refresh(
    ui: &mut UiState,
    tx: mpsc::UnboundedSender<CodexRecentRefreshResult>,
) {
    ui.codex_recent_refresh_generation = ui.codex_recent_refresh_generation.wrapping_add(1);
    let generation = ui.codex_recent_refresh_generation;
    let raw_cwd = ui.codex_recent_raw_cwd;
    let branch_cache = ui.codex_recent_branch_cache.clone();
    ui.codex_recent_loading = true;
    ui.codex_recent_error = None;
    ui.toast = Some((
        i18n::text(ui.language, i18n::msg::RECENT_REFRESHING).to_string(),
        Instant::now(),
    ));

    tokio::spawn(async move {
        let result = load_codex_recent_rows(raw_cwd, branch_cache)
            .await
            .map_err(|err| err.to_string());
        let _ = tx.send(CodexRecentRefreshResult { generation, result });
    });
}

fn apply_codex_recent_refresh_result(ui: &mut UiState, result: CodexRecentRefreshResult) -> bool {
    if result.generation != ui.codex_recent_refresh_generation {
        return false;
    }

    ui.codex_recent_loading = false;
    ui.codex_recent_loaded_at_ms = Some(now_ms());
    match result.result {
        Ok(payload) => {
            ui.codex_recent_rows = payload.rows;
            ui.codex_recent_branch_cache = payload.branch_cache;
            ui.codex_recent_error = None;
            ui.codex_recent_selected_idx = 0;
            ui.codex_recent_selected_id =
                ui.codex_recent_rows.first().map(|r| r.session_id.clone());
            ui.codex_recent_table
                .select((!ui.codex_recent_rows.is_empty()).then_some(0));
            ui.toast = Some((
                i18n::format_recent_loaded(ui.language, ui.codex_recent_rows.len()),
                Instant::now(),
            ));
        }
        Err(err) => {
            ui.codex_recent_error = Some(err.clone());
            ui.toast = Some((
                i18n::format_recent_load_failed(ui.language, &err),
                Instant::now(),
            ));
        }
    }
    true
}

async fn load_codex_recent_rows(
    raw_cwd: bool,
    mut branch_cache: std::collections::HashMap<String, Option<String>>,
) -> anyhow::Result<CodexRecentRefreshPayload> {
    let since = Duration::from_secs(24 * 60 * 60);
    let list = crate::sessions::find_recent_codex_sessions(since, 500).await?;
    let mut rows = Vec::with_capacity(list.len());
    for s in list {
        let cwd_opt = s.cwd.clone();
        let cwd = cwd_opt.as_deref().unwrap_or("-");
        let root = if raw_cwd {
            cwd.to_string()
        } else {
            crate::sessions::infer_project_root_from_cwd(cwd)
        };
        let branch =
            if root.trim().is_empty() || root == "-" || !std::path::Path::new(&root).exists() {
                None
            } else if let Some(v) = branch_cache.get(&root) {
                v.clone()
            } else {
                let v = read_git_branch_shallow(&root).await;
                branch_cache.insert(root.clone(), v.clone());
                v
            };
        rows.push(RecentCodexRow {
            root,
            branch,
            session_id: s.id,
            cwd: cwd_opt,
            mtime_ms: s.mtime_ms,
        });
    }

    Ok(CodexRecentRefreshPayload { rows, branch_cache })
}

async fn read_git_branch_shallow(workdir: &str) -> Option<String> {
    use tokio::fs;

    let root = std::path::PathBuf::from(workdir);
    if !root.is_absolute() {
        return None;
    }

    let dot_git = root.join(".git");
    if !dot_git.exists() {
        return None;
    }

    let gitdir = if dot_git.is_dir() {
        dot_git
    } else {
        let content = fs::read_to_string(&dot_git).await.ok()?;
        let first = content.lines().next()?.trim();
        let path = first.strip_prefix("gitdir:")?.trim();
        let mut p = std::path::PathBuf::from(path);
        if p.is_relative() {
            p = root.join(p);
        }
        p
    };

    let head = fs::read_to_string(gitdir.join("HEAD")).await.ok()?;
    let head = head.lines().next().unwrap_or("").trim();
    if let Some(r) = head.strip_prefix("ref:") {
        let r = r.trim();
        return Some(r.rsplit('/').next().unwrap_or(r).to_string());
    }
    if head.len() >= 8 {
        Some(head[..8].to_string())
    } else if head.is_empty() {
        None
    } else {
        Some(head.to_string())
    }
}

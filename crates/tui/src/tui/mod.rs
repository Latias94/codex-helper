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

use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use crossterm::ExecutableCommand;
use crossterm::event::{Event, EventStream};
use crossterm::terminal::LeaveAlternateScreen;
use futures_util::StreamExt;
use futures_util::stream;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::{mpsc, watch};

use crate::config::ProxyConfig;
use crate::config::storage::load_config;
use crate::proxy::ProxyService;
use crate::state::ProxyState;
use crate::usage_providers::UsageProviderRefreshSummary;

use self::model::{Palette, Snapshot, now_ms, refresh_snapshot};
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

#[derive(Debug)]
struct SnapshotRefreshResult {
    generation: u64,
    config_version: Option<u32>,
    stats_days: usize,
    snapshot: Snapshot,
}

fn snapshot_refresh_result_is_current(
    result_generation: u64,
    result_config_version: Option<u32>,
    result_stats_days: usize,
    current_generation: u64,
    current_config_version: Option<u32>,
    current_stats_days: usize,
) -> bool {
    result_generation == current_generation
        && result_config_version == current_config_version
        && result_stats_days == current_stats_days
}

#[derive(Debug)]
struct SnapshotRefreshController {
    tx: mpsc::UnboundedSender<SnapshotRefreshResult>,
    generation: u64,
    in_flight: Option<u64>,
    pending: bool,
}

impl SnapshotRefreshController {
    fn new(tx: mpsc::UnboundedSender<SnapshotRefreshResult>) -> Self {
        Self {
            tx,
            generation: 0,
            in_flight: None,
            pending: false,
        }
    }

    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.in_flight = None;
        self.pending = false;
    }

    fn request(
        &mut self,
        state: Arc<ProxyState>,
        cfg: Arc<ProxyConfig>,
        service_name: &'static str,
        stats_days: usize,
    ) {
        if self.in_flight.is_some() {
            self.pending = true;
            return;
        }

        self.start(state, cfg, service_name, stats_days);
    }

    fn finish(&mut self, generation: u64) {
        if self.in_flight == Some(generation) {
            self.in_flight = None;
        }
    }

    fn result_is_current(
        &self,
        result: &SnapshotRefreshResult,
        current_config_version: Option<u32>,
        current_stats_days: usize,
    ) -> bool {
        snapshot_refresh_result_is_current(
            result.generation,
            result.config_version,
            result.stats_days,
            self.generation,
            current_config_version,
            current_stats_days,
        )
    }

    fn request_pending_if_idle(
        &mut self,
        state: Arc<ProxyState>,
        cfg: Arc<ProxyConfig>,
        service_name: &'static str,
        stats_days: usize,
    ) {
        if self.pending && self.in_flight.is_none() {
            self.request(state, cfg, service_name, stats_days);
        }
    }

    fn start(
        &mut self,
        state: Arc<ProxyState>,
        cfg: Arc<ProxyConfig>,
        service_name: &'static str,
        stats_days: usize,
    ) {
        debug_assert!(self.in_flight.is_none());
        self.pending = false;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.in_flight = Some(generation);
        let config_version = cfg.version;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let snapshot = refresh_snapshot(&state, cfg, service_name, stats_days).await;
            let _ = tx.send(SnapshotRefreshResult {
                generation,
                config_version,
                stats_days,
                snapshot,
            });
        });
    }
}

fn balance_refresh_summary_message(
    lang: Language,
    summary: &UsageProviderRefreshSummary,
) -> String {
    if summary.deduplicated > 0 && summary.attempted == 0 {
        return match lang {
            Language::Zh => "余额刷新已在进行中".to_string(),
            Language::En => "balance refresh already requested".to_string(),
        };
    }

    let mut parts = Vec::new();
    match lang {
        Language::Zh => {
            parts.push(format!("成功 {}/{}", summary.refreshed, summary.attempted));
            if summary.failed > 0 {
                parts.push(format!("失败 {}", summary.failed));
            }
            if summary.missing_token > 0 {
                parts.push(format!("缺 key {}", summary.missing_token));
            }
            if summary.auto_attempted > 0 {
                parts.push(format!(
                    "自动 {}/{}",
                    summary.auto_refreshed, summary.auto_attempted
                ));
            }
            if summary.deduplicated > 0 {
                parts.push(format!("去重 {}", summary.deduplicated));
            }
        }
        Language::En => {
            parts.push(format!("ok {}/{}", summary.refreshed, summary.attempted));
            if summary.failed > 0 {
                parts.push(format!("failed {}", summary.failed));
            }
            if summary.missing_token > 0 {
                parts.push(format!("missing key {}", summary.missing_token));
            }
            if summary.auto_attempted > 0 {
                parts.push(format!(
                    "auto {}/{}",
                    summary.auto_refreshed, summary.auto_attempted
                ));
            }
            if summary.deduplicated > 0 {
                parts.push(format!("dedup {}", summary.deduplicated));
            }
        }
    }
    parts.join(" · ")
}

async fn refresh_runtime_config_if_due(
    ui: &mut UiState,
    proxy: &ProxyService,
    io_timeout: Duration,
) {
    if ui.page != crate::tui::types::Page::Settings
        || ui
            .last_runtime_config_refresh_at
            .is_some_and(|t| t.elapsed() <= Duration::from_secs(2))
    {
        return;
    }

    if let Ok(status) = tokio::time::timeout(io_timeout, proxy.runtime_status()).await {
        ui.last_runtime_config_loaded_at_ms = Some(status.loaded_at_ms);
        ui.last_runtime_config_source_mtime_ms = status.source_mtime_ms;
        ui.last_runtime_retry = Some(status.retry);
    }
    ui.last_runtime_config_refresh_at = Some(Instant::now());
}

#[allow(clippy::too_many_arguments)]
async fn refresh_route_graph_control_if_needed(
    ui: &mut UiState,
    proxy: &ProxyService,
    snapshot: &Snapshot,
    providers_len: usize,
    io_timeout: Duration,
    force: bool,
    suppress_error_toast: bool,
) {
    if !ui.uses_route_graph_routing()
        || ui.page != crate::tui::types::Page::Stations
        || (!force
            && ui
                .last_routing_control_refresh_at
                .is_some_and(|t| t.elapsed() <= Duration::from_secs(5)))
    {
        return;
    }

    let refresh = input::refresh_routing_control_state(ui, proxy);
    match tokio::time::timeout(io_timeout, refresh).await {
        Ok(Ok(())) => {
            ui.clamp_selection(snapshot, ui.station_page_rows_len(providers_len));
        }
        Ok(Err(err)) => {
            ui.last_routing_control_refresh_at = Some(Instant::now());
            if !suppress_error_toast {
                ui.toast = Some((format!("routing refresh failed: {err}"), Instant::now()));
            }
        }
        Err(_) => {
            ui.last_routing_control_refresh_at = Some(Instant::now());
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_ticker_refreshes(
    ui: &mut UiState,
    proxy: &ProxyService,
    state: Arc<ProxyState>,
    cfg: Arc<ProxyConfig>,
    service_name: &'static str,
    snapshot: &Snapshot,
    providers_len: usize,
    io_timeout: Duration,
    snapshot_fallback_interval: Duration,
    snapshot_refresh: &mut SnapshotRefreshController,
) {
    if snapshot.refreshed_at.elapsed() >= snapshot_fallback_interval {
        snapshot_refresh.request(state, cfg, service_name, ui.stats_days);
    }
    refresh_runtime_config_if_due(ui, proxy, io_timeout).await;
    refresh_route_graph_control_if_needed(
        ui,
        proxy,
        snapshot,
        providers_len,
        io_timeout,
        false,
        false,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn handle_balance_refresh_result(
    ui: &mut UiState,
    proxy: &ProxyService,
    state: Arc<ProxyState>,
    cfg: Arc<ProxyConfig>,
    service_name: &'static str,
    snapshot: &Snapshot,
    providers_len: usize,
    io_timeout: Duration,
    snapshot_refresh: &mut SnapshotRefreshController,
    result: input::BalanceRefreshOutcome,
) {
    ui.balance_refresh_in_flight = false;
    ui.last_balance_refresh_finished_at = Some(Instant::now());
    match result {
        Ok(summary) => {
            ui.last_balance_refresh_summary = Some(summary.clone());
            ui.last_balance_refresh_error = None;
            ui.last_balance_refresh_message =
                Some(balance_refresh_summary_message(ui.language, &summary));
        }
        Err(err) => {
            ui.last_balance_refresh_summary = None;
            ui.last_balance_refresh_message = None;
            ui.last_balance_refresh_error = Some(err.clone());
            ui.toast = Some((format!("balance refresh failed: {err}"), Instant::now()));
        }
    }
    snapshot_refresh.request(state, cfg, service_name, ui.stats_days);
    refresh_route_graph_control_if_needed(
        ui,
        proxy,
        snapshot,
        providers_len,
        io_timeout,
        true,
        ui.last_balance_refresh_error.is_some(),
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn apply_pending_refresh_requests(
    ui: &mut UiState,
    state: Arc<ProxyState>,
    cfg: &mut Arc<ProxyConfig>,
    service_name: &'static str,
    snapshot: &Snapshot,
    providers: &mut Vec<ProviderOption>,
    snapshot_refresh: &mut SnapshotRefreshController,
    history_refresh_tx: mpsc::UnboundedSender<CodexHistoryRefreshResult>,
    recent_refresh_tx: mpsc::UnboundedSender<CodexRecentRefreshResult>,
) {
    if ui.needs_config_refresh {
        match load_config().await {
            Ok(new_cfg) => {
                *cfg = Arc::new(new_cfg);
                ui.config_version = cfg.version;
                snapshot_refresh.invalidate();
                *providers = build_provider_options(cfg.as_ref(), service_name);
                snapshot_refresh.request(state.clone(), cfg.clone(), service_name, ui.stats_days);
                ui.clamp_selection(snapshot, ui.station_page_rows_len(providers.len()));
            }
            Err(err) => {
                ui.toast = Some((format!("config refresh failed: {err}"), Instant::now()));
            }
        }
        ui.needs_config_refresh = false;
    }
    if ui.needs_snapshot_refresh {
        snapshot_refresh.invalidate();
        snapshot_refresh.request(state.clone(), cfg.clone(), service_name, ui.stats_days);
        ui.needs_snapshot_refresh = false;
    }
    if ui.needs_codex_history_refresh {
        start_codex_history_refresh(ui, history_refresh_tx);
        ui.needs_codex_history_refresh = false;
    }
    if ui.needs_codex_recent_refresh {
        start_codex_recent_refresh(ui, recent_refresh_tx);
        ui.needs_codex_recent_refresh = false;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RenderSurfaceKey {
    page: types::Page,
    overlay: types::Overlay,
    focus: types::Focus,
    stats_focus: types::StatsFocus,
    stats_attention_only: bool,
    stats_errors_only: bool,
    selected_station_idx: usize,
    selected_session_idx: usize,
    selected_request_idx: usize,
    selected_request_page_idx: usize,
    selected_sessions_page_idx: usize,
    selected_codex_history_idx: usize,
    codex_recent_selected_idx: usize,
    selected_stats_station_idx: usize,
    selected_stats_provider_idx: usize,
    stats_provider_detail_scroll: u16,
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
    branch_cache: HashMap<String, Option<String>>,
}

const CODEX_RECENT_BRANCH_LOOKUP_CONCURRENCY: usize = 8;

impl RenderSurfaceKey {
    fn capture(ui: &UiState) -> Self {
        Self {
            page: ui.page,
            overlay: ui.overlay,
            focus: ui.focus,
            stats_focus: ui.stats_focus,
            stats_attention_only: ui.stats_attention_only,
            stats_errors_only: ui.stats_errors_only,
            selected_station_idx: ui.selected_station_idx,
            selected_session_idx: ui.selected_session_idx,
            selected_request_idx: ui.selected_request_idx,
            selected_request_page_idx: ui.selected_request_page_idx,
            selected_sessions_page_idx: ui.selected_sessions_page_idx,
            selected_codex_history_idx: ui.selected_codex_history_idx,
            codex_recent_selected_idx: ui.codex_recent_selected_idx,
            selected_stats_station_idx: ui.selected_stats_station_idx,
            selected_stats_provider_idx: ui.selected_stats_provider_idx,
            stats_provider_detail_scroll: ui.stats_provider_detail_scroll,
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
    proxy: ProxyService,
    state: Arc<ProxyState>,
    cfg: Arc<ProxyConfig>,
    service_name: &'static str,
    port: u16,
    _admin_port: u16,
    providers: Vec<ProviderOption>,
    language: Language,
    shutdown: watch::Sender<bool>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let refresh_ms = std::env::var("CODEX_HELPER_TUI_REFRESH_MS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(1_000)
        .clamp(250, 5_000);

    let io_timeout = Duration::from_millis((refresh_ms / 2).clamp(50, 250));
    let snapshot_fallback_interval = Duration::from_secs(
        std::env::var("CODEX_HELPER_TUI_SNAPSHOT_FALLBACK_SECS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(10)
            .clamp(2, 300),
    );

    let mut term_guard = TerminalGuard::enter()?;
    let stdout = io::stdout();

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let mut ui = UiState {
        service_name,
        language,
        refresh_ms,
        config_version: cfg.version,
        ..Default::default()
    };
    let _ = input::refresh_profile_control_state(&mut ui, &proxy).await;
    let palette = Palette::default();

    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(refresh_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut state_changes = state.subscribe_state_changes();

    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());

    let mut cfg = cfg;
    let mut snapshot = refresh_snapshot(&state, cfg.clone(), service_name, ui.stats_days).await;
    let mut providers = providers;
    ui.clamp_selection(&snapshot, ui.station_page_rows_len(providers.len()));
    let (balance_refresh_tx, mut balance_refresh_rx) =
        mpsc::unbounded_channel::<input::BalanceRefreshOutcome>();
    let (snapshot_refresh_tx, mut snapshot_refresh_rx) =
        mpsc::unbounded_channel::<SnapshotRefreshResult>();
    let (history_refresh_tx, mut history_refresh_rx) =
        mpsc::unbounded_channel::<CodexHistoryRefreshResult>();
    let (recent_refresh_tx, mut recent_refresh_rx) =
        mpsc::unbounded_channel::<CodexRecentRefreshResult>();
    let mut snapshot_refresh = SnapshotRefreshController::new(snapshot_refresh_tx);

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
                if ui.uses_route_graph_routing() && ui.page == types::Page::Stations {
                    let _ = input::request_provider_balance_refresh(
                        &mut ui,
                        &snapshot,
                        &proxy,
                        input::BalanceRefreshMode::Auto,
                        &balance_refresh_tx,
                    );
                }
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
                handle_ticker_refreshes(
                    &mut ui,
                    &proxy,
                    state.clone(),
                    cfg.clone(),
                    service_name,
                    &snapshot,
                    providers.len(),
                    io_timeout,
                    snapshot_fallback_interval,
                    &mut snapshot_refresh,
                ).await;
                request_redraw(&mut render_invalidation);
            }
            changed = state_changes.changed() => {
                if changed.is_ok() {
                    snapshot_refresh.request(
                        state.clone(),
                        cfg.clone(),
                        service_name,
                        ui.stats_days,
                    );
                }
            }
            maybe_snapshot_refresh = snapshot_refresh_rx.recv() => {
                if let Some(result) = maybe_snapshot_refresh {
                    snapshot_refresh.finish(result.generation);
                    if snapshot_refresh.result_is_current(&result, cfg.version, ui.stats_days) {
                        snapshot = result.snapshot;
                        ui.clamp_selection(&snapshot, ui.station_page_rows_len(providers.len()));
                        request_redraw(&mut render_invalidation);
                    }
                    snapshot_refresh.request_pending_if_idle(
                        state.clone(),
                        cfg.clone(),
                        service_name,
                        ui.stats_days,
                    );
                }
            }
            maybe_balance_refresh = balance_refresh_rx.recv() => {
                if let Some(result) = maybe_balance_refresh {
                    handle_balance_refresh_result(
                        &mut ui,
                        &proxy,
                        state.clone(),
                        cfg.clone(),
                        service_name,
                        &snapshot,
                        providers.len(),
                        io_timeout,
                        &mut snapshot_refresh,
                        result,
                    ).await;
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
                            &proxy,
                            balance_refresh_tx.clone(),
                            key,
                        )
                        .await
                        {
                            apply_pending_refresh_requests(
                                &mut ui,
                                state.clone(),
                                &mut cfg,
                                service_name,
                                &snapshot,
                                &mut providers,
                                &mut snapshot_refresh,
                                history_refresh_tx.clone(),
                                recent_refresh_tx.clone(),
                            ).await;
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
    mut branch_cache: HashMap<String, Option<String>>,
) -> anyhow::Result<CodexRecentRefreshPayload> {
    let since = Duration::from_secs(24 * 60 * 60);
    let list = crate::sessions::find_recent_codex_sessions(since, 500).await?;
    let mut rows = Vec::with_capacity(list.len());
    let mut missing_roots = Vec::new();
    let mut missing_seen = HashSet::new();
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
                if missing_seen.insert(root.clone()) {
                    missing_roots.push(root.clone());
                }
                None
            };
        rows.push(RecentCodexRow {
            root,
            branch,
            session_id: s.id,
            cwd: cwd_opt,
            mtime_ms: s.mtime_ms,
        });
    }

    let mut branch_stream = stream::iter(missing_roots)
        .map(|root| async move {
            let branch = read_git_branch_shallow(&root).await;
            (root, branch)
        })
        .buffer_unordered(CODEX_RECENT_BRANCH_LOOKUP_CONCURRENCY);
    while let Some((root, branch)) = branch_stream.next().await {
        branch_cache.insert(root, branch);
    }

    for row in &mut rows {
        if row.branch.is_none()
            && let Some(branch) = branch_cache.get(&row.root)
        {
            row.branch = branch.clone();
        }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{SnapshotRefreshController, snapshot_refresh_result_is_current};
    use crate::config::ProxyConfig;
    use crate::state::ProxyState;

    #[test]
    fn snapshot_refresh_result_guard_rejects_stale_results() {
        assert!(snapshot_refresh_result_is_current(
            3,
            Some(5),
            7,
            3,
            Some(5),
            7
        ));
        assert!(!snapshot_refresh_result_is_current(
            2,
            Some(5),
            7,
            3,
            Some(5),
            7
        ));
        assert!(!snapshot_refresh_result_is_current(
            3,
            Some(4),
            7,
            3,
            Some(5),
            7
        ));
        assert!(!snapshot_refresh_result_is_current(
            3,
            Some(5),
            30,
            3,
            Some(5),
            7
        ));
    }

    #[test]
    fn snapshot_refresh_controller_invalidation_clears_task_state() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut controller = SnapshotRefreshController::new(tx);
        controller.generation = 41;
        controller.in_flight = Some(41);
        controller.pending = true;

        controller.invalidate();

        assert_eq!(controller.generation, 42);
        assert_eq!(controller.in_flight, None);
        assert!(!controller.pending);
    }

    #[test]
    fn snapshot_refresh_controller_request_marks_pending_without_invalidating_in_flight() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut controller = SnapshotRefreshController::new(tx);
        controller.generation = 7;
        controller.in_flight = Some(7);

        controller.request(
            ProxyState::new(),
            Arc::new(ProxyConfig::default()),
            "codex",
            7,
        );

        assert_eq!(controller.generation, 7);
        assert_eq!(controller.in_flight, Some(7));
        assert!(controller.pending);
    }

    #[tokio::test]
    async fn snapshot_refresh_controller_restarts_pending_work_after_current_finish() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut controller = SnapshotRefreshController::new(tx);
        controller.generation = 7;
        controller.in_flight = Some(7);
        controller.pending = true;

        controller.finish(7);
        controller.request_pending_if_idle(
            ProxyState::new(),
            Arc::new(ProxyConfig::default()),
            "codex",
            7,
        );

        assert_eq!(controller.generation, 8);
        assert_eq!(controller.in_flight, Some(8));
        assert!(!controller.pending);
    }
}

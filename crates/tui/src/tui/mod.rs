mod i18n;
mod input;
mod model;
mod report;
mod session_refresh;
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
use crate::proxy::ProxyService;
use crate::state::ProxyState;
use crate::usage_providers::UsageProviderRefreshSummary;

use self::model::{Palette, Snapshot, refresh_snapshot};
use self::session_refresh::{
    CodexHistoryRefreshResult, CodexRecentRefreshResult, apply_codex_history_refresh_result,
    apply_codex_recent_refresh_result, start_codex_history_refresh, start_codex_recent_refresh,
};
use self::state::UiState;
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

#[derive(Debug, Clone, Copy)]
struct DashboardTiming {
    refresh_ms: u64,
    io_timeout: Duration,
    snapshot_fallback_interval: Duration,
}

impl DashboardTiming {
    fn from_env() -> Self {
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

        Self {
            refresh_ms,
            io_timeout,
            snapshot_fallback_interval,
        }
    }
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

type DashboardTerminal = Terminal<CrosstermBackend<io::Stdout>>;

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
fn render_dashboard_if_needed(
    terminal: &mut DashboardTerminal,
    render_invalidation: &mut RenderInvalidation,
    last_drawn_page: &mut types::Page,
    ui: &mut UiState,
    snapshot: &Snapshot,
    proxy: &ProxyService,
    balance_refresh_tx: &input::BalanceRefreshSender,
    palette: Palette,
    service_name: &'static str,
    port: u16,
    providers: &[ProviderOption],
) -> anyhow::Result<()> {
    if *render_invalidation == RenderInvalidation::None {
        return Ok(());
    }

    if ui.page != *last_drawn_page {
        // Defensive: some terminals occasionally leave stale cells when only a small
        // region changes (e.g., switching tabs). A full clear on page switch keeps the
        // UI visually consistent without clearing on every tick.
        request_full_clear(render_invalidation);
        ui.reset_table_viewports();
        *last_drawn_page = ui.page;
        if ui.uses_route_graph_routing() && ui.page == types::Page::Stations {
            let _ = input::request_provider_balance_refresh(
                ui,
                snapshot,
                proxy,
                input::BalanceRefreshMode::Auto,
                balance_refresh_tx,
            );
        }
    }
    if matches!(render_invalidation, RenderInvalidation::FullClear) {
        terminal.clear()?;
    }
    terminal.draw(|f| view::render_app(f, palette, ui, snapshot, service_name, port, providers))?;
    *render_invalidation = RenderInvalidation::None;
    Ok(())
}

fn enter_dashboard_terminal() -> anyhow::Result<(TerminalGuard, DashboardTerminal)> {
    let term_guard = TerminalGuard::enter()?;
    let stdout = io::stdout();

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    Ok((term_guard, terminal))
}

fn leave_dashboard_terminal(
    mut term_guard: TerminalGuard,
    terminal: &mut DashboardTerminal,
) -> anyhow::Result<()> {
    terminal.show_cursor()?;
    crossterm::terminal::disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    term_guard.disarm();
    Ok(())
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
    let timing = DashboardTiming::from_env();
    let (term_guard, mut terminal) = enter_dashboard_terminal()?;

    let mut ui = UiState {
        service_name,
        language,
        refresh_ms: timing.refresh_ms,
        config_version: cfg.version,
        ..Default::default()
    };
    let _ = input::refresh_profile_control_state(&mut ui, &proxy).await;
    let palette = Palette::default();

    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(timing.refresh_ms));
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
        render_dashboard_if_needed(
            &mut terminal,
            &mut render_invalidation,
            &mut last_drawn_page,
            &mut ui,
            &snapshot,
            &proxy,
            &balance_refresh_tx,
            palette,
            service_name,
            port,
            &providers,
        )?;

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
                    timing.io_timeout,
                    timing.snapshot_fallback_interval,
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
                        timing.io_timeout,
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

    leave_dashboard_terminal(term_guard, &mut terminal)
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

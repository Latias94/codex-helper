mod i18n;
mod input;
mod model;
mod report;
mod runtime_refresh;
mod session_refresh;
mod snapshot_refresh;
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

use crossterm::ExecutableCommand;
use crossterm::event::{Event, EventStream};
use crossterm::terminal::LeaveAlternateScreen;
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::{mpsc, watch};

use crate::config::ProxyConfig;
use crate::proxy::ProxyService;
use crate::state::ProxyState;

use self::model::{Palette, Snapshot, refresh_snapshot};
use self::runtime_refresh::{
    DashboardTiming, apply_pending_refresh_requests, handle_balance_refresh_result,
    handle_ticker_refreshes,
};
use self::session_refresh::{
    CodexHistoryRefreshResult, CodexRecentRefreshResult, apply_codex_history_refresh_result,
    apply_codex_recent_refresh_result,
};
use self::snapshot_refresh::{SnapshotRefreshController, SnapshotRefreshResult};
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

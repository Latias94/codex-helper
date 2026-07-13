mod attached;
mod fleet_refresh;
mod i18n;
mod input;
mod model;
mod operator_projection;
mod report;
mod runtime_refresh;
mod session_refresh;
mod snapshot_refresh;
mod state;
mod terminal;
mod types;
mod view;

pub use attached::{run_attached_dashboard, run_attached_dashboard_with_admin_base_url};
pub use i18n::Language;
pub use i18n::{detect_system_language, parse_language, resolve_language_preference};
#[allow(unused_imports)]
pub use model::{ProviderOption, UpstreamSummary};

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

use crate::codex_integration::CodexStartupReadiness;
use crate::config::HelperConfig;
use crate::proxy::ProxyService;
use crate::state::ProxyState;

use self::fleet_refresh::{
    FleetRefreshResult, FleetRefreshSource, apply_fleet_refresh_result, start_fleet_refresh,
};
use self::model::{Palette, Snapshot};
use self::operator_projection::apply_operator_read_model;
use self::runtime_refresh::{
    DashboardTiming, apply_pending_refresh_requests, handle_ticker_refreshes,
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

fn start_integrated_fleet_refresh(
    ui: &mut UiState,
    cfg: &Arc<HelperConfig>,
    tx: mpsc::UnboundedSender<FleetRefreshResult>,
) {
    let model = ui
        .operator_read_model
        .clone()
        .unwrap_or_else(|| crate::dashboard_core::OperatorReadModel::disconnected(ui.service_name));
    start_fleet_refresh(
        ui,
        FleetRefreshSource::Integrated {
            model: Box::new(model),
            cfg: cfg.clone(),
        },
        tx,
    );
    ui.needs_fleet_refresh = false;
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
    selected_fleet_node_idx: usize,
    selected_fleet_unit_idx: usize,
    fleet_loading: bool,
    fleet_refresh_generation: u64,
    fleet_view_mode: state::FleetViewMode,
    selected_stats_provider_endpoint_idx: usize,
    selected_stats_provider_idx: usize,
    stats_provider_detail_scroll: u16,
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
            selected_fleet_node_idx: ui.selected_fleet_node_idx,
            selected_fleet_unit_idx: ui.selected_fleet_unit_idx,
            fleet_loading: ui.fleet_loading,
            fleet_refresh_generation: ui.fleet_refresh_generation,
            fleet_view_mode: ui.fleet_view_mode,
            selected_stats_provider_endpoint_idx: ui.selected_stats_provider_endpoint_idx,
            selected_stats_provider_idx: ui.selected_stats_provider_idx,
            stats_provider_detail_scroll: ui.stats_provider_detail_scroll,
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
    cfg: Arc<HelperConfig>,
    service_name: &'static str,
    port: u16,
    _admin_port: u16,
    startup_readiness: Option<CodexStartupReadiness>,
    language: Language,
    _shutdown: watch::Sender<bool>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let timing = DashboardTiming::from_env();
    let (term_guard, mut terminal) = enter_dashboard_terminal()?;

    let show_startup_alert = startup_readiness
        .as_ref()
        .is_some_and(CodexStartupReadiness::has_issues);
    let mut ui = UiState {
        service_name,
        proxy_port: port,
        language,
        config_version: Some(cfg.version),
        overlay: if show_startup_alert {
            types::Overlay::StartupAlert
        } else {
            types::Overlay::None
        },
        startup_readiness,
        ..Default::default()
    };
    let palette = Palette::default();

    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(timing.refresh_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut state_changes = state.subscribe_state_changes();

    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());

    let mut snapshot = Snapshot::default();
    let mut providers = Vec::new();
    let mut local_session_ids = std::collections::HashMap::new();
    let initial_model = match proxy.operator_read_capture().await {
        Ok(capture) => {
            local_session_ids = capture.local_session_ids;
            capture.model
        }
        Err(_) => crate::dashboard_core::OperatorReadModel::disconnected(service_name),
    };
    apply_operator_read_model(
        &mut ui,
        &mut snapshot,
        &mut providers,
        initial_model,
        &local_session_ids,
    );
    let (snapshot_refresh_tx, mut snapshot_refresh_rx) =
        mpsc::unbounded_channel::<SnapshotRefreshResult>();
    let (history_refresh_tx, mut history_refresh_rx) =
        mpsc::unbounded_channel::<CodexHistoryRefreshResult>();
    let (recent_refresh_tx, mut recent_refresh_rx) =
        mpsc::unbounded_channel::<CodexRecentRefreshResult>();
    let (fleet_refresh_tx, mut fleet_refresh_rx) = mpsc::unbounded_channel::<FleetRefreshResult>();
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
            palette,
            service_name,
            port,
            &providers,
        )?;

        if ui.should_exit || *shutdown_rx.borrow() {
            break;
        }

        tokio::select! {
            _ = ticker.tick() => {
                handle_ticker_refreshes(
                    &proxy,
                    timing.snapshot_fallback_interval,
                    &snapshot,
                    &mut snapshot_refresh,
                );
                if ui.page == types::Page::Fleet
                    && !ui.fleet_loading
                    && ui
                        .fleet_last_refresh_at
                        .is_none_or(|last| last.elapsed() >= Duration::from_secs(5))
                {
                    ui.needs_fleet_refresh = true;
                }
                if ui.needs_fleet_refresh && !ui.fleet_loading {
                    start_integrated_fleet_refresh(
                        &mut ui,
                        &cfg,
                        fleet_refresh_tx.clone(),
                    );
                }
                request_redraw(&mut render_invalidation);
            }
            changed = state_changes.changed() => {
                if changed.is_ok() {
                    snapshot_refresh.request(proxy.clone());
                }
            }
            maybe_snapshot_refresh = snapshot_refresh_rx.recv() => {
                if let Some(result) = maybe_snapshot_refresh {
                    snapshot_refresh.finish(result.generation);
                    if snapshot_refresh.result_is_current(&result) {
                        let model = match result.capture {
                            Ok(capture) => {
                                local_session_ids = capture.local_session_ids;
                                capture.model
                            }
                            Err(_error) => {
                                ui.operator_read_model
                                    .as_ref()
                                    .map(crate::dashboard_core::OperatorReadModel::stale_from)
                                    .unwrap_or_else(|| {
                                        crate::dashboard_core::OperatorReadModel::disconnected(
                                            service_name,
                                        )
                                    })
                            }
                        };
                        apply_operator_read_model(
                            &mut ui,
                            &mut snapshot,
                            &mut providers,
                            model,
                            &local_session_ids,
                        );
                        request_redraw(&mut render_invalidation);
                    }
                    snapshot_refresh.request_pending_if_idle(proxy.clone());
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
            maybe_fleet_refresh = fleet_refresh_rx.recv() => {
                if let Some(result) = maybe_fleet_refresh
                    && apply_fleet_refresh_result(&mut ui, result)
                {
                    request_redraw(&mut render_invalidation);
                }
            }
            changed = shutdown_rx.changed() => {
                let _ = changed;
                ui.should_exit = true;
                break;
            }
            _ = &mut ctrl_c => {
                ui.should_exit = true;
                break;
            }
            maybe_event = events.next() => {
                let Some(Ok(event)) = maybe_event else { continue; };
                match event {
                    Event::Key(key) if input::should_accept_key_event(&key) => {
                        let before_surface = RenderSurfaceKey::capture(&ui);
                        if input::handle_key_event(
                            input::KeyEventContext {
                                providers: &mut providers,
                                ui: &mut ui,
                                snapshot: &snapshot,
                            },
                            key,
                        )
                        .await
                        {
                            apply_pending_refresh_requests(
                                &mut ui,
                                &proxy,
                                &mut snapshot_refresh,
                                history_refresh_tx.clone(),
                                recent_refresh_tx.clone(),
                            ).await;
                            if ui.needs_fleet_refresh && !ui.fleet_loading {
                                start_integrated_fleet_refresh(
                                    &mut ui,
                                    &cfg,
                                    fleet_refresh_tx.clone(),
                                );
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

    leave_dashboard_terminal(term_guard, &mut terminal)
}

use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use futures_util::StreamExt;
use tokio::sync::mpsc;

use crate::config::storage::load_config;
use crate::control_plane_client::{ControlPlaneClient, ControlPlaneEndpoint};
use crate::dashboard_core::ApiV1Snapshot;
use crate::proxy::RuntimeStatusResponse;

use super::fleet_refresh::{
    FleetRefreshResult, FleetRefreshSource, apply_fleet_refresh_result, start_fleet_refresh,
};
use super::i18n;
use super::model::{
    Palette, ProviderOption, Snapshot, build_provider_options, filtered_request_page_len,
    filtered_requests_len, snapshot_from_api_v1,
};
use super::runtime_refresh::DashboardTiming;
use super::state::{FleetViewMode, RuntimeConnectionKind, UiState, adjust_table_selection};
use super::types::{Focus, Overlay, Page, StatsFocus};
use super::{RenderInvalidation, enter_dashboard_terminal, input, leave_dashboard_terminal};

struct AttachedDashboardRuntime {
    client: ControlPlaneClient,
}

impl AttachedDashboardRuntime {
    fn new(_service_name: &'static str, _port: u16, admin_port: u16) -> anyhow::Result<Self> {
        Self::new_with_admin_base_url(format!("http://127.0.0.1:{admin_port}"), None)
    }

    fn new_with_admin_base_url(
        admin_base_url: impl Into<String>,
        admin_token_env: Option<String>,
    ) -> anyhow::Result<Self> {
        let endpoint = ControlPlaneEndpoint::new(admin_base_url, admin_token_env)?;
        let client = ControlPlaneClient::new(endpoint)?;
        Ok(Self { client })
    }

    fn admin_base_url(&self) -> &str {
        &self.client.endpoint().admin_base_url
    }

    async fn fetch_json<T>(&self, path: &str) -> anyhow::Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        self.client.fetch_json(path).await
    }

    async fn runtime_status(&self) -> anyhow::Result<RuntimeStatusResponse> {
        self.fetch_json("/__codex_helper/api/v1/runtime/status")
            .await
    }

    async fn snapshot(&self, stats_days: usize) -> anyhow::Result<ApiV1Snapshot> {
        self.client
            .snapshot(
                crate::state::recent_finished_max().min(2_000),
                stats_days.min(365),
            )
            .await
    }
}

pub async fn run_attached_dashboard(
    service_name: &'static str,
    port: u16,
    admin_port: u16,
) -> anyhow::Result<()> {
    let runtime = AttachedDashboardRuntime::new(service_name, port, admin_port)?;
    run_attached_dashboard_runtime(service_name, port, runtime).await
}

pub async fn run_attached_dashboard_with_admin_base_url(
    service_name: &'static str,
    port: u16,
    admin_base_url: String,
    admin_token_env: Option<String>,
) -> anyhow::Result<()> {
    let runtime =
        AttachedDashboardRuntime::new_with_admin_base_url(admin_base_url, admin_token_env)?;
    run_attached_dashboard_runtime(service_name, port, runtime).await
}

async fn run_attached_dashboard_runtime(
    service_name: &'static str,
    port: u16,
    runtime: AttachedDashboardRuntime,
) -> anyhow::Result<()> {
    let cfg = load_config().await.unwrap_or_default();
    let language = resolve_attached_language(&cfg);
    let timing = DashboardTiming::from_env();

    let status = runtime.runtime_status().await?;
    let api_snapshot = runtime.snapshot(7).await?;
    if api_snapshot.service_name.as_str() != service_name {
        anyhow::bail!(
            "attached proxy on port {port} is service '{}', expected '{service_name}'",
            api_snapshot.service_name
        );
    }

    let mut providers = build_provider_options(&cfg, service_name);
    let mut snapshot = snapshot_from_api_v1(api_snapshot).await;
    let mut ui = UiState {
        service_name,
        proxy_port: port,
        language,
        usage_forecast: cfg.ui.usage_forecast.clone(),
        fleet_registry: cfg.fleet.clone(),
        refresh_ms: timing.refresh_ms,
        config_version: cfg.version,
        runtime_connection: RuntimeConnectionKind::Attached,
        runtime_shutdown_available: Some(status.shutdown_available),
        last_runtime_config_loaded_at_ms: Some(status.loaded_at_ms),
        last_runtime_config_source_mtime_ms: status.source_mtime_ms,
        last_runtime_retry: Some(status.retry),
        last_runtime_config_refresh_at: Some(Instant::now()),
        toast: Some((
            attached_start_toast(language, runtime.admin_base_url()),
            Instant::now(),
        )),
        ..Default::default()
    };
    hydrate_attached_profile_state(&mut ui, &runtime).await;
    hydrate_attached_routing_state(&mut ui, &runtime).await;
    ui.clamp_selection(&snapshot, ui.station_page_rows_len(providers.len()));

    let (term_guard, mut terminal) = enter_dashboard_terminal()?;
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(timing.refresh_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());
    let (fleet_refresh_tx, mut fleet_refresh_rx) = mpsc::unbounded_channel::<FleetRefreshResult>();
    let palette = Palette::default();
    let mut render_invalidation = RenderInvalidation::FullClear;

    loop {
        render_attached_if_needed(
            &mut terminal,
            &mut render_invalidation,
            &mut ui,
            &snapshot,
            palette,
            service_name,
            port,
            &providers,
        )?;

        if ui.should_exit {
            break;
        }

        tokio::select! {
            _ = ticker.tick() => {
                refresh_attached_snapshot(&runtime, &mut ui, &mut snapshot, providers.len()).await;
                if ui.page == Page::Fleet
                    && !ui.fleet_loading
                    && ui
                        .fleet_last_refresh_at
                        .is_none_or(|last| last.elapsed() >= Duration::from_secs(5))
                {
                    ui.needs_fleet_refresh = true;
                }
                if ui.needs_fleet_refresh && !ui.fleet_loading {
                    start_attached_fleet_refresh(&mut ui, &runtime, fleet_refresh_tx.clone());
                }
                render_invalidation = RenderInvalidation::Redraw;
            }
            maybe_fleet_refresh = fleet_refresh_rx.recv() => {
                if let Some(result) = maybe_fleet_refresh
                    && apply_fleet_refresh_result(&mut ui, result)
                {
                    render_invalidation = RenderInvalidation::Redraw;
                }
            }
            _ = &mut ctrl_c => {
                ui.should_exit = true;
                render_invalidation = RenderInvalidation::Redraw;
            }
            maybe_event = events.next() => {
                let Some(Ok(event)) = maybe_event else { continue; };
                match event {
                    Event::Key(key)
                        if input::should_accept_key_event(&key)
                            && handle_attached_key(&mut ui, &snapshot, &mut providers, key) =>
                    {
                        if ui.needs_fleet_refresh && !ui.fleet_loading {
                            start_attached_fleet_refresh(&mut ui, &runtime, fleet_refresh_tx.clone());
                        }
                        render_invalidation = RenderInvalidation::FullClear;
                    }
                    Event::Resize(_, _) => {
                        ui.reset_table_viewports();
                        render_invalidation = RenderInvalidation::FullClear;
                    }
                    _ => {}
                }
            }
        }
    }

    leave_dashboard_terminal(term_guard, &mut terminal)
}

fn start_attached_fleet_refresh(
    ui: &mut UiState,
    runtime: &AttachedDashboardRuntime,
    tx: mpsc::UnboundedSender<FleetRefreshResult>,
) {
    start_fleet_refresh(
        ui,
        FleetRefreshSource::Attached {
            client: runtime.client.clone(),
        },
        tx,
    );
    ui.needs_fleet_refresh = false;
}

fn resolve_attached_language(cfg: &crate::config::ProxyConfig) -> super::Language {
    if let Ok(s) = std::env::var("CODEX_HELPER_TUI_LANG") {
        super::resolve_language_preference(Some(&s))
    } else if let Some(s) = cfg.ui.language.as_deref() {
        super::resolve_language_preference(Some(s))
    } else {
        super::detect_system_language()
    }
}

fn attached_start_toast(language: super::Language, admin_base_url: &str) -> String {
    match language {
        super::Language::Zh => {
            format!("已进入附着观察模式：{admin_base_url}；q 只退出控制台，不停止目标 proxy")
        }
        super::Language::En => {
            format!(
                "attached observer mode: {admin_base_url}; q exits only this console and keeps the target proxy running"
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_attached_if_needed(
    terminal: &mut super::DashboardTerminal,
    render_invalidation: &mut RenderInvalidation,
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
    if matches!(render_invalidation, RenderInvalidation::FullClear) {
        terminal.clear()?;
    }
    terminal.draw(|f| {
        super::view::render_app(f, palette, ui, snapshot, service_name, port, providers)
    })?;
    *render_invalidation = RenderInvalidation::None;
    Ok(())
}

async fn refresh_attached_snapshot(
    runtime: &AttachedDashboardRuntime,
    ui: &mut UiState,
    snapshot: &mut Snapshot,
    providers_len: usize,
) {
    match runtime.runtime_status().await {
        Ok(status) => {
            ui.runtime_status_error = None;
            ui.runtime_shutdown_available = Some(status.shutdown_available);
            ui.last_runtime_config_loaded_at_ms = Some(status.loaded_at_ms);
            ui.last_runtime_config_source_mtime_ms = status.source_mtime_ms;
            ui.last_runtime_retry = Some(status.retry);
            ui.last_runtime_config_refresh_at = Some(Instant::now());
        }
        Err(err) => {
            ui.runtime_status_error = Some(err.to_string());
            ui.last_runtime_config_refresh_at = Some(Instant::now());
        }
    }

    match runtime.snapshot(ui.stats_days).await {
        Ok(api_snapshot) => {
            *snapshot = snapshot_from_api_v1(api_snapshot).await;
            ui.clamp_selection(snapshot, ui.station_page_rows_len(providers_len));
        }
        Err(err) => {
            ui.runtime_status_error = Some(err.to_string());
        }
    }
}

async fn hydrate_attached_profile_state(ui: &mut UiState, runtime: &AttachedDashboardRuntime) {
    let Ok(response) = runtime
        .fetch_json::<crate::proxy::ProfilesResponse>("/__codex_helper/api/v1/profiles")
        .await
    else {
        return;
    };

    ui.configured_default_profile = response.configured_default_profile.clone();
    ui.effective_default_profile = response.default_profile.clone();
    ui.runtime_default_profile_override =
        if response.default_profile != response.configured_default_profile {
            response.default_profile.clone()
        } else {
            None
        };
    ui.profile_options = response.profiles;
}

async fn hydrate_attached_routing_state(ui: &mut UiState, runtime: &AttachedDashboardRuntime) {
    if !ui.uses_route_graph_routing() {
        return;
    }
    let Ok(spec) = runtime
        .fetch_json::<crate::config::PersistedRoutingSpec>("/__codex_helper/api/v1/routing")
        .await
    else {
        return;
    };
    ui.routing_spec = Some(super::model::RoutingSpecView::from(spec));
    ui.clamp_routing_menu_selection();
    ui.last_routing_control_refresh_at = Some(Instant::now());
}

fn handle_attached_key(
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers: &mut [ProviderOption],
    key: KeyEvent,
) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
        ui.should_exit = true;
        return true;
    }

    if ui.overlay == Overlay::Help {
        return match key.code {
            KeyCode::Esc | KeyCode::Char('?') => {
                ui.overlay = Overlay::None;
                true
            }
            KeyCode::Char('L') => {
                toggle_attached_language(ui);
                true
            }
            _ => false,
        };
    }

    match key.code {
        KeyCode::Char('q') => {
            ui.should_exit = true;
            true
        }
        KeyCode::Char('?') => {
            ui.overlay = Overlay::Help;
            true
        }
        KeyCode::Esc => {
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Char('L') => {
            toggle_attached_language(ui);
            true
        }
        KeyCode::Char('1') => switch_attached_page(ui, Page::Dashboard),
        KeyCode::Char('2') => switch_attached_page(ui, Page::Stations),
        KeyCode::Char('3') => switch_attached_page(ui, Page::Sessions),
        KeyCode::Char('4') => switch_attached_page(ui, Page::Requests),
        KeyCode::Char('5') => switch_attached_page(ui, Page::Stats),
        KeyCode::Char('6') => switch_attached_page(ui, Page::Settings),
        KeyCode::Char('7') => switch_attached_page(ui, Page::History),
        KeyCode::Char('8') => switch_attached_page(ui, Page::Recent),
        KeyCode::Char('9') => switch_attached_page(ui, Page::Fleet),
        KeyCode::Tab => {
            cycle_attached_focus(ui);
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            move_attached_selection(ui, snapshot, providers.len(), -1)
        }
        KeyCode::Down | KeyCode::Char('j') => {
            move_attached_selection(ui, snapshot, providers.len(), 1)
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
            ui.stats_days = options[(idx + 1) % options.len()];
            ui.stats_provider_detail_scroll = 0;
            true
        }
        KeyCode::Char('r') if ui.page == Page::Fleet => {
            ui.needs_fleet_refresh = true;
            true
        }
        KeyCode::Char('t') if ui.page == Page::Fleet => {
            ui.fleet_view_mode = match ui.fleet_view_mode {
                FleetViewMode::Tree => FleetViewMode::Flat,
                FleetViewMode::Flat => FleetViewMode::Tree,
            };
            true
        }
        _ => false,
    }
}

fn switch_attached_page(ui: &mut UiState, page: Page) -> bool {
    ui.page = page;
    match ui.page {
        Page::Stations => ui.focus = Focus::Stations,
        Page::Requests => ui.focus = Focus::Requests,
        Page::Sessions | Page::History | Page::Recent => ui.focus = Focus::Sessions,
        Page::Fleet => {
            ui.focus = Focus::Stations;
            ui.needs_fleet_refresh = true;
            ui.sync_fleet_selection();
        }
        Page::Dashboard if ui.focus == Focus::Stations => ui.focus = Focus::Sessions,
        _ => {}
    }
    true
}

fn cycle_attached_focus(ui: &mut UiState) {
    match ui.page {
        Page::Dashboard => {
            ui.focus = match ui.focus {
                Focus::Sessions => Focus::Requests,
                Focus::Requests | Focus::Stations => Focus::Sessions,
            };
        }
        Page::Stations => ui.focus = Focus::Stations,
        Page::Stats => {
            ui.stats_focus = match ui.stats_focus {
                StatsFocus::Stations => StatsFocus::Providers,
                StatsFocus::Providers => StatsFocus::Stations,
            };
            ui.stats_provider_detail_scroll = 0;
        }
        Page::Fleet => {
            ui.focus = match ui.focus {
                Focus::Stations => Focus::Sessions,
                Focus::Sessions | Focus::Requests => Focus::Stations,
            };
        }
        _ => {}
    }
}

fn move_attached_selection(
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers_len: usize,
    delta: i32,
) -> bool {
    match ui.page {
        Page::Stations => {
            let len = ui.station_page_rows_len(providers_len);
            if let Some(next) = adjust_table_selection(&mut ui.stations_table, delta, len) {
                ui.selected_station_idx = next;
                return true;
            }
            false
        }
        Page::Stats => match ui.stats_focus {
            StatsFocus::Stations => {
                let len = snapshot.usage_rollup.by_config.len();
                if let Some(next) = adjust_table_selection(&mut ui.stats_stations_table, delta, len)
                {
                    ui.selected_stats_station_idx = next;
                    return true;
                }
                false
            }
            StatsFocus::Providers => {
                let len = ui.usage_balance_provider_rows_len(snapshot);
                if let Some(next) =
                    adjust_table_selection(&mut ui.stats_providers_table, delta, len)
                {
                    ui.selected_stats_provider_idx = next;
                    ui.stats_provider_detail_scroll = 0;
                    return true;
                }
                false
            }
        },
        Page::Sessions => {
            if let Some(next) =
                adjust_table_selection(&mut ui.sessions_page_table, delta, snapshot.rows.len())
            {
                ui.selected_sessions_page_idx = next;
                return true;
            }
            false
        }
        Page::Requests => {
            let filtered_len = filtered_request_page_len(
                snapshot,
                ui.focused_request_session_id.as_deref(),
                ui.selected_session_idx,
                ui.request_page_errors_only,
                ui.request_page_scope_session,
            );
            if let Some(next) =
                adjust_table_selection(&mut ui.request_page_table, delta, filtered_len)
            {
                ui.selected_request_page_idx = next;
                return true;
            }
            false
        }
        Page::Fleet => move_attached_fleet_selection(ui, delta),
        _ => match ui.focus {
            Focus::Sessions => {
                if let Some(next) =
                    adjust_table_selection(&mut ui.sessions_table, delta, snapshot.rows.len())
                {
                    ui.selected_session_idx = next;
                    ui.selected_session_id = snapshot
                        .rows
                        .get(next)
                        .and_then(|row| row.session_id.clone());
                    ui.selected_request_idx = 0;
                    ui.requests_table.select(
                        (filtered_requests_len(snapshot, ui.selected_session_idx) > 0).then_some(0),
                    );
                    return true;
                }
                false
            }
            Focus::Requests => {
                let filtered_len = filtered_requests_len(snapshot, ui.selected_session_idx);
                if let Some(next) =
                    adjust_table_selection(&mut ui.requests_table, delta, filtered_len)
                {
                    ui.selected_request_idx = next;
                    return true;
                }
                false
            }
            Focus::Stations => false,
        },
    }
}

fn move_attached_fleet_selection(ui: &mut UiState, delta: i32) -> bool {
    let Some(snapshot) = ui.fleet_snapshot.as_ref() else {
        return false;
    };

    if ui.focus == Focus::Stations {
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

fn toggle_attached_language(ui: &mut UiState) {
    let next = i18n::next_language(ui.language);
    ui.language = next;
    ui.toast = Some((
        match next {
            super::Language::Zh => "语言：中文（本次附着会话内生效）".to_string(),
            super::Language::En => "language: English (attached session only)".to_string(),
        },
        Instant::now(),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attached_page_switch_keeps_exit_semantics_read_only() {
        let mut ui = UiState {
            runtime_connection: RuntimeConnectionKind::Attached,
            ..Default::default()
        };

        assert!(handle_attached_key(
            &mut ui,
            &empty_snapshot(),
            &mut [],
            KeyEvent::from(KeyCode::Char('q')),
        ));

        assert!(ui.should_exit);
        assert!(ui.runtime_connection.is_attached());
    }

    #[test]
    fn attached_navigation_supports_core_pages() {
        let mut ui = UiState {
            runtime_connection: RuntimeConnectionKind::Attached,
            ..Default::default()
        };
        let snapshot = empty_snapshot();

        assert!(handle_attached_key(
            &mut ui,
            &snapshot,
            &mut [],
            KeyEvent::from(KeyCode::Char('4')),
        ));

        assert_eq!(ui.page, Page::Requests);
        assert_eq!(ui.focus, Focus::Requests);
    }

    #[test]
    fn attached_navigation_supports_fleet_page() {
        let mut ui = UiState {
            runtime_connection: RuntimeConnectionKind::Attached,
            ..Default::default()
        };
        let snapshot = empty_snapshot();

        assert!(handle_attached_key(
            &mut ui,
            &snapshot,
            &mut [],
            KeyEvent::from(KeyCode::Char('9')),
        ));

        assert_eq!(ui.page, Page::Fleet);
        assert_eq!(ui.focus, Focus::Stations);
        assert!(ui.needs_fleet_refresh);
    }

    #[test]
    fn attached_start_toast_names_observer_lifecycle() {
        let text = attached_start_toast(crate::tui::Language::En, "http://127.0.0.1:4211");

        assert!(text.contains("attached observer mode"), "{text}");
        assert!(text.contains("keeps the target proxy running"), "{text}");
    }

    fn empty_snapshot() -> Snapshot {
        Snapshot {
            rows: Vec::new(),
            recent: Vec::new(),
            forecast_recent: Vec::new(),
            forecast_recent_source: crate::tui::model::UsageForecastSampleSource::RuntimeOnly,
            model_overrides: std::collections::HashMap::new(),
            overrides: std::collections::HashMap::new(),
            station_overrides: std::collections::HashMap::new(),
            route_target_overrides: std::collections::HashMap::new(),
            service_tier_overrides: std::collections::HashMap::new(),
            global_station_override: None,
            global_route_target_override: None,
            station_meta_overrides: std::collections::HashMap::new(),
            usage_rollup: crate::state::UsageRollupView::default(),
            provider_balances: std::collections::HashMap::new(),
            station_health: std::collections::HashMap::new(),
            health_checks: std::collections::HashMap::new(),
            lb_view: std::collections::HashMap::new(),
            stats_5m: crate::dashboard_core::WindowStats::default(),
            stats_1h: crate::dashboard_core::WindowStats::default(),
            pricing_catalog: crate::pricing::ModelPriceCatalogSnapshot::default(),
            refreshed_at: Instant::now(),
        }
    }
}

use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use futures_util::StreamExt;
use tokio::sync::mpsc;

use crate::control_plane_client::{ControlPlaneClient, ControlPlaneEndpoint, LocalOperatorClient};
use crate::dashboard_core::OperatorReadModel;

use super::fleet_refresh::{
    FleetRefreshResult, FleetRefreshSource, apply_fleet_refresh_result, start_fleet_refresh,
};
use super::i18n;
use super::model::{
    Palette, ProviderOption, Snapshot, filtered_request_page_len, filtered_requests_len,
};
use super::operator_actions::{
    OperatorActionOutcome, apply_operator_action_outcome, queue_balance_refresh,
    start_attached_operator_action,
};
use super::operator_projection::apply_operator_read_model;
use super::runtime_refresh::DashboardTiming;
use super::state::{FleetViewMode, RuntimeConnectionKind, UiState, adjust_table_selection};
use super::types::{Focus, Overlay, Page};
use super::{RenderInvalidation, enter_dashboard_terminal, input, leave_dashboard_terminal};

struct AttachedDashboardRuntime {
    client: ControlPlaneClient,
    operator_client: Option<LocalOperatorClient>,
    operator_transport_error: Option<String>,
    connection_kind: RuntimeConnectionKind,
}

impl AttachedDashboardRuntime {
    fn new(_service_name: &'static str, _port: u16, admin_port: u16) -> anyhow::Result<Self> {
        Self::new_local_with_admin_base_url(format!("http://127.0.0.1:{admin_port}"), None)
    }

    fn new_with_admin_base_url(
        admin_base_url: impl Into<String>,
        admin_token_env: Option<String>,
    ) -> anyhow::Result<Self> {
        Self::new_remote_with_admin_base_url(admin_base_url, admin_token_env)
    }

    fn new_local_with_admin_base_url(
        admin_base_url: impl Into<String>,
        admin_token_env: Option<String>,
    ) -> anyhow::Result<Self> {
        Self::new_local_with_admin_base_url_and_token_loader(
            admin_base_url,
            admin_token_env,
            crate::local_operator::ensure_local_operator_token,
        )
    }

    fn new_local_with_admin_base_url_and_token_loader(
        admin_base_url: impl Into<String>,
        admin_token_env: Option<String>,
        load_operator_token: impl FnOnce() -> anyhow::Result<String>,
    ) -> anyhow::Result<Self> {
        let endpoint = ControlPlaneEndpoint::new(admin_base_url, admin_token_env)?;
        let client = ControlPlaneClient::new(endpoint.clone())?;
        let operator_client =
            load_operator_token().and_then(|token| LocalOperatorClient::new(endpoint, &token));
        let (operator_client, operator_transport_error) = match operator_client {
            Ok(client) => (Some(client), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Ok(Self {
            client,
            operator_client,
            operator_transport_error,
            connection_kind: RuntimeConnectionKind::LocalAttached,
        })
    }

    fn new_remote_with_admin_base_url(
        admin_base_url: impl Into<String>,
        admin_token_env: Option<String>,
    ) -> anyhow::Result<Self> {
        let endpoint = ControlPlaneEndpoint::new(admin_base_url, admin_token_env)?;
        let client = ControlPlaneClient::new(endpoint)?;
        Ok(Self {
            client,
            operator_client: None,
            operator_transport_error: None,
            connection_kind: RuntimeConnectionKind::RemoteObserver,
        })
    }

    fn admin_base_url(&self) -> &str {
        self.client.endpoint().admin_base_url()
    }

    fn operator_client(&self) -> Option<&LocalOperatorClient> {
        self.operator_client.as_ref()
    }

    async fn operator_read_model(
        &self,
        service_name: &str,
        previous: Option<&OperatorReadModel>,
    ) -> OperatorReadModel {
        self.client
            .refresh_operator_read_model(service_name, previous)
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

pub async fn run_local_attached_dashboard_with_admin_base_url(
    service_name: &'static str,
    port: u16,
    admin_base_url: String,
    admin_token_env: Option<String>,
) -> anyhow::Result<()> {
    let runtime =
        AttachedDashboardRuntime::new_local_with_admin_base_url(admin_base_url, admin_token_env)?;
    run_attached_dashboard_runtime(service_name, port, runtime).await
}

async fn run_attached_dashboard_runtime(
    service_name: &'static str,
    port: u16,
    runtime: AttachedDashboardRuntime,
) -> anyhow::Result<()> {
    let language = resolve_attached_language();
    let timing = DashboardTiming::from_env();

    let mut providers = Vec::new();
    let mut start_toast =
        attached_start_toast(language, runtime.admin_base_url(), runtime.connection_kind);
    if let Some(error) = runtime.operator_transport_error.as_deref() {
        start_toast.push_str(match language {
            super::Language::Zh => "；本机 operator 操作不可用，已降级只读：",
            super::Language::En => "; local operator actions unavailable; read-only fallback: ",
        });
        start_toast.push_str(error);
    }
    let mut ui = UiState {
        service_name,
        proxy_port: port,
        language,
        runtime_connection: runtime.connection_kind,
        local_operator_transport_available: runtime.operator_client.is_some(),
        toast: Some((start_toast, Instant::now())),
        ..Default::default()
    };
    let mut snapshot = Snapshot::default();
    refresh_attached_snapshot(&runtime, &mut ui, &mut snapshot, &mut providers).await;

    let (term_guard, mut terminal) = enter_dashboard_terminal()?;
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(timing.refresh_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());
    let (fleet_refresh_tx, mut fleet_refresh_rx) = mpsc::unbounded_channel::<FleetRefreshResult>();
    let (operator_action_tx, mut operator_action_rx) =
        mpsc::unbounded_channel::<OperatorActionOutcome>();
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
                refresh_attached_snapshot(&runtime, &mut ui, &mut snapshot, &mut providers).await;
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
            maybe_operator_action = operator_action_rx.recv() => {
                if let Some(outcome) = maybe_operator_action {
                    apply_operator_action_outcome(&mut ui, outcome);
                    if let Some(client) = runtime.operator_client() {
                        start_attached_operator_action(
                            &mut ui,
                            client,
                            operator_action_tx.clone(),
                        );
                    }
                    refresh_attached_snapshot(&runtime, &mut ui, &mut snapshot, &mut providers).await;
                    ui.needs_snapshot_refresh = false;
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
                        if let Some(client) = runtime.operator_client() {
                            start_attached_operator_action(
                                &mut ui,
                                client,
                                operator_action_tx.clone(),
                            );
                        }
                        if ui.needs_fleet_refresh && !ui.fleet_loading {
                            start_attached_fleet_refresh(&mut ui, &runtime, fleet_refresh_tx.clone());
                        }
                        if ui.needs_snapshot_refresh {
                            refresh_attached_snapshot(&runtime, &mut ui, &mut snapshot, &mut providers).await;
                            ui.needs_snapshot_refresh = false;
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
    let model = ui
        .operator_read_model
        .clone()
        .unwrap_or_else(|| OperatorReadModel::disconnected(ui.service_name));
    start_fleet_refresh(
        ui,
        FleetRefreshSource::Attached {
            model: Box::new(model),
            admin_base_url: runtime.admin_base_url().to_string(),
        },
        tx,
    );
    ui.needs_fleet_refresh = false;
}

fn resolve_attached_language() -> super::Language {
    if let Ok(s) = std::env::var("CODEX_HELPER_TUI_LANG") {
        super::resolve_language_preference(Some(&s))
    } else {
        super::detect_system_language()
    }
}

fn attached_start_toast(
    language: super::Language,
    admin_base_url: &str,
    connection_kind: RuntimeConnectionKind,
) -> String {
    match (language, connection_kind) {
        (super::Language::Zh, RuntimeConnectionKind::LocalAttached) => {
            format!("已进入本机附着控制模式：{admin_base_url}；q 只退出控制台，不停止 proxy")
        }
        (super::Language::En, RuntimeConnectionKind::LocalAttached) => format!(
            "local attached control mode: {admin_base_url}; q exits only this console and keeps the proxy running"
        ),
        (super::Language::Zh, _) => {
            format!("已进入远程只读观察模式：{admin_base_url}；q 只退出控制台，不停止目标 proxy")
        }
        (super::Language::En, _) => format!(
            "remote read-only observer mode: {admin_base_url}; q exits only this console and keeps the target proxy running"
        ),
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
    providers: &mut Vec<ProviderOption>,
) {
    let model = runtime
        .operator_read_model(ui.service_name, ui.operator_read_model.as_ref())
        .await;
    apply_operator_read_model(
        ui,
        snapshot,
        providers,
        model,
        &std::collections::HashMap::new(),
    );
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

    if ui.overlay == Overlay::RoutingActions {
        return input::handle_routing_actions_key(ui, snapshot, key);
    }

    if ui.overlay == Overlay::RoutingConfirmation {
        return input::handle_routing_confirmation_key(ui, key);
    }

    if ui.overlay == Overlay::SessionAffinityActions {
        return input::handle_session_affinity_actions_key(ui, snapshot, key);
    }

    if ui.overlay == Overlay::SessionAffinityConfirmation {
        return input::handle_session_affinity_confirmation_key(ui, key);
    }

    if ui.overlay == Overlay::ProviderInfo {
        if input::handle_provider_info_key(ui, key) {
            return true;
        }
        return match key.code {
            KeyCode::Char('L') => {
                toggle_attached_language(ui);
                true
            }
            _ => false,
        };
    }

    if input::handle_routing_operator_key(ui, snapshot, key.code) {
        return true;
    }
    if input::handle_session_affinity_operator_key(ui, snapshot, key.code) {
        return true;
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
        KeyCode::Char('i') if ui.page == Page::Routing => {
            ui.sync_selected_provider_from_routing(snapshot, providers);
            input::open_provider_info(ui);
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
        KeyCode::Char('2') => switch_attached_page(ui, Page::Routing),
        KeyCode::Char('3') => switch_attached_page(ui, Page::Sessions),
        KeyCode::Char('4') => switch_attached_page(ui, Page::Requests),
        KeyCode::Char('5') => switch_attached_page(ui, Page::Stats),
        KeyCode::Char('6') => switch_attached_page(ui, Page::ServiceStatus),
        KeyCode::Char('7') => switch_attached_page(ui, Page::Settings),
        KeyCode::Char('8') => switch_attached_page(ui, Page::History),
        KeyCode::Char('9') => switch_attached_page(ui, Page::Recent),
        KeyCode::Char('0') => switch_attached_page(ui, Page::Fleet),
        KeyCode::Tab => {
            cycle_attached_focus(ui);
            true
        }
        KeyCode::Char('p') if ui.page == Page::Routing => {
            if ui.select_preferred_routing_candidate(snapshot) {
                true
            } else {
                ui.toast = Some((
                    match ui.language {
                        super::Language::Zh => "当前没有可定位的新会话偏好".to_string(),
                        super::Language::En => {
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
            input::move_routing_page_selection(ui, snapshot, providers.len(), delta)
        }
        KeyCode::PageDown if ui.page == Page::Routing => {
            let delta = ui.routing_candidates_visible_rows.min(i32::MAX as usize) as i32;
            input::move_routing_page_selection(ui, snapshot, providers.len(), delta)
        }
        KeyCode::Home if ui.page == Page::Routing => {
            input::select_routing_page_edge(ui, snapshot, providers.len(), false)
        }
        KeyCode::End if ui.page == Page::Routing => {
            input::select_routing_page_edge(ui, snapshot, providers.len(), true)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            move_attached_selection(ui, snapshot, providers.len(), -1)
        }
        KeyCode::Down | KeyCode::Char('j') => {
            move_attached_selection(ui, snapshot, providers.len(), 1)
        }
        KeyCode::Char('g') if ui.page == Page::Stats => {
            ui.needs_snapshot_refresh = true;
            true
        }
        KeyCode::Char('y') if ui.page == Page::Stats => {
            input::export_selected_stats_report(ui, snapshot)
        }
        KeyCode::Char('r') if ui.page == Page::Fleet => {
            ui.needs_fleet_refresh = true;
            true
        }
        KeyCode::Char('r') if ui.page == Page::ServiceStatus => {
            ui.needs_snapshot_refresh = true;
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
    let previous_page = ui.page;
    ui.page = page;
    if previous_page == Page::Stats || ui.page == Page::Stats || ui.page == Page::ServiceStatus {
        ui.needs_snapshot_refresh = true;
    }
    match ui.page {
        Page::Routing => {
            ui.focus = Focus::Providers;
            if previous_page != Page::Routing {
                let _ = queue_balance_refresh(ui, false, false);
            }
        }
        Page::Requests => ui.focus = Focus::Requests,
        Page::Sessions | Page::History | Page::Recent => ui.focus = Focus::Sessions,
        Page::ServiceStatus => {}
        Page::Fleet => {
            ui.focus = Focus::Providers;
            ui.needs_fleet_refresh = true;
            ui.sync_fleet_selection();
        }
        Page::Dashboard if ui.focus == Focus::Providers => ui.focus = Focus::Sessions,
        _ => {}
    }
    true
}

fn cycle_attached_focus(ui: &mut UiState) {
    match ui.page {
        Page::Dashboard => {
            ui.focus = match ui.focus {
                Focus::Sessions => Focus::Requests,
                Focus::Requests | Focus::Providers => Focus::Sessions,
            };
        }
        Page::Routing => ui.focus = Focus::Providers,
        Page::Stats => {
            ui.cycle_stats_focus();
        }
        Page::Fleet => {
            ui.focus = match ui.focus {
                Focus::Providers => Focus::Sessions,
                Focus::Sessions | Focus::Requests => Focus::Providers,
            };
        }
        Page::ServiceStatus => {}
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
        Page::Routing => {
            if snapshot.routing.is_some() {
                return ui.move_routing_selection(snapshot, delta);
            }
            if let Some(next) =
                adjust_table_selection(&mut ui.providers_table, delta, providers_len)
            {
                ui.selected_provider_idx = next;
                return true;
            }
            false
        }
        Page::Stats => ui.move_stats_selection(snapshot, delta),
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
                ui.request_page_control_filter,
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
        Page::ServiceStatus => false,
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
            Focus::Providers => false,
        },
    }
}

fn move_attached_fleet_selection(ui: &mut UiState, delta: i32) -> bool {
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
    use axum::{Json, Router, extract::State, http::Uri, routing::get};
    use std::sync::{Arc, Mutex};

    use crate::dashboard_core::{
        ApiV1OperatorSummary, OperatorReadData, OperatorReadModel, OperatorReadStatus,
        OperatorRevisionBundle, OperatorRuntimeSummary, OperatorSummaryCounts,
    };

    #[derive(Clone)]
    struct RequestLog {
        paths: Arc<Mutex<Vec<String>>>,
        model: OperatorReadModel,
    }

    async fn operator_read_model_response(
        State(state): State<RequestLog>,
        uri: Uri,
    ) -> Json<OperatorReadModel> {
        state
            .paths
            .lock()
            .expect("request log lock")
            .push(uri.path().to_string());
        Json(state.model)
    }

    async fn unexpected_attached_request(State(state): State<RequestLog>, uri: Uri) {
        state
            .paths
            .lock()
            .expect("request log lock")
            .push(uri.path().to_string());
    }

    async fn spawn_operator_server(
        model: OperatorReadModel,
    ) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
        let paths = Arc::new(Mutex::new(Vec::new()));
        let state = RequestLog {
            paths: paths.clone(),
            model,
        };
        let app = Router::new()
            .route(
                "/__codex_helper/api/v1/operator/read-model",
                get(operator_read_model_response),
            )
            .fallback(get(unexpected_attached_request))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind operator server");
        let address = listener.local_addr().expect("operator server address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve operator model");
        });
        (format!("http://{address}"), paths, server)
    }

    fn ready_operator_model() -> OperatorReadModel {
        let mut model = OperatorReadModel::ready(
            "codex",
            42,
            OperatorRevisionBundle {
                runtime_revision: 7,
                runtime_digest: "runtime-7".to_string(),
                route_digest: "route-7".to_string(),
                catalog_revision: "catalog-7".to_string(),
                pricing_revision: "pricing-7".to_string(),
                operator_pricing_revision: "operator-pricing-7".to_string(),
                policy_revision: 8,
                ledger_revision: "ledger-9".to_string(),
            },
            OperatorReadData {
                summary: ApiV1OperatorSummary {
                    api_version: 1,
                    service_name: "codex".to_string(),
                    runtime: OperatorRuntimeSummary::default(),
                    counts: OperatorSummaryCounts::default(),
                    retry: Default::default(),
                    credential_readiness: None,
                    sessions: Vec::new(),
                    profiles: Vec::new(),
                    providers: Vec::new(),
                },
                routing: None,
                active_requests: Vec::new(),
                recent_requests: Vec::new(),
                usage_summaries: Vec::new(),
                usage_day: Default::default(),
                quota_analytics: Default::default(),
                usage_rollup: Default::default(),
                stats_5m: Default::default(),
                stats_1h: Default::default(),
                pricing_catalog: Default::default(),
                service_status: None,
                provider_balances: Vec::new(),
            },
        );
        let data = model.data.as_mut().expect("ready operator data");
        data.summary.runtime.runtime_loaded_at_ms = Some(77);
        data.stats_5m.total = 3;
        model
    }

    #[tokio::test]
    async fn attached_refresh_reads_exactly_one_operator_bundle() {
        let (base_url, paths, server) = spawn_operator_server(ready_operator_model()).await;
        let runtime = AttachedDashboardRuntime::new_with_admin_base_url(base_url, None)
            .expect("attached runtime");
        let mut ui = UiState {
            service_name: "codex",
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            ..Default::default()
        };
        let mut snapshot = empty_snapshot();
        let mut providers = Vec::new();

        refresh_attached_snapshot(&runtime, &mut ui, &mut snapshot, &mut providers).await;

        assert_eq!(
            *paths.lock().expect("request log lock"),
            vec!["/__codex_helper/api/v1/operator/read-model"],
        );
        assert_eq!(
            ui.operator_read_model.as_ref().map(|model| model.status),
            Some(OperatorReadStatus::Ready)
        );
        assert_eq!(snapshot.stats_5m.total, 3);
        server.abort();
    }

    #[test]
    fn stale_attached_bundle_keeps_facts_but_disables_runtime_actions() {
        let ready = ready_operator_model();
        let stale = OperatorReadModel::stale_from(&ready);
        let mut ui = UiState {
            service_name: "codex",
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            ..Default::default()
        };
        let mut snapshot = Snapshot::default();
        let mut providers = Vec::new();

        apply_operator_read_model(
            &mut ui,
            &mut snapshot,
            &mut providers,
            stale,
            &std::collections::HashMap::new(),
        );

        let model = ui.operator_read_model.as_ref().expect("stale model");
        assert_eq!(model.status, OperatorReadStatus::Stale);
        assert!(!model.can_use_runtime_actions());
        assert_eq!(snapshot.stats_5m.total, 3);
        assert_eq!(ui.last_runtime_config_loaded_at_ms, Some(77));
        assert!(
            ui.runtime_status_error
                .as_deref()
                .is_some_and(|error| error.contains("stale"))
        );
    }

    #[test]
    fn unavailable_attached_states_drop_previous_runtime_facts() {
        for unavailable in [
            OperatorReadModel::auth_required("codex"),
            OperatorReadModel::disconnected("codex"),
        ] {
            let mut ui = UiState {
                service_name: "codex",
                runtime_connection: RuntimeConnectionKind::RemoteObserver,
                ..Default::default()
            };
            let mut snapshot = Snapshot::default();
            let mut providers = Vec::new();
            apply_operator_read_model(
                &mut ui,
                &mut snapshot,
                &mut providers,
                ready_operator_model(),
                &std::collections::HashMap::new(),
            );

            apply_operator_read_model(
                &mut ui,
                &mut snapshot,
                &mut providers,
                unavailable,
                &std::collections::HashMap::new(),
            );

            let model = ui.operator_read_model.as_ref().expect("unavailable model");
            assert!(!model.can_use_runtime_actions());
            assert!(model.data.is_none());
            assert_eq!(snapshot.stats_5m.total, 0);
            assert!(providers.is_empty());
            assert_eq!(ui.last_runtime_config_loaded_at_ms, None);
        }
    }

    #[test]
    fn attached_page_switch_keeps_exit_semantics_read_only() {
        let mut ui = UiState {
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
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
    fn attached_settings_do_not_handle_local_codex_switch_keys() {
        let mut ui = UiState {
            service_name: "codex",
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            page: Page::Settings,
            ..Default::default()
        };
        let snapshot = empty_snapshot();

        for code in ['n', 'o'] {
            assert!(
                !handle_attached_key(
                    &mut ui,
                    &snapshot,
                    &mut [],
                    KeyEvent::from(KeyCode::Char(code)),
                ),
                "{code:?} must remain unhandled in attached Settings"
            );
            assert!(ui.toast.is_none(), "{code:?} must not trigger a switch");
        }
        assert!(!ui.allows_local_codex_switch());
    }

    #[test]
    fn attached_navigation_supports_core_pages() {
        let mut ui = UiState {
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            ..Default::default()
        };
        let snapshot = empty_snapshot();

        assert!(handle_attached_key(
            &mut ui,
            &snapshot,
            &mut [],
            KeyEvent::from(KeyCode::Char('2')),
        ));
        assert_eq!(ui.page, Page::Routing);
        assert_eq!(ui.focus, Focus::Providers);

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
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            ..Default::default()
        };
        let snapshot = empty_snapshot();

        assert!(handle_attached_key(
            &mut ui,
            &snapshot,
            &mut [],
            KeyEvent::from(KeyCode::Char('0')),
        ));

        assert_eq!(ui.page, Page::Fleet);
        assert_eq!(ui.focus, Focus::Providers);
        assert!(ui.needs_fleet_refresh);
    }

    #[test]
    fn attached_start_toast_names_remote_observer_lifecycle() {
        let text = attached_start_toast(
            crate::tui::Language::En,
            "http://127.0.0.1:4211",
            RuntimeConnectionKind::RemoteObserver,
        );

        assert!(text.contains("remote read-only observer mode"), "{text}");
        assert!(text.contains("keeps the target proxy running"), "{text}");
    }

    #[test]
    fn attached_start_toast_names_local_control_lifecycle() {
        let text = attached_start_toast(
            crate::tui::Language::En,
            "http://127.0.0.1:4211",
            RuntimeConnectionKind::LocalAttached,
        );

        assert!(text.contains("local attached control mode"), "{text}");
        assert!(text.contains("keeps the proxy running"), "{text}");
    }

    #[test]
    fn local_attached_runtime_falls_back_to_read_only_when_operator_token_fails() {
        let runtime = AttachedDashboardRuntime::new_local_with_admin_base_url_and_token_loader(
            "http://127.0.0.1:4211",
            None,
            || anyhow::bail!("test token ACL failure"),
        )
        .expect("read-only local runtime");

        assert_eq!(
            runtime.connection_kind,
            RuntimeConnectionKind::LocalAttached
        );
        assert!(runtime.operator_client.is_none());
        assert!(
            runtime
                .operator_transport_error
                .as_deref()
                .is_some_and(|error| error.contains("token ACL failure"))
        );
    }

    #[test]
    fn attached_stats_refresh_reloads_operator_read_model() {
        let mut ui = UiState {
            page: Page::Stats,
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            ..Default::default()
        };

        assert!(handle_attached_key(
            &mut ui,
            &empty_snapshot(),
            &mut [],
            KeyEvent::from(KeyCode::Char('g')),
        ));

        assert!(ui.needs_snapshot_refresh);
    }

    #[test]
    fn attached_stats_tab_uses_shared_four_focus_cycle() {
        let mut ui = UiState {
            page: Page::Stats,
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            ..Default::default()
        };

        for expected in [
            crate::tui::types::StatsFocus::Projects,
            crate::tui::types::StatsFocus::Providers,
            crate::tui::types::StatsFocus::ProviderEndpoints,
            crate::tui::types::StatsFocus::Pools,
        ] {
            assert!(handle_attached_key(
                &mut ui,
                &empty_snapshot(),
                &mut [],
                KeyEvent::from(KeyCode::Tab),
            ));
            assert_eq!(ui.stats_focus, expected);
        }
    }

    #[test]
    fn attached_routing_provider_info_uses_read_only_overlay_controls() {
        let mut ui = UiState {
            page: Page::Routing,
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            provider_info_scroll: 9,
            ..Default::default()
        };
        let snapshot = empty_snapshot();

        assert!(handle_attached_key(
            &mut ui,
            &snapshot,
            &mut [],
            KeyEvent::from(KeyCode::Char('i')),
        ));
        assert_eq!(ui.overlay, Overlay::ProviderInfo);
        assert_eq!(ui.provider_info_scroll, 0);

        assert!(handle_attached_key(
            &mut ui,
            &snapshot,
            &mut [],
            KeyEvent::from(KeyCode::PageDown),
        ));
        assert_eq!(ui.provider_info_scroll, 10);

        assert!(handle_attached_key(
            &mut ui,
            &snapshot,
            &mut [],
            KeyEvent::from(KeyCode::Char('i')),
        ));
        assert_eq!(ui.overlay, Overlay::None);
    }

    #[test]
    fn attached_stats_report_export_key_handles_empty_selection() {
        let mut ui = UiState {
            page: Page::Stats,
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            ..Default::default()
        };

        assert!(handle_attached_key(
            &mut ui,
            &empty_snapshot(),
            &mut [],
            KeyEvent::from(KeyCode::Char('y')),
        ));
        let toast = ui.toast.as_ref().expect("report toast").0.as_str();
        assert!(toast.contains("no selection"), "{toast}");
    }

    fn empty_snapshot() -> Snapshot {
        Snapshot {
            rows: Vec::new(),
            recent: Vec::new(),
            request_control_evidence: std::collections::HashMap::new(),
            usage_day: crate::state::UsageDayView::default(),
            quota_analytics: crate::quota_analytics::QuotaAnalyticsView::default(),
            usage_rollup: crate::state::UsageRollupView::default(),
            provider_balances: std::collections::HashMap::new(),
            routing: None,
            pricing_catalog: Default::default(),
            stats_5m: crate::dashboard_core::WindowStats::default(),
            stats_1h: crate::dashboard_core::WindowStats::default(),
            service_status: None,
            refreshed_at: Instant::now(),
        }
    }
}

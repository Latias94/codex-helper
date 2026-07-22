use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream};
use futures_util::StreamExt;
use tokio::sync::mpsc;

use crate::control_plane_client::{
    ControlPlaneClient, ControlPlaneEndpoint, ControlPlaneError, LocalOperatorClient,
};
use crate::dashboard_core::{OperatorLocalSessionMetadata, OperatorReadModel};

use super::fleet_refresh::{
    FleetRefreshResult, FleetRefreshSource, apply_fleet_refresh_result,
    local_session_metadata_for_fleet, start_fleet_refresh,
};
use super::local_session_enrichment::{
    AttachedLocalSessionEnrichment, AttachedLocalSessionEnrichmentResult,
    LocalSessionEnrichmentIssue,
};
use super::model::{Palette, ProviderOption, Snapshot};
use super::operator_actions::{
    OperatorActionOutcome, apply_operator_action_outcome, start_attached_operator_action,
};
use super::operator_projection::apply_operator_read_model;
use super::runtime_refresh::{DashboardTiming, maybe_queue_stale_balance_refresh};
use super::session_refresh::{
    CodexHistoryRefreshResult, CodexRecentRefreshResult, apply_codex_history_refresh_result,
    apply_codex_recent_refresh_result, start_codex_history_refresh, start_codex_recent_refresh,
};
use super::state::{RuntimeConnectionKind, UiState};
use super::types::Page;
use super::{RenderInvalidation, enter_dashboard_terminal, input, leave_dashboard_terminal};

struct AttachedDashboardRuntime {
    client: ControlPlaneClient,
    operator_client: Option<LocalOperatorClient>,
    operator_transport_error: Option<String>,
    connection_kind: RuntimeConnectionKind,
    local_session_enrichment: tokio::sync::Mutex<AttachedLocalSessionEnrichment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachedOperatorRefreshIssue {
    AuthenticationRequired { status: u16 },
    HttpStatus { status: u16 },
    WrongService,
    InvalidJson,
    InvalidPayload,
    Transport,
}

#[derive(Debug)]
struct AttachedSnapshotRefreshPayload {
    model: OperatorReadModel,
    local_sessions: HashMap<String, OperatorLocalSessionMetadata>,
    operator_issue: Option<AttachedOperatorRefreshIssue>,
    local_session_issue: Option<LocalSessionEnrichmentIssue>,
}

#[derive(Debug)]
struct AttachedSnapshotRefreshResult {
    generation: u64,
    payload: AttachedSnapshotRefreshPayload,
}

#[derive(Debug)]
struct AttachedSnapshotRefreshController {
    tx: mpsc::UnboundedSender<AttachedSnapshotRefreshResult>,
    generation: u64,
    in_flight: Option<u64>,
    pending: bool,
}

impl AttachedSnapshotRefreshController {
    fn new(tx: mpsc::UnboundedSender<AttachedSnapshotRefreshResult>) -> Self {
        Self {
            tx,
            generation: 0,
            in_flight: None,
            pending: false,
        }
    }

    fn request(
        &mut self,
        runtime: Arc<AttachedDashboardRuntime>,
        service_name: &'static str,
        previous: Option<&OperatorReadModel>,
    ) {
        if self.in_flight.is_some() {
            self.pending = true;
            return;
        }

        self.generation = self.generation.wrapping_add(1);
        self.pending = false;
        self.spawn(runtime, service_name, previous, self.generation);
    }

    fn supersede(
        &mut self,
        runtime: Arc<AttachedDashboardRuntime>,
        service_name: &'static str,
        previous: Option<&OperatorReadModel>,
    ) {
        self.generation = self.generation.wrapping_add(1);
        if self.in_flight.is_some() {
            self.pending = true;
            return;
        }

        self.pending = false;
        self.spawn(runtime, service_name, previous, self.generation);
    }

    fn finish(&mut self, generation: u64) {
        if self.in_flight == Some(generation) {
            self.in_flight = None;
        }
    }

    fn result_is_current(&self, result: &AttachedSnapshotRefreshResult) -> bool {
        result.generation == self.generation
    }

    fn request_pending_if_idle(
        &mut self,
        runtime: Arc<AttachedDashboardRuntime>,
        service_name: &'static str,
        previous: Option<&OperatorReadModel>,
    ) {
        if !self.pending || self.in_flight.is_some() {
            return;
        }

        self.pending = false;
        if self.generation == 0 {
            self.generation = 1;
        }
        self.spawn(runtime, service_name, previous, self.generation);
    }

    fn spawn(
        &mut self,
        runtime: Arc<AttachedDashboardRuntime>,
        service_name: &'static str,
        previous: Option<&OperatorReadModel>,
        generation: u64,
    ) {
        self.in_flight = Some(generation);
        let tx = self.tx.clone();
        let previous = previous.cloned();
        tokio::spawn(async move {
            let payload =
                capture_attached_snapshot(&runtime, service_name, previous.as_ref()).await;
            let _ = tx.send(AttachedSnapshotRefreshResult {
                generation,
                payload,
            });
        });
    }
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
            local_session_enrichment: tokio::sync::Mutex::new(
                AttachedLocalSessionEnrichment::default(),
            ),
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
            local_session_enrichment: tokio::sync::Mutex::new(
                AttachedLocalSessionEnrichment::default(),
            ),
        })
    }

    fn admin_base_url(&self) -> &str {
        self.client.endpoint().admin_base_url()
    }

    fn operator_client(&self) -> Option<&LocalOperatorClient> {
        self.operator_client.as_ref()
    }

    async fn operator_read_model(&self) -> Result<OperatorReadModel, ControlPlaneError> {
        self.client.operator_read_model().await
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
    let runtime = Arc::new(runtime);
    let language = resolve_attached_language().await;
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

    let (term_guard, mut terminal) = enter_dashboard_terminal()?;
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(timing.refresh_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());
    let (fleet_refresh_tx, mut fleet_refresh_rx) = mpsc::unbounded_channel::<FleetRefreshResult>();
    let (operator_action_tx, mut operator_action_rx) =
        mpsc::unbounded_channel::<OperatorActionOutcome>();
    let (history_refresh_tx, mut history_refresh_rx) =
        mpsc::unbounded_channel::<CodexHistoryRefreshResult>();
    let (recent_refresh_tx, mut recent_refresh_rx) =
        mpsc::unbounded_channel::<CodexRecentRefreshResult>();
    let (snapshot_refresh_tx, mut snapshot_refresh_rx) =
        mpsc::unbounded_channel::<AttachedSnapshotRefreshResult>();
    let mut snapshot_refresh = AttachedSnapshotRefreshController::new(snapshot_refresh_tx);
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
                snapshot_refresh.request(
                    Arc::clone(&runtime),
                    service_name,
                    ui.operator_read_model.as_ref(),
                );
                if maybe_queue_stale_balance_refresh(&mut ui, &snapshot)
                    && let Some(client) = runtime.operator_client()
                {
                    start_attached_operator_action(
                        &mut ui,
                        client,
                        operator_action_tx.clone(),
                    );
                }
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
            maybe_snapshot_refresh = snapshot_refresh_rx.recv() => {
                if let Some(result) = maybe_snapshot_refresh {
                    snapshot_refresh.finish(result.generation);
                    if snapshot_refresh.result_is_current(&result) {
                        apply_attached_snapshot_refresh(
                            &mut ui,
                            &mut snapshot,
                            &mut providers,
                            result.payload,
                        );
                        render_invalidation = RenderInvalidation::Redraw;
                    }
                    snapshot_refresh.request_pending_if_idle(
                        Arc::clone(&runtime),
                        service_name,
                        ui.operator_read_model.as_ref(),
                    );
                }
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
                    snapshot_refresh.supersede(
                        Arc::clone(&runtime),
                        service_name,
                        ui.operator_read_model.as_ref(),
                    );
                    ui.needs_snapshot_refresh = false;
                    render_invalidation = RenderInvalidation::Redraw;
                }
            }
            maybe_history_refresh = history_refresh_rx.recv() => {
                if let Some(result) = maybe_history_refresh
                    && apply_codex_history_refresh_result(&mut ui, result)
                {
                    render_invalidation = RenderInvalidation::Redraw;
                }
            }
            maybe_recent_refresh = recent_refresh_rx.recv() => {
                if let Some(result) = maybe_recent_refresh
                    && apply_codex_recent_refresh_result(&mut ui, result)
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
                    Event::Key(key) if input::should_accept_key_event(&key) => {
                        let handled = input::handle_key_event(
                            input::KeyEventContext {
                                providers: &mut providers,
                                ui: &mut ui,
                                snapshot: &snapshot,
                            },
                            key,
                        )
                        .await;
                        if !handled {
                            continue;
                        }
                        if ui.needs_codex_history_refresh {
                            start_codex_history_refresh(&mut ui, history_refresh_tx.clone());
                            ui.needs_codex_history_refresh = false;
                        }
                        if ui.needs_codex_recent_refresh {
                            start_codex_recent_refresh(&mut ui, recent_refresh_tx.clone());
                            ui.needs_codex_recent_refresh = false;
                        }
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
                            snapshot_refresh.supersede(
                                Arc::clone(&runtime),
                                service_name,
                                ui.operator_read_model.as_ref(),
                            );
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
    let local_sessions = local_session_metadata_for_fleet(ui);
    start_fleet_refresh(
        ui,
        FleetRefreshSource::Attached {
            model: Box::new(model),
            local_sessions,
            admin_base_url: runtime.admin_base_url().to_string(),
            connection_kind: runtime.connection_kind,
        },
        tx,
    );
    ui.needs_fleet_refresh = false;
}

async fn resolve_attached_language() -> super::Language {
    let environment = std::env::var("CODEX_HELPER_TUI_LANG").ok();
    let configured = crate::config::load_config()
        .await
        .ok()
        .and_then(|config| config.ui.language);
    select_attached_language(
        environment.as_deref(),
        configured.as_deref(),
        super::detect_system_language(),
    )
}

fn select_attached_language(
    environment: Option<&str>,
    configured: Option<&str>,
    system: super::Language,
) -> super::Language {
    environment
        .or(configured)
        .map(|value| super::resolve_language_preference(Some(value)))
        .unwrap_or(system)
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

async fn capture_attached_snapshot(
    runtime: &AttachedDashboardRuntime,
    service_name: &str,
    previous: Option<&OperatorReadModel>,
) -> AttachedSnapshotRefreshPayload {
    let (model, operator_issue, may_refresh_local_sessions) = classify_attached_operator_result(
        runtime.operator_read_model().await,
        service_name,
        previous,
    );

    let local_session_result = if let Some(client) = runtime.operator_client() {
        let mut enrichment = runtime.local_session_enrichment.lock().await;
        if may_refresh_local_sessions {
            enrichment.resolve(client, &model).await
        } else {
            enrichment.current()
        }
    } else {
        AttachedLocalSessionEnrichmentResult::default()
    };

    AttachedSnapshotRefreshPayload {
        model,
        local_sessions: local_session_result.sessions,
        operator_issue,
        local_session_issue: local_session_result.issue,
    }
}

fn classify_attached_operator_result(
    result: Result<OperatorReadModel, ControlPlaneError>,
    service_name: &str,
    previous: Option<&OperatorReadModel>,
) -> (
    OperatorReadModel,
    Option<AttachedOperatorRefreshIssue>,
    bool,
) {
    match result {
        Ok(model) if model.service_name == service_name => (model, None, true),
        Ok(_) => (
            attached_last_known_or_disconnected(service_name, previous),
            Some(AttachedOperatorRefreshIssue::WrongService),
            false,
        ),
        Err(ControlPlaneError::HttpStatus { status, .. }) if status == 401 || status == 403 => {
            let model = if previous.is_some_and(|model| model.data.is_some()) {
                attached_last_known_or_disconnected(service_name, previous)
            } else {
                OperatorReadModel::auth_required(service_name)
            };
            (
                model,
                Some(AttachedOperatorRefreshIssue::AuthenticationRequired { status }),
                false,
            )
        }
        Err(ControlPlaneError::HttpStatus { status, .. }) => (
            attached_last_known_or_disconnected(service_name, previous),
            Some(AttachedOperatorRefreshIssue::HttpStatus { status }),
            false,
        ),
        Err(ControlPlaneError::Decode { .. }) => (
            attached_last_known_or_disconnected(service_name, previous),
            Some(AttachedOperatorRefreshIssue::InvalidJson),
            false,
        ),
        Err(
            ControlPlaneError::InvalidPayload { .. }
            | ControlPlaneError::UntrustedRequestPath { .. },
        ) => (
            attached_last_known_or_disconnected(service_name, previous),
            Some(AttachedOperatorRefreshIssue::InvalidPayload),
            false,
        ),
        Err(ControlPlaneError::Transport { .. }) => (
            attached_last_known_or_disconnected(service_name, previous),
            Some(AttachedOperatorRefreshIssue::Transport),
            false,
        ),
    }
}

fn attached_last_known_or_disconnected(
    service_name: &str,
    previous: Option<&OperatorReadModel>,
) -> OperatorReadModel {
    previous
        .filter(|model| model.service_name == service_name && model.data.is_some())
        .map(OperatorReadModel::stale_from)
        .unwrap_or_else(|| OperatorReadModel::disconnected(service_name))
}

fn apply_attached_snapshot_refresh(
    ui: &mut UiState,
    snapshot: &mut Snapshot,
    providers: &mut Vec<ProviderOption>,
    payload: AttachedSnapshotRefreshPayload,
) {
    let showing_last_known = payload.model.data.is_some();
    apply_operator_read_model(
        ui,
        snapshot,
        providers,
        payload.model,
        &payload.local_sessions,
    );

    let mut issues = Vec::new();
    if let Some(issue) = payload.operator_issue {
        issues.push(attached_operator_issue_message(
            issue,
            showing_last_known,
            ui.language,
        ));
    }
    if let Some(issue) = payload.local_session_issue {
        issues.push(attached_local_session_issue_message(issue, ui.language));
    }
    if !issues.is_empty() {
        ui.runtime_status_error = Some(issues.join("; "));
    }
}

fn attached_operator_issue_message(
    issue: AttachedOperatorRefreshIssue,
    showing_last_known: bool,
    language: super::Language,
) -> String {
    let suffix = match (language, showing_last_known) {
        (super::Language::Zh, true) => "；正在显示上次成功数据",
        (super::Language::En, true) => "; showing last known data",
        _ => "",
    };
    let message = match (language, issue) {
        (super::Language::Zh, AttachedOperatorRefreshIssue::AuthenticationRequired { status }) => {
            format!("operator read-model 认证失败（HTTP {status}）")
        }
        (super::Language::En, AttachedOperatorRefreshIssue::AuthenticationRequired { status }) => {
            format!("operator read-model authentication failed (HTTP {status})")
        }
        (super::Language::Zh, AttachedOperatorRefreshIssue::HttpStatus { status }) => {
            format!("operator read-model 刷新失败（HTTP {status}）")
        }
        (super::Language::En, AttachedOperatorRefreshIssue::HttpStatus { status }) => {
            format!("operator read-model refresh failed (HTTP {status})")
        }
        (super::Language::Zh, AttachedOperatorRefreshIssue::WrongService) => {
            "operator read-model 属于其他服务，已忽略该响应".to_string()
        }
        (super::Language::En, AttachedOperatorRefreshIssue::WrongService) => {
            "operator read-model belongs to another service; response ignored".to_string()
        }
        (super::Language::Zh, AttachedOperatorRefreshIssue::InvalidJson) => {
            "operator read-model 返回了无效 JSON，已忽略该响应".to_string()
        }
        (super::Language::En, AttachedOperatorRefreshIssue::InvalidJson) => {
            "operator read-model returned invalid JSON; response ignored".to_string()
        }
        (super::Language::Zh, AttachedOperatorRefreshIssue::InvalidPayload) => {
            "operator read-model 响应不符合契约，已忽略该响应".to_string()
        }
        (super::Language::En, AttachedOperatorRefreshIssue::InvalidPayload) => {
            "operator read-model response violates its contract; response ignored".to_string()
        }
        (super::Language::Zh, AttachedOperatorRefreshIssue::Transport) => {
            "operator read-model 暂时无法连接".to_string()
        }
        (super::Language::En, AttachedOperatorRefreshIssue::Transport) => {
            "operator read-model is temporarily unreachable".to_string()
        }
    };
    format!("{message}{suffix}")
}

fn attached_local_session_issue_message(
    issue: LocalSessionEnrichmentIssue,
    language: super::Language,
) -> String {
    match (language, issue) {
        (super::Language::Zh, LocalSessionEnrichmentIssue::MetadataUnavailable) => {
            "本机会话元数据暂时不可用；正在显示上次成功结果".to_string()
        }
        (super::Language::En, LocalSessionEnrichmentIssue::MetadataUnavailable) => {
            "local session metadata is temporarily unavailable; showing last successful result"
                .to_string()
        }
        (super::Language::Zh, LocalSessionEnrichmentIssue::ServiceMismatch) => {
            "本机会话元数据属于其他服务；已忽略该响应".to_string()
        }
        (super::Language::En, LocalSessionEnrichmentIssue::ServiceMismatch) => {
            "local session metadata belongs to another service; response ignored".to_string()
        }
    }
}

#[cfg(test)]
async fn handle_attached_key(
    ui: &mut UiState,
    snapshot: &Snapshot,
    _providers: &mut [ProviderOption],
    key: crossterm::event::KeyEvent,
) -> bool {
    let mut providers = Vec::new();
    input::handle_key_event(
        input::KeyEventContext {
            providers: &mut providers,
            ui,
            snapshot,
        },
        key,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, http::Uri, routing::get};
    use crossterm::event::{KeyCode, KeyEvent};
    use std::sync::{Arc, Mutex};

    use crate::dashboard_core::{
        ApiV1OperatorSummary, OperatorReadData, OperatorReadModel, OperatorReadStatus,
        OperatorRevisionBundle, OperatorRuntimeSummary, OperatorSummaryCounts,
    };
    use crate::tui::types::{Focus, Overlay};

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

        let payload = capture_attached_snapshot(&runtime, ui.service_name, None).await;
        apply_attached_snapshot_refresh(&mut ui, &mut snapshot, &mut providers, payload);

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
    fn attached_refresh_failure_keeps_last_known_facts_without_exposing_error_body() {
        let previous = ready_operator_model();
        let (model, issue, may_refresh_local_sessions) = classify_attached_operator_result(
            Err(ControlPlaneError::HttpStatus {
                status: 500,
                body_excerpt: "secret-upstream-response".to_string(),
            }),
            "codex",
            Some(&previous),
        );

        assert_eq!(model.status, OperatorReadStatus::Stale);
        assert_eq!(model.data.as_ref().map(|data| data.stats_5m.total), Some(3));
        assert_eq!(
            issue,
            Some(AttachedOperatorRefreshIssue::HttpStatus { status: 500 })
        );
        assert!(!may_refresh_local_sessions);

        let message = attached_operator_issue_message(
            issue.expect("classified issue"),
            true,
            crate::tui::Language::En,
        );
        assert!(message.contains("HTTP 500"), "{message}");
        assert!(message.contains("last known data"), "{message}");
        assert!(!message.contains("secret-upstream-response"), "{message}");
    }

    #[test]
    fn attached_auth_failure_without_prior_facts_requires_authentication() {
        let (model, issue, may_refresh_local_sessions) = classify_attached_operator_result(
            Err(ControlPlaneError::HttpStatus {
                status: 401,
                body_excerpt: "credential detail".to_string(),
            }),
            "codex",
            None,
        );

        assert_eq!(model.status, OperatorReadStatus::AuthRequired);
        assert_eq!(
            issue,
            Some(AttachedOperatorRefreshIssue::AuthenticationRequired { status: 401 })
        );
        assert!(!may_refresh_local_sessions);
    }

    #[test]
    fn attached_wrong_service_response_cannot_replace_last_known_facts() {
        let previous = ready_operator_model();
        let mut wrong_service = ready_operator_model();
        wrong_service.service_name = "claude".to_string();

        let (model, issue, may_refresh_local_sessions) =
            classify_attached_operator_result(Ok(wrong_service), "codex", Some(&previous));

        assert_eq!(model.service_name, "codex");
        assert_eq!(model.status, OperatorReadStatus::Stale);
        assert_eq!(issue, Some(AttachedOperatorRefreshIssue::WrongService));
        assert!(!may_refresh_local_sessions);
    }

    #[test]
    fn superseded_attached_refresh_result_is_not_current() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut controller = AttachedSnapshotRefreshController::new(tx);
        controller.generation = 2;
        controller.in_flight = Some(1);
        controller.pending = true;
        let result = AttachedSnapshotRefreshResult {
            generation: 1,
            payload: AttachedSnapshotRefreshPayload {
                model: ready_operator_model(),
                local_sessions: HashMap::new(),
                operator_issue: None,
                local_session_issue: None,
            },
        };

        controller.finish(result.generation);

        assert!(!controller.result_is_current(&result));
        assert_eq!(controller.in_flight, None);
        assert!(controller.pending);
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

    #[tokio::test]
    async fn attached_page_switch_keeps_exit_semantics_read_only() {
        let mut ui = UiState {
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            ..Default::default()
        };

        assert!(
            handle_attached_key(
                &mut ui,
                &empty_snapshot(),
                &mut [],
                KeyEvent::from(KeyCode::Char('q')),
            )
            .await
        );

        assert!(ui.should_exit);
        assert!(ui.runtime_connection.is_attached());
    }

    #[tokio::test]
    async fn remote_observer_settings_do_not_handle_local_codex_switch_keys() {
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
                )
                .await,
                "{code:?} must remain unhandled in attached Settings"
            );
            assert!(ui.toast.is_none(), "{code:?} must not trigger a switch");
        }
        assert!(!ui.allows_local_codex_switch());
    }

    #[tokio::test]
    async fn attached_navigation_supports_core_pages() {
        let mut ui = UiState {
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            ..Default::default()
        };
        let snapshot = empty_snapshot();

        assert!(
            handle_attached_key(
                &mut ui,
                &snapshot,
                &mut [],
                KeyEvent::from(KeyCode::Char('2')),
            )
            .await
        );
        assert_eq!(ui.page, Page::Routing);
        assert_eq!(ui.focus, Focus::Providers);

        assert!(
            handle_attached_key(
                &mut ui,
                &snapshot,
                &mut [],
                KeyEvent::from(KeyCode::Char('4')),
            )
            .await
        );

        assert_eq!(ui.page, Page::Requests);
        assert_eq!(ui.focus, Focus::Requests);
    }

    #[tokio::test]
    async fn attached_navigation_supports_fleet_page() {
        let mut ui = UiState {
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            ..Default::default()
        };
        let snapshot = empty_snapshot();

        assert!(
            handle_attached_key(
                &mut ui,
                &snapshot,
                &mut [],
                KeyEvent::from(KeyCode::Char('0')),
            )
            .await
        );

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
    fn attached_language_prefers_environment_then_local_config_then_system() {
        assert_eq!(
            select_attached_language(Some("zh"), Some("en"), crate::tui::Language::En,),
            crate::tui::Language::Zh,
        );
        assert_eq!(
            select_attached_language(None, Some("zh"), crate::tui::Language::En),
            crate::tui::Language::Zh,
        );
        assert_eq!(
            select_attached_language(None, None, crate::tui::Language::En),
            crate::tui::Language::En,
        );
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

    #[tokio::test]
    async fn attached_stats_refresh_reloads_operator_read_model() {
        let mut ui = UiState {
            page: Page::Stats,
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            ..Default::default()
        };

        assert!(
            handle_attached_key(
                &mut ui,
                &empty_snapshot(),
                &mut [],
                KeyEvent::from(KeyCode::Char('g')),
            )
            .await
        );

        assert!(ui.needs_snapshot_refresh);
    }

    #[tokio::test]
    async fn attached_stats_tab_uses_shared_four_focus_cycle() {
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
            assert!(
                handle_attached_key(
                    &mut ui,
                    &empty_snapshot(),
                    &mut [],
                    KeyEvent::from(KeyCode::Tab),
                )
                .await
            );
            assert_eq!(ui.stats_focus, expected);
        }
    }

    #[tokio::test]
    async fn attached_routing_provider_info_uses_read_only_overlay_controls() {
        let mut ui = UiState {
            page: Page::Routing,
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            provider_info_scroll: 9,
            ..Default::default()
        };
        let snapshot = empty_snapshot();

        assert!(
            handle_attached_key(
                &mut ui,
                &snapshot,
                &mut [],
                KeyEvent::from(KeyCode::Char('i')),
            )
            .await
        );
        assert_eq!(ui.overlay, Overlay::ProviderInfo);
        assert_eq!(ui.provider_info_scroll, 0);

        assert!(
            handle_attached_key(
                &mut ui,
                &snapshot,
                &mut [],
                KeyEvent::from(KeyCode::PageDown),
            )
            .await
        );
        assert_eq!(ui.provider_info_scroll, 10);

        assert!(
            handle_attached_key(
                &mut ui,
                &snapshot,
                &mut [],
                KeyEvent::from(KeyCode::Char('i')),
            )
            .await
        );
        assert_eq!(ui.overlay, Overlay::None);
    }

    #[tokio::test]
    async fn attached_stats_report_export_key_handles_empty_selection() {
        let mut ui = UiState {
            page: Page::Stats,
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            ..Default::default()
        };

        assert!(
            handle_attached_key(
                &mut ui,
                &empty_snapshot(),
                &mut [],
                KeyEvent::from(KeyCode::Char('y')),
            )
            .await
        );
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

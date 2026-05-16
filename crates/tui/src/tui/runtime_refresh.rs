use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::config::ProxyConfig;
use crate::config::storage::load_config;
use crate::proxy::ProxyService;
use crate::state::ProxyState;
use crate::usage_providers::UsageProviderRefreshSummary;

use super::input;
use super::model::{Snapshot, build_provider_options};
use super::session_refresh::{
    CodexHistoryRefreshResult, CodexRecentRefreshResult, start_codex_history_refresh,
    start_codex_recent_refresh,
};
use super::snapshot_refresh::SnapshotRefreshController;
use super::state::UiState;
use super::types::Page;
use super::{Language, ProviderOption};
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
pub(super) struct DashboardTiming {
    pub(super) refresh_ms: u64,
    pub(super) io_timeout: Duration,
    pub(super) snapshot_fallback_interval: Duration,
}

impl DashboardTiming {
    pub(super) fn from_env() -> Self {
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
    if ui.page != Page::Settings
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
        || ui.page != Page::Stations
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
pub(super) async fn handle_ticker_refreshes(
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
pub(super) async fn handle_balance_refresh_result(
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
pub(super) async fn apply_pending_refresh_requests(
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

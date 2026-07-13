use std::time::Duration;

use tokio::sync::mpsc;

use super::model::Snapshot;
use super::session_refresh::{
    CodexHistoryRefreshResult, CodexRecentRefreshResult, start_codex_history_refresh,
    start_codex_recent_refresh,
};
use super::snapshot_refresh::SnapshotRefreshController;
use super::state::UiState;
use crate::proxy::ProxyService;

#[derive(Debug, Clone, Copy)]
pub(super) struct DashboardTiming {
    pub(super) refresh_ms: u64,
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
            snapshot_fallback_interval,
        }
    }
}

pub(super) fn handle_ticker_refreshes(
    proxy: &ProxyService,
    snapshot_fallback_interval: Duration,
    snapshot: &Snapshot,
    snapshot_refresh: &mut SnapshotRefreshController,
) {
    if snapshot.refreshed_at.elapsed() >= snapshot_fallback_interval {
        snapshot_refresh.request(proxy.clone());
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn apply_pending_refresh_requests(
    ui: &mut UiState,
    proxy: &ProxyService,
    snapshot_refresh: &mut SnapshotRefreshController,
    history_refresh_tx: mpsc::UnboundedSender<CodexHistoryRefreshResult>,
    recent_refresh_tx: mpsc::UnboundedSender<CodexRecentRefreshResult>,
) {
    if ui.needs_snapshot_refresh {
        snapshot_refresh.invalidate();
        snapshot_refresh.request(proxy.clone());
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

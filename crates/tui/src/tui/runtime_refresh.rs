use std::time::Duration;

use tokio::sync::mpsc;

use super::model::{Snapshot, now_ms};
use super::operator_actions::queue_balance_refresh;
use super::session_refresh::{
    CodexHistoryRefreshResult, CodexRecentRefreshResult, start_codex_history_refresh,
    start_codex_recent_refresh,
};
use super::snapshot_refresh::SnapshotRefreshController;
use super::state::UiState;
use super::types::Page;
use crate::proxy::ProxyService;
use crate::state::BalanceSnapshotStatus;

const BALANCE_MISSING_RETRY_INTERVAL: Duration = Duration::from_secs(10);
const BALANCE_PROBLEM_RETRY_INTERVAL: Duration = Duration::from_secs(60);
const BALANCE_SAMPLE_MAX_AGE_MS: u64 = 6 * 60 * 1_000;

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

fn balance_auto_refresh_retry_interval(snapshot: &Snapshot, now_ms: u64) -> Option<Duration> {
    let mut samples = snapshot.provider_balances.values().flatten().peekable();
    if samples.peek().is_none() {
        return Some(BALANCE_MISSING_RETRY_INTERVAL);
    }

    samples
        .any(|balance| {
            balance.stale
                || balance.stale_at(now_ms)
                || balance.fetched_at_ms == 0
                || now_ms.saturating_sub(balance.fetched_at_ms) >= BALANCE_SAMPLE_MAX_AGE_MS
                || matches!(
                    balance.status,
                    BalanceSnapshotStatus::Unknown
                        | BalanceSnapshotStatus::Stale
                        | BalanceSnapshotStatus::Error
                )
        })
        .then_some(BALANCE_PROBLEM_RETRY_INTERVAL)
}

fn should_auto_refresh_provider_balances(
    snapshot: &Snapshot,
    now_ms: u64,
    last_request_elapsed: Option<Duration>,
) -> bool {
    let Some(retry_interval) = balance_auto_refresh_retry_interval(snapshot, now_ms) else {
        return false;
    };
    last_request_elapsed.is_none_or(|elapsed| elapsed >= retry_interval)
}

pub(super) fn maybe_queue_stale_balance_refresh(ui: &mut UiState, snapshot: &Snapshot) -> bool {
    let last_request_elapsed = ui
        .last_balance_refresh_requested_at
        .map(|last| last.elapsed());
    if !matches!(ui.page, Page::Routing | Page::Stats)
        || !ui.can_refresh_provider_balances()
        || !should_auto_refresh_provider_balances(snapshot, now_ms(), last_request_elapsed)
    {
        return false;
    }

    queue_balance_refresh(ui, false, false)
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

#[cfg(test)]
mod tests {
    use super::super::operator_actions::PendingOperatorAction;
    use super::*;
    use crate::runtime_identity::ProviderEndpointKey;
    use crate::state::ProviderBalanceSnapshot;

    fn routing_ui() -> UiState {
        UiState {
            page: Page::Routing,
            ..UiState::default()
        }
    }

    fn snapshot_with_balance(
        fetched_at_ms: u64,
        stale_after_ms: Option<u64>,
        status: BalanceSnapshotStatus,
    ) -> Snapshot {
        let mut snapshot = Snapshot::default();
        snapshot.provider_balances.insert(
            "input".to_string(),
            vec![ProviderBalanceSnapshot {
                observation_provider_id: "input".to_string(),
                provider_endpoint: ProviderEndpointKey::new("codex", "input", "default"),
                source: "usage".to_string(),
                fetched_at_ms,
                stale_after_ms,
                status,
                ..ProviderBalanceSnapshot::default()
            }],
        );
        snapshot
    }

    #[test]
    fn missing_balance_sample_queues_one_non_forced_refresh() {
        let mut ui = routing_ui();

        assert!(maybe_queue_stale_balance_refresh(
            &mut ui,
            &Snapshot::default()
        ));
        assert!(matches!(
            ui.pending_operator_action,
            Some(PendingOperatorAction::RefreshBalances { force: false })
        ));
    }

    #[test]
    fn missing_balance_sample_retries_after_ten_seconds() {
        let snapshot = Snapshot::default();

        assert!(!should_auto_refresh_provider_balances(
            &snapshot,
            1_000,
            Some(BALANCE_MISSING_RETRY_INTERVAL - Duration::from_millis(1))
        ));
        assert!(should_auto_refresh_provider_balances(
            &snapshot,
            1_000,
            Some(BALANCE_MISSING_RETRY_INTERVAL)
        ));
    }

    #[test]
    fn unknown_and_error_samples_retry_after_one_minute() {
        let now = now_ms();
        for status in [BalanceSnapshotStatus::Unknown, BalanceSnapshotStatus::Error] {
            let snapshot = snapshot_with_balance(
                now.saturating_sub(1_000),
                Some(now.saturating_add(60_000)),
                status,
            );

            assert!(should_auto_refresh_provider_balances(&snapshot, now, None));
            assert!(!should_auto_refresh_provider_balances(
                &snapshot,
                now,
                Some(BALANCE_PROBLEM_RETRY_INTERVAL - Duration::from_millis(1))
            ));
            assert!(should_auto_refresh_provider_balances(
                &snapshot,
                now,
                Some(BALANCE_PROBLEM_RETRY_INTERVAL)
            ));
        }
    }

    #[test]
    fn stale_balance_refresh_is_rate_limited() {
        let now = now_ms();
        let snapshot = snapshot_with_balance(
            now.saturating_sub(10_000),
            Some(now.saturating_sub(1)),
            BalanceSnapshotStatus::Stale,
        );
        let mut ui = routing_ui();
        ui.last_balance_refresh_requested_at = std::time::Instant::now()
            .checked_sub(BALANCE_PROBLEM_RETRY_INTERVAL - Duration::from_millis(1));

        assert!(!maybe_queue_stale_balance_refresh(&mut ui, &snapshot));
        assert!(ui.pending_operator_action.is_none());
    }

    #[test]
    fn repeated_ticks_do_not_queue_while_a_recent_refresh_is_in_flight() {
        let mut ui = routing_ui();
        ui.operator_action_in_flight = true;
        ui.balance_refresh_in_flight = true;
        ui.last_balance_refresh_requested_at = Some(std::time::Instant::now());

        for _ in 0..3 {
            assert!(!maybe_queue_stale_balance_refresh(
                &mut ui,
                &Snapshot::default()
            ));
        }
        assert!(ui.pending_operator_action.is_none());
        assert!(!ui.deferred_auto_balance_refresh);
    }

    #[test]
    fn automatic_balance_refresh_runs_on_locally_controllable_balance_pages() {
        let mut dashboard_ui = UiState::default();
        assert!(!maybe_queue_stale_balance_refresh(
            &mut dashboard_ui,
            &Snapshot::default()
        ));

        let mut remote_ui = UiState {
            page: Page::Routing,
            runtime_connection: super::super::state::RuntimeConnectionKind::RemoteObserver,
            ..UiState::default()
        };
        assert!(!maybe_queue_stale_balance_refresh(
            &mut remote_ui,
            &Snapshot::default()
        ));
        assert!(dashboard_ui.pending_operator_action.is_none());
        assert!(remote_ui.pending_operator_action.is_none());

        let mut stats_ui = UiState {
            page: Page::Stats,
            ..UiState::default()
        };
        assert!(maybe_queue_stale_balance_refresh(
            &mut stats_ui,
            &Snapshot::default()
        ));
        assert!(matches!(
            stats_ui.pending_operator_action,
            Some(PendingOperatorAction::RefreshBalances { force: false })
        ));
    }

    #[test]
    fn stale_balance_refresh_retries_after_the_low_frequency_interval() {
        let now = now_ms();
        let snapshot = snapshot_with_balance(
            now.saturating_sub(10_000),
            Some(now.saturating_sub(1)),
            BalanceSnapshotStatus::Stale,
        );
        let mut ui = routing_ui();
        ui.last_balance_refresh_requested_at = std::time::Instant::now()
            .checked_sub(BALANCE_PROBLEM_RETRY_INTERVAL + Duration::from_secs(1));

        assert!(maybe_queue_stale_balance_refresh(&mut ui, &snapshot));
        assert!(matches!(
            ui.pending_operator_action,
            Some(PendingOperatorAction::RefreshBalances { force: false })
        ));
    }

    #[test]
    fn fresh_balance_sample_does_not_queue_a_refresh() {
        let now = now_ms();
        let snapshot = snapshot_with_balance(
            now.saturating_sub(60_000),
            Some(now.saturating_add(60_000)),
            BalanceSnapshotStatus::Ok,
        );
        let mut ui = routing_ui();

        assert!(!maybe_queue_stale_balance_refresh(&mut ui, &snapshot));
        assert!(ui.pending_operator_action.is_none());
    }

    #[test]
    fn exhausted_sample_is_fresh_until_its_deadline() {
        let now = now_ms();
        let snapshot = snapshot_with_balance(
            now.saturating_sub(60_000),
            Some(now.saturating_add(1)),
            BalanceSnapshotStatus::Exhausted,
        );

        assert!(!should_auto_refresh_provider_balances(&snapshot, now, None));
        assert!(should_auto_refresh_provider_balances(
            &snapshot,
            now.saturating_add(2),
            None
        ));
    }

    #[test]
    fn recent_sample_does_not_hide_an_expired_peer() {
        let now = now_ms();
        let mut snapshot = snapshot_with_balance(
            now.saturating_sub(1_000),
            Some(now.saturating_add(60_000)),
            BalanceSnapshotStatus::Ok,
        );
        snapshot
            .provider_balances
            .get_mut("input")
            .expect("provider balances")
            .push(ProviderBalanceSnapshot {
                observation_provider_id: "input-peer".to_string(),
                provider_endpoint: ProviderEndpointKey::new("codex", "input", "peer"),
                source: "usage".to_string(),
                fetched_at_ms: now.saturating_sub(BALANCE_SAMPLE_MAX_AGE_MS + 1),
                status: BalanceSnapshotStatus::Ok,
                ..ProviderBalanceSnapshot::default()
            });

        assert!(should_auto_refresh_provider_balances(&snapshot, now, None));
    }

    #[test]
    fn old_balance_sample_without_a_deadline_queues_a_refresh() {
        let now = now_ms();
        let snapshot = snapshot_with_balance(
            now.saturating_sub(BALANCE_SAMPLE_MAX_AGE_MS + 1),
            None,
            BalanceSnapshotStatus::Ok,
        );
        let mut ui = routing_ui();

        assert!(maybe_queue_stale_balance_refresh(&mut ui, &snapshot));
        assert!(matches!(
            ui.pending_operator_action,
            Some(PendingOperatorAction::RefreshBalances { force: false })
        ));
    }
}

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::proxy::ProxyService;
use crate::state::ProviderBalanceSnapshot;
use crate::tui::Language;
use crate::tui::model::{Snapshot, now_ms};
use crate::tui::state::UiState;
use crate::usage_providers::UsageProviderRefreshSummary;

pub(in crate::tui) type BalanceRefreshOutcome = Result<UsageProviderRefreshSummary, String>;
pub(in crate::tui) type BalanceRefreshSender = mpsc::UnboundedSender<BalanceRefreshOutcome>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum BalanceRefreshMode {
    Auto,
    Force,
    ControlChanged,
}

fn balance_auto_refresh_cooldown(
    provider_balances: &HashMap<String, Vec<ProviderBalanceSnapshot>>,
    now_ms: u64,
) -> Option<Duration> {
    let mut seen = false;
    let mut any_refresh_worthy = false;

    for snapshot in provider_balances.values().flatten() {
        seen = true;
        let expired = snapshot.stale
            || snapshot
                .stale_after_ms
                .is_some_and(|stale_after_ms| now_ms > stale_after_ms);
        let problematic = matches!(
            snapshot.status,
            crate::state::BalanceSnapshotStatus::Unknown
                | crate::state::BalanceSnapshotStatus::Error
        );
        any_refresh_worthy |= expired || problematic;
    }

    if !seen {
        Some(Duration::from_secs(10))
    } else if any_refresh_worthy {
        Some(Duration::from_secs(60))
    } else {
        None
    }
}

pub(super) fn should_request_provider_balance_refresh(
    provider_balances: &HashMap<String, Vec<ProviderBalanceSnapshot>>,
    mode: BalanceRefreshMode,
    now_ms: u64,
    last_request_elapsed: Option<Duration>,
) -> bool {
    let cooldown = match mode {
        BalanceRefreshMode::Force => Some(Duration::from_secs(2)),
        BalanceRefreshMode::ControlChanged => Some(Duration::ZERO),
        BalanceRefreshMode::Auto => balance_auto_refresh_cooldown(provider_balances, now_ms),
    };

    let Some(cooldown) = cooldown else {
        return false;
    };

    last_request_elapsed.is_none_or(|elapsed| elapsed >= cooldown)
}

pub(in crate::tui) fn request_provider_balance_refresh(
    ui: &mut UiState,
    snapshot: &Snapshot,
    proxy: &ProxyService,
    mode: BalanceRefreshMode,
    balance_refresh_tx: &BalanceRefreshSender,
) -> bool {
    let now = Instant::now();
    let last_request_elapsed = ui
        .last_balance_refresh_requested_at
        .map(|last| now.duration_since(last));
    if !should_request_provider_balance_refresh(
        &snapshot.provider_balances,
        mode,
        now_ms(),
        last_request_elapsed,
    ) {
        return false;
    }
    ui.last_balance_refresh_requested_at = Some(now);
    ui.balance_refresh_in_flight = true;
    ui.last_balance_refresh_message = Some(match ui.language {
        Language::Zh => "余额刷新中".to_string(),
        Language::En => "balance refresh in progress".to_string(),
    });
    ui.last_balance_refresh_error = None;
    ui.last_balance_refresh_summary = None;
    let proxy = proxy.clone();
    let balance_refresh_tx = balance_refresh_tx.clone();
    tokio::spawn(async move {
        let outcome = proxy
            .refresh_provider_balances(None, None)
            .await
            .map(|response| response.refresh)
            .map_err(|err| err.to_string());
        let _ = balance_refresh_tx.send(outcome);
    });
    true
}

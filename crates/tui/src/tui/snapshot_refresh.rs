use std::sync::Arc;

use tokio::sync::mpsc;

use crate::config::ProxyConfig;
use crate::state::ProxyState;

use super::model::{ForecastRecentMode, Snapshot, refresh_snapshot};

#[derive(Debug)]
pub(super) struct SnapshotRefreshResult {
    pub(super) generation: u64,
    pub(super) config_version: Option<u32>,
    pub(super) stats_days: usize,
    pub(super) forecast_mode: ForecastRecentMode,
    pub(super) snapshot: Snapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SnapshotRefreshKey {
    generation: u64,
    config_version: Option<u32>,
    stats_days: usize,
    forecast_mode: ForecastRecentMode,
}

fn snapshot_refresh_result_is_current(
    result: SnapshotRefreshKey,
    current: SnapshotRefreshKey,
) -> bool {
    result == current
}

#[derive(Debug)]
pub(super) struct SnapshotRefreshController {
    tx: mpsc::UnboundedSender<SnapshotRefreshResult>,
    generation: u64,
    in_flight: Option<u64>,
    pending: bool,
}

impl SnapshotRefreshController {
    pub(super) fn new(tx: mpsc::UnboundedSender<SnapshotRefreshResult>) -> Self {
        Self {
            tx,
            generation: 0,
            in_flight: None,
            pending: false,
        }
    }

    pub(super) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.in_flight = None;
        self.pending = false;
    }

    pub(super) fn request(
        &mut self,
        state: Arc<ProxyState>,
        cfg: Arc<ProxyConfig>,
        service_name: &'static str,
        stats_days: usize,
        forecast_mode: ForecastRecentMode,
    ) {
        if self.in_flight.is_some() {
            self.pending = true;
            return;
        }

        self.start(state, cfg, service_name, stats_days, forecast_mode);
    }

    pub(super) fn finish(&mut self, generation: u64) {
        if self.in_flight == Some(generation) {
            self.in_flight = None;
        }
    }

    pub(super) fn result_is_current(
        &self,
        result: &SnapshotRefreshResult,
        current_config_version: Option<u32>,
        current_stats_days: usize,
        current_forecast_mode: ForecastRecentMode,
    ) -> bool {
        snapshot_refresh_result_is_current(
            SnapshotRefreshKey {
                generation: result.generation,
                config_version: result.config_version,
                stats_days: result.stats_days,
                forecast_mode: result.forecast_mode,
            },
            SnapshotRefreshKey {
                generation: self.generation,
                config_version: current_config_version,
                stats_days: current_stats_days,
                forecast_mode: current_forecast_mode,
            },
        )
    }

    pub(super) fn request_pending_if_idle(
        &mut self,
        state: Arc<ProxyState>,
        cfg: Arc<ProxyConfig>,
        service_name: &'static str,
        stats_days: usize,
        forecast_mode: ForecastRecentMode,
    ) {
        if self.pending && self.in_flight.is_none() {
            self.request(state, cfg, service_name, stats_days, forecast_mode);
        }
    }

    fn start(
        &mut self,
        state: Arc<ProxyState>,
        cfg: Arc<ProxyConfig>,
        service_name: &'static str,
        stats_days: usize,
        forecast_mode: ForecastRecentMode,
    ) {
        debug_assert!(self.in_flight.is_none());
        self.pending = false;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.in_flight = Some(generation);
        let config_version = cfg.version;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let snapshot =
                refresh_snapshot(&state, cfg, service_name, stats_days, forecast_mode).await;
            let _ = tx.send(SnapshotRefreshResult {
                generation,
                config_version,
                stats_days,
                forecast_mode,
                snapshot,
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        ForecastRecentMode, SnapshotRefreshController, SnapshotRefreshKey,
        snapshot_refresh_result_is_current,
    };
    use crate::config::ProxyConfig;
    use crate::state::ProxyState;

    #[test]
    fn snapshot_refresh_result_guard_rejects_stale_results() {
        let current = SnapshotRefreshKey {
            generation: 3,
            config_version: Some(5),
            stats_days: 7,
            forecast_mode: ForecastRecentMode::RuntimeOnly,
        };

        assert!(snapshot_refresh_result_is_current(
            SnapshotRefreshKey { ..current },
            current
        ));
        assert!(!snapshot_refresh_result_is_current(
            SnapshotRefreshKey {
                generation: 2,
                ..current
            },
            current
        ));
        assert!(!snapshot_refresh_result_is_current(
            SnapshotRefreshKey {
                config_version: Some(4),
                ..current
            },
            current
        ));
        assert!(!snapshot_refresh_result_is_current(
            SnapshotRefreshKey {
                stats_days: 30,
                ..current
            },
            current
        ));
        assert!(!snapshot_refresh_result_is_current(
            SnapshotRefreshKey {
                forecast_mode: ForecastRecentMode::IncludeRequestLedger,
                ..current
            },
            current
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
            ForecastRecentMode::RuntimeOnly,
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
            ForecastRecentMode::RuntimeOnly,
        );

        assert_eq!(controller.generation, 8);
        assert_eq!(controller.in_flight, Some(8));
        assert!(!controller.pending);
    }
}

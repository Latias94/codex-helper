use std::sync::Arc;

use tokio::sync::mpsc;

use crate::config::ProxyConfig;
use crate::state::ProxyState;

use super::model::{Snapshot, refresh_snapshot};

#[derive(Debug)]
pub(super) struct SnapshotRefreshResult {
    pub(super) generation: u64,
    pub(super) config_version: Option<u32>,
    pub(super) stats_days: usize,
    pub(super) snapshot: Snapshot,
}

fn snapshot_refresh_result_is_current(
    result_generation: u64,
    result_config_version: Option<u32>,
    result_stats_days: usize,
    current_generation: u64,
    current_config_version: Option<u32>,
    current_stats_days: usize,
) -> bool {
    result_generation == current_generation
        && result_config_version == current_config_version
        && result_stats_days == current_stats_days
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
    ) {
        if self.in_flight.is_some() {
            self.pending = true;
            return;
        }

        self.start(state, cfg, service_name, stats_days);
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
    ) -> bool {
        snapshot_refresh_result_is_current(
            result.generation,
            result.config_version,
            result.stats_days,
            self.generation,
            current_config_version,
            current_stats_days,
        )
    }

    pub(super) fn request_pending_if_idle(
        &mut self,
        state: Arc<ProxyState>,
        cfg: Arc<ProxyConfig>,
        service_name: &'static str,
        stats_days: usize,
    ) {
        if self.pending && self.in_flight.is_none() {
            self.request(state, cfg, service_name, stats_days);
        }
    }

    fn start(
        &mut self,
        state: Arc<ProxyState>,
        cfg: Arc<ProxyConfig>,
        service_name: &'static str,
        stats_days: usize,
    ) {
        debug_assert!(self.in_flight.is_none());
        self.pending = false;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.in_flight = Some(generation);
        let config_version = cfg.version;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let snapshot = refresh_snapshot(&state, cfg, service_name, stats_days).await;
            let _ = tx.send(SnapshotRefreshResult {
                generation,
                config_version,
                stats_days,
                snapshot,
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{SnapshotRefreshController, snapshot_refresh_result_is_current};
    use crate::config::ProxyConfig;
    use crate::state::ProxyState;

    #[test]
    fn snapshot_refresh_result_guard_rejects_stale_results() {
        assert!(snapshot_refresh_result_is_current(
            3,
            Some(5),
            7,
            3,
            Some(5),
            7
        ));
        assert!(!snapshot_refresh_result_is_current(
            2,
            Some(5),
            7,
            3,
            Some(5),
            7
        ));
        assert!(!snapshot_refresh_result_is_current(
            3,
            Some(4),
            7,
            3,
            Some(5),
            7
        ));
        assert!(!snapshot_refresh_result_is_current(
            3,
            Some(5),
            30,
            3,
            Some(5),
            7
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
        );

        assert_eq!(controller.generation, 8);
        assert_eq!(controller.in_flight, Some(8));
        assert!(!controller.pending);
    }
}

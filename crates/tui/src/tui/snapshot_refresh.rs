use tokio::sync::mpsc;

use crate::dashboard_core::OperatorReadCapture;
use crate::proxy::ProxyService;

#[derive(Debug)]
pub(super) struct SnapshotRefreshResult {
    pub(super) generation: u64,
    pub(super) capture: Result<OperatorReadCapture, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SnapshotRefreshKey {
    generation: u64,
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

    pub(super) fn request(&mut self, proxy: ProxyService) {
        let Some(generation) = self.begin_request() else {
            return;
        };
        self.spawn(proxy, generation);
    }

    pub(super) fn finish(&mut self, generation: u64) {
        if self.in_flight == Some(generation) {
            self.in_flight = None;
        }
    }

    pub(super) fn result_is_current(&self, result: &SnapshotRefreshResult) -> bool {
        snapshot_refresh_result_is_current(
            SnapshotRefreshKey {
                generation: result.generation,
            },
            SnapshotRefreshKey {
                generation: self.generation,
            },
        )
    }

    pub(super) fn request_pending_if_idle(&mut self, proxy: ProxyService) {
        if let Some(generation) = self.begin_pending_request() {
            self.spawn(proxy, generation);
        }
    }

    fn begin_request(&mut self) -> Option<u64> {
        if self.in_flight.is_some() {
            self.pending = true;
            return None;
        }

        self.pending = false;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.in_flight = Some(generation);
        Some(generation)
    }

    fn begin_pending_request(&mut self) -> Option<u64> {
        (self.pending && self.in_flight.is_none())
            .then(|| self.begin_request())
            .flatten()
    }

    fn spawn(&self, proxy: ProxyService, generation: u64) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let capture = proxy
                .operator_read_capture()
                .await
                .map_err(|error| error.to_string());
            let _ = tx.send(SnapshotRefreshResult {
                generation,
                capture,
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SnapshotRefreshController, SnapshotRefreshKey, snapshot_refresh_result_is_current,
    };

    #[test]
    fn snapshot_refresh_result_guard_rejects_stale_results() {
        let current = SnapshotRefreshKey { generation: 3 };

        assert!(snapshot_refresh_result_is_current(
            SnapshotRefreshKey { ..current },
            current
        ));
        assert!(!snapshot_refresh_result_is_current(
            SnapshotRefreshKey { generation: 2 },
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

        assert_eq!(controller.begin_request(), None);

        assert_eq!(controller.generation, 7);
        assert_eq!(controller.in_flight, Some(7));
        assert!(controller.pending);
    }

    #[test]
    fn snapshot_refresh_controller_restarts_pending_work_after_current_finish() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut controller = SnapshotRefreshController::new(tx);
        controller.generation = 7;
        controller.in_flight = Some(7);
        controller.pending = true;

        controller.finish(7);
        assert_eq!(controller.begin_pending_request(), Some(8));

        assert_eq!(controller.generation, 8);
        assert_eq!(controller.in_flight, Some(8));
        assert!(!controller.pending);
    }
}

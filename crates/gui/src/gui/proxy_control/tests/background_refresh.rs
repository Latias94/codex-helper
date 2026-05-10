use super::*;
use crate::config::ServiceKind;

#[test]
fn forced_foreground_refresh_clears_inflight_background_refresh() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build runtime");
    let (_tx, rx) = std::sync::mpsc::channel();
    let join = rt.spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
    });
    let mut controller = ProxyController::new(3210, ServiceKind::Codex);
    controller.background_refresh =
        Some(super::super::types::ProxyBackgroundRefreshTask { rx, join });

    controller.refresh_current_if_due(&rt, Duration::ZERO);

    assert!(controller.background_refresh.is_none());
}

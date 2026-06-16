use axum::Json;
use axum::http::StatusCode;

use crate::fleet::{FleetSnapshot, build_local_fleet_snapshot_from_dashboard};

use super::super::ProxyService;

pub(in crate::proxy) async fn api_fleet_snapshot(
    proxy: ProxyService,
) -> Result<Json<FleetSnapshot>, (StatusCode, String)> {
    let cfg = proxy.config.snapshot().await;
    let mgr = proxy.service_manager(cfg.as_ref());
    let mut dashboard =
        crate::dashboard_core::build_dashboard_snapshot(&proxy.state, proxy.service_name, 2_000, 7)
            .await;
    crate::state::enrich_session_identity_cards_with_runtime(&mut dashboard.session_cards, mgr);

    Ok(Json(build_local_fleet_snapshot_from_dashboard(
        proxy.service_name,
        "local",
        "local",
        &dashboard,
    )))
}

use super::*;

pub(super) fn capability_and_session_routes(proxy: ProxyService) -> Router {
    let capabilities_proxy = proxy.clone();
    let codex_capabilities_proxy = proxy.clone();
    let codex_live_smoke_proxy = proxy.clone();
    let snapshot_proxy = proxy.clone();
    let fleet_snapshot_proxy = proxy.clone();
    let summary_proxy = proxy.clone();
    let sessions_proxy = proxy.clone();

    Router::new()
        .route(
            API_V1_CAPABILITIES,
            get(move || api_capabilities(capabilities_proxy.clone())),
        )
        .route(
            API_V1_CODEX_RELAY_CAPABILITIES,
            post(move |payload| {
                codex_relay_capabilities(codex_capabilities_proxy.clone(), payload)
            }),
        )
        .route(
            API_V1_CODEX_RELAY_LIVE_SMOKE,
            post(move |payload| codex_relay_live_smoke(codex_live_smoke_proxy.clone(), payload)),
        )
        .route(
            API_V1_SNAPSHOT,
            get(move |query| api_v1_snapshot(snapshot_proxy.clone(), query)),
        )
        .route(
            API_V1_FLEET_SNAPSHOT,
            get(move || api_fleet_snapshot(fleet_snapshot_proxy.clone())),
        )
        .route(
            API_V1_OPERATOR_SUMMARY,
            get(move || api_operator_summary(summary_proxy.clone())),
        )
        .route(
            API_V1_SESSIONS,
            get(move || list_session_identity_cards(sessions_proxy.clone())),
        )
        .route(
            API_V1_SESSION_BY_ID,
            get(move |session_id| get_session_identity_card(proxy.clone(), session_id)),
        )
}

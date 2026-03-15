use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;

use crate::state::{ActiveRequest, FinishedRequest};

use super::super::ProxyService;
use super::{RecentQuery, require_session_id};

async fn load_session_identity_cards(
    proxy: &ProxyService,
) -> Vec<crate::state::SessionIdentityCard> {
    let mut cards = proxy
        .state
        .list_session_identity_cards_with_host_transcripts(2_000)
        .await;
    let cfg = proxy.config.snapshot().await;
    let mgr = proxy.service_manager(cfg.as_ref());
    crate::state::enrich_session_identity_cards_with_runtime(&mut cards, mgr);
    cards
}

pub(in crate::proxy) async fn list_active_requests(
    proxy: ProxyService,
) -> Result<Json<Vec<ActiveRequest>>, (StatusCode, String)> {
    Ok(Json(proxy.state.list_active_requests().await))
}

pub(in crate::proxy) async fn list_session_stats(
    proxy: ProxyService,
) -> Result<Json<HashMap<String, crate::state::SessionStats>>, (StatusCode, String)> {
    Ok(Json(proxy.state.list_session_stats().await))
}

pub(in crate::proxy) async fn list_session_identity_cards(
    proxy: ProxyService,
) -> Result<Json<Vec<crate::state::SessionIdentityCard>>, (StatusCode, String)> {
    Ok(Json(load_session_identity_cards(&proxy).await))
}

pub(in crate::proxy) async fn get_session_identity_card(
    proxy: ProxyService,
    Path(session_id): Path<String>,
) -> Result<Json<crate::state::SessionIdentityCard>, (StatusCode, String)> {
    require_session_id(session_id.as_str())?;
    let cards = load_session_identity_cards(&proxy).await;
    cards
        .into_iter()
        .find(|card| card.session_id.as_deref() == Some(session_id.as_str()))
        .map(Json)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("session '{}' not found", session_id),
            )
        })
}

pub(in crate::proxy) async fn list_recent_finished(
    proxy: ProxyService,
    Query(query): Query<RecentQuery>,
) -> Result<Json<Vec<FinishedRequest>>, (StatusCode, String)> {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    Ok(Json(proxy.state.list_recent_finished(limit).await))
}

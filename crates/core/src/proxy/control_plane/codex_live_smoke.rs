use axum::Json;

use super::super::{
    CodexRelayLiveSmokeRequest, CodexRelayLiveSmokeResponse, ProxyControlError, ProxyService,
};

pub(in crate::proxy) async fn codex_relay_live_smoke(
    proxy: ProxyService,
    Json(payload): Json<CodexRelayLiveSmokeRequest>,
) -> Result<Json<CodexRelayLiveSmokeResponse>, (axum::http::StatusCode, String)> {
    proxy
        .codex_relay_live_smoke(payload)
        .await
        .map(Json)
        .map_err(ProxyControlError::into_http_error)
}

use axum::Json;

use super::super::{
    CodexRelayCapabilitiesRequest, CodexRelayCapabilitiesResponse, ProxyControlError, ProxyService,
};

pub(in crate::proxy) async fn codex_relay_capabilities(
    proxy: ProxyService,
    Json(payload): Json<CodexRelayCapabilitiesRequest>,
) -> Result<Json<CodexRelayCapabilitiesResponse>, (axum::http::StatusCode, String)> {
    proxy
        .codex_relay_capabilities(payload)
        .await
        .map(Json)
        .map_err(ProxyControlError::into_http_error)
}

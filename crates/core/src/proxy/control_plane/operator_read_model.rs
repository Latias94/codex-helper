use axum::Json;
use axum::http::StatusCode;

use crate::dashboard_core::OperatorReadModel;

use super::super::ProxyService;
async fn operator_read_model(
    proxy: &ProxyService,
) -> Result<OperatorReadModel, (StatusCode, String)> {
    proxy.operator_read_model().await.map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            error.message().to_string(),
        )
    })
}

pub(in crate::proxy) async fn api_operator_read_model(
    proxy: ProxyService,
) -> Result<Json<OperatorReadModel>, (StatusCode, String)> {
    operator_read_model(&proxy).await.map(Json)
}

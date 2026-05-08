use axum::Json;
use axum::extract::Query;
use axum::http::StatusCode;

use super::ProxyService;
use super::api_responses::{
    ProfilesResponse, ReloadResult, RetryConfigResponse, RuntimeStatusResponse,
    build_reload_result, build_retry_config_response, build_runtime_status_response,
    make_profiles_response,
};
use super::control_plane_service::save_runtime_proxy_settings_and_reload;

#[derive(serde::Deserialize)]
pub(super) struct ControlTraceQuery {
    limit: Option<usize>,
}

#[derive(serde::Deserialize)]
pub(super) struct RequestLedgerRecentQuery {
    limit: Option<usize>,
    session: Option<String>,
    model: Option<String>,
    station: Option<String>,
    provider: Option<String>,
    status_min: Option<u64>,
    status_max: Option<u64>,
    fast: Option<bool>,
    retried: Option<bool>,
}

impl RequestLedgerRecentQuery {
    fn filters(&self) -> crate::request_ledger::RequestLogFilters {
        crate::request_ledger::RequestLogFilters {
            session: clean_filter(self.session.clone()),
            model: clean_filter(self.model.clone()),
            station: clean_filter(self.station.clone()),
            provider: clean_filter(self.provider.clone()),
            status_min: self.status_min,
            status_max: self.status_max,
            fast: self.fast.unwrap_or(false),
            retried: self.retried.unwrap_or(false),
        }
    }
}

#[derive(serde::Deserialize)]
pub(super) struct RequestLedgerSummaryQuery {
    limit: Option<usize>,
    by: Option<String>,
    session: Option<String>,
    model: Option<String>,
    station: Option<String>,
    provider: Option<String>,
    status_min: Option<u64>,
    status_max: Option<u64>,
    fast: Option<bool>,
    retried: Option<bool>,
}

impl RequestLedgerSummaryQuery {
    fn filters(&self) -> crate::request_ledger::RequestLogFilters {
        crate::request_ledger::RequestLogFilters {
            session: clean_filter(self.session.clone()),
            model: clean_filter(self.model.clone()),
            station: clean_filter(self.station.clone()),
            provider: clean_filter(self.provider.clone()),
            status_min: self.status_min,
            status_max: self.status_max,
            fast: self.fast.unwrap_or(false),
            retried: self.retried.unwrap_or(false),
        }
    }
}

fn clean_filter(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(super) async fn runtime_status(
    proxy: ProxyService,
) -> Result<Json<RuntimeStatusResponse>, (StatusCode, String)> {
    Ok(Json(build_runtime_status_response(&proxy).await))
}

pub(super) async fn get_retry_config(
    proxy: ProxyService,
) -> Result<Json<RetryConfigResponse>, (StatusCode, String)> {
    let cfg = proxy.config.snapshot().await;
    Ok(Json(build_retry_config_response(cfg.as_ref())))
}

pub(super) async fn get_pricing_catalog(
    _proxy: ProxyService,
) -> Result<Json<crate::pricing::ModelPriceCatalogSnapshot>, (StatusCode, String)> {
    Ok(Json(crate::pricing::operator_model_price_catalog_snapshot()))
}

pub(super) async fn get_request_ledger_recent(
    _proxy: ProxyService,
    Query(q): Query<RequestLedgerRecentQuery>,
) -> Result<Json<Vec<crate::state::FinishedRequest>>, (StatusCode, String)> {
    let limit = q.limit.unwrap_or(1000).clamp(20, 5000);
    let filters = q.filters();
    let path = crate::request_ledger::request_log_path();
    let records = if filters.is_empty() {
        crate::request_ledger::tail_finished_requests_from_log(&path, limit)
    } else {
        crate::request_ledger::find_finished_requests_from_log(&path, &filters, limit)
    };
    records
        .map(Json)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

pub(super) async fn get_request_ledger_summary(
    _proxy: ProxyService,
    Query(q): Query<RequestLedgerSummaryQuery>,
) -> Result<Json<Vec<crate::request_ledger::RequestUsageSummaryRow>>, (StatusCode, String)> {
    let limit = q.limit.unwrap_or(30).clamp(1, 100);
    let group = match q
        .by
        .as_deref()
        .unwrap_or("station")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "provider" => crate::request_ledger::RequestUsageSummaryGroup::Provider,
        "model" => crate::request_ledger::RequestUsageSummaryGroup::Model,
        "session" => crate::request_ledger::RequestUsageSummaryGroup::Session,
        _ => crate::request_ledger::RequestUsageSummaryGroup::Station,
    };
    let filters = q.filters();
    let path = crate::request_ledger::request_log_path();
    crate::request_ledger::summarize_request_log(&path, group, &filters, limit)
        .map(Json)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

pub(super) async fn set_retry_config(
    proxy: ProxyService,
    Json(payload): Json<crate::config::RetryConfig>,
) -> Result<Json<RetryConfigResponse>, (StatusCode, String)> {
    let cfg_snapshot = proxy.config.snapshot().await;
    let mut cfg = cfg_snapshot.as_ref().clone();
    cfg.retry = payload;

    save_runtime_proxy_settings_and_reload(&proxy, cfg).await?;
    let cfg = proxy.config.snapshot().await;
    Ok(Json(build_retry_config_response(cfg.as_ref())))
}

pub(super) async fn reload_runtime_config(
    proxy: ProxyService,
) -> Result<Json<ReloadResult>, (StatusCode, String)> {
    let changed = proxy
        .config
        .force_reload_from_disk()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let status = build_runtime_status_response(&proxy).await;
    Ok(Json(build_reload_result(changed, status)))
}

pub(super) async fn list_profiles(
    proxy: ProxyService,
) -> Result<Json<ProfilesResponse>, (StatusCode, String)> {
    Ok(Json(make_profiles_response(&proxy).await))
}

pub(super) async fn get_control_trace(
    _proxy: ProxyService,
    Query(q): Query<ControlTraceQuery>,
) -> Result<Json<Vec<crate::logging::ControlTraceLogEntry>>, (StatusCode, String)> {
    let limit = q.limit.unwrap_or(80).clamp(20, 400);
    crate::logging::read_recent_control_trace_entries(limit)
        .map(Json)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

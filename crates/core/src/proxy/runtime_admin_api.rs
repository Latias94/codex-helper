use std::sync::Arc;

use axum::Json;
use axum::extract::Query;
use axum::http::StatusCode;

use crate::config::ProxyConfig;
use crate::lb::LoadBalancer;
use crate::routing_explain::{RoutingExplainResponse, build_routing_explain_response};
use crate::routing_ir::compile_legacy_route_plan_template;

use super::ProxyService;
use super::api_responses::{
    ProfilesResponse, ReloadResult, RetryConfigResponse, RuntimeStatusResponse,
    build_reload_result, build_retry_config_response, build_runtime_status_response,
    make_profiles_response,
};
use super::control_plane_service::{
    prune_runtime_observability_after_reload, save_runtime_proxy_settings_and_reload,
};
use super::route_executor_runtime::route_plan_runtime_state_from_lbs;
use super::routing_plan::{
    PinnedRoutingSelection, build_station_routing_plan, resolve_pinned_station_selection,
};

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

#[derive(serde::Deserialize)]
pub(super) struct RoutingExplainQuery {
    model: Option<String>,
    session: Option<String>,
    session_id: Option<String>,
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

impl RoutingExplainQuery {
    fn request_model(&self) -> Option<String> {
        clean_filter(self.model.clone())
    }

    fn session_id(&self) -> Option<String> {
        clean_filter(self.session_id.clone()).or_else(|| clean_filter(self.session.clone()))
    }
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

pub(super) async fn get_routing_explain(
    proxy: ProxyService,
    Query(q): Query<RoutingExplainQuery>,
) -> Result<Json<RoutingExplainResponse>, (StatusCode, String)> {
    let cfg = proxy.config.snapshot().await;
    let request_model = q.request_model();
    let session_id = q.session_id();
    let lbs = routing_explain_load_balancers(&proxy, cfg.as_ref(), session_id.as_deref()).await;
    let template = compile_legacy_route_plan_template(
        proxy.service_name,
        lbs.iter().map(|lb| lb.service.as_ref()),
    );
    let runtime = route_plan_runtime_state_from_lbs(&lbs);
    Ok(Json(build_routing_explain_response(
        proxy.service_name,
        Some(proxy.config.last_loaded_at_ms()),
        request_model,
        session_id,
        &template,
        &runtime,
    )))
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

async fn routing_explain_load_balancers(
    proxy: &ProxyService,
    cfg: &ProxyConfig,
    session_id: Option<&str>,
) -> Vec<LoadBalancer> {
    let mgr = proxy.service_manager(cfg);
    let (meta_overrides, state_overrides, upstream_overrides, provider_balances) = tokio::join!(
        proxy.state.get_station_meta_overrides(proxy.service_name),
        proxy
            .state
            .get_station_runtime_state_overrides(proxy.service_name),
        proxy.state.get_upstream_meta_overrides(proxy.service_name),
        proxy
            .state
            .get_provider_balance_summary_view(proxy.service_name),
    );

    if let Some((name, _source)) = proxy.pinned_config(mgr, session_id).await {
        return match resolve_pinned_station_selection(
            mgr,
            name.as_str(),
            &state_overrides,
            &upstream_overrides,
        ) {
            PinnedRoutingSelection::Selected(candidate) => vec![LoadBalancer::new(
                Arc::new(candidate.service),
                proxy.lb_states.clone(),
            )],
            PinnedRoutingSelection::BlockedBreakerOpen | PinnedRoutingSelection::Missing => {
                Vec::new()
            }
        };
    }

    let plan = build_station_routing_plan(
        mgr,
        mgr.active.as_deref(),
        &meta_overrides,
        &state_overrides,
        &upstream_overrides,
        &provider_balances,
    );

    plan.selected_stations
        .into_iter()
        .map(|candidate| LoadBalancer::new(Arc::new(candidate.service), proxy.lb_states.clone()))
        .collect()
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
    if changed {
        prune_runtime_observability_after_reload(&proxy).await;
    }
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

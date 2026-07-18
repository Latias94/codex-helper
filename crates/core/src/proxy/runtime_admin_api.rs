use axum::Json;
use axum::extract::Query;

use crate::routing_explain::{RoutingExplainResponse, build_routing_explain_response_with_request};
use crate::routing_ir::RouteRequestContext;

use super::ProxyService;
use super::admin_api_error::{AdminApiHttpError, AdminApiResult};
use super::route_affinity::apply_session_route_affinity_for_template;
use super::route_target_selection::{
    apply_auth_resolution_to_runtime, apply_concurrency_snapshots_to_runtime,
    apply_routing_operator_control_to_runtime,
};

#[derive(serde::Deserialize)]
pub(super) struct RequestLedgerChainQuery {
    limit: Option<usize>,
    trace_id: Option<String>,
    request_id: Option<u64>,
    session: Option<String>,
    session_id: Option<String>,
}

impl RequestLedgerChainQuery {
    fn selector(&self) -> crate::request_chain::RequestChainSelector {
        crate::request_chain::RequestChainSelector {
            trace_id: clean_filter(self.trace_id.clone()),
            request_id: self.request_id,
            session_id: clean_filter(self.session_id.clone())
                .or_else(|| clean_filter(self.session.clone())),
        }
    }
}

fn clean_filter(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(super) async fn routing_explain_for_proxy(
    proxy: &ProxyService,
    request: RouteRequestContext,
    session_id: Option<String>,
) -> Result<RoutingExplainResponse, AdminApiHttpError> {
    let runtime_snapshot = proxy.config.capture().await;
    let provider_policy = runtime_snapshot.provider_policy();
    let graph = runtime_snapshot
        .route_graph(proxy.service_name)
        .ok_or_else(|| {
            AdminApiHttpError::internal(
                "admin_routing_explain_unknown_service",
                "captured runtime snapshot has no route graph for the service",
            )
        })?;
    let routing_control_graph_key = graph.digest().to_string();
    let template = graph.route_plan(&request).map_err(|error| {
        AdminApiHttpError::internal("admin_routing_explain_route_plan_failed", error.to_string())
    })?;
    let runtime_identities = template.candidate_identities().map_err(|error| {
        AdminApiHttpError::internal(
            "admin_routing_explain_credential_binding_failed",
            error.to_string(),
        )
    })?;
    let mut runtime = proxy
        .state
        .route_plan_runtime_state_with_provider_policy(
            proxy.service_name,
            provider_policy.as_ref(),
            runtime_snapshot.revision(),
            runtime_identities.as_slice(),
        )
        .await;
    apply_auth_resolution_to_runtime(proxy.service_name, &template, &mut runtime).map_err(
        |error| {
            AdminApiHttpError::internal(
                "admin_routing_explain_credential_binding_failed",
                error.to_string(),
            )
        },
    )?;
    apply_concurrency_snapshots_to_runtime(
        proxy,
        &template,
        runtime_snapshot.revision(),
        &mut runtime,
    );
    apply_session_route_affinity_for_template(
        proxy,
        session_id.as_deref(),
        &template,
        &mut runtime,
    )
    .await;
    apply_routing_operator_control_to_runtime(
        proxy,
        routing_control_graph_key.as_str(),
        &mut runtime,
    )
    .await;
    Ok(build_routing_explain_response_with_request(
        proxy.service_name,
        Some(runtime_snapshot.loaded_at_ms()),
        request,
        session_id,
        &template,
        &runtime,
    ))
}

pub(super) async fn get_request_ledger_chain(
    proxy: ProxyService,
    Query(query): Query<RequestLedgerChainQuery>,
) -> AdminApiResult<crate::request_chain::RequestChainExport> {
    let selector = query.selector();
    if !selector.has_identity() {
        return Err(AdminApiHttpError::bad_request(
            "admin_request_chain_selector_required",
            "trace_id, request_id, or session is required",
        ));
    }
    let limit = query
        .limit
        .unwrap_or(crate::request_chain::REQUEST_CHAIN_EXPORT_DEFAULT_LIMIT)
        .clamp(1, crate::request_chain::REQUEST_CHAIN_EXPORT_MAX_LIMIT);
    crate::request_ledger::RequestLedger::new(proxy.state.runtime_store())
        .export_request_chain(proxy.service_name, selector, limit)
        .map(Json)
        .map_err(|error| {
            AdminApiHttpError::internal("admin_request_chain_export_failed", error.to_string())
        })
}

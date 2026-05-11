use std::time::Instant;

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use tracing::instrument;

use super::ProxyService;
use super::provider_execution::{
    ExecuteProviderChainParams, ProviderExecutionOutcome, execute_provider_chain, log_retry_options,
};
use super::request_context::prepare_proxy_request;
use super::request_failures::finish_failed_proxy_request;
use super::retry::retry_info_for_failed_attempts;

#[instrument(skip_all, fields(service = %proxy.service_name))]
pub async fn handle_proxy(
    proxy: ProxyService,
    req: Request<Body>,
) -> Result<Response<Body>, (StatusCode, String)> {
    let start = Instant::now();
    let started_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let prepared = prepare_proxy_request(&proxy, req, &start, started_at_ms).await?;
    log_retry_options(proxy.service_name, prepared.request_id, &prepared.plan);
    let provider_chain_params = ExecuteProviderChainParams {
        proxy: &proxy,
        lbs: &prepared.lbs,
        method: &prepared.method,
        uri: &prepared.uri,
        client_headers: &prepared.client_headers,
        client_headers_entries_cache: &prepared.client_headers_entries_cache,
        client_uri: prepared.client_uri.as_str(),
        start: &start,
        started_at_ms,
        request_id: prepared.request_id,
        request_body_len: prepared.request_body_len,
        body_for_upstream: &prepared.body_for_upstream,
        request_model: prepared.request_model.as_deref(),
        session_binding: prepared.session_binding.as_ref(),
        session_override_config: prepared.session_override_config.as_deref(),
        global_station_override: prepared.global_station_override.as_deref(),
        override_model: prepared.override_model.as_deref(),
        override_effort: prepared.override_effort.as_deref(),
        override_service_tier: prepared.override_service_tier.as_deref(),
        effective_effort: prepared.effective_effort.as_deref(),
        effective_service_tier: prepared.effective_service_tier.as_deref(),
        base_service_tier: &prepared.base_service_tier,
        session_id: prepared.session_id.as_deref(),
        cwd: prepared.cwd.as_deref(),
        request_flavor: &prepared.request_flavor,
        request_body_previews: prepared.request_body_previews,
        debug_max: prepared.debug_max,
        warn_max: prepared.warn_max,
        client_body_debug: prepared.client_body_debug.as_ref(),
        client_body_warn: prepared.client_body_warn.as_ref(),
        plan: &prepared.plan,
        cooldown_backoff: prepared.cooldown_backoff,
    };
    #[cfg(test)]
    let provider_execution = if super::provider_execution::route_executor_request_path_test_enabled(
        &prepared.client_headers,
    ) {
        super::provider_execution::execute_provider_chain_with_route_executor(provider_chain_params)
            .await
    } else {
        execute_provider_chain(provider_chain_params).await
    };
    #[cfg(not(test))]
    let provider_execution = execute_provider_chain(provider_chain_params).await;
    let (upstream_chain, route_attempts, last_err) = match provider_execution {
        ProviderExecutionOutcome::Return(response) => return Ok(response),
        ProviderExecutionOutcome::Exhausted(state) => {
            (state.upstream_chain, state.route_attempts, state.last_err)
        }
    };

    let dur = start.elapsed().as_millis() as u64;
    let retry = retry_info_for_failed_attempts(&upstream_chain, &route_attempts);
    let (status, msg) = last_err.unwrap_or_else(|| {
        (
            StatusCode::BAD_GATEWAY,
            "no upstreams available".to_string(),
        )
    });

    Err(
        finish_failed_proxy_request(super::request_failures::FailedProxyRequestParams {
            proxy: &proxy,
            method: &prepared.method,
            path: prepared.uri.path(),
            request_id: prepared.request_id,
            status,
            message: msg,
            duration_ms: dur,
            started_at_ms,
            session_id: prepared.session_id.clone(),
            cwd: prepared.cwd.clone(),
            effective_effort: prepared.effective_effort.clone(),
            service_tier: prepared.base_service_tier.clone(),
            retry,
            failure_route_attempts: route_attempts,
        })
        .await,
    )
}

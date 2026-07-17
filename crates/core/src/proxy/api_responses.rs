use anyhow::{Result, anyhow};

use crate::config::resolve_service_profile_from_catalog;
use crate::dashboard_core::window_stats::compute_window_stats;
use crate::dashboard_core::{
    ApiV1OperatorSummary, ControlProfileOption, OperatorActiveRequestSummary,
    OperatorProfileSummary, OperatorProviderBalanceSummary, OperatorProviderSummary,
    OperatorReadCapture, OperatorReadData, OperatorReadModel, OperatorRequestSummary,
    OperatorRetrySummary, OperatorRevisionBundle, OperatorRoutingControlView,
    OperatorRuntimeSummary, OperatorSessionSummary, OperatorSummaryCounts,
    build_operator_routing_summary, build_operator_session_stats,
    build_profile_options_from_route_view, redact_operator_pricing_catalog,
    redact_operator_quota_analytics, redact_operator_usage_day, redact_operator_usage_summaries,
    summarize_recent_retry_observations,
};
use crate::state::{
    OperatorLifecycleSnapshot, SessionIdentityCardBuildInputs,
    build_session_identity_cards_from_parts,
};

use super::ProxyService;
use super::profile_defaults::effective_default_profile_name;
use super::providers_api::build_provider_options_for_runtime_snapshot;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProfilesResponse {
    pub default_profile: Option<String>,
    pub configured_default_profile: Option<String>,
    pub profiles: Vec<ControlProfileOption>,
}

pub(super) async fn build_operator_read_capture(
    proxy: &ProxyService,
) -> Result<OperatorReadCapture> {
    const MAX_CAPTURE_ATTEMPTS: usize = 3;

    for _ in 0..MAX_CAPTURE_ATTEMPTS {
        let lifecycle_snapshot = proxy
            .state
            .capture_operator_lifecycle_snapshot(proxy.service_name, 200)
            .await;
        let state_revision = lifecycle_snapshot.state_revision;
        #[cfg(test)]
        proxy
            .state
            .pause_operator_aggregation_after_snapshot_for_test()
            .await;
        let capture = build_operator_read_model_once(proxy, lifecycle_snapshot).await?;
        if proxy.state.operator_revision() == state_revision {
            return Ok(capture);
        }
    }

    Err(anyhow!(
        "operator state changed during each coherent capture attempt"
    ))
}

async fn build_operator_read_model_once(
    proxy: &ProxyService,
    lifecycle_snapshot: OperatorLifecycleSnapshot,
) -> Result<OperatorReadCapture> {
    let runtime_snapshot = proxy.config.capture().await;
    let config = runtime_snapshot.config();
    let view =
        super::control_plane_service::service_route_config(config.as_ref(), proxy.service_name);
    let configured_default_profile = view.default_profile.clone();
    let configured_retry = config.retry.clone();
    let resolved_retry = configured_retry.resolve();
    let loaded_at_ms = runtime_snapshot.loaded_at_ms();
    let source_mtime_ms = runtime_snapshot.source_mtime_ms();
    let captured_at_ms = crate::logging::now_ms();
    let route_graph = runtime_snapshot
        .route_graph(proxy.service_name)
        .ok_or_else(|| {
            anyhow!(
                "runtime snapshot has no '{}' route graph",
                proxy.service_name
            )
        })?;
    let provider_catalog = runtime_snapshot.provider_catalog();
    let operator_pricing_catalog = runtime_snapshot.operator_pricing_catalog();
    let provider_policy = runtime_snapshot.provider_policy();
    let usage_summaries = lifecycle_snapshot.usage_summaries(100);
    let mut usage_day = lifecycle_snapshot.usage_day_view(12, captured_at_ms);
    let usage_rollup = lifecycle_snapshot.usage_rollup_view(12, 7);
    let ledger_revision = lifecycle_snapshot.ledger_revision.to_string();
    let active = lifecycle_snapshot.active_requests;
    let recent = lifecycle_snapshot.recent_finished;
    let (session_bindings, session_route_affinities, provider_balances, routing_control) = tokio::join!(
        proxy.state.list_session_bindings(),
        proxy.state.list_session_route_affinities(),
        proxy.state.get_provider_balance_view(proxy.service_name),
        proxy.state.capture_routing_operator_control(),
    );
    let default_profile = effective_default_profile_name(view);
    let session_stats = build_operator_session_stats(&recent);
    let policy_actions = proxy.state.policy_action_projections_for_snapshot(
        proxy.service_name,
        captured_at_ms,
        provider_policy.as_ref(),
    );
    usage_day.retry_gate =
        crate::state::UsageRetryGateSummary::from_policy_actions(&policy_actions);
    let session_cards = build_session_identity_cards_from_parts(SessionIdentityCardBuildInputs {
        active: &active,
        recent: &recent,
        bindings: &session_bindings,
        route_affinities: &session_route_affinities,
        stats: &session_stats,
    });
    let default_profile_summary = default_profile.as_deref().and_then(|profile_name| {
        resolve_service_profile_from_catalog(&view.profiles, profile_name)
            .ok()
            .map(|profile| OperatorProfileSummary {
                name: profile_name.to_string(),
                model: profile.model,
                reasoning_effort: profile.reasoning_effort,
                service_tier: profile.service_tier.clone(),
                fast_mode: profile.service_tier.as_deref() == Some("priority"),
            })
    });
    let profiles = build_profile_options_from_route_view(view, default_profile.as_deref());
    let providers = build_provider_options_for_runtime_snapshot(proxy, runtime_snapshot.as_ref())
        .await
        .map_err(|(status, message)| {
            anyhow!("build operator providers failed with {status}: {message}")
        })?;
    let route_template = route_graph.handshake_plan();
    let route_graph_key = route_graph.digest();
    let routing = build_operator_routing_summary(
        view,
        &route_template,
        OperatorRoutingControlView {
            route_graph_key,
            control_revision: routing_control.revision(),
            provider_policy_revision: provider_policy.policy_revision,
            new_session_preference: routing_control
                .new_session_preference(proxy.service_name, route_graph_key),
        },
    )?;
    let operator_providers = providers
        .iter()
        .map(OperatorProviderSummary::from)
        .collect::<Vec<_>>();
    let retry_observations = summarize_recent_retry_observations(&recent);
    let local_session_ids = session_cards
        .iter()
        .filter_map(|card| {
            card.session_id.as_ref().map(|session_id| {
                (
                    crate::dashboard_core::operator_summary::operator_session_key(session_id),
                    session_id.clone(),
                )
            })
        })
        .collect();
    let sessions = session_cards
        .iter()
        .enumerate()
        .map(|(index, card)| OperatorSessionSummary::from_session_card(card, index))
        .collect::<Vec<_>>();
    let summary = ApiV1OperatorSummary {
        api_version: 1,
        service_name: proxy.service_name.to_string(),
        runtime: OperatorRuntimeSummary {
            runtime_loaded_at_ms: Some(loaded_at_ms),
            runtime_source_mtime_ms: source_mtime_ms,
            configured_default_profile,
            default_profile,
            default_profile_summary,
            operator_actions: crate::dashboard_core::OperatorActionCapabilities {
                refresh_provider_balances: true,
                mutate_routing: true,
                mutate_session_affinity: true,
            },
        },
        counts: OperatorSummaryCounts {
            active_requests: active.len(),
            recent_requests: recent.len(),
            sessions: sessions.len(),
            profiles: view.profiles.len(),
            providers: providers.len(),
        },
        retry: OperatorRetrySummary {
            configured_profile: configured_retry.profile,
            upstream_max_attempts: resolved_retry.upstream.max_attempts,
            provider_max_attempts: resolved_retry.route.max_attempts,
            recent_retried_requests: retry_observations.recent_retried_requests,
            recent_cross_provider_failovers: retry_observations.recent_cross_provider_failovers,
            recent_same_provider_retries: retry_observations.recent_same_provider_retries,
            recent_fast_mode_requests: retry_observations.recent_fast_mode_requests,
        },
        sessions,
        profiles,
        providers: operator_providers,
    };
    let stats_5m = compute_window_stats(&recent, captured_at_ms, 5 * 60_000, |_| true);
    let stats_1h = compute_window_stats(&recent, captured_at_ms, 60 * 60_000, |_| true);
    let mut operator_balances = provider_balances
        .iter()
        .map(OperatorProviderBalanceSummary::from)
        .collect::<Vec<_>>();
    operator_balances.sort_by(|left, right| {
        left.provider_id
            .cmp(&right.provider_id)
            .then_with(|| left.endpoint_id.cmp(&right.endpoint_id))
            .then_with(|| {
                left.observation_provider_id
                    .cmp(&right.observation_provider_id)
            })
            .then_with(|| left.provider_endpoint_key.cmp(&right.provider_endpoint_key))
    });
    let revisions = OperatorRevisionBundle {
        runtime_revision: runtime_snapshot.revision(),
        runtime_digest: runtime_snapshot.digest().to_string(),
        route_digest: route_graph.digest().to_string(),
        catalog_revision: provider_catalog.catalog_revision().as_str().to_string(),
        pricing_revision: provider_catalog.pricing_revision().as_str().to_string(),
        operator_pricing_revision: operator_pricing_catalog.revision().to_string(),
        policy_revision: provider_policy.policy_revision,
        ledger_revision,
    };
    let model = OperatorReadModel::ready(
        proxy.service_name,
        captured_at_ms,
        revisions,
        OperatorReadData {
            summary,
            routing: Some(routing),
            active_requests: active
                .iter()
                .map(OperatorActiveRequestSummary::from_active_request)
                .collect(),
            recent_requests: recent
                .iter()
                .map(OperatorRequestSummary::from_finished_request)
                .collect(),
            usage_summaries: redact_operator_usage_summaries(usage_summaries),
            usage_day: redact_operator_usage_day(usage_day),
            usage_rollup,
            stats_5m,
            stats_1h,
            pricing_catalog: redact_operator_pricing_catalog(operator_pricing_catalog.snapshot()),
            quota_analytics: redact_operator_quota_analytics(
                proxy
                    .state
                    .quota_analytics_view(proxy.service_name, captured_at_ms)
                    .await,
            ),
            provider_balances: operator_balances,
        },
    );
    model
        .validate()
        .map_err(|message| anyhow!("invalid operator read model: {message}"))?;
    Ok(OperatorReadCapture {
        model,
        local_session_ids,
    })
}

pub(super) async fn make_profiles_response(proxy: &ProxyService) -> ProfilesResponse {
    let config = proxy.config.capture().await.config();
    let view =
        super::control_plane_service::service_route_config(config.as_ref(), proxy.service_name);
    let default_profile = effective_default_profile_name(view);
    ProfilesResponse {
        default_profile: default_profile.clone(),
        configured_default_profile: view.default_profile.clone(),
        profiles: build_profile_options_from_route_view(view, default_profile.as_deref()),
    }
}

use super::*;
use crate::dashboard_core::{OperatorReadModel, OperatorReadStatus};
use crate::proxy::tests::harness::{proxy_service, spawn_proxy_service};
use crate::runtime_identity::ProviderEndpointKey;
use crate::runtime_store::ProviderManualEligibility;
use crate::state::FinishRequestParams;

fn route_provider(base_url: &str) -> ProviderConfig {
    ProviderConfig {
        base_url: Some(base_url.to_string()),
        inline_auth: UpstreamAuth::default(),
        ..ProviderConfig::default()
    }
}

fn operator_provider_config(route_order: &[&str], unused_provider: Option<&str>) -> HelperConfig {
    let mut providers = route_order
        .iter()
        .enumerate()
        .map(|(index, provider)| {
            (
                (*provider).to_string(),
                route_provider(format!("https://{index}.example.test/v1").as_str()),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    if let Some(provider) = unused_provider {
        providers.insert(
            provider.to_string(),
            route_provider("https://unused.example.test/v1"),
        );
    }

    HelperConfig {
        codex: ServiceRouteConfig {
            providers,
            routing: Some(RouteGraphConfig::ordered_failover(
                route_order
                    .iter()
                    .map(|provider| (*provider).to_string())
                    .collect(),
            )),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    }
}

async fn begin_request(
    proxy: &ProxyService,
    service: &'static str,
    session_id: &'static str,
    started_at_ms: u64,
) -> u64 {
    proxy
        .state
        .begin_request(
            service,
            "POST",
            "/v1/responses",
            Some(session_id.to_string()),
            None,
            None,
            None,
            None,
            Some("gpt-test".to_string()),
            None,
            None,
            started_at_ms,
        )
        .await
}

async fn finish_request(proxy: &ProxyService, id: u64, ended_at_ms: u64) {
    assert!(
        proxy
            .state
            .finish_request(FinishRequestParams {
                id,
                winning_attempt: None,
                status_code: 200,
                duration_ms: 10,
                ended_at_ms,
                observed_service_tier: None,
                reported_model: Some("gpt-test".to_string()),
                usage: None,
                retry: None,
                ttfb_ms: Some(2),
                streaming: false,
            })
            .await
    );
}

#[tokio::test]
async fn operator_read_model_isolates_requests_and_sessions_by_service() {
    let proxy = proxy_service(make_helper_config(Vec::new(), RetryConfig::default()));
    let codex_active = begin_request(&proxy, "codex", "codex-active-session", 10).await;
    let claude_active = begin_request(&proxy, "claude", "claude-active-secret", 20).await;
    let codex_finished = begin_request(&proxy, "codex", "codex-finished-session", 30).await;
    let claude_finished = begin_request(&proxy, "claude", "claude-finished-secret", 40).await;
    finish_request(&proxy, codex_finished, 31).await;
    finish_request(&proxy, claude_finished, 41).await;

    let server = spawn_proxy_service(proxy);
    let model = reqwest::Client::new()
        .get(server.url("/__codex_helper/api/v1/operator/read-model"))
        .send()
        .await
        .expect("request operator read model")
        .error_for_status()
        .expect("operator read-model status")
        .json::<OperatorReadModel>()
        .await
        .expect("decode operator read model");

    assert_eq!(model.status, OperatorReadStatus::Ready);
    assert!(model.validate().is_ok());
    let data = model.data.as_ref().expect("ready operator data");
    assert_eq!(data.active_requests.len(), 1);
    assert_eq!(data.active_requests[0].id, codex_active);
    assert_eq!(data.recent_requests.len(), 1);
    assert_eq!(data.recent_requests[0].id, codex_finished);
    assert_eq!(data.summary.counts.active_requests, 1);
    assert_eq!(data.summary.counts.recent_requests, 1);
    assert_eq!(data.summary.counts.sessions, 2);

    let serialized = serde_json::to_string(&model).expect("serialize operator read model");
    assert!(!serialized.contains("claude-active-secret"));
    assert!(!serialized.contains("claude-finished-secret"));
    assert!(!serialized.contains("codex-active-session"));
    assert!(!serialized.contains("codex-finished-session"));
    for removed_field in [
        "\"links\"",
        "\"surface_capabilities\"",
        "\"shared_capabilities\"",
        "\"host_local_capabilities\"",
        "\"remote_admin_access\"",
        "\"configured_active_station\"",
        "\"effective_active_station\"",
        "\"global_station_override\"",
        "\"global_route_target_override\"",
        "\"override_effort\"",
        "\"override_model\"",
        "\"override_route_target\"",
        "\"override_service_tier\"",
        "\"override_station_name\"",
    ] {
        assert!(
            !serialized.contains(removed_field),
            "operator read model retained compatibility field {removed_field}"
        );
    }

    assert_ne!(codex_active, claude_active);
}

#[tokio::test]
async fn local_operator_capture_keeps_raw_session_ids_out_of_the_wire_model() {
    let proxy = proxy_service(make_helper_config(Vec::new(), RetryConfig::default()));
    let raw_session_id = "local-session-command-handle";
    let _request_id = begin_request(&proxy, "codex", raw_session_id, 10).await;

    let capture = proxy
        .operator_read_capture()
        .await
        .expect("capture local operator model");
    let data = capture.model.data.as_ref().expect("ready operator data");
    let session_key = &data.summary.sessions[0].session_key;

    assert_eq!(
        capture
            .local_session_ids
            .get(session_key)
            .map(String::as_str),
        Some(raw_session_id)
    );
    let serialized = serde_json::to_string(&capture.model).expect("serialize wire model");
    assert!(!serialized.contains(raw_session_id));
}

#[tokio::test]
async fn post_snapshot_operator_aggregation_does_not_delay_terminal_publication() {
    let proxy = proxy_service(make_helper_config(Vec::new(), RetryConfig::default()));
    let request_id = begin_request(&proxy, "codex", "operator-aggregation-session", 10).await;
    let (snapshot_captured, resume_aggregation) = proxy
        .state
        .pause_next_operator_aggregation_after_snapshot_for_test()
        .await;

    let capture_proxy = proxy.clone();
    let capture = tokio::spawn(async move { capture_proxy.operator_read_capture().await });
    snapshot_captured
        .await
        .expect("operator lifecycle snapshot must be captured");

    tokio::time::timeout(
        std::time::Duration::from_millis(250),
        finish_request(&proxy, request_id, 20),
    )
    .await
    .expect("terminal publication must not wait for post-snapshot aggregation");
    resume_aggregation
        .send(())
        .expect("resume operator aggregation");

    let capture = capture
        .await
        .expect("join operator capture")
        .expect("capture operator read model");
    let data = capture.model.data.expect("ready operator data");
    assert!(
        data.active_requests
            .iter()
            .all(|request| request.id != request_id)
    );
    assert!(
        data.recent_requests
            .iter()
            .any(|request| request.id == request_id)
    );
}

#[tokio::test]
async fn operator_provider_projection_uses_compiled_candidate_order_and_route_membership() {
    let proxy = proxy_service(operator_provider_config(
        &["z-preferred", "a-fallback"],
        Some("m-unused"),
    ));

    let capture = proxy
        .operator_read_capture()
        .await
        .expect("capture operator read model");
    let providers = &capture
        .model
        .data
        .as_ref()
        .expect("ready operator data")
        .summary
        .providers;

    assert_eq!(
        providers
            .iter()
            .map(|provider| provider.name.as_str())
            .collect::<Vec<_>>(),
        vec!["z-preferred", "a-fallback", "m-unused"]
    );
    let unused = providers
        .iter()
        .find(|provider| provider.name == "m-unused")
        .expect("configured but unreferenced provider remains inspectable");
    assert!(unused.configured_enabled);
    assert!(!unused.effective_enabled);
    assert_eq!(unused.routable_endpoints, 0);
    assert!(unused.endpoints.iter().all(|endpoint| !endpoint.routable));
}

#[tokio::test]
async fn operator_provider_projection_matches_captured_manual_and_automatic_policy() {
    let proxy = proxy_service(operator_provider_config(&["manual", "automatic"], None));
    proxy
        .state
        .set_provider_manual_eligibility(
            ProviderEndpointKey::new("codex", "manual", "default"),
            ProviderManualEligibility::Disabled,
            Some("operator disabled endpoint".to_string()),
            1,
        )
        .await
        .expect("commit manual eligibility");
    proxy
        .set_provider_automatic_block_for_test(
            ProviderEndpointKey::new("codex", "automatic", "default"),
            true,
            2,
        )
        .await;
    proxy
        .config
        .publish_provider_policy(proxy.state.capture_provider_policy_snapshot().await)
        .await
        .expect("publish captured provider policy");

    let capture = proxy
        .operator_read_capture()
        .await
        .expect("capture operator read model");
    let providers = &capture
        .model
        .data
        .as_ref()
        .expect("ready operator data")
        .summary
        .providers;
    let manual = providers
        .iter()
        .find(|provider| provider.name == "manual")
        .expect("manual provider");
    let automatic = providers
        .iter()
        .find(|provider| provider.name == "automatic")
        .expect("automatic provider");

    assert!(!manual.effective_enabled);
    assert_eq!(manual.routable_endpoints, 0);
    assert!(manual.endpoints.iter().all(|endpoint| !endpoint.routable));
    assert!(automatic.effective_enabled);
    assert_eq!(automatic.routable_endpoints, 0);
    assert!(
        automatic
            .endpoints
            .iter()
            .all(|endpoint| !endpoint.routable)
    );
    assert_eq!(
        automatic.endpoints[0].policy_actions[0].code,
        "balance_exhausted"
    );
}

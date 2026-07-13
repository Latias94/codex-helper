use super::*;
use crate::dashboard_core::OperatorReadStatus;
use crate::proxy::tests::harness::{BeginRequestTestBuilder, proxy_service};
use crate::request_chain::{REQUEST_CHAIN_EXPORT_DEFAULT_LIMIT, RequestChainSelector};
use crate::request_ledger::{RequestLedger, RequestLogFilters, format_finished_request_lines};
use crate::runtime_store::{CommittedRequestQuery, RuntimeStore, RuntimeStoreReader};
use crate::state::FinishRequestParams;
use crate::usage::UsageMetrics;

fn price_reload_terminal(id: u64, ended_at_ms: u64) -> FinishRequestParams {
    FinishRequestParams {
        id,
        winning_attempt: None,
        status_code: 200,
        duration_ms: 10,
        ended_at_ms,
        observed_service_tier: None,
        reported_model: None,
        usage: Some(UsageMetrics {
            input_tokens: 1_000_000,
            total_tokens: 1_000_000,
            ..UsageMetrics::default()
        }),
        retry: None,
        ttfb_ms: Some(4),
        streaming: false,
    }
}

async fn finish_chain_request(state: &crate::state::ProxyState, id: u64, ended_at_ms: u64) {
    assert!(
        state
            .finish_request(FinishRequestParams {
                id,
                winning_attempt: None,
                status_code: 200,
                duration_ms: 10,
                ended_at_ms,
                observed_service_tier: None,
                reported_model: None,
                usage: None,
                retry: None,
                ttfb_ms: None,
                streaming: false,
            })
            .await
    );
}

async fn request_chain(
    app: &axum::Router,
    query: &str,
) -> crate::request_chain::RequestChainExport {
    let mut request = Request::builder()
        .uri(format!(
            "/__codex_helper/api/v1/request-ledger/chain?{query}"
        ))
        .body(Body::empty())
        .expect("build request-chain query");
    request
        .extensions_mut()
        .insert(ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            42_111,
        ))));
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("request-chain response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 256 * 1024)
        .await
        .expect("read request-chain response");
    serde_json::from_slice(&body).expect("decode request-chain response")
}

#[tokio::test]
async fn request_chain_uses_service_scoped_exact_and_identity_selectors() {
    let proxy = proxy_service(make_helper_config(Vec::new(), RetryConfig::default()));
    let state = proxy.state.clone();

    let codex_exact = BeginRequestTestBuilder::new(&state)
        .session_id("sid-1")
        .started_at_ms(10)
        .begin()
        .await;
    finish_chain_request(&state, codex_exact, 20).await;
    let codex_prefix_decoy = BeginRequestTestBuilder::new(&state)
        .session_id("sid-10")
        .started_at_ms(30)
        .begin()
        .await;
    finish_chain_request(&state, codex_prefix_decoy, 40).await;
    let claude_service_decoy = BeginRequestTestBuilder::new(&state)
        .service("claude")
        .session_id("sid-1")
        .started_at_ms(50)
        .begin()
        .await;
    finish_chain_request(&state, claude_service_decoy, 60).await;

    let app = crate::proxy::router(proxy);
    let exact_session = request_chain(&app, "session_id=sid-1").await;
    assert_eq!(
        exact_session
            .requests
            .iter()
            .map(|request| (request.service.as_str(), request.request_id))
            .collect::<Vec<_>>(),
        vec![("codex", codex_exact)]
    );

    let mismatched_and = request_chain(
        &app,
        &format!(
            "session_id=sid-1&request_id={codex_prefix_decoy}&trace_id=codex-{codex_prefix_decoy}"
        ),
    )
    .await;
    assert!(mismatched_and.requests.is_empty());
}

#[tokio::test]
async fn operator_read_model_and_request_chain_project_committed_runtime_store_terminals() {
    let proxy = proxy_service(make_helper_config(Vec::new(), RetryConfig::default()));
    let state = proxy.state.clone();
    let request_id = BeginRequestTestBuilder::new(&state)
        .session_id("sid-sqlite-ledger")
        .model("gpt-5")
        .started_at_ms(100)
        .begin()
        .await;
    assert!(
        state
            .finish_request(FinishRequestParams {
                id: request_id,
                winning_attempt: None,
                status_code: 200,
                duration_ms: 25,
                ended_at_ms: 125,
                observed_service_tier: None,
                reported_model: Some("gpt-5".to_string()),
                usage: Some(UsageMetrics {
                    input_tokens: 1_000,
                    output_tokens: 100,
                    total_tokens: 1_100,
                    ..UsageMetrics::default()
                }),
                retry: None,
                ttfb_ms: Some(5),
                streaming: false,
            })
            .await
    );

    let model = proxy
        .operator_read_model()
        .await
        .expect("build operator read model");
    assert_eq!(model.status, OperatorReadStatus::Ready);
    assert!(model.validate().is_ok());
    let data = model.data.as_ref().expect("ready operator data");
    let recent = &data.recent_requests;
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].id, request_id);
    let expected_session_key =
        crate::dashboard_core::operator_summary::operator_session_key("sid-sqlite-ledger");
    assert_eq!(
        recent[0].session_key.as_deref(),
        Some(expected_session_key.as_str())
    );
    assert_eq!(recent[0].cost.total_cost_usd.as_deref(), Some("0.00225"));

    let summary = data
        .usage_summaries
        .iter()
        .find(|summary| summary.group == crate::request_ledger::RequestUsageSummaryGroup::Provider)
        .expect("provider usage summary");
    assert_eq!(summary.rows.len(), 1);
    assert_eq!(summary.rows[0].group_value, "-");
    assert_eq!(summary.rows[0].aggregate.requests, 1);
    assert_eq!(summary.rows[0].aggregate.total_tokens, 1_100);

    let chain = RequestLedger::new(state.runtime_store())
        .export_request_chain(
            "codex",
            RequestChainSelector {
                session_id: Some("sid-sqlite-ledger".to_string()),
                ..RequestChainSelector::default()
            },
            REQUEST_CHAIN_EXPORT_DEFAULT_LIMIT,
        )
        .expect("export committed request chain");
    assert_eq!(chain.requests.len(), 1);
    assert_eq!(chain.requests[0].request_id, request_id);
    assert_eq!(
        chain.requests[0].cost.total_cost_usd.as_deref(),
        Some("0.00225")
    );
}

#[tokio::test]
async fn runtime_price_reload_preserves_captured_cost_in_operator_read_model() {
    let _env_lock = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }
    let pricing_path = temp_dir.join("pricing_overrides.toml");
    std::fs::write(
        &pricing_path,
        r#"[models.catalog-race]
input_per_1m_usd = "1"
output_per_1m_usd = "2"
confidence = "exact"
"#,
    )
    .expect("write price A");

    let route_config = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([(
                "price-provider".to_string(),
                ProviderConfig {
                    base_url: Some("http://127.0.0.1:9/v1".to_string()),
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "price-provider".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    crate::config::save_helper_config(&route_config)
        .await
        .expect("save runtime config");
    let loaded = crate::config::load_config_with_source()
        .await
        .expect("load runtime config");
    let runtime_store = Arc::new(
        RuntimeStore::open_in_home(&temp_dir).expect("open persistent runtime store for proxy"),
    );
    let proxy = ProxyService::new_with_runtime_store(
        Client::new(),
        Arc::new(loaded.source),
        "codex",
        runtime_store,
    )
    .expect("build proxy with persistent runtime store");
    let state = proxy.state.clone();
    let snapshot_a = proxy.config.capture().await;
    let revision_a = snapshot_a.revision();
    let digest_a = snapshot_a.digest().to_string();
    let request_a = state
        .try_begin_request(
            "codex",
            "POST",
            "/v1/responses",
            Some("sid-price-a".to_string()),
            None,
            None,
            None,
            None,
            Some("catalog-race".to_string()),
            Some("catalog-race".to_string()),
            None,
            None,
            None,
            snapshot_a.provider_catalog(),
            snapshot_a.operator_pricing_catalog(),
            revision_a,
            digest_a.clone(),
            0,
            100,
        )
        .await
        .expect("begin request with price A snapshot");

    let (completion_ready_tx, completion_ready_rx) = tokio::sync::oneshot::channel();
    let (completion_release_tx, completion_release_rx) = tokio::sync::oneshot::channel();
    let state_for_completion = state.clone();
    let completion = tokio::spawn(async move {
        completion_ready_tx
            .send(())
            .expect("signal old request completion task ready");
        completion_release_rx
            .await
            .expect("release old request completion after reload");
        state_for_completion
            .finish_request(price_reload_terminal(request_a, 110))
            .await
    });
    completion_ready_rx
        .await
        .expect("old request completion task should be waiting");

    std::fs::write(
        &pricing_path,
        r#"[models.catalog-race]
input_per_1m_usd = "9"
output_per_1m_usd = "18"
confidence = "exact"
"#,
    )
    .expect("write price B");
    assert!(
        proxy
            .config
            .force_reload_from_disk()
            .await
            .expect("reload runtime pricing snapshot")
    );
    let snapshot_b = proxy.config.capture().await;
    let revision_b = snapshot_b.revision();
    let digest_b = snapshot_b.digest().to_string();
    assert_eq!(revision_b, revision_a + 1);
    assert_ne!(
        snapshot_b.operator_pricing_catalog().revision(),
        snapshot_a.operator_pricing_catalog().revision()
    );
    let request_b = state
        .try_begin_request(
            "codex",
            "POST",
            "/v1/responses",
            Some("sid-price-b".to_string()),
            None,
            None,
            None,
            None,
            Some("catalog-race".to_string()),
            Some("catalog-race".to_string()),
            None,
            None,
            None,
            snapshot_b.provider_catalog(),
            snapshot_b.operator_pricing_catalog(),
            revision_b,
            digest_b.clone(),
            0,
            101,
        )
        .await
        .expect("begin request with price B snapshot");
    assert!(
        state
            .list_active_requests()
            .await
            .iter()
            .any(|request| request.id == request_a),
        "price A request must remain active across the price B reload"
    );
    completion_release_tx
        .send(())
        .expect("release old request completion");
    assert!(
        completion.await.expect("join old request completion task"),
        "old request terminal should commit after the reload"
    );
    assert!(
        state
            .finish_request(price_reload_terminal(request_b, 111))
            .await
    );

    drop(snapshot_a);
    drop(snapshot_b);
    drop(state);
    drop(proxy);

    let reader = RuntimeStoreReader::open_in_home(&temp_dir).expect("reopen runtime store reader");
    let projections = reader
        .query_committed_requests(&CommittedRequestQuery {
            limit: 10,
            ..CommittedRequestQuery::default()
        })
        .expect("read frozen terminal projections after reopen");
    let projection_for_session = |session: &str| {
        projections
            .items
            .iter()
            .find(|projection| {
                projection.payload.finished_request.session_id.as_deref() == Some(session)
            })
            .expect("frozen request projection for session")
    };
    let projection_a = projection_for_session("sid-price-a");
    assert_eq!(projection_a.payload.runtime_revision, revision_a);
    assert_eq!(projection_a.payload.runtime_digest, digest_a);
    let projection_b = projection_for_session("sid-price-b");
    assert_eq!(projection_b.payload.runtime_revision, revision_b);
    assert_eq!(projection_b.payload.runtime_digest, digest_b);

    {
        let ledger = RequestLedger::new(&reader);
        let read_session = |session: &str| {
            ledger
                .find_finished_requests(
                    &RequestLogFilters {
                        session: Some(session.to_string()),
                        ..RequestLogFilters::default()
                    },
                    10,
                )
                .expect("read frozen terminal through typed ledger")
        };
        let request_a_after_reopen = read_session("sid-price-a")
            .pop()
            .expect("price A request after reopen");
        let request_b_after_reopen = read_session("sid-price-b")
            .pop()
            .expect("price B request after reopen");
        assert_eq!(
            request_a_after_reopen.cost.total_cost_usd.as_deref(),
            Some("1")
        );
        assert_eq!(
            request_b_after_reopen.cost.total_cost_usd.as_deref(),
            Some("9")
        );
        assert!(
            format_finished_request_lines(&request_a_after_reopen)
                .iter()
                .any(|line| line.contains("cost=$1 (exact)")),
            "operator formatter must use the frozen price A cost"
        );
    }
    drop(reader);

    let loaded = crate::config::load_config_with_source()
        .await
        .expect("reload runtime config after restart");
    let reopened_store = Arc::new(
        RuntimeStore::open_in_home(&temp_dir).expect("reopen persistent runtime store for admin"),
    );
    let reopened_proxy = ProxyService::new_with_runtime_store(
        Client::new(),
        Arc::new(loaded.source),
        "codex",
        reopened_store,
    )
    .expect("restart proxy with committed request store");
    let model = reopened_proxy
        .operator_read_model()
        .await
        .expect("build price reload operator model");
    assert_eq!(model.status, OperatorReadStatus::Ready);
    assert!(model.validate().is_ok());
    let recent = &model
        .data
        .as_ref()
        .expect("ready price reload operator data")
        .recent_requests;
    assert_eq!(recent.len(), 2);
    for (request_id, expected_cost) in [(request_a, "1"), (request_b, "9")] {
        let request = recent
            .iter()
            .find(|request| request.id == request_id)
            .expect("price reload request in operator model");
        assert_eq!(request.cost.total_cost_usd.as_deref(), Some(expected_cost));
    }

    drop(scoped);
    let _ = std::fs::remove_dir_all(temp_dir);
}

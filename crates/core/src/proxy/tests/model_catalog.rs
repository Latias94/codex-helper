use super::harness::{spawn_proxy_service, spawn_test_proxy, spawn_test_upstream};
use super::*;
use axum::http::header;

#[tokio::test]
async fn model_catalog_unions_upstreams_by_default_and_routes_new_models() {
    let hits = Arc::new(AtomicUsize::new(0));
    let first_hits = hits.clone();
    let first = spawn_test_upstream(
        axum::Router::new()
            .route(
                "/v1/models",
                get(move |uri: axum::http::Uri| {
                    first_hits.fetch_add(1, Ordering::SeqCst);
                    async move {
                        assert!(
                            uri.query()
                                .is_some_and(|q| q.contains("client_version=0.162.1"))
                        );
                        Json(serde_json::json!({"data": [{"id": "gpt-6-sol"}]}))
                    }
                }),
            )
            .route(
                "/v1/responses",
                post(|| async { Json(serde_json::json!({"provider": "first"})) }),
            ),
    );
    let second_hits = hits.clone();
    let second = spawn_test_upstream(axum::Router::new()
        .route("/v1/models", get(move || {
            second_hits.fetch_add(1, Ordering::SeqCst);
            async { Json(serde_json::json!({"models": [
                {"slug": "gpt-6.1-sol", "display_name": "Relay Sol", "supported_reasoning_levels": [{"effort": "high", "description": "High"}]},
                {"slug": "relay-new-model", "display_name": "Relay New", "context_window": 123456,
                 "default_reasoning_level": "medium", "supported_reasoning_levels": [{"effort": "medium", "description": "Medium"}]}
            ]})) }
        }))
        .route("/v1/responses", post(|| async { Json(serde_json::json!({"provider": "second"})) })));
    let proxy = spawn_test_proxy(make_proxy_config(
        vec![first.upstream_config(), second.upstream_config()],
        RetryConfig::default(),
    ));
    let client = Client::new();
    for _ in 0..2 {
        let response = client
            .get(proxy.url("/models?client_version=0.162.1"))
            .send()
            .await
            .expect("catalog");
        assert_eq!(response.status(), StatusCode::OK);
        let value: serde_json::Value = response.json().await.expect("catalog JSON");
        let models = value["models"].as_array().expect("native models");
        assert_eq!(models.len(), 3);
        let sol = models
            .iter()
            .find(|m| m["slug"] == "gpt-6.1-sol")
            .expect("Sol");
        assert_eq!(sol["display_name"], "GPT-6.1-Sol");
        assert_eq!(
            sol["supported_reasoning_levels"]
                .as_array()
                .expect("efforts")
                .len(),
            6
        );
        let new = models
            .iter()
            .find(|m| m["slug"] == "relay-new-model")
            .expect("new model");
        assert_eq!(new["context_window"], 123456);
    }
    assert_eq!(
        hits.load(Ordering::SeqCst),
        2,
        "cached queries must not refetch each provider"
    );
    let response = client
        .post(proxy.responses_url())
        .json(&serde_json::json!({"model": "relay-new-model", "input": []}))
        .send()
        .await
        .expect("inference");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await.expect("JSON")["provider"],
        "second"
    );
}

#[tokio::test]
async fn model_catalog_routes_explicit_reasoning_limits_without_penalizing_sources() {
    let first = spawn_test_upstream(axum::Router::new()
        .route("/v1/models", get(|| async { Json(serde_json::json!({"models": [{"slug": "gpt-6.1-sol", "supported_reasoning_levels": [{"effort": "high", "description": "High"}]}]})) }))
        .route("/v1/responses", post(|| async { Json(serde_json::json!({"provider": "first"})) })));
    let second = spawn_test_upstream(axum::Router::new()
        .route("/v1/models", get(|| async { Json(serde_json::json!({"models": [{"slug": "gpt-6.1-sol", "supported_reasoning_levels": [{"effort": "ultra", "description": "Ultra"}]}]})) }))
        .route("/v1/responses", post(|| async { Json(serde_json::json!({"provider": "second"})) })));
    let proxy = spawn_test_proxy(make_proxy_config(
        vec![first.upstream_config(), second.upstream_config()],
        RetryConfig::default(),
    ));
    let client = Client::new();
    let catalog = client
        .get(proxy.url("/models?client_version=0.162.1"))
        .send()
        .await
        .expect("catalog");
    assert_eq!(catalog.status(), StatusCode::OK);
    let response = client.post(proxy.responses_url()).json(&serde_json::json!({"model": "gpt-6.1-sol", "reasoning": {"effort": "ultra"}, "input": []})).send().await.expect("inference");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await.expect("JSON")["provider"],
        "second"
    );
    let response = client.post(proxy.responses_url())
        .json(&serde_json::json!({"model": "gpt-6.1-sol", "reasoning": {"effort": "high"}, "input": []}))
        .send().await.expect("next inference");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await.expect("JSON")["provider"],
        "first"
    );
}

#[tokio::test]
async fn model_catalog_supports_root_and_non_v1_bases_and_versioned_probes() {
    for base_path in ["", "/v1", "/backend-api/codex", "/custom/api"] {
        let expected = format!("{base_path}/models");
        let upstream = spawn_test_upstream(axum::Router::new().route(
            &expected,
            get(|uri: axum::http::Uri| async move {
                assert!(uri.query().is_some_and(|q| q.contains("client_version=")));
                Json(serde_json::json!({"models": [{"slug": "gpt-6.1-sol"}]}))
            }),
        ));
        let mut target = upstream.upstream_config();
        target.base_url = format!("http://{}{base_path}", upstream.addr);
        let proxy = spawn_test_proxy(make_proxy_config(
            vec![target.clone()],
            RetryConfig::default(),
        ));
        for path in ["/models", "/v1/models"] {
            let response = Client::new()
                .get(proxy.url(&format!("{path}?client_version=0.158.0")))
                .send()
                .await
                .expect("catalog");
            assert_eq!(response.status(), StatusCode::OK);
            let value: serde_json::Value = response.json().await.expect("JSON");
            assert_eq!(value["models"][0]["display_name"], "GPT-6.1-Sol");
        }
        let probe = super::super::CodexRelayProbeClient::new(Client::new());
        let result = probe
            .probe_upstream(
                &target,
                &super::super::CodexRelayProbeSpec {
                    kind: super::super::CodexRelayProbeKind::Models,
                    method: "GET".to_string(),
                    path: "/models".to_string(),
                    side_effect: super::super::CodexRelayProbeSideEffect::ReadOnly,
                    body: None,
                },
            )
            .await;
        assert_eq!(result.response_shape.as_deref(), Some("codex_models"));
        assert!(!result.translation_required);
    }
}

#[tokio::test]
async fn model_catalog_observations_are_isolated_by_client_version_and_query() {
    let first = spawn_test_upstream(
        axum::Router::new()
            .route(
                "/v1/models",
                get(|uri: axum::http::Uri| async move {
                    let id = if uri
                        .query()
                        .is_some_and(|q| q.contains("0.158.0") && !q.contains("route=beta"))
                    {
                        "old-only"
                    } else {
                        "new-only"
                    };
                    Json(serde_json::json!({"data": [{"id": id}]}))
                }),
            )
            .route(
                "/v1/responses",
                post(|| async { Json(serde_json::json!({"provider": "first"})) }),
            ),
    );
    let second = spawn_test_upstream(
        axum::Router::new()
            .route(
                "/v1/models",
                get(|| async { Json(serde_json::json!({"data": [{"id": "old-only"}]})) }),
            )
            .route(
                "/v1/responses",
                post(|| async { Json(serde_json::json!({"provider": "second"})) }),
            ),
    );
    let proxy = spawn_test_proxy(make_proxy_config(
        vec![first.upstream_config(), second.upstream_config()],
        RetryConfig::default(),
    ));
    let client = Client::new();
    for version in ["0.158.0", "0.162.1"] {
        assert_eq!(
            client
                .get(proxy.url(&format!("/models?client_version={version}")))
                .send()
                .await
                .expect("catalog")
                .status(),
            StatusCode::OK
        );
    }
    let response = client
        .post(proxy.responses_url())
        .header(header::USER_AGENT, "codex_cli_rs/0.158.0")
        .json(&serde_json::json!({"model": "old-only", "input": []}))
        .send()
        .await
        .expect("inference");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await.expect("JSON")["provider"],
        "first"
    );
    for route in ["alpha", "beta"] {
        assert_eq!(
            client
                .get(proxy.url(&format!("/models?client_version=0.158.0&route={route}")))
                .send()
                .await
                .expect("catalog")
                .status(),
            StatusCode::OK
        );
    }
    let response = client
        .post(proxy.url("/v1/responses?route=alpha"))
        .header(header::USER_AGENT, "codex_cli_rs/0.158.0")
        .json(&serde_json::json!({"model": "old-only", "input": []}))
        .send()
        .await
        .expect("inference with query");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await.expect("JSON")["provider"],
        "first"
    );
}

#[tokio::test]
async fn model_catalog_does_not_restrict_non_responses_requests() {
    let upstream = spawn_test_upstream(
        axum::Router::new()
            .route(
                "/v1/models",
                get(|| async { Json(serde_json::json!({"data": [{"id": "gpt-6.1-sol"}]})) }),
            )
            .route(
                "/v1/embeddings",
                post(|| async { Json(serde_json::json!({"data": []})) }),
            ),
    );
    let proxy = spawn_test_proxy(make_proxy_config(
        vec![upstream.upstream_config()],
        RetryConfig::default(),
    ));
    let client = Client::new();
    assert_eq!(
        client
            .get(proxy.url("/models"))
            .send()
            .await
            .expect("catalog")
            .status(),
        StatusCode::OK
    );
    let response = client
        .post(proxy.url("/v1/embeddings"))
        .json(&serde_json::json!({"model": "text-embedding-3-small", "input": "fixture"}))
        .send()
        .await
        .expect("embeddings");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn model_catalog_only_advertises_models_reachable_through_conditional_routes() {
    let first = spawn_test_upstream(
        axum::Router::new()
            .route(
                "/v1/models",
                get(|| async {
                    Json(serde_json::json!({"data": [
                        {"id": "gpt-6.1-sol"}, {"id": "unreachable-model"}
                    ]}))
                }),
            )
            .route(
                "/v1/responses",
                post(|| async { Json(serde_json::json!({"provider": "first"})) }),
            ),
    );
    let second = spawn_test_upstream(
        axum::Router::new()
            .route(
                "/v1/models",
                get(|| async { Json(serde_json::json!({"data": [{"id": "gpt-6-sol"}]})) }),
            )
            .route(
                "/v1/responses",
                post(|| async { Json(serde_json::json!({"provider": "second"})) }),
            ),
    );
    let v4 = ProxyConfigV4 {
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "first".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(first.base_url()),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "second".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(second.base_url()),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(RoutingConfigV4 {
                entry: "root".to_string(),
                routes: std::collections::BTreeMap::from([(
                    "root".to_string(),
                    RoutingNodeV4 {
                        strategy: RoutingPolicyV4::Conditional,
                        when: Some(RoutingConditionV4 {
                            model: Some("gpt-6.1-sol".to_string()),
                            ..RoutingConditionV4::default()
                        }),
                        then: Some("first".to_string()),
                        default_route: Some("second".to_string()),
                        ..RoutingNodeV4::default()
                    },
                )]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("runtime");
    let proxy = spawn_proxy_service(ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    ));
    let client = Client::new();
    let value: serde_json::Value = client
        .get(proxy.url("/models"))
        .send()
        .await
        .expect("catalog")
        .json()
        .await
        .expect("JSON");
    let models = value["models"].as_array().expect("models");
    assert_eq!(
        models
            .iter()
            .map(|model| model["slug"].as_str().expect("slug"))
            .collect::<Vec<_>>(),
        ["gpt-6.1-sol", "gpt-6-sol"]
    );
    let response = client.post(proxy.responses_url())
        .json(&serde_json::json!({"model": "gpt-6.1-sol", "reasoning": {"effort": "high"}, "input": []}))
        .send().await.expect("inference");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await.expect("JSON")["provider"],
        "first"
    );
}

#[tokio::test]
async fn model_catalog_failure_does_not_break_inference_and_mapping_remains_routable() {
    let broken = spawn_test_upstream(
        axum::Router::new()
            .route("/v1/models", get(|| async { StatusCode::BAD_GATEWAY }))
            .route(
                "/v1/responses",
                post(|| async { Json(serde_json::json!({"provider": "working-inference"})) }),
            ),
    );
    let mapped = spawn_test_upstream(
        axum::Router::new()
            .route(
                "/v1/models",
                get(|| async {
                    Json(serde_json::json!({"data": [{"name": "models/internal-sol"}]}))
                }),
            )
            .route(
                "/v1/responses",
                post(|Json(value): Json<serde_json::Value>| async move {
                    assert_eq!(value["model"], "models/internal-sol");
                    Json(serde_json::json!({"provider": "mapped"}))
                }),
            ),
    );
    let mut mapped_target = mapped.upstream_config();
    mapped_target
        .model_mapping
        .insert("client-sol".to_string(), "models/internal-sol".to_string());
    let mut broken_target = broken.upstream_config();
    broken_target
        .supported_models
        .insert("inference-only".to_string(), true);
    let proxy = spawn_test_proxy(make_proxy_config(
        vec![broken_target, mapped_target],
        RetryConfig::default(),
    ));
    let client = Client::new();
    let catalog: serde_json::Value = client
        .get(proxy.url("/models"))
        .send()
        .await
        .expect("catalog")
        .json()
        .await
        .expect("JSON");
    let models = catalog["models"].as_array().expect("models");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["slug"], "client-sol");
    let response = client
        .post(proxy.responses_url())
        .json(&serde_json::json!({"model": "inference-only", "input": []}))
        .send()
        .await
        .expect("inference");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await.expect("JSON")["provider"],
        "working-inference"
    );
    let response = client
        .post(proxy.responses_url())
        .json(&serde_json::json!({"model": "client-sol", "input": []}))
        .send()
        .await
        .expect("mapped inference");
    assert_eq!(
        response.json::<serde_json::Value>().await.expect("JSON")["provider"],
        "mapped"
    );
}

#[tokio::test]
async fn model_catalog_cache_isolates_credentials_on_shared_base_urls() {
    let upstream = spawn_test_upstream(axum::Router::new().route(
        "/v1/models",
        get(|headers: HeaderMap| async move {
            let id = if headers
                .get(header::AUTHORIZATION)
                .and_then(|h| h.to_str().ok())
                == Some("Bearer fixture-a")
            {
                "model-a"
            } else {
                "model-b"
            };
            Json(serde_json::json!({"data": [{"id": id}]}))
        }),
    ));
    let mut first = upstream.upstream_config();
    first.auth.auth_token = Some("fixture-a".to_string());
    let mut second = upstream.upstream_config();
    second.auth.auth_token = Some("fixture-b".to_string());
    let proxy = spawn_test_proxy(make_proxy_config(
        vec![first, second],
        RetryConfig::default(),
    ));
    let value: serde_json::Value = Client::new()
        .get(proxy.url("/models"))
        .send()
        .await
        .expect("catalog")
        .json()
        .await
        .expect("JSON");
    let ids = value["models"]
        .as_array()
        .expect("models")
        .iter()
        .map(|m| m["slug"].as_str().expect("slug"))
        .collect::<Vec<_>>();
    assert_eq!(ids, ["model-a", "model-b"]);
}

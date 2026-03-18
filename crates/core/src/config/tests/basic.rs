use super::*;

#[test]
fn infer_env_key_from_auth_json_single_key() {
    let json = serde_json::json!({
        "OPENAI_API_KEY": "sk-test-123",
        "tokens": null
    });
    let auth = Some(json);
    let inferred = infer_env_key_from_auth_json(&auth);
    assert!(inferred.is_some());
    let (key, value) = inferred.unwrap();
    assert_eq!(key, "OPENAI_API_KEY");
    assert_eq!(value, "sk-test-123");
}

#[test]
fn infer_env_key_from_auth_json_multiple_keys() {
    let json = serde_json::json!({
        "OPENAI_API_KEY": "sk-test-1",
        "MISTRAL_API_KEY": "sk-test-2"
    });
    let auth = Some(json);
    let inferred = infer_env_key_from_auth_json(&auth);
    assert!(inferred.is_none());
}

#[test]
fn infer_env_key_from_auth_json_none() {
    let json = serde_json::json!({
        "tokens": {
            "id_token": "xxx"
        }
    });
    let auth = Some(json);
    let inferred = infer_env_key_from_auth_json(&auth);
    assert!(inferred.is_none());
}

#[test]
fn service_routing_explanation_serializes_station_first_fields() {
    let mut mgr = ServiceConfigManager {
        active: Some("alpha".to_string()),
        ..Default::default()
    };
    mgr.configs.insert(
        "alpha".to_string(),
        ServiceConfig {
            name: "alpha".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: vec![UpstreamConfig {
                base_url: "https://alpha.example/v1".to_string(),
                auth: UpstreamAuth::default(),
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );

    let explanation = explain_service_routing(&mgr);
    let json = serde_json::to_value(&explanation).expect("serialize routing explanation");

    assert_eq!(
        json.get("active_station").and_then(|v| v.as_str()),
        Some("alpha")
    );
    assert!(json.get("active_config").is_none());
    assert!(json.get("eligible_stations").is_some());
    assert!(json.get("eligible_configs").is_none());
    assert!(json.get("fallback_station").is_some_and(|v| v.is_null()));
    assert!(json.get("fallback_config").is_none());
}

#[test]
fn service_routing_explanation_uses_station_first_fallback_mode_names() {
    let mut mgr = ServiceConfigManager {
        active: Some("alpha".to_string()),
        ..Default::default()
    };
    mgr.configs.insert(
        "alpha".to_string(),
        ServiceConfig {
            name: "alpha".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: Vec::new(),
        },
    );

    let explanation = explain_service_routing(&mgr);
    assert_eq!(explanation.mode, "single_level_fallback_active_station");
    assert_eq!(
        explanation
            .fallback_station
            .as_ref()
            .map(|candidate| candidate.name.as_str()),
        Some("alpha")
    );
}

#[test]
fn service_config_manager_serializes_stations_and_reads_legacy_configs_alias() {
    let mut mgr = ServiceConfigManager::default();
    mgr.configs.insert(
        "alpha".to_string(),
        ServiceConfig {
            name: "alpha".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: vec![UpstreamConfig {
                base_url: "https://alpha.example/v1".to_string(),
                auth: UpstreamAuth::default(),
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );

    let json = serde_json::to_value(&mgr).expect("serialize manager");
    assert!(json.get("stations").is_some());
    assert!(json.get("configs").is_none());

    let decoded: ServiceConfigManager = serde_json::from_value(serde_json::json!({
        "configs": {
            "legacy": {
                "name": "legacy",
                "enabled": true,
                "level": 1,
                "upstreams": []
            }
        }
    }))
    .expect("deserialize legacy configs alias");
    assert!(decoded.configs.contains_key("legacy"));
}

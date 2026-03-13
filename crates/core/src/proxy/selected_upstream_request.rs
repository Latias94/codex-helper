use axum::body::Bytes;

use crate::lb::SelectedUpstream;
use crate::logging::now_ms;
use crate::logging::BodyPreview;
use crate::model_routing;
use crate::state::{RouteDecisionProvenance, SessionBinding};

use super::request_body::apply_model_override;
use super::request_preparation::build_body_previews;
use super::route_provenance::build_route_decision_provenance;
use super::ProxyService;

pub(super) struct SelectedUpstreamRequestSetup {
    pub(super) model_note: String,
    pub(super) provider_id: Option<String>,
    pub(super) route_decision: RouteDecisionProvenance,
    pub(super) filtered_body: Bytes,
    pub(super) upstream_request_body_len: usize,
    pub(super) upstream_request_body_debug: Option<BodyPreview>,
    pub(super) upstream_request_body_warn: Option<BodyPreview>,
}

pub(super) struct SelectedUpstreamRequestSetupParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) selected: &'a SelectedUpstream,
    pub(super) body_for_upstream: &'a Bytes,
    pub(super) request_model: Option<&'a str>,
    pub(super) session_binding: Option<&'a SessionBinding>,
    pub(super) session_override_config: Option<&'a str>,
    pub(super) global_station_override: Option<&'a str>,
    pub(super) override_model: Option<&'a str>,
    pub(super) override_effort: Option<&'a str>,
    pub(super) override_service_tier: Option<&'a str>,
    pub(super) effective_effort: Option<&'a str>,
    pub(super) effective_service_tier: Option<&'a str>,
    pub(super) client_content_type: Option<&'a str>,
    pub(super) request_body_previews: bool,
    pub(super) debug_max: usize,
    pub(super) warn_max: usize,
}

pub(super) fn prepare_selected_upstream_request(
    params: SelectedUpstreamRequestSetupParams<'_>,
) -> SelectedUpstreamRequestSetup {
    let SelectedUpstreamRequestSetupParams {
        proxy,
        selected,
        body_for_upstream,
        request_model,
        session_binding,
        session_override_config,
        global_station_override,
        override_model,
        override_effort,
        override_service_tier,
        effective_effort,
        effective_service_tier,
        client_content_type,
        request_body_previews,
        debug_max,
        warn_max,
    } = params;

    let (model_note, body_for_selected) =
        apply_selected_model_mapping(selected, body_for_upstream, request_model);
    let provider_id = selected.upstream.tags.get("provider_id").cloned();
    let route_decision = build_route_decision_provenance(
        now_ms(),
        session_binding,
        session_override_config,
        global_station_override,
        override_model,
        override_effort,
        override_service_tier,
        request_model,
        effective_effort,
        effective_service_tier,
        selected,
        provider_id.as_deref(),
    );

    let filtered_body = proxy.filter.apply_bytes(body_for_selected);
    let upstream_request_body_len = filtered_body.len();
    let upstream_body_previews = build_body_previews(
        &filtered_body,
        client_content_type,
        request_body_previews,
        debug_max,
        warn_max,
    );

    SelectedUpstreamRequestSetup {
        model_note,
        provider_id,
        route_decision,
        filtered_body,
        upstream_request_body_len,
        upstream_request_body_debug: upstream_body_previews.debug,
        upstream_request_body_warn: upstream_body_previews.warn,
    }
}

fn apply_selected_model_mapping(
    selected: &SelectedUpstream,
    body_for_upstream: &Bytes,
    request_model: Option<&str>,
) -> (String, Bytes) {
    let Some(requested_model) = request_model else {
        return ("-".to_string(), body_for_upstream.clone());
    };

    let effective_model =
        model_routing::effective_model(&selected.upstream.model_mapping, requested_model);
    if effective_model != requested_model {
        let body = apply_model_override(body_for_upstream.as_ref(), effective_model.as_str())
            .map(Bytes::from)
            .unwrap_or_else(|| body_for_upstream.clone());
        return (format!("{requested_model}->{effective_model}"), body);
    }

    (requested_model.to_string(), body_for_upstream.clone())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::config::{ProxyConfig, UpstreamAuth, UpstreamConfig};
    use crate::lb::{LbState, SelectedUpstream};
    use crate::state::SessionContinuityMode;

    fn test_proxy_service() -> ProxyService {
        ProxyService::new(
            reqwest::Client::new(),
            Arc::new(ProxyConfig::default()),
            "codex",
            Arc::new(Mutex::new(HashMap::<String, LbState>::new())),
        )
    }

    fn test_selected_upstream() -> SelectedUpstream {
        let mut tags = HashMap::new();
        tags.insert("provider_id".to_string(), "test-provider".to_string());
        let mut model_mapping = HashMap::new();
        model_mapping.insert("gpt-5".to_string(), "gpt-5.4".to_string());

        SelectedUpstream {
            station_name: "alpha".to_string(),
            index: 0,
            upstream: UpstreamConfig {
                base_url: "https://example.com/v1".to_string(),
                auth: UpstreamAuth::default(),
                tags,
                supported_models: HashMap::new(),
                model_mapping,
            },
        }
    }

    fn test_binding() -> SessionBinding {
        SessionBinding {
            session_id: "session-1".to_string(),
            profile_name: Some("default".to_string()),
            station_name: Some("alpha".to_string()),
            model: Some("gpt-5".to_string()),
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("priority".to_string()),
            continuity_mode: SessionContinuityMode::DefaultProfile,
            created_at_ms: 1,
            updated_at_ms: 2,
            last_seen_ms: 3,
        }
    }

    #[tokio::test]
    async fn prepare_selected_upstream_request_applies_mapping_and_route_provenance() {
        let proxy = test_proxy_service();
        let selected = test_selected_upstream();
        let body = Bytes::from_static(br#"{"model":"gpt-5"}"#);
        let binding = test_binding();

        let setup = prepare_selected_upstream_request(SelectedUpstreamRequestSetupParams {
            proxy: &proxy,
            selected: &selected,
            body_for_upstream: &body,
            request_model: Some("gpt-5"),
            session_binding: Some(&binding),
            session_override_config: None,
            global_station_override: None,
            override_model: None,
            override_effort: Some("medium"),
            override_service_tier: None,
            effective_effort: Some("medium"),
            effective_service_tier: Some("priority"),
            client_content_type: Some("application/json"),
            request_body_previews: true,
            debug_max: 128,
            warn_max: 64,
        });

        assert_eq!(setup.model_note, "gpt-5->gpt-5.4");
        assert_eq!(setup.provider_id.as_deref(), Some("test-provider"));
        assert_eq!(
            setup.route_decision.provider_id.as_deref(),
            Some("test-provider")
        );
        assert_eq!(setup.upstream_request_body_len, setup.filtered_body.len());
        assert!(setup.upstream_request_body_debug.is_some());
        assert!(String::from_utf8_lossy(setup.filtered_body.as_ref()).contains("gpt-5.4"));
    }

    #[test]
    fn apply_selected_model_mapping_keeps_original_when_mapping_missing() {
        let selected = test_selected_upstream();
        let body = Bytes::from_static(br#"{"model":"gpt-4.1"}"#);

        let (model_note, mapped_body) =
            apply_selected_model_mapping(&selected, &body, Some("gpt-4.1"));

        assert_eq!(model_note, "gpt-4.1");
        assert_eq!(mapped_body, body);
    }
}

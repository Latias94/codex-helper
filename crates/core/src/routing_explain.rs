use std::collections::BTreeMap;

use crate::config::RoutingConditionV4;
use crate::lb::SelectedUpstream;
use crate::routing_ir::{
    RouteCandidate, RoutePlanAttemptState, RoutePlanExecutor, RoutePlanRuntimeState,
    RoutePlanSkipReason, RoutePlanTemplate, RouteRef, RouteRequestContext,
    request_matches_condition,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RoutingExplainResponse {
    pub api_version: u32,
    pub service_name: String,
    pub runtime_loaded_at_ms: Option<u64>,
    pub request_model: Option<String>,
    pub session_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "RoutingExplainRequestContext::is_empty"
    )]
    pub request_context: RoutingExplainRequestContext,
    pub selected_route: Option<RoutingExplainCandidate>,
    pub candidates: Vec<RoutingExplainCandidate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditional_routes: Vec<RoutingExplainConditionalRoute>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RoutingExplainRequestContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<String>,
}

impl RoutingExplainRequestContext {
    fn is_empty(&self) -> bool {
        self.model.is_none()
            && self.service_tier.is_none()
            && self.reasoning_effort.is_none()
            && self.path.is_none()
            && self.method.is_none()
            && self.headers.is_empty()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RoutingExplainConditionalRoute {
    pub route_name: String,
    pub condition: RoutingExplainCondition,
    pub matched: bool,
    pub selected_branch: RoutingExplainConditionalBranch,
    pub selected_target: Option<RoutingExplainRouteRef>,
    pub then: Option<RoutingExplainRouteRef>,
    #[serde(rename = "default")]
    pub default_route: Option<RoutingExplainRouteRef>,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoutingExplainConditionalBranch {
    Then,
    Default,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RoutingExplainRouteRef {
    pub kind: RoutingExplainRouteRefKind,
    pub name: String,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoutingExplainRouteRefKind {
    Route,
    Provider,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RoutingExplainCondition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RoutingExplainCandidate {
    pub provider_id: String,
    pub provider_alias: Option<String>,
    pub endpoint_id: String,
    pub route_path: Vec<String>,
    pub station_name: String,
    pub upstream_index: usize,
    pub upstream_base_url: String,
    pub selected: bool,
    pub skip_reasons: Vec<RoutingExplainSkipReason>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum RoutingExplainSkipReason {
    UnsupportedModel { requested_model: String },
    RuntimeDisabled,
    Cooldown,
    BreakerOpen { failure_count: u32 },
    UsageExhausted,
    MissingAuth,
}

pub fn build_routing_explain_response(
    service_name: impl Into<String>,
    runtime_loaded_at_ms: Option<u64>,
    request_model: Option<String>,
    session_id: Option<String>,
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
) -> RoutingExplainResponse {
    build_routing_explain_response_with_request(
        service_name,
        runtime_loaded_at_ms,
        RouteRequestContext {
            model: request_model,
            ..RouteRequestContext::default()
        },
        session_id,
        template,
        runtime,
    )
}

pub fn build_routing_explain_response_with_request(
    service_name: impl Into<String>,
    runtime_loaded_at_ms: Option<u64>,
    request: RouteRequestContext,
    session_id: Option<String>,
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
) -> RoutingExplainResponse {
    let executor = RoutePlanExecutor::new(template);
    let mut state = RoutePlanAttemptState::default();
    let selection = executor.select_supported_candidate_with_runtime_state(
        &mut state,
        runtime,
        request.model.as_deref(),
    );
    let selected_key = selection
        .selected
        .as_ref()
        .map(|selected| candidate_key(&selected.selected_upstream));
    let skip_reasons_by_candidate = executor
        .explain_candidate_skip_reasons_with_runtime_state(runtime, request.model.as_deref())
        .into_iter()
        .map(|explanation| {
            (
                candidate_key(&explanation.selected_upstream),
                explanation
                    .reasons
                    .iter()
                    .map(RoutingExplainSkipReason::from)
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let candidates = executor
        .iter_selected_upstreams()
        .map(|selected| {
            let key = candidate_key(&selected.selected_upstream);
            routing_explain_candidate(
                selected.candidate,
                &selected.selected_upstream,
                selected_key.as_ref() == Some(&key),
                skip_reasons_by_candidate
                    .get(&key)
                    .cloned()
                    .unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    let selected_route = candidates
        .iter()
        .find(|candidate| candidate.selected)
        .cloned();

    RoutingExplainResponse {
        api_version: 1,
        service_name: service_name.into(),
        runtime_loaded_at_ms,
        request_model: request.model.clone(),
        session_id,
        request_context: RoutingExplainRequestContext::from(&request),
        selected_route,
        candidates,
        conditional_routes: routing_explain_conditional_routes(template, &request),
    }
}

pub fn parse_routing_explain_headers(
    headers: &[String],
) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for header in headers {
        let Some((name, value)) = header.split_once('=') else {
            return Err(format!("header condition '{header}' must use NAME=VALUE"));
        };
        let name = name.trim();
        if name.is_empty() {
            return Err("header condition name cannot be empty".to_string());
        }
        out.insert(name.to_string(), value.trim().to_string());
    }
    Ok(out)
}

impl From<&RoutePlanSkipReason> for RoutingExplainSkipReason {
    fn from(reason: &RoutePlanSkipReason) -> Self {
        match reason {
            RoutePlanSkipReason::UnsupportedModel { requested_model } => {
                RoutingExplainSkipReason::UnsupportedModel {
                    requested_model: requested_model.clone(),
                }
            }
            RoutePlanSkipReason::RuntimeDisabled => RoutingExplainSkipReason::RuntimeDisabled,
            RoutePlanSkipReason::Cooldown => RoutingExplainSkipReason::Cooldown,
            RoutePlanSkipReason::BreakerOpen { failure_count } => {
                RoutingExplainSkipReason::BreakerOpen {
                    failure_count: *failure_count,
                }
            }
            RoutePlanSkipReason::UsageExhausted => RoutingExplainSkipReason::UsageExhausted,
            RoutePlanSkipReason::MissingAuth => RoutingExplainSkipReason::MissingAuth,
        }
    }
}

impl RoutingExplainSkipReason {
    pub fn code(&self) -> &'static str {
        match self {
            RoutingExplainSkipReason::UnsupportedModel { .. } => "unsupported_model",
            RoutingExplainSkipReason::RuntimeDisabled => "runtime_disabled",
            RoutingExplainSkipReason::Cooldown => "cooldown",
            RoutingExplainSkipReason::BreakerOpen { .. } => "breaker_open",
            RoutingExplainSkipReason::UsageExhausted => "usage_exhausted",
            RoutingExplainSkipReason::MissingAuth => "missing_auth",
        }
    }
}

fn candidate_key(selected: &SelectedUpstream) -> (String, usize) {
    (selected.station_name.clone(), selected.index)
}

fn routing_explain_candidate(
    candidate: &RouteCandidate,
    selected_upstream: &SelectedUpstream,
    selected: bool,
    skip_reasons: Vec<RoutingExplainSkipReason>,
) -> RoutingExplainCandidate {
    RoutingExplainCandidate {
        provider_id: candidate.provider_id.clone(),
        provider_alias: candidate.provider_alias.clone(),
        endpoint_id: candidate.endpoint_id.clone(),
        route_path: candidate.route_path.clone(),
        station_name: selected_upstream.station_name.clone(),
        upstream_index: selected_upstream.index,
        upstream_base_url: selected_upstream.upstream.base_url.clone(),
        selected,
        skip_reasons,
    }
}

fn routing_explain_conditional_routes(
    template: &RoutePlanTemplate,
    request: &RouteRequestContext,
) -> Vec<RoutingExplainConditionalRoute> {
    template
        .nodes
        .values()
        .filter_map(|node| {
            let condition = node.when.as_ref()?;
            let matched = request_matches_condition(request, condition);
            let selected_branch = if matched {
                RoutingExplainConditionalBranch::Then
            } else {
                RoutingExplainConditionalBranch::Default
            };
            let selected_target = match selected_branch {
                RoutingExplainConditionalBranch::Then => node.then.as_ref(),
                RoutingExplainConditionalBranch::Default => node.default_route.as_ref(),
            }
            .map(RoutingExplainRouteRef::from);

            Some(RoutingExplainConditionalRoute {
                route_name: node.name.clone(),
                condition: RoutingExplainCondition::from(condition),
                matched,
                selected_branch,
                selected_target,
                then: node.then.as_ref().map(RoutingExplainRouteRef::from),
                default_route: node
                    .default_route
                    .as_ref()
                    .map(RoutingExplainRouteRef::from),
            })
        })
        .collect()
}

impl From<&RouteRequestContext> for RoutingExplainRequestContext {
    fn from(request: &RouteRequestContext) -> Self {
        Self {
            model: request.model.clone(),
            service_tier: request.service_tier.clone(),
            reasoning_effort: request.reasoning_effort.clone(),
            path: request.path.clone(),
            method: request.method.clone(),
            headers: request.headers.keys().cloned().collect(),
        }
    }
}

impl From<&RoutingConditionV4> for RoutingExplainCondition {
    fn from(condition: &RoutingConditionV4) -> Self {
        Self {
            model: condition.model.clone(),
            service_tier: condition.service_tier.clone(),
            reasoning_effort: condition.reasoning_effort.clone(),
            path: condition.path.clone(),
            method: condition.method.clone(),
            headers: condition.headers.keys().cloned().collect(),
        }
    }
}

impl From<&RouteRef> for RoutingExplainRouteRef {
    fn from(route_ref: &RouteRef) -> Self {
        match route_ref {
            RouteRef::Route(name) => Self {
                kind: RoutingExplainRouteRefKind::Route,
                name: name.clone(),
            },
            RouteRef::Provider(name) => Self {
                kind: RoutingExplainRouteRefKind::Provider,
                name: name.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::Value;

    use super::*;
    use crate::config::{
        ProviderConfigV4, RoutingConditionV4, RoutingConfigV4, RoutingNodeV4, RoutingPolicyV4,
        ServiceViewV4, UpstreamAuth,
    };
    use crate::routing_ir::compile_v4_route_plan_template_with_request;

    fn provider(base_url: &str) -> ProviderConfigV4 {
        ProviderConfigV4 {
            base_url: Some(base_url.to_string()),
            inline_auth: UpstreamAuth::default(),
            ..ProviderConfigV4::default()
        }
    }

    #[test]
    fn routing_explain_reports_conditional_route_without_header_values() {
        let request = RouteRequestContext {
            model: Some("gpt-5".to_string()),
            headers: BTreeMap::from([("Authorization".to_string(), "secret-token".to_string())]),
            ..RouteRequestContext::default()
        };
        let view = ServiceViewV4 {
            providers: BTreeMap::from([
                ("small".to_string(), provider("https://small.example/v1")),
                ("large".to_string(), provider("https://large.example/v1")),
            ]),
            routing: Some(RoutingConfigV4 {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RoutingNodeV4 {
                        strategy: RoutingPolicyV4::Conditional,
                        when: Some(RoutingConditionV4 {
                            model: Some("gpt-5".to_string()),
                            headers: BTreeMap::from([(
                                "Authorization".to_string(),
                                "secret-token".to_string(),
                            )]),
                            ..RoutingConditionV4::default()
                        }),
                        then: Some("large".to_string()),
                        default_route: Some("small".to_string()),
                        ..RoutingNodeV4::default()
                    },
                )]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        };
        let template = compile_v4_route_plan_template_with_request("codex", &view, &request)
            .expect("conditional route template");

        let explain = build_routing_explain_response_with_request(
            "codex",
            None,
            request,
            None,
            &template,
            &RoutePlanRuntimeState::default(),
        );
        let value = serde_json::to_value(&explain).expect("serialize explain");

        assert_eq!(
            value["conditional_routes"][0]["selected_branch"].as_str(),
            Some("then")
        );
        assert_eq!(
            value["conditional_routes"][0]["selected_target"]["kind"].as_str(),
            Some("provider")
        );
        assert_eq!(
            value["conditional_routes"][0]["selected_target"]["name"].as_str(),
            Some("large")
        );
        assert_eq!(
            value["conditional_routes"][0]["condition"]["headers"]
                .as_array()
                .map(|headers| headers.iter().filter_map(Value::as_str).collect::<Vec<_>>()),
            Some(vec!["Authorization"])
        );
        assert_eq!(
            value["request_context"]["headers"]
                .as_array()
                .map(|headers| headers.iter().filter_map(Value::as_str).collect::<Vec<_>>()),
            Some(vec!["Authorization"])
        );

        let text = serde_json::to_string(&value).expect("serialize value");
        assert!(!text.contains("secret-token"));
    }
}

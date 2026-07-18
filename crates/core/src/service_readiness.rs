use std::path::Path;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::auth_resolution::target_credential_readiness;
use crate::config::{HelperConfig, ServiceKind};
use crate::credentials::{
    CredentialAggregateReadiness, CredentialCandidateInput, CredentialReadinessCode,
    CredentialReadinessDetail, CredentialReadinessEvaluator, CredentialSourceCapabilities,
    InstallationIdentity,
};
use crate::routing_ir::CompiledRouteGraph;
use crate::runtime_identity::ProviderEndpointKey;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceCredentialEndpointReadiness {
    pub provider_id: String,
    pub endpoint_id: String,
    pub code: CredentialReadinessCode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<CredentialReadinessDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceCredentialReadinessReport {
    pub service: ServiceKind,
    pub aggregate: CredentialAggregateReadiness,
    pub endpoints: Vec<ServiceCredentialEndpointReadiness>,
}

pub fn evaluate_service_credential_readiness(
    config: &HelperConfig,
    service: ServiceKind,
    capabilities: CredentialSourceCapabilities,
    helper_home: impl AsRef<Path>,
) -> Result<ServiceCredentialReadinessReport> {
    let service_name = service_name(service);
    let view = match service {
        ServiceKind::Codex => &config.codex,
        ServiceKind::Claude => &config.claude,
    };
    let graph = CompiledRouteGraph::compile(service_name, view)
        .with_context(|| format!("compile {service_name} route graph for credential preflight"))?;
    let installation = InstallationIdentity::resolve_in_home(helper_home)
        .context("resolve canonical installation identity for credential preflight")?;
    let evaluator = CredentialReadinessEvaluator::new(capabilities, installation);
    let mut evaluated = evaluator.evaluate(graph.candidates().iter().map(|candidate| {
        CredentialCandidateInput {
            provider_endpoint: ProviderEndpointKey::new(
                service_name,
                candidate.provider_id.clone(),
                candidate.endpoint_id.clone(),
            ),
            auth: &candidate.auth,
        }
    }))?;
    let endpoints = graph
        .candidates()
        .iter()
        .map(|candidate| {
            let key = ProviderEndpointKey::new(
                service_name,
                candidate.provider_id.clone(),
                candidate.endpoint_id.clone(),
            );
            let endpoint = evaluated
                .remove(&key)
                .ok_or_else(|| anyhow!("credential evaluator omitted provider endpoint {key}"))?;
            let code = target_credential_readiness(
                service_name,
                endpoint.configured_contract,
                endpoint.allow_anonymous,
                candidate.base_url.as_str(),
                endpoint.code,
            );
            Ok(ServiceCredentialEndpointReadiness {
                provider_id: candidate.provider_id.clone(),
                endpoint_id: candidate.endpoint_id.clone(),
                code,
                details: endpoint.details,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let aggregate = CredentialAggregateReadiness::from_endpoint_codes(
        endpoints.iter().map(|endpoint| endpoint.code),
    );
    Ok(ServiceCredentialReadinessReport {
        service,
        aggregate,
        endpoints,
    })
}

fn service_name(service: ServiceKind) -> &'static str {
    match service {
        ServiceKind::Codex => "codex",
        ServiceKind::Claude => "claude",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config::{ProviderConfig, RouteGraphConfig, ServiceRouteConfig, UpstreamAuth};

    fn config_with_auth(auth: Vec<(&str, UpstreamAuth)>) -> HelperConfig {
        let providers = auth
            .iter()
            .map(|(name, auth)| {
                (
                    (*name).to_string(),
                    ProviderConfig {
                        base_url: Some(format!("https://{name}.example.test/v1")),
                        auth: auth.clone(),
                        ..ProviderConfig::default()
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        HelperConfig {
            codex: ServiceRouteConfig {
                providers,
                routing: Some(RouteGraphConfig::ordered_failover(
                    auth.into_iter().map(|(name, _)| name.to_string()).collect(),
                )),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        }
    }

    #[test]
    fn offline_readiness_distinguishes_ready_degraded_and_blocked_routes() {
        let home = tempfile::tempdir().expect("create helper home");
        let missing = format!("CODEX_HELPER_TEST_MISSING_{}", uuid::Uuid::new_v4());
        let ready = UpstreamAuth {
            auth_token: Some("test-ready-token".to_string().into()),
            ..UpstreamAuth::default()
        };
        let unavailable = UpstreamAuth {
            auth_token_env: Some(missing),
            ..UpstreamAuth::default()
        };

        let degraded = evaluate_service_credential_readiness(
            &config_with_auth(vec![
                ("ready", ready.clone()),
                ("missing", unavailable.clone()),
            ]),
            ServiceKind::Codex,
            CredentialSourceCapabilities::server(),
            home.path(),
        )
        .expect("evaluate degraded route");
        assert_eq!(degraded.aggregate, CredentialAggregateReadiness::Degraded);

        let blocked = evaluate_service_credential_readiness(
            &config_with_auth(vec![("missing", unavailable)]),
            ServiceKind::Codex,
            CredentialSourceCapabilities::server(),
            home.path(),
        )
        .expect("evaluate blocked route");
        assert_eq!(blocked.aggregate, CredentialAggregateReadiness::Blocked);

        let ready = evaluate_service_credential_readiness(
            &config_with_auth(vec![("ready", ready)]),
            ServiceKind::Codex,
            CredentialSourceCapabilities::server(),
            home.path(),
        )
        .expect("evaluate ready route");
        assert_eq!(ready.aggregate, CredentialAggregateReadiness::Ready);
    }
}

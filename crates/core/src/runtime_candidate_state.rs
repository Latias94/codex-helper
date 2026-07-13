use serde::{Deserialize, Serialize};

use crate::balance::{ProviderBalanceSnapshot, ProviderRoutingBalanceSummary};
use crate::routing_ir::{RouteCandidate, RoutePlanTemplate};
use crate::runtime_identity::RuntimeUpstreamIdentity;

#[derive(Debug, Clone, Copy, Default)]
pub struct RouteRuntimeSignalInputs<'a> {
    pub provider_balances: Option<&'a [ProviderBalanceSnapshot]>,
    pub now_ms: u64,
}

impl<'a> RouteRuntimeSignalInputs<'a> {
    pub fn empty(now_ms: u64) -> Self {
        Self {
            provider_balances: None,
            now_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteCandidateRuntimeSignals {
    pub identity: RuntimeUpstreamIdentity,
    #[serde(
        default,
        skip_serializing_if = "ProviderRoutingBalanceSummary::is_empty"
    )]
    pub balance: ProviderRoutingBalanceSummary,
}

impl RoutePlanTemplate {
    pub fn candidate_runtime_signals(
        &self,
        candidate: &RouteCandidate,
        inputs: &RouteRuntimeSignalInputs<'_>,
    ) -> RouteCandidateRuntimeSignals {
        let identity = self.candidate_identity(candidate);
        RouteCandidateRuntimeSignals {
            balance: candidate_balance_summary(&identity, inputs.provider_balances, inputs.now_ms),
            identity,
        }
    }

    pub fn candidate_runtime_signal_view(
        &self,
        inputs: &RouteRuntimeSignalInputs<'_>,
    ) -> Vec<RouteCandidateRuntimeSignals> {
        self.candidates
            .iter()
            .map(|candidate| self.candidate_runtime_signals(candidate, inputs))
            .collect()
    }
}

fn candidate_balance_summary(
    identity: &RuntimeUpstreamIdentity,
    provider_balances: Option<&[ProviderBalanceSnapshot]>,
    now_ms: u64,
) -> ProviderRoutingBalanceSummary {
    let Some(provider_balances) = provider_balances else {
        return ProviderRoutingBalanceSummary::default();
    };

    let snapshots = provider_balances
        .iter()
        .filter(|snapshot| snapshot.provider_endpoint == identity.provider_endpoint);

    ProviderRoutingBalanceSummary::from_snapshot_iter_at(snapshots, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::BalanceSnapshotStatus;
    use crate::config::{
        ProviderConcurrencyLimits, ProviderConfig, ProviderEndpointConfig, ServiceRouteConfig,
        UpstreamAuth,
    };
    use crate::routing_ir::compile_route_plan_template;
    use std::collections::BTreeMap;

    fn provider(base_url: &str) -> ProviderConfig {
        ProviderConfig {
            base_url: Some(base_url.to_string()),
            ..ProviderConfig::default()
        }
    }

    fn balance_snapshot(
        observation_provider_id: &str,
        provider_id: &str,
        endpoint_id: &str,
        exhausted: bool,
    ) -> ProviderBalanceSnapshot {
        ProviderBalanceSnapshot {
            observation_provider_id: observation_provider_id.to_string(),
            provider_endpoint: crate::runtime_identity::ProviderEndpointKey::new(
                "codex",
                provider_id,
                endpoint_id,
            ),
            source: "test".to_string(),
            fetched_at_ms: 100,
            stale_after_ms: Some(200),
            status: if exhausted {
                BalanceSnapshotStatus::Exhausted
            } else {
                BalanceSnapshotStatus::Ok
            },
            exhausted: Some(exhausted),
            ..ProviderBalanceSnapshot::default()
        }
    }

    #[test]
    fn route_candidate_runtime_signals_ignore_balance_for_another_endpoint() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "input".to_string(),
                provider("https://input.example/v1"),
            )]),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");

        let provider_balances = vec![balance_snapshot("source-a", "other", "default", true)];
        let inputs = RouteRuntimeSignalInputs {
            provider_balances: Some(&provider_balances),
            now_ms: 150,
        };

        let signals = template.candidate_runtime_signal_view(&inputs);

        assert_eq!(signals.len(), 1);
        assert_eq!(
            signals[0].identity.provider_endpoint.stable_key(),
            "codex/input/default"
        );
        assert_eq!(signals[0].identity.base_url, "https://input.example/v1");
        assert!(signals[0].identity.continuity_domain.is_none());
        assert!(signals[0].balance.is_empty());
    }

    #[test]
    fn route_candidate_runtime_signals_disambiguate_multi_endpoint_provider_by_endpoint_key() {
        let mut endpoints = BTreeMap::new();
        endpoints.insert(
            "slow".to_string(),
            ProviderEndpointConfig {
                base_url: "https://slow.example/v1".to_string(),
                continuity_domain: None,
                enabled: true,
                priority: 10,
                tags: BTreeMap::new(),
                supported_models: BTreeMap::new(),
                model_mapping: BTreeMap::new(),
                limits: ProviderConcurrencyLimits::default(),
            },
        );
        endpoints.insert(
            "fast".to_string(),
            ProviderEndpointConfig {
                base_url: "https://fast.example/v1".to_string(),
                continuity_domain: None,
                enabled: true,
                priority: 0,
                tags: BTreeMap::new(),
                supported_models: BTreeMap::new(),
                model_mapping: BTreeMap::new(),
                limits: ProviderConcurrencyLimits::default(),
            },
        );
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfig {
                    endpoints,
                    auth: UpstreamAuth::default(),
                    ..ProviderConfig::default()
                },
            )]),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");
        let provider_balances = vec![
            balance_snapshot("source-a", "input", "fast", false),
            balance_snapshot("source-a", "input", "slow", true),
        ];
        let inputs = RouteRuntimeSignalInputs {
            provider_balances: Some(&provider_balances),
            now_ms: 150,
        };

        let signals = template.candidate_runtime_signal_view(&inputs);

        assert_eq!(
            signals
                .iter()
                .map(|signal| signal.identity.provider_endpoint.stable_key())
                .collect::<Vec<_>>(),
            vec!["codex/input/fast", "codex/input/slow"]
        );
        assert_eq!(signals[0].balance.ok, 1);
        assert_eq!(signals[0].balance.routing_snapshots, 1);
        assert_eq!(signals[1].balance.exhausted, 1);
        assert_eq!(signals[1].balance.routing_exhausted, 1);
    }

    #[test]
    fn route_candidate_runtime_signals_aggregate_matching_endpoint_across_source_buckets() {
        let view = ServiceRouteConfig {
            providers: BTreeMap::from([(
                "input".to_string(),
                provider("https://input.example/v1"),
            )]),
            ..ServiceRouteConfig::default()
        };
        let template = compile_route_plan_template("codex", &view).expect("route template");

        let provider_balances = vec![
            balance_snapshot("source-a", "input", "default", false),
            balance_snapshot("source-b", "input", "default", true),
            balance_snapshot("source-b", "other", "default", true),
        ];
        let inputs = RouteRuntimeSignalInputs {
            provider_balances: Some(&provider_balances),
            now_ms: 150,
        };

        let signals = template.candidate_runtime_signal_view(&inputs);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].balance.snapshots, 2);
        assert_eq!(signals[0].balance.ok, 1);
        assert_eq!(signals[0].balance.exhausted, 1);
        assert_eq!(signals[0].balance.routing_snapshots, 2);
        assert_eq!(signals[0].balance.routing_exhausted, 1);
    }
}

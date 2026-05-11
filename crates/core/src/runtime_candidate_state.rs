use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::balance::{ProviderBalanceSnapshot, StationRoutingBalanceSummary};
use crate::routing_ir::{RouteCandidate, RoutePlanTemplate};
use crate::runtime_identity::RuntimeUpstreamIdentity;
use crate::state::{LbConfigView, LbUpstreamView, PassiveUpstreamHealth, StationHealth};

#[derive(Debug, Clone, Copy, Default)]
pub struct RouteRuntimeSignalInputs<'a> {
    pub station_health: Option<&'a HashMap<String, StationHealth>>,
    pub load_balancers: Option<&'a HashMap<String, LbConfigView>>,
    pub provider_balances: Option<&'a HashMap<String, Vec<ProviderBalanceSnapshot>>>,
    pub now_ms: u64,
}

impl<'a> RouteRuntimeSignalInputs<'a> {
    pub fn empty(now_ms: u64) -> Self {
        Self {
            station_health: None,
            load_balancers: None,
            provider_balances: None,
            now_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteCandidateRuntimeSignals {
    pub identity: RuntimeUpstreamIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passive_health: Option<PassiveUpstreamHealth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_balancer: Option<LbUpstreamView>,
    #[serde(
        default,
        skip_serializing_if = "StationRoutingBalanceSummary::is_empty"
    )]
    pub balance: StationRoutingBalanceSummary,
}

impl RoutePlanTemplate {
    pub fn candidate_runtime_signals(
        &self,
        candidate: &RouteCandidate,
        inputs: &RouteRuntimeSignalInputs<'_>,
    ) -> RouteCandidateRuntimeSignals {
        let identity = self.candidate_identity(candidate);
        RouteCandidateRuntimeSignals {
            passive_health: candidate_passive_health(&identity, inputs.station_health),
            load_balancer: candidate_load_balancer_state(&identity, inputs.load_balancers),
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

fn candidate_passive_health(
    identity: &RuntimeUpstreamIdentity,
    station_health: Option<&HashMap<String, StationHealth>>,
) -> Option<PassiveUpstreamHealth> {
    station_health?
        .get(identity.legacy.station_name.as_str())?
        .upstreams
        .iter()
        .find(|upstream| upstream.base_url == identity.base_url)
        .and_then(|upstream| upstream.passive.clone())
}

fn candidate_load_balancer_state(
    identity: &RuntimeUpstreamIdentity,
    load_balancers: Option<&HashMap<String, LbConfigView>>,
) -> Option<LbUpstreamView> {
    load_balancers?
        .get(identity.legacy.station_name.as_str())?
        .upstreams
        .get(identity.legacy.upstream_index)
        .cloned()
}

fn candidate_balance_summary(
    identity: &RuntimeUpstreamIdentity,
    provider_balances: Option<&HashMap<String, Vec<ProviderBalanceSnapshot>>>,
    now_ms: u64,
) -> StationRoutingBalanceSummary {
    let Some(snapshots) =
        provider_balances.and_then(|balances| balances.get(identity.legacy.station_name.as_str()))
    else {
        return StationRoutingBalanceSummary::default();
    };

    StationRoutingBalanceSummary::from_snapshot_iter_at(
        snapshots
            .iter()
            .filter(|snapshot| balance_snapshot_matches_candidate(snapshot, identity)),
        now_ms,
    )
}

fn balance_snapshot_matches_candidate(
    snapshot: &ProviderBalanceSnapshot,
    identity: &RuntimeUpstreamIdentity,
) -> bool {
    snapshot.provider_id == identity.provider_endpoint.provider_id
        && snapshot.station_name.as_deref() == Some(identity.legacy.station_name.as_str())
        && snapshot.upstream_index == Some(identity.legacy.upstream_index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::balance::BalanceSnapshotStatus;
    use crate::config::{ProviderConfigV4, ProviderEndpointV4, ServiceViewV4, UpstreamAuth};
    use crate::routing_ir::compile_v4_route_plan_template;
    use crate::state::{PassiveHealthState, UpstreamHealth};
    use std::collections::BTreeMap;

    fn provider(base_url: &str) -> ProviderConfigV4 {
        ProviderConfigV4 {
            base_url: Some(base_url.to_string()),
            ..ProviderConfigV4::default()
        }
    }

    fn passive_health(state: PassiveHealthState, score: u8) -> PassiveUpstreamHealth {
        PassiveUpstreamHealth {
            score,
            state,
            observed_at_ms: 100,
            last_failure_at_ms: Some(100),
            consecutive_failures: 1,
            ..PassiveUpstreamHealth::default()
        }
    }

    fn balance_snapshot(
        provider_id: &str,
        upstream_index: usize,
        exhausted: bool,
    ) -> ProviderBalanceSnapshot {
        ProviderBalanceSnapshot {
            provider_id: provider_id.to_string(),
            station_name: Some("routing".to_string()),
            upstream_index: Some(upstream_index),
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
    fn route_candidate_runtime_signals_attach_existing_legacy_state() {
        let view = ServiceViewV4 {
            providers: BTreeMap::from([(
                "input".to_string(),
                provider("https://input.example/v1"),
            )]),
            ..ServiceViewV4::default()
        };
        let template = compile_v4_route_plan_template("codex", &view).expect("route template");

        let station_health = HashMap::from([(
            "routing".to_string(),
            StationHealth {
                checked_at_ms: 100,
                upstreams: vec![UpstreamHealth {
                    base_url: "https://input.example/v1".to_string(),
                    passive: Some(passive_health(PassiveHealthState::Failing, 20)),
                    ..UpstreamHealth::default()
                }],
            },
        )]);
        let load_balancers = HashMap::from([(
            "routing".to_string(),
            LbConfigView {
                last_good_index: None,
                upstreams: vec![LbUpstreamView {
                    failure_count: 3,
                    cooldown_remaining_secs: Some(11),
                    usage_exhausted: true,
                }],
            },
        )]);
        let provider_balances = HashMap::from([(
            "routing".to_string(),
            vec![balance_snapshot("input", 0, true)],
        )]);
        let inputs = RouteRuntimeSignalInputs {
            station_health: Some(&station_health),
            load_balancers: Some(&load_balancers),
            provider_balances: Some(&provider_balances),
            now_ms: 150,
        };

        let signals = template.candidate_runtime_signal_view(&inputs);

        assert_eq!(signals.len(), 1);
        assert_eq!(
            signals[0].identity.provider_endpoint.stable_key(),
            "codex/input/default"
        );
        assert_eq!(signals[0].identity.legacy.stable_key(), "codex/routing/0");
        assert_eq!(
            signals[0]
                .passive_health
                .as_ref()
                .map(|health| health.state),
            Some(PassiveHealthState::Failing)
        );
        assert_eq!(
            signals[0]
                .load_balancer
                .as_ref()
                .and_then(|view| view.cooldown_remaining_secs),
            Some(11)
        );
        assert_eq!(signals[0].balance.exhausted, 1);
        assert_eq!(signals[0].balance.routing_exhausted, 1);
    }

    #[test]
    fn route_candidate_runtime_signals_disambiguate_multi_endpoint_provider_by_legacy_index() {
        let mut endpoints = BTreeMap::new();
        endpoints.insert(
            "slow".to_string(),
            ProviderEndpointV4 {
                base_url: "https://slow.example/v1".to_string(),
                enabled: true,
                priority: 10,
                tags: BTreeMap::new(),
                supported_models: BTreeMap::new(),
                model_mapping: BTreeMap::new(),
            },
        );
        endpoints.insert(
            "fast".to_string(),
            ProviderEndpointV4 {
                base_url: "https://fast.example/v1".to_string(),
                enabled: true,
                priority: 0,
                tags: BTreeMap::new(),
                supported_models: BTreeMap::new(),
                model_mapping: BTreeMap::new(),
            },
        );
        let view = ServiceViewV4 {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfigV4 {
                    endpoints,
                    auth: UpstreamAuth::default(),
                    ..ProviderConfigV4::default()
                },
            )]),
            ..ServiceViewV4::default()
        };
        let template = compile_v4_route_plan_template("codex", &view).expect("route template");
        let provider_balances = HashMap::from([(
            "routing".to_string(),
            vec![
                balance_snapshot("input", 0, false),
                balance_snapshot("input", 1, true),
            ],
        )]);
        let load_balancers = HashMap::from([(
            "routing".to_string(),
            LbConfigView {
                last_good_index: Some(0),
                upstreams: vec![
                    LbUpstreamView {
                        failure_count: 0,
                        cooldown_remaining_secs: None,
                        usage_exhausted: false,
                    },
                    LbUpstreamView {
                        failure_count: 3,
                        cooldown_remaining_secs: Some(30),
                        usage_exhausted: true,
                    },
                ],
            },
        )]);
        let inputs = RouteRuntimeSignalInputs {
            load_balancers: Some(&load_balancers),
            provider_balances: Some(&provider_balances),
            now_ms: 150,
            ..RouteRuntimeSignalInputs::default()
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
        assert_eq!(signals[0].balance.exhausted, 0);
        assert_eq!(signals[0].load_balancer.as_ref().unwrap().failure_count, 0);
        assert_eq!(signals[1].balance.ok, 0);
        assert_eq!(signals[1].balance.exhausted, 1);
        assert_eq!(
            signals[1]
                .load_balancer
                .as_ref()
                .unwrap()
                .cooldown_remaining_secs,
            Some(30)
        );
    }
}

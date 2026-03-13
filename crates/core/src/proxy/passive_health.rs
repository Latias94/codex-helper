use std::time::Instant;

use crate::lb::LoadBalancer;
use crate::logging::now_ms;
use crate::state::ProxyState;

use super::classify::class_is_health_neutral;

pub(super) async fn record_passive_upstream_success(
    state: &ProxyState,
    service_name: &str,
    station_name: &str,
    base_url: &str,
    status_code: u16,
) {
    state
        .record_passive_upstream_success(
            service_name,
            station_name,
            base_url,
            Some(status_code),
            now_ms(),
        )
        .await;
}

pub(super) async fn record_passive_upstream_failure(
    state: &ProxyState,
    service_name: &str,
    station_name: &str,
    base_url: &str,
    status_code: Option<u16>,
    error_class: Option<&str>,
    error: Option<String>,
) {
    if class_is_health_neutral(error_class) {
        return;
    }
    state
        .record_passive_upstream_failure(
            service_name,
            station_name,
            base_url,
            status_code,
            error_class.map(str::to_owned),
            error,
            now_ms(),
        )
        .await;
}

pub(super) fn lb_state_snapshot_json(lb: &LoadBalancer) -> Option<serde_json::Value> {
    let map = match lb.states.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let state = map.get(&lb.service.name)?;
    let now = Instant::now();
    let upstreams = (0..lb.service.upstreams.len())
        .map(|idx| {
            let cooldown_remaining_ms = state
                .cooldown_until
                .get(idx)
                .and_then(|value| *value)
                .map(|until| until.saturating_duration_since(now).as_millis() as u64)
                .filter(|ms| *ms > 0);
            serde_json::json!({
                "idx": idx,
                "failure_count": state.failure_counts.get(idx).copied(),
                "penalty_streak": state.penalty_streak.get(idx).copied(),
                "usage_exhausted": state.usage_exhausted.get(idx).copied(),
                "cooldown_remaining_ms": cooldown_remaining_ms,
            })
        })
        .collect::<Vec<_>>();
    Some(serde_json::json!({
        "last_good_index": state.last_good_index,
        "upstreams": upstreams,
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::{
        LoadBalancer, ProxyState, lb_state_snapshot_json, record_passive_upstream_failure,
        record_passive_upstream_success,
    };
    use crate::config::{ServiceConfig, UpstreamAuth, UpstreamConfig};
    use crate::lb::LbState;
    use crate::proxy::classify::ROUTING_MISMATCH_CAPABILITY_CLASS;

    fn make_load_balancer() -> LoadBalancer {
        let service = ServiceConfig {
            name: "right".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: vec![
                UpstreamConfig {
                    base_url: "https://right.example/v1".to_string(),
                    auth: UpstreamAuth::default(),
                    tags: HashMap::new(),
                    supported_models: HashMap::new(),
                    model_mapping: HashMap::new(),
                },
                UpstreamConfig {
                    base_url: "https://backup.example/v1".to_string(),
                    auth: UpstreamAuth::default(),
                    tags: HashMap::new(),
                    supported_models: HashMap::new(),
                    model_mapping: HashMap::new(),
                },
            ],
        };
        let mut states = HashMap::new();
        states.insert(
            "right".to_string(),
            LbState {
                failure_counts: vec![2, 0],
                cooldown_until: vec![Some(Instant::now() + Duration::from_millis(200)), None],
                usage_exhausted: vec![false, true],
                last_good_index: Some(1),
                penalty_streak: vec![3, 0],
            },
        );
        LoadBalancer::new(Arc::new(service), Arc::new(Mutex::new(states)))
    }

    #[test]
    fn lb_state_snapshot_json_reports_runtime_lb_state() {
        let lb = make_load_balancer();

        let snapshot = lb_state_snapshot_json(&lb).expect("snapshot");

        assert_eq!(
            snapshot.get("last_good_index").and_then(|v| v.as_u64()),
            Some(1)
        );
        let upstreams = snapshot
            .get("upstreams")
            .and_then(|value| value.as_array())
            .expect("upstreams");
        assert_eq!(upstreams.len(), 2);
        assert_eq!(
            upstreams[0]
                .get("failure_count")
                .and_then(|value| value.as_u64()),
            Some(2)
        );
        assert_eq!(
            upstreams[0]
                .get("penalty_streak")
                .and_then(|value| value.as_u64()),
            Some(3)
        );
        assert_eq!(
            upstreams[0]
                .get("cooldown_remaining_ms")
                .and_then(|value| value.as_u64())
                .is_some(),
            true
        );
        assert_eq!(
            upstreams[1]
                .get("usage_exhausted")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn passive_failure_skips_health_neutral_classes() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            record_passive_upstream_failure(
                state.as_ref(),
                "codex",
                "right",
                "https://right.example/v1",
                Some(404),
                Some(ROUTING_MISMATCH_CAPABILITY_CLASS),
                Some("should be ignored".to_string()),
            )
            .await;

            let health = state.get_station_health("codex").await;
            assert!(health.is_empty());
        });
    }

    #[test]
    fn passive_success_records_status_code() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            record_passive_upstream_success(
                state.as_ref(),
                "codex",
                "right",
                "https://right.example/v1",
                200,
            )
            .await;

            let health = state.get_station_health("codex").await;
            let right = health.get("right").expect("right health");
            let upstream = right.upstreams.first().expect("upstream");
            let passive = upstream.passive.as_ref().expect("passive");
            assert_eq!(passive.last_status_code, Some(200));
            assert_eq!(passive.last_error_class, None);
        });
    }
}

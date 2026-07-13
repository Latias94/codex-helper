use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::runtime_identity::ProviderEndpointKey;
use crate::state::{FinishedRequest, is_logical_request_success_status};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct WindowStats {
    pub total: usize,
    pub ok_2xx: usize,
    pub err_429: usize,
    pub err_4xx: usize,
    pub err_5xx: usize,
    pub p50_ms: Option<u64>,
    pub p95_ms: Option<u64>,
    pub avg_attempts: Option<f64>,
    pub retry_rate: Option<f64>,
    pub top_provider: Option<(String, usize)>,
    pub top_provider_endpoint: Option<(String, usize)>,
}

pub fn compute_window_stats<F>(
    recent: &[FinishedRequest],
    now_ms: u64,
    window_ms: u64,
    mut include: F,
) -> WindowStats
where
    F: FnMut(&FinishedRequest) -> bool,
{
    fn percentile(mut v: Vec<u64>, p: f64) -> Option<u64> {
        if v.is_empty() {
            return None;
        }
        let n = v.len();
        let idx = ((p * (n.saturating_sub(1) as f64)).ceil() as usize).min(n - 1);
        let (_, nth, _) = v.select_nth_unstable(idx);
        Some(*nth)
    }

    let cutoff = now_ms.saturating_sub(window_ms);
    let mut out = WindowStats::default();
    let mut ok_lat = Vec::new();
    let mut attempts_sum: u64 = 0;
    let mut retry_cnt: u64 = 0;

    let mut by_provider: HashMap<String, usize> = HashMap::new();
    let mut by_provider_endpoint: HashMap<String, usize> = HashMap::new();

    for r in recent.iter() {
        if r.ended_at_ms < cutoff {
            continue;
        }
        if !include(r) {
            continue;
        }
        out.total += 1;

        let attempts = r.attempt_count();
        attempts_sum = attempts_sum.saturating_add(attempts as u64);
        if attempts > 1 {
            retry_cnt = retry_cnt.saturating_add(1);
        }

        if r.status_code == 429 {
            out.err_429 += 1;
        } else if (400..500).contains(&r.status_code) {
            out.err_4xx += 1;
        } else if (500..600).contains(&r.status_code) {
            out.err_5xx += 1;
        }

        if is_logical_request_success_status(r.status_code) {
            out.ok_2xx += 1;
            ok_lat.push(r.duration_ms);

            if let Some(pid) = r.provider_id.as_deref()
                && !pid.trim().is_empty()
            {
                *by_provider.entry(pid.to_string()).or_insert(0) += 1;
            }
            if let Some(provider_endpoint_key) = canonical_provider_endpoint_key(r) {
                *by_provider_endpoint
                    .entry(provider_endpoint_key)
                    .or_insert(0) += 1;
            }
        }
    }

    out.p50_ms = percentile(ok_lat.clone(), 0.50);
    out.p95_ms = percentile(ok_lat, 0.95);
    if out.total > 0 {
        out.avg_attempts = Some(attempts_sum as f64 / out.total as f64);
        out.retry_rate = Some(retry_cnt as f64 / out.total as f64);
    }

    out.top_provider = by_provider.into_iter().max_by_key(|(_, v)| *v);
    out.top_provider_endpoint = by_provider_endpoint.into_iter().max_by_key(|(_, v)| *v);
    out
}

fn canonical_provider_endpoint_key(request: &FinishedRequest) -> Option<String> {
    let route_decision = request.route_decision.as_ref()?;
    let service_name = non_empty(request.service.as_str())?;
    let provider_id = non_empty(route_decision.provider_id.as_deref()?)?;
    let endpoint_id = non_empty(route_decision.endpoint_id.as_deref()?)?;

    Some(ProviderEndpointKey::new(service_name, provider_id, endpoint_id).stable_key())
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::RouteDecisionProvenance;

    fn finished_request(
        id: u64,
        status_code: u16,
        provider_id: Option<&str>,
        endpoint_id: Option<&str>,
    ) -> FinishedRequest {
        FinishedRequest {
            id,
            trace_id: None,
            session_id: None,
            session_identity_source: None,
            client_name: None,
            client_addr: None,
            cwd: None,
            model: None,
            reasoning_effort: None,
            service_tier: None,
            provider_id: provider_id.map(str::to_string),
            route_decision: Some(RouteDecisionProvenance {
                provider_id: provider_id.map(str::to_string),
                endpoint_id: endpoint_id.map(str::to_string),
                ..RouteDecisionProvenance::default()
            }),
            usage: None,
            cost: Default::default(),
            retry: None,
            provider_signals: Vec::new(),
            policy_actions: Vec::new(),
            observability: Default::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code,
            duration_ms: 100,
            ttfb_ms: None,
            streaming: false,
            ended_at_ms: 1_000,
        }
    }

    #[test]
    fn top_provider_endpoint_groups_successes_by_canonical_provider_endpoint_key() {
        let recent = vec![
            finished_request(1, 200, Some("relay"), Some("primary")),
            finished_request(2, 204, Some("relay"), Some("primary")),
            finished_request(3, 500, Some("other"), Some("default")),
        ];

        let stats = compute_window_stats(&recent, 1_000, 60_000, |_| true);

        assert_eq!(
            stats.top_provider_endpoint,
            Some(("codex/relay/primary".to_string(), 2))
        );
    }

    #[test]
    fn top_provider_endpoint_does_not_fall_back_to_incomplete_route_identity() {
        let recent = vec![finished_request(1, 200, Some("relay"), None)];

        let stats = compute_window_stats(&recent, 1_000, 60_000, |_| true);

        assert_eq!(stats.top_provider, Some(("relay".to_string(), 1)));
        assert_eq!(stats.top_provider_endpoint, None);
    }

    #[test]
    fn websocket_switching_protocols_counts_as_logical_success() {
        let recent = vec![
            finished_request(1, 101, Some("relay"), Some("websocket")),
            finished_request(2, 200, Some("relay"), Some("responses")),
            finished_request(3, 500, Some("relay"), Some("responses")),
        ];

        let stats = compute_window_stats(&recent, 1_000, 60_000, |_| true);

        assert_eq!(stats.total, 3);
        assert_eq!(stats.ok_2xx, 2);
        assert_eq!(stats.err_5xx, 1);
        assert_eq!(stats.p50_ms, Some(100));
    }
}

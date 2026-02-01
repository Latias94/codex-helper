use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::state::FinishedRequest;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    pub top_config: Option<(String, usize)>,
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
    let mut by_config: HashMap<String, usize> = HashMap::new();

    for r in recent.iter() {
        if r.ended_at_ms < cutoff {
            continue;
        }
        if !include(r) {
            continue;
        }
        out.total += 1;

        let attempts = r.retry.as_ref().map(|x| x.attempts).unwrap_or(1);
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

        if (200..300).contains(&r.status_code) {
            out.ok_2xx += 1;
            ok_lat.push(r.duration_ms);

            if let Some(pid) = r.provider_id.as_deref()
                && !pid.trim().is_empty()
            {
                *by_provider.entry(pid.to_string()).or_insert(0) += 1;
            }
            if let Some(cfg) = r.config_name.as_deref()
                && !cfg.trim().is_empty()
            {
                *by_config.entry(cfg.to_string()).or_insert(0) += 1;
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
    out.top_config = by_config.into_iter().max_by_key(|(_, v)| *v);
    out
}

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::config::{ServiceConfig, UpstreamConfig};
use tracing::info;

pub const FAILURE_THRESHOLD: u32 = 3;
pub const COOLDOWN_SECS: u64 = 30;

#[derive(Debug, Clone, Copy)]
pub struct CooldownBackoff {
    pub factor: u64,
    pub max_secs: u64,
}

impl CooldownBackoff {
    pub const fn disabled() -> Self {
        Self {
            factor: 1,
            max_secs: 0,
        }
    }

    fn effective_cooldown_secs(&self, base_secs: u64, penalty_streak: u32) -> u64 {
        if base_secs == 0 {
            return 0;
        }
        if self.factor <= 1 {
            return base_secs;
        }
        let cap = if self.max_secs == 0 {
            base_secs
        } else {
            self.max_secs.max(base_secs)
        };

        let mut secs = base_secs;
        for _ in 0..penalty_streak.min(64) {
            secs = secs.saturating_mul(self.factor);
            if secs >= cap {
                return cap;
            }
        }
        secs.min(cap)
    }
}

#[derive(Debug, Default)]
pub struct LbState {
    pub failure_counts: Vec<u32>,
    pub cooldown_until: Vec<Option<std::time::Instant>>,
    pub usage_exhausted: Vec<bool>,
    pub last_good_index: Option<usize>,
    pub penalty_streak: Vec<u32>,
}

impl LbState {
    fn ensure_len(&mut self, len: usize) {
        if self.failure_counts.len() != len {
            self.failure_counts = vec![0; len];
            self.cooldown_until = vec![None; len];
            self.usage_exhausted = vec![false; len];
            self.penalty_streak = vec![0; len];
            // 如果 upstream 数量发生变化，原来的 last_good_index 很可能已经无效，直接清空。
            self.last_good_index = None;
        }
    }
}

/// Upstream selection result
#[derive(Debug, Clone)]
pub struct SelectedUpstream {
    pub config_name: String,
    pub index: usize,
    pub upstream: UpstreamConfig,
}

/// 简单的负载选择器，当前仅按权重随机，未来可扩展为按 usage / 失败次数等切换。
#[derive(Clone)]
pub struct LoadBalancer {
    pub service: Arc<ServiceConfig>,
    pub states: Arc<Mutex<HashMap<String, LbState>>>,
}

impl LoadBalancer {
    pub fn new(service: Arc<ServiceConfig>, states: Arc<Mutex<HashMap<String, LbState>>>) -> Self {
        Self { service, states }
    }

    #[allow(dead_code)]
    pub fn select_upstream(&self) -> Option<SelectedUpstream> {
        self.select_upstream_avoiding(&HashSet::new())
    }

    pub fn select_upstream_avoiding(&self, avoid: &HashSet<usize>) -> Option<SelectedUpstream> {
        if self.service.upstreams.is_empty() {
            return None;
        }

        let mut map = match self.states.lock() {
            Ok(m) => m,
            Err(e) => e.into_inner(),
        };
        let entry = map.entry(self.service.name.clone()).or_default();
        entry.ensure_len(self.service.upstreams.len());

        let now = std::time::Instant::now();

        // 更新冷却状态：如果冷却期已过，重置失败计数和冷却时间。
        for idx in 0..self.service.upstreams.len() {
            if let Some(until) = entry.cooldown_until.get(idx).and_then(|v| *v)
                && now >= until
            {
                entry.failure_counts[idx] = 0;
                if let Some(slot) = entry.cooldown_until.get_mut(idx) {
                    *slot = None;
                }
            }
        }

        // 优先使用最近一次“成功”的 upstream，实现粘性路由：
        // 一旦已经切换到可用线路，就尽量保持在该线路上，而不是每次都从头熔断。
        if let Some(idx) = entry.last_good_index
            && idx < self.service.upstreams.len()
            && entry.failure_counts[idx] < FAILURE_THRESHOLD
            && !entry.usage_exhausted.get(idx).copied().unwrap_or(false)
            && !avoid.contains(&idx)
        {
            let upstream = self.service.upstreams[idx].clone();
            return Some(SelectedUpstream {
                config_name: self.service.name.clone(),
                index: idx,
                upstream,
            });
        }

        // 第一轮：按顺序选择第一个「未熔断 + 未标记用量用尽」的 upstream。
        if let Some(idx) = self
            .service
            .upstreams
            .iter()
            .enumerate()
            .find_map(|(idx, _)| {
                if avoid.contains(&idx) {
                    return None;
                }
                if entry.failure_counts[idx] >= FAILURE_THRESHOLD {
                    return None;
                }
                if entry.usage_exhausted.get(idx).copied().unwrap_or(false) {
                    return None;
                }
                Some(idx)
            })
        {
            let upstream = self.service.upstreams[idx].clone();
            return Some(SelectedUpstream {
                config_name: self.service.name.clone(),
                index: idx,
                upstream,
            });
        }

        // 第二轮：忽略 usage_exhausted，只看失败阈值，仍然按顺序选第一个。
        if let Some(idx) = self
            .service
            .upstreams
            .iter()
            .enumerate()
            .find_map(|(idx, _)| {
                if avoid.contains(&idx) {
                    return None;
                }
                if entry.failure_counts[idx] >= FAILURE_THRESHOLD {
                    None
                } else {
                    Some(idx)
                }
            })
        {
            let upstream = self.service.upstreams[idx].clone();
            return Some(SelectedUpstream {
                config_name: self.service.name.clone(),
                index: idx,
                upstream,
            });
        }

        // 兜底：所有 upstream 都已达到失败阈值时，仍然返回第一个，以保证永远有兜底。
        // 如果 avoid 把所有都排除了，则兜底返回第一个“非 avoid”的 upstream；仍然没有则返回 0。
        let idx = (0..self.service.upstreams.len())
            .find(|i| !avoid.contains(i))
            .unwrap_or(0);
        let upstream = self.service.upstreams[idx].clone();
        Some(SelectedUpstream {
            config_name: self.service.name.clone(),
            index: idx,
            upstream,
        })
    }

    pub fn penalize(&self, index: usize, cooldown_secs: u64, reason: &str) {
        self.penalize_with_backoff(index, cooldown_secs, reason, CooldownBackoff::disabled());
    }

    pub fn penalize_with_backoff(
        &self,
        index: usize,
        cooldown_secs: u64,
        reason: &str,
        backoff: CooldownBackoff,
    ) {
        let mut map = match self.states.lock() {
            Ok(m) => m,
            Err(_) => return,
        };
        let entry = map
            .entry(self.service.name.clone())
            .or_insert_with(LbState::default);
        entry.ensure_len(self.service.upstreams.len());
        if index >= entry.failure_counts.len() {
            return;
        }

        let streak = entry.penalty_streak.get(index).copied().unwrap_or(0);
        let effective_secs = backoff.effective_cooldown_secs(cooldown_secs, streak);

        entry.failure_counts[index] = FAILURE_THRESHOLD;
        if let Some(slot) = entry.cooldown_until.get_mut(index) {
            *slot = Some(std::time::Instant::now() + std::time::Duration::from_secs(effective_secs));
        }
        if let Some(slot) = entry.penalty_streak.get_mut(index) {
            *slot = streak.saturating_add(1);
        }
        if entry.last_good_index == Some(index) {
            entry.last_good_index = None;
        }
        info!(
            "lb: upstream '{}' index {} penalized for {}s (reason: {})",
            self.service.name, index, effective_secs, reason
        );
    }

    pub fn record_result(&self, index: usize, success: bool) {
        self.record_result_with_backoff(index, success, COOLDOWN_SECS, CooldownBackoff::disabled());
    }

    pub fn record_result_with_backoff(
        &self,
        index: usize,
        success: bool,
        failure_threshold_cooldown_secs: u64,
        backoff: CooldownBackoff,
    ) {
        let mut map = match self.states.lock() {
            Ok(m) => m,
            Err(_) => return,
        };
        let entry = map
            .entry(self.service.name.clone())
            .or_insert_with(LbState::default);
        entry.ensure_len(self.service.upstreams.len());
        if index >= entry.failure_counts.len() {
            return;
        }
        if success {
            entry.failure_counts[index] = 0;
            if let Some(slot) = entry.cooldown_until.get_mut(index) {
                *slot = None;
            }
            if let Some(slot) = entry.penalty_streak.get_mut(index) {
                *slot = 0;
            }
            // 成功请求会将该 upstream 记为“最近可用线路”，后续优先继续使用。
            entry.last_good_index = Some(index);
        } else {
            entry.failure_counts[index] = entry.failure_counts[index].saturating_add(1);
            if entry.failure_counts[index] >= FAILURE_THRESHOLD
                && let Some(slot) = entry.cooldown_until.get_mut(index)
            {
                let base_secs = if failure_threshold_cooldown_secs == 0 {
                    COOLDOWN_SECS
                } else {
                    failure_threshold_cooldown_secs
                };
                let streak = entry.penalty_streak.get(index).copied().unwrap_or(0);
                let effective_secs = backoff.effective_cooldown_secs(base_secs, streak);
                let now = std::time::Instant::now();
                let new_until = now + std::time::Duration::from_secs(effective_secs);
                let should_update = match *slot {
                    Some(existing) => new_until > existing,
                    None => true,
                };
                if should_update {
                    *slot = Some(new_until);
                }
                if let Some(slot) = entry.penalty_streak.get_mut(index) {
                    *slot = streak.saturating_add(1);
                }
                info!(
                    "lb: upstream '{}' index {} reached failure threshold {} (count = {}), entering cooldown for {}s",
                    self.service.name,
                    index,
                    FAILURE_THRESHOLD,
                    entry.failure_counts[index],
                    effective_secs
                );
                // 触发熔断时，如当前 last_good_index 指向该线路，则清空，允许后续选择其他线路。
                if entry.last_good_index == Some(index) {
                    entry.last_good_index = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ServiceConfig, UpstreamAuth, UpstreamConfig};

    fn make_service(name: &str, urls: &[&str]) -> ServiceConfig {
        ServiceConfig {
            name: name.to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: urls
                .iter()
                .map(|u| UpstreamConfig {
                    base_url: u.to_string(),
                    auth: UpstreamAuth {
                        auth_token: Some("sk-test".to_string()),
                        auth_token_env: None,
                        api_key: None,
                        api_key_env: None,
                    },
                    tags: HashMap::new(),
                    supported_models: HashMap::new(),
                    model_mapping: HashMap::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn lb_prefers_non_exhausted_upstream_when_available() {
        let service = make_service(
            "codex-main",
            &["https://primary.example", "https://backup.example"],
        );
        let states = Arc::new(Mutex::new(HashMap::new()));
        let lb = LoadBalancer::new(Arc::new(service), states.clone());

        // 初次选择应选第一个 upstream（index 0）。
        let first = lb.select_upstream().expect("should select an upstream");
        assert_eq!(first.index, 0);

        // 标记 index 0 为 usage_exhausted，index 1 为可用。
        {
            let mut guard = states.lock().unwrap();
            let entry = guard
                .entry("codex-main".to_string())
                .or_insert_with(LbState::default);
            entry.ensure_len(2);
            entry.usage_exhausted[0] = true;
            entry.usage_exhausted[1] = false;
        }

        // 此时应优先选择未 exhausted 的 index 1。
        let second = lb.select_upstream().expect("should select backup upstream");
        assert_eq!(second.index, 1);
    }

    #[test]
    fn lb_falls_back_when_all_exhausted() {
        let service = make_service(
            "codex-main",
            &["https://primary.example", "https://backup.example"],
        );
        let states = Arc::new(Mutex::new(HashMap::new()));
        let lb = LoadBalancer::new(Arc::new(service), states.clone());

        // 初始化状态
        let _ = lb.select_upstream();

        {
            let mut guard = states.lock().unwrap();
            let entry = guard
                .entry("codex-main".to_string())
                .or_insert_with(LbState::default);
            entry.ensure_len(2);
            entry.usage_exhausted[0] = true;
            entry.usage_exhausted[1] = true;
        }

        // 所有 upstream 都 exhausted 时，仍然应返回 index 0 做兜底。
        let selected = lb
            .select_upstream()
            .expect("should still select an upstream");
        assert_eq!(selected.index, 0);
    }

    #[test]
    fn lb_avoids_upstreams_past_failure_threshold() {
        let service = make_service(
            "codex-main",
            &["https://primary.example", "https://backup.example"],
        );
        let states = Arc::new(Mutex::new(HashMap::new()));
        let lb = LoadBalancer::new(Arc::new(service), states.clone());

        // 对 primary 连续记录 FAILURE_THRESHOLD 次失败。
        for _ in 0..FAILURE_THRESHOLD {
            lb.record_result(0, false);
        }

        // 此时应选择 backup（index 1），因为 index 0 已达到失败阈值。
        let selected = lb
            .select_upstream()
            .expect("should select backup after failures");
        assert_eq!(selected.index, 1);
    }
}

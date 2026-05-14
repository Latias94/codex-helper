use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::config::{ServiceConfig, UpstreamConfig};
use crate::runtime_identity::ProviderEndpointKey;
use tracing::info;

pub const FAILURE_THRESHOLD: u32 = 3;
pub const COOLDOWN_SECS: u64 = 30;

#[derive(Debug, Clone, Copy)]
pub struct CooldownBackoff {
    pub factor: u64,
    pub max_secs: u64,
}

impl CooldownBackoff {
    pub(crate) fn effective_cooldown_secs(&self, base_secs: u64, penalty_streak: u32) -> u64 {
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

#[derive(Debug, Clone, Default)]
pub struct LbState {
    pub failure_counts: Vec<u32>,
    pub cooldown_until: Vec<Option<std::time::Instant>>,
    pub usage_exhausted: Vec<bool>,
    pub last_good_index: Option<usize>,
    pub penalty_streak: Vec<u32>,
    pub(crate) upstream_signature: Vec<String>,
}

impl LbState {
    pub(crate) fn ensure_layout(&mut self, service_name: &str, upstreams: &[UpstreamConfig]) {
        let signature = upstreams
            .iter()
            .enumerate()
            .map(|(idx, upstream)| upstream_signature_key(service_name, idx, upstream))
            .collect::<Vec<_>>();
        let legacy_signature = upstreams
            .iter()
            .map(|upstream| upstream.base_url.clone())
            .collect::<Vec<_>>();

        if has_duplicate_signatures(&signature) {
            self.reset_for_layout(signature);
            return;
        }

        let len = upstreams.len();
        if self.upstream_signature == signature
            && self.failure_counts.len() == len
            && self.cooldown_until.len() == len
            && self.usage_exhausted.len() == len
            && self.penalty_streak.len() == len
        {
            return;
        }

        self.migrate_layout(signature, legacy_signature);
    }

    fn reset_for_layout(&mut self, signature: Vec<String>) {
        let len = signature.len();
        self.failure_counts = vec![0; len];
        self.cooldown_until = vec![None; len];
        self.usage_exhausted = vec![false; len];
        self.penalty_streak = vec![0; len];
        // upstream 布局变化时，原来的粘性索引不再可信，直接清空。
        self.last_good_index = None;
        self.upstream_signature = signature;
    }

    fn migrate_layout(&mut self, signature: Vec<String>, legacy_signature: Vec<String>) {
        if self.upstream_signature.is_empty() {
            self.reset_for_layout(signature);
            return;
        }

        let old_signature = std::mem::take(&mut self.upstream_signature);
        if has_duplicate_signatures(&old_signature) {
            self.reset_for_layout(signature);
            return;
        }

        let old_index_by_signature = old_signature
            .iter()
            .enumerate()
            .map(|(idx, key)| (key.clone(), idx))
            .collect::<std::collections::HashMap<_, _>>();
        let legacy_fallback_enabled = !has_duplicate_signatures(&legacy_signature);

        let old_failure_counts = std::mem::take(&mut self.failure_counts);
        let old_cooldown_until = std::mem::take(&mut self.cooldown_until);
        let old_usage_exhausted = std::mem::take(&mut self.usage_exhausted);
        let old_penalty_streak = std::mem::take(&mut self.penalty_streak);
        let old_last_good_index = self.last_good_index.take();

        let len = signature.len();
        self.failure_counts = vec![0; len];
        self.cooldown_until = vec![None; len];
        self.usage_exhausted = vec![false; len];
        self.penalty_streak = vec![0; len];

        for (new_idx, key) in signature.iter().enumerate() {
            let old_idx = old_index_by_signature.get(key).copied().or_else(|| {
                legacy_fallback_enabled
                    .then(|| legacy_signature.get(new_idx))
                    .flatten()
                    .and_then(|legacy_key| old_index_by_signature.get(legacy_key).copied())
            });
            let Some(old_idx) = old_idx else {
                continue;
            };
            self.failure_counts[new_idx] = old_failure_counts.get(old_idx).copied().unwrap_or(0);
            self.cooldown_until[new_idx] = old_cooldown_until.get(old_idx).and_then(|until| *until);
            self.usage_exhausted[new_idx] =
                old_usage_exhausted.get(old_idx).copied().unwrap_or(false);
            self.penalty_streak[new_idx] = old_penalty_streak.get(old_idx).copied().unwrap_or(0);
        }

        self.last_good_index = old_last_good_index.and_then(|old_idx| {
            old_signature.get(old_idx).and_then(|key| {
                signature
                    .iter()
                    .position(|new_key| new_key == key)
                    .or_else(|| {
                        legacy_fallback_enabled
                            .then(|| {
                                legacy_signature
                                    .iter()
                                    .position(|legacy_key| legacy_key == key)
                            })
                            .flatten()
                    })
            })
        });
        self.upstream_signature = signature;
    }
}

fn has_duplicate_signatures(values: &[String]) -> bool {
    let mut seen = HashSet::new();
    values.iter().any(|value| !seen.insert(value))
}

fn upstream_signature_key(
    service_name: &str,
    upstream_index: usize,
    upstream: &UpstreamConfig,
) -> String {
    let provider_id = upstream
        .tags
        .get("provider_id")
        .cloned()
        .unwrap_or_else(|| format!("{service_name}#{upstream_index}"));
    let endpoint_id = upstream
        .tags
        .get("endpoint_id")
        .cloned()
        .unwrap_or_else(|| upstream_index.to_string());
    let provider_endpoint = ProviderEndpointKey::new(service_name, provider_id, endpoint_id);
    format!("{}|{}", provider_endpoint.stable_key(), upstream.base_url)
}

/// Upstream selection result
#[derive(Debug, Clone)]
pub struct SelectedUpstream {
    pub station_name: String,
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

    #[cfg(test)]
    pub fn select_upstream(&self) -> Option<SelectedUpstream> {
        self.select_upstream_avoiding(&HashSet::new())
    }

    pub fn select_upstream_avoiding(&self, avoid: &HashSet<usize>) -> Option<SelectedUpstream> {
        self.select_upstream_avoiding_inner(avoid, false)
    }

    pub fn select_upstream_avoiding_strict(
        &self,
        avoid: &HashSet<usize>,
    ) -> Option<SelectedUpstream> {
        self.select_upstream_avoiding_inner(avoid, true)
    }

    fn select_upstream_avoiding_inner(
        &self,
        avoid: &HashSet<usize>,
        strict: bool,
    ) -> Option<SelectedUpstream> {
        if self.service.upstreams.is_empty() {
            return None;
        }

        let mut map = match self.states.lock() {
            Ok(m) => m,
            Err(e) => e.into_inner(),
        };
        let entry = map.entry(self.service.name.clone()).or_default();
        entry.ensure_layout(self.service.name.as_str(), &self.service.upstreams);

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
                station_name: self.service.name.clone(),
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
                station_name: self.service.name.clone(),
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
                station_name: self.service.name.clone(),
                index: idx,
                upstream,
            });
        }

        if strict {
            return None;
        }

        // 兜底：所有 upstream 都已达到失败阈值时，仍然返回第一个，以保证永远有兜底。
        // 如果 avoid 把所有都排除了，则兜底返回第一个“非 avoid”的 upstream；仍然没有则返回 0。
        let idx = (0..self.service.upstreams.len())
            .find(|i| !avoid.contains(i))
            .unwrap_or(0);
        let upstream = self.service.upstreams[idx].clone();
        Some(SelectedUpstream {
            station_name: self.service.name.clone(),
            index: idx,
            upstream,
        })
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
        entry.ensure_layout(self.service.name.as_str(), &self.service.upstreams);
        if index >= entry.failure_counts.len() {
            return;
        }

        let streak = entry.penalty_streak.get(index).copied().unwrap_or(0);
        let effective_secs = backoff.effective_cooldown_secs(cooldown_secs, streak);

        entry.failure_counts[index] = FAILURE_THRESHOLD;
        if let Some(slot) = entry.cooldown_until.get_mut(index) {
            *slot =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(effective_secs));
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
        entry.ensure_layout(self.service.name.as_str(), &self.service.upstreams);
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

    fn make_provider_endpoint_service(
        name: &str,
        upstreams: &[(&str, &str, &str)],
    ) -> ServiceConfig {
        ServiceConfig {
            name: name.to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: upstreams
                .iter()
                .map(|(base_url, provider_id, endpoint_id)| UpstreamConfig {
                    base_url: (*base_url).to_string(),
                    auth: UpstreamAuth {
                        auth_token: Some("sk-test".to_string()),
                        auth_token_env: None,
                        api_key: None,
                        api_key_env: None,
                    },
                    tags: HashMap::from([
                        ("provider_id".to_string(), (*provider_id).to_string()),
                        ("endpoint_id".to_string(), (*endpoint_id).to_string()),
                    ]),
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
            entry.ensure_layout(lb.service.name.as_str(), &lb.service.upstreams);
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
            entry.ensure_layout(lb.service.name.as_str(), &lb.service.upstreams);
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
    fn lb_strict_mode_still_falls_back_when_all_usage_exhausted() {
        let service = make_service(
            "codex-main",
            &["https://primary.example", "https://backup.example"],
        );
        let states = Arc::new(Mutex::new(HashMap::new()));
        let lb = LoadBalancer::new(Arc::new(service), states.clone());

        {
            let mut guard = states.lock().unwrap();
            let entry = guard
                .entry("codex-main".to_string())
                .or_insert_with(LbState::default);
            entry.ensure_layout(lb.service.name.as_str(), &lb.service.upstreams);
            entry.usage_exhausted[0] = true;
            entry.usage_exhausted[1] = true;
        }

        let selected = lb
            .select_upstream_avoiding_strict(&HashSet::new())
            .expect("strict mode should still ignore usage exhaustion on fallback");
        assert_eq!(selected.index, 0);
    }

    #[test]
    fn lb_resets_state_when_upstream_layout_changes() {
        let states = Arc::new(Mutex::new(HashMap::new()));
        let initial = LoadBalancer::new(
            Arc::new(make_service(
                "codex-main",
                &["https://primary.example", "https://backup.example"],
            )),
            states.clone(),
        );
        initial.record_result_with_backoff(
            0,
            false,
            COOLDOWN_SECS,
            CooldownBackoff {
                factor: 1,
                max_secs: 0,
            },
        );

        {
            let guard = states.lock().unwrap();
            let entry = guard.get("codex-main").expect("state exists");
            assert_eq!(entry.failure_counts, vec![1, 0]);
        }

        let reordered = LoadBalancer::new(
            Arc::new(make_service(
                "codex-main",
                &["https://backup.example", "https://primary.example"],
            )),
            states.clone(),
        );
        let selected = reordered
            .select_upstream()
            .expect("should select an upstream");
        assert_eq!(selected.index, 0);

        let guard = states.lock().unwrap();
        let entry = guard.get("codex-main").expect("state exists");
        assert_eq!(entry.failure_counts, vec![0, 0]);
        assert_eq!(entry.last_good_index, None);
    }

    #[test]
    fn lb_migrates_state_when_provider_endpoint_order_changes() {
        let states = Arc::new(Mutex::new(HashMap::new()));
        let initial = LoadBalancer::new(
            Arc::new(make_provider_endpoint_service(
                "routing",
                &[
                    ("https://primary.example", "primary", "default"),
                    ("https://backup.example", "backup", "default"),
                ],
            )),
            states.clone(),
        );

        {
            let mut guard = states.lock().unwrap();
            let entry = guard
                .entry("routing".to_string())
                .or_insert_with(LbState::default);
            entry.ensure_layout(initial.service.name.as_str(), &initial.service.upstreams);
            entry.failure_counts[0] = 2;
            entry.cooldown_until[0] =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(30));
            entry.penalty_streak[0] = 3;
            entry.usage_exhausted[1] = true;
            entry.last_good_index = Some(1);
        }

        let reordered = LoadBalancer::new(
            Arc::new(make_provider_endpoint_service(
                "routing",
                &[
                    ("https://backup.example", "backup", "default"),
                    ("https://primary.example", "primary", "default"),
                ],
            )),
            states.clone(),
        );
        let selected = reordered
            .select_upstream()
            .expect("should select a migrated non-exhausted upstream");
        assert_eq!(selected.index, 1);

        let guard = states.lock().unwrap();
        let entry = guard.get("routing").expect("state exists");
        assert_eq!(entry.failure_counts, vec![0, 2]);
        assert_eq!(entry.usage_exhausted, vec![true, false]);
        assert_eq!(entry.penalty_streak, vec![0, 3]);
        assert!(entry.cooldown_until[0].is_none());
        assert!(entry.cooldown_until[1].is_some());
        assert_eq!(entry.last_good_index, Some(0));
    }

    #[test]
    fn lb_migrates_legacy_base_url_signature_when_endpoint_identity_is_unambiguous() {
        let states = Arc::new(Mutex::new(HashMap::new()));
        let primary_url = "https://primary.example";
        let backup_url = "https://backup.example";

        {
            let mut guard = states.lock().unwrap();
            guard.insert(
                "routing".to_string(),
                LbState {
                    failure_counts: vec![FAILURE_THRESHOLD, 0],
                    cooldown_until: vec![None, None],
                    usage_exhausted: vec![false, true],
                    last_good_index: Some(1),
                    penalty_streak: vec![2, 0],
                    upstream_signature: vec![primary_url.to_string(), backup_url.to_string()],
                },
            );
        }

        let reordered = LoadBalancer::new(
            Arc::new(make_provider_endpoint_service(
                "routing",
                &[
                    (backup_url, "backup", "default"),
                    (primary_url, "primary", "default"),
                ],
            )),
            states.clone(),
        );
        {
            let mut guard = states.lock().unwrap();
            let entry = guard.get_mut("routing").expect("state exists");
            entry.ensure_layout(
                reordered.service.name.as_str(),
                &reordered.service.upstreams,
            );
        }

        let guard = states.lock().unwrap();
        let entry = guard.get("routing").expect("state exists");
        assert_eq!(entry.failure_counts, vec![0, FAILURE_THRESHOLD]);
        assert_eq!(entry.usage_exhausted, vec![true, false]);
        assert_eq!(entry.penalty_streak, vec![0, 2]);
        assert_eq!(entry.last_good_index, Some(0));
    }

    #[test]
    fn lb_replaces_state_when_provider_endpoint_base_url_changes() {
        let states = Arc::new(Mutex::new(HashMap::new()));
        let initial = LoadBalancer::new(
            Arc::new(make_provider_endpoint_service(
                "routing",
                &[("https://old.example", "input", "default")],
            )),
            states.clone(),
        );

        {
            let mut guard = states.lock().unwrap();
            let entry = guard
                .entry("routing".to_string())
                .or_insert_with(LbState::default);
            entry.ensure_layout(initial.service.name.as_str(), &initial.service.upstreams);
            entry.failure_counts[0] = FAILURE_THRESHOLD;
            entry.cooldown_until[0] =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(30));
            entry.usage_exhausted[0] = true;
            entry.penalty_streak[0] = 2;
            entry.last_good_index = Some(0);
        }

        let updated = LoadBalancer::new(
            Arc::new(make_provider_endpoint_service(
                "routing",
                &[("https://new.example", "input", "default")],
            )),
            states.clone(),
        );
        let selected = updated
            .select_upstream()
            .expect("new endpoint URL should be selectable after state replacement");
        assert_eq!(selected.index, 0);

        let guard = states.lock().unwrap();
        let entry = guard.get("routing").expect("state exists");
        assert_eq!(entry.failure_counts, vec![0]);
        assert_eq!(entry.cooldown_until, vec![None]);
        assert_eq!(entry.usage_exhausted, vec![false]);
        assert_eq!(entry.penalty_streak, vec![0]);
        assert_eq!(entry.last_good_index, None);
    }

    #[test]
    fn lb_avoids_upstreams_past_failure_threshold() {
        let service = make_service(
            "codex-main",
            &["https://primary.example", "https://backup.example"],
        );
        let states = Arc::new(Mutex::new(HashMap::new()));
        let lb = LoadBalancer::new(Arc::new(service), states.clone());

        let disabled_backoff = CooldownBackoff {
            factor: 1,
            max_secs: 0,
        };

        // 对 primary 连续记录 FAILURE_THRESHOLD 次失败。
        for _ in 0..FAILURE_THRESHOLD {
            lb.record_result_with_backoff(0, false, COOLDOWN_SECS, disabled_backoff);
        }

        // 此时应选择 backup（index 1），因为 index 0 已达到失败阈值。
        let selected = lb
            .select_upstream()
            .expect("should select backup after failures");
        assert_eq!(selected.index, 1);
    }

    #[test]
    fn lb_cooldown_expiry_restores_upstream_selection() {
        let service = make_service(
            "codex-main",
            &["https://primary.example", "https://backup.example"],
        );
        let states = Arc::new(Mutex::new(HashMap::new()));
        let lb = LoadBalancer::new(Arc::new(service), states.clone());

        let disabled_backoff = CooldownBackoff {
            factor: 1,
            max_secs: 0,
        };

        for _ in 0..FAILURE_THRESHOLD {
            lb.record_result_with_backoff(0, false, 2, disabled_backoff);
        }

        {
            let guard = states.lock().unwrap();
            let entry = guard.get("codex-main").expect("lb state exists");
            assert_eq!(entry.failure_counts[0], FAILURE_THRESHOLD);
            assert!(entry.cooldown_until[0].is_some());
        }

        let during_cooldown = lb
            .select_upstream()
            .expect("should select backup while primary cools down");
        assert_eq!(during_cooldown.index, 1);

        {
            let mut guard = states.lock().unwrap();
            let entry = guard.get_mut("codex-main").expect("lb state exists");
            entry.cooldown_until[0] =
                Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
        }

        let recovered = lb
            .select_upstream()
            .expect("should select primary after cooldown expiry");
        assert_eq!(recovered.index, 0);

        {
            let guard = states.lock().unwrap();
            let entry = guard.get("codex-main").expect("lb state exists");
            assert_eq!(entry.failure_counts[0], 0);
            assert!(entry.cooldown_until[0].is_none());
        }
    }

    #[test]
    fn lb_threshold_cooldown_backoff_grows_and_success_resets_streak() {
        let service = make_service(
            "codex-main",
            &["https://primary.example", "https://backup.example"],
        );
        let states = Arc::new(Mutex::new(HashMap::new()));
        let lb = LoadBalancer::new(Arc::new(service), states.clone());

        let backoff = CooldownBackoff {
            factor: 2,
            max_secs: 10,
        };

        for _ in 0..FAILURE_THRESHOLD {
            lb.record_result_with_backoff(0, false, 2, backoff);
        }

        let first_remaining_secs = {
            let guard = states.lock().unwrap();
            let entry = guard.get("codex-main").expect("lb state exists");
            assert_eq!(entry.penalty_streak[0], 1);
            entry.cooldown_until[0]
                .map(|until| {
                    until
                        .saturating_duration_since(std::time::Instant::now())
                        .as_secs()
                })
                .expect("first cooldown exists")
        };
        assert!(first_remaining_secs <= 2);

        {
            let mut guard = states.lock().unwrap();
            let entry = guard.get_mut("codex-main").expect("lb state exists");
            entry.cooldown_until[0] =
                Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
        }
        let _ = lb.select_upstream();

        for _ in 0..FAILURE_THRESHOLD {
            lb.record_result_with_backoff(0, false, 2, backoff);
        }

        let second_remaining_secs = {
            let guard = states.lock().unwrap();
            let entry = guard.get("codex-main").expect("lb state exists");
            assert_eq!(entry.penalty_streak[0], 2);
            entry.cooldown_until[0]
                .map(|until| {
                    until
                        .saturating_duration_since(std::time::Instant::now())
                        .as_secs()
                })
                .expect("second cooldown exists")
        };
        assert!(second_remaining_secs <= 4);
        assert!(second_remaining_secs >= first_remaining_secs);

        lb.record_result_with_backoff(0, true, 2, backoff);

        {
            let guard = states.lock().unwrap();
            let entry = guard.get("codex-main").expect("lb state exists");
            assert_eq!(entry.failure_counts[0], 0);
            assert!(entry.cooldown_until[0].is_none());
            assert_eq!(entry.penalty_streak[0], 0);
            assert_eq!(entry.last_good_index, Some(0));
        }
    }
}

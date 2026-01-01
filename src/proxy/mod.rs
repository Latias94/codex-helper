use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Result, anyhow};
use axum::Json;
use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::Query;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, Uri};
use axum::routing::{any, get};
use reqwest::Client;
use std::sync::OnceLock;
use tracing::{info, instrument, warn};

mod classify;
mod retry;
mod runtime_config;
mod stream;
#[cfg(test)]
mod tests;

use crate::config::{ProxyConfig, RetryStrategy, ServiceConfigManager};
use crate::filter::RequestFilter;
use crate::lb::{LbState, LoadBalancer, SelectedUpstream};
use crate::logging::{
    AuthResolutionLog, BodyPreview, HeaderEntry, HttpDebugLog, http_debug_options,
    http_warn_options, log_request_with_debug, log_retry_trace, make_body_preview,
    should_include_http_debug, should_include_http_warn, should_log_request_body_preview,
};
use crate::model_routing;
use crate::state::{ActiveRequest, FinishedRequest, ProxyState};
use crate::usage::extract_usage_from_bytes;
use crate::usage_providers;

use self::classify::classify_upstream_response;
use self::retry::{
    backoff_sleep, retry_info_for_chain, retry_options, retry_sleep, should_retry_class,
    should_retry_status,
};
use self::runtime_config::RuntimeConfig;
use self::stream::{SseSuccessMeta, build_sse_success_response};

fn lb_state_snapshot_json(lb: &LoadBalancer) -> Option<serde_json::Value> {
    let map = match lb.states.lock() {
        Ok(m) => m,
        Err(e) => e.into_inner(),
    };
    let st = map.get(&lb.service.name)?;
    let now = std::time::Instant::now();
    let upstreams = (0..lb.service.upstreams.len())
        .map(|idx| {
            let cooldown_remaining_ms = st
                .cooldown_until
                .get(idx)
                .and_then(|x| *x)
                .map(|until| until.saturating_duration_since(now).as_millis() as u64)
                .filter(|&ms| ms > 0);
            serde_json::json!({
                "idx": idx,
                "failure_count": st.failure_counts.get(idx).copied(),
                "penalty_streak": st.penalty_streak.get(idx).copied(),
                "usage_exhausted": st.usage_exhausted.get(idx).copied(),
                "cooldown_remaining_ms": cooldown_remaining_ms,
            })
        })
        .collect::<Vec<_>>();
    Some(serde_json::json!({
        "last_good_index": st.last_good_index,
        "upstreams": upstreams,
    }))
}

fn read_json_file(path: &std::path::Path) -> Option<serde_json::Value> {
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    if text.trim().is_empty() {
        return None;
    }
    serde_json::from_str(&text).ok()
}

fn codex_auth_json_value(key: &str) -> Option<String> {
    static CACHE: std::sync::OnceLock<Option<serde_json::Value>> = std::sync::OnceLock::new();
    let v = CACHE.get_or_init(|| read_json_file(&crate::config::codex_auth_path()));
    let obj = v.as_ref()?.as_object()?;
    obj.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

fn claude_settings_env_value(key: &str) -> Option<String> {
    static CACHE: std::sync::OnceLock<Option<serde_json::Value>> = std::sync::OnceLock::new();
    let v = CACHE.get_or_init(|| read_json_file(&crate::config::claude_settings_path()));
    let obj = v.as_ref()?.as_object()?;
    let env_obj = obj.get("env")?.as_object()?;
    env_obj
        .get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

fn resolve_auth_token_with_source(
    service_name: &str,
    auth: &crate::config::UpstreamAuth,
    client_has_auth: bool,
) -> (Option<String>, String) {
    if let Some(token) = auth.auth_token.as_deref()
        && !token.trim().is_empty()
    {
        return (Some(token.to_string()), "inline".to_string());
    }

    if let Some(env_name) = auth.auth_token_env.as_deref()
        && !env_name.trim().is_empty()
    {
        if let Ok(v) = std::env::var(env_name)
            && !v.trim().is_empty()
        {
            return (Some(v), format!("env:{env_name}"));
        }

        let file_value = match service_name {
            "codex" => codex_auth_json_value(env_name),
            "claude" => claude_settings_env_value(env_name),
            _ => None,
        };
        if let Some(v) = file_value
            && !v.trim().is_empty()
        {
            let src = match service_name {
                "codex" => format!("codex_auth_json:{env_name}"),
                "claude" => format!("claude_settings_env:{env_name}"),
                _ => format!("file:{env_name}"),
            };
            return (Some(v), src);
        }

        if client_has_auth {
            return (None, format!("client_passthrough (missing_env:{env_name})"));
        }
        return (None, format!("missing_env:{env_name}"));
    }

    if client_has_auth {
        (None, "client_passthrough".to_string())
    } else {
        (None, "none".to_string())
    }
}

fn resolve_api_key_with_source(
    service_name: &str,
    auth: &crate::config::UpstreamAuth,
    client_has_x_api_key: bool,
) -> (Option<String>, String) {
    if let Some(key) = auth.api_key.as_deref()
        && !key.trim().is_empty()
    {
        return (Some(key.to_string()), "inline".to_string());
    }

    if let Some(env_name) = auth.api_key_env.as_deref()
        && !env_name.trim().is_empty()
    {
        if let Ok(v) = std::env::var(env_name)
            && !v.trim().is_empty()
        {
            return (Some(v), format!("env:{env_name}"));
        }

        let file_value = match service_name {
            "codex" => codex_auth_json_value(env_name),
            "claude" => claude_settings_env_value(env_name),
            _ => None,
        };
        if let Some(v) = file_value
            && !v.trim().is_empty()
        {
            let src = match service_name {
                "codex" => format!("codex_auth_json:{env_name}"),
                "claude" => format!("claude_settings_env:{env_name}"),
                _ => format!("file:{env_name}"),
            };
            return (Some(v), src);
        }

        if client_has_x_api_key {
            return (None, format!("client_passthrough (missing_env:{env_name})"));
        }
        return (None, format!("missing_env:{env_name}"));
    }

    if client_has_x_api_key {
        (None, "client_passthrough".to_string())
    } else {
        (None, "none".to_string())
    }
}

fn is_hop_by_hop_header(name_lower: &str) -> bool {
    matches!(
        name_lower,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn hop_by_hop_connection_tokens(headers: &HeaderMap) -> Vec<String> {
    let mut out = Vec::new();
    for value in headers.get_all("connection").iter() {
        let Ok(s) = value.to_str() else {
            continue;
        };
        for token in s.split(',').map(|t| t.trim()).filter(|t| !t.is_empty()) {
            out.push(token.to_ascii_lowercase());
        }
    }
    out
}

fn filter_request_headers(src: &HeaderMap) -> HeaderMap {
    let extra = hop_by_hop_connection_tokens(src);
    let mut out = HeaderMap::new();
    for (name, value) in src.iter() {
        let name_lower = name.as_str().to_ascii_lowercase();
        if name_lower == "host"
            || name_lower == "content-length"
            || is_hop_by_hop_header(&name_lower)
        {
            continue;
        }
        if extra.iter().any(|t| t == &name_lower) {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

fn filter_response_headers(src: &HeaderMap) -> HeaderMap {
    let extra = hop_by_hop_connection_tokens(src);
    let mut out = HeaderMap::new();
    for (name, value) in src.iter() {
        let name_lower = name.as_str().to_ascii_lowercase();
        // reqwest 可能会自动解压响应体；为避免 content-length/content-encoding 与实际 body 不一致，这里不透传它们。
        if is_hop_by_hop_header(&name_lower)
            || name_lower == "content-length"
            || name_lower == "content-encoding"
        {
            continue;
        }
        if extra.iter().any(|t| t == &name_lower) {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

fn header_map_to_entries(headers: &HeaderMap) -> Vec<HeaderEntry> {
    fn is_sensitive(name_lower: &str) -> bool {
        matches!(
            name_lower,
            "authorization"
                | "proxy-authorization"
                | "cookie"
                | "set-cookie"
                | "x-api-key"
                | "x-forwarded-api-key"
                | "x-goog-api-key"
        )
    }

    let mut out = Vec::new();
    for (name, value) in headers.iter() {
        let name_lower = name.as_str().to_ascii_lowercase();
        let v = if is_sensitive(name_lower.as_str()) {
            "[REDACTED]".to_string()
        } else {
            String::from_utf8_lossy(value.as_bytes()).into_owned()
        };
        out.push(HeaderEntry {
            name: name.as_str().to_string(),
            value: v,
        });
    }
    out
}

#[derive(Clone)]
struct HttpDebugBase {
    debug_max_body_bytes: usize,
    warn_max_body_bytes: usize,
    request_body_len: usize,
    upstream_request_body_len: usize,
    client_uri: String,
    target_url: String,
    client_headers: Vec<HeaderEntry>,
    upstream_request_headers: Vec<HeaderEntry>,
    auth_resolution: Option<AuthResolutionLog>,
    client_body_debug: Option<BodyPreview>,
    upstream_request_body_debug: Option<BodyPreview>,
    client_body_warn: Option<BodyPreview>,
    upstream_request_body_warn: Option<BodyPreview>,
}

fn warn_http_debug(status_code: u16, http_debug: &HttpDebugLog) {
    let max_chars = 2048usize;
    let Ok(mut json) = serde_json::to_string(http_debug) else {
        return;
    };
    if json.chars().count() > max_chars {
        json = json.chars().take(max_chars).collect::<String>() + "...[TRUNCATED_FOR_LOG]";
    }
    warn!("upstream non-2xx http_debug={json} status_code={status_code}");
}

/// Generic proxy service; currently used by both Codex and Claude.
#[derive(Clone)]
pub struct ProxyService {
    pub client: Client,
    config: Arc<RuntimeConfig>,
    pub service_name: &'static str,
    lb_states: Arc<Mutex<HashMap<String, LbState>>>,
    filter: RequestFilter,
    state: Arc<ProxyState>,
}

impl ProxyService {
    pub fn new(
        client: Client,
        config: Arc<ProxyConfig>,
        service_name: &'static str,
        lb_states: Arc<Mutex<HashMap<String, LbState>>>,
    ) -> Self {
        let state = ProxyState::new_with_lb_states(Some(lb_states.clone()));
        ProxyState::spawn_cleanup_task(state.clone());
        {
            let state = state.clone();
            let log_path = crate::config::proxy_home_dir()
                .join("logs")
                .join("requests.jsonl");
            let mut base_url_to_provider_id = HashMap::new();
            let mgr = match service_name {
                "claude" => &config.claude,
                _ => &config.codex,
            };
            for svc in mgr.configs.values() {
                for up in &svc.upstreams {
                    if let Some(pid) = up.tags.get("provider_id") {
                        base_url_to_provider_id.insert(up.base_url.clone(), pid.clone());
                    }
                }
            }
            tokio::spawn(async move {
                let _ = state
                    .replay_usage_from_requests_log(service_name, log_path, base_url_to_provider_id)
                    .await;
            });
        }
        Self {
            client,
            config: Arc::new(RuntimeConfig::new(config)),
            service_name,
            lb_states,
            filter: RequestFilter::new(),
            state,
        }
    }

    fn service_manager<'a>(&self, cfg: &'a ProxyConfig) -> &'a ServiceConfigManager {
        match self.service_name {
            "codex" => &cfg.codex,
            "claude" => &cfg.claude,
            _ => &cfg.codex,
        }
    }

    async fn pinned_config(&self, session_id: Option<&str>) -> Option<(String, &'static str)> {
        if let Some(sid) = session_id
            && let Some(name) = self.state.get_session_config_override(sid).await
            && !name.trim().is_empty()
        {
            return Some((name, "session"));
        }
        if let Some(name) = self.state.get_global_config_override().await
            && !name.trim().is_empty()
        {
            return Some((name, "global"));
        }
        None
    }

    async fn lbs_for_request(
        &self,
        cfg: &ProxyConfig,
        session_id: Option<&str>,
    ) -> Vec<LoadBalancer> {
        let mgr = self.service_manager(cfg);
        let meta_overrides = self
            .state
            .get_config_meta_overrides(self.service_name)
            .await;
        if let Some((name, source)) = self.pinned_config(session_id).await {
            if let Some(svc) = mgr
                .configs
                .get(&name)
                .or_else(|| mgr.active_config())
                .cloned()
            {
                log_retry_trace(serde_json::json!({
                    "event": "lbs_for_request",
                    "service": self.service_name,
                    "session_id": session_id,
                    "mode": "pinned",
                    "pinned_source": source,
                    "pinned_name": name,
                    "selected_config": svc.name,
                    "selected_level": svc.level.clamp(1, 10),
                    "selected_upstreams": svc.upstreams.len(),
                    "active_config": mgr.active.as_deref(),
                    "config_count": mgr.configs.len(),
                }));
                return vec![LoadBalancer::new(Arc::new(svc), self.lb_states.clone())];
            }
            log_retry_trace(serde_json::json!({
                "event": "lbs_for_request",
                "service": self.service_name,
                "session_id": session_id,
                "mode": "pinned",
                "pinned_source": source,
                "pinned_name": name,
                "selected_config": null,
                "active_config": mgr.active.as_deref(),
                "config_count": mgr.configs.len(),
                "note": "pinned_config_not_found",
            }));
            return Vec::new();
        }

        let active_name = mgr.active.as_deref();
        let mut configs = mgr
            .configs
            .iter()
            .filter(|(name, svc)| {
                let (enabled_ovr, _) = meta_overrides
                    .get(name.as_str())
                    .copied()
                    .unwrap_or((None, None));
                let enabled = enabled_ovr.unwrap_or(svc.enabled);
                !svc.upstreams.is_empty()
                    && (enabled || active_name.is_some_and(|n| n == name.as_str()))
            })
            .collect::<Vec<_>>();

        let has_multi_level = {
            let mut levels = configs
                .iter()
                .map(|(name, svc)| {
                    let (_, level_ovr) = meta_overrides
                        .get(name.as_str())
                        .copied()
                        .unwrap_or((None, None));
                    level_ovr.unwrap_or(svc.level).clamp(1, 10)
                })
                .collect::<Vec<_>>();
            levels.sort_unstable();
            levels.dedup();
            levels.len() > 1
        };

        if !has_multi_level {
            let eligible_details = || {
                configs
                    .iter()
                    .map(|(name, svc)| {
                        let (_, level_ovr) = meta_overrides
                            .get(name.as_str())
                            .copied()
                            .unwrap_or((None, None));
                        serde_json::json!({
                            "name": (*name).clone(),
                            "level": level_ovr.unwrap_or(svc.level).clamp(1, 10),
                            "enabled": svc.enabled,
                            "upstreams": svc.upstreams.len(),
                        })
                    })
                    .collect::<Vec<_>>()
            };

            let mut ordered = configs
                .iter()
                .map(|(name, svc)| ((*name).clone(), (*svc).clone()))
                .collect::<Vec<_>>();
            ordered.sort_by(|(a, _), (b, _)| a.cmp(b));
            if let Some(active) = active_name
                && let Some(pos) = ordered.iter().position(|(n, _)| n == active)
            {
                let item = ordered.remove(pos);
                ordered.insert(0, item);
            }

            let lbs = ordered
                .into_iter()
                .map(|(_, svc)| LoadBalancer::new(Arc::new(svc), self.lb_states.clone()))
                .collect::<Vec<_>>();
            if !lbs.is_empty() {
                log_retry_trace(serde_json::json!({
                    "event": "lbs_for_request",
                    "service": self.service_name,
                    "session_id": session_id,
                    "mode": "single_level_multi",
                    "active_config": active_name,
                    "selected_configs": lbs.iter().map(|lb| lb.service.name.clone()).collect::<Vec<_>>(),
                    "eligible_configs": configs.iter().map(|(n, _)| (*n).clone()).collect::<Vec<_>>(),
                    "eligible_details": eligible_details(),
                    "eligible_count": configs.len(),
                }));
                return lbs;
            }

            if let Some(svc) = mgr.active_config().cloned() {
                log_retry_trace(serde_json::json!({
                    "event": "lbs_for_request",
                    "service": self.service_name,
                    "session_id": session_id,
                    "mode": "single_level_fallback_active_config",
                    "active_config": active_name,
                    "selected_config": svc.name,
                    "selected_level": svc.level.clamp(1, 10),
                    "selected_upstreams": svc.upstreams.len(),
                    "eligible_configs": configs.iter().map(|(n, _)| (*n).clone()).collect::<Vec<_>>(),
                    "eligible_details": eligible_details(),
                    "eligible_count": configs.len(),
                }));
                return vec![LoadBalancer::new(Arc::new(svc), self.lb_states.clone())];
            }

            log_retry_trace(serde_json::json!({
                "event": "lbs_for_request",
                "service": self.service_name,
                "session_id": session_id,
                "mode": "single_level_empty",
                "active_config": active_name,
                "eligible_configs": configs.iter().map(|(n, _)| (*n).clone()).collect::<Vec<_>>(),
                "eligible_details": eligible_details(),
                "eligible_count": configs.len(),
            }));
            return Vec::new();
        }

        configs.sort_by(|(a_name, a), (b_name, b)| {
            let a_level = meta_overrides
                .get(a_name.as_str())
                .and_then(|(_, l)| *l)
                .unwrap_or(a.level)
                .clamp(1, 10);
            let b_level = meta_overrides
                .get(b_name.as_str())
                .and_then(|(_, l)| *l)
                .unwrap_or(b.level)
                .clamp(1, 10);
            let a_active = active_name.is_some_and(|n| n == a_name.as_str());
            let b_active = active_name.is_some_and(|n| n == b_name.as_str());
            a_level
                .cmp(&b_level)
                .then_with(|| b_active.cmp(&a_active))
                .then_with(|| a_name.cmp(b_name))
        });

        let lbs = configs
            .into_iter()
            .map(|(_, svc)| LoadBalancer::new(Arc::new(svc.clone()), self.lb_states.clone()))
            .collect::<Vec<_>>();
        if !lbs.is_empty() {
            log_retry_trace(serde_json::json!({
                "event": "lbs_for_request",
                "service": self.service_name,
                "session_id": session_id,
                "mode": "multi_level",
                "active_config": active_name,
                "eligible_configs": lbs.iter().map(|lb| serde_json::json!({
                    "name": lb.service.name,
                    "level": lb.service.level.clamp(1, 10),
                    "upstreams": lb.service.upstreams.len(),
                })).collect::<Vec<_>>(),
                "eligible_count": lbs.len(),
            }));
            return lbs;
        }

        if let Some(svc) = mgr.active_config().cloned() {
            log_retry_trace(serde_json::json!({
                "event": "lbs_for_request",
                "service": self.service_name,
                "session_id": session_id,
                "mode": "multi_level_fallback_active_config",
                "active_config": active_name,
                "selected_config": svc.name,
                "selected_level": svc.level.clamp(1, 10),
                "selected_upstreams": svc.upstreams.len(),
            }));
            return vec![LoadBalancer::new(Arc::new(svc), self.lb_states.clone())];
        }
        log_retry_trace(serde_json::json!({
            "event": "lbs_for_request",
            "service": self.service_name,
            "session_id": session_id,
            "mode": "multi_level_empty",
            "active_config": active_name,
        }));
        Vec::new()
    }

    fn build_target(
        &self,
        upstream: &SelectedUpstream,
        uri: &Uri,
    ) -> Result<(reqwest::Url, HeaderMap)> {
        let base = upstream.upstream.base_url.trim_end_matches('/').to_string();

        let base_url = reqwest::Url::parse(&base)
            .map_err(|e| anyhow!("invalid upstream base_url {base}: {e}"))?;
        let base_path = base_url.path().trim_end_matches('/').to_string();

        let mut path = uri.path().to_string();
        if !base_path.is_empty()
            && base_path != "/"
            && (path == base_path || path.starts_with(&format!("{base_path}/")))
        {
            // If the incoming request path already contains the base_url path prefix,
            // strip it to avoid double-prefixing (e.g. base_url=/v1 and request=/v1/responses).
            let rest = &path[base_path.len()..];
            path = if rest.is_empty() {
                "/".to_string()
            } else {
                rest.to_string()
            };
            if !path.starts_with('/') {
                path = format!("/{path}");
            }
        }
        let path_and_query = if let Some(q) = uri.query() {
            format!("{path}?{q}")
        } else {
            path
        };

        let full = format!("{base}{path_and_query}");
        let url =
            reqwest::Url::parse(&full).map_err(|e| anyhow!("invalid upstream url {full}: {e}"))?;

        // ensure query preserved (Url::parse already includes it)
        let headers = HeaderMap::new();
        Ok((url, headers))
    }

    pub fn state_handle(&self) -> Arc<ProxyState> {
        self.state.clone()
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn extract_session_id(headers: &HeaderMap) -> Option<String> {
    header_str(headers, "session_id")
        .or_else(|| header_str(headers, "conversation_id"))
        .map(|s| s.to_string())
}

fn extract_reasoning_effort_from_request_body(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("reasoning")
        .and_then(|r| r.get("effort"))
        .and_then(|e| e.as_str())
        .map(|s| s.to_string())
}

fn extract_model_from_request_body(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("model")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
}

fn apply_reasoning_effort_override(body: &[u8], effort: &str) -> Option<Vec<u8>> {
    let mut v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let reasoning = v.get_mut("reasoning").and_then(|r| r.as_object_mut());
    if let Some(obj) = reasoning {
        obj.insert(
            "effort".to_string(),
            serde_json::Value::String(effort.to_string()),
        );
    } else {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "effort".to_string(),
            serde_json::Value::String(effort.to_string()),
        );
        v.as_object_mut()?
            .insert("reasoning".to_string(), serde_json::Value::Object(obj));
    }
    serde_json::to_vec(&v).ok()
}

fn apply_model_override(body: &[u8], model: &str) -> Option<Vec<u8>> {
    let mut v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.as_object_mut()?.insert(
        "model".to_string(),
        serde_json::Value::String(model.to_string()),
    );
    serde_json::to_vec(&v).ok()
}

#[instrument(skip_all, fields(service = %proxy.service_name))]
pub async fn handle_proxy(
    proxy: ProxyService,
    req: Request<Body>,
) -> Result<Response<Body>, (StatusCode, String)> {
    let start = Instant::now();
    let started_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let (parts, body) = req.into_parts();
    let uri = parts.uri;
    let method = parts.method;
    let client_headers = parts.headers;
    let client_headers_entries_cache: OnceLock<Vec<HeaderEntry>> = OnceLock::new();

    let session_id = extract_session_id(&client_headers);

    proxy.config.maybe_reload_from_disk().await;
    let cfg_snapshot = proxy.config.snapshot().await;
    let lbs = proxy
        .lbs_for_request(cfg_snapshot.as_ref(), session_id.as_deref())
        .await;
    if lbs.is_empty() {
        let dur = start.elapsed().as_millis() as u64;
        let status = StatusCode::BAD_GATEWAY;
        let client_headers_entries = client_headers_entries_cache
            .get_or_init(|| header_map_to_entries(&client_headers))
            .clone();
        let http_debug = if should_include_http_warn(status.as_u16()) {
            Some(HttpDebugLog {
                request_body_len: None,
                upstream_request_body_len: None,
                upstream_headers_ms: None,
                upstream_first_chunk_ms: None,
                upstream_body_read_ms: None,
                upstream_error_class: Some("no_active_upstream_config".to_string()),
                upstream_error_hint: Some(
                    "未找到任何可用的上游配置（active_config 为空或 upstreams 为空）。".to_string(),
                ),
                upstream_cf_ray: None,
                client_uri: uri.to_string(),
                target_url: "-".to_string(),
                client_headers: client_headers_entries,
                upstream_request_headers: Vec::new(),
                auth_resolution: None,
                client_body: None,
                upstream_request_body: None,
                upstream_response_headers: None,
                upstream_response_body: None,
                upstream_error: Some("no active upstream config".to_string()),
            })
        } else {
            None
        };
        log_request_with_debug(
            proxy.service_name,
            method.as_str(),
            uri.path(),
            status.as_u16(),
            dur,
            None,
            "-",
            None,
            "-",
            session_id.clone(),
            None,
            None,
            None,
            None,
            http_debug,
        );
        return Err((status, "no active upstream config".to_string()));
    }
    let client_content_type = client_headers
        .get("content-type")
        .and_then(|v| v.to_str().ok());

    // Detect streaming (SSE).
    let is_stream = client_headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.contains("text/event-stream"))
        .unwrap_or(false);

    let path = uri.path();
    let is_responses_path = path.ends_with("/responses");
    let is_user_turn = method == Method::POST && is_responses_path;
    let is_codex_service = proxy.service_name == "codex";

    let cwd = if let Some(id) = session_id.as_deref() {
        proxy.state.resolve_session_cwd(id).await
    } else {
        None
    };
    if let Some(id) = session_id.as_deref() {
        proxy.state.touch_session_override(id, started_at_ms).await;
        proxy
            .state
            .touch_session_config_override(id, started_at_ms)
            .await;
    }

    // Read request body and apply filters.
    let raw_body = match to_bytes(body, 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            let dur = start.elapsed().as_millis() as u64;
            let status = StatusCode::BAD_REQUEST;
            let err_str = e.to_string();
            let client_headers_entries = client_headers_entries_cache
                .get_or_init(|| header_map_to_entries(&client_headers))
                .clone();
            let http_debug = if should_include_http_warn(status.as_u16()) {
                Some(HttpDebugLog {
                    request_body_len: None,
                    upstream_request_body_len: None,
                    upstream_headers_ms: None,
                    upstream_first_chunk_ms: None,
                    upstream_body_read_ms: None,
                    upstream_error_class: Some("client_body_read_error".to_string()),
                    upstream_error_hint: Some(
                        "读取客户端请求 body 失败（可能超过大小限制或连接中断）。".to_string(),
                    ),
                    upstream_cf_ray: None,
                    client_uri: uri.to_string(),
                    target_url: "-".to_string(),
                    client_headers: client_headers_entries,
                    upstream_request_headers: Vec::new(),
                    auth_resolution: None,
                    client_body: None,
                    upstream_request_body: None,
                    upstream_response_headers: None,
                    upstream_response_body: None,
                    upstream_error: Some(err_str.clone()),
                })
            } else {
                None
            };
            log_request_with_debug(
                proxy.service_name,
                method.as_str(),
                uri.path(),
                status.as_u16(),
                dur,
                None,
                "-",
                None,
                "-",
                session_id.clone(),
                cwd.clone(),
                None,
                None,
                None,
                http_debug,
            );
            return Err((status, err_str));
        }
    };
    let original_effort = extract_reasoning_effort_from_request_body(&raw_body);
    let override_effort = if let Some(id) = session_id.as_deref() {
        proxy.state.get_session_effort_override(id).await
    } else {
        None
    };
    let effective_effort = override_effort.clone().or(original_effort.clone());

    let body_for_upstream = if let Some(ref effort) = override_effort {
        Bytes::from(
            apply_reasoning_effort_override(&raw_body, effort)
                .unwrap_or_else(|| raw_body.as_ref().to_vec()),
        )
    } else {
        raw_body.clone()
    };
    let request_model = extract_model_from_request_body(body_for_upstream.as_ref());
    let request_body_len = raw_body.len();

    let debug_opt = http_debug_options();
    let warn_opt = http_warn_options();
    let debug_max = if debug_opt.enabled {
        debug_opt.max_body_bytes
    } else {
        0
    };
    let warn_max = if warn_opt.enabled {
        warn_opt.max_body_bytes
    } else {
        0
    };
    let request_body_previews = should_log_request_body_preview();
    let client_body_debug = if request_body_previews && debug_max > 0 {
        Some(make_body_preview(&raw_body, client_content_type, debug_max))
    } else {
        None
    };
    let client_body_warn = if request_body_previews && warn_max > 0 {
        Some(make_body_preview(&raw_body, client_content_type, warn_max))
    } else {
        None
    };

    let request_id = proxy
        .state
        .begin_request(
            proxy.service_name,
            method.as_str(),
            uri.path(),
            session_id.clone(),
            cwd.clone(),
            request_model.clone(),
            effective_effort.clone(),
            started_at_ms,
        )
        .await;

    let retry_cfg = cfg_snapshot.retry.resolve();
    let retry_opt = retry_options(&retry_cfg);
    let retry_failover = retry_opt.strategy == RetryStrategy::Failover;
    let cooldown_backoff = crate::lb::CooldownBackoff {
        factor: retry_opt.cooldown_backoff_factor,
        max_secs: retry_opt.cooldown_backoff_max_secs,
    };
    log_retry_trace(serde_json::json!({
        "event": "retry_options",
        "service": proxy.service_name,
        "request_id": request_id,
        "max_attempts": retry_opt.max_attempts,
        "base_backoff_ms": retry_opt.base_backoff_ms,
        "max_backoff_ms": retry_opt.max_backoff_ms,
        "jitter_ms": retry_opt.jitter_ms,
        "retry_status_ranges": retry_opt.retry_status_ranges,
        "retry_error_classes": retry_opt.retry_error_classes,
        "strategy": if retry_failover { "failover" } else { "same_upstream" },
        "cloudflare_challenge_cooldown_secs": retry_opt.cloudflare_challenge_cooldown_secs,
        "cloudflare_timeout_cooldown_secs": retry_opt.cloudflare_timeout_cooldown_secs,
        "transport_cooldown_secs": retry_opt.transport_cooldown_secs,
        "cooldown_backoff_factor": retry_opt.cooldown_backoff_factor,
        "cooldown_backoff_max_secs": retry_opt.cooldown_backoff_max_secs,
    }));
    let total_upstreams = lbs
        .iter()
        .map(|lb| lb.service.upstreams.len())
        .sum::<usize>();
    let mut avoid: HashMap<String, HashSet<usize>> = HashMap::new();
    let mut upstream_chain: Vec<String> = Vec::new();

    for attempt_index in 0..retry_opt.max_attempts {
        let avoided_total = avoid.values().map(|s| s.len()).sum::<usize>();
        if total_upstreams > 0 && avoided_total >= total_upstreams {
            upstream_chain.push(format!("all_upstreams_avoided total={total_upstreams}"));
            break;
        }

        let strict_multi_config = lbs.len() > 1;

        let mut chosen: Option<(LoadBalancer, SelectedUpstream)> = None;
        for lb in &lbs {
            let cfg_name = lb.service.name.clone();
            let avoid_set = avoid.entry(cfg_name.clone()).or_default();
            loop {
                let upstream_total = lb.service.upstreams.len();
                if upstream_total > 0 && avoid_set.len() >= upstream_total {
                    break;
                }
                let next = {
                    let avoid_ref: &HashSet<usize> = &*avoid_set;
                    if strict_multi_config {
                        lb.select_upstream_avoiding_strict(avoid_ref)
                    } else {
                        lb.select_upstream_avoiding(avoid_ref)
                    }
                };
                let Some(selected) = next else {
                    break;
                };

                if let Some(ref requested_model) = request_model {
                    let supported = model_routing::is_model_supported(
                        &selected.upstream.supported_models,
                        &selected.upstream.model_mapping,
                        requested_model,
                    );
                    if !supported {
                        upstream_chain.push(format!(
                            "{}:{} (idx={}) skipped_unsupported_model={}",
                            selected.config_name,
                            selected.upstream.base_url,
                            selected.index,
                            requested_model
                        ));
                        avoid_set.insert(selected.index);
                        continue;
                    }
                }

                chosen = Some((lb.clone(), selected));
                break;
            }
            if chosen.is_some() {
                break;
            }
        }

        // When we have multiple config candidates, prefer skipping configs that are fully cooled down.
        // However, if *all* configs are cooled/unavailable, fall back to the original "always pick one"
        // behavior to avoid a hard outage.
        if chosen.is_none() && strict_multi_config {
            for lb in &lbs {
                let cfg_name = lb.service.name.clone();
                let avoid_set = avoid.entry(cfg_name.clone()).or_default();
                let upstream_total = lb.service.upstreams.len();
                if upstream_total > 0 && avoid_set.len() >= upstream_total {
                    continue;
                }
                let avoid_ref: &HashSet<usize> = &*avoid_set;
                if let Some(selected) = lb.select_upstream_avoiding(avoid_ref) {
                    chosen = Some((lb.clone(), selected));
                    break;
                }
            }
        }

        let Some((lb, selected)) = chosen else {
            log_retry_trace(serde_json::json!({
                "event": "attempt_no_upstream",
                "service": proxy.service_name,
                "request_id": request_id,
                "attempt": attempt_index + 1,
                "max_attempts": retry_opt.max_attempts,
                "avoid": avoid.iter().map(|(k, v)| serde_json::json!({
                    "config": k,
                    "indices": v.iter().copied().collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "total_upstreams": total_upstreams,
                "model": request_model.clone(),
            }));
            let dur = start.elapsed().as_millis() as u64;
            let status = if request_model.is_some() {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_GATEWAY
            };
            log_request_with_debug(
                proxy.service_name,
                method.as_str(),
                uri.path(),
                status.as_u16(),
                dur,
                None,
                "-",
                None,
                "-",
                session_id.clone(),
                cwd.clone(),
                effective_effort.clone(),
                None,
                retry_info_for_chain(&upstream_chain),
                None,
            );
            let retry = retry_info_for_chain(&upstream_chain);
            proxy
                .state
                .finish_request(crate::state::FinishRequestParams {
                    id: request_id,
                    status_code: status.as_u16(),
                    duration_ms: dur,
                    ended_at_ms: started_at_ms + dur,
                    usage: None,
                    retry,
                    ttfb_ms: None,
                })
                .await;
            if let Some(model) = request_model.as_deref() {
                return Err((
                    status,
                    format!("no upstreams support requested model '{model}'"),
                ));
            }
            return Err((status, "no upstreams available".to_string()));
        };

        let selected_config_name = selected.config_name.clone();
        let selected_upstream_index = selected.index;
        let selected_upstream_base_url = selected.upstream.base_url.clone();
        let provider_id = selected.upstream.tags.get("provider_id").cloned();
        log_retry_trace(serde_json::json!({
            "event": "attempt_select",
            "service": proxy.service_name,
            "request_id": request_id,
            "attempt": attempt_index + 1,
            "max_attempts": retry_opt.max_attempts,
            "strategy": if retry_failover { "failover" } else { "same_upstream" },
            "config_name": selected_config_name,
            "upstream_index": selected_upstream_index,
            "upstream_base_url": selected_upstream_base_url,
            "provider_id": provider_id.clone(),
            "model": request_model.clone(),
            "lb_state": lb_state_snapshot_json(&lb),
            "avoid_for_config": avoid.get(&selected.config_name).map(|s| s.iter().copied().collect::<Vec<_>>()),
            "avoided_total": avoid.values().map(|s| s.len()).sum::<usize>(),
            "total_upstreams": total_upstreams,
        }));

        let mut model_note = "-".to_string();
        let mut body_for_selected = body_for_upstream.clone();
        if let Some(ref requested_model) = request_model {
            let effective_model =
                model_routing::effective_model(&selected.upstream.model_mapping, requested_model);
            if effective_model != *requested_model {
                if let Some(modified) =
                    apply_model_override(body_for_upstream.as_ref(), effective_model.as_str())
                {
                    body_for_selected = Bytes::from(modified);
                }
                model_note = format!("{requested_model}->{effective_model}");
            } else {
                model_note = requested_model.clone();
            }
        }

        let filtered_body = proxy.filter.apply_bytes(body_for_selected);
        let upstream_request_body_len = filtered_body.len();
        let upstream_request_body_debug = if request_body_previews && debug_max > 0 {
            Some(make_body_preview(
                &filtered_body,
                client_content_type,
                debug_max,
            ))
        } else {
            None
        };
        let upstream_request_body_warn = if request_body_previews && warn_max > 0 {
            Some(make_body_preview(
                &filtered_body,
                client_content_type,
                warn_max,
            ))
        } else {
            None
        };

        let target_url = match proxy.build_target(&selected, &uri) {
            Ok((url, _headers)) => url,
            Err(e) => {
                lb.record_result_with_backoff(
                    selected.index,
                    false,
                    crate::lb::COOLDOWN_SECS,
                    cooldown_backoff,
                );
                let err_str = e.to_string();
                upstream_chain.push(format!(
                    "{}:{} (idx={}) target_build_error={} model={}",
                    selected.config_name,
                    selected.upstream.base_url,
                    selected.index,
                    err_str,
                    model_note.as_str()
                ));
                avoid
                    .entry(selected.config_name.clone())
                    .or_default()
                    .insert(selected.index);

                let can_retry = attempt_index + 1 < retry_opt.max_attempts;
                if can_retry {
                    backoff_sleep(&retry_opt, attempt_index).await;
                    continue;
                }

                let dur = start.elapsed().as_millis() as u64;
                let status = StatusCode::BAD_GATEWAY;
                let client_headers_entries = client_headers_entries_cache
                    .get_or_init(|| header_map_to_entries(&client_headers))
                    .clone();
                let http_debug = if should_include_http_warn(status.as_u16()) {
                    Some(HttpDebugLog {
                        request_body_len: Some(request_body_len),
                        upstream_request_body_len: Some(upstream_request_body_len),
                        upstream_headers_ms: None,
                        upstream_first_chunk_ms: None,
                        upstream_body_read_ms: None,
                        upstream_error_class: Some("target_build_error".to_string()),
                        upstream_error_hint: Some(
                            "构造上游 target_url 失败（通常是 base_url 配置错误）。".to_string(),
                        ),
                        upstream_cf_ray: None,
                        client_uri: uri.to_string(),
                        target_url: "-".to_string(),
                        client_headers: client_headers_entries,
                        upstream_request_headers: Vec::new(),
                        auth_resolution: None,
                        client_body: client_body_warn.clone(),
                        upstream_request_body: upstream_request_body_warn.clone(),
                        upstream_response_headers: None,
                        upstream_response_body: None,
                        upstream_error: Some(err_str.clone()),
                    })
                } else {
                    None
                };
                log_request_with_debug(
                    proxy.service_name,
                    method.as_str(),
                    uri.path(),
                    status.as_u16(),
                    dur,
                    None,
                    &selected.config_name,
                    selected.upstream.tags.get("provider_id").cloned(),
                    &selected.upstream.base_url,
                    session_id.clone(),
                    cwd.clone(),
                    effective_effort.clone(),
                    None,
                    retry_info_for_chain(&upstream_chain),
                    http_debug,
                );
                let retry = retry_info_for_chain(&upstream_chain);
                proxy
                    .state
                    .finish_request(crate::state::FinishRequestParams {
                        id: request_id,
                        status_code: status.as_u16(),
                        duration_ms: dur,
                        ended_at_ms: started_at_ms + dur,
                        usage: None,
                        retry,
                        ttfb_ms: None,
                    })
                    .await;
                return Err((status, err_str));
            }
        };

        // copy headers, stripping host/content-length and hop-by-hop.
        // auth headers:
        // - if upstream config provides a token/key, override client values;
        // - otherwise, preserve client Authorization / X-API-Key (required for requires_openai_auth=true providers).
        let mut headers = filter_request_headers(&client_headers);
        let client_has_auth = headers.contains_key("authorization");
        let (token, token_src) = resolve_auth_token_with_source(
            proxy.service_name,
            &selected.upstream.auth,
            client_has_auth,
        );
        if let Some(token) = token
            && let Ok(v) = HeaderValue::from_str(&format!("Bearer {token}"))
        {
            headers.insert(HeaderName::from_static("authorization"), v);
        }

        let client_has_x_api_key = headers.contains_key("x-api-key");
        let (api_key, api_key_src) = resolve_api_key_with_source(
            proxy.service_name,
            &selected.upstream.auth,
            client_has_x_api_key,
        );
        if let Some(key) = api_key
            && let Ok(v) = HeaderValue::from_str(&key)
        {
            headers.insert(HeaderName::from_static("x-api-key"), v);
        }

        let upstream_request_headers = headers.clone();
        proxy
            .state
            .update_request_route(
                request_id,
                selected.config_name.clone(),
                provider_id.clone(),
                selected.upstream.base_url.clone(),
            )
            .await;
        let auth_resolution = AuthResolutionLog {
            authorization: Some(token_src),
            x_api_key: Some(api_key_src),
        };

        let debug_base = if debug_max > 0 || warn_max > 0 {
            Some(HttpDebugBase {
                debug_max_body_bytes: debug_max,
                warn_max_body_bytes: warn_max,
                request_body_len,
                upstream_request_body_len,
                client_uri: uri.to_string(),
                target_url: target_url.to_string(),
                client_headers: client_headers_entries_cache
                    .get_or_init(|| header_map_to_entries(&client_headers))
                    .clone(),
                upstream_request_headers: header_map_to_entries(&upstream_request_headers),
                auth_resolution: Some(auth_resolution),
                client_body_debug: client_body_debug.clone(),
                upstream_request_body_debug: upstream_request_body_debug.clone(),
                client_body_warn: client_body_warn.clone(),
                upstream_request_body_warn: upstream_request_body_warn.clone(),
            })
        } else {
            None
        };

        // 详细转发日志仅在 debug 级别输出，避免刷屏。
        tracing::debug!(
            "forwarding {} {} to {} ({})",
            method,
            uri.path(),
            target_url,
            selected.config_name
        );

        let builder = proxy
            .client
            .request(method.clone(), target_url.clone())
            .headers(headers)
            .body(filtered_body.clone());

        let upstream_start = Instant::now();
        let resp = match builder.send().await {
            Ok(r) => r,
            Err(e) => {
                log_retry_trace(serde_json::json!({
                    "event": "attempt_transport_error",
                    "service": proxy.service_name,
                    "request_id": request_id,
                    "attempt": attempt_index + 1,
                    "max_attempts": retry_opt.max_attempts,
                    "strategy": if retry_failover { "failover" } else { "same_upstream" },
                    "config_name": selected.config_name.as_str(),
                    "upstream_index": selected.index,
                    "upstream_base_url": selected.upstream.base_url.as_str(),
                    "provider_id": provider_id.as_deref(),
                    "error": e.to_string(),
                }));
                if retry_failover {
                    lb.record_result_with_backoff(
                        selected.index,
                        false,
                        crate::lb::COOLDOWN_SECS,
                        cooldown_backoff,
                    );
                }
                let err_str = e.to_string();
                upstream_chain.push(format!(
                    "{}:{} (idx={}) transport_error={} model={}",
                    selected.config_name,
                    selected.upstream.base_url,
                    selected.index,
                    err_str,
                    model_note.as_str()
                ));
                let can_retry = attempt_index + 1 < retry_opt.max_attempts
                    && should_retry_class(&retry_opt, Some("upstream_transport_error"));
                if can_retry {
                    if retry_failover {
                        lb.penalize_with_backoff(
                            selected.index,
                            retry_opt.transport_cooldown_secs,
                            "upstream_transport_error",
                            cooldown_backoff,
                        );
                        avoid
                            .entry(selected.config_name.clone())
                            .or_default()
                            .insert(selected.index);
                    }
                    backoff_sleep(&retry_opt, attempt_index).await;
                    continue;
                }

                // Even when we have no remaining in-request retries, mark this upstream as cooled down
                // so external retries (e.g. Codex request_max_retries) can fail over to another upstream.
                if should_retry_class(&retry_opt, Some("upstream_transport_error")) {
                    lb.penalize_with_backoff(
                        selected.index,
                        retry_opt.transport_cooldown_secs,
                        "upstream_transport_error_final",
                        cooldown_backoff,
                    );
                }

                let dur = start.elapsed().as_millis() as u64;
                let status_code = StatusCode::BAD_GATEWAY.as_u16();
                let upstream_headers_ms = upstream_start.elapsed().as_millis() as u64;
                let retry = retry_info_for_chain(&upstream_chain);
                let http_debug_warn = debug_base.as_ref().and_then(|b| {
                    if b.warn_max_body_bytes == 0 {
                        return None;
                    }
                    Some(HttpDebugLog {
                        request_body_len: Some(b.request_body_len),
                        upstream_request_body_len: Some(b.upstream_request_body_len),
                        upstream_headers_ms: Some(upstream_headers_ms),
                        upstream_first_chunk_ms: None,
                        upstream_body_read_ms: None,
                        upstream_error_class: Some("upstream_transport_error".to_string()),
                        upstream_error_hint: Some(
                            "上游连接/发送请求失败（reqwest 错误）；请检查网络、DNS、TLS、代理设置或上游可用性。".to_string(),
                        ),
                        upstream_cf_ray: None,
                        client_uri: b.client_uri.clone(),
                        target_url: b.target_url.clone(),
                        client_headers: b.client_headers.clone(),
                        upstream_request_headers: b.upstream_request_headers.clone(),
                        auth_resolution: b.auth_resolution.clone(),
                        client_body: b.client_body_warn.clone(),
                        upstream_request_body: b.upstream_request_body_warn.clone(),
                        upstream_response_headers: None,
                        upstream_response_body: None,
                        upstream_error: Some(err_str.clone()),
                    })
                });
                if should_include_http_warn(status_code)
                    && let Some(h) = http_debug_warn.as_ref()
                {
                    warn_http_debug(status_code, h);
                }
                let http_debug = if should_include_http_debug(status_code) {
                    debug_base.as_ref().and_then(|b| {
                        if b.debug_max_body_bytes == 0 {
                            return None;
                        }
                        Some(HttpDebugLog {
                            request_body_len: Some(b.request_body_len),
                            upstream_request_body_len: Some(b.upstream_request_body_len),
                            upstream_headers_ms: Some(upstream_headers_ms),
                            upstream_first_chunk_ms: None,
                            upstream_body_read_ms: None,
                            upstream_error_class: Some("upstream_transport_error".to_string()),
                            upstream_error_hint: Some(
                                "上游连接/发送请求失败（reqwest 错误）；请检查网络、DNS、TLS、代理设置或上游可用性。".to_string(),
                            ),
                            upstream_cf_ray: None,
                            client_uri: b.client_uri.clone(),
                            target_url: b.target_url.clone(),
                            client_headers: b.client_headers.clone(),
                            upstream_request_headers: b.upstream_request_headers.clone(),
                            auth_resolution: b.auth_resolution.clone(),
                            client_body: b.client_body_debug.clone(),
                            upstream_request_body: b.upstream_request_body_debug.clone(),
                            upstream_response_headers: None,
                            upstream_response_body: None,
                            upstream_error: Some(err_str.clone()),
                        })
                    })
                } else if should_include_http_warn(status_code) {
                    http_debug_warn.clone()
                } else {
                    None
                };
                log_request_with_debug(
                    proxy.service_name,
                    method.as_str(),
                    uri.path(),
                    status_code,
                    dur,
                    None,
                    &selected.config_name,
                    selected.upstream.tags.get("provider_id").cloned(),
                    &selected.upstream.base_url,
                    session_id.clone(),
                    cwd.clone(),
                    effective_effort.clone(),
                    None,
                    retry.clone(),
                    http_debug,
                );
                proxy
                    .state
                    .finish_request(crate::state::FinishRequestParams {
                        id: request_id,
                        status_code,
                        duration_ms: dur,
                        ended_at_ms: started_at_ms + dur,
                        usage: None,
                        retry,
                        ttfb_ms: None,
                    })
                    .await;
                return Err((StatusCode::BAD_GATEWAY, e.to_string()));
            }
        };

        let upstream_headers_ms = upstream_start.elapsed().as_millis() as u64;
        let status = resp.status();
        let success = status.is_success();
        let resp_headers = resp.headers().clone();
        let resp_headers_filtered = filter_response_headers(&resp_headers);

        // 对用户对话轮次输出更有信息量的 info 日志（仅最终返回时打印，避免重试期间刷屏）。

        if is_stream && success {
            lb.record_result_with_backoff(
                selected.index,
                true,
                crate::lb::COOLDOWN_SECS,
                cooldown_backoff,
            );
            upstream_chain.push(format!(
                "{} (idx={}) status={} model={}",
                selected.upstream.base_url,
                selected.index,
                status.as_u16(),
                model_note.as_str()
            ));
            let retry = retry_info_for_chain(&upstream_chain);

            return Ok(build_sse_success_response(
                &proxy,
                lb.clone(),
                selected,
                resp,
                SseSuccessMeta {
                    status,
                    resp_headers,
                    resp_headers_filtered,
                    start,
                    started_at_ms,
                    upstream_start,
                    upstream_headers_ms,
                    request_body_len,
                    upstream_request_body_len,
                    debug_base,
                    retry,
                    session_id: session_id.clone(),
                    cwd: cwd.clone(),
                    effective_effort: effective_effort.clone(),
                    request_id,
                    is_user_turn,
                    is_codex_service,
                    transport_cooldown_secs: retry_opt.transport_cooldown_secs,
                    cooldown_backoff,
                    method: method.clone(),
                    path: uri.path().to_string(),
                },
            )
            .await);
        } else {
            let bytes = match resp.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    log_retry_trace(serde_json::json!({
                        "event": "attempt_body_read_error",
                        "service": proxy.service_name,
                        "request_id": request_id,
                        "attempt": attempt_index + 1,
                        "max_attempts": retry_opt.max_attempts,
                        "strategy": if retry_failover { "failover" } else { "same_upstream" },
                        "config_name": selected.config_name.as_str(),
                        "upstream_index": selected.index,
                        "upstream_base_url": selected.upstream.base_url.as_str(),
                        "provider_id": provider_id.as_deref(),
                        "error": e.to_string(),
                    }));
                    if retry_failover {
                        lb.record_result_with_backoff(
                            selected.index,
                            false,
                            crate::lb::COOLDOWN_SECS,
                            cooldown_backoff,
                        );
                    }
                    let err_str = e.to_string();
                    upstream_chain.push(format!(
                        "{}:{} (idx={}) body_read_error={} model={}",
                        selected.config_name,
                        selected.upstream.base_url,
                        selected.index,
                        err_str,
                        model_note.as_str()
                    ));
                    let can_retry = attempt_index + 1 < retry_opt.max_attempts
                        && should_retry_class(&retry_opt, Some("upstream_transport_error"));
                    if can_retry {
                        if retry_failover {
                            lb.penalize_with_backoff(
                                selected.index,
                                retry_opt.transport_cooldown_secs,
                                "upstream_body_read_error",
                                cooldown_backoff,
                            );
                            avoid
                                .entry(selected.config_name.clone())
                                .or_default()
                                .insert(selected.index);
                        }
                        backoff_sleep(&retry_opt, attempt_index).await;
                        continue;
                    }

                    // Same reasoning as transport errors: without in-request retries, external retries
                    // should not get stuck repeatedly selecting the same broken upstream.
                    if should_retry_class(&retry_opt, Some("upstream_transport_error")) {
                        lb.penalize_with_backoff(
                            selected.index,
                            retry_opt.transport_cooldown_secs,
                            "upstream_body_read_error_final",
                            cooldown_backoff,
                        );
                    }

                    let dur = start.elapsed().as_millis() as u64;
                    let status = StatusCode::BAD_GATEWAY;
                    let http_debug = if should_include_http_warn(status.as_u16())
                        && let Some(b) = debug_base.as_ref()
                    {
                        Some(HttpDebugLog {
                            request_body_len: Some(b.request_body_len),
                            upstream_request_body_len: Some(b.upstream_request_body_len),
                            upstream_headers_ms: Some(upstream_headers_ms),
                            upstream_first_chunk_ms: None,
                            upstream_body_read_ms: None,
                            upstream_error_class: Some("upstream_transport_error".to_string()),
                            upstream_error_hint: Some(
                                "读取上游响应 body 失败（连接中断/解码错误等）；可视为传输错误。"
                                    .to_string(),
                            ),
                            upstream_cf_ray: None,
                            client_uri: b.client_uri.clone(),
                            target_url: b.target_url.clone(),
                            client_headers: b.client_headers.clone(),
                            upstream_request_headers: b.upstream_request_headers.clone(),
                            auth_resolution: b.auth_resolution.clone(),
                            client_body: b.client_body_warn.clone(),
                            upstream_request_body: b.upstream_request_body_warn.clone(),
                            upstream_response_headers: Some(header_map_to_entries(&resp_headers)),
                            upstream_response_body: None,
                            upstream_error: Some(err_str.clone()),
                        })
                    } else {
                        None
                    };
                    log_request_with_debug(
                        proxy.service_name,
                        method.as_str(),
                        uri.path(),
                        status.as_u16(),
                        dur,
                        Some(upstream_headers_ms),
                        &selected.config_name,
                        selected.upstream.tags.get("provider_id").cloned(),
                        &selected.upstream.base_url,
                        session_id.clone(),
                        cwd.clone(),
                        effective_effort.clone(),
                        None,
                        retry_info_for_chain(&upstream_chain),
                        http_debug,
                    );
                    let retry = retry_info_for_chain(&upstream_chain);
                    proxy
                        .state
                        .finish_request(crate::state::FinishRequestParams {
                            id: request_id,
                            status_code: status.as_u16(),
                            duration_ms: dur,
                            ended_at_ms: started_at_ms + dur,
                            usage: None,
                            retry,
                            ttfb_ms: Some(upstream_headers_ms),
                        })
                        .await;
                    return Err((status, err_str));
                }
            };
            let upstream_body_read_ms = upstream_start.elapsed().as_millis() as u64;
            let dur = start.elapsed().as_millis() as u64;
            let usage = extract_usage_from_bytes(&bytes);
            let status_code = status.as_u16();
            let (cls, hint, cf_ray) =
                classify_upstream_response(status_code, &resp_headers, bytes.as_ref());

            upstream_chain.push(format!(
                "{} (idx={}) status={} class={} model={}",
                selected.upstream.base_url,
                selected.index,
                status_code,
                cls.as_deref().unwrap_or("-"),
                model_note.as_str()
            ));

            // If this looks like a transient / retryable upstream failure, but we have no remaining
            // in-request retries, proactively cool down this upstream so the next external retry
            // (Codex/app-level) can fail over to other upstreams.
            if !success
                && attempt_index + 1 >= retry_opt.max_attempts
                && (should_retry_status(&retry_opt, status_code)
                    || should_retry_class(&retry_opt, cls.as_deref()))
            {
                log_retry_trace(serde_json::json!({
                    "event": "attempt_final_retryable_failure",
                    "service": proxy.service_name,
                    "request_id": request_id,
                    "attempt": attempt_index + 1,
                    "max_attempts": retry_opt.max_attempts,
                    "status_code": status_code,
                    "class": cls.as_deref(),
                    "hint": hint.as_deref(),
                    "cf_ray": cf_ray.as_deref(),
                    "config_name": selected.config_name.as_str(),
                    "upstream_index": selected.index,
                    "upstream_base_url": selected.upstream.base_url.as_str(),
                    "provider_id": provider_id.as_deref(),
                    "should_retry_status": should_retry_status(&retry_opt, status_code),
                    "should_retry_class": should_retry_class(&retry_opt, cls.as_deref()),
                }));
                match cls.as_deref() {
                    Some("cloudflare_challenge") => lb.penalize_with_backoff(
                        selected.index,
                        retry_opt.cloudflare_challenge_cooldown_secs,
                        "cloudflare_challenge_final",
                        cooldown_backoff,
                    ),
                    Some("cloudflare_timeout") => lb.penalize_with_backoff(
                        selected.index,
                        retry_opt.cloudflare_timeout_cooldown_secs,
                        "cloudflare_timeout_final",
                        cooldown_backoff,
                    ),
                    _ if status_code >= 500 => lb.penalize_with_backoff(
                        selected.index,
                        retry_opt.transport_cooldown_secs,
                        &format!("status_{}_final", status_code),
                        cooldown_backoff,
                    ),
                    _ => {}
                }
            }

            let retryable = !status.is_success()
                && attempt_index + 1 < retry_opt.max_attempts
                && (should_retry_status(&retry_opt, status_code)
                    || should_retry_class(&retry_opt, cls.as_deref()));
            if retryable {
                log_retry_trace(serde_json::json!({
                    "event": "attempt_retryable_failure",
                    "service": proxy.service_name,
                    "request_id": request_id,
                    "attempt": attempt_index + 1,
                    "next_attempt": attempt_index + 2,
                    "max_attempts": retry_opt.max_attempts,
                    "status_code": status_code,
                    "class": cls.as_deref(),
                    "hint": hint.as_deref(),
                    "cf_ray": cf_ray.as_deref(),
                    "config_name": selected.config_name.as_str(),
                    "upstream_index": selected.index,
                    "upstream_base_url": selected.upstream.base_url.as_str(),
                    "provider_id": provider_id.as_deref(),
                    "strategy": if retry_failover { "failover" } else { "same_upstream" },
                    "retry_after": resp_headers.get("retry-after").and_then(|v| v.to_str().ok()),
                    "should_retry_status": should_retry_status(&retry_opt, status_code),
                    "should_retry_class": should_retry_class(&retry_opt, cls.as_deref()),
                }));
                let cls_s = cls.as_deref().unwrap_or("-");
                info!(
                    "retrying after non-2xx status {} (class={}) for {} {} (config: {}, mode={}, next_attempt={}/{})",
                    status_code,
                    cls_s,
                    method,
                    uri.path(),
                    selected.config_name,
                    if retry_failover {
                        "failover"
                    } else {
                        "same_upstream"
                    },
                    attempt_index + 2,
                    retry_opt.max_attempts
                );
                if retry_failover {
                    // Treat retryable 5xx / WAF-like responses as upstream failures for LB tracking.
                    if status_code >= 500 || cls.is_some() {
                        lb.record_result_with_backoff(
                            selected.index,
                            false,
                            crate::lb::COOLDOWN_SECS,
                            cooldown_backoff,
                        );
                    }
                    match cls.as_deref() {
                        Some("cloudflare_challenge") => lb.penalize_with_backoff(
                            selected.index,
                            retry_opt.cloudflare_challenge_cooldown_secs,
                            "cloudflare_challenge",
                            cooldown_backoff,
                        ),
                        Some("cloudflare_timeout") => lb.penalize_with_backoff(
                            selected.index,
                            retry_opt.cloudflare_timeout_cooldown_secs,
                            "cloudflare_timeout",
                            cooldown_backoff,
                        ),
                        _ if status_code >= 500 => lb.penalize_with_backoff(
                            selected.index,
                            retry_opt.transport_cooldown_secs,
                            &format!("status_{}", status_code),
                            cooldown_backoff,
                        ),
                        _ => {}
                    }
                    avoid
                        .entry(selected.config_name.clone())
                        .or_default()
                        .insert(selected.index);
                }
                retry_sleep(&retry_opt, attempt_index, &resp_headers).await;
                continue;
            }

            // Update LB state (final attempt):
            // - 2xx => success
            // - transport / 5xx / classified WAF failures => failure
            // - generic 3xx/4xx => neutral (do not mark upstream good/bad to avoid sticky routing to a failing upstream,
            //   and also avoid penalizing upstreams for client-side mistakes).
            if success {
                lb.record_result_with_backoff(
                    selected.index,
                    true,
                    crate::lb::COOLDOWN_SECS,
                    cooldown_backoff,
                );
            } else if status_code >= 500 || cls.is_some() {
                lb.record_result_with_backoff(
                    selected.index,
                    false,
                    crate::lb::COOLDOWN_SECS,
                    cooldown_backoff,
                );
            }

            let retry = retry_info_for_chain(&upstream_chain);

            if is_user_turn {
                let provider_id = selected
                    .upstream
                    .tags
                    .get("provider_id")
                    .map(|s| s.as_str())
                    .unwrap_or("-");
                info!(
                    "user turn {} {} using config '{}' upstream[{}] provider_id='{}' base_url='{}'",
                    method,
                    uri.path(),
                    selected.config_name,
                    selected.index,
                    provider_id,
                    selected.upstream.base_url
                );
            }

            let http_debug_warn = if should_include_http_warn(status_code)
                && let Some(b) = debug_base.as_ref()
            {
                let max = b.warn_max_body_bytes;
                let resp_ct = resp_headers
                    .get("content-type")
                    .and_then(|v| v.to_str().ok());
                Some(HttpDebugLog {
                    request_body_len: Some(b.request_body_len),
                    upstream_request_body_len: Some(b.upstream_request_body_len),
                    upstream_headers_ms: Some(upstream_headers_ms),
                    upstream_first_chunk_ms: None,
                    upstream_body_read_ms: Some(upstream_body_read_ms),
                    upstream_error_class: cls.clone(),
                    upstream_error_hint: hint.clone(),
                    upstream_cf_ray: cf_ray.clone(),
                    client_uri: b.client_uri.clone(),
                    target_url: b.target_url.clone(),
                    client_headers: b.client_headers.clone(),
                    upstream_request_headers: b.upstream_request_headers.clone(),
                    auth_resolution: b.auth_resolution.clone(),
                    client_body: b.client_body_warn.clone(),
                    upstream_request_body: b.upstream_request_body_warn.clone(),
                    upstream_response_headers: Some(header_map_to_entries(&resp_headers)),
                    upstream_response_body: Some(make_body_preview(bytes.as_ref(), resp_ct, max)),
                    upstream_error: None,
                })
            } else {
                None
            };

            if !status.is_success() {
                if let Some(h) = http_debug_warn.as_ref() {
                    warn_http_debug(status_code, h);
                } else {
                    let cls_s = cls.as_deref().unwrap_or("-");
                    let cf_ray_s = cf_ray.as_deref().unwrap_or("-");
                    warn!(
                        "upstream returned non-2xx status {} (class={}, cf_ray={}) for {} {} (config: {}); set CODEX_HELPER_HTTP_WARN=0 to disable preview logs (or CODEX_HELPER_HTTP_DEBUG=1 for full debug)",
                        status_code,
                        cls_s,
                        cf_ray_s,
                        method,
                        uri.path(),
                        selected.config_name
                    );
                }
            }

            let http_debug = if should_include_http_debug(status_code) {
                debug_base.map(|b| {
                    let max = b.debug_max_body_bytes;
                    let resp_ct = resp_headers
                        .get("content-type")
                        .and_then(|v| v.to_str().ok());
                    HttpDebugLog {
                        request_body_len: Some(b.request_body_len),
                        upstream_request_body_len: Some(b.upstream_request_body_len),
                        upstream_headers_ms: Some(upstream_headers_ms),
                        upstream_first_chunk_ms: None,
                        upstream_body_read_ms: Some(upstream_body_read_ms),
                        upstream_error_class: cls,
                        upstream_error_hint: hint,
                        upstream_cf_ray: cf_ray,
                        client_uri: b.client_uri,
                        target_url: b.target_url,
                        client_headers: b.client_headers,
                        upstream_request_headers: b.upstream_request_headers,
                        auth_resolution: b.auth_resolution,
                        client_body: b.client_body_debug,
                        upstream_request_body: b.upstream_request_body_debug,
                        upstream_response_headers: Some(header_map_to_entries(&resp_headers)),
                        upstream_response_body: Some(make_body_preview(
                            bytes.as_ref(),
                            resp_ct,
                            max,
                        )),
                        upstream_error: None,
                    }
                })
            } else if should_include_http_warn(status_code) {
                http_debug_warn.clone()
            } else {
                None
            };

            log_request_with_debug(
                proxy.service_name,
                method.as_str(),
                uri.path(),
                status_code,
                dur,
                Some(upstream_headers_ms),
                &selected.config_name,
                selected.upstream.tags.get("provider_id").cloned(),
                &selected.upstream.base_url,
                session_id.clone(),
                cwd.clone(),
                effective_effort.clone(),
                usage.clone(),
                retry.clone(),
                http_debug,
            );
            proxy
                .state
                .finish_request(crate::state::FinishRequestParams {
                    id: request_id,
                    status_code,
                    duration_ms: dur,
                    ended_at_ms: started_at_ms + dur,
                    usage: usage.clone(),
                    retry,
                    ttfb_ms: Some(upstream_headers_ms),
                })
                .await;

            // Poll usage once after a user request finishes (e.g. packycode), used to drive auto-switching.
            if is_user_turn && is_codex_service {
                usage_providers::poll_for_codex_upstream(
                    cfg_snapshot.clone(),
                    proxy.lb_states.clone(),
                    &selected.config_name,
                    selected.index,
                )
                .await;
            }

            let mut builder = Response::builder().status(status);
            for (name, value) in resp_headers_filtered.iter() {
                builder = builder.header(name, value);
            }
            return Ok(builder.body(Body::from(bytes)).unwrap());
        }
    }

    let dur = start.elapsed().as_millis() as u64;
    let status = StatusCode::BAD_GATEWAY;
    let http_debug = if should_include_http_warn(status.as_u16()) {
        let client_headers_entries = client_headers_entries_cache
            .get_or_init(|| header_map_to_entries(&client_headers))
            .clone();
        Some(HttpDebugLog {
            request_body_len: Some(request_body_len),
            upstream_request_body_len: None,
            upstream_headers_ms: None,
            upstream_first_chunk_ms: None,
            upstream_body_read_ms: None,
            upstream_error_class: Some("retry_exhausted".to_string()),
            upstream_error_hint: Some("所有重试尝试均未能返回可用响应。".to_string()),
            upstream_cf_ray: None,
            client_uri: uri.to_string(),
            target_url: "-".to_string(),
            client_headers: client_headers_entries,
            upstream_request_headers: Vec::new(),
            auth_resolution: None,
            client_body: client_body_warn.clone(),
            upstream_request_body: None,
            upstream_response_headers: None,
            upstream_response_body: None,
            upstream_error: Some(format!(
                "retry attempts exhausted; chain={:?}",
                upstream_chain
            )),
        })
    } else {
        None
    };
    log_request_with_debug(
        proxy.service_name,
        method.as_str(),
        uri.path(),
        status.as_u16(),
        dur,
        None,
        "-",
        None,
        "-",
        session_id.clone(),
        cwd.clone(),
        effective_effort.clone(),
        None,
        retry_info_for_chain(&upstream_chain),
        http_debug,
    );
    let retry = retry_info_for_chain(&upstream_chain);
    proxy
        .state
        .finish_request(crate::state::FinishRequestParams {
            id: request_id,
            status_code: status.as_u16(),
            duration_ms: dur,
            ended_at_ms: started_at_ms + dur,
            usage: None,
            retry,
            ttfb_ms: None,
        })
        .await;
    Err((status, "retry attempts exhausted".to_string()))
}

pub fn router(proxy: ProxyService) -> Router {
    // In axum 0.8, wildcard segments use `/{*path}` (equivalent to `/*path` from axum 0.7).
    #[derive(serde::Deserialize)]
    struct SessionOverrideRequest {
        session_id: String,
        effort: Option<String>,
    }

    #[derive(serde::Serialize)]
    struct RuntimeConfigStatus {
        config_path: String,
        loaded_at_ms: u64,
        source_mtime_ms: Option<u64>,
        retry: crate::config::ResolvedRetryConfig,
    }

    #[derive(serde::Serialize)]
    struct ReloadResult {
        reloaded: bool,
        status: RuntimeConfigStatus,
    }

    async fn runtime_config_status(
        proxy: ProxyService,
    ) -> Result<Json<RuntimeConfigStatus>, (StatusCode, String)> {
        let cfg = proxy.config.snapshot().await;
        Ok(Json(RuntimeConfigStatus {
            config_path: crate::config::config_file_path().display().to_string(),
            loaded_at_ms: proxy.config.last_loaded_at_ms(),
            source_mtime_ms: proxy.config.last_mtime_ms().await,
            retry: cfg.retry.resolve(),
        }))
    }

    async fn reload_runtime_config(
        proxy: ProxyService,
    ) -> Result<Json<ReloadResult>, (StatusCode, String)> {
        let changed = proxy
            .config
            .force_reload_from_disk()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let cfg = proxy.config.snapshot().await;
        Ok(Json(ReloadResult {
            reloaded: changed,
            status: RuntimeConfigStatus {
                config_path: crate::config::config_file_path().display().to_string(),
                loaded_at_ms: proxy.config.last_loaded_at_ms(),
                source_mtime_ms: proxy.config.last_mtime_ms().await,
                retry: cfg.retry.resolve(),
            },
        }))
    }

    async fn set_session_override(
        proxy: ProxyService,
        Json(payload): Json<SessionOverrideRequest>,
    ) -> Result<StatusCode, (StatusCode, String)> {
        if payload.session_id.trim().is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "session_id is required".to_string(),
            ));
        }
        if let Some(effort) = payload.effort {
            if effort.trim().is_empty() {
                return Err((StatusCode::BAD_REQUEST, "effort is empty".to_string()));
            }
            proxy
                .state
                .set_session_effort_override(
                    payload.session_id,
                    effort,
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0),
                )
                .await;
        } else {
            proxy
                .state
                .clear_session_effort_override(payload.session_id.as_str())
                .await;
        }
        Ok(StatusCode::NO_CONTENT)
    }

    async fn list_session_overrides(
        proxy: ProxyService,
    ) -> Result<Json<std::collections::HashMap<String, String>>, (StatusCode, String)> {
        let map = proxy.state.list_session_effort_overrides().await;
        Ok(Json(map))
    }

    async fn list_active_requests(
        proxy: ProxyService,
    ) -> Result<Json<Vec<ActiveRequest>>, (StatusCode, String)> {
        let vec = proxy.state.list_active_requests().await;
        Ok(Json(vec))
    }

    #[derive(serde::Deserialize)]
    struct RecentQuery {
        limit: Option<usize>,
    }

    async fn list_recent_finished(
        proxy: ProxyService,
        Query(q): Query<RecentQuery>,
    ) -> Result<Json<Vec<FinishedRequest>>, (StatusCode, String)> {
        let limit = q.limit.unwrap_or(50).clamp(1, 200);
        let vec = proxy.state.list_recent_finished(limit).await;
        Ok(Json(vec))
    }

    let p0 = proxy.clone();
    let p1 = proxy.clone();
    let p2 = proxy.clone();
    let p3 = proxy.clone();
    let p4 = proxy.clone();
    let p5 = proxy.clone();
    let p6 = proxy.clone();
    let p7 = proxy.clone();

    Router::new()
        .route(
            "/__codex_helper/override/session",
            get(move || list_session_overrides(p0.clone()))
                .post(move |payload| set_session_override(p1.clone(), payload)),
        )
        .route(
            "/__codex_helper/config/runtime",
            get(move || runtime_config_status(p5.clone())),
        )
        .route(
            "/__codex_helper/config/reload",
            get(move || runtime_config_status(p6.clone()))
                .post(move || reload_runtime_config(p7.clone())),
        )
        .route(
            "/__codex_helper/status/active",
            get(move || list_active_requests(p3.clone())),
        )
        .route(
            "/__codex_helper/status/recent",
            get(move |q| list_recent_finished(p4.clone(), q)),
        )
        .route("/{*path}", any(move |req| handle_proxy(p2.clone(), req)))
}

use axum::http::HeaderMap;
use rand::RngExt;
use tokio::time::sleep;

use crate::config::ReasoningGuardAction;
use crate::config::ResolvedReasoningGuardConfig;
use crate::config::ResolvedRetryConfig;
use crate::config::ResolvedRetryLayerConfig;
use crate::config::RetryStrategy;
use crate::logging::{RetryInfo, RouteAttemptLog};

use super::classify::{UPSTREAM_OVERLOADED_CLASS, UPSTREAM_RATE_LIMITED_CLASS};
use super::reasoning_guard::REASONING_GUARD_TRIGGERED_CLASS;

#[derive(Clone)]
pub(super) struct RetryLayerOptions {
    pub(super) max_attempts: u32,
    pub(super) base_backoff_ms: u64,
    pub(super) max_backoff_ms: u64,
    pub(super) jitter_ms: u64,
    pub(super) retry_status_ranges: Vec<(u16, u16)>,
    pub(super) retry_error_classes: Vec<String>,
    pub(super) strategy: RetryStrategy,
}

#[derive(Clone)]
pub(super) struct RetryPlan {
    pub(super) upstream: RetryLayerOptions,
    pub(super) route: RetryLayerOptions,
    pub(super) reasoning_guard: ResolvedReasoningGuardConfig,
    pub(super) never_status_ranges: Vec<(u16, u16)>,
    pub(super) never_error_classes: Vec<String>,
    pub(super) cloudflare_challenge_cooldown_secs: u64,
    pub(super) cloudflare_timeout_cooldown_secs: u64,
    pub(super) transport_cooldown_secs: u64,
    pub(super) cooldown_backoff_factor: u64,
    pub(super) cooldown_backoff_max_secs: u64,
}

pub(super) fn parse_status_ranges(spec: &str) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    for raw in spec.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if let Some((a, b)) = raw.split_once('-') {
            let (Ok(start), Ok(end)) = (a.trim().parse::<u16>(), b.trim().parse::<u16>()) else {
                continue;
            };
            out.push((start.min(end), start.max(end)));
        } else if let Ok(code) = raw.parse::<u16>() {
            out.push((code, code));
        }
    }
    out
}

fn layer_options(cfg: &ResolvedRetryLayerConfig) -> RetryLayerOptions {
    let max_attempts = cfg.max_attempts.clamp(1, 8);
    let base_backoff_ms = cfg.backoff_ms;
    let max_backoff_ms = cfg.backoff_max_ms;
    let jitter_ms = cfg.jitter_ms;
    let retry_status_ranges = parse_status_ranges(cfg.on_status.as_str());
    let retry_error_classes = cfg.on_class.clone();
    let strategy = cfg.strategy;

    RetryLayerOptions {
        max_attempts,
        base_backoff_ms,
        max_backoff_ms,
        jitter_ms,
        retry_status_ranges,
        retry_error_classes,
        strategy,
    }
}

pub(super) fn retry_plan(cfg: &ResolvedRetryConfig) -> RetryPlan {
    let mut upstream = layer_options(&cfg.upstream);
    let mut route = layer_options(&cfg.route);
    if cfg.reasoning_guard.enabled
        && cfg.reasoning_guard.action == ReasoningGuardAction::Retry
        && cfg.reasoning_guard.max_guard_retries > 0
    {
        push_retry_class_once(
            &mut upstream.retry_error_classes,
            REASONING_GUARD_TRIGGERED_CLASS,
        );
        push_retry_class_once(
            &mut route.retry_error_classes,
            REASONING_GUARD_TRIGGERED_CLASS,
        );
    }
    let never_status_ranges = parse_status_ranges(cfg.never_on_status.as_str());
    let never_error_classes = cfg.never_on_class.clone();
    let cloudflare_challenge_cooldown_secs = cfg.cloudflare_challenge_cooldown_secs;
    let cloudflare_timeout_cooldown_secs = cfg.cloudflare_timeout_cooldown_secs;
    let transport_cooldown_secs = cfg.transport_cooldown_secs;
    let cooldown_backoff_factor = cfg.cooldown_backoff_factor.clamp(1, 16);
    let cooldown_backoff_max_secs = cfg.cooldown_backoff_max_secs.clamp(0, 24 * 60 * 60);

    RetryPlan {
        upstream,
        route,
        reasoning_guard: cfg.reasoning_guard.clone(),
        never_status_ranges,
        never_error_classes,
        cloudflare_challenge_cooldown_secs,
        cloudflare_timeout_cooldown_secs,
        transport_cooldown_secs,
        cooldown_backoff_factor,
        cooldown_backoff_max_secs,
    }
}

fn push_retry_class_once(classes: &mut Vec<String>, class: &str) {
    if !classes.iter().any(|existing| existing == class) {
        classes.push(class.to_string());
    }
}

pub(super) fn retry_info_for_observed_attempts(
    route_attempts: &[RouteAttemptLog],
) -> Option<RetryInfo> {
    retry_info_from_route_attempts(route_attempts)
}

pub(super) fn retry_info_for_failed_attempts(
    route_attempts: &[RouteAttemptLog],
) -> Option<RetryInfo> {
    retry_info_from_route_attempts(route_attempts)
}

fn retry_info_from_route_attempts(route_attempts: &[RouteAttemptLog]) -> Option<RetryInfo> {
    if route_attempts.is_empty() {
        return None;
    }

    Some(RetryInfo {
        attempts: observed_attempt_count(route_attempts),
        route_attempts: route_attempts.to_vec(),
    })
}

fn observed_attempt_count(route_attempts: &[RouteAttemptLog]) -> u32 {
    route_attempts
        .iter()
        .filter(|attempt| !attempt.skipped && attempt.decision != "all_upstreams_avoided")
        .count() as u32
}

pub(super) fn should_retry_status(opt: &RetryLayerOptions, status_code: u16) -> bool {
    opt.retry_status_ranges
        .iter()
        .any(|(a, b)| status_code >= *a && status_code <= *b)
}

pub(super) fn should_retry_class(opt: &RetryLayerOptions, class: Option<&str>) -> bool {
    let Some(c) = class else {
        return false;
    };
    opt.retry_error_classes.iter().any(|x| x == c)
}

pub(super) fn should_never_retry_status(plan: &RetryPlan, status_code: u16) -> bool {
    plan.never_status_ranges
        .iter()
        .any(|(a, b)| status_code >= *a && status_code <= *b)
}

pub(super) fn should_never_retry_class(plan: &RetryPlan, class: Option<&str>) -> bool {
    let Some(c) = class else {
        return false;
    };
    plan.never_error_classes.iter().any(|x| x == c)
}

/// Effective guardrail decision:
/// - `never_on_class` always wins.
/// - `never_on_status` is a guardrail for unclassified / client-ish errors, but is allowed to be
///   overridden by an explicit `on_class` match on either retry layer (e.g. Cloudflare/WAF HTML
///   challenge pages may return status 400).
pub(super) fn should_never_retry(plan: &RetryPlan, status_code: u16, class: Option<&str>) -> bool {
    if should_never_retry_class(plan, class) {
        return true;
    }
    if !should_never_retry_status(plan, status_code) {
        return false;
    }

    let class_is_explicitly_retryable =
        should_retry_class(&plan.upstream, class) || should_retry_class(&plan.route, class);
    !class_is_explicitly_retryable
}

pub(super) fn response_penalty_cooldown_secs(
    cloudflare_challenge_cooldown_secs: u64,
    cloudflare_timeout_cooldown_secs: u64,
    transport_cooldown_secs: u64,
    class: Option<&str>,
    retry_after_secs: Option<u64>,
) -> u64 {
    match class {
        Some("cloudflare_challenge") => cloudflare_challenge_cooldown_secs,
        Some("cloudflare_timeout") => cloudflare_timeout_cooldown_secs,
        Some(UPSTREAM_RATE_LIMITED_CLASS) => retry_after_secs
            .filter(|value| *value > 0)
            .unwrap_or(transport_cooldown_secs),
        Some(UPSTREAM_OVERLOADED_CLASS) => retry_after_secs
            .filter(|value| *value > 0)
            .map(|value| value.max(transport_cooldown_secs))
            .unwrap_or(transport_cooldown_secs),
        _ => transport_cooldown_secs,
    }
}

fn retry_after_ms(headers: &HeaderMap, opt: &RetryLayerOptions) -> Option<u64> {
    let raw = headers.get("retry-after")?.to_str().ok()?.trim();
    if raw.is_empty() {
        return None;
    }
    let seconds = raw.parse::<u64>().ok()?;
    let ms = seconds.saturating_mul(1000);
    let cap = opt.max_backoff_ms.max(opt.base_backoff_ms);
    Some(ms.min(cap))
}

fn retry_after_ms_from_secs(secs: u64, opt: &RetryLayerOptions) -> Option<u64> {
    if secs == 0 {
        return None;
    }
    let ms = secs.saturating_mul(1000);
    let cap = opt.max_backoff_ms.max(opt.base_backoff_ms);
    Some(ms.min(cap))
}

pub(super) async fn backoff_sleep(opt: &RetryLayerOptions, attempt_index: u32) {
    if opt.base_backoff_ms == 0 {
        return;
    }
    let pow = 1u64 << attempt_index.min(20);
    let base = opt.base_backoff_ms.saturating_mul(pow);
    let capped = base.min(opt.max_backoff_ms.max(opt.base_backoff_ms));
    let jitter = if opt.jitter_ms == 0 {
        0
    } else {
        rand::rng().random_range(0..=opt.jitter_ms)
    };
    sleep(std::time::Duration::from_millis(
        capped.saturating_add(jitter),
    ))
    .await;
}

pub(super) async fn retry_sleep(
    opt: &RetryLayerOptions,
    attempt_index: u32,
    resp_headers: &HeaderMap,
    retry_after_secs: Option<u64>,
) {
    if let Some(mut ms) = retry_after_secs
        .and_then(|secs| retry_after_ms_from_secs(secs, opt))
        .or_else(|| retry_after_ms(resp_headers, opt))
    {
        if opt.jitter_ms > 0 {
            let jitter = rand::rng().random_range(0..=opt.jitter_ms);
            let cap = opt.max_backoff_ms.max(opt.base_backoff_ms);
            ms = ms.saturating_add(jitter).min(cap);
        }
        if ms > 0 {
            sleep(std::time::Duration::from_millis(ms)).await;
        }
        return;
    }
    backoff_sleep(opt, attempt_index).await;
}

#[cfg(test)]
mod tests {
    use super::super::classify::CLIENT_ERROR_NON_RETRYABLE_CLASS;
    use super::*;

    use crate::config::{ReasoningGuardConfig, RetryProfileName};
    use axum::http::HeaderValue;
    use pretty_assertions::assert_eq;

    #[test]
    fn parse_status_ranges_accepts_single_codes_and_ranges() {
        assert_eq!(
            parse_status_ranges("429,500-599"),
            vec![(429, 429), (500, 599)]
        );
    }

    #[test]
    fn retry_after_ms_parses_seconds_and_caps() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("10"));
        let opt = RetryLayerOptions {
            max_attempts: 3,
            base_backoff_ms: 200,
            max_backoff_ms: 2_000,
            jitter_ms: 0,
            retry_status_ranges: vec![(429, 429)],
            retry_error_classes: Vec::new(),
            strategy: RetryStrategy::Failover,
        };
        assert_eq!(retry_after_ms(&headers, &opt), Some(2_000));
    }

    #[test]
    fn retry_info_attempts_excludes_skipped_route_decisions() {
        let route_attempts = vec![
            RouteAttemptLog {
                attempt_index: 0,
                decision: "failed_status".to_string(),
                status_code: Some(502),
                ..Default::default()
            },
            RouteAttemptLog {
                attempt_index: 1,
                decision: "failed_status".to_string(),
                status_code: Some(502),
                ..Default::default()
            },
            RouteAttemptLog {
                attempt_index: 2,
                decision: "all_upstreams_avoided".to_string(),
                skipped: true,
                ..Default::default()
            },
        ];
        let info = retry_info_for_observed_attempts(&route_attempts).unwrap();
        assert_eq!(info.attempts, 2);
        assert_eq!(info.route_attempts, route_attempts);
    }

    #[test]
    fn retry_info_prefers_structured_route_attempts() {
        let route_attempts = vec![
            RouteAttemptLog {
                attempt_index: 0,
                provider_id: Some("provider-a".to_string()),
                provider_attempt: Some(1),
                provider_max_attempts: Some(2),
                upstream_attempt: Some(1),
                upstream_max_attempts: Some(1),
                endpoint_id: Some("default".to_string()),
                provider_endpoint_key: Some("codex/provider-a/default".to_string()),
                decision: "failed_status".to_string(),
                status_code: Some(502),
                ..Default::default()
            },
            RouteAttemptLog {
                attempt_index: 1,
                provider_id: Some("provider-b".to_string()),
                provider_attempt: Some(2),
                provider_max_attempts: Some(2),
                upstream_attempt: Some(1),
                upstream_max_attempts: Some(1),
                endpoint_id: Some("default".to_string()),
                provider_endpoint_key: Some("codex/provider-b/default".to_string()),
                decision: "completed".to_string(),
                status_code: Some(200),
                upstream_headers_ms: Some(42),
                duration_ms: Some(100),
                ..Default::default()
            },
        ];

        let info = retry_info_for_observed_attempts(&route_attempts).unwrap();

        assert_eq!(info.attempts, 2);
        assert_eq!(info.route_attempts, route_attempts);
        assert_eq!(
            info.route_attempts[1].provider_id.as_deref(),
            Some("provider-b")
        );
        assert_eq!(info.route_attempts[1].upstream_headers_ms, Some(42));
    }

    #[test]
    fn retry_info_preserves_skipped_route_decisions_without_counting_them_as_attempts() {
        let route_attempts = vec![RouteAttemptLog {
            attempt_index: 0,
            provider_id: Some("provider-a".to_string()),
            decision: "route_unavailable".to_string(),
            skipped: true,
            ..Default::default()
        }];

        let info = retry_info_for_failed_attempts(&route_attempts).unwrap();

        assert_eq!(info.attempts, 0);
        assert_eq!(info.route_attempts, route_attempts);
    }

    #[test]
    fn retry_info_requires_structured_route_decisions() {
        assert!(retry_info_for_observed_attempts(&[]).is_none());
        assert!(retry_info_for_failed_attempts(&[]).is_none());
    }

    #[test]
    fn failed_retry_info_keeps_single_structured_attempt_for_logs() {
        let route_attempts = vec![RouteAttemptLog {
            attempt_index: 0,
            decision: "failed_status".to_string(),
            status_code: Some(502),
            error_class: Some("upstream_server_error".to_string()),
            model: Some("gpt".to_string()),
            ..Default::default()
        }];
        let info = retry_info_for_failed_attempts(&route_attempts).unwrap();

        assert_eq!(info.attempts, 1);
        assert_eq!(info.route_attempts, route_attempts);
        assert_eq!(info.route_attempts[0].decision, "failed_status");
        assert_eq!(info.route_attempts[0].status_code, Some(502));
    }

    #[test]
    fn failed_retry_info_is_none_without_any_route_decision() {
        assert!(retry_info_for_failed_attempts(&[]).is_none());
    }

    #[test]
    fn should_never_retry_allows_on_class_to_override_never_on_status() {
        let resolved = RetryProfileName::Balanced.defaults();
        let plan = retry_plan(&resolved);

        // Default guardrail no longer blocks raw 400 by status alone.
        assert!(!should_never_retry(&plan, 400, None));

        // But Cloudflare/WAF challenge pages may still be retryable even if they are 400.
        assert!(!should_never_retry(
            &plan,
            400,
            Some("cloudflare_challenge")
        ));

        // Explicitly non-retryable client-side mistakes should remain blocked.
        assert!(should_never_retry(
            &plan,
            400,
            Some(CLIENT_ERROR_NON_RETRYABLE_CLASS)
        ));
    }

    #[test]
    fn retry_plan_adds_reasoning_guard_class_only_when_retry_enabled() {
        let mut resolved = RetryProfileName::Balanced.defaults();
        resolved.reasoning_guard = ReasoningGuardConfig {
            enabled: Some(true),
            ..ReasoningGuardConfig::default()
        }
        .resolve();

        let plan = retry_plan(&resolved);

        assert!(
            plan.upstream
                .retry_error_classes
                .iter()
                .any(|class| class == REASONING_GUARD_TRIGGERED_CLASS)
        );
        assert!(
            plan.route
                .retry_error_classes
                .iter()
                .any(|class| class == REASONING_GUARD_TRIGGERED_CLASS)
        );
    }
}

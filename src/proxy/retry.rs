use axum::http::HeaderMap;
use rand::Rng;
use tokio::time::sleep;

use crate::config::RetryConfig;
use crate::config::RetryStrategy;
use crate::logging::RetryInfo;

#[derive(Clone)]
pub(super) struct RetryOptions {
    pub(super) max_attempts: u32,
    pub(super) base_backoff_ms: u64,
    pub(super) max_backoff_ms: u64,
    pub(super) jitter_ms: u64,
    pub(super) retry_status_ranges: Vec<(u16, u16)>,
    pub(super) retry_error_classes: Vec<String>,
    pub(super) strategy: RetryStrategy,
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

pub(super) fn retry_options(cfg: &RetryConfig) -> RetryOptions {
    let max_attempts = cfg.max_attempts.clamp(1, 8);
    let base_backoff_ms = cfg.backoff_ms;
    let max_backoff_ms = cfg.backoff_max_ms;
    let jitter_ms = cfg.jitter_ms;
    let retry_status_ranges = parse_status_ranges(cfg.on_status.as_str());
    let retry_error_classes = cfg.on_class.clone();
    let strategy = cfg.strategy;
    let cloudflare_challenge_cooldown_secs = cfg.cloudflare_challenge_cooldown_secs;
    let cloudflare_timeout_cooldown_secs = cfg.cloudflare_timeout_cooldown_secs;
    let transport_cooldown_secs = cfg.transport_cooldown_secs;
    let cooldown_backoff_factor = cfg.cooldown_backoff_factor.clamp(1, 16);
    let cooldown_backoff_max_secs = cfg.cooldown_backoff_max_secs.clamp(0, 24 * 60 * 60);

    RetryOptions {
        max_attempts,
        base_backoff_ms,
        max_backoff_ms,
        jitter_ms,
        retry_status_ranges,
        retry_error_classes,
        strategy,
        cloudflare_challenge_cooldown_secs,
        cloudflare_timeout_cooldown_secs,
        transport_cooldown_secs,
        cooldown_backoff_factor,
        cooldown_backoff_max_secs,
    }
}

pub(super) fn retry_info_for_chain(chain: &[String]) -> Option<RetryInfo> {
    let mut attempts = chain.len() as u32;
    if chain
        .last()
        .is_some_and(|s| s.starts_with("all_upstreams_avoided"))
    {
        attempts = attempts.saturating_sub(1);
    }

    if attempts <= 1 {
        return None;
    }
    Some(RetryInfo {
        attempts,
        upstream_chain: chain.to_vec(),
    })
}

pub(super) fn should_retry_status(opt: &RetryOptions, status_code: u16) -> bool {
    opt.retry_status_ranges
        .iter()
        .any(|(a, b)| status_code >= *a && status_code <= *b)
}

pub(super) fn should_retry_class(opt: &RetryOptions, class: Option<&str>) -> bool {
    let Some(c) = class else {
        return false;
    };
    opt.retry_error_classes.iter().any(|x| x == c)
}

fn retry_after_ms(headers: &HeaderMap, opt: &RetryOptions) -> Option<u64> {
    let raw = headers.get("retry-after")?.to_str().ok()?.trim();
    if raw.is_empty() {
        return None;
    }
    let seconds = raw.parse::<u64>().ok()?;
    let ms = seconds.saturating_mul(1000);
    let cap = opt.max_backoff_ms.max(opt.base_backoff_ms);
    Some(ms.min(cap))
}

pub(super) async fn backoff_sleep(opt: &RetryOptions, attempt_index: u32) {
    if opt.base_backoff_ms == 0 {
        return;
    }
    let pow = 1u64 << attempt_index.min(20);
    let base = opt.base_backoff_ms.saturating_mul(pow);
    let capped = base.min(opt.max_backoff_ms.max(opt.base_backoff_ms));
    let jitter = if opt.jitter_ms == 0 {
        0
    } else {
        rand::thread_rng().gen_range(0..=opt.jitter_ms)
    };
    sleep(std::time::Duration::from_millis(
        capped.saturating_add(jitter),
    ))
    .await;
}

pub(super) async fn retry_sleep(opt: &RetryOptions, attempt_index: u32, resp_headers: &HeaderMap) {
    if let Some(mut ms) = retry_after_ms(resp_headers, opt) {
        if opt.jitter_ms > 0 {
            let jitter = rand::thread_rng().gen_range(0..=opt.jitter_ms);
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
    use super::*;

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
        let opt = RetryOptions {
            max_attempts: 3,
            base_backoff_ms: 200,
            max_backoff_ms: 2_000,
            jitter_ms: 0,
            retry_status_ranges: vec![(429, 429)],
            retry_error_classes: Vec::new(),
            strategy: RetryStrategy::Failover,
            cloudflare_challenge_cooldown_secs: 0,
            cloudflare_timeout_cooldown_secs: 0,
            transport_cooldown_secs: 0,
            cooldown_backoff_factor: 1,
            cooldown_backoff_max_secs: 0,
        };
        assert_eq!(retry_after_ms(&headers, &opt), Some(2_000));
    }

    #[test]
    fn retry_info_attempts_excludes_all_upstreams_avoided_marker() {
        let chain = vec![
            "https://a.example/v1 (idx=0) status=502 class=-".to_string(),
            "https://b.example/v1 (idx=1) status=502 class=-".to_string(),
            "all_upstreams_avoided total=2".to_string(),
        ];
        let info = retry_info_for_chain(&chain).unwrap();
        assert_eq!(info.attempts, 2);
        assert_eq!(info.upstream_chain, chain);
    }

    #[test]
    fn retry_info_is_none_when_only_one_real_attempt_happened() {
        let chain = vec![
            "https://a.example/v1 (idx=0) status=502 class=-".to_string(),
            "all_upstreams_avoided total=1".to_string(),
        ];
        assert!(retry_info_for_chain(&chain).is_none());
    }
}

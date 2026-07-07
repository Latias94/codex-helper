use axum::body::Bytes;
use serde_json::json;

use crate::config::{
    ReasoningGuardAction, ReasoningGuardRetryExhaustedAction, ReasoningGuardStreamMode,
    ResolvedReasoningGuardConfig,
};
use crate::logging::RouteAttemptLog;
use crate::usage::UsageMetrics;

pub(super) const REASONING_GUARD_TRIGGERED_CLASS: &str = "reasoning_guard_triggered";
pub(super) const REASONING_GUARD_BLOCKED_CLASS: &str = "reasoning_guard_blocked";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReasoningGuardMatch {
    pub(super) reasoning_tokens: i64,
    pub(super) rule: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ReasoningGuardDecision {
    Pass,
    Observe(ReasoningGuardMatch),
    Retry(ReasoningGuardMatch),
    Block(ReasoningGuardMatch),
    ExhaustedPass(ReasoningGuardMatch),
    ExhaustedBlock(ReasoningGuardMatch),
}

impl ReasoningGuardDecision {
    pub(super) fn matched(&self) -> Option<&ReasoningGuardMatch> {
        match self {
            Self::Pass => None,
            Self::Observe(matched)
            | Self::Retry(matched)
            | Self::Block(matched)
            | Self::ExhaustedPass(matched)
            | Self::ExhaustedBlock(matched) => Some(matched),
        }
    }

    pub(super) fn failure_class(&self) -> Option<&'static str> {
        match self {
            Self::Retry(_) => Some(REASONING_GUARD_TRIGGERED_CLASS),
            Self::Block(_) | Self::ExhaustedBlock(_) => Some(REASONING_GUARD_BLOCKED_CLASS),
            Self::Pass | Self::Observe(_) | Self::ExhaustedPass(_) => None,
        }
    }

    pub(super) fn retryable(&self) -> bool {
        matches!(self, Self::Retry(_))
    }

    pub(super) fn action_label(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Observe(_) => "observe",
            Self::Retry(_) => "retry",
            Self::Block(_) => "block",
            Self::ExhaustedPass(_) => "exhausted-pass",
            Self::ExhaustedBlock(_) => "exhausted-block",
        }
    }
}

pub(super) fn should_strict_buffer_reasoning_guard(
    cfg: &ResolvedReasoningGuardConfig,
    service_name: &str,
    path: &str,
) -> bool {
    cfg.enabled
        && cfg.stream_mode != ReasoningGuardStreamMode::Off
        && reasoning_guard_scope_matches(cfg, service_name, path)
}

pub(super) fn evaluate_reasoning_guard(
    cfg: &ResolvedReasoningGuardConfig,
    service_name: &str,
    path: &str,
    usage: Option<&UsageMetrics>,
    prior_retry_matches: u32,
) -> ReasoningGuardDecision {
    if !cfg.enabled || !reasoning_guard_scope_matches(cfg, service_name, path) {
        return ReasoningGuardDecision::Pass;
    }

    let Some(reasoning_tokens) = usage.map(UsageMetrics::reasoning_output_tokens_total) else {
        return ReasoningGuardDecision::Pass;
    };
    let Some(rule) = reasoning_guard_match_rule(cfg, reasoning_tokens) else {
        return ReasoningGuardDecision::Pass;
    };

    let matched = ReasoningGuardMatch {
        reasoning_tokens,
        rule,
    };
    match cfg.action {
        ReasoningGuardAction::Observe => ReasoningGuardDecision::Observe(matched),
        ReasoningGuardAction::Block => ReasoningGuardDecision::Block(matched),
        ReasoningGuardAction::Retry if prior_retry_matches < cfg.max_guard_retries => {
            ReasoningGuardDecision::Retry(matched)
        }
        ReasoningGuardAction::Retry => match cfg.on_retry_exhausted {
            ReasoningGuardRetryExhaustedAction::Pass => {
                ReasoningGuardDecision::ExhaustedPass(matched)
            }
            ReasoningGuardRetryExhaustedAction::Block => {
                ReasoningGuardDecision::ExhaustedBlock(matched)
            }
        },
    }
}

fn reasoning_guard_match_rule(
    cfg: &ResolvedReasoningGuardConfig,
    reasoning_tokens: i64,
) -> Option<String> {
    if cfg.reasoning_equals.contains(&reasoning_tokens) {
        return Some(format!("reasoning_tokens={reasoning_tokens}"));
    }

    if let Some(n) = reasoning_boundary_sequence_n(reasoning_tokens)
        && n <= cfg.boundary_sequence_max_n
    {
        return Some(format!(
            "reasoning_tokens={reasoning_tokens} boundary=518*n-2 n={n}"
        ));
    }

    None
}

fn reasoning_boundary_sequence_n(reasoning_tokens: i64) -> Option<u32> {
    if reasoning_tokens <= 0 {
        return None;
    }
    let shifted = reasoning_tokens.checked_add(2)?;
    if shifted % 518 != 0 {
        return None;
    }
    let n = shifted / 518;
    u32::try_from(n).ok().filter(|value| *value > 0)
}

pub(super) fn reasoning_guard_retry_count(route_attempts: &[RouteAttemptLog]) -> u32 {
    route_attempts
        .iter()
        .filter(|attempt| attempt.error_class.as_deref() == Some(REASONING_GUARD_TRIGGERED_CLASS))
        .count() as u32
}

pub(super) fn reasoning_guard_error_body(
    matched: &ReasoningGuardMatch,
    class: &str,
    retryable: bool,
) -> Bytes {
    let body = json!({
        "error": {
            "message": format!(
                "codex-helper reasoning guard triggered: {}",
                matched.rule
            ),
            "type": class,
            "retryable": retryable,
            "reasoning_tokens": matched.reasoning_tokens,
        }
    });
    Bytes::from(body.to_string())
}

fn reasoning_guard_scope_matches(
    cfg: &ResolvedReasoningGuardConfig,
    service_name: &str,
    path: &str,
) -> bool {
    service_name.eq_ignore_ascii_case("codex")
        && cfg
            .paths
            .iter()
            .any(|allowed| allowed == &normalize_path(path))
}

fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut normalized = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    while normalized.len() > 1 && normalized.ends_with('/') {
        normalized.pop();
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::ReasoningGuardConfig;

    #[test]
    fn reasoning_guard_matches_default_codex_responses_path() {
        let mut cfg = ReasoningGuardConfig::default_resolved();
        cfg.enabled = true;
        let usage = UsageMetrics {
            reasoning_output_tokens: 516,
            ..UsageMetrics::default()
        };

        assert!(matches!(
            evaluate_reasoning_guard(&cfg, "codex", "/v1/responses", Some(&usage), 0),
            ReasoningGuardDecision::Retry(_)
        ));
    }

    #[test]
    fn reasoning_guard_matches_default_anomaly_token_buckets() {
        let mut cfg = ReasoningGuardConfig::default_resolved();
        cfg.enabled = true;

        for reasoning_output_tokens in [516, 1034, 1552] {
            let usage = UsageMetrics {
                reasoning_output_tokens,
                ..UsageMetrics::default()
            };

            assert!(matches!(
                evaluate_reasoning_guard(&cfg, "codex", "/v1/responses", Some(&usage), 0),
                ReasoningGuardDecision::Retry(_)
            ));
        }
    }

    #[test]
    fn reasoning_guard_matches_default_boundary_sequence() {
        let mut cfg = ReasoningGuardConfig::default_resolved();
        cfg.enabled = true;
        let usage = UsageMetrics {
            reasoning_output_tokens: 2070,
            ..UsageMetrics::default()
        };

        assert_eq!(
            evaluate_reasoning_guard(&cfg, "codex", "/v1/responses", Some(&usage), 0),
            ReasoningGuardDecision::Retry(ReasoningGuardMatch {
                reasoning_tokens: 2070,
                rule: "reasoning_tokens=2070 boundary=518*n-2 n=4".to_string(),
            })
        );
    }

    #[test]
    fn reasoning_guard_can_disable_boundary_sequence() {
        let mut cfg = ReasoningGuardConfig::default_resolved();
        cfg.enabled = true;
        cfg.boundary_sequence_max_n = 0;
        let usage = UsageMetrics {
            reasoning_output_tokens: 2070,
            ..UsageMetrics::default()
        };

        assert_eq!(
            evaluate_reasoning_guard(&cfg, "codex", "/v1/responses", Some(&usage), 0),
            ReasoningGuardDecision::Pass
        );
    }

    #[test]
    fn reasoning_guard_passes_after_exhausting_retry_budget_by_default() {
        let mut cfg = ReasoningGuardConfig::default_resolved();
        cfg.enabled = true;
        cfg.max_guard_retries = 1;
        let usage = UsageMetrics {
            reasoning_output_tokens: 516,
            ..UsageMetrics::default()
        };

        assert!(matches!(
            evaluate_reasoning_guard(&cfg, "codex", "/v1/responses", Some(&usage), 1),
            ReasoningGuardDecision::ExhaustedPass(_)
        ));
    }

    #[test]
    fn reasoning_guard_can_block_after_exhausting_retry_budget() {
        let mut cfg = ReasoningGuardConfig::default_resolved();
        cfg.enabled = true;
        cfg.max_guard_retries = 1;
        cfg.on_retry_exhausted = ReasoningGuardRetryExhaustedAction::Block;
        let usage = UsageMetrics {
            reasoning_output_tokens: 516,
            ..UsageMetrics::default()
        };

        assert!(matches!(
            evaluate_reasoning_guard(&cfg, "codex", "/v1/responses", Some(&usage), 1),
            ReasoningGuardDecision::ExhaustedBlock(_)
        ));
    }

    #[test]
    fn reasoning_guard_ignores_other_services() {
        let mut cfg = ReasoningGuardConfig::default_resolved();
        cfg.enabled = true;
        let usage = UsageMetrics {
            reasoning_output_tokens: 516,
            ..UsageMetrics::default()
        };

        assert_eq!(
            evaluate_reasoning_guard(&cfg, "claude", "/v1/responses", Some(&usage), 0),
            ReasoningGuardDecision::Pass
        );
    }
}

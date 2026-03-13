use super::*;

pub(super) fn sync_stations_retry_editor(
    editor: &mut StationsRetryEditorState,
    retry: &RetryConfig,
) {
    let signature = format!("{retry:?}");
    if editor.source_signature.as_deref() == Some(signature.as_str()) {
        return;
    }
    load_stations_retry_editor_fields(editor, retry);
    editor.source_signature = Some(signature);
}

pub(super) fn load_stations_retry_editor_fields(
    editor: &mut StationsRetryEditorState,
    retry: &RetryConfig,
) {
    editor.profile = retry
        .profile
        .map(retry_profile_name_value)
        .unwrap_or_default()
        .to_string();
    editor.cloudflare_challenge_cooldown_secs =
        optional_u64_editor_value(retry.cloudflare_challenge_cooldown_secs);
    editor.cloudflare_timeout_cooldown_secs =
        optional_u64_editor_value(retry.cloudflare_timeout_cooldown_secs);
    editor.transport_cooldown_secs = optional_u64_editor_value(retry.transport_cooldown_secs);
    editor.cooldown_backoff_factor = optional_u64_editor_value(retry.cooldown_backoff_factor);
    editor.cooldown_backoff_max_secs = optional_u64_editor_value(retry.cooldown_backoff_max_secs);
}

fn optional_u64_editor_value(value: Option<u64>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

pub(super) fn build_retry_config_from_editor(
    editor: &StationsRetryEditorState,
    base: &RetryConfig,
) -> Result<RetryConfig, String> {
    let mut retry = base.clone();
    retry.profile = retry_profile_name_from_value(editor.profile.as_str());
    retry.cloudflare_challenge_cooldown_secs = parse_optional_u64_editor_value(
        "cloudflare_challenge_cooldown_secs",
        &editor.cloudflare_challenge_cooldown_secs,
    )?;
    retry.cloudflare_timeout_cooldown_secs = parse_optional_u64_editor_value(
        "cloudflare_timeout_cooldown_secs",
        &editor.cloudflare_timeout_cooldown_secs,
    )?;
    retry.transport_cooldown_secs = parse_optional_u64_editor_value(
        "transport_cooldown_secs",
        &editor.transport_cooldown_secs,
    )?;
    retry.cooldown_backoff_factor = parse_optional_u64_editor_value(
        "cooldown_backoff_factor",
        &editor.cooldown_backoff_factor,
    )?;
    retry.cooldown_backoff_max_secs = parse_optional_u64_editor_value(
        "cooldown_backoff_max_secs",
        &editor.cooldown_backoff_max_secs,
    )?;
    Ok(retry)
}

fn parse_optional_u64_editor_value(field: &str, raw: &str) -> Result<Option<u64>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<u64>()
        .map(Some)
        .map_err(|_| format!("{field} must be a non-negative integer"))
}

pub(super) fn retry_profile_name_value(profile: RetryProfileName) -> &'static str {
    match profile {
        RetryProfileName::Balanced => "balanced",
        RetryProfileName::SameUpstream => "same-upstream",
        RetryProfileName::AggressiveFailover => "aggressive-failover",
        RetryProfileName::CostPrimary => "cost-primary",
    }
}

pub(super) fn retry_profile_name_from_value(value: &str) -> Option<RetryProfileName> {
    match value.trim() {
        "balanced" => Some(RetryProfileName::Balanced),
        "same-upstream" => Some(RetryProfileName::SameUpstream),
        "aggressive-failover" => Some(RetryProfileName::AggressiveFailover),
        "cost-primary" => Some(RetryProfileName::CostPrimary),
        _ => None,
    }
}

pub(super) fn retry_profile_display_text(
    lang: Language,
    profile: Option<RetryProfileName>,
) -> String {
    match profile {
        None => pick(lang, "自动（默认 balanced）", "Auto (default balanced)").to_string(),
        Some(RetryProfileName::Balanced) => pick(lang, "balanced（均衡）", "balanced").to_string(),
        Some(RetryProfileName::SameUpstream) => {
            pick(lang, "same-upstream（优先同上游）", "same-upstream").to_string()
        }
        Some(RetryProfileName::AggressiveFailover) => pick(
            lang,
            "aggressive-failover（积极切换）",
            "aggressive-failover",
        )
        .to_string(),
        Some(RetryProfileName::CostPrimary) => {
            pick(lang, "cost-primary（成本优先）", "cost-primary").to_string()
        }
    }
}

pub(super) fn retry_strategy_label(strategy: RetryStrategy) -> &'static str {
    match strategy {
        RetryStrategy::Failover => "failover",
        RetryStrategy::SameUpstream => "same_upstream",
    }
}

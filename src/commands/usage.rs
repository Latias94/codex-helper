use crate::config::load_config;
use crate::control_plane_client::{ControlPlaneClient, ControlPlaneEndpoint};
use crate::dashboard_core::{
    OperatorReadData, OperatorReadModel, OperatorReadStatus, OperatorRequestSummary,
};
use crate::relay_target::resolve_relay_target;
use crate::request_chain::RequestChainSelector;
use crate::request_ledger::{
    RequestUsageSummary, RequestUsageSummaryGroup, RequestUsageSummaryRow,
};
use crate::{CliError, CliResult, UsageCommand, UsageSummaryBy};
use codex_helper_core::runtime_identity::ProviderEndpointKey;
use codex_helper_core::{quota_analytics as analytics, quota_pool as pool};
use owo_colors::OwoColorize;
use std::io::Write;

pub async fn handle_usage_cmd(
    cmd: UsageCommand,
    client: &ControlPlaneClient,
    model: OperatorReadModel,
) -> CliResult<()> {
    let cmd = match extract_quota_command(cmd) {
        Ok((target, json)) => {
            let mut stdout = std::io::stdout();
            return handle_usage_quota(target, json, &mut stdout).await;
        }
        Err(cmd) => *cmd,
    };
    let machine_readable = usage_command_is_machine_readable(&cmd);
    let Some(data) = model.data.as_ref() else {
        print_operator_state(&model, machine_readable)?;
        return Ok(());
    };
    if model.status == OperatorReadStatus::Stale {
        eprintln!(
            "Operator read model for {} is stale (captured_at_ms={}).",
            model.service_name, model.captured_at_ms
        );
    }

    match cmd {
        UsageCommand::Tail { limit, raw } => {
            let requests = recent_requests(data, limit);
            for request in requests {
                if raw {
                    println!(
                        "{}",
                        serde_json::to_string(request).map_err(|error| {
                            CliError::Usage(format!("无法序列化 operator 请求: {error}"))
                        })?
                    );
                } else {
                    println!("{}", format_operator_request(request));
                }
            }
        }
        UsageCommand::Summary { limit, by } => {
            let group = RequestUsageSummaryGroup::from(by);
            let (mut rows, source) = summary_rows(data, by);
            rows.sort_by(|left, right| {
                right
                    .aggregate
                    .total_tokens
                    .cmp(&left.aggregate.total_tokens)
                    .then_with(|| left.group_value.cmp(&right.group_value))
            });
            rows.truncate(limit);

            println!(
                "{}",
                format!(
                    "Usage summary by {} ({source}; status={:?})",
                    group.column_name(),
                    model.status
                )
                .bold()
            );
            println!(
                "{}",
                format!(
                    "{} | requests | input | output | cache_read | cache_create | reasoning | total | avg_duration_ms",
                    group.column_name()
                )
                .bold()
            );
            for row in rows {
                println!("{}", row.aggregate.summary_line(&row.group_value));
            }
        }
        UsageCommand::Find {
            limit,
            session,
            model: model_filter,
            provider_endpoint,
            provider,
            path,
            status_min,
            status_max,
            errors,
            fast,
            retried,
            raw,
        } => {
            let filters = OperatorRequestFilters {
                session,
                model: model_filter,
                provider_endpoint: provider_endpoint.map(|endpoint| endpoint.into_key()),
                provider,
                path,
                status_min: status_min.or(errors.then_some(400)),
                status_max,
                fast,
                retried,
            };
            let mut requests = data
                .recent_requests
                .iter()
                .filter(|request| filters.matches(request))
                .collect::<Vec<_>>();
            requests.sort_by_key(|request| std::cmp::Reverse((request.ended_at_ms, request.id)));
            requests.truncate(limit);

            for request in &requests {
                if raw {
                    println!(
                        "{}",
                        serde_json::to_string(request).map_err(|error| {
                            CliError::Usage(format!("无法序列化 operator 请求: {error}"))
                        })?
                    );
                } else {
                    println!("{}", format_operator_request(request));
                }
            }
            if requests.is_empty() && !raw {
                println!(
                    "No requests matched the filters in the {} operator snapshot.",
                    model.service_name
                );
            }
        }
        UsageCommand::Chain {
            limit,
            trace_id,
            request_id,
            session,
            json,
        } => {
            let selector = RequestChainSelector {
                trace_id,
                request_id,
                session_id: session,
            }
            .normalized();
            if !selector.has_identity() {
                return Err(CliError::Usage(
                    "usage chain requires --trace-id, --request-id, or --session".to_string(),
                ));
            }

            let export = client
                .request_chain(selector, limit)
                .await
                .map_err(|error| {
                    CliError::Usage(format!("无法从 runtime control plane 读取请求链: {error}"))
                })?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&export)
                        .map_err(|error| CliError::Usage(format!("无法序列化请求链: {error}")))?
                );
                return Ok(());
            }

            println!(
                "{}",
                format!(
                    "Request chain export: {} request(s), truncated={} (runtime control plane)",
                    export.requests.len(),
                    export.truncated,
                )
                .bold()
            );
            for request in &export.requests {
                println!(
                    "[{}] request={} trace={} session={} status={} provider={} model={} attempts={} events={}",
                    request.ended_at_ms,
                    request.request_id,
                    request.trace_id.as_deref().unwrap_or("-"),
                    request.session_id.as_deref().unwrap_or("-"),
                    request.status_code,
                    request.provider_id.as_deref().unwrap_or("-"),
                    request.model.as_deref().unwrap_or("-"),
                    request.route_attempts.len(),
                    request.timeline.len(),
                );
                for attempt in &request.route_attempts {
                    println!(
                        "    attempt#{} code={} decision={} status={} endpoint={} model={}",
                        attempt.attempt_index,
                        attempt.code,
                        attempt.decision,
                        attempt
                            .status_code
                            .map(|status| status.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                        attempt.provider_endpoint_key.as_deref().unwrap_or("-"),
                        attempt.model.as_deref().unwrap_or("-"),
                    );
                }
            }
        }
        UsageCommand::Quota { target, .. } => {
            return Err(quota_error(&target, "internal_dispatch_error"));
        }
    }

    Ok(())
}

fn usage_command_is_machine_readable(cmd: &UsageCommand) -> bool {
    matches!(
        cmd,
        UsageCommand::Tail { raw: true, .. }
            | UsageCommand::Find { raw: true, .. }
            | UsageCommand::Chain { json: true, .. }
    )
}

fn print_operator_state(model: &OperatorReadModel, machine_readable: bool) -> CliResult<()> {
    if machine_readable {
        println!(
            "{}",
            serde_json::to_string_pretty(model)
                .map_err(|error| CliError::Usage(format!("无法序列化 operator 状态: {error}")))?
        );
        return Ok(());
    }

    let status = match model.status {
        OperatorReadStatus::Ready => "ready",
        OperatorReadStatus::Stale => "stale",
        OperatorReadStatus::Disconnected => "disconnected",
        OperatorReadStatus::AuthRequired => "auth_required",
    };
    println!(
        "Runtime operator state: service={} status={} issue={}",
        model.service_name,
        status,
        model
            .issue
            .map(|issue| format!("{issue:?}"))
            .unwrap_or_else(|| "-".to_string())
    );
    Ok(())
}

fn recent_requests(data: &OperatorReadData, limit: usize) -> Vec<&OperatorRequestSummary> {
    let mut requests = data.recent_requests.iter().collect::<Vec<_>>();
    requests.sort_by_key(|request| std::cmp::Reverse((request.ended_at_ms, request.id)));
    requests.truncate(limit);
    requests
}

fn format_operator_request(request: &OperatorRequestSummary) -> String {
    let provider = request.provider_id.as_deref().unwrap_or("-");
    let endpoint = request.endpoint_id.as_deref().unwrap_or("-");
    let model = request.model.as_deref().unwrap_or("-");
    let session = request.session_key.as_deref().unwrap_or("-");
    let total_tokens = request
        .usage
        .as_ref()
        .map(|usage| usage.total_tokens)
        .unwrap_or(0);
    format!(
        "[{}] {} {} -> {} ({}ms, provider={}/{}, model={}, tokens={}, session={})",
        request.ended_at_ms,
        request.method,
        request.path,
        request.status_code,
        request.duration_ms,
        provider,
        endpoint,
        model,
        total_tokens,
        session,
    )
}

fn summary_rows(
    data: &OperatorReadData,
    by: UsageSummaryBy,
) -> (Vec<RequestUsageSummaryRow>, String) {
    canonical_summary_rows(&data.usage_summaries, by)
}

fn canonical_summary_rows(
    summaries: &[RequestUsageSummary],
    by: UsageSummaryBy,
) -> (Vec<RequestUsageSummaryRow>, String) {
    let group = RequestUsageSummaryGroup::from(by);
    let Some(summary) = summaries.iter().find(|summary| summary.group == group) else {
        return (
            Vec::new(),
            "committed operator ledger projection unavailable".to_string(),
        );
    };
    let window = match (
        summary.coverage.first_terminal_at_ms,
        summary.coverage.last_terminal_at_ms,
    ) {
        (Some(first), Some(last)) => format!("{first}..={last}"),
        _ => "empty".to_string(),
    };
    (
        summary.rows.clone(),
        format!(
            "{}; window={window}; requests={}",
            summary.coverage.source, summary.coverage.requests
        ),
    )
}

#[derive(Debug, Default)]
struct OperatorRequestFilters {
    session: Option<String>,
    model: Option<String>,
    provider_endpoint: Option<ProviderEndpointKey>,
    provider: Option<String>,
    path: Option<String>,
    status_min: Option<u64>,
    status_max: Option<u64>,
    fast: bool,
    retried: bool,
}

impl OperatorRequestFilters {
    fn matches(&self, request: &OperatorRequestSummary) -> bool {
        contains_optional(request.session_key.as_deref(), self.session.as_deref())
            && contains_optional(request.model.as_deref(), self.model.as_deref())
            && contains_optional(request.provider_id.as_deref(), self.provider.as_deref())
            && contains_optional(Some(request.path.as_str()), self.path.as_deref())
            && self.provider_endpoint.as_ref().is_none_or(|key| {
                request.service == key.service_name
                    && request.provider_id.as_deref() == Some(key.provider_id.as_str())
                    && request.endpoint_id.as_deref() == Some(key.endpoint_id.as_str())
            })
            && self
                .status_min
                .is_none_or(|minimum| u64::from(request.status_code) >= minimum)
            && self
                .status_max
                .is_none_or(|maximum| u64::from(request.status_code) <= maximum)
            && (!self.fast || request.observability.fast_mode)
            && (!self.retried || request.observability.retried)
    }
}

fn contains_optional(value: Option<&str>, expected: Option<&str>) -> bool {
    let Some(expected) = expected else {
        return true;
    };
    value.is_some_and(|value| {
        value
            .to_ascii_lowercase()
            .contains(&expected.to_ascii_lowercase())
    })
}

fn extract_quota_command(cmd: UsageCommand) -> Result<(String, bool), Box<UsageCommand>> {
    match cmd {
        UsageCommand::Quota { target, json } => Ok((target, json)),
        operator_read_command => Err(Box::new(operator_read_command)),
    }
}

async fn handle_usage_quota(
    target_name: String,
    json: bool,
    writer: &mut dyn Write,
) -> CliResult<()> {
    let cfg = load_config()
        .await
        .map_err(|_| quota_error(&target_name, "configuration_load_failed"))?;
    let target = resolve_relay_target(&cfg, &target_name)
        .map_err(|_| quota_error(&target_name, "target_resolution_failed"))?;
    let resolved_name = target.name;
    let admin_url = target
        .admin_url
        .ok_or_else(|| quota_error(&resolved_name, "admin_endpoint_unavailable"))?;
    let endpoint = ControlPlaneEndpoint::new(admin_url, target.admin_token_env)
        .map_err(|_| quota_error(&resolved_name, "client_initialization_failed"))?;
    let client = ControlPlaneClient::new(endpoint)
        .map_err(|_| quota_error(&resolved_name, "client_initialization_failed"))?;
    let model = client
        .operator_read_model()
        .await
        .map_err(|_| quota_error(&resolved_name, "operator_read_model_fetch_failed"))?;
    let data = model
        .data
        .as_ref()
        .ok_or_else(|| quota_error(&resolved_name, "operator_read_model_unavailable"))?;
    let quota = &data.quota_analytics;

    if json {
        writeln!(writer, "{}", quota_json_text(&resolved_name, quota)?)
            .map_err(|_| quota_error(&resolved_name, "output_write_failed"))?;
    } else {
        write!(
            writer,
            "{}",
            quota_text(&resolved_name, &model.service_name, quota)
        )
        .map_err(|_| quota_error(&resolved_name, "output_write_failed"))?;
    }
    Ok(())
}

fn quota_error(target: &str, category: &str) -> CliError {
    CliError::Usage(format!("quota target '{target}': {category}"))
}

fn quota_json_text(target: &str, view: &analytics::QuotaAnalyticsView) -> CliResult<String> {
    serde_json::to_string_pretty(view).map_err(|_| quota_error(target, "json_serialization_failed"))
}

fn quota_text(target: &str, service_name: &str, view: &analytics::QuotaAnalyticsView) -> String {
    let mut output = format!("Quota analytics for target '{target}' (service={service_name})\n");
    output.push_str(&format!(
        "support={} generated_at_ms={} registry_generation={} pools={} omitted_pools={}\n",
        quota_support_text(view.support),
        view.generated_at_ms,
        view.registry_generation,
        view.pools.len(),
        view.omitted_pools,
    ));
    if view.support == analytics::QuotaAnalyticsSupport::Unsupported {
        output.push_str("Remote quota analytics are not supported by this daemon.\n");
        return output;
    }
    if view.pools.is_empty() {
        output.push_str("No quota pools reported.\n");
        return output;
    }

    for (index, quota_pool) in view.pools.iter().enumerate() {
        output.push_str(&format!(
            "pool[{}] key={} revision={} scope={} confidence={} aggregation_eligible={} conflicting_evidence={}\n",
            index + 1,
            quota_pool.identity.key,
            quota_pool.identity.revision,
            quota_pool.identity.scope.as_key(),
            confidence_text(quota_pool.identity.confidence),
            quota_pool.identity.aggregation_eligible,
            quota_pool.identity.conflicting_evidence,
        ));
        output.push_str(&format!(
            "  used={} direct_total={} remaining={} limit={} observed_burn={} unit={}\n",
            quantity_text(quota_pool.remote_used.as_ref()),
            quantity_text(quota_pool.remote_direct_total.as_ref()),
            quantity_text(quota_pool.remote_remaining.as_ref()),
            quantity_text(quota_pool.remote_limit.as_ref()),
            quantity_text(quota_pool.observed_burn.as_ref()),
            quota_pool.unit.as_str(),
        ));
        output.push_str(&format!(
            "  15m={} 60m={} required={} pace={} pace_ratio_basis_points={}\n",
            rate_text(&quota_pool.rate_15m),
            rate_text(&quota_pool.rate_60m),
            per_hour_text(quota_pool.pacing.required_rate_per_hour.as_ref()),
            pace_status_text(quota_pool.pacing.status),
            optional_u32(quota_pool.pacing.pace_ratio_basis_points),
        ));
        output.push_str(&format!(
            "  rate_15m_status={} samples_15m={} span_15m_ms={} lower_bound_15m={} rate_60m_status={} samples_60m={} span_60m_ms={} lower_bound_60m={}\n",
            rate_status_text(quota_pool.rate_15m.status),
            quota_pool.rate_15m.sample_count,
            quota_pool.rate_15m.span_ms,
            quota_pool.rate_15m.lower_bound,
            rate_status_text(quota_pool.rate_60m.status),
            quota_pool.rate_60m.sample_count,
            quota_pool.rate_60m.span_ms,
            quota_pool.rate_60m.lower_bound,
        ));
        output.push_str(&format!(
            "  exhaustion_eta_ms={} projected_remaining_at_reset={} reset_at_ms={}\n",
            optional_u64(quota_pool.pacing.exhaustion_eta_ms),
            quantity_text(quota_pool.pacing.projected_remaining_at_reset.as_ref()),
            optional_u64(quota_pool.pacing.reset_at_ms),
        ));
        output.push_str(&format!(
            "  source={} scope={} confidence={} freshness={} observed_at_ms={} last_success_at_ms={} last_attempt_at_ms={} latest_adjustment={}\n",
            quota_pool.source,
            quota_pool.identity.scope.as_key(),
            confidence_text(quota_pool.identity.confidence),
            freshness_text(quota_pool.freshness),
            quota_pool.observed_at_ms,
            optional_u64(quota_pool.last_success_at_ms),
            optional_u64(quota_pool.last_attempt_at_ms),
            quota_pool
                .latest_adjustment
                .map(adjustment_text)
                .unwrap_or("-"),
        ));
        output.push_str(&format!(
            "  window={} reset_semantics={} reset_timezone={} rolling_duration_ms={} epoch_start_ms={} epoch_end_ms={}\n",
            window_kind_text(quota_pool.window.kind),
            reset_kind_text(quota_pool.window.reset),
            quota_pool.window.reset_timezone.as_deref().unwrap_or("-"),
            optional_u64(quota_pool.window.rolling_duration_ms),
            quota_pool.epoch_start_ms,
            optional_u64(quota_pool.epoch_end_ms),
        ));
    }
    output
}

fn quantity_text(quantity: Option<&pool::QuotaQuantity>) -> String {
    let Some(quantity) = quantity else {
        return "-".to_string();
    };
    let Some(decimal) = decimal_text(quantity.value, quantity.scale) else {
        return "-".to_string();
    };
    match quantity.unit {
        pool::QuotaUnit::Usd => format!("${decimal}"),
        pool::QuotaUnit::Tokens => format!("{decimal} tokens"),
        pool::QuotaUnit::Raw => format!("{decimal} raw"),
        pool::QuotaUnit::Unknown => format!("{decimal} unknown"),
    }
}

fn decimal_text(value: i128, scale: u32) -> Option<String> {
    let scale = usize::try_from(scale).ok()?;
    if scale > 38 {
        return None;
    }
    let negative = value < 0;
    let mut digits = value.unsigned_abs().to_string();
    if scale > 0 {
        if digits.len() <= scale {
            digits.insert_str(0, &"0".repeat(scale + 1 - digits.len()));
        }
        digits.insert(digits.len() - scale, '.');
    }
    Some(if negative {
        format!("-{digits}")
    } else {
        digits
    })
}

fn rate_text(rate: &analytics::QuotaRateWindow) -> String {
    let value = per_hour_text(rate.rate_per_hour.as_ref());
    if rate.lower_bound && value != "-" {
        format!(">={value}")
    } else {
        value
    }
}

fn per_hour_text(quantity: Option<&pool::QuotaQuantity>) -> String {
    quantity.map_or_else(
        || "-".to_string(),
        |quantity| format!("{}/h", quantity_text(Some(quantity))),
    )
}

fn optional_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_string(), |value| value.to_string())
}

fn optional_u32(value: Option<u32>) -> String {
    value.map_or_else(|| "-".to_string(), |value| value.to_string())
}

fn quota_support_text(support: analytics::QuotaAnalyticsSupport) -> &'static str {
    match support {
        analytics::QuotaAnalyticsSupport::Unsupported => "unsupported",
        analytics::QuotaAnalyticsSupport::Supported => "supported",
    }
}

fn rate_status_text(status: analytics::QuotaRateStatus) -> &'static str {
    match status {
        analytics::QuotaRateStatus::Available => "available",
        analytics::QuotaRateStatus::InsufficientSamples => "insufficient_samples",
        analytics::QuotaRateStatus::ShortSpan => "short_span",
        analytics::QuotaRateStatus::Stale => "stale",
        analytics::QuotaRateStatus::Gap => "gap",
        analytics::QuotaRateStatus::Adjustment => "adjustment",
        analytics::QuotaRateStatus::NegativeDelta => "negative_delta",
        analytics::QuotaRateStatus::Unordered => "unordered",
        analytics::QuotaRateStatus::NoCounter => "no_counter",
        analytics::QuotaRateStatus::Overflow => "overflow",
    }
}

fn pace_status_text(status: analytics::QuotaPaceStatus) -> &'static str {
    match status {
        analytics::QuotaPaceStatus::Unlimited => "unlimited",
        analytics::QuotaPaceStatus::Faster => "faster",
        analytics::QuotaPaceStatus::OnPace => "on_pace",
        analytics::QuotaPaceStatus::Slower => "slower",
        analytics::QuotaPaceStatus::NoReset => "no_reset",
        analytics::QuotaPaceStatus::ResetUnknown => "reset_unknown",
        analytics::QuotaPaceStatus::LowSample => "low_sample",
        analytics::QuotaPaceStatus::Stale => "stale",
        analytics::QuotaPaceStatus::Unavailable => "unavailable",
    }
}

fn freshness_text(status: analytics::QuotaFreshnessStatus) -> &'static str {
    match status {
        analytics::QuotaFreshnessStatus::Fresh => "fresh",
        analytics::QuotaFreshnessStatus::Stale => "stale",
        analytics::QuotaFreshnessStatus::Offline => "offline",
        analytics::QuotaFreshnessStatus::Unknown => "unknown",
    }
}

fn confidence_text(confidence: pool::IdentityConfidence) -> &'static str {
    match confidence {
        pool::IdentityConfidence::High => "high",
        pool::IdentityConfidence::Medium => "medium",
        pool::IdentityConfidence::Low => "low",
        pool::IdentityConfidence::Unknown => "unknown",
    }
}

fn window_kind_text(kind: pool::QuotaWindowKind) -> &'static str {
    match kind {
        pool::QuotaWindowKind::CalendarDay => "calendar_day",
        pool::QuotaWindowKind::Rolling => "rolling",
        pool::QuotaWindowKind::Custom => "custom",
        pool::QuotaWindowKind::Monthly => "monthly",
        pool::QuotaWindowKind::Resetless => "resetless",
        pool::QuotaWindowKind::Unknown => "unknown",
    }
}

fn reset_kind_text(kind: pool::QuotaResetKind) -> &'static str {
    match kind {
        pool::QuotaResetKind::ExplicitTimestamp => "explicit_timestamp",
        pool::QuotaResetKind::ConfiguredCalendarBoundary => "configured_calendar_boundary",
        pool::QuotaResetKind::NoReset => "no_reset",
        pool::QuotaResetKind::Unknown => "unknown",
    }
}

fn adjustment_text(kind: pool::QuotaAdjustmentKind) -> &'static str {
    match kind {
        pool::QuotaAdjustmentKind::Discontinuity => "discontinuity",
        pool::QuotaAdjustmentKind::CounterResetOrRollback => "counter_reset_or_rollback",
        pool::QuotaAdjustmentKind::TopUp => "top_up",
        pool::QuotaAdjustmentKind::LimitOrPlanChanged => "limit_or_plan_changed",
        pool::QuotaAdjustmentKind::NormalizationChanged => "normalization_changed",
    }
}

impl From<UsageSummaryBy> for RequestUsageSummaryGroup {
    fn from(value: UsageSummaryBy) -> Self {
        match value {
            UsageSummaryBy::ProviderEndpoint => Self::ProviderEndpoint,
            UsageSummaryBy::Provider => Self::Provider,
            UsageSummaryBy::Model => Self::Model,
            UsageSummaryBy::Session => Self::Session,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_types::{Cli, Command};
    use crate::commands::test_support::{ScopedEnv, TempTestDir, env_lock};
    use crate::dashboard_core::OperatorRequestObservability;
    use crate::pricing::CostBreakdown;
    use crate::request_ledger::{RequestUsageAggregate, RequestUsageSummaryCoverage};
    use crate::usage::UsageMetrics;
    use axum::http::{HeaderMap, Uri};
    use axum::routing::get;
    use axum::{Json, Router};
    use clap::Parser;
    use codex_helper_core::config::{
        HelperConfig, RelayTargetConfig, ServiceKind, save_helper_config,
    };
    use codex_helper_core::dashboard_core::{ApiV1OperatorSummary, OperatorRevisionBundle};
    use codex_helper_core::proxy::ADMIN_TOKEN_HEADER;
    use codex_helper_core::quota_analytics::{
        PoolQuotaAnalytics, QuotaAnalyticsSupport, QuotaAnalyticsView, QuotaFreshnessStatus,
        QuotaPaceStatus, QuotaPacingView, QuotaRateStatus, QuotaRateWindow,
    };
    use codex_helper_core::quota_pool::{
        IdentityConfidence, PoolIdentity, QuotaQuantity, QuotaResetKind, QuotaScope, QuotaUnit,
        QuotaWindowKind, QuotaWindowSemantics,
    };
    use std::sync::{Arc, Mutex};

    fn operator_request() -> OperatorRequestSummary {
        OperatorRequestSummary {
            id: 7,
            session_key: Some("session:sha256:abc".to_string()),
            model: Some("gpt-5.6".to_string()),
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("priority".to_string()),
            provider_id: Some("sol".to_string()),
            endpoint_id: Some("responses".to_string()),
            provider_endpoint_key: Some("endpoint:sha256:def".to_string()),
            route_path: vec!["main".to_string(), "sol".to_string()],
            upstream_origin: Some("https://relay.example.test".to_string()),
            usage: Some(UsageMetrics {
                input_tokens: 100,
                output_tokens: 20,
                reasoning_output_tokens: 5,
                total_tokens: 120,
                cache_read_input_tokens: 30,
                cache_creation_input_tokens: 10,
                ..UsageMetrics::default()
            }),
            cost: CostBreakdown::default(),
            retry: None,
            provider_signal_codes: Vec::new(),
            policy_action_codes: Vec::new(),
            observability: OperatorRequestObservability {
                duration_ms: Some(250),
                ttfb_ms: Some(50),
                generation_ms: Some(200),
                output_tokens_per_second: Some(100.0),
                attempt_count: 2,
                route_attempt_count: 2,
                retried: true,
                cross_provider_failover: false,
                same_provider_retry: true,
                fast_mode: true,
                streaming: true,
            },
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 250,
            ttfb_ms: Some(50),
            streaming: true,
            ended_at_ms: 1_000,
        }
    }

    #[test]
    fn operator_filters_match_projected_identity_and_runtime_flags() {
        let request = operator_request();
        let filters = OperatorRequestFilters {
            session: Some("SHA256:ABC".to_string()),
            model: Some("5.6".to_string()),
            provider_endpoint: Some(ProviderEndpointKey::new("codex", "sol", "responses")),
            provider: Some("SOL".to_string()),
            path: Some("responses".to_string()),
            status_min: Some(200),
            status_max: Some(299),
            fast: true,
            retried: true,
        };

        assert!(filters.matches(&request));
        assert!(
            !OperatorRequestFilters {
                provider_endpoint: Some(ProviderEndpointKey::new("codex", "terra", "responses")),
                ..OperatorRequestFilters::default()
            }
            .matches(&request)
        );
    }

    #[test]
    fn summary_selection_uses_server_canonical_buckets_without_recent_reaggregation() {
        let summaries = vec![RequestUsageSummary {
            group: RequestUsageSummaryGroup::ProviderEndpoint,
            coverage: RequestUsageSummaryCoverage {
                source: "runtime_store_retained_terminals".to_string(),
                first_terminal_at_ms: Some(10),
                last_terminal_at_ms: Some(20),
                requests: 301,
                all_history: false,
            },
            rows: vec![RequestUsageSummaryRow {
                group_value: "codex/sol/responses".to_string(),
                aggregate: RequestUsageAggregate {
                    requests: 301,
                    duration_ms_total: 250,
                    input_tokens: 700,
                    output_tokens: 20,
                    reasoning_tokens: 5,
                    cache_read_input_tokens: 100,
                    cache_creation_input_tokens: 200,
                    total_tokens: 1_020,
                },
            }],
        }];

        let (rows, source) = canonical_summary_rows(&summaries, UsageSummaryBy::ProviderEndpoint);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].aggregate.requests, 301);
        assert_eq!(rows[0].aggregate.input_tokens, 700);
        assert_eq!(rows[0].aggregate.cache_read_input_tokens, 100);
        assert_eq!(rows[0].aggregate.cache_creation_input_tokens, 200);
        assert!(source.contains("requests=301"));
    }

    fn quota_view() -> QuotaAnalyticsView {
        let usd = |value| QuotaQuantity::from_integer(value, QuotaUnit::Usd);
        QuotaAnalyticsView {
            support: QuotaAnalyticsSupport::Supported,
            generated_at_ms: 1_000_000,
            registry_generation: 7,
            pools: vec![PoolQuotaAnalytics {
                identity: PoolIdentity {
                    key: "pool-safe-id".to_string(),
                    origin: "https://relay.example".to_string(),
                    scope: QuotaScope::Account,
                    confidence: IdentityConfidence::High,
                    aggregation_eligible: true,
                    ..PoolIdentity::default()
                },
                observed_at_ms: 990_000,
                last_success_at_ms: Some(990_000),
                last_attempt_at_ms: Some(995_000),
                freshness: QuotaFreshnessStatus::Fresh,
                source: "usage_provider:new_api_user_self".to_string(),
                unit: QuotaUnit::Usd,
                window: QuotaWindowSemantics {
                    kind: QuotaWindowKind::CalendarDay,
                    reset: QuotaResetKind::ExplicitTimestamp,
                    reset_timezone: Some("Asia/Shanghai".to_string()),
                    rolling_duration_ms: None,
                },
                remote_used: Some(usd(25)),
                remote_remaining: Some(usd(75)),
                remote_limit: Some(usd(100)),
                rate_15m: QuotaRateWindow {
                    status: QuotaRateStatus::Available,
                    rate_per_hour: Some(usd(3)),
                    sample_count: 4,
                    span_ms: 15 * 60_000,
                    ..QuotaRateWindow::default()
                },
                rate_60m: QuotaRateWindow {
                    status: QuotaRateStatus::Available,
                    rate_per_hour: Some(usd(4)),
                    sample_count: 12,
                    span_ms: 60 * 60_000,
                    ..QuotaRateWindow::default()
                },
                pacing: QuotaPacingView {
                    status: QuotaPaceStatus::OnPace,
                    required_rate_per_hour: Some(usd(5)),
                    pace_ratio_basis_points: Some(8_000),
                    exhaustion_eta_ms: Some(18 * 60 * 60_000),
                    reset_at_ms: Some(86_400_000),
                    ..QuotaPacingView::default()
                },
                ..PoolQuotaAnalytics::default()
            }],
            omitted_pools: 2,
        }
    }

    fn operator_read_model(quota_analytics: QuotaAnalyticsView) -> OperatorReadModel {
        OperatorReadModel::ready(
            "codex",
            1_000_000,
            OperatorRevisionBundle {
                runtime_revision: 1,
                runtime_digest: "runtime-1".to_string(),
                route_digest: "route-1".to_string(),
                catalog_revision: "catalog-1".to_string(),
                pricing_revision: "pricing-1".to_string(),
                operator_pricing_revision: "operator-pricing-1".to_string(),
                policy_revision: 1,
                ledger_revision: "ledger-1".to_string(),
            },
            OperatorReadData {
                summary: ApiV1OperatorSummary {
                    api_version: 1,
                    service_name: "codex".to_string(),
                    runtime: Default::default(),
                    counts: Default::default(),
                    retry: Default::default(),
                    sessions: Vec::new(),
                    profiles: Vec::new(),
                    providers: Vec::new(),
                },
                active_requests: Vec::new(),
                recent_requests: Vec::new(),
                usage_summaries: Vec::new(),
                usage_day: Default::default(),
                usage_rollup: Default::default(),
                stats_5m: Default::default(),
                stats_1h: Default::default(),
                pricing_catalog: Default::default(),
                provider_balances: Vec::new(),
                quota_analytics,
            },
        )
    }

    async fn spawn_control_plane(
        body: serde_json::Value,
    ) -> (
        std::net::SocketAddr,
        Arc<Mutex<Option<(String, Option<String>)>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let observed = Arc::new(Mutex::new(None));
        let observed_for_route = Arc::clone(&observed);
        let app = Router::new().route(
            "/__codex_helper/api/v1/operator/read-model",
            get(move |headers: HeaderMap, uri: Uri| {
                let body = body.clone();
                let observed = Arc::clone(&observed_for_route);
                async move {
                    let token = headers
                        .get(ADMIN_TOKEN_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_string);
                    *observed.lock().expect("record request") = Some((uri.to_string(), token));
                    Json(body)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind control-plane fixture");
        let address = listener.local_addr().expect("control-plane address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve control-plane fixture");
        });
        (address, observed, server)
    }

    #[test]
    fn quota_command_is_extracted_before_operator_read_commands() {
        assert_eq!(
            extract_quota_command(UsageCommand::Quota {
                target: "nas".to_string(),
                json: true,
            })
            .expect("quota command"),
            ("nas".to_string(), true)
        );
        assert!(
            extract_quota_command(UsageCommand::Tail {
                limit: 20,
                raw: false,
            })
            .is_err()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn quota_json_runs_the_command_path_against_a_mock_control_plane() {
        let _env_lock = env_lock().await;
        let helper_home = TempTestDir::new("codex-helper-cli-test-usage-quota");
        let token = "admin-token-must-not-leak";
        let token_env = "CODEX_HELPER_CLI_QUOTA_TEST_TOKEN";
        let mut scoped_env = ScopedEnv::default();
        unsafe {
            scoped_env.set_path("CODEX_HELPER_HOME", helper_home.path());
            scoped_env.set(token_env, token);
        }

        let mut expected = quota_view();
        let expected_pool = expected.pools.first_mut().expect("quota pool");
        expected_pool.last_success_at_ms = None;
        expected_pool.last_attempt_at_ms = None;
        expected_pool.epoch_end_ms = None;
        expected_pool.remote_direct_total = None;
        expected_pool.observed_burn = None;
        expected_pool.pacing.projected_remaining_at_reset = None;
        let mut wire_body = serde_json::to_value(operator_read_model(expected.clone()))
            .expect("operator read model JSON");
        let wire_object = wire_body.as_object_mut().expect("operator model object");
        wire_object.insert(
            "validator".to_string(),
            serde_json::json!("validator-must-not-leak"),
        );
        wire_object.insert(
            "raw_payload".to_string(),
            serde_json::json!("raw-payload-must-not-leak"),
        );
        wire_object.insert(
            "credential_url".to_string(),
            serde_json::json!("https://user:credential@relay.invalid/private?token=secret"),
        );
        let wire_pool = wire_body["data"]["quota_analytics"]["pools"][0]
            .as_object_mut()
            .expect("wire quota pool");
        for optional in [
            "last_success_at_ms",
            "last_attempt_at_ms",
            "conversion",
            "epoch_end_ms",
            "remote_direct_total",
            "observed_burn",
        ] {
            wire_pool.remove(optional);
        }
        wire_pool.insert(
            "raw_payload".to_string(),
            serde_json::json!("nested-raw-payload-must-not-leak"),
        );

        let (address, observed, server) = spawn_control_plane(wire_body).await;
        let mut config = HelperConfig::default();
        config.relay_targets.insert(
            "nas".to_string(),
            RelayTargetConfig {
                service: Some(ServiceKind::Codex),
                proxy_url: format!("http://{address}"),
                admin_url: Some(format!("http://{address}")),
                admin_token_env: Some(token_env.to_string()),
            },
        );
        save_helper_config(&config)
            .await
            .expect("write isolated relay config");

        let cli = Cli::try_parse_from([
            "codex-helper",
            "usage",
            "quota",
            "--target",
            "nas",
            "--json",
        ])
        .expect("parse usage quota command");
        let Some(Command::Usage { cmd, .. }) = cli.command else {
            panic!("expected usage command");
        };
        let (target, json) = extract_quota_command(cmd).expect("extract quota command");
        let mut stdout = Vec::new();
        let result = handle_usage_quota(target, json, &mut stdout).await;
        server.abort();
        let _ = server.await;
        result.expect("run usage quota command");

        let text = String::from_utf8(stdout).expect("UTF-8 stdout");
        let decoded: QuotaAnalyticsView =
            serde_json::from_str(&text).expect("deserialize quota view");
        let value: serde_json::Value = serde_json::from_str(&text).expect("parse quota JSON");

        assert_eq!(decoded, expected);
        assert_eq!(value["support"], "supported");
        assert_eq!(value["pools"][0]["remote_used"]["value"], "25");
        assert_eq!(value["pools"][0]["rate_15m"]["status"], "available");
        assert!(value.get("schema_version").is_none());
        assert!(value.get("quota").is_none());
        assert!(value["pools"][0]["last_success_at_ms"].is_null());
        assert!(value["pools"][0]["last_attempt_at_ms"].is_null());
        assert!(value["pools"][0]["remote_direct_total"].is_null());
        assert!(text.contains("reconciliation"));
        for forbidden in [
            "admin_url",
            "install_key",
            token,
            "validator-must-not-leak",
            "raw-payload-must-not-leak",
            "nested-raw-payload-must-not-leak",
            "user:credential",
            "token=secret",
        ] {
            assert!(!text.contains(forbidden), "leaked {forbidden}: {text}");
        }
        let observed = observed
            .lock()
            .expect("read observed request")
            .clone()
            .expect("operator read-model request");
        assert_eq!(observed.0, "/__codex_helper/api/v1/operator/read-model");
        assert_eq!(observed.1.as_deref(), Some(token));
    }

    #[test]
    fn quota_text_covers_pool_pace_source_freshness_and_window() {
        let text = quota_text("nas", "codex", &quota_view());

        assert!(text.contains("Quota analytics for target 'nas'"));
        assert!(text.contains("used=$25"));
        assert!(text.contains("remaining=$75"));
        assert!(text.contains("15m=$3/h"));
        assert!(text.contains("60m=$4/h"));
        assert!(text.contains("required=$5/h"));
        assert!(text.contains("pace=on_pace"));
        assert!(text.contains("source=usage_provider:new_api_user_self"));
        assert!(text.contains("scope=account"));
        assert!(text.contains("confidence=high"));
        assert!(text.contains("freshness=fresh"));
        assert!(text.contains("window=calendar_day"));
        assert!(text.contains("reset_semantics=explicit_timestamp"));
        assert!(text.contains("omitted_pools=2"));
    }

    #[test]
    fn quota_text_distinguishes_unsupported_and_supported_empty() {
        let unsupported = quota_text("local", "codex", &QuotaAnalyticsView::default());
        assert!(unsupported.contains("not supported"));

        let empty = quota_text(
            "local",
            "codex",
            &QuotaAnalyticsView {
                support: QuotaAnalyticsSupport::Supported,
                generated_at_ms: 42,
                ..QuotaAnalyticsView::default()
            },
        );
        assert!(empty.contains("No quota pools reported"));
    }
}

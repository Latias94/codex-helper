use crate::control_plane_client::ControlPlaneClient;
use crate::dashboard_core::{
    OperatorReadData, OperatorReadModel, OperatorReadStatus, OperatorRequestSummary,
};
use crate::request_chain::RequestChainSelector;
use crate::request_ledger::{
    RequestUsageSummary, RequestUsageSummaryGroup, RequestUsageSummaryRow,
};
use crate::{CliError, CliResult, UsageCommand, UsageSummaryBy};
use codex_helper_core::runtime_identity::ProviderEndpointKey;
use owo_colors::OwoColorize;

pub async fn handle_usage_cmd(
    cmd: UsageCommand,
    client: &ControlPlaneClient,
    model: OperatorReadModel,
) -> CliResult<()> {
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
    use crate::dashboard_core::OperatorRequestObservability;
    use crate::pricing::CostBreakdown;
    use crate::request_ledger::{RequestUsageAggregate, RequestUsageSummaryCoverage};
    use crate::usage::UsageMetrics;

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
}

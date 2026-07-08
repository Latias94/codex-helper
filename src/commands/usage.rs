use crate::request_chain::RequestChainSelector;
use crate::request_ledger::{RequestLedgerStore, RequestLogFilters, RequestUsageSummaryGroup};
use crate::{CliError, CliResult, UsageCommand, UsageSummaryBy};
use owo_colors::OwoColorize;

pub async fn handle_usage_cmd(cmd: UsageCommand) -> CliResult<()> {
    let store = RequestLedgerStore::default();
    if !store.exists() {
        println!("No request logs found at {:?}", store.path());
        return Ok(());
    }

    match cmd {
        UsageCommand::Tail { limit, raw } => {
            let lines = store.tail_lines(limit).map_err(|err| {
                CliError::Usage(format!("无法打开请求日志 {:?}: {}", store.path(), err))
            })?;
            for line in lines {
                if raw {
                    println!("{}", line.raw());
                    continue;
                }
                for out in line.display_lines() {
                    println!("{out}");
                }
            }
        }
        UsageCommand::Summary { limit, by } => {
            let group = RequestUsageSummaryGroup::from(by);
            let rows = store
                .summarize(group, &RequestLogFilters::default(), limit)
                .map_err(|err| {
                    CliError::Usage(format!("无法打开请求日志 {:?}: {}", store.path(), err))
                })?;

            println!(
                "{}",
                format!(
                    "Usage summary by {} (from {:?})",
                    group.column_name(),
                    store.path()
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
            model,
            station,
            provider,
            path,
            status_min,
            status_max,
            errors,
            fast,
            retried,
            raw,
        } => {
            let filters = RequestLogFilters {
                session,
                model,
                station,
                provider,
                path,
                status_min: status_min.or(errors.then_some(400)),
                status_max,
                fast,
                retried,
                signal_kind: None,
                policy_action_kind: None,
                ..RequestLogFilters::default()
            };
            let lines = store.find_lines(&filters, limit).map_err(|err| {
                CliError::Usage(format!("无法打开请求日志 {:?}: {}", store.path(), err))
            })?;

            for line in &lines {
                if raw {
                    println!("{}", line.raw());
                    continue;
                }
                for out in line.display_lines() {
                    println!("{out}");
                }
            }
            if lines.is_empty() && !raw {
                println!(
                    "No request records matched the filters in {:?}.",
                    store.path()
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

            let export = store.export_request_chain(selector, limit).map_err(|err| {
                CliError::Usage(format!("无法打开请求日志 {:?}: {}", store.path(), err))
            })?;
            if json {
                let text = serde_json::to_string_pretty(&export)
                    .map_err(|err| CliError::Usage(format!("无法序列化请求链: {err}")))?;
                println!("{text}");
                return Ok(());
            }

            println!(
                "{}",
                format!(
                    "Request chain export: {} request(s), truncated={} (from {:?})",
                    export.requests.len(),
                    export.truncated,
                    store.path()
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

impl From<UsageSummaryBy> for RequestUsageSummaryGroup {
    fn from(value: UsageSummaryBy) -> Self {
        match value {
            UsageSummaryBy::Station => Self::Station,
            UsageSummaryBy::Provider => Self::Provider,
            UsageSummaryBy::Model => Self::Model,
            UsageSummaryBy::Session => Self::Session,
        }
    }
}

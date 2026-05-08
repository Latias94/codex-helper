use crate::request_ledger::{
    RequestLogFilters, RequestUsageSummaryGroup, find_request_log, request_log_path,
    summarize_request_log, tail_request_log,
};
use crate::{CliError, CliResult, UsageCommand, UsageSummaryBy};
use owo_colors::OwoColorize;

pub async fn handle_usage_cmd(cmd: UsageCommand) -> CliResult<()> {
    let log_path = request_log_path();
    if !log_path.exists() {
        println!("No request logs found at {:?}", log_path);
        return Ok(());
    }

    match cmd {
        UsageCommand::Tail { limit, raw } => {
            let lines = tail_request_log(&log_path, limit).map_err(|err| {
                CliError::Usage(format!("无法打开请求日志 {:?}: {}", log_path, err))
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
            let rows =
                summarize_request_log(&log_path, group, &RequestLogFilters::default(), limit)
                    .map_err(|err| {
                        CliError::Usage(format!("无法打开请求日志 {:?}: {}", log_path, err))
                    })?;

            println!(
                "{}",
                format!(
                    "Usage summary by {} (from {:?})",
                    group.column_name(),
                    log_path
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
                status_min: status_min.or(errors.then_some(400)),
                status_max,
                fast,
                retried,
            };
            let lines = find_request_log(&log_path, &filters, limit).map_err(|err| {
                CliError::Usage(format!("无法打开请求日志 {:?}: {}", log_path, err))
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
                println!("No request records matched the filters in {:?}.", log_path);
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

use crate::config::proxy_home_dir;
use crate::{CliError, CliResult, UsageCommand};
use owo_colors::OwoColorize;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

pub async fn handle_usage_cmd(cmd: UsageCommand) -> CliResult<()> {
    let log_path: PathBuf = proxy_home_dir().join("logs").join("requests.jsonl");
    if !log_path.exists() {
        println!("No request logs found at {:?}", log_path);
        return Ok(());
    }

    match cmd {
        UsageCommand::Tail { limit, raw } => {
            let file = File::open(&log_path)
                .map_err(|e| CliError::Usage(format!("无法打开请求日志 {:?}: {}", log_path, e)))?;
            let reader = BufReader::new(file);
            let lines: Vec<String> = reader.lines().map_while(Result::ok).collect();
            let total = lines.len();
            let start = total.saturating_sub(limit);
            for line in &lines[start..] {
                if raw {
                    // 原样输出 JSON 行，方便 jq/脚本进一步处理
                    println!("{line}");
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<JsonValue>(line) {
                    let ts = v.get("timestamp_ms").and_then(|x| x.as_i64()).unwrap_or(0);
                    let service = v.get("service").and_then(|x| x.as_str()).unwrap_or("-");
                    let method = v.get("method").and_then(|x| x.as_str()).unwrap_or("-");
                    let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("-");
                    let status = v.get("status_code").and_then(|x| x.as_u64()).unwrap_or(0);
                    let station_name = v
                        .get("station_name")
                        .and_then(|x| x.as_str())
                        .or_else(|| v.get("config_name").and_then(|x| x.as_str()))
                        .unwrap_or("-");
                    let total_tokens = v
                        .get("usage")
                        .and_then(|u| u.get("total_tokens"))
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0);
                    println!(
                        "[{}] {} {} {} (station: {}, tokens: {})",
                        ts, service, method, path, station_name, total_tokens
                    );
                    println!("    status: {}", status);
                }
            }
        }
        UsageCommand::Summary { limit } => {
            let file = File::open(&log_path)
                .map_err(|e| CliError::Usage(format!("无法打开请求日志 {:?}: {}", log_path, e)))?;
            let reader = BufReader::new(file);
            let mut aggregate: HashMap<String, (u64, i64, i64, i64)> = HashMap::new();

            for line in reader.lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<JsonValue>(&line) {
                    let station_name = v
                        .get("station_name")
                        .and_then(|x| x.as_str())
                        .or_else(|| v.get("config_name").and_then(|x| x.as_str()))
                        .unwrap_or("-")
                        .to_string();
                    let usage = v.get("usage");
                    if usage.is_none() {
                        continue;
                    }
                    let usage = usage.unwrap();
                    let input = usage
                        .get("input_tokens")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0);
                    let output = usage
                        .get("output_tokens")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0);
                    let total = usage
                        .get("total_tokens")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(input + output);

                    let entry = aggregate
                        .entry(station_name)
                        .or_insert((0u64, 0i64, 0i64, 0i64));
                    entry.0 += 1;
                    entry.1 += input;
                    entry.2 += output;
                    entry.3 += total;
                }
            }

            let mut items: Vec<(String, (u64, i64, i64, i64))> = aggregate.into_iter().collect();
            items.sort_by(|a, b| b.1.3.cmp(&a.1.3));

            println!(
                "{}",
                format!("Usage summary by station (from {:?})", log_path).bold()
            );
            println!(
                "{}",
                "station_name | requests | input_tokens | output_tokens | total_tokens".bold()
            );
            for (name, (count, input, output, total)) in items.into_iter().take(limit) {
                println!("{} | {} | {} | {} | {}", name, count, input, output, total);
            }
        }
    }

    Ok(())
}

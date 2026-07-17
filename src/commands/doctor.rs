use crate::CliResult;
use crate::dashboard_core::{OperatorReadModel, OperatorReadStatus};
use crate::doctor::{DoctorLang, DoctorStatus, run_doctor};
use owo_colors::OwoColorize;

pub async fn handle_status_cmd(
    json: bool,
    codex: &OperatorReadModel,
    claude: &OperatorReadModel,
) -> CliResult<()> {
    if json {
        let payload = serde_json::json!({
            "api_version": 1,
            "source": "operator_read_model",
            "codex": codex,
            "claude": claude,
        });
        let text = serde_json::to_string_pretty(&payload)
            .map_err(|error| crate::CliError::Other(error.to_string()))?;
        println!("{text}");
        return Ok(());
    }

    println!("{}", "codex-helper status".bold());
    println!("{}", "===================".bold());

    print_operator_status("Codex", codex);
    print_operator_status("Claude", claude);

    Ok(())
}

fn print_operator_status(label: &str, model: &OperatorReadModel) {
    let status = match model.status {
        OperatorReadStatus::Ready => "ready".green().to_string(),
        OperatorReadStatus::Stale => "stale".yellow().to_string(),
        OperatorReadStatus::Disconnected => "disconnected".yellow().to_string(),
        OperatorReadStatus::AuthRequired => "auth_required".yellow().to_string(),
    };
    println!("{} {status}", format!("{label} runtime:").bold());

    let Some(data) = model.data.as_ref() else {
        if let Some(issue) = model.issue {
            println!("  issue: {issue:?}");
        }
        return;
    };

    println!("  captured_at_ms: {}", model.captured_at_ms);
    println!(
        "  default profile: {}",
        data.summary
            .runtime
            .default_profile
            .as_deref()
            .unwrap_or("<none>")
    );
    println!(
        "  active requests: {}, recent requests: {}, providers: {}",
        data.summary.counts.active_requests,
        data.summary.counts.recent_requests,
        data.summary.counts.providers
    );
    for provider in &data.summary.providers {
        let state = if provider.effective_enabled {
            "enabled"
        } else {
            "disabled"
        };
        println!(
            "    {} [{state}; routable endpoints: {}/{}]",
            provider.name,
            provider.routable_endpoints,
            provider.endpoints.len()
        );
    }
}

pub async fn handle_doctor_cmd(json: bool) -> CliResult<()> {
    let report = run_doctor(DoctorLang::Zh).await;
    if json {
        let text = serde_json::to_string_pretty(&report)
            .map_err(|error| crate::CliError::Other(error.to_string()))?;
        println!("{text}");
        return Ok(());
    }

    println!("{}", "codex-helper doctor".bold());
    println!("{}", "===================".bold());
    for check in report.checks {
        match check.status {
            DoctorStatus::Ok => println!("{}   {}", "[OK]".green(), check.message),
            DoctorStatus::Info => println!("{} {}", "[INFO]".cyan(), check.message),
            DoctorStatus::Warn => println!("{} {}", "[WARN]".yellow(), check.message),
            DoctorStatus::Fail => println!("{} {}", "[FAIL]".red(), check.message),
        }
    }

    Ok(())
}

/// 辅助函数：对长字符串做安全截断，供 session 输出使用。
pub fn truncate_for_display(s: &str, max_chars: usize) -> String {
    let mut result = String::new();
    let mut count = 0usize;
    for ch in s.chars() {
        if count >= max_chars {
            break;
        }
        result.push(ch);
        count += 1;
    }
    if count < s.chars().count() {
        result.push_str("...");
    }
    result
}

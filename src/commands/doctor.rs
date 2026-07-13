use crate::CliResult;
use crate::codex_switch::{CodexSwitchPhase, inspect as inspect_codex_switch};
use crate::config::{load_config, proxy_home_dir};
use crate::dashboard_core::{OperatorReadModel, OperatorReadStatus};
use crate::logging::request_log_path;
use owo_colors::OwoColorize;
use serde::Serialize;
use std::env;
use std::path::PathBuf;

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

#[derive(Debug, Serialize)]
struct DoctorCheck {
    id: &'static str,
    status: &'static str,
    message: String,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    checks: Vec<DoctorCheck>,
}

pub async fn handle_doctor_cmd(json: bool) -> CliResult<()> {
    let mut checks: Vec<DoctorCheck> = Vec::new();

    if !json {
        println!("{}", "codex-helper doctor".bold());
        println!("{}", "===================".bold());
    }

    // 1) 检查 codex-helper 主配置是否可读
    match load_config().await {
        Ok(cfg) => {
            let codex_count = cfg.codex.providers.len();
            if codex_count == 0 {
                let msg = "检测到 canonical ~/.codex-helper/config.toml 中尚无 Codex provider；请用 `codex-helper provider add` 显式添加。".to_string();
                if !json {
                    println!("{} {}", "[WARN]".yellow(), msg);
                }
                checks.push(DoctorCheck {
                    id: "proxy_config.codex",
                    status: "warn",
                    message: msg,
                });
            } else {
                let msg = format!(
                    "已从 canonical ~/.codex-helper/config.toml 读取到 {} 个 Codex provider",
                    codex_count
                );
                if !json {
                    println!("{}   {}", "[OK]".green(), msg);
                }
                checks.push(DoctorCheck {
                    id: "proxy_config.codex",
                    status: "ok",
                    message: msg,
                });
            }

            // 1.1) 认证与安全性检查：缺失环境变量 / 明文密钥落盘
            fn env_is_set(key: &str) -> bool {
                env::var(key)
                    .ok()
                    .map(|v| !v.trim().is_empty())
                    .unwrap_or(false)
            }

            for (svc_label, view) in [("Codex", &cfg.codex), ("Claude", &cfg.claude)] {
                for (provider_id, provider) in &view.providers {
                    let auth = provider.effective_auth();
                    if let Some(env_name) = auth.auth_token_env.as_deref()
                        && !env_is_set(env_name)
                    {
                        let msg = format!(
                            "{} provider '{}' 缺少环境变量 {}（Bearer token）；请在运行 codex-helper 前设置该变量",
                            svc_label, provider_id, env_name
                        );
                        if !json {
                            println!("{} {}", "[WARN]".yellow(), msg);
                        }
                        checks.push(DoctorCheck {
                            id: "proxy_config.auth.env_missing",
                            status: "warn",
                            message: msg,
                        });
                    }
                    if let Some(env_name) = auth.api_key_env.as_deref()
                        && !env_is_set(env_name)
                    {
                        let msg = format!(
                            "{} provider '{}' 缺少环境变量 {}（X-API-Key）；请在运行 codex-helper 前设置该变量",
                            svc_label, provider_id, env_name
                        );
                        if !json {
                            println!("{} {}", "[WARN]".yellow(), msg);
                        }
                        checks.push(DoctorCheck {
                            id: "proxy_config.auth.env_missing",
                            status: "warn",
                            message: msg,
                        });
                    }
                    if [&provider.auth, &provider.inline_auth]
                        .into_iter()
                        .any(|auth| {
                            auth.auth_token
                                .as_deref()
                                .is_some_and(|value| !value.trim().is_empty())
                                || auth
                                    .api_key
                                    .as_deref()
                                    .is_some_and(|value| !value.trim().is_empty())
                        })
                    {
                        let msg = format!(
                            "{} provider '{}' 在 ~/.codex-helper/config.toml 中检测到明文密钥字段（建议改用 auth_token_env/api_key_env 以避免落盘泄露）",
                            svc_label, provider_id
                        );
                        if !json {
                            println!("{} {}", "[WARN]".yellow(), msg);
                        }
                        checks.push(DoctorCheck {
                            id: "proxy_config.auth.plaintext",
                            status: "warn",
                            message: msg,
                        });
                    }
                }
            }
        }
        Err(err) => {
            let msg = format!(
                "无法读取 canonical ~/.codex-helper/config.toml：{}；请确认它是有效的 version = 5 TOML。",
                err
            );
            if !json {
                println!("{} {}", "[FAIL]".red(), msg);
            }
            checks.push(DoctorCheck {
                id: "proxy_config.codex",
                status: "fail",
                message: msg,
            });
        }
    }

    // 2) Helper-owned explicit Codex switch state. This path never reads auth.json.
    let switch_check = match inspect_codex_switch() {
        Ok(status) if status.phase == CodexSwitchPhase::Off => DoctorCheck {
            id: "codex.switch_state",
            status: "info",
            message:
                "未检测到 codex-helper 显式 switch state；doctor 不推断或导入 Codex CLI 配置。"
                    .to_string(),
        },
        Ok(status) if status.phase == CodexSwitchPhase::Applied && status.enabled => DoctorCheck {
            id: "codex.switch_state",
            status: "ok",
            message: format!(
                "显式 switch state 与当前 Codex helper stanza 一致（base_url = {}）。",
                status.base_url.as_deref().unwrap_or("<missing>")
            ),
        },
        Ok(status) => DoctorCheck {
            id: "codex.switch_state",
            status: "warn",
            message: format!(
                "Codex switch 状态需要核对；请运行 `codex-helper switch status`，不要直接覆盖 config.toml。{}",
                status
                    .recovery_reason
                    .as_deref()
                    .map(|reason| format!(" {reason}"))
                    .unwrap_or_default()
            ),
        },
        Err(err) => DoctorCheck {
            id: "codex.switch_state",
            status: "warn",
            message: format!("无法验证 codex-helper 显式 switch state：{err}"),
        },
    };
    if !json {
        match switch_check.status {
            "ok" => println!("{}   {}", "[OK]".green(), switch_check.message),
            "warn" => println!("{} {}", "[WARN]".yellow(), switch_check.message),
            _ => println!("{} {}", "[INFO]".cyan(), switch_check.message),
        }
    }
    checks.push(switch_check);

    // 3) Check request logs and usage_providers configuration.
    let log_path = request_log_path();
    if log_path.exists() {
        let msg = format!("检测到请求日志文件：{:?}", log_path);
        if !json {
            println!("{}   {}", "[OK]".green(), msg);
        }
        checks.push(DoctorCheck {
            id: "logs.requests",
            status: "ok",
            message: msg,
        });
    } else {
        let msg = format!(
            "尚未生成请求日志：{:?}，可能尚未通过 codex-helper 代理发送请求",
            log_path
        );
        if !json {
            println!("{} {}", "[INFO]".cyan(), msg);
        }
        checks.push(DoctorCheck {
            id: "logs.requests",
            status: "info",
            message: msg,
        });
    }

    let usage_path: PathBuf = proxy_home_dir().join("usage_providers.json");
    if usage_path.exists() {
        match std::fs::read_to_string(&usage_path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        {
            Some(_) => {
                let msg = format!("检测到用量提供商配置：{:?}", usage_path);
                if !json {
                    println!("{}   {}", "[OK]".green(), msg);
                }
                checks.push(DoctorCheck {
                    id: "usage_providers",
                    status: "ok",
                    message: msg,
                });
            }
            None => {
                let msg = format!(
                    "无法解析 {:?} 为 JSON，用量查询（如 Packy 额度）将不可用",
                    usage_path
                );
                if !json {
                    println!("{} {}", "[WARN]".yellow(), msg);
                }
                checks.push(DoctorCheck {
                    id: "usage_providers",
                    status: "warn",
                    message: msg,
                });
            }
        }
    } else {
        let msg = format!(
            "未找到 {:?}，codex-helper 将在首次需要时写入一个默认示例（当前包含 packycode）",
            usage_path
        );
        if !json {
            println!("{} {}", "[INFO]".cyan(), msg);
        }
        checks.push(DoctorCheck {
            id: "usage_providers",
            status: "info",
            message: msg,
        });
    }

    if json {
        let report = DoctorReport { checks };
        let text =
            serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{\"checks\":[]}".to_string());
        println!("{text}");
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

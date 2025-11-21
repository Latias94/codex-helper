use crate::CliResult;
use crate::config::{
    codex_auth_path, codex_config_path, load_config, probe_codex_bootstrap_from_cli, proxy_home_dir,
};
use owo_colors::OwoColorize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

#[derive(Debug, Serialize)]
struct StatusJson<'a> {
    version: Option<u32>,
    #[serde(borrow)]
    codex: &'a crate::config::ServiceConfigManager,
    #[serde(borrow)]
    claude: &'a crate::config::ServiceConfigManager,
    lb_failure_threshold: u32,
    lb_cooldown_secs: u64,
}

pub async fn handle_status_cmd(json: bool) -> CliResult<()> {
    let cfg = load_config().await?;

    if json {
        let payload = StatusJson {
            version: cfg.version,
            codex: &cfg.codex,
            claude: &cfg.claude,
            lb_failure_threshold: crate::lb::FAILURE_THRESHOLD,
            lb_cooldown_secs: crate::lb::COOLDOWN_SECS,
        };
        let text = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
        println!("{text}");
        return Ok(());
    }

    println!("{}", "codex-helper status".bold());
    println!("{}", "===================".bold());

    if let Some(ver) = cfg.version {
        println!("{} {}", "Config version:".bold(), ver);
    }

    // Codex section
    if cfg.codex.configs.is_empty() {
        println!(
            "{} none in ~/.codex-helper/config.json",
            "Codex configs:".bold()
        );
    } else {
        let active = cfg.codex.active.as_deref();
        println!("{}", "Codex configs in ~/.codex-helper/config.json:".bold());
        for (name, svc) in &cfg.codex.configs {
            let marker = if Some(name.as_str()) == active {
                "*"
            } else {
                " "
            };
            println!("  {} {}", marker, name.bold());
            if svc.upstreams.is_empty() {
                println!("      {}", "<no upstreams configured>".yellow());
            } else {
                for (idx, up) in svc.upstreams.iter().enumerate() {
                    let role = if idx == 0 { "primary" } else { "backup" };
                    println!("      [{}] {} ({})", idx, up.base_url, role);
                }
            }
        }
        println!(
            "  {}",
            format!(
                "LB policy: FAILURE_THRESHOLD = {}, COOLDOWN_SECS = {}",
                crate::lb::FAILURE_THRESHOLD,
                crate::lb::COOLDOWN_SECS
            )
            .dimmed()
        );
    }

    // Claude section
    if cfg.claude.configs.is_empty() {
        println!(
            "{} none in ~/.codex-helper/config.json",
            "Claude configs:".bold()
        );
    } else {
        let active = cfg.claude.active.as_deref();
        println!("{}", "Claude configs in ~/.codex-helper/config.json:".bold());
        for (name, svc) in &cfg.claude.configs {
            let marker = if Some(name.as_str()) == active {
                "*"
            } else {
                " "
            };
            let upstream = svc.upstreams.first();
            let base_url = upstream
                .map(|u| u.base_url.as_str())
                .unwrap_or("<no upstream>");
            println!("  {} {} -> {}", marker, name, base_url);
        }
    }

    Ok(())
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
            let codex_count = cfg.codex.configs.len();
            if codex_count == 0 {
                let msg = "检测到 ~/.codex-helper/config.json 中尚无 Codex upstream 配置；建议使用 `codex-helper config add` 手动添加，或运行 `codex-helper config import-from-codex` 从 Codex CLI 配置导入。".to_string();
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
                    "已从 ~/.codex-helper/config.json 读取到 {} 条 Codex 配置（active = {:?}）",
                    codex_count, cfg.codex.active
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
        }
        Err(err) => {
            let msg = format!(
                "无法读取 ~/.codex-helper/config.json：{}；请检查该文件是否为有效 JSON，或尝试备份后删除以重新初始化。",
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

    // 2) 检查 Codex 官方配置目录与文件
    let codex_cfg_path = codex_config_path();
    let codex_auth_path = codex_auth_path();

    if codex_cfg_path.exists() {
        let msg = format!("检测到 Codex 配置文件：{:?}", codex_cfg_path);
        if !json {
            println!("{}   {}", "[OK]".green(), msg);
        }
        checks.push(DoctorCheck {
            id: "codex.config.toml",
            status: "ok",
            message: msg.clone(),
        });
        match std::fs::read_to_string(&codex_cfg_path)
            .ok()
            .and_then(|s| s.parse::<toml::Value>().ok())
        {
            Some(value) => {
                let provider = value
                    .get("model_provider")
                    .and_then(|v| v.as_str())
                    .unwrap_or("openai");
                let msg = format!(
                    "当前 Codex model_provider = \"{}\"（doctor 仅做读取，不会修改该文件）",
                    provider
                );
                if !json {
                    println!("       {}", msg);
                }
            }
            None => {
                let msg = format!(
                    "无法解析 {:?} 为有效 TOML，codex-helper 将无法自动推导上游配置",
                    codex_cfg_path
                );
                if !json {
                    println!("{} {}", "[WARN]".yellow(), msg);
                }
                checks.push(DoctorCheck {
                    id: "codex.config.toml",
                    status: "warn",
                    message: msg,
                });
            }
        }
    } else {
        let msg = format!(
            "未找到 Codex 配置文件：{:?}；建议先安装并运行 Codex CLI，完成登录和基础配置。",
            codex_cfg_path
        );
        if !json {
            println!("{} {}", "[WARN]".yellow(), msg);
        }
        checks.push(DoctorCheck {
            id: "codex.config.toml",
            status: "warn",
            message: msg,
        });
    }

    if codex_auth_path.exists() {
        let msg = format!("检测到 Codex 认证文件：{:?}", codex_auth_path);
        if !json {
            println!("{}   {}", "[OK]".green(), msg);
        }
        checks.push(DoctorCheck {
            id: "codex.auth.json",
            status: "ok",
            message: msg.clone(),
        });
        match File::open(&codex_auth_path).ok().and_then(|f| {
            let reader = BufReader::new(f);
            serde_json::from_reader::<_, JsonValue>(reader).ok()
        }) {
            Some(json_val) => {
                if let Some(obj) = json_val.as_object() {
                    let api_keys: Vec<_> = obj
                        .iter()
                        .filter_map(|(k, v)| {
                            if k.ends_with("_API_KEY")
                                && v.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false)
                            {
                                Some(k.clone())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if api_keys.is_empty() {
                        let msg = "`~/.codex/auth.json` 中未找到任何 `*_API_KEY` 字段，可能尚未通过 API Key 方式配置 Codex".to_string();
                        if !json {
                            println!("{} {}", "[WARN]".yellow(), msg);
                        }
                        checks.push(DoctorCheck {
                            id: "codex.auth.api_key",
                            status: "warn",
                            message: msg,
                        });
                    } else if api_keys.len() == 1 {
                        let msg = format!(
                            "检测到唯一 API Key 字段：{}（codex-helper 可在缺少 env_key 时自动复用）",
                            api_keys[0]
                        );
                        if !json {
                            println!("{}   {}", "[OK]".green(), msg);
                        }
                        checks.push(DoctorCheck {
                            id: "codex.auth.api_key",
                            status: "ok",
                            message: msg,
                        });
                    } else {
                        let msg = format!(
                            "检测到多个 `*_API_KEY` 字段：{:?}，自动推断 token 时可能需要手动指定 env_key",
                            api_keys
                        );
                        if !json {
                            println!("{} {}", "[WARN]".yellow(), msg);
                        }
                        checks.push(DoctorCheck {
                            id: "codex.auth.api_key",
                            status: "warn",
                            message: msg,
                        });
                    }
                } else {
                    let msg =
                        "`~/.codex/auth.json` 根节点不是 JSON 对象，可能不是 Codex CLI 生成的标准格式"
                            .to_string();
                    if !json {
                        println!("{} {}", "[WARN]".yellow(), msg);
                    }
                    checks.push(DoctorCheck {
                        id: "codex.auth.json",
                        status: "warn",
                        message: msg,
                    });
                }
            }
            None => {
                let msg = format!(
                    "无法解析 {:?} 为 JSON，codex-helper 将无法从中读取 token",
                    codex_auth_path
                );
                if !json {
                    println!("{} {}", "[WARN]".yellow(), msg);
                }
                checks.push(DoctorCheck {
                    id: "codex.auth.json",
                    status: "warn",
                    message: msg,
                });
            }
        }
    } else {
        let msg = format!(
            "未找到 Codex 认证文件：{:?}；建议运行 `codex login` 完成登录，或按照 Codex 文档手动创建 auth.json。",
            codex_auth_path
        );
        if !json {
            println!("{} {}", "[WARN]".yellow(), msg);
        }
        checks.push(DoctorCheck {
            id: "codex.auth.json",
            status: "warn",
            message: msg,
        });
    }

    // 3) 尝试模拟一次从 Codex CLI 配置推导上游（不落盘），用于验证整体链路
    match probe_codex_bootstrap_from_cli().await {
        Ok(()) => {
            let msg = "成功从 ~/.codex/config.toml 与 ~/.codex/auth.json 模拟推导 Codex 上游；如需导入，可运行 `codex-helper config import-from-codex`".to_string();
            if !json {
                println!("{}   {}", "[OK]".green(), msg);
            }
            checks.push(DoctorCheck {
                id: "bootstrap.codex",
                status: "ok",
                message: msg,
            });
        }
        Err(err) => {
            let msg = format!(
                "无法从 ~/.codex 自动推导 Codex 上游：{}；这不会影响手动在 ~/.codex-helper/config.json 中配置上游，但自动导入功能将不可用。",
                err
            );
            if !json {
                println!("{} {}", "[WARN]".yellow(), msg);
            }
            checks.push(DoctorCheck {
                id: "bootstrap.codex",
                status: "warn",
                message: msg,
            });
        }
    }

    // 4) 检查请求日志与 usage_providers 配置
    let log_path: PathBuf = proxy_home_dir().join("logs").join("requests.jsonl");
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

use serde::Serialize;
use serde_json::Value as JsonValue;
use std::env;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use crate::config::{
    codex_auth_path, codex_config_path, load_config, probe_codex_bootstrap_from_cli, proxy_home_dir,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoctorLang {
    Zh,
    En,
}

fn pick(lang: DoctorLang, zh: &'static str, en: &'static str) -> &'static str {
    match lang {
        DoctorLang::Zh => zh,
        DoctorLang::En => en,
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DoctorStatus {
    Ok,
    Info,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    pub id: &'static str,
    pub status: DoctorStatus,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
}

pub async fn run_doctor(lang: DoctorLang) -> DoctorReport {
    let mut checks: Vec<DoctorCheck> = Vec::new();

    // 1) codex-helper main config
    match load_config().await {
        Ok(cfg) => {
            let codex_count = cfg.codex.configs.len();
            if codex_count == 0 {
                checks.push(DoctorCheck {
                    id: "proxy_config.codex",
                    status: DoctorStatus::Warn,
                    message: pick(
                        lang,
                        "检测到 ~/.codex-helper/config.json 中尚无 Codex upstream 配置；建议使用 `codex-helper config add` 手动添加，或运行 `codex-helper config import-from-codex` 从 Codex CLI 配置导入。",
                        "No Codex upstreams found in ~/.codex-helper/config.json; use `codex-helper config add`, or run `codex-helper config import-from-codex` to import from Codex CLI.",
                    )
                    .to_string(),
                });
            } else {
                checks.push(DoctorCheck {
                    id: "proxy_config.codex",
                    status: DoctorStatus::Ok,
                    message: match lang {
                        DoctorLang::Zh => format!(
                            "已从 ~/.codex-helper/config.json 读取到 {} 条 Codex 配置（active = {:?}）",
                            codex_count, cfg.codex.active
                        ),
                        DoctorLang::En => format!(
                            "Loaded {} Codex configs from ~/.codex-helper/config.json (active = {:?})",
                            codex_count, cfg.codex.active
                        ),
                    },
                });
            }

            fn env_is_set(key: &str) -> bool {
                env::var(key).ok().is_some_and(|v| !v.trim().is_empty())
            }

            for (svc_label, mgr) in [("Codex", &cfg.codex), ("Claude", &cfg.claude)] {
                let Some(active_name) = mgr.active.as_deref() else {
                    continue;
                };
                let Some(active_cfg) = mgr.active_config() else {
                    continue;
                };
                for (idx, up) in active_cfg.upstreams.iter().enumerate() {
                    if let Some(env_name) = up.auth.auth_token_env.as_deref()
                        && !env_is_set(env_name)
                    {
                        checks.push(DoctorCheck {
                            id: "proxy_config.auth.env_missing",
                            status: DoctorStatus::Warn,
                            message: match lang {
                                DoctorLang::Zh => format!(
                                    "{} active config '{}' upstream[{}] 缺少环境变量 {}（Bearer token）；请在运行 codex-helper 前设置该变量",
                                    svc_label, active_name, idx, env_name
                                ),
                                DoctorLang::En => format!(
                                    "{} active config '{}' upstream[{}] is missing env var {} (Bearer token); set it before running codex-helper",
                                    svc_label, active_name, idx, env_name
                                ),
                            },
                        });
                    }
                    if let Some(env_name) = up.auth.api_key_env.as_deref()
                        && !env_is_set(env_name)
                    {
                        checks.push(DoctorCheck {
                            id: "proxy_config.auth.env_missing",
                            status: DoctorStatus::Warn,
                            message: match lang {
                                DoctorLang::Zh => format!(
                                    "{} active config '{}' upstream[{}] 缺少环境变量 {}（X-API-Key）；请在运行 codex-helper 前设置该变量",
                                    svc_label, active_name, idx, env_name
                                ),
                                DoctorLang::En => format!(
                                    "{} active config '{}' upstream[{}] is missing env var {} (X-API-Key); set it before running codex-helper",
                                    svc_label, active_name, idx, env_name
                                ),
                            },
                        });
                    }
                    let has_plaintext = up
                        .auth
                        .auth_token
                        .as_deref()
                        .is_some_and(|s| !s.trim().is_empty())
                        || up
                            .auth
                            .api_key
                            .as_deref()
                            .is_some_and(|s| !s.trim().is_empty());
                    if has_plaintext {
                        checks.push(DoctorCheck {
                            id: "proxy_config.auth.plaintext",
                            status: DoctorStatus::Warn,
                            message: match lang {
                                DoctorLang::Zh => format!(
                                    "{} active config '{}' upstream[{}] 在 ~/.codex-helper/config.json 中检测到明文密钥字段（建议改用 auth_token_env/api_key_env 以避免落盘泄露）",
                                    svc_label, active_name, idx
                                ),
                                DoctorLang::En => format!(
                                    "{} active config '{}' upstream[{}] contains plaintext secrets in ~/.codex-helper/config.json (prefer auth_token_env/api_key_env)",
                                    svc_label, active_name, idx
                                ),
                            },
                        });
                    }
                }
            }
        }
        Err(err) => {
            checks.push(DoctorCheck {
                id: "proxy_config.codex",
                status: DoctorStatus::Fail,
                message: match lang {
                    DoctorLang::Zh => format!(
                        "无法读取 ~/.codex-helper/config.json：{}；请检查该文件是否为有效 JSON，或尝试备份后删除以重新初始化。",
                        err
                    ),
                    DoctorLang::En => format!(
                        "Failed to read ~/.codex-helper/config.json: {err}; ensure it is valid JSON, or back it up and reinitialize.",
                    ),
                },
            });
        }
    }

    // 2) Codex CLI config/auth
    let codex_cfg_path = codex_config_path();
    let codex_auth_path = codex_auth_path();

    if codex_cfg_path.exists() {
        checks.push(DoctorCheck {
            id: "codex.config.toml",
            status: DoctorStatus::Ok,
            message: match lang {
                DoctorLang::Zh => format!("检测到 Codex 配置文件：{:?}", codex_cfg_path),
                DoctorLang::En => format!("Found Codex config file: {:?}", codex_cfg_path),
            },
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
                checks.push(DoctorCheck {
                    id: "codex.config.model_provider",
                    status: DoctorStatus::Info,
                    message: match lang {
                        DoctorLang::Zh => format!(
                            "当前 Codex model_provider = \"{}\"（doctor 仅做读取，不会修改该文件）",
                            provider
                        ),
                        DoctorLang::En => format!(
                            "Current Codex model_provider = \"{}\" (doctor is read-only)",
                            provider
                        ),
                    },
                });
            }
            None => checks.push(DoctorCheck {
                id: "codex.config.toml",
                status: DoctorStatus::Warn,
                message: match lang {
                    DoctorLang::Zh => format!(
                        "无法解析 {:?} 为有效 TOML，codex-helper 将无法自动推导上游配置",
                        codex_cfg_path
                    ),
                    DoctorLang::En => format!(
                        "Failed to parse {:?} as TOML; codex-helper cannot infer upstreams from Codex CLI",
                        codex_cfg_path
                    ),
                },
            }),
        }
    } else {
        checks.push(DoctorCheck {
            id: "codex.config.toml",
            status: DoctorStatus::Warn,
            message: match lang {
                DoctorLang::Zh => format!(
                    "未找到 Codex 配置文件：{:?}；建议先安装并运行 Codex CLI，完成登录和基础配置。",
                    codex_cfg_path
                ),
                DoctorLang::En => format!(
                    "Codex config file not found: {:?}; install/run Codex CLI and complete initial setup.",
                    codex_cfg_path
                ),
            },
        });
    }

    if codex_auth_path.exists() {
        checks.push(DoctorCheck {
            id: "codex.auth.json",
            status: DoctorStatus::Ok,
            message: match lang {
                DoctorLang::Zh => format!("检测到 Codex 认证文件：{:?}", codex_auth_path),
                DoctorLang::En => format!("Found Codex auth file: {:?}", codex_auth_path),
            },
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
                                && v.as_str().is_some_and(|s| !s.trim().is_empty())
                            {
                                Some(k.clone())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if api_keys.is_empty() {
                        checks.push(DoctorCheck {
                            id: "codex.auth.api_key",
                            status: DoctorStatus::Warn,
                            message: pick(
                                lang,
                                "`~/.codex/auth.json` 中未找到任何 `*_API_KEY` 字段，可能尚未通过 API Key 方式配置 Codex",
                                "No `*_API_KEY` fields found in ~/.codex/auth.json; Codex may not be configured via API key.",
                            )
                            .to_string(),
                        });
                    } else if api_keys.len() == 1 {
                        checks.push(DoctorCheck {
                            id: "codex.auth.api_key",
                            status: DoctorStatus::Ok,
                            message: match lang {
                                DoctorLang::Zh => {
                                    format!("检测到 Codex API key 字段：{:?}", api_keys)
                                }
                                DoctorLang::En => {
                                    format!("Found Codex API key field: {:?}", api_keys)
                                }
                            },
                        });
                    } else {
                        checks.push(DoctorCheck {
                            id: "codex.auth.api_key",
                            status: DoctorStatus::Warn,
                            message: match lang {
                                DoctorLang::Zh => format!(
                                    "检测到多个 `*_API_KEY` 字段：{:?}，自动推断 token 时可能需要手动指定 env_key",
                                    api_keys
                                ),
                                DoctorLang::En => format!(
                                    "Multiple `*_API_KEY` fields detected: {:?}; inference may require manual env_key selection",
                                    api_keys
                                ),
                            },
                        });
                    }
                } else {
                    checks.push(DoctorCheck {
                        id: "codex.auth.json",
                        status: DoctorStatus::Warn,
                        message: pick(
                            lang,
                            "`~/.codex/auth.json` 根节点不是 JSON 对象，可能不是 Codex CLI 生成的标准格式",
                            "~/.codex/auth.json root is not a JSON object; it may not be in Codex CLI's standard format.",
                        )
                        .to_string(),
                    });
                }
            }
            None => checks.push(DoctorCheck {
                id: "codex.auth.json",
                status: DoctorStatus::Warn,
                message: match lang {
                    DoctorLang::Zh => format!(
                        "无法解析 {:?} 为 JSON，codex-helper 将无法从中读取 token",
                        codex_auth_path
                    ),
                    DoctorLang::En => format!(
                        "Failed to parse {:?} as JSON; codex-helper cannot read tokens from it.",
                        codex_auth_path
                    ),
                },
            }),
        }
    } else {
        checks.push(DoctorCheck {
            id: "codex.auth.json",
            status: DoctorStatus::Warn,
            message: match lang {
                DoctorLang::Zh => format!(
                    "未找到 Codex 认证文件：{:?}；建议运行 `codex login` 完成登录，或按照 Codex 文档手动创建 auth.json。",
                    codex_auth_path
                ),
                DoctorLang::En => format!(
                    "Codex auth file not found: {:?}; run `codex login` or create auth.json per Codex docs.",
                    codex_auth_path
                ),
            },
        });
    }

    // 3) bootstrap probe (no disk write)
    match probe_codex_bootstrap_from_cli().await {
        Ok(()) => checks.push(DoctorCheck {
            id: "bootstrap.codex",
            status: DoctorStatus::Ok,
            message: pick(
                lang,
                "成功从 ~/.codex/config.toml 与 ~/.codex/auth.json 模拟推导 Codex 上游；如需导入，可运行 `codex-helper config import-from-codex`",
                "Successfully inferred Codex upstreams from ~/.codex/config.toml and ~/.codex/auth.json; to import, run `codex-helper config import-from-codex`",
            )
            .to_string(),
        }),
        Err(err) => checks.push(DoctorCheck {
            id: "bootstrap.codex",
            status: DoctorStatus::Warn,
            message: match lang {
                DoctorLang::Zh => format!(
                    "无法从 ~/.codex 自动推导 Codex 上游：{}；这不会影响手动在 ~/.codex-helper/config.json 中配置上游，但自动导入功能将不可用。",
                    err
                ),
                DoctorLang::En => format!(
                    "Failed to infer Codex upstreams from ~/.codex: {err}; manual ~/.codex-helper config still works but auto-import won't.",
                ),
            },
        }),
    }

    // 4) logs and usage_providers
    let log_path: PathBuf = proxy_home_dir().join("logs").join("requests.jsonl");
    if log_path.exists() {
        checks.push(DoctorCheck {
            id: "logs.requests",
            status: DoctorStatus::Ok,
            message: match lang {
                DoctorLang::Zh => format!("检测到请求日志文件：{:?}", log_path),
                DoctorLang::En => format!("Found request logs: {:?}", log_path),
            },
        });
    } else {
        checks.push(DoctorCheck {
            id: "logs.requests",
            status: DoctorStatus::Info,
            message: match lang {
                DoctorLang::Zh => format!(
                    "尚未生成请求日志：{:?}，可能尚未通过 codex-helper 代理发送请求",
                    log_path
                ),
                DoctorLang::En => format!(
                    "Request logs not found: {:?}; you may not have sent requests through codex-helper yet.",
                    log_path
                ),
            },
        });
    }

    let usage_path: PathBuf = proxy_home_dir().join("usage_providers.json");
    if usage_path.exists() {
        match std::fs::read_to_string(&usage_path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        {
            Some(_) => checks.push(DoctorCheck {
                id: "usage_providers",
                status: DoctorStatus::Ok,
                message: match lang {
                    DoctorLang::Zh => format!("检测到用量提供商配置：{:?}", usage_path),
                    DoctorLang::En => format!("Found usage providers config: {:?}", usage_path),
                },
            }),
            None => checks.push(DoctorCheck {
                id: "usage_providers",
                status: DoctorStatus::Warn,
                message: match lang {
                    DoctorLang::Zh => format!(
                        "无法解析 {:?} 为 JSON，用量查询（如 Packy 额度）将不可用",
                        usage_path
                    ),
                    DoctorLang::En => format!(
                        "Failed to parse {:?} as JSON; usage queries (e.g. Packy quota) will be unavailable.",
                        usage_path
                    ),
                },
            }),
        }
    } else {
        checks.push(DoctorCheck {
            id: "usage_providers",
            status: DoctorStatus::Info,
            message: match lang {
                DoctorLang::Zh => format!(
                    "未找到 {:?}，codex-helper 将在首次需要时写入一个默认示例（当前包含 packycode）",
                    usage_path
                ),
                DoctorLang::En => format!(
                    "{:?} not found; codex-helper will write a default example when needed (currently includes packycode).",
                    usage_path
                ),
            },
        });
    }

    DoctorReport { checks }
}

use serde::Serialize;
use std::env;
use std::path::PathBuf;

use crate::codex_switch::{CodexSwitchPhase, inspect as inspect_codex_switch};
use crate::config::{load_config, proxy_home_dir};
use crate::logging::request_log_path;

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
            let codex_count = cfg.codex.providers.len();
            if codex_count == 0 {
                checks.push(DoctorCheck {
                    id: "proxy_config.codex",
                    status: DoctorStatus::Warn,
                    message: pick(
                        lang,
                        "检测到 canonical ~/.codex-helper/config.toml 中尚无 Codex provider；请用 `codex-helper provider add` 显式添加。",
                        "No Codex providers found in canonical ~/.codex-helper/config.toml; add one explicitly with `codex-helper provider add`.",
                    )
                    .to_string(),
                });
            } else {
                checks.push(DoctorCheck {
                    id: "proxy_config.codex",
                    status: DoctorStatus::Ok,
                    message: match lang {
                        DoctorLang::Zh => format!(
                            "已从 canonical ~/.codex-helper/config.toml 读取到 {} 个 Codex provider",
                            codex_count
                        ),
                        DoctorLang::En => format!(
                            "Loaded {} Codex providers from canonical ~/.codex-helper/config.toml",
                            codex_count
                        ),
                    },
                });
            }

            fn env_is_set(key: &str) -> bool {
                env::var(key).ok().is_some_and(|v| !v.trim().is_empty())
            }

            for (svc_label, view) in [("Codex", &cfg.codex), ("Claude", &cfg.claude)] {
                for (provider_id, provider) in &view.providers {
                    let auth = provider.effective_auth();
                    if let Some(env_name) = auth.auth_token_env.as_deref()
                        && !env_is_set(env_name)
                    {
                        checks.push(DoctorCheck {
                            id: "proxy_config.auth.env_missing",
                            status: DoctorStatus::Warn,
                            message: match lang {
                                DoctorLang::Zh => format!(
                                    "{} provider '{}' 缺少环境变量 {}（Bearer token）；请在运行 codex-helper 前设置该变量",
                                    svc_label, provider_id, env_name
                                ),
                                DoctorLang::En => format!(
                                    "{} provider '{}' is missing env var {} (Bearer token); set it before running codex-helper",
                                    svc_label, provider_id, env_name
                                ),
                            },
                        });
                    }
                    if let Some(env_name) = auth.api_key_env.as_deref()
                        && !env_is_set(env_name)
                    {
                        checks.push(DoctorCheck {
                            id: "proxy_config.auth.env_missing",
                            status: DoctorStatus::Warn,
                            message: match lang {
                                DoctorLang::Zh => format!(
                                    "{} provider '{}' 缺少环境变量 {}（X-API-Key）；请在运行 codex-helper 前设置该变量",
                                    svc_label, provider_id, env_name
                                ),
                                DoctorLang::En => format!(
                                    "{} provider '{}' is missing env var {} (X-API-Key); set it before running codex-helper",
                                    svc_label, provider_id, env_name
                                ),
                            },
                        });
                    }
                    let has_plaintext =
                        [&provider.auth, &provider.inline_auth]
                            .into_iter()
                            .any(|auth| {
                                auth.auth_token
                                    .as_deref()
                                    .is_some_and(|value| !value.trim().is_empty())
                                    || auth
                                        .api_key
                                        .as_deref()
                                        .is_some_and(|value| !value.trim().is_empty())
                            });
                    if has_plaintext {
                        checks.push(DoctorCheck {
                            id: "proxy_config.auth.plaintext",
                            status: DoctorStatus::Warn,
                            message: match lang {
                                DoctorLang::Zh => format!(
                                    "{} provider '{}' 在 ~/.codex-helper/config.toml 中检测到明文密钥字段（建议改用 auth_token_env/api_key_env 以避免落盘泄露）",
                                    svc_label, provider_id
                                ),
                                DoctorLang::En => format!(
                                    "{} provider '{}' contains plaintext secrets in ~/.codex-helper/config.toml (prefer auth_token_env/api_key_env)",
                                    svc_label, provider_id
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
                        "无法读取 canonical ~/.codex-helper/config.toml：{}；请确认它是有效的 version = 5 TOML。",
                        err
                    ),
                    DoctorLang::En => format!(
                        "Failed to read canonical ~/.codex-helper/config.toml: {err}; ensure it is valid version = 5 TOML.",
                    ),
                },
            });
        }
    }

    // 2) Helper-owned explicit Codex switch state. This path never reads auth.json.
    match inspect_codex_switch() {
        Ok(status) if status.phase == CodexSwitchPhase::Off => checks.push(DoctorCheck {
            id: "codex.switch_state",
            status: DoctorStatus::Info,
            message: pick(
                lang,
                "未检测到 codex-helper 显式 switch state；doctor 不推断或导入 Codex CLI 配置。",
                "No explicit codex-helper switch state found; doctor does not infer or import Codex CLI configuration.",
            )
            .to_string(),
        }),
        Ok(status) if status.phase == CodexSwitchPhase::Applied && status.enabled => checks.push(DoctorCheck {
            id: "codex.switch_state",
            status: DoctorStatus::Ok,
            message: match lang {
                DoctorLang::Zh => format!(
                    "显式 switch state 与当前 Codex helper stanza 一致（base_url = {}）。",
                    status.base_url.as_deref().unwrap_or("<missing>")
                ),
                DoctorLang::En => format!(
                    "Explicit switch state matches the current Codex helper stanza (base_url = {}).",
                    status.base_url.as_deref().unwrap_or("<missing>")
                ),
            },
        }),
        Ok(status) => checks.push(DoctorCheck {
            id: "codex.switch_state",
            status: DoctorStatus::Warn,
            message: pick(
                lang,
                "Codex switch 状态需要核对；请运行 `codex-helper switch status`，不要直接覆盖 config.toml。",
                "The Codex switch state needs reconciliation; run `codex-helper switch status` and do not overwrite config.toml.",
            )
            .to_string()
                + status
                    .recovery_reason
                    .as_deref()
                    .map(|reason| format!(" {reason}"))
                    .as_deref()
                    .unwrap_or(""),
        }),
        Err(err) => checks.push(DoctorCheck {
            id: "codex.switch_state",
            status: DoctorStatus::Warn,
            message: match lang {
                DoctorLang::Zh => format!("无法验证 codex-helper 显式 switch state：{err}"),
                DoctorLang::En => {
                    format!("Failed to validate explicit codex-helper switch state: {err}")
                }
            },
        }),
    }

    // 3) logs and usage_providers
    let log_path = request_log_path();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};

    struct ScopedEnv {
        saved: Vec<(String, Option<String>)>,
    }

    impl ScopedEnv {
        fn new() -> Self {
            Self { saved: Vec::new() }
        }

        unsafe fn set(&mut self, key: &str, value: &Path) {
            self.saved.push((key.to_string(), std::env::var(key).ok()));
            unsafe { std::env::set_var(key, value) };
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            for (key, previous) in self.saved.drain(..).rev() {
                unsafe {
                    match previous {
                        Some(value) => std::env::set_var(&key, value),
                        None => std::env::remove_var(&key),
                    }
                }
            }
        }
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        match LOCK.get_or_init(|| Mutex::new(())).lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[test]
    fn doctor_ignores_codex_auth_and_import_feasibility() {
        let _lock = env_lock();
        let home =
            std::env::temp_dir().join(format!("codex-helper-doctor-test-{}", uuid::Uuid::new_v4()));
        let helper_home = home.join(".codex-helper");
        let codex_home = home.join(".codex");
        std::fs::create_dir_all(&helper_home).expect("create helper home");
        std::fs::create_dir_all(&codex_home).expect("create Codex home");

        let mut env = ScopedEnv::new();
        unsafe {
            env.set("HOME", &home);
            env.set("USERPROFILE", &home);
            env.set("CODEX_HELPER_HOME", &helper_home);
            env.set("CODEX_HOME", &codex_home);
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let report = runtime.block_on(async {
            crate::config::init_config_toml(true)
                .await
                .expect("write canonical config");
            std::fs::write(codex_home.join("auth.json"), "not-json")
                .expect("write invalid auth file");
            run_doctor(DoctorLang::En).await
        });
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.id == "proxy_config.codex")
        );
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.id == "codex.switch_state")
        );
        assert!(report.checks.iter().all(|check| {
            !check.id.starts_with("codex.auth")
                && check.id != "bootstrap.codex"
                && !check.message.contains("import-from-codex")
        }));
    }
}

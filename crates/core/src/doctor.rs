use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::auth_resolution::target_credential_readiness;
use crate::codex_onboarding::{CodexOnboardingFeasibility, inspect_codex_onboarding_feasibility};
use crate::codex_switch::{CodexSwitchPhase, CodexSwitchStatus, inspect as inspect_codex_switch};
use crate::config::{
    CURRENT_CONFIG_VERSION, CodexClientPatchConfig, CodexClientPreset, CodexCompactionStrategy,
    CodexHostedImageGenerationMode, HelperConfig, ServiceKind, ServiceRouteConfig, UpstreamAuth,
    load_config, proxy_home_dir,
};
use crate::credentials::{
    CredentialBindingKind, CredentialCandidateInput, CredentialReadinessCode,
    CredentialReadinessDetail, CredentialReadinessEvaluator, CredentialSourceCapabilities,
    InstallationIdentity,
};
use crate::logging::request_log_path;
use crate::relay_target::default_proxy_port_for_service_kind;
use crate::routing_ir::CompiledRouteGraph;
use crate::runtime_identity::ProviderEndpointKey;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration: Option<ConfigurationStatusSnapshot>,
    pub checks: Vec<DoctorCheck>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConfigurationStatusSnapshot {
    pub config_version: u32,
    pub codex: ConfigurationServiceStatusSnapshot,
    pub claude: ConfigurationServiceStatusSnapshot,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConfigurationServiceStatusSnapshot {
    pub service_name: String,
    pub default_profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_patch: Option<ConfigurationClientPatchStatusSnapshot>,
    pub providers: Vec<ConfigurationProviderStatusSnapshot>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct ConfigurationClientPatchStatusSnapshot {
    pub preset: CodexClientPreset,
    pub responses_websocket: bool,
    pub compaction: CodexCompactionStrategy,
    pub translate_models: bool,
    pub hosted_image_generation: CodexHostedImageGenerationMode,
}

impl From<CodexClientPatchConfig> for ConfigurationClientPatchStatusSnapshot {
    fn from(client_patch: CodexClientPatchConfig) -> Self {
        Self {
            preset: client_patch.preset,
            responses_websocket: client_patch.responses_websocket,
            compaction: client_patch.compaction,
            translate_models: client_patch.translate_models,
            hosted_image_generation: client_patch.hosted_image_generation,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConfigurationProviderStatusSnapshot {
    pub provider_id: String,
    pub alias: Option<String>,
    pub enabled: bool,
    pub in_route_graph: bool,
    pub endpoints: Vec<ConfigurationEndpointStatusSnapshot>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConfigurationEndpointStatusSnapshot {
    pub endpoint_id: String,
    pub enabled: bool,
    pub in_route_graph: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_order: Option<usize>,
}

pub fn configuration_status_snapshot(config: &HelperConfig) -> Result<ConfigurationStatusSnapshot> {
    Ok(ConfigurationStatusSnapshot {
        config_version: config.version,
        codex: configuration_service_status_snapshot("codex", &config.codex)?,
        claude: configuration_service_status_snapshot("claude", &config.claude)?,
    })
}

fn configuration_service_status_snapshot(
    service_name: &str,
    view: &ServiceRouteConfig,
) -> Result<ConfigurationServiceStatusSnapshot> {
    let graph = CompiledRouteGraph::compile(service_name, view)
        .with_context(|| format!("compile {service_name} route graph for status"))?;
    let mut route_endpoints = BTreeMap::<String, Vec<(String, usize)>>::new();
    let mut provider_order = Vec::new();
    let mut seen_providers = BTreeSet::new();
    for candidate in graph.candidates() {
        if seen_providers.insert(candidate.provider_id.clone()) {
            provider_order.push(candidate.provider_id.clone());
        }
        route_endpoints
            .entry(candidate.provider_id.clone())
            .or_default()
            .push((candidate.endpoint_id.clone(), candidate.stable_index));
    }
    for provider_id in view.providers.keys() {
        if seen_providers.insert(provider_id.clone()) {
            provider_order.push(provider_id.clone());
        }
    }

    let providers = provider_order
        .into_iter()
        .filter_map(|provider_id| {
            let provider = view.providers.get(&provider_id)?;
            let routed = route_endpoints.remove(&provider_id).unwrap_or_default();
            let mut endpoints = Vec::new();
            let mut seen_endpoints = BTreeSet::new();
            for (endpoint_id, route_order) in routed {
                if seen_endpoints.insert(endpoint_id.clone()) {
                    endpoints.push(ConfigurationEndpointStatusSnapshot {
                        enabled: true,
                        endpoint_id,
                        in_route_graph: true,
                        route_order: Some(route_order),
                    });
                }
            }
            if provider
                .base_url
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty())
                && seen_endpoints.insert("default".to_string())
            {
                endpoints.push(ConfigurationEndpointStatusSnapshot {
                    endpoint_id: "default".to_string(),
                    enabled: provider.enabled,
                    in_route_graph: false,
                    route_order: None,
                });
            }
            for (endpoint_id, endpoint) in &provider.endpoints {
                if seen_endpoints.insert(endpoint_id.clone()) {
                    endpoints.push(ConfigurationEndpointStatusSnapshot {
                        endpoint_id: endpoint_id.clone(),
                        enabled: provider.enabled && endpoint.enabled,
                        in_route_graph: false,
                        route_order: None,
                    });
                }
            }
            Some(ConfigurationProviderStatusSnapshot {
                provider_id,
                alias: provider.alias.clone(),
                enabled: provider.enabled,
                in_route_graph: endpoints.iter().any(|endpoint| endpoint.in_route_graph),
                endpoints,
            })
        })
        .collect();

    Ok(ConfigurationServiceStatusSnapshot {
        service_name: service_name.to_string(),
        default_profile: view.default_profile.clone(),
        client_patch: (service_name == "codex").then(|| {
            ConfigurationClientPatchStatusSnapshot::from(view.client_patch.unwrap_or_default())
        }),
        providers,
    })
}

#[derive(Clone)]
struct DoctorCredentialTarget {
    service_label: &'static str,
    provider_endpoint: ProviderEndpointKey,
    base_url: String,
    auth: UpstreamAuth,
}

struct DoctorCredentialObservation {
    service_label: &'static str,
    provider_endpoint: ProviderEndpointKey,
    details: Vec<CredentialReadinessDetail>,
}

async fn capture_doctor_credential_observations(
    config: HelperConfig,
    credential_sources: CredentialSourceCapabilities,
) -> Result<Vec<DoctorCredentialObservation>> {
    tokio::task::spawn_blocking(move || {
        let installation = InstallationIdentity::resolve_default()
            .context("resolve canonical installation identity")?;
        let targets = doctor_credential_targets(&config)?;
        let evaluator = CredentialReadinessEvaluator::new(credential_sources, installation);
        let mut evaluated =
            evaluator.evaluate(targets.iter().map(|target| CredentialCandidateInput {
                provider_endpoint: target.provider_endpoint.clone(),
                auth: &target.auth,
            }))?;

        targets
            .into_iter()
            .map(|target| {
                let endpoint = evaluated.remove(&target.provider_endpoint).ok_or_else(|| {
                    anyhow::anyhow!("credential evaluator omitted {}", target.provider_endpoint)
                })?;
                let code = target_credential_readiness(
                    target.provider_endpoint.service_name.as_str(),
                    endpoint.configured_contract,
                    endpoint.allow_anonymous,
                    target.base_url.as_str(),
                    endpoint.code,
                );
                let mut details = endpoint.details;
                if code == CredentialReadinessCode::Missing && details.is_empty() {
                    details.push(CredentialReadinessDetail {
                        kind: None,
                        code,
                        stale_cause: None,
                        source_kind: Some("configuration".to_string()),
                        reference: None,
                    });
                }
                Ok(DoctorCredentialObservation {
                    service_label: target.service_label,
                    provider_endpoint: target.provider_endpoint,
                    details,
                })
            })
            .collect()
    })
    .await
    .context("credential readiness task failed")?
}

fn doctor_credential_targets(config: &HelperConfig) -> Result<Vec<DoctorCredentialTarget>> {
    let mut targets = Vec::new();
    for (service_name, service_label, view) in [
        ("codex", "Codex", &config.codex),
        ("claude", "Claude", &config.claude),
    ] {
        let graph = CompiledRouteGraph::compile(service_name, view)
            .with_context(|| format!("compile {service_name} route graph for doctor"))?;
        targets.extend(
            graph
                .candidates()
                .iter()
                .map(|candidate| DoctorCredentialTarget {
                    service_label,
                    provider_endpoint: ProviderEndpointKey::new(
                        service_name,
                        candidate.provider_id.clone(),
                        candidate.endpoint_id.clone(),
                    ),
                    base_url: candidate.base_url.clone(),
                    auth: candidate.auth.clone(),
                }),
        );
    }
    Ok(targets)
}

pub async fn run_doctor(
    lang: DoctorLang,
    credential_sources: CredentialSourceCapabilities,
) -> DoctorReport {
    let mut checks: Vec<DoctorCheck> = Vec::new();
    let mut configuration = None;
    let mut configured_codex_patch = None;
    let mut loaded_config = None;

    // 1) codex-helper main config
    match load_config().await {
        Ok(cfg) => {
            match configuration_status_snapshot(&cfg) {
                Ok(snapshot) => configuration = Some(snapshot),
                Err(error) => checks.push(DoctorCheck {
                    id: "proxy_config.routes",
                    status: DoctorStatus::Fail,
                    message: match lang {
                        DoctorLang::Zh => format!("无法编译 canonical 路由快照：{error}"),
                        DoctorLang::En => {
                            format!("Failed to compile the canonical route snapshot: {error}")
                        }
                    },
                }),
            }
            let client_patch = cfg.codex.client_patch.unwrap_or_default();
            configured_codex_patch = Some(client_patch);
            checks.push(DoctorCheck {
                id: "codex.client_patch.configured",
                status: DoctorStatus::Info,
                message: match lang {
                    DoctorLang::Zh => format!(
                        "canonical Codex client patch：{}",
                        codex_client_patch_summary(&client_patch)
                    ),
                    DoctorLang::En => format!(
                        "Canonical Codex client patch: {}",
                        codex_client_patch_summary(&client_patch)
                    ),
                },
            });
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

            match capture_doctor_credential_observations(cfg.clone(), credential_sources).await {
                Ok(observations) => {
                    for observation in &observations {
                        append_credential_readiness_checks(&mut checks, lang, observation);
                    }
                }
                Err(error) => checks.push(DoctorCheck {
                    id: "proxy_config.auth.readiness",
                    status: DoctorStatus::Fail,
                    message: match lang {
                        DoctorLang::Zh => {
                            format!("无法从 canonical credential runtime 评估凭据状态：{error}")
                        }
                        DoctorLang::En => format!(
                            "Failed to evaluate credential readiness through the canonical credential runtime: {error}"
                        ),
                    },
                }),
            }

            for (svc_label, view) in [("Codex", &cfg.codex), ("Claude", &cfg.claude)] {
                for (provider_id, provider) in &view.providers {
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
            loaded_config = Some(cfg);
        }
        Err(err) => {
            checks.push(DoctorCheck {
                id: "proxy_config.codex",
                status: DoctorStatus::Fail,
                message: match lang {
                    DoctorLang::Zh => format!(
                        "无法读取 canonical ~/.codex-helper/config.toml：{}；请确认它是有效的 version = {} TOML。",
                        err, CURRENT_CONFIG_VERSION
                    ),
                    DoctorLang::En => format!(
                        "Failed to read canonical ~/.codex-helper/config.toml: {err}; ensure it is valid version = {CURRENT_CONFIG_VERSION} TOML.",
                    ),
                },
            });
        }
    }

    // 2) Helper-owned explicit Codex switch state and retained auth recovery health.
    let switch_status = inspect_codex_switch();
    match switch_status.as_ref() {
        Ok(status) if status.phase == CodexSwitchPhase::Off && status.managed => {
            checks.push(DoctorCheck {
                id: "codex.switch_state",
                status: DoctorStatus::Info,
                message: pick(
                    lang,
                    "Codex 本地 switch 已关闭；codex-helper 仍保留认证恢复点，用于修复旧 Codex 进程延迟写回的 facade。",
                    "The local Codex switch is off; codex-helper retains an auth recovery point for repairing a delayed facade write from an older Codex process.",
                )
                .to_string(),
            });
        }
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
    if let Ok(status) = switch_status.as_ref() {
        append_applied_client_patch_check(
            &mut checks,
            lang,
            configured_codex_patch.as_ref(),
            status,
        );
    }
    if let Some(config) = loaded_config.as_ref() {
        append_codex_onboarding_check(
            &mut checks,
            lang,
            inspect_codex_onboarding_feasibility(
                config,
                default_proxy_port_for_service_kind(ServiceKind::Codex),
            ),
        );
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

    DoctorReport {
        configuration,
        checks,
    }
}

fn codex_client_patch_summary(client_patch: &CodexClientPatchConfig) -> String {
    format!(
        "preset={}, responses_websocket={}, compaction={}, translate_models={}, hosted_image_generation={}",
        client_patch.preset,
        client_patch.responses_websocket,
        client_patch.compaction,
        client_patch.translate_models,
        client_patch.hosted_image_generation,
    )
}

fn append_applied_client_patch_check(
    checks: &mut Vec<DoctorCheck>,
    lang: DoctorLang,
    configured: Option<&CodexClientPatchConfig>,
    status: &CodexSwitchStatus,
) {
    let applied = status
        .client_patch
        .as_ref()
        .map(codex_client_patch_summary)
        .unwrap_or_else(|| "<none>".to_string());
    let differs = configured
        .zip(status.client_patch.as_ref())
        .is_some_and(|(configured, applied)| configured != applied);
    let difference = if differs {
        match lang {
            DoctorLang::Zh => "；应用值与当前配置不同，可能来自显式 CLI 覆盖",
            DoctorLang::En => {
                "; the applied value differs from current configuration and may be an explicit CLI override"
            }
        }
    } else {
        ""
    };
    checks.push(DoctorCheck {
        id: "codex.client_patch.applied",
        status: if status.phase == CodexSwitchPhase::RecoveryRequired {
            DoctorStatus::Warn
        } else if status.phase == CodexSwitchPhase::Applied && status.enabled {
            DoctorStatus::Ok
        } else {
            DoctorStatus::Info
        },
        message: match lang {
            DoctorLang::Zh => format!(
                "Codex client 集成：phase={}，applied patch={}{}",
                status.phase.as_str(),
                applied,
                difference
            ),
            DoctorLang::En => format!(
                "Codex client integration: phase={}, applied patch={}{}",
                status.phase.as_str(),
                applied,
                difference
            ),
        },
    });
}

fn append_codex_onboarding_check(
    checks: &mut Vec<DoctorCheck>,
    lang: DoctorLang,
    feasibility: CodexOnboardingFeasibility,
) {
    let (status, message) = match feasibility {
        CodexOnboardingFeasibility::ExistingConfiguration => (
            DoctorStatus::Ok,
            pick(
                lang,
                "canonical Codex 路由已存在；无需自动 onboarding。",
                "A canonical Codex route already exists; automatic onboarding is not required.",
            )
            .to_string(),
        ),
        CodexOnboardingFeasibility::Importable {
            provider_id,
            credential_reference,
        } => {
            let provider_id = escape_doctor_reference(&provider_id);
            let credential = credential_reference
                .as_deref()
                .map(escape_doctor_reference)
                .map(|reference| match lang {
                    DoctorLang::Zh => format!("环境变量引用 `{reference}`"),
                    DoctorLang::En => format!("environment reference `{reference}`"),
                })
                .unwrap_or_else(|| {
                    pick(
                        lang,
                        "Codex client 认证透传",
                        "Codex client authentication passthrough",
                    )
                    .to_string()
                });
            (
                DoctorStatus::Info,
                match lang {
                    DoctorLang::Zh => format!(
                        "原始 Codex client 投影可 onboarding：provider=`{provider_id}`，credential={credential}；doctor 未写入配置。"
                    ),
                    DoctorLang::En => format!(
                        "The original Codex client projection is importable: provider=`{provider_id}`, credential={credential}; doctor did not write configuration."
                    ),
                },
            )
        }
        CodexOnboardingFeasibility::Blocked { reason } => {
            let reason = escape_doctor_reference(&reason);
            (
                DoctorStatus::Warn,
                match lang {
                    DoctorLang::Zh => {
                        format!("原始 Codex client 投影当前无法 onboarding：{reason}")
                    }
                    DoctorLang::En => format!(
                        "The original Codex client projection cannot currently be onboarded: {reason}"
                    ),
                },
            )
        }
    };
    checks.push(DoctorCheck {
        id: "codex.onboarding",
        status,
        message,
    });
}

fn append_credential_readiness_checks(
    checks: &mut Vec<DoctorCheck>,
    lang: DoctorLang,
    observation: &DoctorCredentialObservation,
) {
    for detail in &observation.details {
        if detail.code == CredentialReadinessCode::Ready {
            continue;
        }
        let credential_kind = credential_kind_label(detail.kind, lang);
        let source_kind = detail.source_kind.as_deref().unwrap_or("unreported");
        let (id, remediation) = if detail.code == CredentialReadinessCode::Missing
            && source_kind == "configuration"
        {
            (
                "proxy_config.auth.anonymous_not_allowed",
                match lang {
                    DoctorLang::Zh => {
                        "请配置 helper 凭据；确需匿名访问时显式设置 allow_anonymous = true"
                    }
                    DoctorLang::En => {
                        "configure helper credentials, or set allow_anonymous = true only when anonymous access is intentional"
                    }
                },
            )
        } else {
            credential_remediation(detail.code, lang)
        };
        let reference = detail
            .reference
            .as_deref()
            .map(escape_doctor_reference)
            .unwrap_or_else(|| "<none>".to_string());
        let stale_cause = detail
            .stale_cause
            .map(|cause| match lang {
                DoctorLang::Zh => format!("，最近刷新失败类别={cause}"),
                DoctorLang::En => format!(", last refresh failure={cause}"),
            })
            .unwrap_or_default();
        checks.push(DoctorCheck {
            id,
            status: DoctorStatus::Warn,
            message: match lang {
                DoctorLang::Zh => format!(
                    "{} provider '{}.{}' 的 {} 状态={}（source={}, reference=`{}`{}）：{}",
                    observation.service_label,
                    observation.provider_endpoint.provider_id,
                    observation.provider_endpoint.endpoint_id,
                    credential_kind,
                    detail.code,
                    source_kind,
                    reference,
                    stale_cause,
                    remediation
                ),
                DoctorLang::En => format!(
                    "{} provider '{}.{}' {} readiness={} (source={}, reference=`{}`{}): {}",
                    observation.service_label,
                    observation.provider_endpoint.provider_id,
                    observation.provider_endpoint.endpoint_id,
                    credential_kind,
                    detail.code,
                    source_kind,
                    reference,
                    stale_cause,
                    remediation
                ),
            },
        });
    }
}

fn credential_kind_label(kind: Option<CredentialBindingKind>, lang: DoctorLang) -> &'static str {
    match (kind, lang) {
        (Some(CredentialBindingKind::Bearer), DoctorLang::Zh) => "Bearer token",
        (Some(CredentialBindingKind::Bearer), DoctorLang::En) => "Bearer token",
        (Some(CredentialBindingKind::ApiKey), DoctorLang::Zh) => "X-API-Key",
        (Some(CredentialBindingKind::ApiKey), DoctorLang::En) => "X-API-Key",
        (None, DoctorLang::Zh) => "上游凭据",
        (None, DoctorLang::En) => "upstream credential",
    }
}

fn credential_remediation(
    code: CredentialReadinessCode,
    lang: DoctorLang,
) -> (&'static str, &'static str) {
    match (code, lang) {
        (CredentialReadinessCode::Ready, _) => ("proxy_config.auth.ready", "credential is ready"),
        (CredentialReadinessCode::Stale, DoctorLang::Zh) => (
            "proxy_config.auth.stale",
            "daemon 正在使用 last-known-good 值并会自动重试；请在硬过期前修复凭据源访问",
        ),
        (CredentialReadinessCode::Stale, DoctorLang::En) => (
            "proxy_config.auth.stale",
            "the daemon is using the last-known-good value and will retry automatically; restore source access before hard expiry",
        ),
        (CredentialReadinessCode::Missing, DoctorLang::Zh) => (
            "proxy_config.auth.missing_reference",
            "请为运行服务的账号创建该凭据，或配置其可读取的 env/secret_file；该候选会在本地失败，不会匿名请求上游",
        ),
        (CredentialReadinessCode::Missing, DoctorLang::En) => (
            "proxy_config.auth.missing_reference",
            "create the credential for the service account or use an env/secret_file it can read; the candidate fails locally and never sends an anonymous upstream request",
        ),
        (CredentialReadinessCode::Invalid, DoctorLang::Zh) => (
            "proxy_config.auth.invalid_value",
            "请替换为空值以外且可用于 HTTP header 的凭据值",
        ),
        (CredentialReadinessCode::Invalid, DoctorLang::En) => (
            "proxy_config.auth.invalid_value",
            "replace it with a non-empty credential value valid for an HTTP header",
        ),
        (CredentialReadinessCode::Locked, DoctorLang::Zh) => (
            "proxy_config.auth.locked",
            "请解锁系统凭据存储，并确认运行服务的账号拥有解锁会话",
        ),
        (CredentialReadinessCode::Locked, DoctorLang::En) => (
            "proxy_config.auth.locked",
            "unlock the system credential store and ensure the service account has an unlocked session",
        ),
        (CredentialReadinessCode::PermissionDenied, DoctorLang::Zh) => (
            "proxy_config.auth.permission_denied",
            "请授予运行服务的账号读取该凭据源的权限",
        ),
        (CredentialReadinessCode::PermissionDenied, DoctorLang::En) => (
            "proxy_config.auth.permission_denied",
            "grant the service account read access to this credential source",
        ),
        (CredentialReadinessCode::InteractionRequired, DoctorLang::Zh) => (
            "proxy_config.auth.interaction_required",
            "请在运行服务的账号会话中完成一次系统授权，或改用无需交互的 secret_file/env",
        ),
        (CredentialReadinessCode::InteractionRequired, DoctorLang::En) => (
            "proxy_config.auth.interaction_required",
            "complete system authorization once in the service account session, or use a non-interactive secret_file/env source",
        ),
        (CredentialReadinessCode::BackendUnavailable, DoctorLang::Zh) => (
            "proxy_config.auth.backend_unavailable",
            "请启动或修复系统凭据服务；daemon 会继续按刷新周期重试",
        ),
        (CredentialReadinessCode::BackendUnavailable, DoctorLang::En) => (
            "proxy_config.auth.backend_unavailable",
            "start or repair the system credential service; the daemon will retry on its refresh schedule",
        ),
        (CredentialReadinessCode::Unsupported, DoctorLang::Zh) => (
            "proxy_config.auth.unsupported_reference",
            "当前执行上下文不支持该凭据源；请使用启用 native credentials 的本机 CLI/runtime，或改用 env/secret_file",
        ),
        (CredentialReadinessCode::Unsupported, DoctorLang::En) => (
            "proxy_config.auth.unsupported_reference",
            "this execution context does not support the source; use a local CLI/runtime with native credentials enabled, or switch to env/secret_file",
        ),
    }
}

fn escape_doctor_reference(reference: &str) -> String {
    reference.chars().flat_map(char::escape_default).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_switch::{CodexSwitchIntent, ValidatedCodexBaseUrl};
    use crate::config::{
        CodexClientPatchConfig, CodexClientPreset, CodexCompactionStrategy, CredentialRef,
        HelperConfig, ProviderConfig, RouteGraphConfig, UpstreamAuth,
    };
    use crate::credentials::SecretValue;
    use crate::runtime_store::RuntimeStore;
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
    fn configuration_snapshot_keeps_route_shape_without_credentials_or_origins() {
        const SECRET: &str = "configuration-snapshot-secret-canary";
        let mut config = HelperConfig::default();
        config.codex.default_profile = Some("work".to_string());
        config.codex.client_patch = Some(CodexClientPatchConfig {
            preset: CodexClientPreset::OfficialImagegen,
            compaction: CodexCompactionStrategy::RemoteV2,
            translate_models: true,
            ..CodexClientPatchConfig::default()
        });
        config.codex.providers.insert(
            "primary".to_string(),
            ProviderConfig {
                alias: Some("Primary".to_string()),
                base_url: Some("https://private-origin.example/v1".to_string()),
                auth: UpstreamAuth {
                    auth_token: Some(SECRET.to_string().into()),
                    ..UpstreamAuth::default()
                },
                ..ProviderConfig::default()
            },
        );
        config.codex.providers.insert(
            "disabled".to_string(),
            ProviderConfig {
                enabled: false,
                base_url: Some("https://disabled-private.example/v1".to_string()),
                ..ProviderConfig::default()
            },
        );
        config.codex.routing = Some(RouteGraphConfig::ordered_failover(vec![
            "primary".to_string(),
        ]));

        let snapshot = configuration_status_snapshot(&config).expect("configuration snapshot");

        assert_eq!(snapshot.codex.default_profile.as_deref(), Some("work"));
        assert_eq!(snapshot.codex.providers[0].provider_id, "primary");
        assert!(snapshot.codex.providers[0].in_route_graph);
        assert_eq!(
            snapshot.codex.providers[0].endpoints[0].endpoint_id,
            "default"
        );
        assert_eq!(
            snapshot.codex.providers[0].endpoints[0].route_order,
            Some(0)
        );
        assert_eq!(snapshot.codex.providers[1].provider_id, "disabled");
        assert!(!snapshot.codex.providers[1].enabled);
        assert!(!snapshot.codex.providers[1].in_route_graph);
        let serialized = serde_json::to_value(&snapshot).expect("serialize snapshot");
        let client_patch = &serialized["codex"]["client_patch"];
        assert_eq!(client_patch["preset"], "official-imagegen");
        assert_eq!(client_patch["responses_websocket"], false);
        assert_eq!(client_patch["compaction"], "remote-v2");
        assert_eq!(client_patch["translate_models"], true);
        assert_eq!(client_patch["hosted_image_generation"], "auto");
        let rendered = serialized.to_string();
        assert!(!rendered.contains(SECRET));
        assert!(!rendered.contains("private-origin"));
        assert!(!rendered.contains("disabled-private"));
        assert!(!rendered.contains("auth_token"));
    }

    #[test]
    fn doctor_reports_blocked_onboarding_without_exposing_invalid_auth_contents() {
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
            run_doctor(DoctorLang::En, CredentialSourceCapabilities::server()).await
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
        assert!(report.configuration.is_some());
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.id == "codex.client_patch.configured")
        );
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.id == "codex.client_patch.applied")
        );
        let onboarding = report
            .checks
            .iter()
            .find(|check| check.id == "codex.onboarding")
            .expect("Codex onboarding doctor check");
        assert_eq!(onboarding.status, DoctorStatus::Warn);
        assert!(report.checks.iter().all(|check| {
            !check.id.starts_with("codex.auth")
                && check.id != "bootstrap.codex"
                && !check.message.contains("import-from-codex")
                && !check.message.contains("not-json")
        }));
    }

    #[test]
    fn doctor_reports_configured_and_applied_patch_from_original_projection() {
        const SECRET: &str = "doctor-original-projection-secret-canary";
        let _lock = env_lock();
        let home = std::env::temp_dir().join(format!(
            "codex-helper-doctor-client-patch-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = home.join(".codex-helper");
        let codex_home = home.join(".codex");
        std::fs::create_dir_all(&helper_home).expect("create helper home");
        std::fs::create_dir_all(&codex_home).expect("create Codex home");
        std::fs::write(
            codex_home.join("config.toml"),
            r#"model_provider = "relay"

[model_providers.relay]
name = "Relay"
base_url = "https://relay.example/v1"
env_key = "RELAY_API_KEY"
requires_openai_auth = false
"#,
        )
        .expect("write original Codex config");
        std::fs::write(
            codex_home.join("auth.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "auth_mode": "apikey",
                "RELAY_API_KEY": SECRET,
            }))
            .expect("serialize original Codex auth"),
        )
        .expect("write original Codex auth");

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
            let mut config = HelperConfig::default();
            config.codex.client_patch = Some(CodexClientPatchConfig {
                preset: CodexClientPreset::OfficialRelay,
                ..CodexClientPatchConfig::default()
            });
            crate::config::save_helper_config(&config)
                .await
                .expect("write canonical config");
            crate::codex_switch::apply_with_client_patch(
                CodexSwitchIntent::On {
                    validated_base_url: ValidatedCodexBaseUrl::local(3211),
                },
                CodexClientPatchConfig {
                    preset: CodexClientPreset::ImagegenBridge,
                    ..CodexClientPatchConfig::default()
                },
            )
            .expect("apply explicit client patch");
            run_doctor(DoctorLang::En, CredentialSourceCapabilities::server()).await
        });

        let configured = report
            .checks
            .iter()
            .find(|check| check.id == "codex.client_patch.configured")
            .expect("configured client patch check");
        assert!(configured.message.contains("preset=official-relay"));
        let applied = report
            .checks
            .iter()
            .find(|check| check.id == "codex.client_patch.applied")
            .expect("applied client patch check");
        assert_eq!(applied.status, DoctorStatus::Ok);
        assert!(applied.message.contains("phase=applied"));
        assert!(applied.message.contains("preset=imagegen-bridge"));
        assert!(!applied.message.contains("preset=default"));
        assert!(applied.message.contains("explicit CLI override"));
        let onboarding = report
            .checks
            .iter()
            .find(|check| check.id == "codex.onboarding")
            .expect("onboarding feasibility check");
        assert_eq!(onboarding.status, DoctorStatus::Info);
        assert!(onboarding.message.contains("provider=`relay`"));
        assert!(onboarding.message.contains("`RELAY_API_KEY`"));

        let serialized = serde_json::to_string(&report).expect("serialize doctor report");
        assert!(!serialized.contains(SECRET));
        assert!(!serialized.contains("relay.example"));
    }

    #[test]
    fn doctor_reports_switch_recovery_anomaly_without_exposing_auth_contents() {
        const SECRET: &str = "doctor-recovery-secret-canary";
        let _lock = env_lock();
        let home = std::env::temp_dir().join(format!(
            "codex-helper-doctor-switch-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = home.join(".codex-helper");
        let codex_home = home.join(".codex");
        std::fs::create_dir_all(&helper_home).expect("create helper home");
        std::fs::create_dir_all(&codex_home).expect("create Codex home");
        std::fs::write(
            codex_home.join("config.toml"),
            "model_provider = \"openai\"\n",
        )
        .expect("write original Codex config");
        std::fs::write(
            codex_home.join("auth.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "auth_mode": "apikey",
                "OPENAI_API_KEY": SECRET,
            }))
            .expect("serialize original Codex auth"),
        )
        .expect("write original Codex auth");

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
            crate::config::save_helper_config(&HelperConfig::default())
                .await
                .expect("write canonical config");
            let outcome = crate::codex_switch::apply(CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            })
            .expect("apply Codex switch");
            std::fs::write(outcome.status.state_path, "{broken")
                .expect("corrupt switch journal for diagnosis");
            run_doctor(DoctorLang::En, CredentialSourceCapabilities::server()).await
        });

        let switch = report
            .checks
            .iter()
            .find(|check| check.id == "codex.switch_state")
            .expect("switch recovery check");
        assert_eq!(switch.status, DoctorStatus::Warn);
        let onboarding = report
            .checks
            .iter()
            .find(|check| check.id == "codex.onboarding")
            .expect("onboarding recovery check");
        assert_eq!(onboarding.status, DoctorStatus::Warn);

        let serialized = serde_json::to_string(&report).expect("serialize doctor report");
        assert!(!serialized.contains(SECRET));
        assert!(!serialized.contains("OPENAI_API_KEY"));
    }

    #[test]
    fn doctor_reports_a_retained_auth_recovery_point_while_switch_is_off() {
        let _lock = env_lock();
        let home = std::env::temp_dir().join(format!(
            "codex-helper-doctor-retained-switch-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = home.join(".codex-helper");
        let codex_home = home.join(".codex");
        std::fs::create_dir_all(&helper_home).expect("create helper home");
        std::fs::create_dir_all(&codex_home).expect("create Codex home");
        std::fs::write(
            codex_home.join("config.toml"),
            "model_provider = \"openai\"\n",
        )
        .expect("write Codex config");
        std::fs::write(
            codex_home.join("auth.json"),
            r#"{"auth_mode":"apikey","OPENAI_API_KEY":"doctor-secret"}"#,
        )
        .expect("write Codex auth");

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
            crate::codex_switch::apply_with_client_patch(
                CodexSwitchIntent::On {
                    validated_base_url: ValidatedCodexBaseUrl::local(3211),
                },
                CodexClientPatchConfig {
                    preset: CodexClientPreset::ImagegenBridge,
                    ..CodexClientPatchConfig::default()
                },
            )
            .expect("apply auth facade");
            crate::codex_switch::apply(CodexSwitchIntent::Off).expect("switch off");
            run_doctor(DoctorLang::En, CredentialSourceCapabilities::server()).await
        });
        let check = report
            .checks
            .iter()
            .find(|check| check.id == "codex.switch_state")
            .expect("switch-state doctor check");
        assert_eq!(check.status, DoctorStatus::Info);
        assert!(check.message.contains("retains an auth recovery point"));
        assert!(
            !check
                .message
                .contains("No explicit codex-helper switch state")
        );
    }

    #[test]
    fn doctor_uses_canonical_readiness_instead_of_environment_only_checks() {
        let _lock = env_lock();
        let home =
            std::env::temp_dir().join(format!("codex-helper-doctor-auth-{}", uuid::Uuid::new_v4()));
        let helper_home = home.join(".codex-helper");
        let codex_home = home.join(".codex");
        std::fs::create_dir_all(&helper_home).expect("create helper home");
        std::fs::create_dir_all(&codex_home).expect("create Codex home");

        let resolved_reference = format!(
            "CODEX_HELPER_TEST_DOCTOR_RESOLVED_{}",
            uuid::Uuid::new_v4().simple()
        );
        let missing_reference = format!(
            "CODEX_HELPER_TEST_DOCTOR_MISSING_{}",
            uuid::Uuid::new_v4().simple()
        );
        let invalid_reference = format!(
            "CODEX_HELPER_TEST_DOCTOR_INVALID_{}",
            uuid::Uuid::new_v4().simple()
        );
        std::fs::write(
            codex_home.join("auth.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                resolved_reference.as_str(): "resolved-from-auth-json",
                invalid_reference.as_str(): "invalid\r\nbearer",
            }))
            .expect("serialize auth.json"),
        )
        .expect("write auth.json");

        let mut config = HelperConfig::default();
        config.codex.providers.insert(
            "resolved".to_string(),
            ProviderConfig {
                base_url: Some("https://relay.example/v1".to_string()),
                auth: UpstreamAuth {
                    auth_token_env: Some(resolved_reference.clone()),
                    ..UpstreamAuth::default()
                },
                ..ProviderConfig::default()
            },
        );
        config.codex.routing = Some(RouteGraphConfig::ordered_failover(vec![
            "resolved".to_string(),
            "missing".to_string(),
            "anonymous-denied".to_string(),
            "invalid".to_string(),
            "anonymous-allowed".to_string(),
        ]));
        config.codex.providers.insert(
            "missing".to_string(),
            ProviderConfig {
                base_url: Some("https://missing.example/v1".to_string()),
                auth: UpstreamAuth {
                    auth_token_env: Some(missing_reference.clone()),
                    ..UpstreamAuth::default()
                },
                ..ProviderConfig::default()
            },
        );
        config.codex.providers.insert(
            "anonymous-denied".to_string(),
            ProviderConfig {
                base_url: Some("https://anonymous-denied.example/v1".to_string()),
                ..ProviderConfig::default()
            },
        );
        config.codex.providers.insert(
            "invalid".to_string(),
            ProviderConfig {
                base_url: Some("https://invalid.example/v1".to_string()),
                auth: UpstreamAuth {
                    auth_token_env: Some(invalid_reference.clone()),
                    ..UpstreamAuth::default()
                },
                ..ProviderConfig::default()
            },
        );
        config.codex.providers.insert(
            "anonymous-allowed".to_string(),
            ProviderConfig {
                base_url: Some("https://anonymous-allowed.example/v1".to_string()),
                auth: UpstreamAuth {
                    allow_anonymous: Some(true),
                    ..UpstreamAuth::default()
                },
                ..ProviderConfig::default()
            },
        );

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
            crate::config::save_helper_config(&config)
                .await
                .expect("write canonical config");
            run_doctor(DoctorLang::En, CredentialSourceCapabilities::server()).await
        });

        let missing_checks = report
            .checks
            .iter()
            .filter(|check| check.id == "proxy_config.auth.missing_reference")
            .collect::<Vec<_>>();
        assert_eq!(missing_checks.len(), 1);
        assert!(
            missing_checks[0]
                .message
                .contains("provider 'missing.default'")
        );
        assert!(missing_checks[0].message.contains(&missing_reference));
        assert!(!missing_checks[0].message.contains(&resolved_reference));

        let invalid_checks = report
            .checks
            .iter()
            .filter(|check| check.id == "proxy_config.auth.invalid_value")
            .collect::<Vec<_>>();
        assert_eq!(invalid_checks.len(), 1);
        assert!(
            invalid_checks[0]
                .message
                .contains("provider 'invalid.default'")
        );
        assert!(invalid_checks[0].message.contains(&invalid_reference));

        let anonymous_checks = report
            .checks
            .iter()
            .filter(|check| check.id == "proxy_config.auth.anonymous_not_allowed")
            .collect::<Vec<_>>();
        assert_eq!(anonymous_checks.len(), 1);
        assert!(anonymous_checks[0].message.contains("anonymous-denied"));
        assert!(!anonymous_checks[0].message.contains("anonymous-allowed"));
    }

    #[test]
    fn doctor_reports_unsupported_native_reference_in_server_context() {
        let _lock = env_lock();
        let home = std::env::temp_dir().join(format!(
            "codex-helper-doctor-native-server-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = home.join(".codex-helper");
        let codex_home = home.join(".codex");
        std::fs::create_dir_all(&helper_home).expect("create helper home");
        std::fs::create_dir_all(&codex_home).expect("create Codex home");

        let reference = format!("relay.doctor.{}", uuid::Uuid::new_v4().simple());
        let mut config = HelperConfig::default();
        config.codex.providers.insert(
            "native".to_string(),
            ProviderConfig {
                base_url: Some("https://native.example/v1".to_string()),
                auth: UpstreamAuth {
                    auth_token_ref: Some(CredentialRef::Native {
                        name: reference.clone(),
                    }),
                    ..UpstreamAuth::default()
                },
                ..ProviderConfig::default()
            },
        );
        config.codex.routing = Some(RouteGraphConfig::ordered_failover(vec![
            "native".to_string(),
        ]));

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
            crate::config::save_helper_config(&config)
                .await
                .expect("write canonical config");
            run_doctor(DoctorLang::En, CredentialSourceCapabilities::server()).await
        });

        let unsupported = report
            .checks
            .iter()
            .filter(|check| check.id == "proxy_config.auth.unsupported_reference")
            .collect::<Vec<_>>();
        assert_eq!(unsupported.len(), 1);
        assert!(unsupported[0].message.contains("provider 'native.default'"));
        assert!(unsupported[0].message.contains(&reference));
        assert!(!unsupported[0].message.contains("native-secret"));
    }

    #[test]
    fn doctor_reads_native_credential_while_daemon_writer_is_active() {
        let _lock = env_lock();
        let home = std::env::temp_dir().join(format!(
            "codex-helper-doctor-native-reader-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = home.join(".codex-helper");
        let codex_home = home.join(".codex");
        std::fs::create_dir_all(&helper_home).expect("create helper home");
        std::fs::create_dir_all(&codex_home).expect("create Codex home");

        let reference = format!("relay.doctor.{}", uuid::Uuid::new_v4().simple());
        let mut config = HelperConfig::default();
        config.codex.providers.insert(
            "native".to_string(),
            ProviderConfig {
                base_url: Some("https://native.example/v1".to_string()),
                auth: UpstreamAuth {
                    auth_token_ref: Some(CredentialRef::Native {
                        name: reference.clone(),
                    }),
                    ..UpstreamAuth::default()
                },
                ..ProviderConfig::default()
            },
        );
        config.codex.routing = Some(RouteGraphConfig::ordered_failover(vec![
            "native".to_string(),
        ]));

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
        let (capabilities, control) = CredentialSourceCapabilities::test_native(
            SecretValue::new(b"native-secret".to_vec()).expect("valid native secret"),
        );
        let report = runtime.block_on(async {
            crate::config::save_helper_config(&config)
                .await
                .expect("write canonical config");
            let _daemon_store = RuntimeStore::open_in_home(&helper_home)
                .expect("open daemon-owned runtime store writer");
            run_doctor(DoctorLang::En, capabilities).await
        });

        assert_eq!(control.read_count(), 1);
        assert!(report.checks.iter().all(|check| {
            check.id != "proxy_config.auth.readiness"
                && check.id != "proxy_config.auth.unsupported_reference"
                && check.id != "proxy_config.auth.missing_reference"
        }));
        assert!(
            report
                .checks
                .iter()
                .all(|check| !check.message.contains("native-secret"))
        );
    }
}

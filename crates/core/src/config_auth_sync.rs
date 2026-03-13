use super::*;

pub(crate) fn read_file_if_exists(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let s = stdfs::read_to_string(path).with_context(|| format!("failed to read {:?}", path))?;
    Ok(Some(s))
}

/// Try to infer a unique API key from ~/.codex/auth.json when the provider
/// does not declare an explicit `env_key`.
///
/// This mirrors the common Codex CLI layout where `auth.json` contains a
/// single `*_API_KEY` field (e.g. `OPENAI_API_KEY`) plus metadata fields
/// like `tokens` / `last_refresh`. We only consider string values whose
/// key ends with `_API_KEY`, and only succeed when there is exactly one
/// such candidate; otherwise we return None and let the caller error out.
pub(crate) fn infer_env_key_from_auth_json(
    auth_json: &Option<JsonValue>,
) -> Option<(String, String)> {
    let json = auth_json.as_ref()?;
    let obj = json.as_object()?;

    let mut candidates: Vec<(String, String)> = obj
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k, s)))
        .filter(|(k, v)| k.ends_with("_API_KEY") && !v.trim().is_empty())
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    if candidates.len() == 1 {
        candidates.pop()
    } else {
        None
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct SyncCodexAuthFromCodexOptions {
    /// Add missing providers found in ~/.codex/config.toml into ~/.codex-helper/config.
    pub add_missing: bool,
    /// Also set codex-helper active station to match Codex CLI's current model_provider.
    pub set_active: bool,
    /// Override existing inline secrets and non-codex-source upstreams (use with care).
    pub force: bool,
}

#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct SyncCodexAuthFromCodexReport {
    pub updated: usize,
    pub added: usize,
    pub active_set: bool,
    pub warnings: Vec<String>,
}

/// Sync Codex auth env vars from ~/.codex/config.toml + auth.json without changing routing config.
///
/// Default behavior:
/// - Only updates upstreams that are strongly associated with a Codex CLI provider:
///   - config key equals provider_id; or
///   - upstream.tags.provider_id equals provider_id.
/// - Does NOT change `active` / `enabled` / `level` unless `options.set_active = true`.
/// - Does NOT write secrets to disk; only syncs env var names (e.g. `OPENAI_API_KEY`).
#[allow(dead_code)]
pub fn sync_codex_auth_from_codex_cli(
    cfg: &mut ProxyConfig,
    options: SyncCodexAuthFromCodexOptions,
) -> Result<SyncCodexAuthFromCodexReport> {
    fn is_non_empty(s: &Option<String>) -> bool {
        s.as_deref().is_some_and(|v| !v.trim().is_empty())
    }

    let backup_path = codex_backup_config_path();
    let cfg_path = codex_config_path();
    let cfg_text_opt = if let Some(text) = read_file_if_exists(&backup_path)?
        && !is_codex_absent_backup_sentinel(&text)
    {
        Some(text)
    } else {
        read_file_if_exists(&cfg_path)?
    };
    let cfg_text = match cfg_text_opt {
        Some(s) if !s.trim().is_empty() => s,
        _ => anyhow::bail!("未找到 ~/.codex/config.toml 或文件为空，无法同步 Codex 账号信息"),
    };

    let value: TomlValue = cfg_text.parse()?;
    let table = value
        .as_table()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Codex config root must be table"))?;

    let current_provider_id = table
        .get("model_provider")
        .and_then(|v| v.as_str())
        .unwrap_or("openai")
        .to_string();

    let providers_table = table
        .get("model_providers")
        .and_then(|v| v.as_table())
        .cloned()
        .unwrap_or_default();

    let auth_json_path = codex_auth_path();
    let auth_json: Option<JsonValue> = match read_file_if_exists(&auth_json_path)? {
        Some(s) if !s.trim().is_empty() => serde_json::from_str(&s).ok(),
        _ => None,
    };
    let inferred_env_key = infer_env_key_from_auth_json(&auth_json).map(|(k, _)| k);

    // Avoid syncing from a self-forwarding Codex config unless we have a valid backup.
    if current_provider_id == "codex_proxy" && !backup_path.exists() {
        let provider_table = providers_table.get(&current_provider_id);
        let is_local_helper = provider_table
            .and_then(|t| t.get("base_url"))
            .and_then(|v| v.as_str())
            .map(|u| u.contains("127.0.0.1") || u.contains("localhost"))
            .unwrap_or(false);
        if is_local_helper {
            anyhow::bail!(
                "检测到 ~/.codex/config.toml 的当前 model_provider 指向本地代理 codex-helper，且未找到备份配置；\
无法安全同步账号信息。请先恢复 ~/.codex/config.toml 后重试。"
            );
        }
    }

    #[derive(Debug, Clone)]
    struct ProviderSpec {
        provider_id: String,
        requires_openai_auth: bool,
        base_url: Option<String>,
        env_key: Option<String>,
        alias: Option<String>,
    }

    let mut providers = Vec::new();
    for (provider_id, provider_val) in providers_table.iter() {
        let Some(provider_table) = provider_val.as_table() else {
            continue;
        };

        let requires_openai_auth = provider_table
            .get("requires_openai_auth")
            .and_then(|v| v.as_bool())
            .unwrap_or(provider_id == "openai");

        let base_url = provider_table
            .get("base_url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                if provider_id == "openai" {
                    Some("https://api.openai.com/v1".to_string())
                } else {
                    None
                }
            });

        // Skip local codex-helper proxy entry to avoid accidental loops.
        if provider_id == "codex_proxy"
            && base_url
                .as_deref()
                .is_some_and(|u| u.contains("127.0.0.1") || u.contains("localhost"))
        {
            continue;
        }

        let env_key = provider_table
            .get("env_key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| inferred_env_key.clone());

        let alias = provider_table
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
            .filter(|s| s != provider_id);

        providers.push(ProviderSpec {
            provider_id: provider_id.to_string(),
            requires_openai_auth,
            base_url,
            env_key,
            alias,
        });
    }

    let mut report = SyncCodexAuthFromCodexReport::default();

    for pvd in providers.iter() {
        let pid = pvd.provider_id.as_str();

        // Target configs:
        // 1) config key equals provider_id; 2) any upstream tagged with provider_id.
        let mut target_cfg_keys = Vec::new();
        if cfg.codex.contains_station(pid) {
            target_cfg_keys.push(pid.to_string());
        }

        for (cfg_key, svc) in cfg.codex.stations() {
            if svc
                .upstreams
                .iter()
                .any(|u| u.tags.get("provider_id").map(|s| s.as_str()) == Some(pid))
                && !target_cfg_keys.iter().any(|k| k == cfg_key)
            {
                target_cfg_keys.push(cfg_key.clone());
            }
        }

        if target_cfg_keys.is_empty() {
            if options.add_missing {
                let Some(base_url) = pvd.base_url.as_deref().filter(|s| !s.trim().is_empty())
                else {
                    report.warnings.push(format!(
                        "skip add provider '{pid}': base_url is missing in ~/.codex/config.toml"
                    ));
                    continue;
                };

                let mut tags = HashMap::new();
                tags.insert("source".into(), "codex-config".into());
                tags.insert("provider_id".into(), pid.to_string());
                tags.insert(
                    "requires_openai_auth".into(),
                    pvd.requires_openai_auth.to_string(),
                );

                let mut upstream = UpstreamConfig {
                    base_url: base_url.to_string(),
                    auth: UpstreamAuth::default(),
                    tags,
                    supported_models: HashMap::new(),
                    model_mapping: HashMap::new(),
                };
                if !pvd.requires_openai_auth {
                    if let Some(env_key) = pvd.env_key.as_deref().filter(|s| !s.trim().is_empty()) {
                        upstream.auth.auth_token_env = Some(env_key.to_string());
                    } else {
                        report.warnings.push(format!(
                            "added provider '{pid}' but auth env_key is missing (no env_key and auth.json can't infer a unique *_API_KEY)"
                        ));
                    }
                }

                let service = ServiceConfig {
                    name: pid.to_string(),
                    alias: pvd.alias.clone(),
                    enabled: true,
                    level: 1,
                    upstreams: vec![upstream],
                };

                cfg.codex.stations_mut().insert(pid.to_string(), service);
                report.added += 1;
            }
            continue;
        }

        // No secrets needed for providers that rely on the client Authorization.
        if pvd.requires_openai_auth {
            continue;
        }

        let Some(desired_env) = pvd.env_key.as_deref().filter(|s| !s.trim().is_empty()) else {
            report.warnings.push(format!(
                "skip provider '{pid}': env_key is missing and auth.json can't infer a unique *_API_KEY"
            ));
            continue;
        };

        for cfg_key in target_cfg_keys {
            let Some(service) = cfg.codex.station_mut(&cfg_key) else {
                continue;
            };

            let single_upstream = service.upstreams.len() == 1;
            let mut updated_in_this_config = false;
            for upstream in service.upstreams.iter_mut() {
                let tag_pid = upstream.tags.get("provider_id").map(|s| s.as_str());
                let should_touch = if tag_pid == Some(pid) {
                    true
                } else if cfg_key == pid {
                    // Strong signal: config key matches provider id.
                    // Touch upstreams that look like Codex-imported entries or single-upstream configs.
                    let src = upstream.tags.get("source").map(|s| s.as_str());
                    src == Some("codex-config") || single_upstream
                } else {
                    false
                };

                if !should_touch && !options.force {
                    continue;
                }

                if !options.force
                    && (is_non_empty(&upstream.auth.auth_token)
                        || is_non_empty(&upstream.auth.api_key))
                {
                    report.warnings.push(format!(
                        "skip '{cfg_key}': upstream has inline secret; use --force to override"
                    ));
                    continue;
                }

                if upstream.auth.auth_token_env.as_deref() != Some(desired_env) {
                    upstream.auth.auth_token_env = Some(desired_env.to_string());
                    if options.force {
                        upstream.auth.auth_token = None;
                        upstream.auth.api_key = None;
                    }
                    report.updated += 1;
                    updated_in_this_config = true;
                }
            }

            if !updated_in_this_config && cfg_key == pid {
                report.warnings.push(format!(
                    "no upstream updated for provider '{pid}' in config '{cfg_key}' (no matching upstream tags)"
                ));
            }
        }
    }

    if options.set_active
        && current_provider_id != "codex_proxy"
        && cfg.codex.contains_station(&current_provider_id)
        && cfg.codex.active.as_deref() != Some(current_provider_id.as_str())
    {
        cfg.codex.active = Some(current_provider_id);
        report.active_set = true;
    }

    Ok(report)
}

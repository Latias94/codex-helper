use super::*;

pub(crate) fn bootstrap_from_codex(cfg: &mut ProxyConfig) -> Result<()> {
    if !cfg.codex.configs.is_empty() {
        return Ok(());
    }

    // 优先从备份配置中推导原始上游，避免在 ~/.codex/config.toml 已被 codex-helper
    // 写成本地 provider（codex_proxy）时出现“自我转发”。
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
        _ => {
            anyhow::bail!("未找到 ~/.codex/config.toml 或文件为空，无法自动推导 Codex 上游");
        }
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

    // 如当前 provider 看起来是本地 codex-helper 代理且没有备份（或备份无效），
    // 则无法安全推导原始上游，直接报错，避免将代理指向自身。
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
无法自动推导原始 Codex 上游。请先恢复 ~/.codex/config.toml 后重试，或在 ~/.codex-helper/config.json 中手动添加 codex 上游配置。"
            );
        }
    }

    let mut imported_any = false;
    let mut imported_active = false;

    // Import all providers from [model_providers.*] as switchable configs.
    for (provider_id, provider_val) in providers_table.iter() {
        let Some(provider_table) = provider_val.as_table() else {
            continue;
        };

        let requires_openai_auth = provider_table
            .get("requires_openai_auth")
            .and_then(|v| v.as_bool())
            .unwrap_or(provider_id == "openai");

        let base_url_opt = provider_table
            .get("base_url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let base_url = match base_url_opt {
            Some(u) if !u.trim().is_empty() => u,
            _ => {
                if provider_id == &current_provider_id {
                    anyhow::bail!(
                        "当前 model_provider '{}' 缺少 base_url，无法自动推导 Codex 上游",
                        provider_id
                    );
                }
                warn!(
                    "skip model_provider '{}' because base_url is missing",
                    provider_id
                );
                continue;
            }
        };

        if provider_id == "codex_proxy"
            && (base_url.contains("127.0.0.1") || base_url.contains("localhost"))
        {
            if provider_id == &current_provider_id && !backup_path.exists() {
                anyhow::bail!(
                    "检测到 ~/.codex/config.toml 的当前 model_provider 指向本地代理 codex-helper，且未找到备份配置；\
无法自动推导原始 Codex 上游。请先恢复 ~/.codex/config.toml 后重试，或在 ~/.codex-helper/config.json 中手动添加 codex 上游配置。"
                );
            }
            warn!("skip model_provider 'codex_proxy' to avoid self-forwarding loop");
            continue;
        }

        let env_key = provider_table
            .get("env_key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty());

        let (auth_token, auth_token_env) = if requires_openai_auth {
            (None, None)
        } else {
            let effective_env_key = env_key.clone().or_else(|| inferred_env_key.clone());
            if effective_env_key.is_none() {
                if provider_id == &current_provider_id {
                    anyhow::bail!(
                        "当前 model_provider 未声明 env_key，且无法从 ~/.codex/auth.json 推断唯一的 `*_API_KEY` 字段；请为该 provider 配置 env_key"
                    );
                }
                warn!(
                    "skip model_provider '{}' because env_key is missing and auth.json can't infer a unique *_API_KEY",
                    provider_id
                );
                continue;
            }
            (None, effective_env_key)
        };

        let alias = provider_table
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
            .filter(|s| s != provider_id);

        let mut tags = HashMap::new();
        tags.insert("source".into(), "codex-config".into());
        tags.insert("provider_id".into(), provider_id.to_string());
        tags.insert(
            "requires_openai_auth".into(),
            requires_openai_auth.to_string(),
        );

        let upstream = UpstreamConfig {
            base_url: base_url.clone(),
            auth: UpstreamAuth {
                auth_token,
                auth_token_env,
                api_key: None,
                api_key_env: None,
            },
            tags,
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        };

        let service = ServiceConfig {
            name: provider_id.to_string(),
            alias,
            enabled: true,
            level: 1,
            upstreams: vec![upstream],
        };

        cfg.codex.configs.insert(provider_id.to_string(), service);
        imported_any = true;
        if provider_id == &current_provider_id {
            imported_active = true;
        }
    }

    if !imported_any {
        anyhow::bail!("未能从 ~/.codex/config.toml 推导出任何可用的 Codex 上游配置");
    }

    // Prefer the Codex CLI current provider as active.
    if imported_active && cfg.codex.configs.contains_key(&current_provider_id) {
        cfg.codex.active = Some(current_provider_id);
    } else {
        cfg.codex.active = cfg.codex.configs.keys().min().cloned();
    }

    Ok(())
}

fn bootstrap_from_claude(cfg: &mut ProxyConfig) -> Result<()> {
    if !cfg.claude.configs.is_empty() {
        return Ok(());
    }

    let settings_path = claude_settings_path();
    let backup_path = claude_settings_backup_path();
    // Claude 配置同样优先从备份读取，避免将代理指向自身（本地 codex-helper）。
    let settings_text_opt = if let Some(text) = read_file_if_exists(&backup_path)?
        && !is_claude_absent_backup_sentinel(&text)
    {
        Some(text)
    } else {
        read_file_if_exists(&settings_path)?
    };
    let settings_text = match settings_text_opt {
        Some(s) if !s.trim().is_empty() => s,
        _ => {
            anyhow::bail!(
                "未找到 Claude Code 配置文件 {:?}（或文件为空），无法自动推导 Claude 上游；请先在 Claude Code 中完成配置，或手动在 ~/.codex-helper/config.json 中添加 claude 配置",
                settings_path
            );
        }
    };

    let value: JsonValue = serde_json::from_str(&settings_text)
        .with_context(|| format!("解析 {:?} 失败，需为有效的 JSON", settings_path))?;
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Claude settings 根节点必须是 JSON object"))?;

    let env_obj = obj
        .get("env")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow::anyhow!("Claude settings 中缺少 env 对象"))?;

    let api_key_env = if env_obj
        .get("ANTHROPIC_AUTH_TOKEN")
        .and_then(|v| v.as_str())
        .is_some()
    {
        Some("ANTHROPIC_AUTH_TOKEN".to_string())
    } else if env_obj
        .get("ANTHROPIC_API_KEY")
        .and_then(|v| v.as_str())
        .is_some()
    {
        Some("ANTHROPIC_API_KEY".to_string())
    } else {
        None
    }
    .ok_or_else(|| {
            anyhow::anyhow!(
                "Claude settings 中缺少 ANTHROPIC_AUTH_TOKEN / ANTHROPIC_API_KEY；请先在 Claude Code 中完成登录或配置 API Key"
            )
        })?;

    let base_url = env_obj
        .get("ANTHROPIC_BASE_URL")
        .and_then(|v| v.as_str())
        .unwrap_or("https://api.anthropic.com/v1")
        .to_string();

    // 如当前 base_url 看起来是本地地址且没有备份，则无法安全推导真实上游，
    // 直接报错，避免将 Claude 代理指向自身。
    if !backup_path.exists() && (base_url.contains("127.0.0.1") || base_url.contains("localhost")) {
        anyhow::bail!(
            "检测到 Claude settings {:?} 的 ANTHROPIC_BASE_URL 指向本地地址 ({base_url})，且未找到备份配置；\
无法自动推导原始 Claude 上游。请先恢复 Claude 配置后重试，或在 ~/.codex-helper/config.json 中手动添加 claude 上游配置。",
            settings_path
        );
    }

    let mut tags = HashMap::new();
    tags.insert("source".into(), "claude-settings".into());
    tags.insert("provider_id".into(), "anthropic".into());

    let upstream = UpstreamConfig {
        base_url,
        auth: UpstreamAuth {
            auth_token: None,
            auth_token_env: None,
            api_key: None,
            api_key_env: Some(api_key_env),
        },
        tags,
        supported_models: HashMap::new(),
        model_mapping: HashMap::new(),
    };

    let service = ServiceConfig {
        name: "default".to_string(),
        alias: Some("Claude default".to_string()),
        enabled: true,
        level: 1,
        upstreams: vec![upstream],
    };

    cfg.claude.configs.insert("default".to_string(), service);
    cfg.claude.active = Some("default".to_string());

    Ok(())
}

/// 加载代理配置，如有必要从 ~/.codex 自动初始化 codex 配置。
pub async fn load_or_bootstrap_from_codex() -> Result<ProxyConfig> {
    let mut cfg = load_config().await?;
    if cfg.codex.configs.is_empty() {
        match bootstrap_from_codex(&mut cfg) {
            Ok(()) => {
                let _ = save_config(&cfg).await;
                info!(
                    "已根据 ~/.codex/config.toml 与 ~/.codex/auth.json 自动创建默认 Codex 上游配置"
                );
            }
            Err(err) => {
                warn!(
                    "无法从 ~/.codex 引导 Codex 配置: {err}; \
                     如果尚未安装或配置 Codex CLI 可以忽略，否则请检查 ~/.codex/config.toml 和 ~/.codex/auth.json，或使用 `codex-helper config add` 手动添加上游"
                );
            }
        }
    } else {
        // 已存在配置但没有 active，提示用户检查
        if cfg.codex.active.is_none() && !cfg.codex.configs.is_empty() {
            warn!(
                "检测到 Codex 配置但没有激活项，将使用任意一条配置作为默认；如需指定，请使用 `codex-helper config set-active <name>`"
            );
        }
    }
    Ok(cfg)
}

/// 显式从 Codex CLI 的配置文件（~/.codex/config.toml + auth.json）导入/刷新 codex 段配置。
/// - 当 force = false 且当前已存在 codex 配置时，将返回错误，避免意外覆盖；
/// - 当 force = true 时，将清空现有 codex 段后重新基于 Codex 配置推导。
pub async fn import_codex_config_from_codex_cli(force: bool) -> Result<ProxyConfig> {
    let mut cfg = load_config().await?;
    if !cfg.codex.configs.is_empty() && !force {
        anyhow::bail!(
            "检测到 ~/.codex-helper/config.json 中已存在 Codex 配置；如需根据 ~/.codex/config.toml 重新导入，请使用 --force 覆盖"
        );
    }

    cfg.codex = ServiceConfigManager::default();
    bootstrap_from_codex(&mut cfg)?;
    save_config(&cfg).await?;
    info!(
        "已根据 ~/.codex/config.toml 与 ~/.codex/auth.json 重新导入 Codex 上游配置（force = {}）",
        force
    );
    Ok(cfg)
}

/// Overwrite Codex configs from ~/.codex/config.toml + auth.json (in-place).
///
/// This resets the codex-helper Codex section back to Codex CLI defaults:
/// it clears existing configs (including grouping/level/enabled) and re-imports providers.
pub fn overwrite_codex_config_from_codex_cli_in_place(cfg: &mut ProxyConfig) -> Result<()> {
    cfg.codex = ServiceConfigManager::default();
    bootstrap_from_codex(cfg)
}
pub async fn load_or_bootstrap_from_claude() -> Result<ProxyConfig> {
    let mut cfg = load_config().await?;
    if cfg.claude.configs.is_empty() {
        match bootstrap_from_claude(&mut cfg) {
            Ok(()) => {
                let _ = save_config(&cfg).await;
                info!("已根据 ~/.claude/settings.json 自动创建默认 Claude 上游配置");
            }
            Err(err) => {
                warn!(
                    "无法从 ~/.claude 引导 Claude 配置: {err}; \
                     如果尚未安装或配置 Claude Code 可以忽略，否则请检查 ~/.claude/settings.json，或在 ~/.codex-helper/config.json 中手动添加 claude 配置"
                );
            }
        }
    } else if cfg.claude.active.is_none() && !cfg.claude.configs.is_empty() {
        warn!(
            "检测到 Claude 配置但没有激活项，将使用任意一条配置作为默认；如需指定，请使用 `codex-helper config set-active <name>`（后续将扩展对 Claude 的专用子命令）"
        );
    }
    Ok(cfg)
}

/// Unified entry to load proxy config and, if necessary, bootstrap upstreams
/// from the official Codex / Claude configuration files.
pub async fn load_or_bootstrap_for_service(kind: ServiceKind) -> Result<ProxyConfig> {
    match kind {
        ServiceKind::Codex => load_or_bootstrap_from_codex().await,
        ServiceKind::Claude => load_or_bootstrap_from_claude().await,
    }
}

/// Probe whether we can successfully bootstrap Codex upstreams from
/// ~/.codex/config.toml and ~/.codex/auth.json without mutating any
/// codex-helper configs. Intended for diagnostics (`codex-helper doctor`).
pub async fn probe_codex_bootstrap_from_cli() -> Result<()> {
    let mut cfg = ProxyConfig::default();
    bootstrap_from_codex(&mut cfg)
}

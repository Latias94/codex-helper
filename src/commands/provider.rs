use super::config_doc::{
    ensure_routing, ensure_routing_order_contains, load_helper_config, ordered_provider_names,
    parse_cli_string_map, parse_cli_tags, print_provider_list, select_service_route_config,
    select_service_route_config_mut,
};
use crate::cli_types::{ProviderAuthKind, ProviderCommand};
use crate::config::{
    CURRENT_CONFIG_VERSION, CredentialRef, ProviderConfig, ProviderEndpointConfig, ServiceKind,
    ServiceRouteConfig, UpstreamAuth,
    storage::{load_config, mutate_helper_config, save_helper_config},
};
use crate::{CliError, CliResult};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
struct ProviderCatalogPayload {
    schema_version: u32,
    service: String,
    providers: Vec<ProviderView>,
}

#[derive(Debug, Serialize)]
struct ProviderShowPayload {
    schema_version: u32,
    service: String,
    provider: ProviderView,
}

#[derive(Debug, Serialize, Clone)]
struct ProviderEndpointView {
    name: String,
    base_url: String,
    enabled: bool,
    priority: u32,
    tags: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Clone)]
struct ProviderView {
    name: String,
    alias: Option<String>,
    enabled: bool,
    routing_index: Option<usize>,
    routing_target: bool,
    auth_token_env: Option<String>,
    auth_token_ref: Option<CredentialRef>,
    api_key_env: Option<String>,
    api_key_ref: Option<CredentialRef>,
    has_inline_auth_token: bool,
    has_inline_api_key: bool,
    allow_anonymous: bool,
    tags: BTreeMap<String, String>,
    supported_models: Vec<String>,
    model_mapping: BTreeMap<String, String>,
    endpoints: Vec<ProviderEndpointView>,
}

pub async fn handle_provider_cmd(cmd: ProviderCommand) -> CliResult<()> {
    match cmd {
        ProviderCommand::List {
            codex,
            claude,
            json,
        } => {
            let (cfg, service, label) = load_helper_config(codex, claude, "provider")
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            let (view, _) = select_service_route_config(&cfg, service);

            if json {
                let payload = ProviderCatalogPayload {
                    schema_version: CURRENT_CONFIG_VERSION,
                    service: service.to_string(),
                    providers: build_provider_views(view),
                };
                let text = serde_json::to_string_pretty(&payload)
                    .map_err(|e| CliError::Configuration(e.to_string()))?;
                println!("{text}");
            } else {
                print_provider_list(label, view);
            }
        }
        ProviderCommand::Show {
            name,
            codex,
            claude,
            json,
        } => {
            let (cfg, service, label) = load_helper_config(codex, claude, "provider")
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            let (view, _) = select_service_route_config(&cfg, service);
            let provider = build_provider_view(view, name.as_str()).ok_or_else(|| {
                CliError::Configuration(format!("provider '{}' not found in source config", name))
            })?;

            if json {
                let payload = ProviderShowPayload {
                    schema_version: CURRENT_CONFIG_VERSION,
                    service: service.to_string(),
                    provider,
                };
                let text = serde_json::to_string_pretty(&payload)
                    .map_err(|e| CliError::Configuration(e.to_string()))?;
                println!("{text}");
            } else {
                print_provider_detail(label, &provider);
            }
        }
        ProviderCommand::Add {
            name,
            base_url,
            auth_token,
            auth_token_env,
            api_key,
            api_key_env,
            allow_anonymous,
            alias,
            tags,
            supported_models,
            model_mapping,
            disabled,
            replace,
            codex,
            claude,
        } => {
            let parsed_tags =
                parse_cli_tags(&tags).map_err(|e| CliError::Configuration(e.to_string()))?;
            let parsed_supported_models = parse_cli_supported_models(&supported_models)
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            let parsed_model_mapping = parse_cli_string_map(&model_mapping, "model-map")
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            let (mut cfg, service, label) = load_helper_config(codex, claude, "provider")
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            {
                let (view, _) = select_service_route_config_mut(&mut cfg, service);
                if view.providers.contains_key(name.as_str()) && !replace {
                    return Err(CliError::Configuration(format!(
                        "provider '{}' already exists; pass --replace to overwrite it",
                        name
                    )));
                }
                view.providers.insert(
                    name.clone(),
                    ProviderConfig {
                        alias,
                        enabled: !disabled,
                        base_url: Some(base_url),
                        inline_auth: UpstreamAuth {
                            auth_token: auth_token.map(Into::into),
                            auth_token_env,
                            auth_token_ref: None,
                            api_key: api_key.map(Into::into),
                            api_key_env,
                            api_key_ref: None,
                            allow_anonymous: allow_anonymous.then_some(true),
                        },
                        tags: parsed_tags,
                        supported_models: parsed_supported_models,
                        model_mapping: parsed_model_mapping,
                        ..ProviderConfig::default()
                    },
                );
                ensure_routing_order_contains(view, name.as_str());
                if disabled {
                    clear_manual_target_for_provider(view, name.as_str());
                }
            }

            save_helper_config(&cfg)
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            println!("Added {label} provider '{}'", name);
        }
        ProviderCommand::Enable {
            name,
            codex,
            claude,
        } => {
            let (mut cfg, service, label) = load_helper_config(codex, claude, "provider")
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            {
                let (view, _) = select_service_route_config_mut(&mut cfg, service);
                let Some(provider) = view.providers.get_mut(name.as_str()) else {
                    return Err(CliError::Configuration(format!(
                        "provider '{}' not found in source config",
                        name
                    )));
                };
                provider.enabled = true;
                ensure_routing_order_contains(view, name.as_str());
            }

            save_helper_config(&cfg)
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            println!("Enabled {label} provider '{}'", name);
        }
        ProviderCommand::Disable {
            name,
            codex,
            claude,
        } => {
            let (mut cfg, service, label) = load_helper_config(codex, claude, "provider")
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            let mut cleared_target = false;
            {
                let (view, _) = select_service_route_config_mut(&mut cfg, service);
                let Some(provider) = view.providers.get_mut(name.as_str()) else {
                    return Err(CliError::Configuration(format!(
                        "provider '{}' not found in source config",
                        name
                    )));
                };
                provider.enabled = false;

                if clear_manual_target_for_provider(view, name.as_str()) {
                    cleared_target = true;
                }
            }

            save_helper_config(&cfg)
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            if cleared_target {
                println!(
                    "Disabled {label} provider '{}' and cleared manual routing target",
                    name
                );
            } else {
                println!("Disabled {label} provider '{}'", name);
            }
        }
        ProviderCommand::SetAuth {
            name,
            kind,
            native,
            secret_file,
            environment,
            codex,
            claude,
        } => {
            let source = ProviderAuthSource::from_args(native, secret_file, environment)?;
            let requested_service = requested_service(codex, claude)?;
            load_config()
                .await
                .map_err(|error| CliError::Configuration(error.to_string()))?;
            let source_label = source.summary();
            let provider_name = name.clone();
            let (_, service) = mutate_helper_config(move |config| {
                let service = select_requested_service(config, requested_service);
                let (view, _) = select_service_route_config_mut(config, service);
                let provider = view
                    .providers
                    .get_mut(provider_name.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("provider '{}' not found in source config", provider_name)
                    })?;
                set_provider_auth(provider, kind, source);
                Ok(service)
            })
            .await
            .map_err(|error| CliError::Configuration(error.to_string()))?;
            let label = service_label(service);
            println!(
                "Set {label} provider '{}' {} auth to {source_label}",
                name,
                provider_auth_kind_label(kind)
            );
        }
        ProviderCommand::ClearAuth {
            name,
            kind,
            codex,
            claude,
        } => {
            let requested_service = requested_service(codex, claude)?;
            load_config()
                .await
                .map_err(|error| CliError::Configuration(error.to_string()))?;
            let provider_name = name.clone();
            let (_, service) = mutate_helper_config(move |config| {
                let service = select_requested_service(config, requested_service);
                let (view, _) = select_service_route_config_mut(config, service);
                let provider = view
                    .providers
                    .get_mut(provider_name.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("provider '{}' not found in source config", provider_name)
                    })?;
                clear_provider_auth(provider, kind);
                Ok(service)
            })
            .await
            .map_err(|error| CliError::Configuration(error.to_string()))?;
            let label = service_label(service);
            println!(
                "Cleared {label} provider '{}' {} auth",
                name,
                provider_auth_kind_label(kind)
            );
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
enum ProviderAuthSource {
    Native(String),
    SecretFile(String),
    Environment(String),
}

impl ProviderAuthSource {
    fn from_args(
        native: Option<String>,
        secret_file: Option<std::path::PathBuf>,
        environment: Option<String>,
    ) -> CliResult<Self> {
        match (native, secret_file, environment) {
            (Some(name), None, None) => {
                let name = codex_helper_core::credentials::CredentialName::parse(name)
                    .map_err(|error| CliError::Configuration(error.to_string()))?;
                Ok(Self::Native(name.to_string()))
            }
            (None, Some(path), None) => {
                if !path.is_absolute() {
                    return Err(CliError::Configuration(
                        "secret-file credential path must be absolute".to_string(),
                    ));
                }
                let path = path.into_os_string().into_string().map_err(|_| {
                    CliError::Configuration(
                        "secret-file credential path must be valid Unicode".to_string(),
                    )
                })?;
                if path.chars().any(char::is_control) {
                    return Err(CliError::Configuration(
                        "secret-file credential path contains control characters".to_string(),
                    ));
                }
                Ok(Self::SecretFile(path))
            }
            (None, None, Some(name)) => {
                validate_environment_reference(&name)?;
                Ok(Self::Environment(name))
            }
            _ => Err(CliError::Configuration(
                "provider auth requires exactly one of --native, --secret-file, or --environment"
                    .to_string(),
            )),
        }
    }

    fn summary(&self) -> String {
        match self {
            Self::Native(name) => format!("native:{name}"),
            Self::SecretFile(path) => format!("secret_file:{path}"),
            Self::Environment(name) => format!("environment:{name}"),
        }
    }
}

fn validate_environment_reference(name: &str) -> CliResult<()> {
    if name.is_empty() || name.contains('=') || name.chars().any(char::is_control) {
        return Err(CliError::Configuration(
            "environment variable name is invalid".to_string(),
        ));
    }
    Ok(())
}

fn set_provider_auth(
    provider: &mut ProviderConfig,
    kind: ProviderAuthKind,
    source: ProviderAuthSource,
) {
    clear_provider_auth(provider, kind);
    match (kind, source) {
        (ProviderAuthKind::Bearer, ProviderAuthSource::Native(name)) => {
            provider.auth.auth_token_ref = Some(CredentialRef::Native { name });
        }
        (ProviderAuthKind::Bearer, ProviderAuthSource::SecretFile(path)) => {
            provider.auth.auth_token_ref = Some(CredentialRef::SecretFile { path });
        }
        (ProviderAuthKind::Bearer, ProviderAuthSource::Environment(name)) => {
            provider.auth.auth_token_env = Some(name);
        }
        (ProviderAuthKind::ApiKey, ProviderAuthSource::Native(name)) => {
            provider.auth.api_key_ref = Some(CredentialRef::Native { name });
        }
        (ProviderAuthKind::ApiKey, ProviderAuthSource::SecretFile(path)) => {
            provider.auth.api_key_ref = Some(CredentialRef::SecretFile { path });
        }
        (ProviderAuthKind::ApiKey, ProviderAuthSource::Environment(name)) => {
            provider.auth.api_key_env = Some(name);
        }
    }
}

fn clear_provider_auth(provider: &mut ProviderConfig, kind: ProviderAuthKind) {
    clear_auth_kind(&mut provider.auth, kind);
    clear_auth_kind(&mut provider.inline_auth, kind);
}

fn clear_auth_kind(auth: &mut UpstreamAuth, kind: ProviderAuthKind) {
    match kind {
        ProviderAuthKind::Bearer => {
            auth.auth_token = None;
            auth.auth_token_env = None;
            auth.auth_token_ref = None;
        }
        ProviderAuthKind::ApiKey => {
            auth.api_key = None;
            auth.api_key_env = None;
            auth.api_key_ref = None;
        }
    }
}

fn provider_auth_kind_label(kind: ProviderAuthKind) -> &'static str {
    match kind {
        ProviderAuthKind::Bearer => "bearer",
        ProviderAuthKind::ApiKey => "api-key",
    }
}

fn service_label(service: &str) -> &'static str {
    if service == "claude" {
        "Claude"
    } else {
        "Codex"
    }
}

fn requested_service(codex: bool, claude: bool) -> CliResult<Option<&'static str>> {
    match (codex, claude) {
        (true, true) => Err(CliError::Configuration(
            "Please specify at most one of --codex / --claude".to_string(),
        )),
        (true, false) => Ok(Some("codex")),
        (false, true) => Ok(Some("claude")),
        (false, false) => Ok(None),
    }
}

fn select_requested_service(
    config: &crate::config::HelperConfig,
    requested: Option<&'static str>,
) -> &'static str {
    requested.unwrap_or(match config.default_service {
        Some(ServiceKind::Claude) => "claude",
        Some(ServiceKind::Codex) | None => "codex",
    })
}

fn build_provider_views(view: &ServiceRouteConfig) -> Vec<ProviderView> {
    ordered_provider_names(view)
        .into_iter()
        .filter_map(|name| build_provider_view(view, name.as_str()))
        .collect()
}

fn build_provider_view(view: &ServiceRouteConfig, name: &str) -> Option<ProviderView> {
    let provider = view.providers.get(name)?;
    let effective_auth = provider.effective_auth();
    let route_order = crate::config::resolved_provider_order("provider-cli", view)
        .unwrap_or_else(|_| ordered_provider_names(view));
    let routing_index = route_order
        .iter()
        .position(|candidate| candidate == name)
        .map(|idx| idx + 1);
    let routing_target = crate::config::effective_routing(view)
        .entry_node()
        .and_then(|node| {
            matches!(node.strategy, crate::config::RouteStrategy::ManualSticky)
                .then(|| node.target.as_deref())
                .flatten()
        })
        .is_some_and(|target| target == name);

    Some(ProviderView {
        name: name.to_string(),
        alias: provider.alias.clone(),
        enabled: provider.enabled,
        routing_index,
        routing_target,
        auth_token_env: effective_auth.auth_token_env.clone(),
        auth_token_ref: effective_auth.auth_token_ref.clone(),
        api_key_env: effective_auth.api_key_env.clone(),
        api_key_ref: effective_auth.api_key_ref.clone(),
        has_inline_auth_token: effective_auth.auth_token.is_some(),
        has_inline_api_key: effective_auth.api_key.is_some(),
        allow_anonymous: effective_auth.allow_anonymous == Some(true),
        tags: provider.tags.clone(),
        supported_models: provider
            .supported_models
            .iter()
            .filter(|(_model, supported)| **supported)
            .map(|(model, _supported)| model.clone())
            .collect(),
        model_mapping: provider.model_mapping.clone(),
        endpoints: provider_endpoints(provider),
    })
}

fn clear_manual_target_for_provider(view: &mut ServiceRouteConfig, provider_name: &str) -> bool {
    ensure_routing(view).clear_manual_target_for(provider_name)
}

fn provider_endpoints(provider: &ProviderConfig) -> Vec<ProviderEndpointView> {
    let mut endpoints = Vec::new();
    if let Some(base_url) = provider
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        endpoints.push(ProviderEndpointView {
            name: "default".to_string(),
            base_url: base_url.to_string(),
            enabled: provider.enabled,
            priority: 0,
            tags: BTreeMap::new(),
        });
    }
    endpoints.extend(
        provider
            .endpoints
            .iter()
            .map(|(name, endpoint)| endpoint_view_from_config(name.as_str(), endpoint)),
    );
    endpoints
}

fn endpoint_view_from_config(
    name: &str,
    endpoint: &ProviderEndpointConfig,
) -> ProviderEndpointView {
    ProviderEndpointView {
        name: name.to_string(),
        base_url: endpoint.base_url.clone(),
        enabled: endpoint.enabled,
        priority: endpoint.priority,
        tags: endpoint.tags.clone(),
    }
}

fn print_provider_detail(label: &str, provider: &ProviderView) {
    println!("Schema version: v{CURRENT_CONFIG_VERSION}");
    println!("Service: {label}");
    println!("Provider: {}", provider.name);
    if let Some(alias) = provider.alias.as_deref() {
        println!("Alias: {alias}");
    }
    println!("Enabled: {}", provider.enabled);
    println!(
        "Routing: target={} index={}",
        provider.routing_target,
        provider
            .routing_index
            .map(|idx| idx.to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!("Auth: {}", provider_auth_summary(provider));
    println!("Tags: {}", format_tags(&provider.tags));
    println!(
        "Supported models: {}",
        format_models(&provider.supported_models)
    );
    println!(
        "Model mapping: {}",
        format_string_map(&provider.model_mapping)
    );
    println!("Endpoints:");
    if provider.endpoints.is_empty() {
        println!("  <none>");
    } else {
        for endpoint in &provider.endpoints {
            println!(
                "  [{}] {} enabled={} priority={} tags={}",
                endpoint.name,
                endpoint.base_url,
                endpoint.enabled,
                endpoint.priority,
                format_tags(&endpoint.tags)
            );
        }
    }
}

fn parse_cli_supported_models(raw_models: &[String]) -> anyhow::Result<BTreeMap<String, bool>> {
    let mut models = BTreeMap::new();
    for raw in raw_models {
        let model = raw.trim();
        if model.is_empty() {
            anyhow::bail!("supported-model must not be empty");
        }
        if models.insert(model.to_string(), true).is_some() {
            anyhow::bail!("duplicate supported-model '{}'", model);
        }
    }
    Ok(models)
}

fn provider_auth_summary(provider: &ProviderView) -> String {
    let mut parts = Vec::new();
    if let Some(reference) = provider.auth_token_ref.as_ref() {
        parts.push(format!("bearer_ref={}", credential_ref_summary(reference)));
    }
    if let Some(env) = provider.auth_token_env.as_deref() {
        parts.push(format!("bearer_env={env}"));
    }
    if let Some(reference) = provider.api_key_ref.as_ref() {
        parts.push(format!("api_key_ref={}", credential_ref_summary(reference)));
    }
    if let Some(env) = provider.api_key_env.as_deref() {
        parts.push(format!("api_key_env={env}"));
    }
    if provider.has_inline_auth_token {
        parts.push("bearer_inline=<redacted>".to_string());
    }
    if provider.has_inline_api_key {
        parts.push("api_key_inline=<redacted>".to_string());
    }
    if provider.allow_anonymous {
        parts.push("anonymous=explicit".to_string());
    }
    if parts.is_empty() {
        "<none>".to_string()
    } else {
        parts.join(" ")
    }
}

fn credential_ref_summary(reference: &CredentialRef) -> String {
    match reference {
        CredentialRef::Native { name } => format!("native:{name}"),
        CredentialRef::SecretFile { path } => format!("secret_file:{path}"),
    }
}

fn format_models(models: &[String]) -> String {
    if models.is_empty() {
        "-".to_string()
    } else {
        models.join(",")
    }
}

fn format_tags(tags: &BTreeMap<String, String>) -> String {
    if tags.is_empty() {
        return "-".to_string();
    }
    format_string_map(tags)
}

fn format_string_map(map: &BTreeMap<String, String>) -> String {
    if map.is_empty() {
        return "-".to_string();
    }
    map.iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cli_supported_models_rejects_empty_and_duplicate_entries() {
        let models = parse_cli_supported_models(&["gpt-5".to_string(), "gpt-5.5".to_string()])
            .expect("valid supported models");
        assert_eq!(models.get("gpt-5").copied(), Some(true));
        assert_eq!(models.get("gpt-5.5").copied(), Some(true));

        assert!(parse_cli_supported_models(&[" ".to_string()]).is_err());
        assert!(parse_cli_supported_models(&["gpt-5".to_string(), "gpt-5".to_string()]).is_err());
    }

    #[test]
    fn provider_view_projects_configured_credential_references() {
        let mut view = ServiceRouteConfig::default();
        view.providers.insert(
            "relay".to_string(),
            ProviderConfig {
                base_url: Some("https://relay.example/v1".to_string()),
                auth: UpstreamAuth {
                    auth_token_ref: Some(crate::config::CredentialRef::Native {
                        name: "relay.primary".to_string(),
                    }),
                    ..UpstreamAuth::default()
                },
                ..ProviderConfig::default()
            },
        );

        let provider = build_provider_view(&view, "relay").expect("provider view");
        let serialized = serde_json::to_value(&provider).expect("serialize provider view");

        assert_eq!(serialized["auth_token_ref"]["source"], "native");
        assert_eq!(serialized["auth_token_ref"]["name"], "relay.primary");
        assert_eq!(
            provider_auth_summary(&provider),
            "bearer_ref=native:relay.primary"
        );
    }

    #[test]
    fn setting_one_auth_kind_preserves_provider_shape_and_other_auth_kind() {
        let mut config = crate::config::HelperConfig::default();
        config.codex.providers.insert(
            "relay".to_string(),
            ProviderConfig {
                alias: Some("primary-relay".to_string()),
                enabled: false,
                base_url: Some("https://relay.example/v1".to_string()),
                continuity_domain: Some("relay-family".to_string()),
                auth: UpstreamAuth {
                    auth_token_env: Some("OLD_BEARER".to_string()),
                    api_key_env: Some("KEEP_API_KEY".to_string()),
                    allow_anonymous: Some(true),
                    ..UpstreamAuth::default()
                },
                inline_auth: UpstreamAuth {
                    auth_token: Some("old-inline-bearer".into()),
                    api_key_ref: Some(CredentialRef::SecretFile {
                        path: "/run/secrets/relay-api-key".to_string(),
                    }),
                    ..UpstreamAuth::default()
                },
                tags: BTreeMap::from([("region".to_string(), "west".to_string())]),
                supported_models: BTreeMap::from([("gpt-5.6".to_string(), true)]),
                model_mapping: BTreeMap::from([("gpt-*".to_string(), "gpt-5.6".to_string())]),
                limits: crate::config::ProviderConcurrencyLimits {
                    max_concurrent_requests: Some(17),
                    limit_group: Some("relay-pool".to_string()),
                },
                endpoints: BTreeMap::from([(
                    "secondary".to_string(),
                    ProviderEndpointConfig {
                        base_url: "https://secondary.example/v1".to_string(),
                        continuity_domain: Some("relay-family".to_string()),
                        enabled: true,
                        priority: 4,
                        tags: BTreeMap::from([("tier".to_string(), "backup".to_string())]),
                        supported_models: BTreeMap::from([("gpt-5.6".to_string(), true)]),
                        model_mapping: BTreeMap::new(),
                        limits: crate::config::ProviderConcurrencyLimits {
                            max_concurrent_requests: Some(3),
                            limit_group: None,
                        },
                    },
                )]),
            },
        );
        config.codex.providers.insert(
            "untouched".to_string(),
            ProviderConfig {
                base_url: Some("https://untouched.example/v1".to_string()),
                auth: UpstreamAuth {
                    auth_token_env: Some("UNTOUCHED_TOKEN".to_string()),
                    ..UpstreamAuth::default()
                },
                ..ProviderConfig::default()
            },
        );
        config.codex.routing = Some(crate::config::RouteGraphConfig::round_robin(vec![
            "relay".to_string(),
            "untouched".to_string(),
        ]));

        let before_provider = serde_json::to_value(&config.codex.providers["relay"])
            .expect("serialize provider before mutation");
        let before_untouched = serde_json::to_value(&config.codex.providers["untouched"])
            .expect("serialize untouched provider");
        let before_routing =
            serde_json::to_value(config.codex.routing.as_ref()).expect("serialize routing");
        set_provider_auth(
            config.codex.providers.get_mut("relay").expect("provider"),
            ProviderAuthKind::Bearer,
            ProviderAuthSource::Native("relay.primary".to_string()),
        );

        let rendered = toml::to_string(&config).expect("serialize mutated config");
        let reparsed = toml::from_str::<crate::config::HelperConfig>(&rendered)
            .expect("reparse mutated config");
        let provider = &reparsed.codex.providers["relay"];
        let effective = provider.effective_auth();
        assert_eq!(
            effective.auth_token_ref,
            Some(CredentialRef::Native {
                name: "relay.primary".to_string()
            })
        );
        assert!(effective.auth_token.is_none());
        assert!(effective.auth_token_env.is_none());
        assert_eq!(
            effective.api_key_ref,
            Some(CredentialRef::SecretFile {
                path: "/run/secrets/relay-api-key".to_string()
            })
        );
        assert!(effective.api_key_env.is_none());
        assert_eq!(provider.auth.api_key_env.as_deref(), Some("KEEP_API_KEY"));
        assert_eq!(effective.allow_anonymous, Some(true));

        let after_provider = serde_json::to_value(provider).expect("serialize provider after");
        for field in [
            "alias",
            "enabled",
            "base_url",
            "continuity_domain",
            "tags",
            "supported_models",
            "model_mapping",
            "limits",
            "endpoints",
        ] {
            assert_eq!(
                after_provider.get(field),
                before_provider.get(field),
                "provider field {field} changed"
            );
        }
        assert_eq!(
            serde_json::to_value(reparsed.codex.routing.as_ref()).expect("serialize routing after"),
            before_routing
        );
        assert_eq!(
            serde_json::to_value(&reparsed.codex.providers["untouched"])
                .expect("serialize untouched provider after"),
            before_untouched
        );
    }

    #[test]
    fn clearing_auth_removes_only_the_selected_kind_from_both_layers() {
        let mut provider = ProviderConfig {
            auth: UpstreamAuth {
                auth_token_env: Some("BEARER".to_string()),
                api_key_env: Some("NESTED_API_KEY".to_string()),
                ..UpstreamAuth::default()
            },
            inline_auth: UpstreamAuth {
                auth_token_ref: Some(CredentialRef::Native {
                    name: "relay.primary".to_string(),
                }),
                api_key: Some("inline-api-key".into()),
                ..UpstreamAuth::default()
            },
            ..ProviderConfig::default()
        };

        clear_provider_auth(&mut provider, ProviderAuthKind::Bearer);

        let effective = provider.effective_auth();
        assert!(effective.auth_token.is_none());
        assert!(effective.auth_token_env.is_none());
        assert!(effective.auth_token_ref.is_none());
        assert!(effective.api_key.is_some());
        assert_eq!(effective.api_key_env.as_deref(), Some("NESTED_API_KEY"));
    }

    #[test]
    fn provider_auth_sources_validate_each_reference_kind() {
        assert!(matches!(
            ProviderAuthSource::from_args(Some("relay.primary".to_string()), None, None)
                .expect("native source"),
            ProviderAuthSource::Native(_)
        ));
        assert!(matches!(
            ProviderAuthSource::from_args(
                None,
                Some(std::env::temp_dir().join("relay-secret")),
                None,
            )
            .expect("secret-file source"),
            ProviderAuthSource::SecretFile(_)
        ));
        assert!(matches!(
            ProviderAuthSource::from_args(None, None, Some("RELAY_TOKEN".to_string()))
                .expect("environment source"),
            ProviderAuthSource::Environment(_)
        ));

        assert!(
            ProviderAuthSource::from_args(Some("Relay.Invalid".to_string()), None, None).is_err()
        );
        assert!(ProviderAuthSource::from_args(None, Some("relative".into()), None).is_err());
        assert!(
            ProviderAuthSource::from_args(None, None, Some("BAD\u{1b}ENV".to_string())).is_err()
        );
    }

    #[test]
    fn provider_auth_service_selection_uses_the_locked_config_snapshot() {
        let mut config = crate::config::HelperConfig {
            default_service: Some(ServiceKind::Claude),
            ..crate::config::HelperConfig::default()
        };
        assert_eq!(select_requested_service(&config, None), "claude");
        assert_eq!(select_requested_service(&config, Some("codex")), "codex");

        config.default_service = Some(ServiceKind::Codex);
        assert_eq!(select_requested_service(&config, None), "codex");
        assert!(requested_service(true, true).is_err());
    }
}

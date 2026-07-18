use super::*;
use crate::routing_ir::compile_route_handshake_plan;
use std::collections::BTreeSet;

fn validate_runtime_provider_shape(
    service_name: &str,
    provider_name: &str,
    provider: &ProviderConfig,
) -> Result<()> {
    validate_provider_auth(service_name, provider_name, provider)?;
    validate_provider_concurrency_limits(service_name, provider_name, None, &provider.limits)?;
    let mut has_endpoint = false;
    if let Some(_base_url) = provider
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if provider.endpoints.contains_key("default") {
            anyhow::bail!(
                "[{service_name}] provider '{provider_name}' cannot define both base_url and endpoints.default"
            );
        }
        has_endpoint = true;
    }

    for (endpoint_name, endpoint) in &provider.endpoints {
        validate_provider_concurrency_limits(
            service_name,
            provider_name,
            Some(endpoint_name),
            &endpoint.limits,
        )?;
        if endpoint.base_url.trim().is_empty() {
            anyhow::bail!(
                "[{service_name}] provider '{provider_name}' endpoint '{endpoint_name}' has an empty base_url"
            );
        }
        has_endpoint = true;
    }

    if !has_endpoint {
        anyhow::bail!("[{service_name}] provider '{provider_name}' has no base_url or endpoints");
    }

    Ok(())
}

fn validate_provider_auth(
    service_name: &str,
    provider_name: &str,
    provider: &ProviderConfig,
) -> Result<()> {
    validate_auth_layer(service_name, provider_name, "auth", &provider.auth)?;
    validate_auth_layer(
        service_name,
        provider_name,
        "flattened auth",
        &provider.inline_auth,
    )?;
    validate_auth_layer(
        service_name,
        provider_name,
        "effective auth",
        &provider.effective_auth(),
    )
}

fn validate_auth_layer(
    service_name: &str,
    provider_name: &str,
    layer: &str,
    auth: &UpstreamAuth,
) -> Result<()> {
    validate_credential_kind(
        service_name,
        provider_name,
        layer,
        "auth_token_ref",
        auth.auth_token_ref.as_ref(),
        auth.auth_token.is_some() || auth.auth_token_env.is_some(),
    )?;
    validate_credential_kind(
        service_name,
        provider_name,
        layer,
        "api_key_ref",
        auth.api_key_ref.as_ref(),
        auth.api_key.is_some() || auth.api_key_env.is_some(),
    )
}

fn validate_credential_kind(
    service_name: &str,
    provider_name: &str,
    layer: &str,
    field: &str,
    reference: Option<&CredentialRef>,
    has_legacy_source: bool,
) -> Result<()> {
    let Some(reference) = reference else {
        return Ok(());
    };
    reference
        .validate()
        .with_context(|| format!("[{service_name}] provider '{provider_name}' {layer}.{field}"))?;
    if has_legacy_source {
        anyhow::bail!(
            "[{service_name}] provider '{provider_name}' {layer}.{field} cannot be combined with legacy inline or environment fields for the same credential kind"
        );
    }
    Ok(())
}

fn validate_provider_concurrency_limits(
    service_name: &str,
    provider_name: &str,
    endpoint_name: Option<&str>,
    limits: &ProviderConcurrencyLimits,
) -> Result<()> {
    if limits.max_concurrent_requests == Some(0) {
        if let Some(endpoint_name) = endpoint_name {
            anyhow::bail!(
                "[{service_name}] provider '{provider_name}' endpoint '{endpoint_name}' limits.max_concurrent_requests must be greater than 0"
            );
        }
        anyhow::bail!(
            "[{service_name}] provider '{provider_name}' limits.max_concurrent_requests must be greater than 0"
        );
    }
    Ok(())
}

fn validate_service_route_runtime_shape(
    service_name: &str,
    view: &ServiceRouteConfig,
) -> Result<()> {
    for (provider_name, provider) in &view.providers {
        validate_runtime_provider_shape(service_name, provider_name, provider)?;
    }
    Ok(())
}

fn default_routing_for_view(view: &ServiceRouteConfig) -> RouteGraphConfig {
    if view.providers.is_empty() {
        RouteGraphConfig::default()
    } else {
        RouteGraphConfig::ordered_failover(view.providers.keys().cloned().collect())
    }
}

pub fn effective_routing(view: &ServiceRouteConfig) -> RouteGraphConfig {
    view.routing
        .clone()
        .unwrap_or_else(|| default_routing_for_view(view))
}

pub fn resolved_provider_order(
    service_name: &str,
    view: &ServiceRouteConfig,
) -> Result<Vec<String>> {
    let plan = compile_route_handshake_plan(service_name, view)?;
    let mut seen = BTreeSet::new();
    Ok(plan
        .expanded_provider_order
        .into_iter()
        .filter(|provider_id| seen.insert(provider_id.clone()))
        .collect())
}

fn validate_service_config(service_name: &str, view: &ServiceRouteConfig) -> Result<()> {
    validate_service_route_runtime_shape(service_name, view)?;
    compile_route_handshake_plan(service_name, view)?;
    validate_service_profile_catalog(
        service_name,
        view.default_profile.as_deref(),
        &view.profiles,
    )
}

pub fn validate_helper_config(source: &HelperConfig) -> Result<()> {
    if !is_supported_config_version(source.version) {
        anyhow::bail!("unsupported route graph config version: {}", source.version);
    }
    source.fleet.validate()?;
    validate_service_config("codex", &source.codex)?;
    validate_service_config("claude", &source.claude)
}

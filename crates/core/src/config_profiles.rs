use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ServiceControlProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    /// Retained for legacy/v2 profiles. Routing-first v3 configs should express provider
    /// selection in the service routing block instead of profile-level station bindings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
}

fn merge_service_control_profile(
    mut base: ServiceControlProfile,
    overlay: &ServiceControlProfile,
) -> ServiceControlProfile {
    base.extends = overlay.extends.clone();
    if overlay.station.is_some() {
        base.station = overlay.station.clone();
    }
    if overlay.model.is_some() {
        base.model = overlay.model.clone();
    }
    if overlay.reasoning_effort.is_some() {
        base.reasoning_effort = overlay.reasoning_effort.clone();
    }
    if overlay.service_tier.is_some() {
        base.service_tier = overlay.service_tier.clone();
    }
    base
}

pub fn resolve_service_profile_from_catalog(
    profiles: &BTreeMap<String, ServiceControlProfile>,
    profile_name: &str,
) -> Result<ServiceControlProfile> {
    fn resolve_inner(
        profiles: &BTreeMap<String, ServiceControlProfile>,
        profile_name: &str,
        stack: &mut Vec<String>,
        cache: &mut BTreeMap<String, ServiceControlProfile>,
    ) -> Result<ServiceControlProfile> {
        if let Some(profile) = cache.get(profile_name) {
            return Ok(profile.clone());
        }

        if let Some(pos) = stack.iter().position(|name| name == profile_name) {
            let mut cycle = stack[pos..].to_vec();
            cycle.push(profile_name.to_string());
            anyhow::bail!("profile inheritance cycle: {}", cycle.join(" -> "));
        }

        let profile = profiles
            .get(profile_name)
            .with_context(|| format!("profile '{}' not found", profile_name))?;

        stack.push(profile_name.to_string());
        let resolved = if let Some(parent_name) = profile
            .extends
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            let parent = resolve_inner(profiles, parent_name, stack, cache)?;
            merge_service_control_profile(parent, profile)
        } else {
            profile.clone()
        };
        stack.pop();

        cache.insert(profile_name.to_string(), resolved.clone());
        Ok(resolved)
    }

    let mut stack = Vec::new();
    let mut cache = BTreeMap::new();
    resolve_inner(profiles, profile_name, &mut stack, &mut cache)
}

pub fn resolve_service_profile(
    mgr: &ServiceConfigManager,
    profile_name: &str,
) -> Result<ServiceControlProfile> {
    resolve_service_profile_from_catalog(&mgr.profiles, profile_name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExplicitCapabilitySupport {
    Unknown,
    Supported,
    Unsupported,
}

fn parse_boolish_capability_tag(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "y" | "on" | "supported" => Some(true),
        "0" | "false" | "no" | "n" | "off" | "unsupported" => Some(false),
        _ => None,
    }
}

fn explicit_capability_support_for_upstreams(
    upstreams: &[UpstreamConfig],
    tag_keys: &[&str],
) -> ExplicitCapabilitySupport {
    let mut saw_supported = false;
    let mut saw_explicit_unsupported = false;
    let mut saw_unknown = false;

    for upstream in upstreams {
        match tag_keys
            .iter()
            .find_map(|key| upstream.tags.get(*key))
            .and_then(|value| parse_boolish_capability_tag(value))
        {
            Some(true) => saw_supported = true,
            Some(false) => saw_explicit_unsupported = true,
            None => saw_unknown = true,
        }
    }

    if saw_supported {
        ExplicitCapabilitySupport::Supported
    } else if saw_explicit_unsupported && !saw_unknown {
        ExplicitCapabilitySupport::Unsupported
    } else {
        ExplicitCapabilitySupport::Unknown
    }
}

pub fn validate_profile_station_compatibility(
    service_name: &str,
    mgr: &ServiceConfigManager,
    profile_name: &str,
    profile: &ServiceControlProfile,
) -> Result<()> {
    let Some(station) = profile
        .station
        .as_deref()
        .map(str::trim)
        .filter(|station| !station.is_empty())
    else {
        return Ok(());
    };

    let Some(config) = mgr.station(station) else {
        anyhow::bail!(
            "[{service_name}] profile '{}' references missing station '{}'",
            profile_name,
            station
        );
    };

    if let Some(model) = profile
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        let supported = config.upstreams.is_empty()
            || config.upstreams.iter().any(|upstream| {
                crate::model_routing::is_model_supported(
                    &upstream.supported_models,
                    &upstream.model_mapping,
                    model,
                )
            });
        if !supported {
            anyhow::bail!(
                "[{service_name}] profile '{}' model '{}' is not supported by station '{}'",
                profile_name,
                model,
                station
            );
        }
    }

    if let Some(service_tier) = profile
        .service_tier
        .as_deref()
        .map(str::trim)
        .filter(|service_tier| !service_tier.is_empty())
        && explicit_capability_support_for_upstreams(
            &config.upstreams,
            &[
                "supports_service_tier",
                "supports_service_tiers",
                "supports_fast_mode",
                "supports_fast",
            ],
        ) == ExplicitCapabilitySupport::Unsupported
    {
        anyhow::bail!(
            "[{service_name}] profile '{}' requires service_tier '{}' but station '{}' explicitly disables fast/service-tier support",
            profile_name,
            service_tier,
            station
        );
    }

    if let Some(reasoning_effort) = profile
        .reasoning_effort
        .as_deref()
        .map(str::trim)
        .filter(|reasoning_effort| !reasoning_effort.is_empty())
        && explicit_capability_support_for_upstreams(
            &config.upstreams,
            &["supports_reasoning_effort", "supports_reasoning"],
        ) == ExplicitCapabilitySupport::Unsupported
    {
        anyhow::bail!(
            "[{service_name}] profile '{}' requires reasoning_effort '{}' but station '{}' explicitly disables reasoning support",
            profile_name,
            reasoning_effort,
            station
        );
    }

    Ok(())
}

pub(crate) fn validate_service_profiles(
    service_name: &str,
    mgr: &ServiceConfigManager,
) -> Result<()> {
    if let Some(default_profile) = mgr.default_profile.as_deref()
        && !mgr.profiles.contains_key(default_profile)
    {
        anyhow::bail!(
            "[{service_name}] default_profile '{}' does not exist in profiles",
            default_profile
        );
    }

    for profile_name in mgr.profiles.keys() {
        let resolved = resolve_service_profile(mgr, profile_name)?;
        validate_profile_station_compatibility(service_name, mgr, profile_name, &resolved)?;
    }

    Ok(())
}

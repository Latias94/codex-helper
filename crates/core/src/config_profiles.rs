use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ServiceControlProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
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

pub(crate) fn validate_service_profile_catalog(
    service_name: &str,
    default_profile: Option<&str>,
    profiles: &BTreeMap<String, ServiceControlProfile>,
) -> Result<()> {
    if let Some(default_profile) = default_profile
        && !profiles.contains_key(default_profile)
    {
        anyhow::bail!(
            "[{service_name}] default_profile '{}' does not exist in profiles",
            default_profile
        );
    }

    for profile_name in profiles.keys() {
        resolve_service_profile_from_catalog(profiles, profile_name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_five_profile_ignores_removed_station_projection() {
        let config = toml::from_str::<HelperConfig>(
            r#"
version = 5

[codex.profiles.daily]
station = "legacy-route-label"
model = "gpt-5"
"#,
        )
        .expect("version 5 config with a removed profile key should remain readable");

        assert_eq!(
            config.codex.profiles["daily"].model.as_deref(),
            Some("gpt-5")
        );
        let serialized = toml::to_string(&config).expect("serialize canonical config");
        assert!(!serialized.contains("station"));
        assert!(!serialized.contains("legacy-route-label"));
    }
}

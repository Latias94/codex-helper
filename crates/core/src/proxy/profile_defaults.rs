use crate::config::ServiceRouteConfig;
pub(super) fn effective_default_profile_name(view: &ServiceRouteConfig) -> Option<String> {
    view.default_profile
        .as_deref()
        .filter(|name| view.profiles.contains_key(*name))
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::effective_default_profile_name;
    use crate::config::{ServiceControlProfile, ServiceRouteConfig};

    fn make_view() -> ServiceRouteConfig {
        ServiceRouteConfig {
            default_profile: Some("fallback".to_string()),
            profiles: BTreeMap::from([
                ("fallback".to_string(), ServiceControlProfile::default()),
                (
                    "fast".to_string(),
                    ServiceControlProfile {
                        model: Some("gpt-5.4".to_string()),
                        ..Default::default()
                    },
                ),
            ]),
            ..ServiceRouteConfig::default()
        }
    }

    #[test]
    fn effective_default_profile_uses_configured_profile() {
        let name = effective_default_profile_name(&make_view()).expect("default profile");

        assert_eq!(name, "fallback");
    }
}

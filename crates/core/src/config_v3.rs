use super::*;
use std::collections::BTreeSet;

const ROUTING_STATION_NAME: &str = "routing";

#[derive(Debug, Clone)]
pub struct ConfigV3MigrationReport {
    pub config: ProxyConfigV3,
    pub warnings: Vec<String>,
}

fn merge_auth(block: &UpstreamAuth, inline: &UpstreamAuth) -> UpstreamAuth {
    UpstreamAuth {
        auth_token: inline
            .auth_token
            .clone()
            .or_else(|| block.auth_token.clone()),
        auth_token_env: inline
            .auth_token_env
            .clone()
            .or_else(|| block.auth_token_env.clone()),
        api_key: inline.api_key.clone().or_else(|| block.api_key.clone()),
        api_key_env: inline
            .api_key_env
            .clone()
            .or_else(|| block.api_key_env.clone()),
    }
}

fn remove_import_metadata_tags(tags: &mut BTreeMap<String, String>) {
    tags.remove("provider_id");
    tags.remove("requires_openai_auth");
    if tags
        .get("source")
        .is_some_and(|value| value == "codex-config")
    {
        tags.remove("source");
    }
}

fn compact_service_view_v3_for_write(view: &mut ServiceViewV3) {
    for provider in view.providers.values_mut() {
        remove_import_metadata_tags(&mut provider.tags);
        for endpoint in provider.endpoints.values_mut() {
            remove_import_metadata_tags(&mut endpoint.tags);
        }
    }
}

pub fn compact_v3_config_for_write(cfg: &mut ProxyConfigV3) {
    compact_service_view_v3_for_write(&mut cfg.codex);
    compact_service_view_v3_for_write(&mut cfg.claude);
}

fn provider_v3_to_v2(
    service_name: &str,
    provider_name: &str,
    provider: &ProviderConfigV3,
) -> Result<ProviderConfigV2> {
    let mut endpoints = BTreeMap::new();
    if let Some(base_url) = provider
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
        endpoints.insert(
            "default".to_string(),
            ProviderEndpointV2 {
                base_url: base_url.to_string(),
                enabled: true,
                priority: default_provider_endpoint_priority(),
                tags: BTreeMap::new(),
                supported_models: BTreeMap::new(),
                model_mapping: BTreeMap::new(),
            },
        );
    }

    for (endpoint_name, endpoint) in &provider.endpoints {
        if endpoint.base_url.trim().is_empty() {
            anyhow::bail!(
                "[{service_name}] provider '{provider_name}' endpoint '{endpoint_name}' has an empty base_url"
            );
        }
        endpoints.insert(
            endpoint_name.clone(),
            ProviderEndpointV2 {
                base_url: endpoint.base_url.trim().to_string(),
                enabled: endpoint.enabled,
                priority: endpoint.priority,
                tags: endpoint.tags.clone(),
                supported_models: endpoint.supported_models.clone(),
                model_mapping: endpoint.model_mapping.clone(),
            },
        );
    }

    if endpoints.is_empty() {
        anyhow::bail!("[{service_name}] provider '{provider_name}' has no base_url or endpoints");
    }

    let mut tags = provider.tags.clone();
    tags.insert("provider_id".to_string(), provider_name.to_string());

    Ok(ProviderConfigV2 {
        alias: provider.alias.clone(),
        enabled: provider.enabled,
        auth: merge_auth(&provider.auth, &provider.inline_auth),
        tags,
        supported_models: provider.supported_models.clone(),
        model_mapping: provider.model_mapping.clone(),
        endpoints,
    })
}

fn provider_order_from_routing(
    service_name: &str,
    view: &ServiceViewV3,
    routing: &RoutingConfigV3,
) -> Result<Vec<String>> {
    let default_order = || view.providers.keys().cloned().collect::<Vec<_>>();

    let mut raw_order = if routing.order.is_empty() {
        default_order()
    } else {
        routing.order.clone()
    };

    if matches!(routing.policy, RoutingPolicyV3::ManualSticky) {
        let target = routing
            .target
            .clone()
            .or_else(|| raw_order.first().cloned())
            .or_else(|| {
                if view.providers.len() == 1 {
                    view.providers.keys().next().cloned()
                } else {
                    None
                }
            })
            .with_context(|| {
                format!(
                    "[{service_name}] manual-sticky routing requires target or a non-empty order"
                )
            })?;
        raw_order = vec![target];
    }

    let mut seen = BTreeMap::<String, ()>::new();
    let mut order = Vec::new();
    for provider_name in raw_order {
        if !view.providers.contains_key(&provider_name) {
            anyhow::bail!("[{service_name}] routing references missing provider '{provider_name}'");
        }
        if seen.insert(provider_name.clone(), ()).is_none() {
            order.push(provider_name);
        }
    }

    if matches!(routing.policy, RoutingPolicyV3::TagPreferred) && !routing.prefer_tags.is_empty() {
        let mut preferred = Vec::new();
        let mut fallback = Vec::new();
        for provider_name in order {
            let provider = view
                .providers
                .get(&provider_name)
                .expect("provider existence was validated above");
            if provider_matches_any_filter(&provider.tags, &routing.prefer_tags) {
                preferred.push(provider_name);
            } else {
                fallback.push(provider_name);
            }
        }

        if matches!(routing.on_exhausted, RoutingExhaustedActionV3::Stop) {
            if preferred.is_empty() {
                anyhow::bail!(
                    "[{service_name}] tag-preferred routing with on_exhausted = 'stop' matched no providers"
                );
            }
            return Ok(preferred);
        }

        preferred.extend(fallback);
        return Ok(preferred);
    }

    Ok(order)
}

fn provider_matches_any_filter(
    tags: &BTreeMap<String, String>,
    filters: &[BTreeMap<String, String>],
) -> bool {
    filters.iter().any(|filter| {
        !filter.is_empty()
            && filter
                .iter()
                .all(|(key, value)| tags.get(key) == Some(value))
    })
}

fn compile_service_view_v3(service_name: &str, view: &ServiceViewV3) -> Result<ServiceViewV2> {
    let providers = view
        .providers
        .iter()
        .map(|(provider_name, provider)| {
            provider_v3_to_v2(service_name, provider_name, provider)
                .map(|provider| (provider_name.clone(), provider))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;

    let routing = view.routing.clone().unwrap_or_else(|| {
        if view.providers.len() <= 1 {
            RoutingConfigV3 {
                policy: RoutingPolicyV3::OrderedFailover,
                order: view.providers.keys().cloned().collect(),
                target: None,
                prefer_tags: Vec::new(),
                on_exhausted: RoutingExhaustedActionV3::Continue,
            }
        } else {
            RoutingConfigV3::default()
        }
    });

    let route_order = provider_order_from_routing(service_name, view, &routing)?;
    let groups = if route_order.is_empty() {
        BTreeMap::new()
    } else {
        BTreeMap::from([(
            ROUTING_STATION_NAME.to_string(),
            GroupConfigV2 {
                alias: Some("active routing".to_string()),
                enabled: true,
                level: default_service_config_level(),
                members: route_order
                    .into_iter()
                    .map(|provider| GroupMemberRefV2 {
                        provider,
                        endpoint_names: Vec::new(),
                        preferred: false,
                    })
                    .collect(),
            },
        )])
    };

    Ok(ServiceViewV2 {
        active_group: if groups.is_empty() {
            None
        } else {
            Some(ROUTING_STATION_NAME.to_string())
        },
        default_profile: view.default_profile.clone(),
        profiles: view.profiles.clone(),
        providers,
        groups,
    })
}

pub fn compile_v3_to_v2(v3: &ProxyConfigV3) -> Result<ProxyConfigV2> {
    if v3.version != 3 {
        anyhow::bail!("unsupported v3 config version: {}", v3.version);
    }

    Ok(ProxyConfigV2 {
        version: 2,
        codex: compile_service_view_v3("codex", &v3.codex)?,
        claude: compile_service_view_v3("claude", &v3.claude)?,
        retry: v3.retry.clone(),
        notify: v3.notify.clone(),
        default_service: v3.default_service,
        ui: v3.ui.clone(),
    })
}

pub fn compile_v3_to_runtime(v3: &ProxyConfigV3) -> Result<ProxyConfig> {
    let v2 = compile_v3_to_v2(v3)?;
    let mut runtime = compile_v2_to_runtime(&v2)?;
    runtime.version = Some(3);
    Ok(runtime)
}

fn endpoint_v2_to_v3(endpoint: &ProviderEndpointV2) -> ProviderEndpointV3 {
    ProviderEndpointV3 {
        base_url: endpoint.base_url.clone(),
        enabled: endpoint.enabled,
        priority: endpoint.priority,
        tags: endpoint.tags.clone(),
        supported_models: endpoint.supported_models.clone(),
        model_mapping: endpoint.model_mapping.clone(),
    }
}

fn endpoint_can_be_inlined(endpoint_name: &str, endpoint: &ProviderEndpointV2) -> bool {
    endpoint_name == "default"
        && endpoint.enabled
        && endpoint.priority == default_provider_endpoint_priority()
        && endpoint.tags.is_empty()
        && endpoint.supported_models.is_empty()
        && endpoint.model_mapping.is_empty()
}

fn provider_v2_to_v3(provider: &ProviderConfigV2) -> ProviderConfigV3 {
    let mut out = ProviderConfigV3 {
        alias: provider.alias.clone(),
        enabled: provider.enabled,
        base_url: None,
        auth: UpstreamAuth::default(),
        inline_auth: provider.auth.clone(),
        tags: provider.tags.clone(),
        supported_models: provider.supported_models.clone(),
        model_mapping: provider.model_mapping.clone(),
        endpoints: BTreeMap::new(),
    };

    if provider.endpoints.len() == 1
        && let Some((endpoint_name, endpoint)) = provider.endpoints.iter().next()
        && endpoint_can_be_inlined(endpoint_name, endpoint)
    {
        out.base_url = Some(endpoint.base_url.clone());
        return out;
    }

    out.endpoints = provider
        .endpoints
        .iter()
        .map(|(name, endpoint)| (name.clone(), endpoint_v2_to_v3(endpoint)))
        .collect();
    out
}

fn member_order_for_group(group: &GroupConfigV2) -> Vec<String> {
    let mut members = group.members.iter().enumerate().collect::<Vec<_>>();
    members.sort_by_key(|(idx, member)| (!member.preferred, *idx));
    members
        .into_iter()
        .map(|(_, member)| member.provider.clone())
        .collect()
}

fn ordered_selected_v2_group_names(view: &ServiceViewV2) -> Vec<String> {
    let active = view.active_group.as_deref();
    let mut groups = view.groups.iter().collect::<Vec<_>>();
    groups.retain(|(name, group)| group.enabled || active == Some(name.as_str()));

    if groups.is_empty() {
        if let Some(active_name) = active
            && view.groups.contains_key(active_name)
        {
            return vec![active_name.to_string()];
        }

        return view.groups.keys().min().cloned().into_iter().collect();
    }

    groups.sort_by(|(left_name, left), (right_name, right)| {
        left.level
            .cmp(&right.level)
            .then_with(|| {
                let left_is_active = active == Some(left_name.as_str());
                let right_is_active = active == Some(right_name.as_str());
                right_is_active.cmp(&left_is_active)
            })
            .then_with(|| left_name.cmp(right_name))
    });
    groups
        .into_iter()
        .map(|(group_name, _)| group_name.clone())
        .collect()
}

fn routing_order_from_v2_groups(view: &ServiceViewV2) -> Vec<String> {
    if view.groups.is_empty() {
        return view.providers.keys().cloned().collect();
    }

    let mut seen = BTreeMap::<String, ()>::new();
    let mut order = Vec::new();
    for group_name in ordered_selected_v2_group_names(view) {
        let Some(group) = view.groups.get(group_name.as_str()) else {
            continue;
        };
        for provider in member_order_for_group(group) {
            if seen.insert(provider.clone(), ()).is_none() {
                order.push(provider);
            }
        }
    }
    order
}

fn enabled_endpoint_name_set(provider: &ProviderConfigV2) -> BTreeSet<String> {
    provider
        .endpoints
        .iter()
        .filter(|(_, endpoint)| endpoint.enabled)
        .map(|(endpoint_name, _)| endpoint_name.clone())
        .collect()
}

fn selected_enabled_endpoint_name_set(
    provider: &ProviderConfigV2,
    member: &GroupMemberRefV2,
) -> BTreeSet<String> {
    if member.endpoint_names.is_empty() {
        return enabled_endpoint_name_set(provider);
    }

    member
        .endpoint_names
        .iter()
        .filter(|endpoint_name| {
            provider
                .endpoints
                .get(endpoint_name.as_str())
                .is_some_and(|endpoint| endpoint.enabled)
        })
        .cloned()
        .collect()
}

fn endpoint_scope_is_full_provider(provider: &ProviderConfigV2, member: &GroupMemberRefV2) -> bool {
    selected_enabled_endpoint_name_set(provider, member) == enabled_endpoint_name_set(provider)
}

fn collect_service_v2_to_v3_warnings(
    service_name: &str,
    view: &ServiceViewV2,
    warnings: &mut Vec<String>,
) {
    if !view.groups.is_empty() {
        let selected_group_names = ordered_selected_v2_group_names(view);
        if view.groups.len() > 1 {
            let selected = if selected_group_names.is_empty() {
                "<none>".to_string()
            } else {
                selected_group_names.join(", ")
            };
            warnings.push(format!(
                "[{service_name}] v2 has {} stations/groups; v3 migration flattens the effective route into a single routing.order (selected groups: {selected}). Station aliases, levels, and enabled flags are not preserved as station metadata.",
                view.groups.len()
            ));
        }

        let active = view.active_group.as_deref();
        let omitted_disabled = view
            .groups
            .iter()
            .filter(|(group_name, group)| !group.enabled && active != Some(group_name.as_str()))
            .map(|(group_name, _)| group_name.clone())
            .collect::<Vec<_>>();
        if !omitted_disabled.is_empty() {
            warnings.push(format!(
                "[{service_name}] disabled inactive v2 stations/groups are omitted from v3 routing.order: {}.",
                omitted_disabled.join(", ")
            ));
        }

        let included_disabled_active = selected_group_names
            .iter()
            .filter(|group_name| {
                view.groups
                    .get(group_name.as_str())
                    .is_some_and(|group| !group.enabled && active == Some(group_name.as_str()))
            })
            .cloned()
            .collect::<Vec<_>>();
        if !included_disabled_active.is_empty() {
            warnings.push(format!(
                "[{service_name}] disabled active v2 stations/groups remain routeable in v3 to match current runtime fallback behavior: {}.",
                included_disabled_active.join(", ")
            ));
        }

        let mut provider_occurrences = BTreeMap::<String, usize>::new();
        for group_name in &selected_group_names {
            let Some(group) = view.groups.get(group_name.as_str()) else {
                continue;
            };
            for member in &group.members {
                *provider_occurrences
                    .entry(member.provider.clone())
                    .or_insert(0) += 1;

                let Some(provider) = view.providers.get(member.provider.as_str()) else {
                    continue;
                };
                if !endpoint_scope_is_full_provider(provider, member) {
                    let selected = selected_enabled_endpoint_name_set(provider, member)
                        .into_iter()
                        .collect::<Vec<_>>()
                        .join(", ");
                    let available = enabled_endpoint_name_set(provider)
                        .into_iter()
                        .collect::<Vec<_>>()
                        .join(", ");
                    warnings.push(format!(
                        "[{service_name}] v2 group '{group_name}' scopes provider '{}' to endpoint(s) [{}], but v3 routing.order is provider-level; provider '{}' keeps all enabled endpoint(s) [{}].",
                        member.provider, selected, member.provider, available
                    ));
                }
            }
        }

        let repeated = provider_occurrences
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(provider, count)| format!("{provider} x{count}"))
            .collect::<Vec<_>>();
        if !repeated.is_empty() {
            warnings.push(format!(
                "[{service_name}] providers referenced multiple times in selected v2 groups are de-duplicated in v3 routing.order: {}.",
                repeated.join(", ")
            ));
        }
    }

    let cleared_profiles = view
        .profiles
        .iter()
        .filter(|(_, profile)| {
            profile
                .station
                .as_deref()
                .map(str::trim)
                .is_some_and(|station| !station.is_empty())
        })
        .map(|(profile_name, profile)| {
            format!(
                "{} -> {}",
                profile_name,
                profile.station.as_deref().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    if !cleared_profiles.is_empty() {
        warnings.push(format!(
            "[{service_name}] profile station bindings are cleared because v3 routing owns active provider selection: {}.",
            cleared_profiles.join(", ")
        ));
    }
}

fn migrate_service_v2_to_v3(view: &ServiceViewV2) -> ServiceViewV3 {
    let mut profiles = view.profiles.clone();
    for profile in profiles.values_mut() {
        if profile
            .station
            .as_deref()
            .map(str::trim)
            .is_some_and(|station| !station.is_empty())
        {
            profile.station = None;
        }
    }

    let providers = view
        .providers
        .iter()
        .map(|(name, provider)| (name.clone(), provider_v2_to_v3(provider)))
        .collect::<BTreeMap<_, _>>();

    let order = routing_order_from_v2_groups(view);
    let routing = if providers.is_empty() {
        None
    } else {
        Some(RoutingConfigV3 {
            policy: RoutingPolicyV3::OrderedFailover,
            order,
            target: None,
            prefer_tags: Vec::new(),
            on_exhausted: RoutingExhaustedActionV3::Continue,
        })
    };

    ServiceViewV3 {
        default_profile: view.default_profile.clone(),
        profiles,
        providers,
        routing,
    }
}

pub fn migrate_v2_to_v3(v2: &ProxyConfigV2) -> Result<ProxyConfigV3> {
    Ok(migrate_v2_to_v3_with_report(v2)?.config)
}

pub fn migrate_v2_to_v3_with_report(v2: &ProxyConfigV2) -> Result<ConfigV3MigrationReport> {
    let compact = compact_v2_config(v2)?;
    let mut warnings = Vec::new();
    collect_service_v2_to_v3_warnings("codex", &compact.codex, &mut warnings);
    collect_service_v2_to_v3_warnings("claude", &compact.claude, &mut warnings);

    let config = ProxyConfigV3 {
        version: 3,
        codex: migrate_service_v2_to_v3(&compact.codex),
        claude: migrate_service_v2_to_v3(&compact.claude),
        retry: compact.retry,
        notify: compact.notify,
        default_service: compact.default_service,
        ui: compact.ui,
    };

    Ok(ConfigV3MigrationReport { config, warnings })
}

pub fn migrate_legacy_to_v3(old: &ProxyConfig) -> Result<ProxyConfigV3> {
    Ok(migrate_legacy_to_v3_with_report(old)?.config)
}

pub fn migrate_legacy_to_v3_with_report(old: &ProxyConfig) -> Result<ConfigV3MigrationReport> {
    migrate_v2_to_v3_with_report(&migrate_legacy_to_v2(old))
}

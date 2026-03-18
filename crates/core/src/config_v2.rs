use super::*;

fn merge_string_maps(
    provider_values: &BTreeMap<String, String>,
    endpoint_values: &BTreeMap<String, String>,
) -> HashMap<String, String> {
    let mut merged = provider_values
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<HashMap<_, _>>();
    for (key, value) in endpoint_values {
        merged.insert(key.clone(), value.clone());
    }
    merged
}

fn merge_bool_maps(
    provider_values: &BTreeMap<String, bool>,
    endpoint_values: &BTreeMap<String, bool>,
) -> HashMap<String, bool> {
    let mut merged = provider_values
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect::<HashMap<_, _>>();
    for (key, value) in endpoint_values {
        merged.insert(key.clone(), *value);
    }
    merged
}

fn compare_provider_endpoints(
    left_name: &str,
    left: &ProviderEndpointV2,
    right_name: &str,
    right: &ProviderEndpointV2,
) -> std::cmp::Ordering {
    left.priority
        .cmp(&right.priority)
        .then_with(|| left_name.cmp(right_name))
        .then_with(|| left.base_url.cmp(&right.base_url))
}

fn ordered_provider_endpoint_names(provider: &ProviderConfigV2) -> Vec<String> {
    let mut endpoints = provider.endpoints.iter().collect::<Vec<_>>();
    endpoints.sort_by(|(left_name, left), (right_name, right)| {
        compare_provider_endpoints(left_name, left, right_name, right)
    });
    endpoints
        .into_iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>()
}

fn compile_service_view_v2(
    service_name: &str,
    view: &ServiceViewV2,
) -> Result<ServiceConfigManager> {
    if let Some(active_group) = view.active_group.as_deref()
        && !view.groups.contains_key(active_group)
    {
        anyhow::bail!(
            "[{service_name}] active_station '{}' does not exist in stations",
            active_group
        );
    }

    let mut configs = HashMap::new();
    for (group_name, group) in &view.groups {
        let mut members = group.members.iter().enumerate().collect::<Vec<_>>();
        members.sort_by_key(|(idx, member)| (!member.preferred, *idx));

        let mut upstreams = Vec::new();
        for (_, member) in members {
            let provider = view.providers.get(&member.provider).with_context(|| {
                format!(
                    "[{service_name}] group '{}' references missing provider '{}'",
                    group_name, member.provider
                )
            })?;

            if !provider.enabled {
                continue;
            }
            if provider.endpoints.is_empty() {
                anyhow::bail!(
                    "[{service_name}] provider '{}' has no endpoints",
                    member.provider
                );
            }

            let endpoint_names = if member.endpoint_names.is_empty() {
                ordered_provider_endpoint_names(provider)
            } else {
                member.endpoint_names.clone()
            };

            for endpoint_name in endpoint_names {
                let endpoint = provider.endpoints.get(&endpoint_name).with_context(|| {
                    format!(
                        "[{service_name}] group '{}' references missing endpoint '{}.{}'",
                        group_name, member.provider, endpoint_name
                    )
                })?;
                if !endpoint.enabled {
                    continue;
                }

                upstreams.push(UpstreamConfig {
                    base_url: endpoint.base_url.clone(),
                    auth: provider.auth.clone(),
                    tags: merge_string_maps(&provider.tags, &endpoint.tags),
                    supported_models: merge_bool_maps(
                        &provider.supported_models,
                        &endpoint.supported_models,
                    ),
                    model_mapping: merge_string_maps(
                        &provider.model_mapping,
                        &endpoint.model_mapping,
                    ),
                });
            }
        }

        configs.insert(
            group_name.clone(),
            ServiceConfig {
                name: group_name.clone(),
                alias: group.alias.clone(),
                enabled: group.enabled,
                level: group.level.clamp(1, 10),
                upstreams,
            },
        );
    }

    let mgr = ServiceConfigManager {
        active: view.active_group.clone(),
        default_profile: view.default_profile.clone(),
        profiles: view.profiles.clone(),
        configs,
    };
    validate_service_profiles(service_name, &mgr)?;
    Ok(mgr)
}

pub fn build_persisted_station_catalog(view: &ServiceViewV2) -> PersistedStationsCatalog {
    let mut providers = view
        .providers
        .iter()
        .map(|(name, provider)| PersistedStationProviderRef {
            name: name.clone(),
            alias: provider.alias.clone(),
            enabled: provider.enabled,
            endpoints: ordered_provider_endpoint_names(provider)
                .into_iter()
                .filter_map(|endpoint_name| {
                    provider
                        .endpoints
                        .get(endpoint_name.as_str())
                        .map(|endpoint| PersistedStationProviderEndpointRef {
                            name: endpoint_name,
                            base_url: endpoint.base_url.clone(),
                            enabled: endpoint.enabled,
                        })
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    providers.sort_by(|a, b| a.name.cmp(&b.name));

    let mut stations = view
        .groups
        .iter()
        .map(|(name, station)| PersistedStationSpec {
            name: name.clone(),
            alias: station.alias.clone(),
            enabled: station.enabled,
            level: station.level.clamp(1, 10),
            members: station.members.clone(),
        })
        .collect::<Vec<_>>();
    stations.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.name.cmp(&b.name)));

    PersistedStationsCatalog {
        stations,
        providers,
    }
}

pub fn build_persisted_provider_catalog(view: &ServiceViewV2) -> PersistedProvidersCatalog {
    let mut providers = view
        .providers
        .iter()
        .map(|(name, provider)| PersistedProviderSpec {
            name: name.clone(),
            alias: provider.alias.clone(),
            enabled: provider.enabled,
            auth_token_env: provider.auth.auth_token_env.clone(),
            api_key_env: provider.auth.api_key_env.clone(),
            endpoints: provider
                .endpoints
                .iter()
                .map(|(endpoint_name, endpoint)| PersistedProviderEndpointSpec {
                    name: endpoint_name.clone(),
                    base_url: endpoint.base_url.clone(),
                    enabled: endpoint.enabled,
                    priority: endpoint.priority,
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    providers.sort_by(|a, b| a.name.cmp(&b.name));
    for provider in &mut providers {
        provider.endpoints.sort_by(|a, b| {
            a.priority.cmp(&b.priority).then_with(|| {
                a.name
                    .cmp(&b.name)
                    .then_with(|| a.base_url.cmp(&b.base_url))
            })
        });
    }
    PersistedProvidersCatalog { providers }
}

pub fn compile_v2_to_runtime(v2: &ProxyConfigV2) -> Result<ProxyConfig> {
    if v2.version != 2 {
        anyhow::bail!("unsupported v2 config version: {}", v2.version);
    }

    Ok(ProxyConfig {
        version: Some(v2.version),
        codex: compile_service_view_v2("codex", &v2.codex)?,
        claude: compile_service_view_v2("claude", &v2.claude)?,
        retry: v2.retry.clone(),
        notify: v2.notify.clone(),
        default_service: v2.default_service,
        ui: v2.ui.clone(),
    })
}

fn migrate_service_manager_to_v2(mgr: &ServiceConfigManager) -> ServiceViewV2 {
    let mut providers = BTreeMap::new();
    let mut groups = BTreeMap::new();

    let mut group_names = mgr.stations().keys().cloned().collect::<Vec<_>>();
    group_names.sort();

    for group_name in group_names {
        let Some(svc) = mgr.station(&group_name) else {
            continue;
        };

        let mut members: Vec<GroupMemberRefV2> = Vec::new();
        for (idx, upstream) in svc.upstreams.iter().enumerate() {
            let provider_name = format!("{}__u{:02}", group_name, idx + 1);
            let endpoint_name = "default".to_string();

            let mut endpoints = BTreeMap::new();
            endpoints.insert(
                endpoint_name.clone(),
                ProviderEndpointV2 {
                    base_url: upstream.base_url.clone(),
                    enabled: true,
                    priority: 0,
                    tags: upstream
                        .tags
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    supported_models: upstream
                        .supported_models
                        .iter()
                        .map(|(k, v)| (k.clone(), *v))
                        .collect(),
                    model_mapping: upstream
                        .model_mapping
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                },
            );

            providers.insert(
                provider_name.clone(),
                ProviderConfigV2 {
                    alias: upstream.tags.get("provider_id").cloned(),
                    enabled: true,
                    auth: upstream.auth.clone(),
                    tags: BTreeMap::new(),
                    supported_models: BTreeMap::new(),
                    model_mapping: BTreeMap::new(),
                    endpoints,
                },
            );

            members.push(GroupMemberRefV2 {
                provider: provider_name,
                endpoint_names: vec![endpoint_name],
                preferred: false,
            });
        }

        groups.insert(
            group_name.clone(),
            GroupConfigV2 {
                alias: svc.alias.clone(),
                enabled: svc.enabled,
                level: svc.level,
                members,
            },
        );
    }

    ServiceViewV2 {
        active_group: mgr.active.clone(),
        default_profile: mgr.default_profile.clone(),
        profiles: mgr.profiles.clone(),
        providers,
        groups,
    }
}

pub fn migrate_legacy_to_v2(old: &ProxyConfig) -> ProxyConfigV2 {
    ProxyConfigV2 {
        version: 2,
        codex: migrate_service_manager_to_v2(&old.codex),
        claude: migrate_service_manager_to_v2(&old.claude),
        retry: old.retry.clone(),
        notify: old.notify.clone(),
        default_service: old.default_service,
        ui: old.ui.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProviderBucketKey {
    hint: String,
    auth_token: Option<String>,
    auth_token_env: Option<String>,
    api_key: Option<String>,
    api_key_env: Option<String>,
    enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EndpointBucketKey {
    enabled: bool,
    base_url: String,
    tags: Vec<(String, String)>,
    supported_models: Vec<(String, bool)>,
    model_mapping: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
struct EndpointCompactBuild {
    key: EndpointBucketKey,
    upstream: UpstreamConfig,
    original_names: Vec<String>,
    priority: u32,
}

#[derive(Debug, Clone)]
struct ProviderCompactBuild {
    alias: Option<String>,
    auth: UpstreamAuth,
    enabled: bool,
    empty_provider_tags: BTreeMap<String, String>,
    empty_provider_supported_models: BTreeMap<String, bool>,
    empty_provider_model_mapping: BTreeMap<String, String>,
    endpoints: Vec<EndpointCompactBuild>,
    endpoint_index: HashMap<EndpointBucketKey, usize>,
    endpoint_names: HashMap<EndpointBucketKey, String>,
}

#[derive(Debug, Clone)]
struct GroupOccurrence {
    provider: String,
    endpoint_name: String,
    preferred: bool,
}

fn hash_string_map_to_btree(values: &HashMap<String, String>) -> BTreeMap<String, String> {
    values.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

fn hash_bool_map_to_btree(values: &HashMap<String, bool>) -> BTreeMap<String, bool> {
    values.iter().map(|(k, v)| (k.clone(), *v)).collect()
}

fn string_map_without_common(
    values: &HashMap<String, String>,
    common: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    values
        .iter()
        .filter(|(key, value)| common.get(*key) != Some(*value))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn bool_map_without_common(
    values: &HashMap<String, bool>,
    common: &BTreeMap<String, bool>,
) -> BTreeMap<String, bool> {
    values
        .iter()
        .filter(|(key, value)| common.get(*key) != Some(*value))
        .map(|(k, v)| (k.clone(), *v))
        .collect()
}

fn common_string_entries(
    upstreams: &[UpstreamConfig],
    selector: fn(&UpstreamConfig) -> &HashMap<String, String>,
) -> BTreeMap<String, String> {
    let Some(first) = upstreams.first() else {
        return BTreeMap::new();
    };
    let mut common = hash_string_map_to_btree(selector(first));
    common.retain(|key, value| {
        upstreams
            .iter()
            .skip(1)
            .all(|upstream| selector(upstream).get(key) == Some(value))
    });
    common
}

fn common_bool_entries(
    upstreams: &[UpstreamConfig],
    selector: fn(&UpstreamConfig) -> &HashMap<String, bool>,
) -> BTreeMap<String, bool> {
    let Some(first) = upstreams.first() else {
        return BTreeMap::new();
    };
    let mut common = hash_bool_map_to_btree(selector(first));
    common.retain(|key, value| {
        upstreams
            .iter()
            .skip(1)
            .all(|upstream| selector(upstream).get(key) == Some(value))
    });
    common
}

fn sanitize_schema_key(raw: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.trim().chars() {
        let normalized = ch.to_ascii_lowercase();
        if normalized.is_ascii_alphanumeric() {
            out.push(normalized);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        fallback.to_string()
    } else {
        out
    }
}

fn looks_generated_provider_name(name: &str) -> bool {
    if let Some((prefix, suffix)) = name.rsplit_once("__u") {
        !prefix.is_empty() && suffix.len() == 2 && suffix.chars().all(|ch| ch.is_ascii_digit())
    } else {
        false
    }
}

fn looks_default_endpoint_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    if lower == "default" {
        return true;
    }
    lower
        .strip_prefix("default-")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()))
}

fn env_provider_hint(auth: &UpstreamAuth) -> Option<String> {
    let raw = auth
        .auth_token_env
        .as_deref()
        .or(auth.api_key_env.as_deref())?
        .trim()
        .to_ascii_lowercase();
    let mut hint = raw;
    for suffix in ["_auth_token", "_api_key", "_token", "_key"] {
        if let Some(stripped) = hint.strip_suffix(suffix) {
            hint = stripped.to_string();
            break;
        }
    }
    Some(sanitize_schema_key(&hint, "provider"))
}

fn host_provider_hint(base_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(base_url).ok()?;
    let host = url.host_str()?;
    let labels = host.split('.').collect::<Vec<_>>();
    let raw = if labels.len() >= 2 {
        labels[labels.len() - 2]
    } else {
        host
    };
    Some(sanitize_schema_key(raw, "provider"))
}

fn subdomain_or_host_hint(base_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(base_url).ok()?;
    let host = url.host_str()?;
    let labels = host.split('.').collect::<Vec<_>>();
    if labels.len() >= 3 {
        let first = labels[0].to_ascii_lowercase();
        if !matches!(first.as_str(), "api" | "www" | "gateway") {
            return Some(sanitize_schema_key(&first, "endpoint"));
        }
    }
    host_provider_hint(base_url).map(|hint| sanitize_schema_key(&hint, "endpoint"))
}

fn path_endpoint_hint(base_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(base_url).ok()?;
    let segment = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .filter(|segment| !matches!(*segment, "v1" | "v2" | "api"))
        .next_back()?;
    Some(sanitize_schema_key(segment, "endpoint"))
}

fn allocate_unique_name(base: &str, counters: &mut HashMap<String, usize>) -> String {
    let entry = counters.entry(base.to_string()).or_insert(0);
    *entry += 1;
    if *entry == 1 {
        base.to_string()
    } else {
        format!("{base}-{}", *entry)
    }
}

fn provider_name_hint(
    original_name: &str,
    provider: &ProviderConfigV2,
) -> (String, Option<String>) {
    let original_name = original_name.trim();
    let explicit_alias = provider
        .alias
        .clone()
        .map(|alias| alias.trim().to_string())
        .filter(|alias| !alias.is_empty());
    let mut raw_hint = None;
    if !original_name.is_empty() && !looks_generated_provider_name(original_name) {
        raw_hint = Some(original_name.to_string());
    }
    if raw_hint.is_none() {
        raw_hint = explicit_alias.clone();
    }
    if raw_hint.is_none() {
        raw_hint = provider
            .tags
            .get("provider_id")
            .cloned()
            .filter(|value| !value.trim().is_empty());
    }
    if raw_hint.is_none() {
        raw_hint = provider
            .endpoints
            .values()
            .find_map(|endpoint| endpoint.tags.get("provider_id").cloned())
            .filter(|value| !value.trim().is_empty());
    }
    if raw_hint.is_none() {
        raw_hint = env_provider_hint(&provider.auth);
    }
    if raw_hint.is_none() {
        raw_hint = provider
            .endpoints
            .values()
            .find_map(|endpoint| host_provider_hint(&endpoint.base_url));
    }

    let raw_hint = raw_hint.unwrap_or_else(|| "provider".to_string());
    let slug = sanitize_schema_key(&raw_hint, "provider");
    let alias = explicit_alias
        .filter(|alias| alias != original_name)
        .or_else(|| {
            if raw_hint == slug {
                None
            } else {
                Some(raw_hint)
            }
        });
    (slug, alias)
}

fn should_persist_provider_alias(alias: &str, provider_name: &str) -> bool {
    let alias = alias.trim();
    !alias.is_empty() && alias != provider_name.trim()
}

fn endpoint_name_hint(endpoint: &EndpointCompactBuild, total: usize) -> String {
    if total == 1 {
        return "default".to_string();
    }

    if let Some(name) = endpoint
        .original_names
        .iter()
        .find(|name| !name.trim().is_empty() && !looks_default_endpoint_name(name))
    {
        return sanitize_schema_key(name, "endpoint");
    }
    if let Some(region) = endpoint.upstream.tags.get("region") {
        return sanitize_schema_key(region, "endpoint");
    }
    if let Some(hint) = subdomain_or_host_hint(&endpoint.upstream.base_url) {
        return hint;
    }
    if let Some(hint) = path_endpoint_hint(&endpoint.upstream.base_url) {
        return hint;
    }
    "endpoint".to_string()
}

fn endpoint_bucket_key(
    endpoint: &ProviderEndpointV2,
    effective: &UpstreamConfig,
) -> EndpointBucketKey {
    let mut tags = effective
        .tags
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<Vec<_>>();
    tags.sort();
    let mut supported_models = effective
        .supported_models
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect::<Vec<_>>();
    supported_models.sort_by(|a, b| a.0.cmp(&b.0));
    let mut model_mapping = effective
        .model_mapping
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<Vec<_>>();
    model_mapping.sort_by(|a, b| a.0.cmp(&b.0));

    EndpointBucketKey {
        enabled: endpoint.enabled,
        base_url: effective.base_url.clone(),
        tags,
        supported_models,
        model_mapping,
    }
}

fn effective_upstream_for_endpoint(
    provider: &ProviderConfigV2,
    endpoint: &ProviderEndpointV2,
) -> UpstreamConfig {
    UpstreamConfig {
        base_url: endpoint.base_url.clone(),
        auth: provider.auth.clone(),
        tags: merge_string_maps(&provider.tags, &endpoint.tags),
        supported_models: merge_bool_maps(&provider.supported_models, &endpoint.supported_models),
        model_mapping: merge_string_maps(&provider.model_mapping, &endpoint.model_mapping),
    }
}

fn compact_service_view_v2(view: &ServiceViewV2) -> Result<ServiceViewV2> {
    let mut provider_name_counters = HashMap::new();
    let mut bucket_lookup = HashMap::<ProviderBucketKey, String>::new();
    let mut provider_lookup = HashMap::<String, String>::new();
    let mut endpoint_lookup = HashMap::<(String, String), (String, EndpointBucketKey)>::new();
    let mut builds = BTreeMap::<String, ProviderCompactBuild>::new();

    for (original_provider_name, provider) in &view.providers {
        let (hint, alias) = provider_name_hint(original_provider_name, provider);
        let bucket_key = ProviderBucketKey {
            hint: hint.clone(),
            auth_token: provider.auth.auth_token.clone(),
            auth_token_env: provider.auth.auth_token_env.clone(),
            api_key: provider.auth.api_key.clone(),
            api_key_env: provider.auth.api_key_env.clone(),
            enabled: provider.enabled,
        };

        let canonical_provider_name = if let Some(existing) = bucket_lookup.get(&bucket_key) {
            existing.clone()
        } else {
            let allocated = allocate_unique_name(&hint, &mut provider_name_counters);
            bucket_lookup.insert(bucket_key, allocated.clone());
            allocated
        };
        provider_lookup.insert(
            original_provider_name.clone(),
            canonical_provider_name.clone(),
        );

        let build = builds
            .entry(canonical_provider_name.clone())
            .or_insert_with(|| ProviderCompactBuild {
                alias: alias.clone(),
                auth: provider.auth.clone(),
                enabled: provider.enabled,
                empty_provider_tags: provider.tags.clone(),
                empty_provider_supported_models: provider.supported_models.clone(),
                empty_provider_model_mapping: provider.model_mapping.clone(),
                endpoints: Vec::new(),
                endpoint_index: HashMap::new(),
                endpoint_names: HashMap::new(),
            });
        if build.alias.is_none() {
            build.alias = alias;
        }

        if provider.endpoints.is_empty() {
            continue;
        }

        for (original_endpoint_name, endpoint) in &provider.endpoints {
            let effective = effective_upstream_for_endpoint(provider, endpoint);
            let key = endpoint_bucket_key(endpoint, &effective);
            let index = if let Some(index) = build.endpoint_index.get(&key) {
                *index
            } else {
                let index = build.endpoints.len();
                build.endpoints.push(EndpointCompactBuild {
                    key: key.clone(),
                    upstream: effective.clone(),
                    original_names: Vec::new(),
                    priority: endpoint.priority,
                });
                build.endpoint_index.insert(key.clone(), index);
                index
            };
            if endpoint.priority < build.endpoints[index].priority {
                build.endpoints[index].priority = endpoint.priority;
            }
            build.endpoints[index]
                .original_names
                .push(original_endpoint_name.clone());
            endpoint_lookup.insert(
                (
                    original_provider_name.clone(),
                    original_endpoint_name.clone(),
                ),
                (canonical_provider_name.clone(), key),
            );
        }
    }

    for build in builds.values_mut() {
        build.endpoints.sort_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| left.upstream.base_url.cmp(&right.upstream.base_url))
        });
        let mut counters = HashMap::new();
        let total = build.endpoints.len();
        for endpoint in &build.endpoints {
            let base = endpoint_name_hint(endpoint, total);
            let name = allocate_unique_name(&base, &mut counters);
            build.endpoint_names.insert(endpoint.key.clone(), name);
        }
    }

    let mut providers = BTreeMap::new();
    for (provider_name, build) in &builds {
        if build.endpoints.is_empty() {
            providers.insert(
                provider_name.clone(),
                ProviderConfigV2 {
                    alias: build
                        .alias
                        .clone()
                        .filter(|alias| should_persist_provider_alias(alias, provider_name)),
                    enabled: build.enabled,
                    auth: build.auth.clone(),
                    tags: build.empty_provider_tags.clone(),
                    supported_models: build.empty_provider_supported_models.clone(),
                    model_mapping: build.empty_provider_model_mapping.clone(),
                    endpoints: BTreeMap::new(),
                },
            );
            continue;
        }

        let upstreams = build
            .endpoints
            .iter()
            .map(|endpoint| endpoint.upstream.clone())
            .collect::<Vec<_>>();
        let common_tags = common_string_entries(&upstreams, |upstream| &upstream.tags);
        let common_supported_models =
            common_bool_entries(&upstreams, |upstream| &upstream.supported_models);
        let common_model_mapping =
            common_string_entries(&upstreams, |upstream| &upstream.model_mapping);

        let mut endpoints = BTreeMap::new();
        for endpoint in &build.endpoints {
            let endpoint_name = build
                .endpoint_names
                .get(&endpoint.key)
                .expect("endpoint name should exist")
                .clone();
            endpoints.insert(
                endpoint_name,
                ProviderEndpointV2 {
                    base_url: endpoint.upstream.base_url.clone(),
                    enabled: endpoint.key.enabled,
                    priority: endpoint.priority,
                    tags: string_map_without_common(&endpoint.upstream.tags, &common_tags),
                    supported_models: bool_map_without_common(
                        &endpoint.upstream.supported_models,
                        &common_supported_models,
                    ),
                    model_mapping: string_map_without_common(
                        &endpoint.upstream.model_mapping,
                        &common_model_mapping,
                    ),
                },
            );
        }

        providers.insert(
            provider_name.clone(),
            ProviderConfigV2 {
                alias: build
                    .alias
                    .clone()
                    .filter(|alias| should_persist_provider_alias(alias, provider_name)),
                enabled: build.enabled,
                auth: build.auth.clone(),
                tags: common_tags,
                supported_models: common_supported_models,
                model_mapping: common_model_mapping,
                endpoints,
            },
        );
    }

    let mut groups = BTreeMap::new();
    for (group_name, group) in &view.groups {
        let mut occurrences = Vec::new();
        for member in &group.members {
            let provider = view.providers.get(&member.provider).with_context(|| {
                format!(
                    "group '{}' references missing provider '{}'",
                    group_name, member.provider
                )
            })?;
            let endpoint_names = if member.endpoint_names.is_empty() {
                ordered_provider_endpoint_names(provider)
            } else {
                member.endpoint_names.clone()
            };

            for endpoint_name in endpoint_names {
                let (canonical_provider, endpoint_key) = endpoint_lookup
                    .get(&(member.provider.clone(), endpoint_name.clone()))
                    .with_context(|| {
                        format!(
                            "group '{}' references missing endpoint '{}.{}'",
                            group_name, member.provider, endpoint_name
                        )
                    })?;
                let mapped_endpoint_name = builds
                    .get(canonical_provider)
                    .and_then(|build| build.endpoint_names.get(endpoint_key))
                    .cloned()
                    .with_context(|| {
                        format!(
                            "group '{}' cannot map endpoint '{}.{}'",
                            group_name, member.provider, endpoint_name
                        )
                    })?;
                occurrences.push(GroupOccurrence {
                    provider: canonical_provider.clone(),
                    endpoint_name: mapped_endpoint_name,
                    preferred: member.preferred,
                });
            }
        }

        let mut members: Vec<GroupMemberRefV2> = Vec::new();
        for occurrence in occurrences {
            if let Some(last) = members.last_mut()
                && last.provider == occurrence.provider
                && last.preferred == occurrence.preferred
            {
                last.endpoint_names.push(occurrence.endpoint_name);
            } else {
                members.push(GroupMemberRefV2 {
                    provider: occurrence.provider,
                    endpoint_names: vec![occurrence.endpoint_name],
                    preferred: occurrence.preferred,
                });
            }
        }

        groups.insert(
            group_name.clone(),
            GroupConfigV2 {
                alias: group.alias.clone(),
                enabled: group.enabled,
                level: group.level,
                members,
            },
        );
    }

    Ok(ServiceViewV2 {
        active_group: view.active_group.clone(),
        default_profile: view.default_profile.clone(),
        profiles: view.profiles.clone(),
        providers,
        groups,
    })
}

pub fn compact_v2_config(v2: &ProxyConfigV2) -> Result<ProxyConfigV2> {
    Ok(ProxyConfigV2 {
        version: 2,
        codex: compact_service_view_v2(&v2.codex)?,
        claude: compact_service_view_v2(&v2.claude)?,
        retry: v2.retry.clone(),
        notify: v2.notify.clone(),
        default_service: v2.default_service,
        ui: v2.ui.clone(),
    })
}

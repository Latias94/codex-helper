use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri, header};
use futures_util::{StreamExt, stream};
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;

use crate::codex_integration::CodexPatchMode;
use crate::config::{RoutingPolicyV4, ServiceViewV4, UpstreamConfig};
use crate::model_routing;
use crate::routing_ir::{
    RoutePlanAttemptState, RoutePlanTemplate, RouteRequestContext,
    compile_v4_route_plan_template_for_compat_runtime, compile_v4_route_plan_template_with_request,
};

use super::ProxyService;
use super::attempt_request::inject_auth_headers;
use super::client_identity::extract_session_identity;
use super::headers::filter_request_headers;
use super::models_compat::{
    catalog_model_id, maybe_decode_models_response_body_without_translation, merge_model_catalogs,
};
use super::request_preparation::{
    codex_path_is_responses_or_compact, load_request_config_context, session_identity_value,
};
use super::request_routing::{RequestRouteSelection, service_view_with_route_target_override};
use super::target_builder::upstream_request_url;

const CATALOG_TTL: Duration = Duration::from_secs(300);
const FAILED_CATALOG_TTL: Duration = Duration::from_secs(30);
const FETCH_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_CATALOG_BYTES: usize = 8 * 1024 * 1024;
const MAX_CACHE_ENTRIES: usize = 128;
pub(super) const BUNDLED_CLIENT_VERSION: &str = "0.162.1";

type CatalogSlot = Arc<AsyncMutex<Option<CachedCatalog>>>;

#[derive(Default)]
pub(super) struct ModelCatalogCache {
    entries: Mutex<HashMap<u64, CatalogSlot>>,
}

struct CachedCatalog {
    expires_at: Instant,
    value: Option<Value>,
}

pub(super) struct CatalogRouteRequest<'a> {
    pub(super) headers: &'a HeaderMap,
    pub(super) uri: &'a Uri,
    pub(super) patch: CodexPatchMode,
    pub(super) model: Option<&'a str>,
    pub(super) effort: Option<&'a str>,
}

pub(super) struct CatalogResponse {
    pub(super) status: StatusCode,
    pub(super) headers: HeaderMap,
    pub(super) body: Bytes,
}

pub(super) fn is_model_catalog_request(service: &str, method: &Method, uri: &Uri) -> bool {
    service == "codex"
        && method == Method::GET
        && matches!(uri.path().trim_end_matches('/'), "/models" | "/v1/models")
}

pub(super) fn catalog_uri(uri: &Uri, headers: &HeaderMap) -> Uri {
    let Ok(mut query_url) = reqwest::Url::parse("http://localhost/") else {
        return uri.clone();
    };
    query_url.set_query(uri.query());
    let mut pairs = query_url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    let provided = pairs
        .iter()
        .find(|(key, value)| key == "client_version" && !value.is_empty())
        .map(|(_, value)| value.clone());
    let inferred = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once('/').map(|(_, tail)| tail))
        .and_then(|tail| tail.split_whitespace().next())
        .filter(|version| {
            version.split('.').count() >= 3
                && version.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
        .unwrap_or(BUNDLED_CLIENT_VERSION);
    pairs.retain(|(key, _)| key != "client_version");
    query_url
        .query_pairs_mut()
        .clear()
        .extend_pairs(pairs)
        .append_pair("client_version", provided.as_deref().unwrap_or(inferred));
    let path = if matches!(uri.path().trim_end_matches('/'), "/models" | "/v1/models") {
        "/models"
    } else {
        uri.path()
    };
    format!("{path}?{}", query_url.query().unwrap_or_default())
        .parse()
        .unwrap_or_else(|_| uri.clone())
}

fn source_headers(
    upstream: &UpstreamConfig,
    client: &HeaderMap,
    patch: CodexPatchMode,
) -> HeaderMap {
    let mut headers = filter_request_headers(client);
    inject_auth_headers("codex", &upstream.auth, patch, &mut headers);
    headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("identity"),
    );
    headers.remove(header::CONTENT_TYPE);
    headers
}

fn catalog_cache_key(upstream: &UpstreamConfig, headers: &HeaderMap, uri: &Uri) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source_identity(upstream, headers).hash(&mut hasher);
    uri.to_string().hash(&mut hasher);
    hasher.finish()
}

fn source_identity(upstream: &UpstreamConfig, headers: &HeaderMap) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    upstream.base_url.hash(&mut hasher);
    for key in [
        "authorization",
        "x-api-key",
        "x-goog-api-key",
        "chatgpt-account-id",
        "openai-organization",
        "openai-project",
    ] {
        key.hash(&mut hasher);
        for value in headers.get_all(key) {
            value.as_bytes().hash(&mut hasher);
        }
    }
    hasher.finish()
}

pub(super) async fn fetch_catalog_response(
    client: &reqwest::Client,
    base_url: &str,
    uri: &Uri,
    headers: HeaderMap,
) -> Result<CatalogResponse, String> {
    let uri = catalog_uri(uri, &headers);
    let url = upstream_request_url(base_url, &uri)
        .map_err(|_| "invalid model catalog URL".to_string())?;
    let response = client
        .get(url)
        .headers(headers)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|_| "model catalog transport error".to_string())?;
    let status = response.status();
    let headers = response.headers().clone();
    let mut chunks = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| "model catalog body read failed".to_string())?;
        if body.len().saturating_add(chunk.len()) > MAX_CATALOG_BYTES {
            return Err("model catalog exceeds size limit".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    let body = maybe_decode_models_response_body_without_translation(
        "codex",
        "/models",
        &headers,
        Bytes::from(body),
    );
    Ok(CatalogResponse {
        status,
        headers,
        body,
    })
}

impl ModelCatalogCache {
    async fn get(
        &self,
        client: &reqwest::Client,
        upstream: &UpstreamConfig,
        uri: &Uri,
        headers: HeaderMap,
    ) -> Option<Value> {
        let uri = catalog_uri(uri, &headers);
        let key = catalog_cache_key(upstream, &headers, &uri);
        let slot = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if entries.len() >= MAX_CACHE_ENTRIES && !entries.contains_key(&key) {
                entries.retain(|_, slot| {
                    slot.try_lock().ok().is_some_and(|guard| {
                        guard
                            .as_ref()
                            .is_some_and(|entry| entry.expires_at > Instant::now())
                    })
                });
                if entries.len() >= MAX_CACHE_ENTRIES {
                    entries.clear();
                }
            }
            entries.entry(key).or_default().clone()
        };
        let mut entry = slot.lock().await;
        if let Some(entry) = entry
            .as_ref()
            .filter(|entry| entry.expires_at > Instant::now())
        {
            return entry.value.clone();
        }
        let value = fetch_catalog_response(client, &upstream.base_url, &uri, headers)
            .await
            .ok()
            .filter(|response| response.status.is_success())
            .and_then(|response| serde_json::from_slice::<Value>(&response.body).ok())
            .filter(|value| {
                let native = value.get("models").and_then(Value::as_array);
                native
                    .or_else(|| value.get("data").and_then(Value::as_array))
                    .is_some_and(|models| {
                        models.is_empty()
                            || models
                                .iter()
                                .any(|model| catalog_model_id(model, native.is_some()).is_some())
                    })
            });
        let now = Instant::now();
        *entry = Some(CachedCatalog {
            expires_at: now
                + if value.is_some() {
                    CATALOG_TTL
                } else {
                    FAILED_CATALOG_TTL
                },
            value: value.clone(),
        });
        value
    }

    fn observed(
        &self,
        upstream: &UpstreamConfig,
        request: &CatalogRouteRequest<'_>,
    ) -> Option<Value> {
        let uri = match request.uri.query() {
            Some(query) => format!("/models?{query}").parse().ok()?,
            None => Uri::from_static("/models"),
        };
        let uri = catalog_uri(&uri, request.headers);
        let headers = source_headers(upstream, request.headers, request.patch);
        let key = catalog_cache_key(upstream, &headers, &uri);
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let entry = entries.get(&key)?.try_lock().ok()?;
        entry
            .as_ref()
            .filter(|entry| entry.expires_at > Instant::now())?
            .value
            .clone()
    }

    pub(super) fn exclude_unsupported(
        &self,
        template: &RoutePlanTemplate,
        state: &mut RoutePlanAttemptState,
        request: CatalogRouteRequest<'_>,
    ) {
        if !codex_path_is_responses_or_compact(request.uri.path()) {
            return;
        }
        let Some(model) = request.model else {
            return;
        };
        for candidate in &template.candidates {
            let upstream = candidate.to_upstream_config();
            if self
                .observed(&upstream, &request)
                .is_some_and(|catalog| !source_allows(&catalog, &upstream, model, request.effort))
            {
                if let (Some(station), Some(index)) = (
                    candidate.compatibility_station_name.as_deref(),
                    candidate.compatibility_upstream_index,
                ) {
                    state.avoid_upstream(station, index);
                } else {
                    state.avoid_candidate(template, candidate);
                }
            }
        }
    }
}

fn source_allows(
    catalog: &Value,
    upstream: &UpstreamConfig,
    requested: &str,
    effort: Option<&str>,
) -> bool {
    let effective = model_routing::effective_model(&upstream.model_mapping, requested);
    let effective = effective.strip_prefix("models/").unwrap_or(&effective);
    let native = catalog.get("models").and_then(Value::as_array);
    let entries = native.or_else(|| catalog.get("data").and_then(Value::as_array));
    let Some(entries) = entries else {
        return true;
    };
    let Some(model) = entries
        .iter()
        .find(|model| catalog_model_id(model, native.is_some()).is_some_and(|id| id == effective))
    else {
        return false;
    };
    if model.get("supported_in_api").and_then(Value::as_bool) == Some(false) {
        return false;
    }
    if let Some(effort) = effort
        && let Some(levels) = model
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
    {
        return levels
            .iter()
            .any(|level| level.get("effort").and_then(Value::as_str) == Some(effort));
    }
    true
}

fn route_catalog(catalog: &Value, upstream: &UpstreamConfig) -> Value {
    let native = catalog.get("models").and_then(Value::as_array);
    let field = if native.is_some() { "slug" } else { "id" };
    let entries = native.or_else(|| catalog.get("data").and_then(Value::as_array));
    let mut models = Vec::new();
    let mut mappings = upstream.model_mapping.iter().collect::<Vec<_>>();
    mappings.sort_by_key(|(alias, _)| *alias);
    for model in entries.into_iter().flatten() {
        let Some(id) = catalog_model_id(model, native.is_some()) else {
            continue;
        };
        if model_routing::is_model_supported(
            &upstream.supported_models,
            &upstream.model_mapping,
            &id,
        ) && model_routing::effective_model(&upstream.model_mapping, &id) == id
        {
            models.push(model.clone());
        }
        for (alias, effective) in &mappings {
            let effective = effective.strip_prefix("models/").unwrap_or(effective);
            if model_routing::match_wildcard(effective, &id) {
                let alias = model_routing::apply_wildcard_mapping(effective, alias, &id);
                let target = model_routing::effective_model(&upstream.model_mapping, &alias);
                if alias.contains('*') || target.strip_prefix("models/").unwrap_or(&target) != id {
                    continue;
                }
                let mut model = model.clone();
                model[field] = Value::String(alias);
                models.push(model);
            }
        }
    }
    serde_json::json!({if native.is_some() { "models" } else { "data" }: models})
}

fn catalog_route_request(headers: &HeaderMap) -> RouteRequestContext {
    RouteRequestContext {
        headers: headers
            .iter()
            .filter_map(|(key, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (key.to_string(), value.to_string()))
            })
            .collect(),
        ..RouteRequestContext::default()
    }
}

fn source_is_routable(
    slug: &str,
    effort: Option<&str>,
    upstream: &UpstreamConfig,
    view: &ServiceViewV4,
    request: &mut RouteRequestContext,
) -> bool {
    request.model = Some(slug.to_string());
    request.reasoning_effort = effort.map(ToOwned::to_owned);
    let tiers = std::iter::once(None).chain(view.routing.iter().flat_map(|routing| {
        routing
            .routes
            .values()
            .filter_map(|node| node.when.as_ref()?.service_tier.as_deref().map(Some))
    }));
    for tier in tiers {
        request.service_tier = tier.map(ToOwned::to_owned);
        if source_is_routable_in_context(upstream, view, request) {
            return true;
        }
    }
    false
}

fn source_is_routable_in_context(
    upstream: &UpstreamConfig,
    view: &ServiceViewV4,
    request: &mut RouteRequestContext,
) -> bool {
    let Some(slug) = request.model.clone() else {
        return false;
    };
    for (method, path) in [
        ("POST", "/responses"),
        ("POST", "/v1/responses"),
        ("POST", "/backend-api/codex/responses"),
        ("GET", "/responses"),
        ("GET", "/v1/responses"),
        ("GET", "/backend-api/codex/responses"),
    ] {
        request.method = Some(method.to_string());
        request.path = Some(path.to_string());
        if compile_v4_route_plan_template_with_request("codex", view, request)
            .ok()
            .is_some_and(|template| {
                template.candidates.iter().any(|candidate| {
                    candidate.base_url == upstream.base_url
                        && upstream
                            .tags
                            .get("provider_id")
                            .is_none_or(|id| id == &candidate.provider_id)
                        && upstream
                            .tags
                            .get("endpoint_id")
                            .is_none_or(|id| id == &candidate.endpoint_id)
                        && {
                            let target = candidate.to_upstream_config();
                            model_routing::is_model_supported(
                                &target.supported_models,
                                &target.model_mapping,
                                &slug,
                            )
                        }
                })
            })
        {
            return true;
        }
    }
    false
}

fn retain_routable_models(
    catalog: &mut Value,
    raw: &Value,
    upstream: &UpstreamConfig,
    view: &ServiceViewV4,
    headers: &HeaderMap,
) {
    let native = catalog.get("models").and_then(Value::as_array).is_some();
    let key = if native { "models" } else { "data" };
    let Some(models) = catalog.get_mut(key).and_then(Value::as_array_mut) else {
        return;
    };
    let mut request = catalog_route_request(headers);
    models.retain(|model| {
        catalog_model_id(model, native).is_some_and(|slug| {
            // Discovery has no selected effort; reuse the route compiler for supported client efforts.
            [
                None,
                Some("none"),
                Some("minimal"),
                Some("low"),
                Some("medium"),
                Some("high"),
                Some("xhigh"),
                Some("max"),
                Some("ultra"),
            ]
            .into_iter()
            .any(|effort| {
                source_allows(raw, upstream, &slug, effort)
                    && source_is_routable(&slug, effort, upstream, view, &mut request)
            })
        })
    });
}

fn constrain_conditional_efforts(
    catalog: &mut Value,
    sources: &[(usize, UpstreamConfig, Value)],
    view: &ServiceViewV4,
    headers: &HeaderMap,
) {
    if !view
        .routing
        .iter()
        .flat_map(|routing| routing.routes.values())
        .any(|node| node.strategy == RoutingPolicyV4::Conditional)
    {
        return;
    }
    let Some(models) = catalog.get_mut("models").and_then(Value::as_array_mut) else {
        return;
    };
    let mut request = catalog_route_request(headers);
    for model in models {
        let Some(slug) = catalog_model_id(model, true) else {
            continue;
        };
        let Some(levels) = model
            .get_mut("supported_reasoning_levels")
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        levels.retain(|level| {
            let effort = level.get("effort").and_then(Value::as_str);
            sources.iter().any(|(_, upstream, raw)| {
                source_allows(raw, upstream, &slug, effort)
                    && source_is_routable(&slug, effort, upstream, view, &mut request)
            })
        });
        let levels = model["supported_reasoning_levels"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let preferred = model.get("default_reasoning_level").and_then(Value::as_str);
        let choices = preferred.into_iter().chain(
            levels
                .iter()
                .filter_map(|level| level.get("effort").and_then(Value::as_str)),
        );
        let selected = choices
            .filter(|effort| levels.iter().any(|level| level["effort"] == *effort))
            .find(|effort| {
                request.model = Some(slug.clone());
                request.reasoning_effort = Some((*effort).to_string());
                request.service_tier = model
                    .get("default_service_tier")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                sources.iter().any(|(_, upstream, raw)| {
                    source_allows(raw, upstream, &slug, Some(effort))
                        && source_is_routable_in_context(upstream, view, &mut request)
                })
            });
        model["default_reasoning_level"] = selected
            .map(|effort| Value::String(effort.to_string()))
            .unwrap_or(Value::Null);
    }
}

pub(super) async fn handle_model_catalog(
    proxy: &ProxyService,
    req: Request<Body>,
) -> Result<Response<Body>, (StatusCode, String)> {
    let (parts, _) = req.into_parts();
    let client_headers = parts.headers;
    let client_uri = parts.uri;
    let config = load_request_config_context(proxy).await;
    let session = extract_session_identity(&client_headers);
    let session_id = session_identity_value(session.as_ref());
    let mut effective_view = None;
    let upstreams = if let Some(v4) = config.v4_snapshot.as_ref() {
        let mut view = v4.codex.clone();
        if let Some((target, _)) = proxy
            .route_target_override_for_session(session_id.as_deref())
            .await
        {
            view =
                service_view_with_route_target_override("codex", &view, &target).map_err(|_| {
                    (
                        StatusCode::BAD_GATEWAY,
                        "invalid catalog route target".to_string(),
                    )
                })?;
        }
        let template = compile_v4_route_plan_template_for_compat_runtime("codex", &view)
            .map_err(|_| (StatusCode::BAD_GATEWAY, "invalid catalog route".to_string()))?;
        let runtime = proxy
            .state
            .route_plan_runtime_state_for_provider_endpoints("codex")
            .await;
        let upstreams = template
            .candidates
            .iter()
            .filter(|candidate| {
                !runtime
                    .provider_endpoint(&template.candidate_provider_endpoint_key(candidate))
                    .runtime_disabled
            })
            .map(|candidate| {
                let mut upstream = candidate.to_upstream_config();
                upstream
                    .tags
                    .insert("provider_id".to_string(), candidate.provider_id.clone());
                upstream
            })
            .collect::<Vec<_>>();
        effective_view = Some(view);
        upstreams
    } else {
        let routes = proxy
            .lbs_for_request(
                &config.cfg_snapshot,
                None,
                &RouteRequestContext::default(),
                session_id.as_deref(),
            )
            .await;
        match routes {
            RequestRouteSelection::Legacy { lbs } => lbs
                .iter()
                .flat_map(|lb| lb.service.upstreams.clone())
                .collect(),
            RequestRouteSelection::RouteGraph { .. } => Vec::new(),
        }
    };
    let uri = catalog_uri(&client_uri, &client_headers);
    let fetches = upstreams
        .into_iter()
        .enumerate()
        .map(|(index, upstream)| {
            let headers = source_headers(&upstream, &client_headers, config.codex_patch_mode);
            let uri = uri.clone();
            async move {
                let value = proxy
                    .model_catalog
                    .get(&proxy.client, &upstream, &uri, headers)
                    .await;
                (index, upstream, value)
            }
        })
        .collect::<Vec<_>>();
    let mut fetches = stream::iter(fetches).buffer_unordered(8);
    let mut catalogs = Vec::new();
    let deadline = tokio::time::Instant::now() + FETCH_TIMEOUT + Duration::from_millis(100);
    while let Ok(Some((index, upstream, value))) =
        tokio::time::timeout_at(deadline, fetches.next()).await
    {
        if let Some(value) = value {
            catalogs.push((index, upstream, value));
        }
    }
    catalogs.sort_by_key(|(index, _, _)| *index);
    let projected = catalogs
        .iter()
        .map(|(_, upstream, raw)| {
            let mut value = route_catalog(raw, upstream);
            if let Some(view) = effective_view.as_ref() {
                retain_routable_models(&mut value, raw, upstream, view, &client_headers);
            }
            value
        })
        .collect::<Vec<_>>();
    let mut merged = merge_model_catalogs(&projected).ok_or_else(|| {
        (
            StatusCode::BAD_GATEWAY,
            "no upstream returned a usable model catalog".to_string(),
        )
    })?;
    if let Some(view) = effective_view.as_ref() {
        constrain_conditional_efforts(&mut merged, &catalogs, view, &client_headers);
    }
    let body = serde_json::to_vec(&merged).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "model catalog serialization failed".to_string(),
        )
    })?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body))
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "model catalog response failed".to_string(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProviderConfigV4, RoutingConditionV4, RoutingConfigV4, RoutingNodeV4};
    use std::collections::BTreeMap;

    fn upstream() -> UpstreamConfig {
        UpstreamConfig {
            base_url: "https://fixture.example/v1".to_string(),
            auth: Default::default(),
            tags: HashMap::from([("provider_id".to_string(), "first".to_string())]),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }
    }

    fn conditional_view(condition: RoutingConditionV4) -> ServiceViewV4 {
        ServiceViewV4 {
            providers: BTreeMap::from([
                (
                    "first".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(upstream().base_url),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "second".to_string(),
                    ProviderConfigV4 {
                        base_url: Some("https://fallback.example/v1".to_string()),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(RoutingConfigV4 {
                entry: "root".to_string(),
                routes: BTreeMap::from([(
                    "root".to_string(),
                    RoutingNodeV4 {
                        strategy: RoutingPolicyV4::Conditional,
                        when: Some(condition),
                        then: Some("first".to_string()),
                        default_route: Some("second".to_string()),
                        ..RoutingNodeV4::default()
                    },
                )]),
                ..RoutingConfigV4::default()
            }),
            ..ServiceViewV4::default()
        }
    }

    #[test]
    fn catalog_mapping_uses_target_metadata_and_respects_exact_mapping_priority() {
        let raw = serde_json::json!({"models": [
            {"slug": "custom-x", "default_reasoning_level": "high",
             "supported_reasoning_levels": [{"effort": "high"}]},
            {"slug": "internal-y", "default_reasoning_level": "ultra",
             "supported_reasoning_levels": [{"effort": "ultra"}]},
            {"slug": "internal-shadowed", "supported_reasoning_levels": [{"effort": "low"}]}
        ]});
        let mut upstream = upstream();
        upstream.model_mapping = HashMap::from([
            ("custom-x".to_string(), "internal-y".to_string()),
            ("custom-*".to_string(), "internal-*".to_string()),
            ("custom-shadowed".to_string(), "internal-y".to_string()),
        ]);
        let projected = route_catalog(&raw, &upstream);
        let merged = merge_model_catalogs(&[projected]).expect("catalog");
        let models = merged["models"].as_array().expect("models");
        for alias in ["custom-x", "custom-shadowed"] {
            let model = models
                .iter()
                .find(|model| model["slug"] == alias)
                .expect("alias");
            assert_eq!(model["default_reasoning_level"], "ultra");
            assert_eq!(
                model["supported_reasoning_levels"]
                    .as_array()
                    .expect("efforts")
                    .len(),
                1
            );
        }
    }

    #[test]
    fn catalog_projection_keeps_websocket_and_priority_only_branches() {
        let view = conditional_view(RoutingConditionV4 {
            method: Some("GET".to_string()),
            path: Some("/backend-api/codex/responses".to_string()),
            service_tier: Some("priority".to_string()),
            ..RoutingConditionV4::default()
        });
        let raw = serde_json::json!({"data": [{"id": "websocket-model"}]});
        let mut projected = route_catalog(&raw, &upstream());
        retain_routable_models(&mut projected, &raw, &upstream(), &view, &HeaderMap::new());
        assert_eq!(projected["data"].as_array().expect("models").len(), 1);
    }

    #[test]
    fn catalog_conditional_efforts_and_default_remain_routable() {
        let view = conditional_view(RoutingConditionV4 {
            reasoning_effort: Some("high".to_string()),
            ..RoutingConditionV4::default()
        });
        let raw = serde_json::json!({"models": [{"slug": "custom-sol",
        "default_reasoning_level": "ultra", "supported_reasoning_levels": [
            {"effort": "high"}, {"effort": "ultra"}
        ]}]});
        let mut projected = route_catalog(&raw, &upstream());
        retain_routable_models(&mut projected, &raw, &upstream(), &view, &HeaderMap::new());
        let mut merged = merge_model_catalogs(&[projected]).expect("catalog");
        constrain_conditional_efforts(
            &mut merged,
            &[(0, upstream(), raw)],
            &view,
            &HeaderMap::new(),
        );
        let model = &merged["models"][0];
        assert_eq!(model["default_reasoning_level"], "high");
        assert_eq!(
            model["supported_reasoning_levels"]
                .as_array()
                .expect("efforts")
                .len(),
            1
        );
        assert_eq!(model["supported_reasoning_levels"][0]["effort"], "high");
    }

    #[test]
    fn catalog_default_effort_prefers_the_default_tier_over_priority_only_support() {
        let view = conditional_view(RoutingConditionV4 {
            service_tier: Some("priority".to_string()),
            ..RoutingConditionV4::default()
        });
        let first = serde_json::json!({"models": [{"slug": "custom-sol", "default_reasoning_level": "ultra",
            "supported_reasoning_levels": [{"effort": "ultra"}]}]});
        let second = serde_json::json!({"models": [{"slug": "custom-sol", "default_reasoning_level": "high",
            "supported_reasoning_levels": [{"effort": "high"}]}]});
        let mut merged = merge_model_catalogs(&[first.clone(), second.clone()]).expect("catalog");
        let mut fallback = upstream();
        fallback.base_url = "https://fallback.example/v1".to_string();
        fallback
            .tags
            .insert("provider_id".to_string(), "second".to_string());
        constrain_conditional_efforts(
            &mut merged,
            &[(0, upstream(), first), (1, fallback, second)],
            &view,
            &HeaderMap::new(),
        );
        let model = &merged["models"][0];
        assert_eq!(
            model["supported_reasoning_levels"]
                .as_array()
                .expect("efforts")
                .len(),
            2
        );
        assert_eq!(model["default_reasoning_level"], "high");
    }

    #[test]
    fn catalog_has_no_default_effort_when_only_priority_support_is_reachable() {
        let view = conditional_view(RoutingConditionV4 {
            service_tier: Some("priority".to_string()),
            ..RoutingConditionV4::default()
        });
        let raw = serde_json::json!({"models": [{"slug": "priority-sol", "default_reasoning_level": "ultra",
            "supported_reasoning_levels": [{"effort": "ultra"}]}]});
        let mut merged = merge_model_catalogs(std::slice::from_ref(&raw)).expect("catalog");
        constrain_conditional_efforts(
            &mut merged,
            &[(0, upstream(), raw)],
            &view,
            &HeaderMap::new(),
        );
        assert_eq!(
            merged["models"][0]["supported_reasoning_levels"][0]["effort"],
            "ultra"
        );
        assert_eq!(merged["models"][0]["default_reasoning_level"], Value::Null);
    }
}

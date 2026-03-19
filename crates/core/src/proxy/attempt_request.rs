use std::sync::OnceLock;

use axum::http::{HeaderMap, HeaderName, HeaderValue};

use crate::config::UpstreamAuth;
use crate::logging::{BodyPreview, HeaderEntry};

use super::ProxyService;
use super::auth_resolution::{resolve_api_key_with_source, resolve_auth_token_with_source};
use super::headers::{filter_request_headers, header_map_to_entries};
use super::http_debug::HttpDebugBase;

pub(super) struct AttemptRequestSetup {
    pub(super) headers: HeaderMap,
    pub(super) debug_base: Option<HttpDebugBase>,
}

struct HttpDebugBaseParams<'a> {
    client_headers: &'a HeaderMap,
    client_headers_entries_cache: &'a OnceLock<Vec<HeaderEntry>>,
    upstream_request_headers: &'a HeaderMap,
    request_body_len: usize,
    upstream_request_body_len: usize,
    debug_max: usize,
    warn_max: usize,
    client_uri: &'a str,
    target_url: &'a str,
    client_body_debug: Option<&'a BodyPreview>,
    upstream_request_body_debug: Option<&'a BodyPreview>,
    client_body_warn: Option<&'a BodyPreview>,
    upstream_request_body_warn: Option<&'a BodyPreview>,
}

pub(super) struct AttemptRequestSetupParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) auth: &'a UpstreamAuth,
    pub(super) client_headers: &'a HeaderMap,
    pub(super) client_headers_entries_cache: &'a OnceLock<Vec<HeaderEntry>>,
    pub(super) request_body_len: usize,
    pub(super) upstream_request_body_len: usize,
    pub(super) debug_max: usize,
    pub(super) warn_max: usize,
    pub(super) client_uri: &'a str,
    pub(super) target_url: &'a str,
    pub(super) client_body_debug: Option<&'a BodyPreview>,
    pub(super) upstream_request_body_debug: Option<&'a BodyPreview>,
    pub(super) client_body_warn: Option<&'a BodyPreview>,
    pub(super) upstream_request_body_warn: Option<&'a BodyPreview>,
}

pub(super) fn prepare_attempt_request(
    params: AttemptRequestSetupParams<'_>,
) -> AttemptRequestSetup {
    let AttemptRequestSetupParams {
        proxy,
        auth,
        client_headers,
        client_headers_entries_cache,
        request_body_len,
        upstream_request_body_len,
        debug_max,
        warn_max,
        client_uri,
        target_url,
        client_body_debug,
        upstream_request_body_debug,
        client_body_warn,
        upstream_request_body_warn,
    } = params;

    // 复制客户端请求头，并按上游配置覆盖认证头；未提供上游凭证时保留客户端值。
    let mut headers = filter_request_headers(client_headers);
    inject_auth_headers(proxy.service_name, auth, &mut headers);

    let debug_base = build_http_debug_base(HttpDebugBaseParams {
        client_headers,
        client_headers_entries_cache,
        upstream_request_headers: &headers,
        request_body_len,
        upstream_request_body_len,
        debug_max,
        warn_max,
        client_uri,
        target_url,
        client_body_debug,
        upstream_request_body_debug,
        client_body_warn,
        upstream_request_body_warn,
    });

    AttemptRequestSetup {
        headers,
        debug_base,
    }
}

fn inject_auth_headers(service_name: &str, auth: &UpstreamAuth, headers: &mut HeaderMap) {
    let client_has_auth = headers.contains_key("authorization");
    let (token, _token_src) = resolve_auth_token_with_source(service_name, auth, client_has_auth);
    if let Some(token) = token
        && let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}"))
    {
        headers.insert(HeaderName::from_static("authorization"), value);
    }

    let client_has_x_api_key = headers.contains_key("x-api-key");
    let (api_key, _api_key_src) =
        resolve_api_key_with_source(service_name, auth, client_has_x_api_key);
    if let Some(key) = api_key
        && let Ok(value) = HeaderValue::from_str(&key)
    {
        headers.insert(HeaderName::from_static("x-api-key"), value);
    }
}

fn build_http_debug_base(params: HttpDebugBaseParams<'_>) -> Option<HttpDebugBase> {
    let HttpDebugBaseParams {
        client_headers,
        client_headers_entries_cache,
        upstream_request_headers,
        request_body_len,
        upstream_request_body_len,
        debug_max,
        warn_max,
        client_uri,
        target_url,
        client_body_debug,
        upstream_request_body_debug,
        client_body_warn,
        upstream_request_body_warn,
    } = params;

    if debug_max == 0 && warn_max == 0 {
        return None;
    }

    Some(HttpDebugBase {
        debug_max_body_bytes: debug_max,
        warn_max_body_bytes: warn_max,
        request_body_len,
        upstream_request_body_len,
        client_uri: client_uri.to_string(),
        target_url: target_url.to_string(),
        client_headers: client_headers_entries_cache
            .get_or_init(|| header_map_to_entries(client_headers))
            .clone(),
        upstream_request_headers: header_map_to_entries(upstream_request_headers),
        auth_resolution: None,
        client_body_debug: client_body_debug.cloned(),
        upstream_request_body_debug: upstream_request_body_debug.cloned(),
        client_body_warn: client_body_warn.cloned(),
        upstream_request_body_warn: upstream_request_body_warn.cloned(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::http::HeaderValue;

    use super::*;
    use crate::config::ProxyConfig;
    use crate::lb::LbState;

    fn test_proxy_service() -> ProxyService {
        ProxyService::new(
            reqwest::Client::new(),
            Arc::new(ProxyConfig::default()),
            "codex",
            Arc::new(Mutex::new(
                std::collections::HashMap::<String, LbState>::new(),
            )),
        )
    }

    #[tokio::test]
    async fn prepare_attempt_request_overrides_auth_headers_from_upstream_auth() {
        let proxy = test_proxy_service();
        let mut client_headers = HeaderMap::new();
        client_headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer client-token"),
        );
        client_headers.insert("x-api-key", HeaderValue::from_static("client-key"));
        client_headers.insert("content-type", HeaderValue::from_static("application/json"));

        let cache = OnceLock::new();
        let setup = prepare_attempt_request(AttemptRequestSetupParams {
            proxy: &proxy,
            auth: &UpstreamAuth {
                auth_token: Some("server-token".to_string()),
                auth_token_env: None,
                api_key: Some("server-key".to_string()),
                api_key_env: None,
            },
            client_headers: &client_headers,
            client_headers_entries_cache: &cache,
            request_body_len: 12,
            upstream_request_body_len: 12,
            debug_max: 0,
            warn_max: 0,
            client_uri: "/v1/responses",
            target_url: "https://example.com/v1/responses",
            client_body_debug: None,
            upstream_request_body_debug: None,
            client_body_warn: None,
            upstream_request_body_warn: None,
        });

        assert_eq!(
            setup.headers.get("authorization"),
            Some(&HeaderValue::from_static("Bearer server-token"))
        );
        assert_eq!(
            setup.headers.get("x-api-key"),
            Some(&HeaderValue::from_static("server-key"))
        );
        assert!(setup.debug_base.is_none());
    }

    #[tokio::test]
    async fn prepare_attempt_request_builds_debug_base_when_limits_enabled() {
        let proxy = test_proxy_service();
        let mut client_headers = HeaderMap::new();
        client_headers.insert("content-type", HeaderValue::from_static("application/json"));

        let cache = OnceLock::new();
        let setup = prepare_attempt_request(AttemptRequestSetupParams {
            proxy: &proxy,
            auth: &UpstreamAuth::default(),
            client_headers: &client_headers,
            client_headers_entries_cache: &cache,
            request_body_len: 18,
            upstream_request_body_len: 22,
            debug_max: 128,
            warn_max: 64,
            client_uri: "/v1/responses",
            target_url: "https://example.com/v1/responses",
            client_body_debug: None,
            upstream_request_body_debug: None,
            client_body_warn: None,
            upstream_request_body_warn: None,
        });

        let debug_base = setup.debug_base.expect("debug_base");
        assert_eq!(debug_base.client_uri, "/v1/responses");
        assert_eq!(debug_base.target_url, "https://example.com/v1/responses");
        assert_eq!(debug_base.request_body_len, 18);
        assert_eq!(debug_base.upstream_request_body_len, 22);
        assert!(!debug_base.client_headers.is_empty());
    }
}

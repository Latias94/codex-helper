use anyhow::{Result, anyhow};
use axum::http::{HeaderMap, Uri};

use crate::config::UpstreamConfig;

use super::ProxyService;
use super::attempt_target::AttemptTarget;

impl ProxyService {
    pub(super) fn build_target(
        &self,
        target: &AttemptTarget,
        uri: &Uri,
    ) -> Result<(reqwest::Url, HeaderMap)> {
        build_target_impl(target.upstream(), uri)
    }
}

fn build_target_impl(upstream: &UpstreamConfig, uri: &Uri) -> Result<(reqwest::Url, HeaderMap)> {
    Ok((
        upstream_request_url(&upstream.base_url, uri)?,
        HeaderMap::new(),
    ))
}

pub(super) fn upstream_request_url(base: &str, uri: &Uri) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(base).map_err(|_| anyhow!("invalid upstream base URL"))?;
    let base_path = url.path().trim_end_matches('/').to_string();

    let mut path = uri.path().to_string();
    if !base_path.is_empty()
        && base_path != "/"
        && (path == base_path || path.starts_with(&format!("{base_path}/")))
    {
        // If the incoming request path already contains the base_url path prefix,
        // strip it to avoid double-prefixing (e.g. base_url=/v1 and request=/v1/responses).
        let rest = &path[base_path.len()..];
        path = if rest.is_empty() {
            "/".to_string()
        } else {
            rest.to_string()
        };
        if !path.starts_with('/') {
            path = format!("/{path}");
        }
    }
    url.set_path(&format!("{base_path}{path}"));
    let query = match (url.query(), uri.query()) {
        (Some(base_query), Some(query)) => Some(format!("{base_query}&{query}")),
        (Some(query), None) | (None, Some(query)) => Some(query.to_string()),
        (None, None) => None,
    };
    url.set_query(query.as_deref());
    url.set_fragment(None);
    Ok(url)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::config::{UpstreamAuth, UpstreamConfig};

    use super::*;

    fn upstream(base_url: &str) -> UpstreamConfig {
        UpstreamConfig {
            base_url: base_url.to_string(),
            auth: UpstreamAuth::default(),
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }
    }

    #[test]
    fn build_target_strips_duplicate_base_path_prefix() {
        let upstream = upstream("https://api.example.com/v1");
        let uri: Uri = "/v1/responses".parse().expect("uri");

        let (url, headers) = build_target_impl(&upstream, &uri).expect("target");

        assert_eq!(url.as_str(), "https://api.example.com/v1/responses");
        assert!(headers.is_empty());
    }

    #[test]
    fn build_target_preserves_query_string() {
        let upstream = upstream("https://api.example.com/v1/");
        let uri: Uri = "/responses?stream=true".parse().expect("uri");

        let (url, _) = build_target_impl(&upstream, &uri).expect("target");

        assert_eq!(
            url.as_str(),
            "https://api.example.com/v1/responses?stream=true"
        );
    }

    #[test]
    fn build_target_does_not_strip_partial_prefix_match() {
        let upstream = upstream("https://api.example.com/v1");
        let uri: Uri = "/v12/responses".parse().expect("uri");

        let (url, _) = build_target_impl(&upstream, &uri).expect("target");

        assert_eq!(url.as_str(), "https://api.example.com/v1/v12/responses");
    }

    #[test]
    fn build_target_joins_base_and_request_queries_without_using_fragment() {
        let upstream =
            upstream("https://api.example.com/backend-api/codex/?tenant=fixture#ignored");
        let uri: Uri = "/models?client_version=0.162.1".parse().expect("uri");
        let (url, _) = build_target_impl(&upstream, &uri).expect("target");
        assert_eq!(
            url.as_str(),
            "https://api.example.com/backend-api/codex/models?tenant=fixture&client_version=0.162.1"
        );
    }
}

use anyhow::{Result, anyhow};
use axum::http::{HeaderMap, Uri};

use crate::lb::SelectedUpstream;

use super::ProxyService;

impl ProxyService {
    pub(super) fn build_target(
        &self,
        upstream: &SelectedUpstream,
        uri: &Uri,
    ) -> Result<(reqwest::Url, HeaderMap)> {
        build_target_impl(upstream, uri)
    }
}

fn build_target_impl(upstream: &SelectedUpstream, uri: &Uri) -> Result<(reqwest::Url, HeaderMap)> {
    let base = upstream.upstream.base_url.trim_end_matches('/').to_string();

    let base_url =
        reqwest::Url::parse(&base).map_err(|e| anyhow!("invalid upstream base_url {base}: {e}"))?;
    let base_path = base_url.path().trim_end_matches('/').to_string();

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
    let path_and_query = if let Some(q) = uri.query() {
        format!("{path}?{q}")
    } else {
        path
    };

    let full = format!("{base}{path_and_query}");
    let url =
        reqwest::Url::parse(&full).map_err(|e| anyhow!("invalid upstream url {full}: {e}"))?;

    // ensure query preserved (Url::parse already includes it)
    let headers = HeaderMap::new();
    Ok((url, headers))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::config::{UpstreamAuth, UpstreamConfig};

    use super::*;

    fn selected_upstream(base_url: &str) -> SelectedUpstream {
        SelectedUpstream {
            station_name: "test".to_string(),
            index: 0,
            upstream: UpstreamConfig {
                base_url: base_url.to_string(),
                auth: UpstreamAuth::default(),
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        }
    }

    #[test]
    fn build_target_strips_duplicate_base_path_prefix() {
        let upstream = selected_upstream("https://api.example.com/v1");
        let uri: Uri = "/v1/responses".parse().expect("uri");

        let (url, headers) = build_target_impl(&upstream, &uri).expect("target");

        assert_eq!(url.as_str(), "https://api.example.com/v1/responses");
        assert!(headers.is_empty());
    }

    #[test]
    fn build_target_preserves_query_string() {
        let upstream = selected_upstream("https://api.example.com/v1/");
        let uri: Uri = "/responses?stream=true".parse().expect("uri");

        let (url, _) = build_target_impl(&upstream, &uri).expect("target");

        assert_eq!(
            url.as_str(),
            "https://api.example.com/v1/responses?stream=true"
        );
    }

    #[test]
    fn build_target_does_not_strip_partial_prefix_match() {
        let upstream = selected_upstream("https://api.example.com/v1");
        let uri: Uri = "/v12/responses".parse().expect("uri");

        let (url, _) = build_target_impl(&upstream, &uri).expect("target");

        assert_eq!(url.as_str(), "https://api.example.com/v1/v12/responses");
    }
}

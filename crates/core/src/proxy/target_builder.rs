use anyhow::{Result, anyhow};
use axum::http::Uri;

use super::ProxyService;
use crate::routing_ir::CapturedRouteCandidate;

impl ProxyService {
    pub(super) fn build_target(
        &self,
        target: &CapturedRouteCandidate,
        uri: &Uri,
    ) -> Result<reqwest::Url> {
        build_target_impl(target.base_url(), uri)
    }
}

fn build_target_impl(base_url: &str, uri: &Uri) -> Result<reqwest::Url> {
    let base = base_url.trim_end_matches('/').to_string();

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
    reqwest::Url::parse(&full).map_err(|e| anyhow!("invalid upstream url {full}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_target_strips_duplicate_base_path_prefix() {
        let uri: Uri = "/v1/responses".parse().expect("uri");

        let url = build_target_impl("https://api.example.com/v1", &uri).expect("target");

        assert_eq!(url.as_str(), "https://api.example.com/v1/responses");
    }

    #[test]
    fn build_target_preserves_query_string() {
        let uri: Uri = "/responses?stream=true".parse().expect("uri");

        let url = build_target_impl("https://api.example.com/v1/", &uri).expect("target");

        assert_eq!(
            url.as_str(),
            "https://api.example.com/v1/responses?stream=true"
        );
    }

    #[test]
    fn build_target_does_not_strip_partial_prefix_match() {
        let uri: Uri = "/v12/responses".parse().expect("uri");

        let url = build_target_impl("https://api.example.com/v1", &uri).expect("target");

        assert_eq!(url.as_str(), "https://api.example.com/v1/v12/responses");
    }
}

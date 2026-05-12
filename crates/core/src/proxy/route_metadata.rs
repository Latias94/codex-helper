use crate::lb::SelectedUpstream;

pub(super) const ENDPOINT_ID_TAG: &str = "endpoint_id";
pub(super) const PROVIDER_ID_TAG: &str = "provider_id";
pub(super) const ROUTE_PATH_TAG: &str = "route_path";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SelectedRouteMetadata {
    pub(super) provider_id: Option<String>,
    pub(super) endpoint_id: Option<String>,
    pub(super) route_path: Vec<String>,
}

pub(super) fn selected_route_metadata(selected: &SelectedUpstream) -> SelectedRouteMetadata {
    let provider_id = selected_tag(selected, PROVIDER_ID_TAG);
    let endpoint_id =
        selected_tag(selected, ENDPOINT_ID_TAG).or_else(|| Some(selected.index.to_string()));
    let route_path = selected
        .upstream
        .tags
        .get(ROUTE_PATH_TAG)
        .and_then(|value| parse_route_path_tag(value))
        .unwrap_or_else(|| {
            vec![
                "legacy".to_string(),
                selected.station_name.clone(),
                provider_id
                    .clone()
                    .unwrap_or_else(|| format!("{}#{}", selected.station_name, selected.index)),
            ]
        });

    SelectedRouteMetadata {
        provider_id,
        endpoint_id,
        route_path,
    }
}

fn selected_tag(selected: &SelectedUpstream, key: &str) -> Option<String> {
    selected
        .upstream
        .tags
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty() && *value != "-")
        .map(ToOwned::to_owned)
}

fn parse_route_path_tag(value: &str) -> Option<Vec<String>> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    if value.starts_with('[')
        && let Ok(path) = serde_json::from_str::<Vec<String>>(value)
    {
        return non_empty_route_path(path);
    }

    non_empty_route_path(
        value
            .split('/')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
    )
}

fn non_empty_route_path(path: Vec<String>) -> Option<Vec<String>> {
    let path = path
        .into_iter()
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    (!path.is_empty()).then_some(path)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::config::{UpstreamAuth, UpstreamConfig};

    use super::*;

    fn selected(tags: HashMap<String, String>) -> SelectedUpstream {
        SelectedUpstream {
            station_name: "alpha".to_string(),
            index: 2,
            upstream: UpstreamConfig {
                base_url: "https://alpha.example/v1".to_string(),
                auth: UpstreamAuth::default(),
                tags,
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        }
    }

    #[test]
    fn route_metadata_uses_explicit_endpoint_and_json_route_path() {
        let metadata = selected_route_metadata(&selected(HashMap::from([
            (PROVIDER_ID_TAG.to_string(), "main".to_string()),
            (ENDPOINT_ID_TAG.to_string(), "fast".to_string()),
            (
                ROUTE_PATH_TAG.to_string(),
                r#"["root","preferred","main"]"#.to_string(),
            ),
        ])));

        assert_eq!(metadata.provider_id.as_deref(), Some("main"));
        assert_eq!(metadata.endpoint_id.as_deref(), Some("fast"));
        assert_eq!(metadata.route_path, vec!["root", "preferred", "main"]);
    }

    #[test]
    fn route_metadata_derives_legacy_path_when_tags_are_absent() {
        let metadata = selected_route_metadata(&selected(HashMap::new()));

        assert_eq!(metadata.provider_id, None);
        assert_eq!(metadata.endpoint_id.as_deref(), Some("2"));
        assert_eq!(metadata.route_path, vec!["legacy", "alpha", "alpha#2"]);
    }
}

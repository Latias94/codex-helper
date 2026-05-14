pub(super) const ENDPOINT_ID_TAG: &str = "endpoint_id";
pub(super) const PREFERENCE_GROUP_TAG: &str = "preference_group";
pub(super) const PROVIDER_ID_TAG: &str = "provider_id";
pub(super) const PROVIDER_ENDPOINT_KEY_TAG: &str = "provider_endpoint_key";
pub(super) const ROUTE_PATH_TAG: &str = "route_path";

pub(super) fn parse_route_path_tag(value: &str) -> Option<Vec<String>> {
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
    use super::*;

    #[test]
    fn route_path_parser_accepts_json_path() {
        assert_eq!(
            parse_route_path_tag(r#"["root","preferred","main"]"#),
            Some(vec![
                "root".to_string(),
                "preferred".to_string(),
                "main".to_string()
            ])
        );
    }

    #[test]
    fn route_path_parser_accepts_slash_path() {
        assert_eq!(
            parse_route_path_tag("root / preferred / main"),
            Some(vec![
                "root".to_string(),
                "preferred".to_string(),
                "main".to_string()
            ])
        );
    }
}

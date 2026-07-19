use axum::http::{HeaderMap, HeaderName};

use crate::codex_switch::{CODEX_CLIENT_FACADE_ACTOR_HEADER, CODEX_CLIENT_FACADE_ACTOR_VALUE};
use crate::config::CODEX_CLIENT_RUNTIME_PATCH_HEADER;
use crate::logging::HeaderEntry;

fn is_hop_by_hop_header(name_lower: &str) -> bool {
    matches!(
        name_lower,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn is_request_header_to_strip(name_lower: &str) -> bool {
    matches!(
        name_lower,
        "host"
            | "content-length"
            | "user-agent"
            | "cookie"
            | CODEX_CLIENT_RUNTIME_PATCH_HEADER
            | "x-forwarded-api-key"
            | "x-codex-helper-admin-token"
    ) || is_hop_by_hop_header(name_lower)
}

fn hop_by_hop_connection_tokens(headers: &HeaderMap) -> Vec<String> {
    let mut out = Vec::new();
    for value in headers.get_all("connection").iter() {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for token in value
            .split(',')
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            out.push(token.to_ascii_lowercase());
        }
    }
    out
}

pub(super) fn filter_request_headers(src: &HeaderMap) -> HeaderMap {
    let extra = hop_by_hop_connection_tokens(src);
    let mut out = HeaderMap::new();
    for (name, value) in src.iter() {
        let name_lower = name.as_str().to_ascii_lowercase();
        if is_request_header_to_strip(&name_lower) {
            continue;
        }
        if extra.iter().any(|token| token == &name_lower) {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

pub(super) fn strip_codex_client_facade_marker(headers: &mut HeaderMap) {
    let retained = headers
        .get_all(CODEX_CLIENT_FACADE_ACTOR_HEADER)
        .iter()
        .filter(|value| value.as_bytes() != CODEX_CLIENT_FACADE_ACTOR_VALUE.as_bytes())
        .cloned()
        .collect::<Vec<_>>();
    if retained.len()
        == headers
            .get_all(CODEX_CLIENT_FACADE_ACTOR_HEADER)
            .iter()
            .count()
    {
        return;
    }

    headers.remove(CODEX_CLIENT_FACADE_ACTOR_HEADER);
    for value in retained {
        headers.append(
            HeaderName::from_static(CODEX_CLIENT_FACADE_ACTOR_HEADER),
            value,
        );
    }
}

pub(super) fn filter_response_headers(src: &HeaderMap) -> HeaderMap {
    let extra = hop_by_hop_connection_tokens(src);
    let mut out = HeaderMap::new();
    for (name, value) in src.iter() {
        let name_lower = name.as_str().to_ascii_lowercase();
        if is_hop_by_hop_header(&name_lower)
            || name_lower == "content-length"
            || name_lower == "set-cookie"
        {
            continue;
        }
        if extra.iter().any(|token| token == &name_lower) {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

pub(super) fn header_map_to_entries(headers: &HeaderMap) -> Vec<HeaderEntry> {
    fn is_sensitive(name_lower: &str) -> bool {
        matches!(
            name_lower,
            "authorization"
                | "proxy-authorization"
                | "cookie"
                | "set-cookie"
                | "x-api-key"
                | "x-codex-helper-admin-token"
                | "x-forwarded-api-key"
                | "x-goog-api-key"
        )
    }

    let mut out = Vec::new();
    for (name, value) in headers.iter() {
        let name_lower = name.as_str().to_ascii_lowercase();
        let value = if is_sensitive(name_lower.as_str())
            || name_lower == CODEX_CLIENT_FACADE_ACTOR_HEADER
        {
            "[REDACTED]".to_string()
        } else {
            String::from_utf8_lossy(value.as_bytes()).into_owned()
        };
        out.push(HeaderEntry {
            name: name.as_str().to_string(),
            value,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use crate::codex_switch::{CODEX_CLIENT_FACADE_ACTOR_HEADER, CODEX_CLIENT_FACADE_ACTOR_VALUE};
    use crate::config::CODEX_CLIENT_RUNTIME_PATCH_HEADER;

    use super::{
        filter_request_headers, filter_response_headers, header_map_to_entries,
        strip_codex_client_facade_marker,
    };

    #[test]
    fn request_header_filter_removes_hop_by_hop_and_connection_targets() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("example.com"));
        headers.insert("content-length", HeaderValue::from_static("123"));
        headers.insert("user-agent", HeaderValue::from_static("Python-urllib/3.13"));
        headers.insert(
            "connection",
            HeaderValue::from_static("keep-alive, x-remove-me"),
        );
        headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        headers.insert("x-remove-me", HeaderValue::from_static("drop"));
        headers.insert("authorization", HeaderValue::from_static("Bearer secret"));
        headers.insert("cookie", HeaderValue::from_static("session=secret"));
        headers.insert(
            "x-forwarded-api-key",
            HeaderValue::from_static("forwarded-secret"),
        );
        headers.insert(
            "x-codex-helper-admin-token",
            HeaderValue::from_static("admin-secret"),
        );
        headers.insert(
            CODEX_CLIENT_RUNTIME_PATCH_HEADER,
            HeaderValue::from_static("v1;models=1;hosted=disabled"),
        );
        headers.insert("x-keep-me", HeaderValue::from_static("ok"));

        let filtered = filter_request_headers(&headers);

        assert!(!filtered.contains_key("host"));
        assert!(!filtered.contains_key("content-length"));
        assert!(!filtered.contains_key("user-agent"));
        assert!(!filtered.contains_key("connection"));
        assert!(!filtered.contains_key("keep-alive"));
        assert!(!filtered.contains_key("x-remove-me"));
        assert!(!filtered.contains_key("cookie"));
        assert!(!filtered.contains_key("x-forwarded-api-key"));
        assert!(!filtered.contains_key("x-codex-helper-admin-token"));
        assert!(!filtered.contains_key(CODEX_CLIENT_RUNTIME_PATCH_HEADER));
        assert_eq!(
            filtered.get("authorization"),
            Some(&HeaderValue::from_static("Bearer secret"))
        );
        assert_eq!(
            filtered.get("x-keep-me"),
            Some(&HeaderValue::from_static("ok"))
        );
    }

    #[test]
    fn response_header_filter_preserves_encoding_and_removes_framing_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("content-length", HeaderValue::from_static("321"));
        headers.insert("content-encoding", HeaderValue::from_static("gzip"));
        headers.insert("connection", HeaderValue::from_static("x-remove-me"));
        headers.insert("x-remove-me", HeaderValue::from_static("drop"));
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        headers.insert(
            "set-cookie",
            HeaderValue::from_static("session=upstream-secret"),
        );
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        let filtered = filter_response_headers(&headers);

        assert!(!filtered.contains_key("content-length"));
        assert_eq!(
            filtered.get("content-encoding"),
            Some(&HeaderValue::from_static("gzip"))
        );
        assert!(!filtered.contains_key("connection"));
        assert!(!filtered.contains_key("transfer-encoding"));
        assert!(!filtered.contains_key("x-remove-me"));
        assert!(!filtered.contains_key("set-cookie"));
        assert_eq!(
            filtered.get("content-type"),
            Some(&HeaderValue::from_static("application/json"))
        );
    }

    #[test]
    fn header_map_to_entries_redacts_sensitive_values() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer secret"));
        headers.insert("x-api-key", HeaderValue::from_static("secret-key"));
        headers.insert(
            "x-codex-helper-admin-token",
            HeaderValue::from_static("admin-secret"),
        );
        headers.insert(
            CODEX_CLIENT_FACADE_ACTOR_HEADER,
            HeaderValue::from_static("actor-secret"),
        );
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        let entries = header_map_to_entries(&headers);

        assert!(entries.iter().any(|entry| {
            entry.name.eq_ignore_ascii_case("authorization") && entry.value == "[REDACTED]"
        }));
        assert!(entries.iter().any(|entry| {
            entry
                .name
                .eq_ignore_ascii_case(CODEX_CLIENT_FACADE_ACTOR_HEADER)
                && entry.value == "[REDACTED]"
        }));
        assert!(entries.iter().any(|entry| {
            entry.name.eq_ignore_ascii_case("x-api-key") && entry.value == "[REDACTED]"
        }));
        assert!(entries.iter().any(|entry| {
            entry
                .name
                .eq_ignore_ascii_case("x-codex-helper-admin-token")
                && entry.value == "[REDACTED]"
        }));
        assert!(entries.iter().any(|entry| {
            entry.name.eq_ignore_ascii_case("content-type") && entry.value == "application/json"
        }));
    }

    #[test]
    fn client_facade_marker_is_removed_without_dropping_real_actor_authorization() {
        let mut headers = HeaderMap::new();
        headers.append(
            CODEX_CLIENT_FACADE_ACTOR_HEADER,
            HeaderValue::from_static(CODEX_CLIENT_FACADE_ACTOR_VALUE),
        );
        headers.append(
            CODEX_CLIENT_FACADE_ACTOR_HEADER,
            HeaderValue::from_static("real-actor-token"),
        );

        strip_codex_client_facade_marker(&mut headers);

        let values = headers
            .get_all(CODEX_CLIENT_FACADE_ACTOR_HEADER)
            .iter()
            .map(HeaderValue::as_bytes)
            .collect::<Vec<_>>();
        assert_eq!(values, vec![b"real-actor-token".as_slice()]);
    }
}

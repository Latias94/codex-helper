use axum::http::HeaderMap;

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
        if name_lower == "host"
            || name_lower == "content-length"
            || is_hop_by_hop_header(&name_lower)
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

pub(super) fn filter_response_headers(src: &HeaderMap) -> HeaderMap {
    let extra = hop_by_hop_connection_tokens(src);
    let mut out = HeaderMap::new();
    for (name, value) in src.iter() {
        let name_lower = name.as_str().to_ascii_lowercase();
        if is_hop_by_hop_header(&name_lower)
            || name_lower == "content-length"
            || name_lower == "content-encoding"
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
                | "x-forwarded-api-key"
                | "x-goog-api-key"
        )
    }

    let mut out = Vec::new();
    for (name, value) in headers.iter() {
        let name_lower = name.as_str().to_ascii_lowercase();
        let value = if is_sensitive(name_lower.as_str()) {
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

    use super::{filter_request_headers, filter_response_headers, header_map_to_entries};

    #[test]
    fn request_header_filter_removes_hop_by_hop_and_connection_targets() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("example.com"));
        headers.insert("content-length", HeaderValue::from_static("123"));
        headers.insert(
            "connection",
            HeaderValue::from_static("keep-alive, x-remove-me"),
        );
        headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        headers.insert("x-remove-me", HeaderValue::from_static("drop"));
        headers.insert("authorization", HeaderValue::from_static("Bearer secret"));
        headers.insert("x-keep-me", HeaderValue::from_static("ok"));

        let filtered = filter_request_headers(&headers);

        assert!(!filtered.contains_key("host"));
        assert!(!filtered.contains_key("content-length"));
        assert!(!filtered.contains_key("connection"));
        assert!(!filtered.contains_key("keep-alive"));
        assert!(!filtered.contains_key("x-remove-me"));
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
    fn response_header_filter_removes_encoding_length_and_connection_targets() {
        let mut headers = HeaderMap::new();
        headers.insert("content-length", HeaderValue::from_static("321"));
        headers.insert("content-encoding", HeaderValue::from_static("gzip"));
        headers.insert("connection", HeaderValue::from_static("x-remove-me"));
        headers.insert("x-remove-me", HeaderValue::from_static("drop"));
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        let filtered = filter_response_headers(&headers);

        assert!(!filtered.contains_key("content-length"));
        assert!(!filtered.contains_key("content-encoding"));
        assert!(!filtered.contains_key("connection"));
        assert!(!filtered.contains_key("transfer-encoding"));
        assert!(!filtered.contains_key("x-remove-me"));
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
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        let entries = header_map_to_entries(&headers);

        assert!(entries.iter().any(|entry| {
            entry.name.eq_ignore_ascii_case("authorization") && entry.value == "[REDACTED]"
        }));
        assert!(entries.iter().any(|entry| {
            entry.name.eq_ignore_ascii_case("x-api-key") && entry.value == "[REDACTED]"
        }));
        assert!(entries.iter().any(|entry| {
            entry.name.eq_ignore_ascii_case("content-type") && entry.value == "application/json"
        }));
    }
}

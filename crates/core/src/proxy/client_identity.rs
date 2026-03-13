use std::net::SocketAddr;

use axum::extract::ConnectInfo;
use axum::http::{Extensions, HeaderMap};

use super::CLIENT_NAME_HEADER;

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn normalize_client_identity_value(value: &str, max_chars: usize) -> Option<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut out = trimmed.to_string();
    if out.chars().count() > max_chars {
        out = out.chars().take(max_chars).collect::<String>();
    }
    Some(out)
}

pub(super) fn extract_session_id(headers: &HeaderMap) -> Option<String> {
    header_str(headers, "session_id")
        .or_else(|| header_str(headers, "conversation_id"))
        .map(str::to_owned)
}

pub(super) fn extract_client_name(headers: &HeaderMap) -> Option<String> {
    header_str(headers, CLIENT_NAME_HEADER)
        .and_then(|value| normalize_client_identity_value(value, 80))
        .or_else(|| {
            header_str(headers, "user-agent")
                .and_then(|value| normalize_client_identity_value(value, 120))
        })
}

pub(super) fn extract_client_addr(extensions: &Extensions) -> Option<String> {
    extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip().to_string())
        .and_then(|value| normalize_client_identity_value(value.as_str(), 64))
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use axum::extract::ConnectInfo;
    use axum::http::{Extensions, HeaderMap, HeaderValue};

    use super::{extract_client_addr, extract_client_name, extract_session_id};
    use crate::proxy::CLIENT_NAME_HEADER;

    #[test]
    fn extract_session_id_prefers_session_id_then_conversation_id() {
        let mut headers = HeaderMap::new();
        headers.insert("conversation_id", HeaderValue::from_static("conv-1"));
        assert_eq!(extract_session_id(&headers).as_deref(), Some("conv-1"));

        headers.insert("session_id", HeaderValue::from_static("sess-1"));
        assert_eq!(extract_session_id(&headers).as_deref(), Some("sess-1"));
    }

    #[test]
    fn extract_client_name_prefers_custom_header_and_normalizes_whitespace() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static("Agent/1.0"));
        headers.insert(
            CLIENT_NAME_HEADER,
            HeaderValue::from_static("  Frank   Desk  "),
        );

        assert_eq!(extract_client_name(&headers).as_deref(), Some("Frank Desk"));
    }

    #[test]
    fn extract_client_name_falls_back_to_user_agent_and_truncates() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "user-agent",
            HeaderValue::from_str(&"A".repeat(140)).expect("header"),
        );

        let name = extract_client_name(&headers).expect("client name");
        assert_eq!(name.chars().count(), 120);
        assert!(name.chars().all(|ch| ch == 'A'));
    }

    #[test]
    fn extract_client_addr_reads_connect_info() {
        let mut extensions = Extensions::new();
        extensions.insert(ConnectInfo(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            8080,
        )));

        assert_eq!(
            extract_client_addr(&extensions).as_deref(),
            Some("127.0.0.1")
        );
    }
}

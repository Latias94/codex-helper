use std::net::SocketAddr;

use axum::extract::ConnectInfo;
use axum::http::{Extensions, HeaderMap};
use serde_json::Value;

use crate::state::SessionIdentitySource;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClientSessionIdentity {
    value: String,
    source: SessionIdentitySource,
}

impl ClientSessionIdentity {
    fn header(value: &str) -> Option<Self> {
        let value = value.trim();
        (!value.is_empty()).then(|| Self {
            value: value.to_string(),
            source: SessionIdentitySource::Header,
        })
    }

    fn prompt_cache_key(value: String) -> Self {
        Self {
            value,
            source: SessionIdentitySource::PromptCacheKey,
        }
    }

    fn body_session_id(value: String) -> Self {
        Self {
            value,
            source: SessionIdentitySource::BodySessionId,
        }
    }

    fn metadata_session_id(value: String) -> Self {
        Self {
            value,
            source: SessionIdentitySource::MetadataSessionId,
        }
    }

    pub(super) fn value(&self) -> &str {
        self.value.as_str()
    }

    pub(super) fn source(&self) -> SessionIdentitySource {
        self.source
    }
}

pub(super) fn extract_session_identity(headers: &HeaderMap) -> Option<ClientSessionIdentity> {
    [
        "session_id",
        "x-session-id",
        "session-id",
        "conversation_id",
        "thread-id",
    ]
    .into_iter()
    .find_map(|name| header_str(headers, name).and_then(ClientSessionIdentity::header))
}

pub(super) fn extract_session_identity_with_body_fallback(
    headers: &HeaderMap,
    body: &[u8],
) -> Option<ClientSessionIdentity> {
    extract_session_identity(headers).or_else(|| extract_body_session_identity(body))
}

pub(super) fn extract_prompt_cache_key_session_identity(
    body: &[u8],
) -> Option<ClientSessionIdentity> {
    extract_prompt_cache_key_session_id(body).map(ClientSessionIdentity::prompt_cache_key)
}

pub(super) fn extract_prompt_cache_key_session_id(body: &[u8]) -> Option<String> {
    extract_body_string_field(body, &["prompt_cache_key"])
}

fn extract_body_session_identity(body: &[u8]) -> Option<ClientSessionIdentity> {
    extract_body_string_field(body, &["session_id"])
        .map(ClientSessionIdentity::body_session_id)
        .or_else(|| {
            extract_body_string_field(body, &["x-session-id"])
                .map(ClientSessionIdentity::body_session_id)
        })
        .or_else(|| extract_prompt_cache_key_session_identity(body))
        .or_else(|| {
            extract_body_string_field(body, &["metadata", "session_id"])
                .map(ClientSessionIdentity::metadata_session_id)
        })
}

fn extract_body_string_field(body: &[u8], path: &[&str]) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    let value = serde_json::from_slice::<Value>(body).ok()?;
    let mut current = &value;
    for key in path {
        current = current.get(*key)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
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

    use super::{
        extract_client_addr, extract_client_name, extract_session_identity,
        extract_session_identity_with_body_fallback,
    };
    use crate::proxy::CLIENT_NAME_HEADER;
    use crate::state::SessionIdentitySource;

    #[test]
    fn extract_session_id_prefers_session_id_then_conversation_id() {
        let mut headers = HeaderMap::new();
        headers.insert("conversation_id", HeaderValue::from_static("conv-1"));
        assert_eq!(
            extract_session_identity(&headers)
                .as_ref()
                .map(|identity| identity.value()),
            Some("conv-1")
        );

        headers.insert("session_id", HeaderValue::from_static("sess-1"));
        assert_eq!(
            extract_session_identity(&headers)
                .as_ref()
                .map(|identity| identity.value()),
            Some("sess-1")
        );
    }

    #[test]
    fn extract_session_id_accepts_official_codex_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("thread-id", HeaderValue::from_static("thread-1"));
        assert_eq!(
            extract_session_identity(&headers)
                .as_ref()
                .map(|identity| identity.value()),
            Some("thread-1")
        );

        headers.insert("conversation_id", HeaderValue::from_static("conv-1"));
        assert_eq!(
            extract_session_identity(&headers)
                .as_ref()
                .map(|identity| identity.value()),
            Some("conv-1")
        );

        headers.insert("session-id", HeaderValue::from_static("sess-hyphen-1"));
        assert_eq!(
            extract_session_identity(&headers)
                .as_ref()
                .map(|identity| identity.value()),
            Some("sess-hyphen-1")
        );

        headers.insert("session_id", HeaderValue::from_static("sess-underscore-1"));
        assert_eq!(
            extract_session_identity(&headers)
                .as_ref()
                .map(|identity| identity.value()),
            Some("sess-underscore-1")
        );

        headers.insert("x-session-id", HeaderValue::from_static("x-sess-1"));
        headers.remove("session_id");
        headers.remove("session-id");
        headers.remove("conversation_id");
        headers.remove("thread-id");
        assert_eq!(
            extract_session_identity(&headers)
                .as_ref()
                .map(|identity| identity.value()),
            Some("x-sess-1")
        );
    }

    #[test]
    fn extract_session_id_uses_prompt_cache_key_body_fallback() {
        let headers = HeaderMap::new();

        let identity = extract_session_identity_with_body_fallback(
            &headers,
            br#"{"model":"gpt-5","prompt_cache_key":"pcache-1"}"#,
        )
        .expect("identity");
        assert_eq!(identity.value(), "pcache-1");
        assert_eq!(identity.source(), SessionIdentitySource::PromptCacheKey);
    }

    #[test]
    fn extract_session_id_prefers_headers_over_prompt_cache_key() {
        let mut headers = HeaderMap::new();
        headers.insert("session_id", HeaderValue::from_static("sid-header"));

        let identity = extract_session_identity_with_body_fallback(
            &headers,
            br#"{"model":"gpt-5","prompt_cache_key":"pcache-1"}"#,
        )
        .expect("identity");
        assert_eq!(identity.value(), "sid-header");
        assert_eq!(identity.source(), SessionIdentitySource::Header);
        assert_eq!(
            extract_session_identity(&headers).map(|identity| identity.source()),
            Some(SessionIdentitySource::Header)
        );
    }

    #[test]
    fn extract_session_id_trims_headers_and_ignores_blank_values() {
        let mut headers = HeaderMap::new();
        headers.insert("session_id", HeaderValue::from_static("  sid-trimmed  "));
        assert_eq!(
            extract_session_identity(&headers)
                .as_ref()
                .map(|identity| identity.value()),
            Some("sid-trimmed")
        );

        headers.insert("session_id", HeaderValue::from_static("   "));
        let identity = extract_session_identity_with_body_fallback(
            &headers,
            br#"{"session_id":"sid-from-body"}"#,
        )
        .expect("blank header should not shadow the body identity");
        assert_eq!(identity.value(), "sid-from-body");
        assert_eq!(identity.source(), SessionIdentitySource::BodySessionId);
    }

    #[test]
    fn extract_session_id_uses_body_session_metadata_and_ignores_previous_response_id() {
        let headers = HeaderMap::new();

        let identity = extract_session_identity_with_body_fallback(
            &headers,
            br#"{"model":"gpt-5","metadata":{"session_id":"meta-1"}}"#,
        )
        .expect("metadata identity");
        assert_eq!(identity.value(), "meta-1");
        assert_eq!(identity.source(), SessionIdentitySource::MetadataSessionId);

        assert!(
            extract_session_identity_with_body_fallback(
                &headers,
                br#"{"model":"gpt-5","previous_response_id":"resp-123"}"#
            )
            .is_none()
        );

        let identity = extract_session_identity_with_body_fallback(
            &headers,
            br#"{"model":"gpt-5","session_id":"body-1","prompt_cache_key":"pcache-1"}"#,
        )
        .expect("body identity");
        assert_eq!(identity.value(), "body-1");
        assert_eq!(identity.source(), SessionIdentitySource::BodySessionId);
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

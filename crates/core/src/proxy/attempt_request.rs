use std::sync::OnceLock;

use axum::http::{HeaderMap, HeaderName, HeaderValue, header};

use crate::auth_resolution::{
    UpstreamAuthResolutionError, trusted_codex_passthrough_origin,
    unconfigured_upstream_auth_contract_requires_opt_in,
};
use crate::codex_switch::CODEX_CLIENT_FACADE_ACTOR_HEADER;
#[cfg(test)]
use crate::config::UpstreamAuth;
use crate::credentials::CapturedUpstreamCredential;
use crate::logging::{BodyPreview, HeaderEntry, upstream_origin};
use crate::provider_catalog::AccountFingerprint;

use super::headers::{
    filter_request_headers, header_map_to_entries, strip_codex_client_facade_marker,
};
use super::http_debug::HttpDebugBase;
use super::request_preparation::codex_path_is_responses_compact;

pub(super) struct AttemptRequestSetup {
    pub(super) headers: HeaderMap,
    #[cfg(test)]
    pub(super) account_fingerprint: AccountFingerprint,
    pub(super) debug_base: Option<HttpDebugBase>,
}

#[derive(Debug, Clone)]
pub(super) struct AttemptRequestIdentity {
    pub(super) headers: HeaderMap,
    pub(super) account_fingerprint: AccountFingerprint,
}

pub(super) struct AttemptRequestIdentityParams<'a> {
    pub(super) service_name: &'a str,
    pub(super) credential: &'a CapturedUpstreamCredential,
    pub(super) credential_scope: Option<&'a str>,
    pub(super) state: &'a crate::state::ProxyState,
    pub(super) client_headers: &'a HeaderMap,
    pub(super) client_uri: &'a str,
    pub(super) target_url: &'a str,
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

#[cfg(test)]
pub(super) struct AttemptRequestSetupParams<'a> {
    pub(super) service_name: &'a str,
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

pub(super) struct FrozenAttemptRequestSetupParams<'a> {
    pub(super) identity: &'a AttemptRequestIdentity,
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

pub(super) fn prepare_attempt_request_identity(
    params: AttemptRequestIdentityParams<'_>,
) -> Result<AttemptRequestIdentity, UpstreamAuthResolutionError> {
    let AttemptRequestIdentityParams {
        service_name,
        credential,
        credential_scope,
        state,
        client_headers,
        client_uri,
        target_url,
    } = params;

    // Codex client credentials pass through only to the official origin when helper auth is absent.
    let mut headers = filter_request_headers(client_headers);
    strip_codex_client_facade_marker(&mut headers);
    headers.insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("identity"),
    );
    inject_auth_headers(service_name, credential, target_url, &mut headers)?;
    normalize_codex_compact_headers(service_name, client_uri, &mut headers);
    let account_fingerprint = state.derive_provider_account_fingerprint(credential_scope, &headers);

    Ok(AttemptRequestIdentity {
        headers,
        account_fingerprint,
    })
}

#[cfg(test)]
pub(super) fn prepare_attempt_request(
    params: AttemptRequestSetupParams<'_>,
) -> Result<AttemptRequestSetup, UpstreamAuthResolutionError> {
    let AttemptRequestSetupParams {
        service_name,
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

    let provider_endpoint =
        crate::runtime_identity::ProviderEndpointKey::new(service_name, "test", "default");
    let (credential, runtime_identity) =
        CapturedUpstreamCredential::runtime_binding_from_config_for_test(
            &provider_endpoint,
            "https://example.test/v1",
            None,
            auth,
        );
    let state = crate::state::ProxyState::new();
    let identity = prepare_attempt_request_identity(AttemptRequestIdentityParams {
        service_name,
        credential: &credential,
        credential_scope: runtime_identity.credential_scope.as_deref(),
        state: state.as_ref(),
        client_headers,
        client_uri,
        target_url,
    })?;

    Ok(prepare_attempt_request_with_identity(
        FrozenAttemptRequestSetupParams {
            identity: &identity,
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
        },
    ))
}

pub(super) fn prepare_attempt_request_with_identity(
    params: FrozenAttemptRequestSetupParams<'_>,
) -> AttemptRequestSetup {
    let FrozenAttemptRequestSetupParams {
        identity,
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

    let debug_base = build_http_debug_base(HttpDebugBaseParams {
        client_headers,
        client_headers_entries_cache,
        upstream_request_headers: &identity.headers,
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
        headers: identity.headers.clone(),
        #[cfg(test)]
        account_fingerprint: identity.account_fingerprint,
        debug_base,
    }
}

fn normalize_codex_compact_headers(service_name: &str, client_uri: &str, headers: &mut HeaderMap) {
    if service_name == "codex" && codex_path_is_responses_compact(client_uri_path(client_uri)) {
        headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
    }
}

fn client_uri_path(client_uri: &str) -> &str {
    client_uri
        .split_once('?')
        .map(|(path, _query)| path)
        .unwrap_or(client_uri)
}

pub(super) fn inject_auth_headers(
    service_name: &str,
    credential: &CapturedUpstreamCredential,
    target_url: &str,
    headers: &mut HeaderMap,
) -> Result<(), UpstreamAuthResolutionError> {
    let client_has_auth = headers.contains_key("authorization");
    let client_has_x_api_key = headers.contains_key("x-api-key");
    if let Some(error) = credential.first_error() {
        let resolution_error = match (error.code(), error.source_kind()) {
            (crate::credentials::CredentialErrorCode::Missing, "environment") => {
                UpstreamAuthResolutionError::MissingReference {
                    kind: error.kind_label(),
                    name: error.reference().to_string(),
                }
            }
            (crate::credentials::CredentialErrorCode::Unsupported, source_kind) => {
                UpstreamAuthResolutionError::UnsupportedReference {
                    kind: error.kind_label(),
                    source_kind,
                }
            }
            _ => UpstreamAuthResolutionError::RuntimeCredentialUnavailable {
                kind: error.kind_label(),
                source_kind: error.source_kind(),
                reason: error.code().as_str(),
                reference: error.reference().to_string(),
            },
        };
        return Err(resolution_error);
    }
    if unconfigured_upstream_auth_contract_requires_opt_in(
        service_name,
        credential.configured_contract(),
        credential.allow_anonymous(),
        target_url,
    ) {
        return Err(UpstreamAuthResolutionError::AnonymousNotAllowed);
    }
    let token = credential.bearer_header();
    let api_key = credential.api_key_header();
    let helper_credential_contract = credential.configured_contract();
    let allow_client_passthrough = service_name != "codex"
        || (!helper_credential_contract && trusted_codex_passthrough_origin(target_url));
    let allow_codex_actor_passthrough = service_name == "codex" && allow_client_passthrough;

    if service_name == "codex" && !allow_client_passthrough {
        strip_codex_client_account_headers(headers);
    }
    if !allow_codex_actor_passthrough {
        headers.remove(CODEX_CLIENT_FACADE_ACTOR_HEADER);
    }

    if let Some(value) = token {
        headers.insert(HeaderName::from_static("authorization"), value);
    } else if client_has_auth && !allow_client_passthrough {
        headers.remove("authorization");
    }

    if let Some(value) = api_key {
        headers.insert(HeaderName::from_static("x-api-key"), value);
    } else if client_has_x_api_key && !allow_client_passthrough {
        headers.remove("x-api-key");
    }
    if let Some(value) = headers.get_mut(CODEX_CLIENT_FACADE_ACTOR_HEADER) {
        value.set_sensitive(true);
    }
    Ok(())
}

fn strip_codex_client_account_headers(headers: &mut HeaderMap) {
    for name in [
        "authorization",
        "chatgpt-account-id",
        "openai-organization",
        "openai-project",
        "x-api-key",
        CODEX_CLIENT_FACADE_ACTOR_HEADER,
        "x-oai-attestation",
        "x-openai-fedramp",
        "x-openai-organization",
        "x-openai-project",
        "x-organization-id",
        "x-project-id",
    ] {
        headers.remove(HeaderName::from_static(name));
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
        upstream_origin: upstream_origin(target_url),
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
    use axum::http::HeaderValue;

    use super::*;
    use crate::provider_catalog::AccountFingerprint;

    fn explicitly_anonymous_remote_auth() -> UpstreamAuth {
        UpstreamAuth {
            allow_anonymous: Some(true),
            ..UpstreamAuth::default()
        }
    }

    #[test]
    fn prepare_attempt_request_overrides_auth_headers_from_upstream_auth() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer client-token"),
        );
        client_headers.insert("x-api-key", HeaderValue::from_static("client-key"));
        client_headers.insert(
            crate::config::CODEX_CLIENT_RUNTIME_PATCH_HEADER,
            HeaderValue::from_static("v1;models=1;hosted=disabled"),
        );
        client_headers.insert("content-type", HeaderValue::from_static("application/json"));

        let cache = OnceLock::new();
        let setup = prepare_attempt_request(AttemptRequestSetupParams {
            service_name: "codex",
            auth: &UpstreamAuth {
                auth_token: Some("server-token".to_string().into()),
                auth_token_env: None,
                auth_token_ref: None,
                api_key: Some("server-key".to_string().into()),
                api_key_env: None,
                api_key_ref: None,
                allow_anonymous: None,
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
        })
        .expect("prepare attempt request");

        assert_eq!(
            setup.headers.get("authorization"),
            Some(&HeaderValue::from_static("Bearer server-token"))
        );
        assert_eq!(
            setup.headers.get("x-api-key"),
            Some(&HeaderValue::from_static("server-key"))
        );
        assert_eq!(
            setup.headers.get("accept-encoding"),
            Some(&HeaderValue::from_static("identity"))
        );
        assert!(
            !setup
                .headers
                .contains_key(crate::config::CODEX_CLIENT_RUNTIME_PATCH_HEADER)
        );
        assert_ne!(setup.account_fingerprint, AccountFingerprint::unscoped());
        assert!(setup.debug_base.is_none());
    }

    #[test]
    fn prepare_attempt_request_rejects_unconfigured_remote_relay_by_default() {
        let credential =
            CapturedUpstreamCredential::from_config_for_test("codex", &UpstreamAuth::default());
        let state = crate::state::ProxyState::new();
        let result = prepare_attempt_request_identity(AttemptRequestIdentityParams {
            service_name: "codex",
            credential: &credential,
            credential_scope: None,
            state: state.as_ref(),
            client_headers: &HeaderMap::new(),
            client_uri: "/v1/responses",
            target_url: "https://third-party.example/v1/responses",
        });

        assert!(matches!(
            result,
            Err(UpstreamAuthResolutionError::AnonymousNotAllowed)
        ));
    }

    #[test]
    fn official_passthrough_account_identity_is_keyed_stable_and_account_scoped() {
        let state = crate::state::ProxyState::new();
        let credential =
            CapturedUpstreamCredential::from_config_for_test("codex", &UpstreamAuth::default());
        let prepare = |headers: &HeaderMap| {
            prepare_attempt_request_identity(AttemptRequestIdentityParams {
                service_name: "codex",
                credential: &credential,
                credential_scope: None,
                state: state.as_ref(),
                client_headers: headers,
                client_uri: "/v1/responses",
                target_url: "https://api.openai.com/v1/responses",
            })
            .expect("prepare official passthrough identity")
        };

        let mut first_headers = HeaderMap::new();
        first_headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer passthrough-account-one"),
        );
        let mut same_headers = first_headers.clone();
        same_headers.insert("x-request-id", HeaderValue::from_static("request-two"));
        let mut second_headers = HeaderMap::new();
        second_headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer passthrough-account-two"),
        );

        let first = prepare(&first_headers);
        let same = prepare(&same_headers);
        let second = prepare(&second_headers);
        let anonymous = prepare(&HeaderMap::new());

        assert_eq!(first.account_fingerprint, same.account_fingerprint);
        assert_ne!(first.account_fingerprint, second.account_fingerprint);
        assert_eq!(
            anonymous.account_fingerprint,
            AccountFingerprint::unscoped()
        );
        assert_ne!(first.account_fingerprint, anonymous.account_fingerprint);
    }

    #[test]
    fn prepare_attempt_request_explicit_anonymous_opt_in_strips_client_credentials() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer chatgpt-token"),
        );
        client_headers.insert("x-api-key", HeaderValue::from_static("client-key"));
        client_headers.insert(
            "chatgpt-account-id",
            HeaderValue::from_static("account-client"),
        );
        client_headers.insert("x-openai-fedramp", HeaderValue::from_static("true"));
        client_headers.insert(
            "x-oai-attestation",
            HeaderValue::from_static("device-attestation"),
        );
        client_headers.insert(
            "x-openai-actor-authorization",
            HeaderValue::from_static("real-actor-token"),
        );
        client_headers.insert("content-type", HeaderValue::from_static("application/json"));

        let cache = OnceLock::new();
        let setup = prepare_attempt_request(AttemptRequestSetupParams {
            service_name: "codex",
            auth: &explicitly_anonymous_remote_auth(),
            client_headers: &client_headers,
            client_headers_entries_cache: &cache,
            request_body_len: 12,
            upstream_request_body_len: 12,
            debug_max: 0,
            warn_max: 0,
            client_uri: "/v1/responses",
            target_url: "https://third-party.example/v1/responses",
            client_body_debug: None,
            upstream_request_body_debug: None,
            client_body_warn: None,
            upstream_request_body_warn: None,
        })
        .expect("prepare attempt request");

        assert!(!setup.headers.contains_key("authorization"));
        assert!(!setup.headers.contains_key("x-api-key"));
        assert!(!setup.headers.contains_key("chatgpt-account-id"));
        assert!(!setup.headers.contains_key("x-openai-fedramp"));
        assert!(!setup.headers.contains_key("x-oai-attestation"));
        assert!(!setup.headers.contains_key("x-openai-actor-authorization"));
        assert_eq!(
            setup.headers.get("content-type"),
            Some(&HeaderValue::from_static("application/json"))
        );
        assert_eq!(setup.account_fingerprint, AccountFingerprint::unscoped());
    }

    #[test]
    fn prepare_attempt_request_uses_explicit_upstream_secret_and_strips_client_account_metadata() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer chatgpt-token"),
        );
        client_headers.insert(
            "chatgpt-account-id",
            HeaderValue::from_static("account-client"),
        );
        client_headers.insert(
            "x-openai-actor-authorization",
            HeaderValue::from_static("real-actor-token"),
        );
        client_headers.insert("x-openai-fedramp", HeaderValue::from_static("true"));

        let cache = OnceLock::new();
        let setup = prepare_attempt_request(AttemptRequestSetupParams {
            service_name: "codex",
            auth: &UpstreamAuth {
                auth_token: Some("relay-token".to_string().into()),
                auth_token_env: None,
                auth_token_ref: None,
                api_key: None,
                api_key_env: None,
                api_key_ref: None,
                allow_anonymous: None,
            },
            client_headers: &client_headers,
            client_headers_entries_cache: &cache,
            request_body_len: 12,
            upstream_request_body_len: 12,
            debug_max: 0,
            warn_max: 0,
            client_uri: "/v1/responses",
            target_url: "https://third-party.example/v1/responses",
            client_body_debug: None,
            upstream_request_body_debug: None,
            client_body_warn: None,
            upstream_request_body_warn: None,
        })
        .expect("prepare attempt request");

        assert_eq!(
            setup.headers.get("authorization"),
            Some(&HeaderValue::from_static("Bearer relay-token"))
        );
        assert!(!setup.headers.contains_key("chatgpt-account-id"));
        assert!(!setup.headers.contains_key("x-openai-fedramp"));
        assert!(!setup.headers.contains_key("x-openai-actor-authorization"));
    }

    #[test]
    fn prepare_attempt_request_allows_passthrough_only_for_official_openai_origin() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer official-token"),
        );
        client_headers.insert(
            "chatgpt-account-id",
            HeaderValue::from_static("account-client"),
        );
        client_headers.insert(
            "x-openai-actor-authorization",
            HeaderValue::from_static("real-actor-token"),
        );

        let cache = OnceLock::new();
        let setup = prepare_attempt_request(AttemptRequestSetupParams {
            service_name: "codex",
            auth: &UpstreamAuth::default(),
            client_headers: &client_headers,
            client_headers_entries_cache: &cache,
            request_body_len: 12,
            upstream_request_body_len: 12,
            debug_max: 0,
            warn_max: 0,
            client_uri: "/v1/responses",
            target_url: "https://api.openai.com/v1/responses",
            client_body_debug: None,
            upstream_request_body_debug: None,
            client_body_warn: None,
            upstream_request_body_warn: None,
        })
        .expect("prepare attempt request");

        assert_eq!(
            setup.headers.get("authorization"),
            Some(&HeaderValue::from_static("Bearer official-token"))
        );
        assert_eq!(
            setup.headers.get("chatgpt-account-id"),
            Some(&HeaderValue::from_static("account-client"))
        );
        assert_eq!(
            setup.headers.get("x-openai-actor-authorization"),
            Some(&HeaderValue::from_static("real-actor-token"))
        );
        assert!(
            setup
                .headers
                .get("x-openai-actor-authorization")
                .expect("actor authorization")
                .is_sensitive()
        );
    }

    #[test]
    fn prepare_attempt_request_never_forwards_codex_actor_auth_for_other_services() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer claude-client-token"),
        );
        client_headers.insert("x-api-key", HeaderValue::from_static("claude-client-key"));
        client_headers.insert(
            CODEX_CLIENT_FACADE_ACTOR_HEADER,
            HeaderValue::from_static("real-actor-token"),
        );

        let cache = OnceLock::new();
        let setup = prepare_attempt_request(AttemptRequestSetupParams {
            service_name: "claude",
            auth: &UpstreamAuth::default(),
            client_headers: &client_headers,
            client_headers_entries_cache: &cache,
            request_body_len: 12,
            upstream_request_body_len: 12,
            debug_max: 0,
            warn_max: 0,
            client_uri: "/v1/messages",
            target_url: "https://third-party.example/v1/messages",
            client_body_debug: None,
            upstream_request_body_debug: None,
            client_body_warn: None,
            upstream_request_body_warn: None,
        })
        .expect("prepare non-Codex attempt request");

        assert_eq!(
            setup.headers.get("authorization"),
            Some(&HeaderValue::from_static("Bearer claude-client-token"))
        );
        assert_eq!(
            setup.headers.get("x-api-key"),
            Some(&HeaderValue::from_static("claude-client-key"))
        );
        assert!(!setup.headers.contains_key(CODEX_CLIENT_FACADE_ACTOR_HEADER));
    }

    #[test]
    fn prepare_attempt_request_consumes_and_redacts_client_facade_marker() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert(
            crate::codex_switch::CODEX_CLIENT_FACADE_ACTOR_HEADER,
            HeaderValue::from_static(crate::codex_switch::CODEX_CLIENT_FACADE_ACTOR_VALUE),
        );

        let cache = OnceLock::new();
        let setup = prepare_attempt_request(AttemptRequestSetupParams {
            service_name: "codex",
            auth: &UpstreamAuth::default(),
            client_headers: &client_headers,
            client_headers_entries_cache: &cache,
            request_body_len: 12,
            upstream_request_body_len: 12,
            debug_max: 128,
            warn_max: 0,
            client_uri: "/v1/responses",
            target_url: "https://api.openai.com/v1/responses",
            client_body_debug: None,
            upstream_request_body_debug: None,
            client_body_warn: None,
            upstream_request_body_warn: None,
        })
        .expect("prepare facade request");

        assert!(
            !setup
                .headers
                .contains_key(crate::codex_switch::CODEX_CLIENT_FACADE_ACTOR_HEADER)
        );
        assert_eq!(setup.account_fingerprint, AccountFingerprint::unscoped());
        assert!(
            setup
                .debug_base
                .expect("debug metadata")
                .client_headers
                .iter()
                .any(|entry| {
                    entry
                        .name
                        .eq_ignore_ascii_case(crate::codex_switch::CODEX_CLIENT_FACADE_ACTOR_HEADER)
                        && entry.value == "[REDACTED]"
                })
        );
    }

    #[test]
    fn prepare_attempt_request_missing_helper_env_contract_fails_closed_on_official_origin() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer official-token"),
        );
        client_headers.insert(
            "chatgpt-account-id",
            HeaderValue::from_static("account-client"),
        );

        let cache = OnceLock::new();
        let result = prepare_attempt_request(AttemptRequestSetupParams {
            service_name: "codex",
            auth: &UpstreamAuth {
                auth_token: None,
                auth_token_env: Some(
                    "CODEX_HELPER_TEST_DEFINITELY_MISSING_PROVIDER_TOKEN_7C2A".to_string(),
                ),
                auth_token_ref: None,
                api_key: None,
                api_key_env: None,
                api_key_ref: None,
                allow_anonymous: None,
            },
            client_headers: &client_headers,
            client_headers_entries_cache: &cache,
            request_body_len: 12,
            upstream_request_body_len: 12,
            debug_max: 0,
            warn_max: 0,
            client_uri: "/v1/responses",
            target_url: "https://api.openai.com/v1/responses",
            client_body_debug: None,
            upstream_request_body_debug: None,
            client_body_warn: None,
            upstream_request_body_warn: None,
        });

        assert!(matches!(
            result,
            Err(UpstreamAuthResolutionError::MissingReference {
                kind: "Bearer token",
                ref name,
            }) if name == "CODEX_HELPER_TEST_DEFINITELY_MISSING_PROVIDER_TOKEN_7C2A"
        ));
    }

    #[test]
    fn prepare_attempt_request_does_not_passthrough_official_auth_for_a_configured_reference() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer official-token"),
        );

        let cache = OnceLock::new();
        let result = prepare_attempt_request(AttemptRequestSetupParams {
            service_name: "codex",
            auth: &UpstreamAuth {
                auth_token_ref: Some(crate::config::CredentialRef::Native {
                    name: "relay.primary".to_string(),
                }),
                ..UpstreamAuth::default()
            },
            client_headers: &client_headers,
            client_headers_entries_cache: &cache,
            request_body_len: 12,
            upstream_request_body_len: 12,
            debug_max: 0,
            warn_max: 0,
            client_uri: "/v1/responses",
            target_url: "https://api.openai.com/v1/responses",
            client_body_debug: None,
            upstream_request_body_debug: None,
            client_body_warn: None,
            upstream_request_body_warn: None,
        });

        assert!(matches!(
            result,
            Err(UpstreamAuthResolutionError::UnsupportedReference {
                kind: "Bearer token",
                source_kind: "native",
            })
        ));
    }

    #[test]
    fn prepare_attempt_request_forces_json_accept_for_codex_compact() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert("accept", HeaderValue::from_static("text/event-stream"));

        let cache = OnceLock::new();
        let setup = prepare_attempt_request(AttemptRequestSetupParams {
            service_name: "codex",
            auth: &explicitly_anonymous_remote_auth(),
            client_headers: &client_headers,
            client_headers_entries_cache: &cache,
            request_body_len: 12,
            upstream_request_body_len: 12,
            debug_max: 0,
            warn_max: 0,
            client_uri: "/v1/responses/compact",
            target_url: "https://third-party.example/v1/responses/compact",
            client_body_debug: None,
            upstream_request_body_debug: None,
            client_body_warn: None,
            upstream_request_body_warn: None,
        })
        .expect("prepare attempt request");

        assert_eq!(
            setup.headers.get(header::ACCEPT),
            Some(&HeaderValue::from_static("application/json"))
        );
    }

    #[test]
    fn prepare_attempt_request_forces_json_accept_for_codex_compact_with_trailing_slash() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert("accept", HeaderValue::from_static("text/event-stream"));

        let cache = OnceLock::new();
        let setup = prepare_attempt_request(AttemptRequestSetupParams {
            service_name: "codex",
            auth: &explicitly_anonymous_remote_auth(),
            client_headers: &client_headers,
            client_headers_entries_cache: &cache,
            request_body_len: 12,
            upstream_request_body_len: 12,
            debug_max: 0,
            warn_max: 0,
            client_uri: "/v1/responses/compact/",
            target_url: "https://third-party.example/v1/responses/compact/",
            client_body_debug: None,
            upstream_request_body_debug: None,
            client_body_warn: None,
            upstream_request_body_warn: None,
        })
        .expect("prepare attempt request");

        assert_eq!(
            setup.headers.get(header::ACCEPT),
            Some(&HeaderValue::from_static("application/json"))
        );
    }

    #[test]
    fn prepare_attempt_request_forces_json_accept_for_codex_compact_with_query() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert("accept", HeaderValue::from_static("text/event-stream"));

        let cache = OnceLock::new();
        let setup = prepare_attempt_request(AttemptRequestSetupParams {
            service_name: "codex",
            auth: &explicitly_anonymous_remote_auth(),
            client_headers: &client_headers,
            client_headers_entries_cache: &cache,
            request_body_len: 12,
            upstream_request_body_len: 12,
            debug_max: 0,
            warn_max: 0,
            client_uri: "/v1/responses/compact?trace=1",
            target_url: "https://third-party.example/v1/responses/compact?trace=1",
            client_body_debug: None,
            upstream_request_body_debug: None,
            client_body_warn: None,
            upstream_request_body_warn: None,
        })
        .expect("prepare attempt request");

        assert_eq!(
            setup.headers.get(header::ACCEPT),
            Some(&HeaderValue::from_static("application/json"))
        );
    }

    #[test]
    fn prepare_attempt_request_preserves_accept_for_codex_responses() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert("accept", HeaderValue::from_static("text/event-stream"));

        let cache = OnceLock::new();
        let setup = prepare_attempt_request(AttemptRequestSetupParams {
            service_name: "codex",
            auth: &explicitly_anonymous_remote_auth(),
            client_headers: &client_headers,
            client_headers_entries_cache: &cache,
            request_body_len: 12,
            upstream_request_body_len: 12,
            debug_max: 0,
            warn_max: 0,
            client_uri: "/v1/responses",
            target_url: "https://third-party.example/v1/responses",
            client_body_debug: None,
            upstream_request_body_debug: None,
            client_body_warn: None,
            upstream_request_body_warn: None,
        })
        .expect("prepare attempt request");

        assert_eq!(
            setup.headers.get(header::ACCEPT),
            Some(&HeaderValue::from_static("text/event-stream"))
        );
    }

    #[test]
    fn prepare_attempt_request_builds_debug_base_when_limits_enabled() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert("content-type", HeaderValue::from_static("application/json"));

        let cache = OnceLock::new();
        let setup = prepare_attempt_request(AttemptRequestSetupParams {
            service_name: "codex",
            auth: &explicitly_anonymous_remote_auth(),
            client_headers: &client_headers,
            client_headers_entries_cache: &cache,
            request_body_len: 18,
            upstream_request_body_len: 22,
            debug_max: 128,
            warn_max: 64,
            client_uri: "/v1/responses",
            target_url: "https://user:secret@example.com:8443/private/secret-path?token=hidden#fragment",
            client_body_debug: None,
            upstream_request_body_debug: None,
            client_body_warn: None,
            upstream_request_body_warn: None,
        })
        .expect("prepare attempt request");

        let debug_base = setup.debug_base.expect("debug_base");
        assert_eq!(debug_base.client_uri, "/v1/responses");
        assert_eq!(
            debug_base.upstream_origin.as_deref(),
            Some("https://example.com:8443")
        );
        assert_eq!(debug_base.request_body_len, 18);
        assert_eq!(debug_base.upstream_request_body_len, 22);
        assert!(!debug_base.client_headers.is_empty());
    }
}

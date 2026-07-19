use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use futures_util::StreamExt;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::config::UpstreamConfig;
use crate::credentials::{CapturedUpstreamCredential, CredentialReadinessCode};

use super::classify::{ROUTING_MISMATCH_CAPABILITY_CLASS, classify_upstream_response};
use super::models_compat::{ModelsTranslationScope, maybe_decode_models_response_body};

const MAX_PROBE_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const UPSTREAM_AUTH_UNAVAILABLE_REASON: &str = "configured upstream credentials are unavailable";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexRelayProbeKind {
    Models,
    Responses,
    ResponsesCompact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexRelayProbeSupport {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexRelayProbeConfidence {
    SuccessStatus,
    EndpointValidation,
    ErrorClassification,
    Credential,
    Transport,
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexRelayProbeSideEffect {
    ReadOnly,
    ValidationOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::proxy) struct CodexRelayProbeCase {
    pub kind: CodexRelayProbeKind,
    pub capability: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub side_effect: CodexRelayProbeSideEffect,
    body: CodexRelayProbeBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexRelayProbeBody {
    None,
    EmptyJsonObject,
}

const CODEX_RELAY_PROBE_CASES: &[CodexRelayProbeCase] = &[
    CodexRelayProbeCase {
        kind: CodexRelayProbeKind::Models,
        capability: "model_catalog",
        method: "GET",
        path: "/models",
        side_effect: CodexRelayProbeSideEffect::ReadOnly,
        body: CodexRelayProbeBody::None,
    },
    CodexRelayProbeCase {
        kind: CodexRelayProbeKind::Responses,
        capability: "responses",
        method: "POST",
        path: "/responses",
        side_effect: CodexRelayProbeSideEffect::ValidationOnly,
        body: CodexRelayProbeBody::EmptyJsonObject,
    },
    CodexRelayProbeCase {
        kind: CodexRelayProbeKind::ResponsesCompact,
        capability: "remote_compaction_v1",
        method: "POST",
        path: "/responses/compact",
        side_effect: CodexRelayProbeSideEffect::ValidationOnly,
        body: CodexRelayProbeBody::EmptyJsonObject,
    },
];

pub(in crate::proxy) fn codex_relay_probe_cases() -> &'static [CodexRelayProbeCase] {
    CODEX_RELAY_PROBE_CASES
}

impl CodexRelayProbeCase {
    pub(in crate::proxy) fn for_kind(kind: CodexRelayProbeKind) -> &'static Self {
        codex_relay_probe_cases()
            .iter()
            .find(|case| case.kind == kind)
            .expect("Codex relay probe kind must be registered")
    }

    pub(in crate::proxy) fn spec(&self) -> CodexRelayProbeSpec {
        CodexRelayProbeSpec {
            kind: self.kind,
            method: self.method.to_string(),
            path: self.path.to_string(),
            side_effect: self.side_effect,
            body: match self.body {
                CodexRelayProbeBody::None => None,
                CodexRelayProbeBody::EmptyJsonObject => Some(serde_json::json!({})),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexRelayProbeSpec {
    pub kind: CodexRelayProbeKind,
    pub method: String,
    pub path: String,
    pub side_effect: CodexRelayProbeSideEffect,
    pub body: Option<Value>,
}

impl CodexRelayProbeSpec {
    pub fn for_kind(kind: CodexRelayProbeKind) -> Self {
        CodexRelayProbeCase::for_kind(kind).spec()
    }

    fn method(&self) -> Method {
        Method::from_bytes(self.method.as_bytes()).unwrap_or(Method::GET)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexRelayProbeResult {
    pub kind: CodexRelayProbeKind,
    pub support: CodexRelayProbeSupport,
    pub confidence: CodexRelayProbeConfidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_readiness: Option<CredentialReadinessCode>,
    pub status_code: Option<u16>,
    pub response_shape: Option<String>,
    pub translation_required: bool,
    pub error_class: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub(in crate::proxy) struct CodexRelayProbeObservation {
    pub result: CodexRelayProbeResult,
    pub status: Option<StatusCode>,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl CodexRelayProbeResult {
    fn supported(
        kind: CodexRelayProbeKind,
        confidence: CodexRelayProbeConfidence,
        status_code: Option<u16>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            support: CodexRelayProbeSupport::Supported,
            confidence,
            credential_readiness: None,
            status_code,
            response_shape: None,
            translation_required: false,
            error_class: None,
            reason: reason.into(),
        }
    }

    fn unsupported(
        kind: CodexRelayProbeKind,
        confidence: CodexRelayProbeConfidence,
        status_code: Option<u16>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            support: CodexRelayProbeSupport::Unsupported,
            confidence,
            credential_readiness: None,
            status_code,
            response_shape: None,
            translation_required: false,
            error_class: None,
            reason: reason.into(),
        }
    }

    fn unknown(
        kind: CodexRelayProbeKind,
        confidence: CodexRelayProbeConfidence,
        status_code: Option<u16>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            support: CodexRelayProbeSupport::Unknown,
            confidence,
            credential_readiness: None,
            status_code,
            response_shape: None,
            translation_required: false,
            error_class: None,
            reason: reason.into(),
        }
    }
}

pub fn classify_codex_relay_probe_response(
    spec: &CodexRelayProbeSpec,
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
) -> CodexRelayProbeResult {
    let body = if spec.kind == CodexRelayProbeKind::Models && status.is_success() {
        maybe_decode_models_response_body(
            "codex",
            "/models",
            headers,
            Bytes::copy_from_slice(body),
            ModelsTranslationScope::Disabled,
        )
    } else {
        Bytes::copy_from_slice(body)
    };
    let status_code = status.as_u16();
    if spec.kind == CodexRelayProbeKind::Models {
        return classify_models_probe_response(spec.kind, status, body.as_ref());
    }

    let (error_class, _, _) = classify_upstream_response(status_code, headers, body.as_ref());
    let mut result = classify_endpoint_probe_response(spec.kind, status, body.as_ref());
    result.error_class = error_class;
    if result.support == CodexRelayProbeSupport::Unknown
        && result.error_class.as_deref() == Some(ROUTING_MISMATCH_CAPABILITY_CLASS)
    {
        result.support = CodexRelayProbeSupport::Supported;
        result.confidence = CodexRelayProbeConfidence::ErrorClassification;
        result.reason =
            "endpoint exists but rejected the probe due to a model or capability mismatch"
                .to_string();
    }
    result
}

fn classify_models_probe_response(
    kind: CodexRelayProbeKind,
    status: StatusCode,
    body: &[u8],
) -> CodexRelayProbeResult {
    if is_unsupported_endpoint_status(status) {
        return CodexRelayProbeResult::unsupported(
            kind,
            CodexRelayProbeConfidence::ErrorClassification,
            Some(status.as_u16()),
            "/models endpoint is not available on this relay",
        );
    }
    if !status.is_success() {
        return CodexRelayProbeResult::unknown(
            kind,
            CodexRelayProbeConfidence::ErrorClassification,
            Some(status.as_u16()),
            "models probe did not return a successful response",
        );
    }

    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return CodexRelayProbeResult::unknown(
            kind,
            CodexRelayProbeConfidence::Malformed,
            Some(status.as_u16()),
            "models probe returned non-JSON or malformed JSON",
        );
    };
    if value.get("models").and_then(Value::as_array).is_some() {
        let mut result = CodexRelayProbeResult::supported(
            kind,
            CodexRelayProbeConfidence::SuccessStatus,
            Some(status.as_u16()),
            "relay returned a Codex models catalog",
        );
        result.response_shape = Some("codex_models".to_string());
        return result;
    }
    if value.get("data").and_then(Value::as_array).is_some() {
        let mut result = CodexRelayProbeResult::supported(
            kind,
            CodexRelayProbeConfidence::SuccessStatus,
            Some(status.as_u16()),
            "relay returned an OpenAI models list that helper can translate",
        );
        result.response_shape = Some("openai_data_list".to_string());
        result.translation_required = true;
        return result;
    }
    CodexRelayProbeResult::unknown(
        kind,
        CodexRelayProbeConfidence::Malformed,
        Some(status.as_u16()),
        "models probe JSON does not contain `models` or `data` arrays",
    )
}

fn classify_endpoint_probe_response(
    kind: CodexRelayProbeKind,
    status: StatusCode,
    body: &[u8],
) -> CodexRelayProbeResult {
    if status.is_success() {
        return CodexRelayProbeResult::supported(
            kind,
            CodexRelayProbeConfidence::SuccessStatus,
            Some(status.as_u16()),
            "endpoint accepted the probe request",
        );
    }
    if is_unsupported_endpoint_status(status)
        || (kind == CodexRelayProbeKind::ResponsesCompact
            && body_mentions_compact_unsupported(body))
    {
        return CodexRelayProbeResult::unsupported(
            kind,
            CodexRelayProbeConfidence::ErrorClassification,
            Some(status.as_u16()),
            "endpoint is missing or explicitly reports unsupported capability",
        );
    }
    if matches!(
        status,
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
    ) && looks_like_validation_error(body)
    {
        return CodexRelayProbeResult::supported(
            kind,
            CodexRelayProbeConfidence::EndpointValidation,
            Some(status.as_u16()),
            "endpoint exists and returned validation feedback for the validation-only probe",
        );
    }
    CodexRelayProbeResult::unknown(
        kind,
        CodexRelayProbeConfidence::ErrorClassification,
        Some(status.as_u16()),
        "endpoint returned an inconclusive response",
    )
}

fn is_unsupported_endpoint_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED
    )
}

fn body_mentions_compact_unsupported(body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    (text.contains("compact") || text.contains("compaction"))
        && (text.contains("unsupported")
            || text.contains("not supported")
            || text.contains("not implemented")
            || text.contains("not found"))
}

fn looks_like_validation_error(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let lower = value.to_string().to_ascii_lowercase();
    lower.contains("missing")
        || lower.contains("required")
        || lower.contains("invalid")
        || lower.contains("validation")
        || lower.contains("model")
        || lower.contains("input")
}

#[derive(Debug, Clone)]
pub(crate) struct CodexRelayProbeClient {
    client: reqwest::Client,
    credential: CapturedUpstreamCredential,
}

impl CodexRelayProbeClient {
    pub(crate) fn new(client: reqwest::Client, credential: CapturedUpstreamCredential) -> Self {
        Self { client, credential }
    }

    pub(in crate::proxy) fn credential_readiness(
        &self,
        upstream: &UpstreamConfig,
    ) -> CredentialReadinessCode {
        crate::auth_resolution::target_credential_readiness(
            "codex",
            self.credential.configured_contract(),
            self.credential.allow_anonymous(),
            upstream.base_url.as_str(),
            self.credential.readiness_code(),
        )
    }

    #[cfg(test)]
    pub(crate) async fn probe_upstream(
        &self,
        upstream: &UpstreamConfig,
        spec: &CodexRelayProbeSpec,
    ) -> CodexRelayProbeResult {
        self.probe_upstream_observation(upstream, spec).await.result
    }

    #[cfg(test)]
    pub(in crate::proxy) async fn probe_upstream_observation(
        &self,
        upstream: &UpstreamConfig,
        spec: &CodexRelayProbeSpec,
    ) -> CodexRelayProbeObservation {
        let readiness = self.credential_readiness(upstream);
        self.probe_upstream_observation_with_readiness(upstream, spec, readiness)
            .await
    }

    pub(in crate::proxy) async fn probe_upstream_observation_with_readiness(
        &self,
        upstream: &UpstreamConfig,
        spec: &CodexRelayProbeSpec,
        readiness: CredentialReadinessCode,
    ) -> CodexRelayProbeObservation {
        if !readiness.is_routable() {
            return credential_observation(spec.kind, readiness);
        }
        let url = match build_probe_url(&upstream.base_url, spec.path.as_str()) {
            Ok(url) => url,
            Err(error) => {
                return transport_observation(spec.kind, None, error)
                    .with_credential_readiness(readiness);
            }
        };

        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::ACCEPT_ENCODING,
            HeaderValue::from_static("identity"),
        );
        if spec.body.is_some() {
            headers.insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
        }
        if super::attempt_request::inject_auth_headers(
            "codex",
            &self.credential,
            url.as_str(),
            &mut headers,
        )
        .is_err()
        {
            return credential_observation(spec.kind, CredentialReadinessCode::Invalid);
        }

        let mut request = self
            .client
            .request(spec.method(), url)
            .headers(headers)
            .timeout(std::time::Duration::from_secs(15));
        if let Some(body) = spec.body.as_ref() {
            request = request.json(body);
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                return transport_observation(
                    spec.kind,
                    None,
                    reqwest_probe_transport_reason(&error),
                )
                .with_credential_readiness(readiness);
            }
        };

        let status = StatusCode::from_u16(response.status().as_u16())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let headers = response.headers().clone();
        let body = match read_limited_body(response, MAX_PROBE_RESPONSE_BYTES).await {
            Ok(body) => body,
            Err(error) => {
                return transport_observation(spec.kind, Some(status), error)
                    .with_credential_readiness(readiness);
            }
        };
        let result = classify_codex_relay_probe_response(spec, status, &headers, body.as_ref());
        CodexRelayProbeObservation {
            result,
            status: Some(status),
            headers,
            body,
        }
        .with_credential_readiness(readiness)
    }
}

impl CodexRelayProbeObservation {
    fn with_credential_readiness(mut self, readiness: CredentialReadinessCode) -> Self {
        self.result.credential_readiness = Some(readiness);
        self
    }
}

pub(in crate::proxy) fn credential_observation(
    kind: CodexRelayProbeKind,
    readiness: CredentialReadinessCode,
) -> CodexRelayProbeObservation {
    CodexRelayProbeObservation {
        result: CodexRelayProbeResult {
            kind,
            support: CodexRelayProbeSupport::Unknown,
            confidence: CodexRelayProbeConfidence::Credential,
            credential_readiness: Some(readiness),
            status_code: None,
            response_shape: None,
            translation_required: false,
            error_class: None,
            reason: UPSTREAM_AUTH_UNAVAILABLE_REASON.to_string(),
        },
        status: None,
        headers: HeaderMap::new(),
        body: Bytes::new(),
    }
}

fn transport_observation(
    kind: CodexRelayProbeKind,
    status: Option<StatusCode>,
    reason: impl Into<String>,
) -> CodexRelayProbeObservation {
    CodexRelayProbeObservation {
        result: CodexRelayProbeResult::unknown(
            kind,
            CodexRelayProbeConfidence::Transport,
            status.map(|status| status.as_u16()),
            reason,
        ),
        status,
        headers: HeaderMap::new(),
        body: Bytes::new(),
    }
}

fn reqwest_probe_transport_reason(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "probe request timed out"
    } else if error.is_connect() {
        "probe connection failed"
    } else {
        "probe transport error"
    }
}

fn build_probe_url(base_url: &str, path: &str) -> Result<reqwest::Url, String> {
    let base = base_url.trim_end_matches('/');
    let base_url =
        reqwest::Url::parse(base).map_err(|_| "invalid upstream base_url".to_string())?;
    let base_path = base_url.path().trim_end_matches('/');
    let mut path = path.to_string();
    if !base_path.is_empty()
        && base_path != "/"
        && (path == base_path || path.starts_with(&format!("{base_path}/")))
    {
        let rest = &path[base_path.len()..];
        path = if rest.is_empty() {
            "/".to_string()
        } else {
            rest.to_string()
        };
    }
    if !path.starts_with('/') {
        path = format!("/{path}");
    }
    let full = format!("{base}{path}");
    reqwest::Url::parse(&full).map_err(|_| "invalid probe url".to_string())
}

async fn read_limited_body(response: reqwest::Response, max_bytes: usize) -> Result<Bytes, String> {
    let mut stream = response.bytes_stream();
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "read probe response body failed".to_string())?;
        if out.len() + chunk.len() > max_bytes {
            return Err(format!("probe response body exceeded {max_bytes} bytes"));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(out))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use axum::Json;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::{get, post};

    use super::*;
    use crate::config::{UpstreamAuth, UpstreamConfig};

    fn spec(kind: CodexRelayProbeKind) -> CodexRelayProbeSpec {
        CodexRelayProbeSpec::for_kind(kind)
    }

    fn json_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers
    }

    fn upstream(base_url: String) -> UpstreamConfig {
        UpstreamConfig {
            base_url,
            auth: UpstreamAuth::default(),
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }
    }

    fn captured_credential(upstream: &UpstreamConfig) -> CapturedUpstreamCredential {
        let store = crate::runtime_store::RuntimeStore::open_in_memory()
            .expect("open credential runtime store");
        let runtime = crate::credentials::CredentialRuntime::from_runtime_store(
            crate::credentials::CredentialSourceCapabilities::server(),
            &store,
        )
        .expect("build credential runtime");
        let provider_endpoint =
            crate::runtime_identity::ProviderEndpointKey::new("codex", "test", "default");
        runtime
            .build_generation([crate::credentials::CredentialCandidateInput {
                provider_endpoint: provider_endpoint.clone(),
                auth: &upstream.auth,
            }])
            .expect("build credential generation")
            .capture_bound(&provider_endpoint)
            .expect("capture registered credential")
    }

    fn spawn_axum_server(app: axum::Router) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        listener.set_nonblocking(true).expect("nonblocking");
        let listener = tokio::net::TcpListener::from_std(listener).expect("to tokio listener");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve probe test");
        });
        (addr, handle)
    }

    #[test]
    fn codex_relay_probe_registry_defines_existing_wire_contracts() {
        let cases = codex_relay_probe_cases();
        assert_eq!(cases.len(), 3);
        assert_eq!(
            cases.iter().map(|case| case.kind).collect::<Vec<_>>(),
            vec![
                CodexRelayProbeKind::Models,
                CodexRelayProbeKind::Responses,
                CodexRelayProbeKind::ResponsesCompact,
            ]
        );
        assert_eq!(
            cases.iter().map(|case| case.capability).collect::<Vec<_>>(),
            vec!["model_catalog", "responses", "remote_compaction_v1"]
        );

        let compact = CodexRelayProbeSpec::for_kind(CodexRelayProbeKind::ResponsesCompact);
        assert_eq!(compact.method, "POST");
        assert_eq!(compact.path, "/responses/compact");
        assert_eq!(
            compact.side_effect,
            CodexRelayProbeSideEffect::ValidationOnly
        );
        assert_eq!(compact.body, Some(serde_json::json!({})));
    }

    #[test]
    fn codex_relay_probe_models_classifies_codex_catalog() {
        let body = br#"{"models":[{"slug":"gpt-5.5"}]}"#;

        let result = classify_codex_relay_probe_response(
            &spec(CodexRelayProbeKind::Models),
            StatusCode::OK,
            &json_headers(),
            body,
        );

        assert_eq!(result.support, CodexRelayProbeSupport::Supported);
        assert_eq!(result.response_shape.as_deref(), Some("codex_models"));
        assert!(!result.translation_required);
    }

    #[test]
    fn codex_relay_probe_old_json_defaults_credential_readiness() {
        let mut value = serde_json::to_value(CodexRelayProbeResult::supported(
            CodexRelayProbeKind::Models,
            CodexRelayProbeConfidence::SuccessStatus,
            Some(200),
            "ok",
        ))
        .expect("serialize probe result");
        value
            .as_object_mut()
            .expect("probe result object")
            .remove("credential_readiness");

        let decoded = serde_json::from_value::<CodexRelayProbeResult>(value)
            .expect("old probe result should deserialize");

        assert_eq!(decoded.credential_readiness, None);
    }

    #[test]
    fn codex_relay_probe_models_classifies_openai_list_as_translatable() {
        let body = br#"{"object":"list","data":[{"id":"gpt-5.5"}]}"#;

        let result = classify_codex_relay_probe_response(
            &spec(CodexRelayProbeKind::Models),
            StatusCode::OK,
            &json_headers(),
            body,
        );

        assert_eq!(result.support, CodexRelayProbeSupport::Supported);
        assert_eq!(result.response_shape.as_deref(), Some("openai_data_list"));
        assert!(result.translation_required);
    }

    #[test]
    fn codex_relay_probe_models_malformed_json_is_unknown() {
        let result = classify_codex_relay_probe_response(
            &spec(CodexRelayProbeKind::Models),
            StatusCode::OK,
            &json_headers(),
            br#"{"object":"list","items":[]}"#,
        );

        assert_eq!(result.support, CodexRelayProbeSupport::Unknown);
        assert_eq!(result.confidence, CodexRelayProbeConfidence::Malformed);
        assert_eq!(result.response_shape, None);
        assert!(!result.translation_required);
    }

    #[test]
    fn codex_relay_probe_responses_validation_error_marks_endpoint_supported() {
        let body = br#"{"error":{"type":"invalid_request_error","message":"Missing required parameter: model"}}"#;

        let result = classify_codex_relay_probe_response(
            &spec(CodexRelayProbeKind::Responses),
            StatusCode::BAD_REQUEST,
            &json_headers(),
            body,
        );

        assert_eq!(result.support, CodexRelayProbeSupport::Supported);
        assert_eq!(
            result.confidence,
            CodexRelayProbeConfidence::EndpointValidation
        );
    }

    #[test]
    fn codex_relay_probe_compact_unsupported_error_marks_endpoint_unsupported() {
        let body =
            br#"{"error":{"code":"compact_not_supported","message":"compact is not supported"}}"#;

        let result = classify_codex_relay_probe_response(
            &spec(CodexRelayProbeKind::ResponsesCompact),
            StatusCode::BAD_REQUEST,
            &json_headers(),
            body,
        );

        assert_eq!(result.support, CodexRelayProbeSupport::Unsupported);
    }

    #[test]
    fn codex_relay_probe_compact_not_found_marks_endpoint_unsupported() {
        let result = classify_codex_relay_probe_response(
            &spec(CodexRelayProbeKind::ResponsesCompact),
            StatusCode::NOT_FOUND,
            &json_headers(),
            br#"{"error":{"message":"not found"}}"#,
        );

        assert_eq!(result.support, CodexRelayProbeSupport::Unsupported);
    }

    #[tokio::test]
    async fn codex_relay_probe_executor_sends_single_validation_request_with_auth() {
        let hits = Arc::new(Mutex::new(0usize));
        let seen_authorization = Arc::new(Mutex::new(None::<String>));
        let seen_body = Arc::new(Mutex::new(None::<String>));

        let hits_for_route = hits.clone();
        let seen_authorization_for_route = seen_authorization.clone();
        let seen_body_for_route = seen_body.clone();
        let app = axum::Router::new().route(
            "/v1/responses/compact",
            post(move |request: Request<Body>| {
                let hits = hits_for_route.clone();
                let seen_authorization = seen_authorization_for_route.clone();
                let seen_body = seen_body_for_route.clone();
                async move {
                    *hits.lock().expect("lock hits") += 1;
                    *seen_authorization.lock().expect("lock auth") = request
                        .headers()
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .map(ToOwned::to_owned);
                    let body = axum::body::to_bytes(request.into_body(), 1024)
                        .await
                        .expect("body");
                    *seen_body.lock().expect("lock body") =
                        Some(String::from_utf8_lossy(body.as_ref()).into_owned());
                    (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": {
                                "type": "invalid_request_error",
                                "message": "Missing required parameter: model"
                            }
                        })),
                    )
                }
            }),
        );
        let (addr, handle) = spawn_axum_server(app);
        let mut upstream = upstream(format!("http://{addr}/v1"));
        upstream.auth.auth_token = Some("probe-token".to_string().into());

        let client =
            CodexRelayProbeClient::new(reqwest::Client::new(), captured_credential(&upstream));
        let result = client
            .probe_upstream(
                &upstream,
                &CodexRelayProbeSpec::for_kind(CodexRelayProbeKind::ResponsesCompact),
            )
            .await;

        assert_eq!(*hits.lock().expect("lock hits"), 1);
        assert_eq!(
            seen_authorization.lock().expect("lock auth").as_deref(),
            Some("Bearer probe-token")
        );
        assert_eq!(seen_body.lock().expect("lock body").as_deref(), Some("{}"));
        assert_eq!(result.support, CodexRelayProbeSupport::Supported);
        assert_eq!(
            result.confidence,
            CodexRelayProbeConfidence::EndpointValidation
        );

        handle.abort();
    }

    #[tokio::test]
    async fn codex_relay_probe_remote_target_requires_helper_auth_or_anonymous_opt_in() {
        let hits = Arc::new(Mutex::new(0usize));
        let hits_for_route = hits.clone();
        let app = axum::Router::new().route(
            "/v1/models",
            get(move || {
                let hits = hits_for_route.clone();
                async move {
                    *hits.lock().expect("lock hits") += 1;
                    Json(serde_json::json!({
                        "object": "list",
                        "data": [{ "id": "gpt-5.5", "object": "model" }]
                    }))
                }
            }),
        );
        let (addr, handle) = spawn_axum_server(app);
        let client = reqwest::Client::builder()
            .no_proxy()
            .resolve("relay.example", addr)
            .build()
            .expect("build probe client");
        let mut target = upstream(format!("http://relay.example:{}/v1", addr.port()));
        let client = CodexRelayProbeClient::new(client, captured_credential(&target));
        let spec = CodexRelayProbeSpec::for_kind(CodexRelayProbeKind::Models);

        let rejected = client.probe_upstream(&target, &spec).await;

        assert_eq!(*hits.lock().expect("lock hits"), 0);
        assert_eq!(rejected.support, CodexRelayProbeSupport::Unknown);
        assert_eq!(rejected.confidence, CodexRelayProbeConfidence::Credential);
        assert_eq!(
            rejected.credential_readiness,
            Some(CredentialReadinessCode::Missing)
        );
        assert_eq!(rejected.status_code, None);
        assert_eq!(rejected.reason, UPSTREAM_AUTH_UNAVAILABLE_REASON);

        target.auth.allow_anonymous = Some(true);
        let client =
            CodexRelayProbeClient::new(client.client.clone(), captured_credential(&target));
        let allowed = client.probe_upstream(&target, &spec).await;

        assert_eq!(*hits.lock().expect("lock hits"), 1);
        assert_eq!(allowed.support, CodexRelayProbeSupport::Supported);
        assert_eq!(
            allowed.credential_readiness,
            Some(CredentialReadinessCode::Ready)
        );

        handle.abort();
    }

    #[tokio::test]
    async fn codex_relay_probe_executor_targets_only_explicit_upstream() {
        let unused_hits = Arc::new(Mutex::new(0usize));
        let target_hits = Arc::new(Mutex::new(0usize));

        let unused_hits_for_route = unused_hits.clone();
        let unused_app = axum::Router::new().route(
            "/v1/models",
            get(move || {
                let unused_hits = unused_hits_for_route.clone();
                async move {
                    *unused_hits.lock().expect("lock unused hits") += 1;
                    Json(serde_json::json!({ "models": [{ "slug": "unused" }] }))
                }
            }),
        );
        let (unused_addr, unused_handle) = spawn_axum_server(unused_app);

        let target_hits_for_route = target_hits.clone();
        let target_app = axum::Router::new().route(
            "/v1/models",
            get(move || {
                let target_hits = target_hits_for_route.clone();
                async move {
                    *target_hits.lock().expect("lock target hits") += 1;
                    Json(serde_json::json!({ "models": [{ "slug": "gpt-5.5" }] }))
                }
            }),
        );
        let (target_addr, target_handle) = spawn_axum_server(target_app);

        let target = upstream(format!("http://{target_addr}/v1"));
        let client =
            CodexRelayProbeClient::new(reqwest::Client::new(), captured_credential(&target));
        let result = client
            .probe_upstream(
                &target,
                &CodexRelayProbeSpec::for_kind(CodexRelayProbeKind::Models),
            )
            .await;

        assert_ne!(unused_addr, target_addr);
        assert_eq!(*unused_hits.lock().expect("lock unused hits"), 0);
        assert_eq!(*target_hits.lock().expect("lock target hits"), 1);
        assert_eq!(result.support, CodexRelayProbeSupport::Supported);
        assert_eq!(result.response_shape.as_deref(), Some("codex_models"));

        unused_handle.abort();
        target_handle.abort();
    }

    #[tokio::test]
    async fn codex_relay_probe_executor_classifies_models_without_normal_proxy_side_effects() {
        let hits = Arc::new(Mutex::new(0usize));
        let hits_for_route = hits.clone();
        let app = axum::Router::new().route(
            "/v1/models",
            get(move || {
                let hits = hits_for_route.clone();
                async move {
                    *hits.lock().expect("lock hits") += 1;
                    Json(serde_json::json!({
                        "object": "list",
                        "data": [
                            { "id": "gpt-5.5", "object": "model" }
                        ]
                    }))
                }
            }),
        );
        let (addr, handle) = spawn_axum_server(app);
        let target = upstream(format!("http://{addr}/v1"));
        let client =
            CodexRelayProbeClient::new(reqwest::Client::new(), captured_credential(&target));

        let result = client
            .probe_upstream(
                &target,
                &CodexRelayProbeSpec::for_kind(CodexRelayProbeKind::Models),
            )
            .await;

        assert_eq!(*hits.lock().expect("lock hits"), 1);
        assert_eq!(result.support, CodexRelayProbeSupport::Supported);
        assert_eq!(result.response_shape.as_deref(), Some("openai_data_list"));
        assert!(result.translation_required);

        handle.abort();
    }

    #[test]
    fn codex_relay_probe_url_builder_avoids_double_v1_prefix() {
        let url = build_probe_url("https://relay.example/v1", "/v1/responses/compact")
            .expect("probe url");

        assert_eq!(url.as_str(), "https://relay.example/v1/responses/compact");
    }
}

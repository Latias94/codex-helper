use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http as tungstenite_http;

use crate::config::UpstreamConfig;
use crate::model_routing;

use super::classify::{ROUTING_MISMATCH_CAPABILITY_CLASS, classify_upstream_response};
use super::codex_relay_target::{CodexRelayTargetSelection, select_codex_relay_target};
use super::{ProxyControlError, ProxyService};

pub const CODEX_RELAY_LIVE_SMOKE_ACK: &str = "run-live-codex-relay-smoke";

const LIVE_SMOKE_API_VERSION: u32 = 1;
const MAX_LIVE_SMOKE_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const ERROR_SNIPPET_LIMIT: usize = 512;
const RESPONSES_WS_BETA_HEADER: &str = "responses_websockets=2026-02-06";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexRelayLiveSmokeRequest {
    #[serde(default)]
    pub acknowledgement: Option<String>,
    #[serde(default)]
    pub station_name: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub endpoint_id: Option<String>,
    #[serde(default)]
    pub upstream_index: Option<usize>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub cases: Vec<CodexRelayLiveSmokeCase>,
    #[serde(default)]
    pub service_tier: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexRelayLiveSmokeCase {
    ResponsesCompact,
    HostedImageGeneration,
    #[serde(rename = "responses_websocket", alias = "responses_web_socket")]
    ResponsesWebSocket,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexRelayLiveSmokeOutcome {
    Passed,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexRelayLiveSmokeConfidence {
    LiveOutputShape,
    LiveAccepted,
    LiveError,
    Transport,
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexRelayLiveSmokeSideEffect {
    LiveRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexRelayLiveSmokeResult {
    pub case: CodexRelayLiveSmokeCase,
    pub outcome: CodexRelayLiveSmokeOutcome,
    pub confidence: CodexRelayLiveSmokeConfidence,
    pub side_effect: CodexRelayLiveSmokeSideEffect,
    pub status_code: Option<u16>,
    pub response_shape: Option<String>,
    pub output_items_seen: usize,
    pub image_generation_call_seen: bool,
    pub image_result_present: bool,
    pub accepted_by_responses: bool,
    pub error_class: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexRelayLiveSmokeResponse {
    pub api_version: u32,
    pub service_name: String,
    pub station_name: String,
    pub upstream_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_endpoint_key: Option<String>,
    pub upstream_base_url: String,
    pub requested_model: String,
    pub upstream_model: String,
    pub cases: Vec<CodexRelayLiveSmokeCase>,
    pub results: Vec<CodexRelayLiveSmokeResult>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CodexRelayLiveSmokeClient {
    client: reqwest::Client,
}

impl CodexRelayLiveSmokeClient {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    async fn run_case(
        &self,
        upstream: &UpstreamConfig,
        model: &str,
        service_tier: Option<&str>,
        case: CodexRelayLiveSmokeCase,
    ) -> CodexRelayLiveSmokeResult {
        let descriptor = live_smoke_case_descriptor(case);
        match descriptor.executor {
            LiveSmokeExecutor::Http(_) => {
                self.run_http_case(upstream, model, service_tier, descriptor)
                    .await
            }
            LiveSmokeExecutor::WebSocket(_) => {
                self.run_websocket_case(upstream, model, service_tier, descriptor)
                    .await
            }
        }
    }

    async fn run_http_case(
        &self,
        upstream: &UpstreamConfig,
        model: &str,
        service_tier: Option<&str>,
        descriptor: &CodexRelayLiveSmokeCaseDescriptor,
    ) -> CodexRelayLiveSmokeResult {
        let spec = LiveSmokeSpec::for_case(descriptor.case, model, service_tier)
            .expect("HTTP live smoke descriptor must build an HTTP spec");
        let url = match build_live_smoke_url(&upstream.base_url, spec.path) {
            Ok(url) => url,
            Err(error) => return transport_result(descriptor.case, None, error),
        };

        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::ACCEPT_ENCODING,
            HeaderValue::from_static("identity"),
        );
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        if spec.stream {
            headers.insert(
                axum::http::header::ACCEPT,
                HeaderValue::from_static("text/event-stream, application/json"),
            );
        }
        apply_upstream_auth_headers(upstream, &mut headers);

        let response = match self
            .client
            .request(spec.method, url)
            .headers(headers)
            .timeout(spec.timeout)
            .json(&spec.body)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                return transport_result(
                    descriptor.case,
                    None,
                    format!("transport error during live smoke: {error}"),
                );
            }
        };

        let status = StatusCode::from_u16(response.status().as_u16())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let headers = response.headers().clone();
        let body = match read_limited_body(response, MAX_LIVE_SMOKE_RESPONSE_BYTES).await {
            Ok(body) => body,
            Err(error) => return transport_result(descriptor.case, Some(status), error),
        };
        (spec.classify)(status, &headers, body.as_ref())
    }

    async fn run_websocket_case(
        &self,
        upstream: &UpstreamConfig,
        model: &str,
        service_tier: Option<&str>,
        descriptor: &CodexRelayLiveSmokeCaseDescriptor,
    ) -> CodexRelayLiveSmokeResult {
        let LiveSmokeExecutor::WebSocket(ws) = descriptor.executor else {
            return transport_result(
                descriptor.case,
                None,
                "non-websocket live smoke case reached websocket executor",
            );
        };
        let url =
            match build_live_smoke_url(&upstream.base_url, ws.path).and_then(http_url_to_ws_url) {
                Ok(url) => url,
                Err(error) => return transport_result(descriptor.case, None, error),
            };

        let mut headers = HeaderMap::new();
        headers.insert("openai-beta", HeaderValue::from_static(ws.beta_header));
        apply_upstream_auth_headers(upstream, &mut headers);
        let request = match websocket_live_smoke_request(&url, &headers) {
            Ok(request) => request,
            Err(error) => return transport_result(descriptor.case, None, error),
        };

        let connect_result = tokio::time::timeout(
            std::time::Duration::from_secs(ws.handshake_timeout_secs),
            connect_async(request),
        )
        .await;
        let (mut socket, _) = match connect_result {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => return websocket_transport_error_result(descriptor.case, error),
            Err(_) => {
                return transport_result(
                    descriptor.case,
                    None,
                    "websocket live smoke handshake timed out",
                );
            }
        };

        let body = (ws.body)(model, service_tier);
        let message = TungsteniteMessage::Text(body.to_string().into());
        if let Err(error) = socket.send(message).await {
            return transport_result(
                descriptor.case,
                Some(StatusCode::SWITCHING_PROTOCOLS),
                format!("websocket live smoke send failed: {error}"),
            );
        }

        read_websocket_live_smoke_result(
            descriptor.case,
            socket,
            std::time::Duration::from_secs(ws.read_timeout_secs),
        )
        .await
    }
}

pub(super) async fn codex_relay_live_smoke_for_proxy(
    proxy: &ProxyService,
    payload: CodexRelayLiveSmokeRequest,
) -> Result<CodexRelayLiveSmokeResponse, ProxyControlError> {
    if proxy.service_name != "codex" {
        return Err(ProxyControlError::new(
            StatusCode::BAD_REQUEST,
            "Codex relay live smoke is only available for the codex service",
        ));
    }

    let cases = requested_cases(payload.cases);
    if live_smoke_cases_require_acknowledgement(&cases)
        && payload
            .acknowledgement
            .as_deref()
            .map(str::trim)
            .filter(|value| *value == CODEX_RELAY_LIVE_SMOKE_ACK)
            .is_none()
    {
        return Err(ProxyControlError::new(
            StatusCode::BAD_REQUEST,
            format!("live smoke requires acknowledgement '{CODEX_RELAY_LIVE_SMOKE_ACK}'"),
        ));
    }

    let requested_model = payload
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            ProxyControlError::new(
                StatusCode::BAD_REQUEST,
                "live smoke requires an explicit model",
            )
        })?;
    let service_tier = payload
        .service_tier
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let cfg = proxy.config.snapshot().await;
    let mgr = proxy.service_manager(cfg.as_ref());
    let target = select_codex_relay_target(
        mgr,
        CodexRelayTargetSelection {
            station_name: payload.station_name.as_deref(),
            upstream_index: payload.upstream_index,
            provider_id: payload.provider_id.as_deref(),
            endpoint_id: payload.endpoint_id.as_deref(),
        },
    )?;
    let upstream_model =
        model_routing::effective_model(&target.upstream.model_mapping, &requested_model);

    let client = CodexRelayLiveSmokeClient::new(proxy.client.clone());
    let mut results = Vec::with_capacity(cases.len());
    for case in cases.iter().copied() {
        results.push(
            client
                .run_case(
                    &target.upstream,
                    upstream_model.as_str(),
                    service_tier.as_deref(),
                    case,
                )
                .await,
        );
    }

    let warnings = live_smoke_warnings(&cases);

    let response = CodexRelayLiveSmokeResponse {
        api_version: LIVE_SMOKE_API_VERSION,
        service_name: proxy.service_name.to_string(),
        station_name: target.station_name,
        upstream_index: target.upstream_index,
        provider_id: target.provider_id,
        endpoint_id: target.endpoint_id,
        provider_endpoint_key: target.provider_endpoint_key,
        upstream_base_url: target.upstream.base_url,
        requested_model,
        upstream_model,
        cases,
        results,
        warnings,
    };
    if let Err(error) = super::codex_relay_evidence::append_codex_relay_live_smoke_evidence(
        &response,
        "proxy_service",
    ) {
        tracing::warn!("failed to write Codex relay live-smoke evidence: {}", error);
    }
    Ok(response)
}

fn requested_cases(cases: Vec<CodexRelayLiveSmokeCase>) -> Vec<CodexRelayLiveSmokeCase> {
    if cases.is_empty() {
        return codex_relay_live_smoke_cases()
            .iter()
            .filter(|descriptor| descriptor.default_enabled)
            .map(|descriptor| descriptor.case)
            .collect();
    }

    let mut out = Vec::new();
    for case in cases {
        if !out.contains(&case) {
            out.push(case);
        }
    }
    out
}

fn live_smoke_cases_require_acknowledgement(cases: &[CodexRelayLiveSmokeCase]) -> bool {
    cases
        .iter()
        .copied()
        .map(live_smoke_case_descriptor)
        .any(|descriptor| descriptor.acknowledgement_required)
}

fn live_smoke_warnings(cases: &[CodexRelayLiveSmokeCase]) -> Vec<String> {
    let mut warnings = vec![
        "live smoke sends real upstream requests and may consume tokens or credits".to_string(),
        "results do not update routing, affinity, passive health, balance, or retry state"
            .to_string(),
    ];
    for descriptor in codex_relay_live_smoke_cases() {
        if let Some(warning) = descriptor.explicit_only_warning
            && !cases.contains(&descriptor.case)
        {
            warnings.push(warning.to_string());
        }
    }
    warnings
}

#[derive(Debug, Clone, Copy)]
struct CodexRelayLiveSmokeCaseDescriptor {
    case: CodexRelayLiveSmokeCase,
    default_enabled: bool,
    acknowledgement_required: bool,
    explicit_only_warning: Option<&'static str>,
    executor: LiveSmokeExecutor,
}

#[derive(Debug, Clone, Copy)]
enum LiveSmokeExecutor {
    Http(LiveSmokeHttpDescriptor),
    WebSocket(LiveSmokeWebSocketDescriptor),
}

#[derive(Debug, Clone, Copy)]
struct LiveSmokeHttpDescriptor {
    method: &'static str,
    path: &'static str,
    stream: bool,
    timeout_secs: u64,
    body: fn(&str, Option<&str>) -> Value,
    classify: fn(StatusCode, &HeaderMap, &[u8]) -> CodexRelayLiveSmokeResult,
}

#[derive(Debug, Clone, Copy)]
struct LiveSmokeWebSocketDescriptor {
    path: &'static str,
    beta_header: &'static str,
    handshake_timeout_secs: u64,
    read_timeout_secs: u64,
    body: fn(&str, Option<&str>) -> Value,
}

const CODEX_RELAY_LIVE_SMOKE_CASES: &[CodexRelayLiveSmokeCaseDescriptor] = &[
    CodexRelayLiveSmokeCaseDescriptor {
        case: CodexRelayLiveSmokeCase::ResponsesCompact,
        default_enabled: true,
        acknowledgement_required: true,
        explicit_only_warning: None,
        executor: LiveSmokeExecutor::Http(LiveSmokeHttpDescriptor {
            method: "POST",
            path: "/responses/compact",
            stream: false,
            timeout_secs: 30,
            body: compact_live_smoke_body,
            classify: classify_compact_live_smoke_response,
        }),
    },
    CodexRelayLiveSmokeCaseDescriptor {
        case: CodexRelayLiveSmokeCase::HostedImageGeneration,
        default_enabled: false,
        acknowledgement_required: true,
        explicit_only_warning: Some(
            "hosted image generation was not tested because image smoke is explicit-only",
        ),
        executor: LiveSmokeExecutor::Http(LiveSmokeHttpDescriptor {
            method: "POST",
            path: "/responses",
            stream: true,
            timeout_secs: 60,
            body: image_generation_live_smoke_body,
            classify: classify_image_live_smoke_response,
        }),
    },
    CodexRelayLiveSmokeCaseDescriptor {
        case: CodexRelayLiveSmokeCase::ResponsesWebSocket,
        default_enabled: false,
        acknowledgement_required: true,
        explicit_only_warning: Some(
            "Responses WebSocket was not tested because websocket smoke is explicit-only",
        ),
        executor: LiveSmokeExecutor::WebSocket(LiveSmokeWebSocketDescriptor {
            path: "/responses",
            beta_header: RESPONSES_WS_BETA_HEADER,
            handshake_timeout_secs: 30,
            read_timeout_secs: 60,
            body: responses_websocket_live_smoke_body,
        }),
    },
];

fn codex_relay_live_smoke_cases() -> &'static [CodexRelayLiveSmokeCaseDescriptor] {
    CODEX_RELAY_LIVE_SMOKE_CASES
}

fn live_smoke_case_descriptor(
    case: CodexRelayLiveSmokeCase,
) -> &'static CodexRelayLiveSmokeCaseDescriptor {
    codex_relay_live_smoke_cases()
        .iter()
        .find(|descriptor| descriptor.case == case)
        .expect("Codex relay live smoke case must be registered")
}

struct LiveSmokeSpec {
    method: Method,
    path: &'static str,
    body: Value,
    stream: bool,
    timeout: std::time::Duration,
    classify: fn(StatusCode, &HeaderMap, &[u8]) -> CodexRelayLiveSmokeResult,
}

impl LiveSmokeSpec {
    fn for_case(
        case: CodexRelayLiveSmokeCase,
        model: &str,
        service_tier: Option<&str>,
    ) -> Option<Self> {
        let descriptor = live_smoke_case_descriptor(case);
        let LiveSmokeExecutor::Http(http) = descriptor.executor else {
            return None;
        };
        Some(Self {
            method: Method::from_bytes(http.method.as_bytes()).unwrap_or(Method::POST),
            path: http.path,
            body: (http.body)(model, service_tier),
            stream: http.stream,
            timeout: std::time::Duration::from_secs(http.timeout_secs),
            classify: http.classify,
        })
    }
}

fn compact_live_smoke_body(model: &str, service_tier: Option<&str>) -> Value {
    let mut body = json!({
        "model": model,
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [
                    {
                        "type": "input_text",
                        "text": "Codex relay live smoke: please compact this short diagnostic conversation."
                    }
                ]
            },
            {
                "type": "message",
                "role": "assistant",
                "content": [
                    {
                        "type": "output_text",
                        "text": "Diagnostic reply for live smoke."
                    }
                ]
            }
        ],
        "instructions": "Return a compacted Codex conversation history for this diagnostic request.",
        "tools": [],
        "parallel_tool_calls": false,
        "prompt_cache_key": "codex-helper-live-smoke"
    });
    if let Some(service_tier) = service_tier {
        body["service_tier"] = Value::String(service_tier.to_string());
    }
    body
}

fn image_generation_live_smoke_body(model: &str, service_tier: Option<&str>) -> Value {
    let mut body = json!({
        "model": model,
        "instructions": "You are running a Codex relay live smoke diagnostic. If available, use the hosted image_generation tool once.",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [
                    {
                        "type": "input_text",
                        "text": "For diagnostics, create a tiny simple blue square PNG. No text."
                    }
                ]
            }
        ],
        "tools": [
            {
                "type": "image_generation",
                "output_format": "png"
            }
        ],
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "store": false,
        "stream": true,
        "include": [],
        "prompt_cache_key": "codex-helper-live-smoke"
    });
    if let Some(service_tier) = service_tier {
        body["service_tier"] = Value::String(service_tier.to_string());
    }
    body
}

fn responses_websocket_live_smoke_body(model: &str, service_tier: Option<&str>) -> Value {
    let mut body = json!({
        "type": "response.create",
        "model": model,
        "instructions": "You are running a Codex relay Responses WebSocket live smoke diagnostic. Reply with exactly OK.",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [
                    {
                        "type": "input_text",
                        "text": "Codex relay Responses WebSocket live smoke. Reply OK."
                    }
                ]
            }
        ],
        "tools": [],
        "parallel_tool_calls": false,
        "store": false,
        "stream": true,
        "prompt_cache_key": "codex-helper-live-smoke"
    });
    if let Some(service_tier) = service_tier {
        body["service_tier"] = Value::String(service_tier.to_string());
    }
    body
}

async fn read_websocket_live_smoke_result(
    case: CodexRelayLiveSmokeCase,
    mut socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    timeout: std::time::Duration,
) -> CodexRelayLiveSmokeResult {
    let read_result = tokio::time::timeout(timeout, async {
        loop {
            let Some(message) = socket.next().await else {
                return base_result(
                    case,
                    CodexRelayLiveSmokeOutcome::Unknown,
                    CodexRelayLiveSmokeConfidence::Transport,
                    Some(StatusCode::SWITCHING_PROTOCOLS.as_u16()),
                    "websocket live smoke closed before any response event",
                );
            };
            let message = match message {
                Ok(message) => message,
                Err(error) => {
                    return transport_result(
                        case,
                        Some(StatusCode::SWITCHING_PROTOCOLS),
                        format!("websocket live smoke read failed: {error}"),
                    );
                }
            };
            match message {
                TungsteniteMessage::Text(text) => {
                    return classify_websocket_live_smoke_message_for_case(case, text.as_bytes());
                }
                TungsteniteMessage::Binary(bytes) => {
                    return classify_websocket_live_smoke_message_for_case(case, bytes.as_ref());
                }
                TungsteniteMessage::Ping(payload) => {
                    let _ = socket.send(TungsteniteMessage::Pong(payload)).await;
                }
                TungsteniteMessage::Pong(_) => {}
                TungsteniteMessage::Close(frame) => {
                    let reason = frame
                        .map(|frame| {
                            if frame.reason.is_empty() {
                                format!("websocket live smoke closed with code {}", frame.code)
                            } else {
                                format!(
                                    "websocket live smoke closed with code {}: {}",
                                    frame.code, frame.reason
                                )
                            }
                        })
                        .unwrap_or_else(|| "websocket live smoke closed".to_string());
                    return base_result(
                        case,
                        CodexRelayLiveSmokeOutcome::Unknown,
                        CodexRelayLiveSmokeConfidence::Transport,
                        Some(StatusCode::SWITCHING_PROTOCOLS.as_u16()),
                        reason,
                    );
                }
                TungsteniteMessage::Frame(_) => {}
            }
        }
    })
    .await;

    match read_result {
        Ok(result) => result,
        Err(_) => transport_result(
            case,
            Some(StatusCode::SWITCHING_PROTOCOLS),
            "websocket live smoke timed out waiting for a response event",
        ),
    }
}

fn classify_websocket_live_smoke_message_for_case(
    case: CodexRelayLiveSmokeCase,
    body: &[u8],
) -> CodexRelayLiveSmokeResult {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return base_result(
            case,
            CodexRelayLiveSmokeOutcome::Unknown,
            CodexRelayLiveSmokeConfidence::Malformed,
            Some(StatusCode::SWITCHING_PROTOCOLS.as_u16()),
            "websocket live smoke returned a non-JSON data frame",
        );
    };

    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("<missing type>");
    let event_message = websocket_error_event_message(&value);
    let mut result = if matches!(
        event_type,
        "error" | "response.failed" | "response.incomplete"
    ) {
        let mut result = base_result(
            case,
            CodexRelayLiveSmokeOutcome::Failed,
            CodexRelayLiveSmokeConfidence::LiveError,
            Some(StatusCode::SWITCHING_PROTOCOLS.as_u16()),
            match event_message {
                Some(message) => format!("responses websocket returned {event_type}: {message}"),
                None => format!("responses websocket returned {event_type}"),
            },
        );
        result.error_class = Some("websocket_error_event".to_string());
        result
    } else if event_type.starts_with("response.") || event_type == "codex.rate_limits" {
        base_result(
            case,
            CodexRelayLiveSmokeOutcome::Passed,
            CodexRelayLiveSmokeConfidence::LiveAccepted,
            Some(StatusCode::SWITCHING_PROTOCOLS.as_u16()),
            format!("responses websocket accepted response.create and returned {event_type}"),
        )
    } else {
        base_result(
            case,
            CodexRelayLiveSmokeOutcome::Unknown,
            CodexRelayLiveSmokeConfidence::Malformed,
            Some(StatusCode::SWITCHING_PROTOCOLS.as_u16()),
            format!("websocket live smoke returned unexpected event type {event_type}"),
        )
    };
    result.response_shape = Some(event_type.to_string());
    result.output_items_seen = count_output_items(&value);
    result.accepted_by_responses =
        event_type.starts_with("response.") || event_type == "codex.rate_limits";
    result
}

fn websocket_error_event_message(value: &Value) -> Option<String> {
    [
        "message",
        "error.message",
        "error.code",
        "response.status",
        "response.status_details.error.message",
    ]
    .iter()
    .find_map(|path| json_string_path(value, path))
    .map(|message| sanitized_error_snippet(message.as_bytes()))
    .filter(|message| !message.is_empty())
}

fn json_string_path(value: &Value, path: &str) -> Option<String> {
    let mut current = value;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    current
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| (!current.is_null()).then(|| current.to_string()))
}

fn classify_compact_live_smoke_response(
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
) -> CodexRelayLiveSmokeResult {
    if !status.is_success() {
        return classify_live_smoke_error(
            CodexRelayLiveSmokeCase::ResponsesCompact,
            status,
            headers,
            body,
        );
    }

    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return base_result(
            CodexRelayLiveSmokeCase::ResponsesCompact,
            CodexRelayLiveSmokeOutcome::Unknown,
            CodexRelayLiveSmokeConfidence::Malformed,
            Some(status.as_u16()),
            "compact live smoke returned non-JSON or malformed JSON",
        );
    };
    let Some(output) = value.get("output").and_then(Value::as_array) else {
        return base_result(
            CodexRelayLiveSmokeCase::ResponsesCompact,
            CodexRelayLiveSmokeOutcome::Unknown,
            CodexRelayLiveSmokeConfidence::Malformed,
            Some(status.as_u16()),
            "compact live smoke JSON did not contain an output array",
        );
    };

    let mut result = base_result(
        CodexRelayLiveSmokeCase::ResponsesCompact,
        CodexRelayLiveSmokeOutcome::Passed,
        CodexRelayLiveSmokeConfidence::LiveOutputShape,
        Some(status.as_u16()),
        "compact endpoint returned a live output array",
    );
    result.output_items_seen = output.len();
    result.response_shape = Some(if output.iter().any(value_mentions_compaction_item) {
        "compact_output_compaction_item".to_string()
    } else {
        "compact_output".to_string()
    });
    result
}

fn classify_image_live_smoke_response(
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
) -> CodexRelayLiveSmokeResult {
    if !status.is_success() {
        return classify_live_smoke_error(
            CodexRelayLiveSmokeCase::HostedImageGeneration,
            status,
            headers,
            body,
        );
    }

    let values = parse_response_values(headers, body);
    if values.is_empty() {
        return base_result(
            CodexRelayLiveSmokeCase::HostedImageGeneration,
            CodexRelayLiveSmokeOutcome::Unknown,
            CodexRelayLiveSmokeConfidence::Malformed,
            Some(status.as_u16()),
            "image live smoke returned success but no parseable JSON or SSE data",
        );
    }

    let output_items_seen = values
        .iter()
        .map(count_output_items)
        .max()
        .unwrap_or_default();
    let image_generation_call_seen = values.iter().any(value_mentions_image_generation_call);
    let image_result_present = values.iter().any(value_mentions_image_result);

    let mut result = if image_generation_call_seen {
        base_result(
            CodexRelayLiveSmokeCase::HostedImageGeneration,
            CodexRelayLiveSmokeOutcome::Passed,
            CodexRelayLiveSmokeConfidence::LiveOutputShape,
            Some(status.as_u16()),
            "responses endpoint returned a hosted image_generation_call",
        )
    } else {
        base_result(
            CodexRelayLiveSmokeCase::HostedImageGeneration,
            CodexRelayLiveSmokeOutcome::Unknown,
            CodexRelayLiveSmokeConfidence::LiveAccepted,
            Some(status.as_u16()),
            "responses endpoint accepted the hosted image_generation request but did not return an image_generation_call",
        )
    };
    result.response_shape = Some(if image_generation_call_seen {
        "image_generation_call".to_string()
    } else {
        "responses_success".to_string()
    });
    result.output_items_seen = output_items_seen;
    result.image_generation_call_seen = image_generation_call_seen;
    result.image_result_present = image_result_present;
    result.accepted_by_responses = true;
    result
}

fn classify_live_smoke_error(
    case: CodexRelayLiveSmokeCase,
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
) -> CodexRelayLiveSmokeResult {
    let (error_class, _, _) = classify_upstream_response(status.as_u16(), headers, body);
    let mut result = base_result(
        case,
        CodexRelayLiveSmokeOutcome::Failed,
        CodexRelayLiveSmokeConfidence::LiveError,
        Some(status.as_u16()),
        live_smoke_error_reason(case, status, body),
    );
    result.error_class = error_class;
    result.response_shape = Some(if body_mentions_unsupported(case, body) {
        "unsupported_capability_error".to_string()
    } else if result.error_class.as_deref() == Some(ROUTING_MISMATCH_CAPABILITY_CLASS) {
        "routing_capability_mismatch".to_string()
    } else if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        "auth_or_entitlement_error".to_string()
    } else {
        "error_response".to_string()
    });
    result
}

fn live_smoke_error_reason(
    case: CodexRelayLiveSmokeCase,
    status: StatusCode,
    body: &[u8],
) -> String {
    let prefix = match case {
        CodexRelayLiveSmokeCase::ResponsesCompact => "compact live smoke failed",
        CodexRelayLiveSmokeCase::HostedImageGeneration => "image live smoke failed",
        CodexRelayLiveSmokeCase::ResponsesWebSocket => "responses websocket live smoke failed",
    };
    let snippet = sanitized_error_snippet(body);
    if snippet.is_empty() {
        return format!("{prefix} with HTTP {}", status.as_u16());
    }
    format!("{prefix} with HTTP {}: {snippet}", status.as_u16())
}

fn body_mentions_unsupported(case: CodexRelayLiveSmokeCase, body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    let capability_terms = match case {
        CodexRelayLiveSmokeCase::ResponsesCompact => {
            text.contains("compact") || text.contains("compaction")
        }
        CodexRelayLiveSmokeCase::HostedImageGeneration => {
            text.contains("image_generation") || text.contains("image generation")
        }
        CodexRelayLiveSmokeCase::ResponsesWebSocket => {
            text.contains("websocket") || text.contains("responses_websockets")
        }
    };
    capability_terms
        && (text.contains("unsupported")
            || text.contains("not supported")
            || text.contains("not implemented")
            || text.contains("not available")
            || text.contains("unknown tool"))
}

fn parse_response_values(headers: &HeaderMap, body: &[u8]) -> Vec<Value> {
    if let Ok(value) = serde_json::from_slice::<Value>(body) {
        return vec![value];
    }
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let text = String::from_utf8_lossy(body);
    if content_type.contains("text/event-stream") || text.contains("data:") {
        return parse_sse_data_values(text.as_ref());
    }
    Vec::new()
}

fn parse_sse_data_values(text: &str) -> Vec<Value> {
    let normalized = text.replace("\r\n", "\n");
    normalized
        .split("\n\n")
        .filter_map(|event| {
            let mut data = String::new();
            for line in event.lines() {
                let Some(rest) = line.strip_prefix("data:") else {
                    continue;
                };
                let chunk = rest.trim_start();
                if chunk == "[DONE]" {
                    return None;
                }
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(chunk);
            }
            if data.is_empty() {
                return None;
            }
            serde_json::from_str::<Value>(&data).ok()
        })
        .collect()
}

fn count_output_items(value: &Value) -> usize {
    if let Some(output) = value.get("output").and_then(Value::as_array) {
        return output.len();
    }
    if value
        .get("item")
        .and_then(|item| item.get("type"))
        .and_then(Value::as_str)
        .is_some()
    {
        return 1;
    }
    0
}

fn value_mentions_compaction_item(value: &Value) -> bool {
    value_mentions_type(value, "compaction") || value_mentions_type(value, "context_compaction")
}

fn value_mentions_image_generation_call(value: &Value) -> bool {
    value_mentions_type(value, "image_generation_call")
}

fn value_mentions_image_result(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            let is_image_call = map
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value == "image_generation_call");
            if is_image_call
                && map
                    .get("result")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty())
            {
                return true;
            }
            map.values().any(value_mentions_image_result)
        }
        Value::Array(items) => items.iter().any(value_mentions_image_result),
        _ => false,
    }
}

fn value_mentions_type(value: &Value, expected_type: &str) -> bool {
    match value {
        Value::Object(map) => {
            if map
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value == expected_type)
            {
                return true;
            }
            map.values()
                .any(|value| value_mentions_type(value, expected_type))
        }
        Value::Array(items) => items
            .iter()
            .any(|value| value_mentions_type(value, expected_type)),
        _ => false,
    }
}

fn sanitized_error_snippet(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let mut out = String::new();
    for ch in text.chars() {
        if out.len() >= ERROR_SNIPPET_LIMIT {
            out.push_str("...");
            break;
        }
        if ch.is_control() && ch != '\n' && ch != '\r' && ch != '\t' {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

fn base_result(
    case: CodexRelayLiveSmokeCase,
    outcome: CodexRelayLiveSmokeOutcome,
    confidence: CodexRelayLiveSmokeConfidence,
    status_code: Option<u16>,
    reason: impl Into<String>,
) -> CodexRelayLiveSmokeResult {
    CodexRelayLiveSmokeResult {
        case,
        outcome,
        confidence,
        side_effect: CodexRelayLiveSmokeSideEffect::LiveRequest,
        status_code,
        response_shape: None,
        output_items_seen: 0,
        image_generation_call_seen: false,
        image_result_present: false,
        accepted_by_responses: false,
        error_class: None,
        reason: reason.into(),
    }
}

fn transport_result(
    case: CodexRelayLiveSmokeCase,
    status: Option<StatusCode>,
    reason: impl Into<String>,
) -> CodexRelayLiveSmokeResult {
    base_result(
        case,
        CodexRelayLiveSmokeOutcome::Unknown,
        CodexRelayLiveSmokeConfidence::Transport,
        status.map(|status| status.as_u16()),
        reason,
    )
}

fn apply_upstream_auth_headers(upstream: &UpstreamConfig, headers: &mut HeaderMap) {
    if let Some(token) = upstream.auth.resolve_auth_token()
        && let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}"))
    {
        headers.insert(axum::http::header::AUTHORIZATION, value);
    }
    if let Some(key) = upstream.auth.resolve_api_key()
        && let Ok(value) = HeaderValue::from_str(&key)
    {
        headers.insert("x-api-key", value);
    }
}

fn websocket_transport_error_result(
    case: CodexRelayLiveSmokeCase,
    error: tungstenite::Error,
) -> CodexRelayLiveSmokeResult {
    match error {
        tungstenite::Error::Http(response) => {
            let status = StatusCode::from_u16(response.status().as_u16()).ok();
            let body = response
                .body()
                .as_deref()
                .map(|body| format!(": {}", sanitized_error_snippet(body)))
                .unwrap_or_default();
            let mut result = classify_live_smoke_error(
                case,
                status.unwrap_or(StatusCode::BAD_GATEWAY),
                &HeaderMap::new(),
                body.as_bytes(),
            );
            result.status_code = status.map(|status| status.as_u16());
            result.reason = format!(
                "websocket live smoke handshake failed with HTTP {}{}",
                response.status().as_u16(),
                body
            );
            result
        }
        other => transport_result(
            case,
            None,
            format!("websocket live smoke transport error: {other}"),
        ),
    }
}

fn build_live_smoke_url(base_url: &str, path: &str) -> Result<reqwest::Url, String> {
    let base = base_url.trim_end_matches('/');
    let base_url =
        reqwest::Url::parse(base).map_err(|error| format!("invalid upstream base_url: {error}"))?;
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
    reqwest::Url::parse(&full).map_err(|error| format!("invalid live smoke url: {error}"))
}

fn http_url_to_ws_url(mut url: reqwest::Url) -> Result<reqwest::Url, String> {
    let scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        other => {
            return Err(format!(
                "unsupported websocket live smoke base_url scheme '{other}'"
            ));
        }
    };
    url.set_scheme(scheme)
        .map_err(|_| format!("failed to convert live smoke url to {scheme}"))?;
    Ok(url)
}

fn websocket_live_smoke_request(
    url: &reqwest::Url,
    headers: &HeaderMap,
) -> Result<tungstenite_http::Request<()>, String> {
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|error| format!("invalid websocket live smoke request: {error}"))?;
    for (name, value) in headers {
        let name = tungstenite_http::HeaderName::from_bytes(name.as_str().as_bytes())
            .map_err(|error| format!("invalid websocket live smoke header name: {error}"))?;
        let value = tungstenite_http::HeaderValue::from_bytes(value.as_bytes())
            .map_err(|error| format!("invalid websocket live smoke header value: {error}"))?;
        request.headers_mut().insert(name, value);
    }
    Ok(request)
}

async fn read_limited_body(response: reqwest::Response, max_bytes: usize) -> Result<Bytes, String> {
    let mut stream = response.bytes_stream();
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("read live smoke body: {error}"))?;
        if out.len() + chunk.len() > max_bytes {
            return Err(format!(
                "live smoke response body exceeded {max_bytes} bytes"
            ));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(out))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::Json;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::post;
    use futures_util::{SinkExt, StreamExt as _};
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
    use tokio_tungstenite::tungstenite::handshake::server::{
        Request as WsRequest, Response as WsResponse,
    };

    use super::*;
    use crate::config::{
        ProviderConfigV4, ProxyConfig, ProxyConfigV4, RetryConfig, RoutingConfigV4, ServiceConfig,
        ServiceConfigManager, ServiceViewV4, UiConfig, UpstreamAuth, UpstreamConfig,
    };
    use crate::lb::LbState;

    fn spawn_axum_server(app: axum::Router) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        listener.set_nonblocking(true).expect("nonblocking");
        let listener = tokio::net::TcpListener::from_std(listener).expect("to tokio listener");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve live smoke test");
        });
        (addr, handle)
    }

    #[derive(Debug, Default)]
    struct CapturedWebSocketSmoke {
        path: Option<String>,
        beta: Option<String>,
        authorization: Option<String>,
        api_key: Option<String>,
        first_message: Option<Value>,
    }

    #[allow(clippy::result_large_err)]
    fn spawn_websocket_server(
        captured: Arc<Mutex<CapturedWebSocketSmoke>>,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ws");
        let addr = listener.local_addr().expect("local_addr");
        listener.set_nonblocking(true).expect("nonblocking");
        let listener = tokio::net::TcpListener::from_std(listener).expect("to tokio listener");
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept ws");
            let captured_for_callback = captured.clone();
            let mut socket = tokio_tungstenite::accept_hdr_async(
                stream,
                move |request: &WsRequest, response: WsResponse| {
                    let mut captured = captured_for_callback.lock().expect("lock captured");
                    captured.path = Some(request.uri().path().to_string());
                    captured.beta = request
                        .headers()
                        .get("openai-beta")
                        .and_then(|value| value.to_str().ok())
                        .map(ToOwned::to_owned);
                    captured.authorization = request
                        .headers()
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .map(ToOwned::to_owned);
                    captured.api_key = request
                        .headers()
                        .get("x-api-key")
                        .and_then(|value| value.to_str().ok())
                        .map(ToOwned::to_owned);
                    Ok(response)
                },
            )
            .await
            .expect("accept websocket handshake");

            if let Some(Ok(message)) = socket.next().await {
                let value = match message {
                    TungsteniteMessage::Text(text) => {
                        serde_json::from_str::<Value>(&text).expect("json ws text")
                    }
                    TungsteniteMessage::Binary(bytes) => {
                        serde_json::from_slice::<Value>(&bytes).expect("json ws binary")
                    }
                    other => panic!("unexpected first ws message: {other:?}"),
                };
                captured.lock().expect("lock captured").first_message = Some(value);
            }

            socket
                .send(TungsteniteMessage::Text(
                    json!({
                        "type": "response.created",
                        "response": { "id": "resp_ws_smoke" }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .expect("send response.created");
            socket
                .send(TungsteniteMessage::Text(
                    json!({
                        "type": "response.completed",
                        "response": {
                            "id": "resp_ws_smoke",
                            "output": [
                                {
                                    "type": "message",
                                    "role": "assistant",
                                    "content": [
                                        { "type": "output_text", "text": "OK" }
                                    ]
                                }
                            ]
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .expect("send response.completed");
            let _ = socket.close(None).await;
        });
        (addr, handle)
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

    fn proxy_for_upstreams(upstreams: Vec<UpstreamConfig>) -> ProxyService {
        let mut mgr = ServiceConfigManager {
            active: Some("test".to_string()),
            ..Default::default()
        };
        mgr.configs.insert(
            "test".to_string(),
            ServiceConfig {
                name: "test".to_string(),
                alias: None,
                enabled: true,
                level: 1,
                upstreams,
            },
        );
        let cfg = ProxyConfig {
            version: Some(1),
            codex: mgr,
            claude: ServiceConfigManager::default(),
            retry: RetryConfig::default(),
            notify: Default::default(),
            default_service: None,
            ui: UiConfig::default(),
        };
        ProxyService::new(
            reqwest::Client::new(),
            Arc::new(cfg),
            "codex",
            Arc::new(Mutex::new(HashMap::<String, LbState>::new())),
        )
    }

    fn proxy_for_v4_providers(providers: Vec<(&str, String)>) -> ProxyService {
        let v4 = ProxyConfigV4 {
            codex: ServiceViewV4 {
                providers: providers
                    .iter()
                    .map(|(provider_id, base_url)| {
                        (
                            (*provider_id).to_string(),
                            ProviderConfigV4 {
                                base_url: Some(base_url.clone()),
                                inline_auth: UpstreamAuth::default(),
                                ..ProviderConfigV4::default()
                            },
                        )
                    })
                    .collect(),
                routing: Some(RoutingConfigV4::ordered_failover(
                    providers
                        .iter()
                        .map(|(provider_id, _)| (*provider_id).to_string())
                        .collect(),
                )),
                ..ServiceViewV4::default()
            },
            ..ProxyConfigV4::default()
        };
        let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4 runtime");
        ProxyService::new_with_v4_source(
            reqwest::Client::new(),
            Arc::new(runtime),
            Some(Arc::new(v4)),
            "codex",
            Arc::new(Mutex::new(HashMap::<String, LbState>::new())),
        )
    }

    fn request(model: &str, cases: Vec<CodexRelayLiveSmokeCase>) -> CodexRelayLiveSmokeRequest {
        CodexRelayLiveSmokeRequest {
            acknowledgement: Some(CODEX_RELAY_LIVE_SMOKE_ACK.to_string()),
            model: Some(model.to_string()),
            cases,
            ..Default::default()
        }
    }

    #[test]
    fn codex_relay_live_smoke_registry_preserves_default_and_explicit_cases() {
        let cases = codex_relay_live_smoke_cases();

        assert_eq!(
            cases
                .iter()
                .map(|descriptor| descriptor.case)
                .collect::<Vec<_>>(),
            vec![
                CodexRelayLiveSmokeCase::ResponsesCompact,
                CodexRelayLiveSmokeCase::HostedImageGeneration,
                CodexRelayLiveSmokeCase::ResponsesWebSocket,
            ]
        );
        assert_eq!(
            requested_cases(Vec::new()),
            vec![CodexRelayLiveSmokeCase::ResponsesCompact]
        );
        assert!(
            cases
                .iter()
                .all(|descriptor| descriptor.acknowledgement_required)
        );
        assert_eq!(
            live_smoke_warnings(&[CodexRelayLiveSmokeCase::ResponsesCompact]),
            vec![
                "live smoke sends real upstream requests and may consume tokens or credits",
                "results do not update routing, affinity, passive health, balance, or retry state",
                "hosted image generation was not tested because image smoke is explicit-only",
                "Responses WebSocket was not tested because websocket smoke is explicit-only",
            ]
        );
    }

    #[test]
    fn codex_relay_live_smoke_http_registry_preserves_wire_specs() {
        let compact = LiveSmokeSpec::for_case(
            CodexRelayLiveSmokeCase::ResponsesCompact,
            "gpt-5.5",
            Some("flex"),
        )
        .expect("compact HTTP spec");
        assert_eq!(compact.method, Method::POST);
        assert_eq!(compact.path, "/responses/compact");
        assert!(!compact.stream);
        assert_eq!(compact.timeout, std::time::Duration::from_secs(30));
        assert_eq!(compact.body["model"].as_str(), Some("gpt-5.5"));
        assert_eq!(compact.body["service_tier"].as_str(), Some("flex"));
        assert!(compact.body.get("stream").is_none());

        let image = LiveSmokeSpec::for_case(
            CodexRelayLiveSmokeCase::HostedImageGeneration,
            "gpt-5.5",
            None,
        )
        .expect("image HTTP spec");
        assert_eq!(image.method, Method::POST);
        assert_eq!(image.path, "/responses");
        assert!(image.stream);
        assert_eq!(image.timeout, std::time::Duration::from_secs(60));
        assert_eq!(image.body["stream"].as_bool(), Some(true));
        assert_eq!(
            image.body["tools"],
            json!([{ "type": "image_generation", "output_format": "png" }])
        );

        assert!(
            LiveSmokeSpec::for_case(CodexRelayLiveSmokeCase::ResponsesWebSocket, "gpt-5.5", None)
                .is_none()
        );
    }

    #[test]
    fn codex_relay_live_smoke_default_cases_exclude_image_generation() {
        assert_eq!(
            requested_cases(Vec::new()),
            vec![CodexRelayLiveSmokeCase::ResponsesCompact]
        );
    }

    #[test]
    fn codex_relay_live_smoke_classifies_compact_output_shape() {
        let result = classify_compact_live_smoke_response(
            StatusCode::OK,
            &HeaderMap::new(),
            br#"{"output":[{"type":"compaction","encrypted_content":"summary"}]}"#,
        );

        assert_eq!(result.outcome, CodexRelayLiveSmokeOutcome::Passed);
        assert_eq!(
            result.confidence,
            CodexRelayLiveSmokeConfidence::LiveOutputShape
        );
        assert_eq!(
            result.response_shape.as_deref(),
            Some("compact_output_compaction_item")
        );
        assert_eq!(result.output_items_seen, 1);
    }

    #[test]
    fn codex_relay_live_smoke_classifies_image_generation_sse() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let body = concat!(
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"status\":\"completed\",\"result\":\"Zm9v\"}}\n\n",
            "data: [DONE]\n\n",
        );

        let result = classify_image_live_smoke_response(StatusCode::OK, &headers, body.as_bytes());

        assert_eq!(result.outcome, CodexRelayLiveSmokeOutcome::Passed);
        assert!(result.accepted_by_responses);
        assert!(result.image_generation_call_seen);
        assert!(result.image_result_present);
        assert_eq!(
            result.response_shape.as_deref(),
            Some("image_generation_call")
        );
    }

    #[test]
    fn codex_relay_live_smoke_websocket_error_includes_message() {
        let result = classify_websocket_live_smoke_message_for_case(
            CodexRelayLiveSmokeCase::ResponsesWebSocket,
            br#"{"type":"error","error":{"code":"quota_exceeded","message":"daily limit exceeded"}}"#,
        );

        assert_eq!(result.outcome, CodexRelayLiveSmokeOutcome::Failed);
        assert_eq!(result.error_class.as_deref(), Some("websocket_error_event"));
        assert!(result.reason.contains("daily limit exceeded"));
    }

    #[test]
    fn codex_relay_live_smoke_websocket_rate_limits_prove_accepted_stream() {
        let result = classify_websocket_live_smoke_message_for_case(
            CodexRelayLiveSmokeCase::ResponsesWebSocket,
            br#"{"type":"codex.rate_limits","rate_limits":{"allowed":true}}"#,
        );

        assert_eq!(result.outcome, CodexRelayLiveSmokeOutcome::Passed);
        assert_eq!(
            result.confidence,
            CodexRelayLiveSmokeConfidence::LiveAccepted
        );
        assert!(result.accepted_by_responses);
        assert_eq!(result.response_shape.as_deref(), Some("codex.rate_limits"));
    }

    #[tokio::test]
    async fn codex_relay_live_smoke_rejects_missing_ack_before_upstream_io() {
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_for_route = hits.clone();
        let upstream_app = axum::Router::new().route(
            "/v1/responses/compact",
            post(move || {
                let hits = hits_for_route.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    Json(json!({ "output": [] }))
                }
            }),
        );
        let (upstream_addr, upstream_handle) = spawn_axum_server(upstream_app);
        let proxy = proxy_for_upstreams(vec![upstream(format!("http://{upstream_addr}/v1"))]);

        let error = codex_relay_live_smoke_for_proxy(
            &proxy,
            CodexRelayLiveSmokeRequest {
                model: Some("gpt-5.5".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect_err("missing ack should fail");

        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(hits.load(Ordering::SeqCst), 0);

        upstream_handle.abort();
    }

    #[tokio::test]
    async fn codex_relay_live_smoke_compact_sends_single_request_with_codex_shape_and_auth() {
        let hits = Arc::new(AtomicUsize::new(0));
        let seen_body = Arc::new(Mutex::new(None::<Value>));
        let seen_authorization = Arc::new(Mutex::new(None::<String>));
        let seen_api_key = Arc::new(Mutex::new(None::<String>));

        let hits_for_route = hits.clone();
        let seen_body_for_route = seen_body.clone();
        let seen_authorization_for_route = seen_authorization.clone();
        let seen_api_key_for_route = seen_api_key.clone();
        let upstream_app = axum::Router::new().route(
            "/v1/responses/compact",
            post(move |request: Request<Body>| {
                let hits = hits_for_route.clone();
                let seen_body = seen_body_for_route.clone();
                let seen_authorization = seen_authorization_for_route.clone();
                let seen_api_key = seen_api_key_for_route.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    *seen_authorization.lock().expect("lock auth") = request
                        .headers()
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .map(ToOwned::to_owned);
                    *seen_api_key.lock().expect("lock api key") = request
                        .headers()
                        .get("x-api-key")
                        .and_then(|value| value.to_str().ok())
                        .map(ToOwned::to_owned);
                    let body = axum::body::to_bytes(request.into_body(), 16 * 1024)
                        .await
                        .expect("body");
                    let body: Value = serde_json::from_slice(body.as_ref()).expect("json body");
                    *seen_body.lock().expect("lock body") = Some(body);
                    Json(json!({
                        "output": [
                            { "type": "compaction", "encrypted_content": "summary" }
                        ]
                    }))
                }
            }),
        );
        let (upstream_addr, upstream_handle) = spawn_axum_server(upstream_app);
        let mut upstream = upstream(format!("http://{upstream_addr}/v1"));
        upstream.auth.auth_token = Some("live-token".to_string());
        upstream.auth.api_key = Some("live-api-key".to_string());
        let proxy = proxy_for_upstreams(vec![upstream]);

        let response = codex_relay_live_smoke_for_proxy(&proxy, request("gpt-5.5", Vec::new()))
            .await
            .expect("live smoke");

        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            seen_authorization.lock().expect("lock auth").as_deref(),
            Some("Bearer live-token")
        );
        assert_eq!(
            seen_api_key.lock().expect("lock api key").as_deref(),
            Some("live-api-key")
        );
        let body = seen_body
            .lock()
            .expect("lock body")
            .clone()
            .expect("captured body");
        assert_eq!(body["model"].as_str(), Some("gpt-5.5"));
        assert!(
            body["input"]
                .as_array()
                .is_some_and(|items| items.len() == 2)
        );
        assert_eq!(body["tools"], json!([]));
        assert_eq!(body["parallel_tool_calls"].as_bool(), Some(false));
        assert!(body.get("stream").is_none());
        assert!(body.get("tool_choice").is_none());
        assert_eq!(
            response.cases,
            vec![CodexRelayLiveSmokeCase::ResponsesCompact]
        );
        assert_eq!(response.results.len(), 1);
        assert_eq!(
            response.results[0].outcome,
            CodexRelayLiveSmokeOutcome::Passed
        );

        upstream_handle.abort();
    }

    #[tokio::test]
    async fn codex_relay_live_smoke_image_request_includes_hosted_tool() {
        let hits = Arc::new(AtomicUsize::new(0));
        let seen_body = Arc::new(Mutex::new(None::<Value>));

        let hits_for_route = hits.clone();
        let seen_body_for_route = seen_body.clone();
        let upstream_app = axum::Router::new().route(
            "/v1/responses",
            post(move |request: Request<Body>| {
                let hits = hits_for_route.clone();
                let seen_body = seen_body_for_route.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    let body = axum::body::to_bytes(request.into_body(), 16 * 1024)
                        .await
                        .expect("body");
                    let body: Value = serde_json::from_slice(body.as_ref()).expect("json body");
                    *seen_body.lock().expect("lock body") = Some(body);
                    (
                        [(
                            axum::http::header::CONTENT_TYPE,
                            HeaderValue::from_static("text/event-stream"),
                        )],
                        concat!(
                            "event: response.output_item.done\n",
                            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"status\":\"completed\",\"result\":\"Zm9v\"}}\n\n",
                            "data: [DONE]\n\n",
                        ),
                    )
                }
            }),
        );
        let (upstream_addr, upstream_handle) = spawn_axum_server(upstream_app);
        let proxy = proxy_for_upstreams(vec![upstream(format!("http://{upstream_addr}/v1"))]);

        let response = codex_relay_live_smoke_for_proxy(
            &proxy,
            request(
                "gpt-5.5",
                vec![CodexRelayLiveSmokeCase::HostedImageGeneration],
            ),
        )
        .await
        .expect("live smoke");

        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let body = seen_body
            .lock()
            .expect("lock body")
            .clone()
            .expect("captured body");
        assert_eq!(body["model"].as_str(), Some("gpt-5.5"));
        assert_eq!(body["stream"].as_bool(), Some(true));
        assert_eq!(body["store"].as_bool(), Some(false));
        assert_eq!(
            body["tools"],
            json!([{ "type": "image_generation", "output_format": "png" }])
        );
        assert_eq!(response.results.len(), 1);
        assert_eq!(
            response.results[0].case,
            CodexRelayLiveSmokeCase::HostedImageGeneration
        );
        assert_eq!(
            response.results[0].outcome,
            CodexRelayLiveSmokeOutcome::Passed
        );
        assert!(response.results[0].image_generation_call_seen);

        upstream_handle.abort();
    }

    #[tokio::test]
    async fn codex_relay_live_smoke_applies_upstream_model_mapping() {
        let seen_body = Arc::new(Mutex::new(None::<Value>));

        let seen_body_for_route = seen_body.clone();
        let upstream_app = axum::Router::new().route(
            "/v1/responses/compact",
            post(move |request: Request<Body>| {
                let seen_body = seen_body_for_route.clone();
                async move {
                    let body = axum::body::to_bytes(request.into_body(), 16 * 1024)
                        .await
                        .expect("body");
                    let body: Value = serde_json::from_slice(body.as_ref()).expect("json body");
                    *seen_body.lock().expect("lock body") = Some(body);
                    Json(json!({ "output": [{ "type": "compaction", "encrypted_content": "summary" }] }))
                }
            }),
        );
        let (upstream_addr, upstream_handle) = spawn_axum_server(upstream_app);
        let mut upstream = upstream(format!("http://{upstream_addr}/v1"));
        upstream
            .model_mapping
            .insert("gpt-5.5".to_string(), "openai/gpt-5.5".to_string());
        let proxy = proxy_for_upstreams(vec![upstream]);

        let response = codex_relay_live_smoke_for_proxy(&proxy, request("gpt-5.5", Vec::new()))
            .await
            .expect("live smoke");

        let body = seen_body
            .lock()
            .expect("lock body")
            .clone()
            .expect("captured body");
        assert_eq!(body["model"].as_str(), Some("openai/gpt-5.5"));
        assert_eq!(response.requested_model, "gpt-5.5");
        assert_eq!(response.upstream_model, "openai/gpt-5.5");

        upstream_handle.abort();
    }

    #[tokio::test]
    async fn codex_relay_live_smoke_websocket_sends_response_create_with_beta_and_auth() {
        let captured = Arc::new(Mutex::new(CapturedWebSocketSmoke::default()));
        let (upstream_addr, upstream_handle) = spawn_websocket_server(captured.clone());
        let mut upstream = upstream(format!("http://{upstream_addr}/v1"));
        upstream.auth.auth_token = Some("live-token".to_string());
        upstream.auth.api_key = Some("live-api-key".to_string());
        upstream
            .model_mapping
            .insert("gpt-5.5".to_string(), "openai/gpt-5.5".to_string());
        let proxy = proxy_for_upstreams(vec![upstream]);

        let response = codex_relay_live_smoke_for_proxy(
            &proxy,
            request("gpt-5.5", vec![CodexRelayLiveSmokeCase::ResponsesWebSocket]),
        )
        .await
        .expect("websocket live smoke");

        let captured = captured.lock().expect("lock captured");
        assert_eq!(captured.path.as_deref(), Some("/v1/responses"));
        assert_eq!(
            captured.beta.as_deref(),
            Some("responses_websockets=2026-02-06")
        );
        assert_eq!(captured.authorization.as_deref(), Some("Bearer live-token"));
        assert_eq!(captured.api_key.as_deref(), Some("live-api-key"));
        let first_message = captured.first_message.as_ref().expect("first message");
        assert_eq!(first_message["type"].as_str(), Some("response.create"));
        assert_eq!(first_message["model"].as_str(), Some("openai/gpt-5.5"));
        assert_eq!(first_message["stream"].as_bool(), Some(true));
        assert_eq!(first_message["store"].as_bool(), Some(false));
        assert_eq!(first_message["tools"], json!([]));
        drop(captured);

        assert_eq!(response.requested_model, "gpt-5.5");
        assert_eq!(response.upstream_model, "openai/gpt-5.5");
        assert_eq!(
            response.cases,
            vec![CodexRelayLiveSmokeCase::ResponsesWebSocket]
        );
        assert_eq!(response.results.len(), 1);
        assert_eq!(
            response.results[0].case,
            CodexRelayLiveSmokeCase::ResponsesWebSocket
        );
        assert_eq!(
            response.results[0].outcome,
            CodexRelayLiveSmokeOutcome::Passed
        );
        assert!(response.results[0].accepted_by_responses);
        assert_eq!(response.results[0].status_code, Some(101));

        upstream_handle.abort();
    }

    #[tokio::test]
    async fn codex_relay_live_smoke_targets_route_graph_provider_id() {
        let ciii_hits = Arc::new(AtomicUsize::new(0));
        let ciii_hits_for_route = ciii_hits.clone();
        let ciii_app = axum::Router::new().route(
            "/v1/responses/compact",
            post(move || {
                let ciii_hits = ciii_hits_for_route.clone();
                async move {
                    ciii_hits.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "output": [
                            { "type": "compaction", "encrypted_content": "summary" }
                        ]
                    }))
                }
            }),
        );
        let input8_hits = Arc::new(AtomicUsize::new(0));
        let input8_hits_for_route = input8_hits.clone();
        let input8_app = axum::Router::new().route(
            "/v1/responses/compact",
            post(move || {
                let input8_hits = input8_hits_for_route.clone();
                async move {
                    input8_hits.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "output": [
                            { "type": "compaction", "encrypted_content": "wrong target" }
                        ]
                    }))
                }
            }),
        );
        let (input8_addr, input8_handle) = spawn_axum_server(input8_app);
        let (ciii_addr, ciii_handle) = spawn_axum_server(ciii_app);
        let proxy = proxy_for_v4_providers(vec![
            ("input8", format!("http://{input8_addr}/v1")),
            ("ciii", format!("http://{ciii_addr}/v1")),
        ]);

        let mut payload = request("gpt-5.5", vec![CodexRelayLiveSmokeCase::ResponsesCompact]);
        payload.provider_id = Some("ciii".to_string());
        let response = codex_relay_live_smoke_for_proxy(&proxy, payload)
            .await
            .expect("live smoke");

        assert_eq!(input8_hits.load(Ordering::SeqCst), 0);
        assert_eq!(ciii_hits.load(Ordering::SeqCst), 1);
        assert_eq!(response.station_name, "routing");
        assert_eq!(response.upstream_index, 1);
        assert_eq!(response.provider_id.as_deref(), Some("ciii"));
        assert_eq!(response.endpoint_id.as_deref(), Some("default"));
        assert_eq!(
            response.provider_endpoint_key.as_deref(),
            Some("codex/ciii/default")
        );
        assert_eq!(
            response.results[0].outcome,
            CodexRelayLiveSmokeOutcome::Passed
        );

        input8_handle.abort();
        ciii_handle.abort();
    }

    #[test]
    fn codex_relay_live_smoke_websocket_case_uses_public_wire_name() {
        let value =
            serde_json::to_value(CodexRelayLiveSmokeCase::ResponsesWebSocket).expect("serialize");
        assert_eq!(value, json!("responses_websocket"));

        let legacy_value =
            serde_json::from_value::<CodexRelayLiveSmokeCase>(json!("responses_web_socket"))
                .expect("deserialize legacy spelling");
        assert_eq!(legacy_value, CodexRelayLiveSmokeCase::ResponsesWebSocket);
    }
}

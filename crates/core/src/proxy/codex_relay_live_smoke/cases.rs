use axum::http::{HeaderMap, Method, StatusCode};
use serde_json::{Value, json};

use super::{
    CodexRelayLiveSmokeCase, CodexRelayLiveSmokeResult, classify_compact_live_smoke_response,
    classify_image_live_smoke_response, classify_remote_compaction_v2_live_smoke_response,
};

pub(super) const RESPONSES_WS_BETA_HEADER: &str = "responses_websockets=2026-02-06";
pub(super) const REMOTE_COMPACTION_V2_BETA_FEATURE: &str = "remote_compaction_v2";

#[derive(Debug, Clone, Copy)]
pub(super) struct CodexRelayLiveSmokeCaseDescriptor {
    pub(super) case: CodexRelayLiveSmokeCase,
    pub(super) default_enabled: bool,
    pub(super) acknowledgement_required: bool,
    pub(super) explicit_only_warning: Option<&'static str>,
    pub(super) executor: LiveSmokeExecutor,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum LiveSmokeExecutor {
    Http(LiveSmokeHttpDescriptor),
    WebSocket(LiveSmokeWebSocketDescriptor),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct LiveSmokeHttpDescriptor {
    method: &'static str,
    path: &'static str,
    headers: &'static [(&'static str, &'static str)],
    stream: bool,
    timeout_secs: u64,
    body: fn(&str, Option<&str>) -> Value,
    classify: fn(StatusCode, &HeaderMap, &[u8]) -> CodexRelayLiveSmokeResult,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct LiveSmokeWebSocketDescriptor {
    pub(super) path: &'static str,
    pub(super) beta_header: &'static str,
    pub(super) handshake_timeout_secs: u64,
    pub(super) read_timeout_secs: u64,
    pub(super) body: fn(&str, Option<&str>) -> Value,
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
            headers: &[],
            stream: false,
            timeout_secs: 30,
            body: compact_live_smoke_body,
            classify: classify_compact_live_smoke_response,
        }),
    },
    CodexRelayLiveSmokeCaseDescriptor {
        case: CodexRelayLiveSmokeCase::RemoteCompactionV2,
        default_enabled: false,
        acknowledgement_required: true,
        explicit_only_warning: Some(
            "remote compaction v2 was not tested because compact v2 smoke is explicit-only",
        ),
        executor: LiveSmokeExecutor::Http(LiveSmokeHttpDescriptor {
            method: "POST",
            path: "/responses",
            headers: &[("x-codex-beta-features", REMOTE_COMPACTION_V2_BETA_FEATURE)],
            stream: true,
            timeout_secs: 60,
            body: remote_compaction_v2_live_smoke_body,
            classify: classify_remote_compaction_v2_live_smoke_response,
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
            headers: &[],
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

pub(super) fn codex_relay_live_smoke_cases() -> &'static [CodexRelayLiveSmokeCaseDescriptor] {
    CODEX_RELAY_LIVE_SMOKE_CASES
}

pub(super) fn live_smoke_case_descriptor(
    case: CodexRelayLiveSmokeCase,
) -> &'static CodexRelayLiveSmokeCaseDescriptor {
    codex_relay_live_smoke_cases()
        .iter()
        .find(|descriptor| descriptor.case == case)
        .expect("Codex relay live smoke case must be registered")
}

pub(super) struct LiveSmokeSpec {
    pub(super) method: Method,
    pub(super) path: &'static str,
    pub(super) headers: &'static [(&'static str, &'static str)],
    pub(super) body: Value,
    pub(super) stream: bool,
    pub(super) timeout: std::time::Duration,
    pub(super) classify: fn(StatusCode, &HeaderMap, &[u8]) -> CodexRelayLiveSmokeResult,
}

impl LiveSmokeSpec {
    pub(super) fn for_case(
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
            headers: http.headers,
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

fn remote_compaction_v2_live_smoke_body(model: &str, service_tier: Option<&str>) -> Value {
    let mut body = json!({
        "model": model,
        "instructions": "Return exactly one Codex compaction output item for this diagnostic request.",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [
                    {
                        "type": "input_text",
                        "text": "Codex relay remote_compaction_v2 live smoke. Compact this short diagnostic conversation."
                    }
                ]
            },
            {
                "type": "message",
                "role": "assistant",
                "content": [
                    {
                        "type": "output_text",
                        "text": "Diagnostic reply for remote compaction v2 live smoke."
                    }
                ]
            },
            {
                "type": "compaction_trigger"
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

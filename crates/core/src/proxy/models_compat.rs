use std::io::Read;

use axum::body::Bytes;
use axum::http::{HeaderMap, header};
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use serde_json::Value;

const MAX_DECODED_MODELS_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn maybe_decode_models_response_body(
    service_name: &str,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Bytes {
    let body =
        maybe_decode_models_response_body_without_translation(service_name, path, headers, body);
    maybe_translate_openai_models_list(body.as_ref()).unwrap_or(body)
}

pub(super) fn maybe_decode_models_response_body_without_translation(
    service_name: &str,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Bytes {
    if service_name != "codex" || path != "/models" {
        return body;
    }

    if looks_like_json(body.as_ref()) {
        body
    } else if let Some(decoded) = decode_from_content_encoding(headers, body.as_ref())
        .or_else(|| decode_from_signature(body.as_ref()))
    {
        Bytes::from(decoded)
    } else {
        body
    }
}

fn decode_from_content_encoding(headers: &HeaderMap, body: &[u8]) -> Option<Vec<u8>> {
    let mut encodings = Vec::new();
    for value in headers.get_all(header::CONTENT_ENCODING).iter() {
        let Ok(value) = value.to_str() else {
            continue;
        };
        encodings.extend(
            value
                .split(',')
                .map(|part| part.trim().to_ascii_lowercase())
                .filter(|part| !part.is_empty() && part != "identity"),
        );
    }
    if encodings.is_empty() {
        return None;
    }

    let mut decoded = body.to_vec();
    for encoding in encodings.iter().rev() {
        decoded = decode_one(encoding, &decoded)?;
    }
    looks_like_json(&decoded).then_some(decoded)
}

fn decode_from_signature(body: &[u8]) -> Option<Vec<u8>> {
    if body.starts_with(&[0x1f, 0x8b]) {
        return decode_gzip(body);
    }
    if body.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        return decode_zstd(body);
    }

    // Brotli and raw deflate do not have a reliable magic prefix. Only accept a
    // decoded result when it is JSON, which keeps this fallback scoped to /models.
    decode_brotli(body)
        .or_else(|| decode_zlib(body))
        .or_else(|| decode_deflate(body))
}

fn decode_one(encoding: &str, body: &[u8]) -> Option<Vec<u8>> {
    match encoding {
        "gzip" | "x-gzip" => decode_gzip(body),
        "br" => decode_brotli(body),
        "zstd" | "zst" => decode_zstd(body),
        "deflate" => decode_zlib(body).or_else(|| decode_deflate(body)),
        _ => None,
    }
}

fn decode_gzip(body: &[u8]) -> Option<Vec<u8>> {
    read_jsonish(GzDecoder::new(body))
}

fn decode_brotli(body: &[u8]) -> Option<Vec<u8>> {
    read_jsonish(brotli::Decompressor::new(body, 4096))
}

fn decode_zstd(body: &[u8]) -> Option<Vec<u8>> {
    read_jsonish(zstd::stream::read::Decoder::new(body).ok()?)
}

fn decode_zlib(body: &[u8]) -> Option<Vec<u8>> {
    read_jsonish(ZlibDecoder::new(body))
}

fn decode_deflate(body: &[u8]) -> Option<Vec<u8>> {
    read_jsonish(DeflateDecoder::new(body))
}

fn read_jsonish<R: Read>(reader: R) -> Option<Vec<u8>> {
    let mut limited = reader.take((MAX_DECODED_MODELS_BYTES + 1) as u64);
    let mut out = Vec::new();
    limited.read_to_end(&mut out).ok()?;
    if out.len() > MAX_DECODED_MODELS_BYTES || !looks_like_json(&out) {
        return None;
    }
    Some(out)
}

fn looks_like_json(bytes: &[u8]) -> bool {
    let Some(first) = bytes.iter().find(|byte| !byte.is_ascii_whitespace()) else {
        return false;
    };
    matches!(first, b'{' | b'[')
}

fn maybe_translate_openai_models_list(body: &[u8]) -> Option<Bytes> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    if value.get("models").is_some() {
        return None;
    }

    let data = value.get("data")?.as_array()?;
    let mut seen = std::collections::HashSet::new();
    let mut models = Vec::new();
    for item in data {
        let Some(slug) = openai_model_id(item) else {
            continue;
        };
        if !seen.insert(slug.to_ascii_lowercase()) {
            continue;
        }
        let display_name = openai_model_display_name(item).unwrap_or_else(|| display_name(&slug));
        models.push(codex_model_info_json(
            &slug,
            display_name.as_str(),
            models.len(),
        ));
    }

    serde_json::to_vec(&serde_json::json!({ "models": models }))
        .ok()
        .map(Bytes::from)
}

fn openai_model_id(item: &Value) -> Option<String> {
    item.get("id")
        .or_else(|| item.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.strip_prefix("models/").unwrap_or(value).to_string())
}

fn openai_model_display_name(item: &Value) -> Option<String> {
    item.get("display_name")
        .or_else(|| item.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn codex_model_info_json(slug: &str, display_name: &str, fallback_priority: usize) -> Value {
    let known = known_codex_model(slug);
    let caps = known.capabilities;
    let priority = known
        .priority
        .unwrap_or_else(|| 10_000 + i32::try_from(fallback_priority).unwrap_or(0));

    serde_json::json!({
        "slug": slug,
        "display_name": known.display_name.unwrap_or(display_name),
        "description": known.description,
        "default_reasoning_level": caps.default_reasoning_level,
        "supported_reasoning_levels": caps.reasoning_levels.json(),
        "shell_type": caps.shell_type,
        "visibility": if known.hidden { "hide" } else { "list" },
        "supported_in_api": caps.supported_in_api,
        "priority": priority,
        "additional_speed_tiers": caps.additional_speed_tiers_json(),
        "service_tiers": caps.service_tiers_json(),
        "availability_nux": null,
        "upgrade": null,
        "base_instructions": "You are Codex, a coding agent based on GPT-5.",
        "model_messages": null,
        "supports_reasoning_summaries": caps.supports_reasoning_summaries,
        "default_reasoning_summary": caps.default_reasoning_summary,
        "support_verbosity": caps.support_verbosity,
        "default_verbosity": caps.default_verbosity,
        "apply_patch_tool_type": caps.apply_patch_tool_type,
        "web_search_tool_type": caps.web_search_tool_type,
        "truncation_policy": {
            "mode": caps.truncation_policy.mode,
            "limit": caps.truncation_policy.limit
        },
        "supports_parallel_tool_calls": caps.supports_parallel_tool_calls,
        "supports_image_detail_original": caps.supports_image_detail_original,
        "context_window": known.context_window,
        "max_context_window": known.max_context_window,
        "auto_compact_token_limit": null,
        "effective_context_window_percent": 95,
        "experimental_supported_tools": [],
        "input_modalities": caps.input_modalities(),
        "supports_search_tool": caps.supports_search_tool
    })
}

#[derive(Clone, Copy)]
struct ModelCapabilities {
    reasoning_levels: ReasoningLevels,
    default_reasoning_level: Option<&'static str>,
    supports_reasoning_summaries: bool,
    default_reasoning_summary: &'static str,
    support_verbosity: bool,
    default_verbosity: Option<&'static str>,
    fast_service_tier: bool,
    text_input: bool,
    image_input: bool,
    supports_image_detail_original: bool,
    shell_type: &'static str,
    supported_in_api: bool,
    apply_patch_tool_type: Option<&'static str>,
    web_search_tool_type: &'static str,
    supports_search_tool: bool,
    truncation_policy: TruncationPolicyCompat,
    supports_parallel_tool_calls: bool,
}

impl ModelCapabilities {
    const fn modern_gpt() -> Self {
        Self {
            reasoning_levels: ReasoningLevels::Modern,
            default_reasoning_level: Some("medium"),
            supports_reasoning_summaries: true,
            default_reasoning_summary: "none",
            support_verbosity: true,
            default_verbosity: Some("low"),
            fast_service_tier: false,
            text_input: true,
            image_input: true,
            supports_image_detail_original: true,
            shell_type: "shell_command",
            supported_in_api: true,
            apply_patch_tool_type: Some("freeform"),
            web_search_tool_type: "text_and_image",
            supports_search_tool: true,
            truncation_policy: TruncationPolicyCompat::tokens(10_000),
            supports_parallel_tool_calls: true,
        }
    }

    const fn gpt_5_2() -> Self {
        Self {
            reasoning_levels: ReasoningLevels::Gpt52,
            default_reasoning_level: Some("medium"),
            supports_reasoning_summaries: true,
            default_reasoning_summary: "auto",
            support_verbosity: true,
            default_verbosity: Some("low"),
            fast_service_tier: false,
            text_input: true,
            image_input: true,
            supports_image_detail_original: false,
            shell_type: "shell_command",
            supported_in_api: true,
            apply_patch_tool_type: Some("freeform"),
            web_search_tool_type: "text",
            supports_search_tool: true,
            truncation_policy: TruncationPolicyCompat::bytes(10_000),
            supports_parallel_tool_calls: true,
        }
    }

    const fn codex_spark() -> Self {
        Self {
            image_input: false,
            supports_image_detail_original: false,
            web_search_tool_type: "text",
            supports_search_tool: false,
            fast_service_tier: false,
            ..Self::modern_gpt()
        }
    }

    const fn gpt_image() -> Self {
        Self {
            reasoning_levels: ReasoningLevels::None,
            default_reasoning_level: None,
            supports_reasoning_summaries: false,
            default_reasoning_summary: "auto",
            support_verbosity: false,
            default_verbosity: None,
            fast_service_tier: false,
            text_input: true,
            image_input: false,
            supports_image_detail_original: false,
            shell_type: "disabled",
            supported_in_api: false,
            apply_patch_tool_type: None,
            web_search_tool_type: "text",
            supports_search_tool: false,
            truncation_policy: TruncationPolicyCompat::tokens(10_000),
            supports_parallel_tool_calls: false,
        }
    }

    const fn conservative_coding() -> Self {
        Self {
            reasoning_levels: ReasoningLevels::None,
            default_reasoning_level: None,
            supports_reasoning_summaries: false,
            default_reasoning_summary: "auto",
            support_verbosity: false,
            default_verbosity: None,
            fast_service_tier: false,
            text_input: true,
            image_input: false,
            supports_image_detail_original: false,
            shell_type: "shell_command",
            supported_in_api: true,
            apply_patch_tool_type: Some("freeform"),
            web_search_tool_type: "text",
            supports_search_tool: false,
            truncation_policy: TruncationPolicyCompat::tokens(10_000),
            supports_parallel_tool_calls: false,
        }
    }

    const fn with_fast_service_tier(mut self) -> Self {
        self.fast_service_tier = true;
        self
    }

    const fn with_default_verbosity(mut self, default_verbosity: &'static str) -> Self {
        self.default_verbosity = Some(default_verbosity);
        self
    }

    const fn with_web_search_type(mut self, web_search_tool_type: &'static str) -> Self {
        self.web_search_tool_type = web_search_tool_type;
        self
    }

    fn input_modalities(self) -> Vec<&'static str> {
        let mut modalities = Vec::new();
        if self.text_input {
            modalities.push("text");
        }
        if self.image_input {
            modalities.push("image");
        }
        modalities
    }

    fn additional_speed_tiers_json(self) -> Value {
        if self.fast_service_tier {
            serde_json::json!(["fast"])
        } else {
            serde_json::json!([])
        }
    }

    fn service_tiers_json(self) -> Value {
        if self.fast_service_tier {
            serde_json::json!([
                {
                    "id": "priority",
                    "name": "Fast",
                    "description": "1.5x speed, increased usage"
                }
            ])
        } else {
            serde_json::json!([])
        }
    }
}

#[derive(Clone, Copy)]
enum ReasoningLevels {
    None,
    Modern,
    Gpt52,
}

impl ReasoningLevels {
    fn json(self) -> Value {
        match self {
            Self::None => serde_json::json!([]),
            Self::Modern => serde_json::json!([
                {
                    "effort": "low",
                    "description": "Fast responses with lighter reasoning"
                },
                {
                    "effort": "medium",
                    "description": "Balances speed and reasoning depth for everyday tasks"
                },
                {
                    "effort": "high",
                    "description": "Greater reasoning depth for complex problems"
                },
                {
                    "effort": "xhigh",
                    "description": "Extra high reasoning depth for complex problems"
                }
            ]),
            Self::Gpt52 => serde_json::json!([
                {
                    "effort": "low",
                    "description": "Balances speed with some reasoning; useful for straightforward queries and short explanations"
                },
                {
                    "effort": "medium",
                    "description": "Provides a solid balance of reasoning depth and latency for general-purpose tasks"
                },
                {
                    "effort": "high",
                    "description": "Maximizes reasoning depth for complex or ambiguous problems"
                },
                {
                    "effort": "xhigh",
                    "description": "Extra high reasoning for complex problems"
                }
            ]),
        }
    }
}

#[derive(Clone, Copy)]
struct TruncationPolicyCompat {
    mode: &'static str,
    limit: i64,
}

impl TruncationPolicyCompat {
    const fn bytes(limit: i64) -> Self {
        Self {
            mode: "bytes",
            limit,
        }
    }

    const fn tokens(limit: i64) -> Self {
        Self {
            mode: "tokens",
            limit,
        }
    }
}

fn is_gpt_capability_excluded_slug(slug: &str) -> bool {
    slug.starts_with("gpt-image-")
        || slug.starts_with("gpt-oss-")
        || slug.starts_with("gpt-realtime")
        || slug.starts_with("gpt-audio")
        || slug.contains("-realtime")
        || slug.contains("-audio")
        || slug.contains("-image")
        || slug.contains("embedding")
        || slug.contains("moderation")
        || slug.contains("whisper")
        || slug.contains("tts")
        || slug.contains("sora")
        || slug.contains("spark")
        || slug.ends_with("-nano")
        || slug.contains("-nano-")
        || slug.ends_with("-pro")
        || slug.contains("-pro-")
}

fn infer_future_gpt_capabilities(slug: &str) -> Option<ModelCapabilities> {
    if !slug.starts_with("gpt-") || is_gpt_capability_excluded_slug(slug) {
        return None;
    }

    let (major, minor) = parse_gpt_version(slug)?;
    let is_future_priority_gpt = major > 5 || (major == 5 && minor.is_some_and(|minor| minor >= 4));
    is_future_priority_gpt.then(|| ModelCapabilities::modern_gpt().with_fast_service_tier())
}

fn parse_gpt_version(slug: &str) -> Option<(u32, Option<u32>)> {
    let rest = slug.strip_prefix("gpt-")?;
    let (major, rest) = parse_ascii_u32_prefix(rest)?;
    let Some(rest) = rest.strip_prefix('.') else {
        return Some((major, None));
    };
    let (minor, _) = parse_ascii_u32_prefix(rest)?;
    Some((major, Some(minor)))
}

fn parse_ascii_u32_prefix(value: &str) -> Option<(u32, &str)> {
    let end = value
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(value.len());
    if end == 0 {
        return None;
    }
    let (digits, rest) = value.split_at(end);
    let parsed = digits.parse().ok()?;
    Some((parsed, rest))
}

struct KnownCodexModel {
    display_name: Option<&'static str>,
    description: Option<&'static str>,
    priority: Option<i32>,
    context_window: i64,
    max_context_window: i64,
    hidden: bool,
    capabilities: ModelCapabilities,
}

fn known_codex_model(slug: &str) -> KnownCodexModel {
    let normalized = slug.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "gpt-5.5" => KnownCodexModel {
            display_name: Some("GPT-5.5"),
            description: Some("Frontier model for complex coding, research, and real-world work."),
            priority: Some(0),
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: false,
            capabilities: ModelCapabilities::modern_gpt().with_fast_service_tier(),
        },
        "gpt-5.4" => KnownCodexModel {
            display_name: Some("gpt-5.4"),
            description: Some("Strong model for everyday coding."),
            priority: Some(10),
            context_window: 272_000,
            max_context_window: 1_000_000,
            hidden: false,
            capabilities: ModelCapabilities::modern_gpt().with_fast_service_tier(),
        },
        "gpt-5.4-mini" => KnownCodexModel {
            display_name: Some("GPT-5.4-Mini"),
            description: Some("Small, fast, and cost-efficient model for simpler coding tasks."),
            priority: Some(20),
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: false,
            capabilities: ModelCapabilities::modern_gpt()
                .with_default_verbosity("medium")
                .with_fast_service_tier(),
        },
        "gpt-5.3-codex" => KnownCodexModel {
            display_name: Some("GPT-5.3 Codex"),
            description: Some("Coding-optimized model."),
            priority: Some(30),
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: false,
            capabilities: ModelCapabilities::modern_gpt()
                .with_web_search_type("text")
                .with_fast_service_tier(),
        },
        "gpt-5.3-codex-spark" => KnownCodexModel {
            display_name: Some("GPT-5.3 Codex Spark"),
            description: Some("Coding-optimized model with limited image support."),
            priority: Some(40),
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: false,
            capabilities: ModelCapabilities::codex_spark(),
        },
        "gpt-5.2" => KnownCodexModel {
            display_name: Some("GPT-5.2"),
            description: Some("Optimized for professional work and long-running agents."),
            priority: Some(50),
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: false,
            capabilities: ModelCapabilities::gpt_5_2(),
        },
        "codex-auto-review" => KnownCodexModel {
            display_name: Some("Codex Auto Review"),
            description: Some("Internal review model."),
            priority: Some(50_000),
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: true,
            capabilities: ModelCapabilities::modern_gpt(),
        },
        _ if normalized.starts_with("gpt-image-") => KnownCodexModel {
            display_name: None,
            description: Some("Image model served by the configured Codex upstream."),
            priority: None,
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: true,
            capabilities: ModelCapabilities::gpt_image(),
        },
        _ => {
            let capabilities = infer_future_gpt_capabilities(&normalized)
                .unwrap_or_else(ModelCapabilities::conservative_coding);
            KnownCodexModel {
                display_name: None,
                description: Some("Model served by the configured Codex upstream."),
                priority: None,
                context_window: 272_000,
                max_context_window: 272_000,
                hidden: false,
                capabilities,
            }
        }
    }
}

fn display_name(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            if part.eq_ignore_ascii_case("gpt") {
                "GPT".to_string()
            } else {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().chain(chars).collect(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_capability_profile::{
        CodexCapabilityProfile, CodexCapabilitySupport, CodexModelCatalogShape,
    };
    use crate::codex_integration::CodexPatchMode;

    #[test]
    fn codex_capability_profile_understands_translated_openai_models_list() {
        let body = br#"{
            "object": "list",
            "data": [
                { "id": "gpt-5.5", "object": "model", "display_name": "GPT-5.5" }
            ]
        }"#;

        let translated = maybe_translate_openai_models_list(body)
            .expect("OpenAI models list should translate to Codex catalog");
        let value: serde_json::Value =
            serde_json::from_slice(translated.as_ref()).expect("translated JSON");
        let profile = CodexCapabilityProfile::for_models_response_json(
            CodexPatchMode::OfficialImagegenBridge,
            &value,
            Some("gpt-5.5"),
        );

        assert_eq!(
            profile.model_catalog.shape,
            CodexModelCatalogShape::CodexModels
        );
        assert_eq!(
            profile.hosted_image_generation.support,
            CodexCapabilitySupport::Supported
        );
        assert_eq!(
            profile.remote_compaction_v1.support,
            CodexCapabilitySupport::Supported
        );

        let model = model_by_slug(&value, "gpt-5.5");
        assert!(model_has_fast_service_tier(model));
        assert!(model_has_legacy_fast_speed_tier(model));
    }

    #[test]
    fn translated_openai_models_list_marks_official_priority_models_as_fast() {
        let body = br#"{
            "object": "list",
            "data": [
                { "id": "gpt-5.4" },
                { "id": "gpt-5.4-mini" },
                { "id": "gpt-5.3-codex" },
                { "id": "gpt-5.2" }
            ]
        }"#;

        let translated = maybe_translate_openai_models_list(body)
            .expect("OpenAI models list should translate to Codex catalog");
        let value: serde_json::Value =
            serde_json::from_slice(translated.as_ref()).expect("translated JSON");

        for slug in ["gpt-5.4", "gpt-5.4-mini", "gpt-5.3-codex"] {
            let model = model_by_slug(&value, slug);
            assert!(model_has_fast_service_tier(model), "{slug} should be fast");
            assert!(
                model_has_legacy_fast_speed_tier(model),
                "{slug} should expose legacy fast tier"
            );
        }

        let model = model_by_slug(&value, "gpt-5.2");
        assert!(!model_has_fast_service_tier(model));
        assert!(!model_has_legacy_fast_speed_tier(model));
    }

    #[test]
    fn translated_openai_models_list_infers_future_gpt_fast_with_exclusions() {
        let body = br#"{
            "object": "list",
            "data": [
                { "id": "gpt-6" },
                { "id": "gpt-5.4-preview" },
                { "id": "gpt-image-1" },
                { "id": "gpt-oss-120b" },
                { "id": "gpt-5.4-nano" },
                { "id": "gpt-5.3-codex-spark" }
            ]
        }"#;

        let translated = maybe_translate_openai_models_list(body)
            .expect("OpenAI models list should translate to Codex catalog");
        let value: serde_json::Value =
            serde_json::from_slice(translated.as_ref()).expect("translated JSON");

        for slug in ["gpt-6", "gpt-5.4-preview"] {
            let model = model_by_slug(&value, slug);
            assert!(model_has_fast_service_tier(model), "{slug} should be fast");
            assert!(
                model_has_legacy_fast_speed_tier(model),
                "{slug} should expose legacy fast tier"
            );
        }

        for slug in [
            "gpt-image-1",
            "gpt-oss-120b",
            "gpt-5.4-nano",
            "gpt-5.3-codex-spark",
        ] {
            let model = model_by_slug(&value, slug);
            assert!(
                !model_has_fast_service_tier(model),
                "{slug} should not be fast"
            );
            assert!(
                !model_has_legacy_fast_speed_tier(model),
                "{slug} should not expose legacy fast tier"
            );
        }
    }

    #[test]
    fn translated_openai_models_list_populates_modern_gpt_capabilities() {
        let value = translated_models_value(&["gpt-5.5"]);
        let model = model_by_slug(&value, "gpt-5.5");

        assert_eq!(
            model.get("default_reasoning_level").and_then(Value::as_str),
            Some("medium")
        );
        assert_reasoning_efforts(model, &["low", "medium", "high", "xhigh"]);
        assert_eq!(
            model
                .get("supports_reasoning_summaries")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            model
                .get("default_reasoning_summary")
                .and_then(Value::as_str),
            Some("none")
        );
        assert_eq!(
            model.get("support_verbosity").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            model.get("default_verbosity").and_then(Value::as_str),
            Some("low")
        );
        assert_eq!(
            model.get("apply_patch_tool_type").and_then(Value::as_str),
            Some("freeform")
        );
        assert_eq!(
            model.get("web_search_tool_type").and_then(Value::as_str),
            Some("text_and_image")
        );
        assert_eq!(
            model
                .get("supports_parallel_tool_calls")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            model
                .get("supports_image_detail_original")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_truncation_policy(model, "tokens", 10_000);
        assert_input_modalities(model, &["text", "image"]);
        assert_eq!(
            model.get("supports_search_tool").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn translated_openai_models_list_preserves_gpt_5_2_capability_differences() {
        let value = translated_models_value(&["gpt-5.2"]);
        let model = model_by_slug(&value, "gpt-5.2");

        assert_eq!(
            model
                .get("default_reasoning_summary")
                .and_then(Value::as_str),
            Some("auto")
        );
        assert_eq!(
            model.get("web_search_tool_type").and_then(Value::as_str),
            Some("text")
        );
        assert_eq!(
            model
                .get("supports_image_detail_original")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_truncation_policy(model, "bytes", 10_000);
        assert!(!model_has_fast_service_tier(model));
    }

    #[test]
    fn translated_openai_models_list_uses_modern_capabilities_for_future_gpt_models() {
        let value = translated_models_value(&["gpt-6-preview"]);
        let model = model_by_slug(&value, "gpt-6-preview");

        assert!(model_has_fast_service_tier(model));
        assert_eq!(
            model
                .get("supports_reasoning_summaries")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            model.get("support_verbosity").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            model.get("apply_patch_tool_type").and_then(Value::as_str),
            Some("freeform")
        );
        assert_input_modalities(model, &["text", "image"]);
    }

    #[test]
    fn translated_openai_models_list_uses_conservative_capabilities_for_unknown_non_gpt_models() {
        let value = translated_models_value(&["claude-sonnet-4-5"]);
        let model = model_by_slug(&value, "claude-sonnet-4-5");

        assert_eq!(model.get("default_reasoning_level"), Some(&Value::Null));
        assert_reasoning_efforts(model, &[]);
        assert_eq!(
            model
                .get("supports_reasoning_summaries")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            model.get("support_verbosity").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            model.get("apply_patch_tool_type").and_then(Value::as_str),
            Some("freeform")
        );
        assert_eq!(
            model
                .get("supports_parallel_tool_calls")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_input_modalities(model, &["text"]);
        assert!(!model_has_fast_service_tier(model));
    }

    fn translated_models_value(slugs: &[&str]) -> Value {
        let data = slugs
            .iter()
            .map(|slug| serde_json::json!({ "id": slug }))
            .collect::<Vec<_>>();
        let body = serde_json::to_vec(&serde_json::json!({
            "object": "list",
            "data": data
        }))
        .expect("serialize OpenAI models list");

        let translated = maybe_translate_openai_models_list(&body)
            .expect("OpenAI models list should translate to Codex catalog");
        serde_json::from_slice(translated.as_ref()).expect("translated JSON")
    }

    fn model_by_slug<'a>(value: &'a Value, slug: &str) -> &'a Value {
        value
            .get("models")
            .and_then(Value::as_array)
            .and_then(|models| {
                models
                    .iter()
                    .find(|model| model.get("slug").and_then(Value::as_str) == Some(slug))
            })
            .unwrap_or_else(|| panic!("model {slug} should exist"))
    }

    fn assert_reasoning_efforts(model: &Value, expected: &[&str]) {
        let actual = model
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
            .expect("supported_reasoning_levels should be an array")
            .iter()
            .map(|level| {
                level
                    .get("effort")
                    .and_then(Value::as_str)
                    .expect("effort should be present")
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    fn assert_truncation_policy(model: &Value, expected_mode: &str, expected_limit: i64) {
        let policy = model
            .get("truncation_policy")
            .expect("truncation_policy should be present");
        assert_eq!(
            policy.get("mode").and_then(Value::as_str),
            Some(expected_mode)
        );
        assert_eq!(
            policy.get("limit").and_then(Value::as_i64),
            Some(expected_limit)
        );
    }

    fn assert_input_modalities(model: &Value, expected: &[&str]) {
        let actual = model
            .get("input_modalities")
            .and_then(Value::as_array)
            .expect("input_modalities should be an array")
            .iter()
            .map(|modality| modality.as_str().expect("modality should be a string"))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    fn model_has_fast_service_tier(model: &Value) -> bool {
        model
            .get("service_tiers")
            .and_then(Value::as_array)
            .is_some_and(|service_tiers| {
                service_tiers.iter().any(|tier| {
                    tier.get("id").and_then(Value::as_str) == Some("priority")
                        && tier.get("name").and_then(Value::as_str) == Some("Fast")
                })
            })
    }

    fn model_has_legacy_fast_speed_tier(model: &Value) -> bool {
        model
            .get("additional_speed_tiers")
            .and_then(Value::as_array)
            .is_some_and(|speed_tiers| speed_tiers.iter().any(|tier| tier.as_str() == Some("fast")))
    }
}

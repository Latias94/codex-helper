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
    let hidden = known.hidden || slug.starts_with("gpt-image-") || slug == "codex-auto-review";
    let input_modalities = if slug.contains("spark") || slug.starts_with("gpt-image-") {
        vec!["text"]
    } else {
        vec!["text", "image"]
    };
    let context_window = known.context_window;
    let supports_search_tool = !slug.contains("spark") && !slug.starts_with("gpt-image-");
    let priority = known
        .priority
        .unwrap_or_else(|| 10_000 + i32::try_from(fallback_priority).unwrap_or(0));
    let supports_fast_service_tier = supports_fast_service_tier(slug);
    let additional_speed_tiers = if supports_fast_service_tier {
        serde_json::json!(["fast"])
    } else {
        serde_json::json!([])
    };
    let service_tiers = if supports_fast_service_tier {
        serde_json::json!([
            {
                "id": "priority",
                "name": "Fast",
                "description": "1.5x speed, increased usage"
            }
        ])
    } else {
        serde_json::json!([])
    };

    serde_json::json!({
        "slug": slug,
        "display_name": known.display_name.unwrap_or(display_name),
        "description": known.description,
        "default_reasoning_level": "medium",
        "supported_reasoning_levels": [
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
        ],
        "shell_type": "shell_command",
        "visibility": if hidden { "hide" } else { "list" },
        "supported_in_api": !slug.starts_with("gpt-image-"),
        "priority": priority,
        "additional_speed_tiers": additional_speed_tiers,
        "service_tiers": service_tiers,
        "availability_nux": null,
        "upgrade": null,
        "base_instructions": "You are Codex, a coding agent based on GPT-5.",
        "model_messages": null,
        "supports_reasoning_summaries": !slug.starts_with("gpt-image-"),
        "default_reasoning_summary": "auto",
        "support_verbosity": true,
        "default_verbosity": "low",
        "apply_patch_tool_type": "freeform",
        "web_search_tool_type": if supports_search_tool { "text_and_image" } else { "text" },
        "truncation_policy": {
            "mode": "tokens",
            "limit": 10000
        },
        "supports_parallel_tool_calls": true,
        "supports_image_detail_original": input_modalities.contains(&"image"),
        "context_window": context_window,
        "max_context_window": context_window,
        "auto_compact_token_limit": null,
        "effective_context_window_percent": 95,
        "experimental_supported_tools": [],
        "input_modalities": input_modalities,
        "supports_search_tool": supports_search_tool
    })
}

fn supports_fast_service_tier(slug: &str) -> bool {
    let normalized = slug.trim().to_ascii_lowercase();
    match normalized.as_str() {
        // OpenAI Priority processing / Codex Fast is exposed to the TUI as a
        // service tier with id `priority` and display name `Fast`.
        "gpt-5.5" | "gpt-5.4" | "gpt-5.4-mini" | "gpt-5.3-codex" => true,
        _ => infer_future_gpt_fast_service_tier(&normalized),
    }
}

fn infer_future_gpt_fast_service_tier(slug: &str) -> bool {
    if !slug.starts_with("gpt-") || is_fast_service_tier_excluded_slug(slug) {
        return false;
    }

    let Some((major, minor)) = parse_gpt_version(slug) else {
        return false;
    };

    // Conservative fallback: current verified GPT 5.4+ models support Priority
    // processing, and future GPT major series are expected to keep supporting it.
    major > 5 || (major == 5 && minor.is_some_and(|minor| minor >= 4))
}

fn is_fast_service_tier_excluded_slug(slug: &str) -> bool {
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

#[derive(Default)]
struct KnownCodexModel {
    display_name: Option<&'static str>,
    description: Option<&'static str>,
    priority: Option<i32>,
    context_window: i64,
    hidden: bool,
}

fn known_codex_model(slug: &str) -> KnownCodexModel {
    match slug {
        "gpt-5.5" => KnownCodexModel {
            display_name: Some("GPT-5.5"),
            description: Some("Frontier model for complex coding, research, and real-world work."),
            priority: Some(0),
            context_window: 272_000,
            hidden: false,
        },
        "gpt-5.4" => KnownCodexModel {
            display_name: Some("gpt-5.4"),
            description: Some("Strong model for everyday coding."),
            priority: Some(10),
            context_window: 272_000,
            hidden: false,
        },
        "gpt-5.4-mini" => KnownCodexModel {
            display_name: Some("GPT-5.4-Mini"),
            description: Some("Small, fast, and cost-efficient model for simpler coding tasks."),
            priority: Some(20),
            context_window: 272_000,
            hidden: false,
        },
        "gpt-5.3-codex" => KnownCodexModel {
            display_name: Some("GPT-5.3 Codex"),
            description: Some("Coding-optimized model."),
            priority: Some(30),
            context_window: 272_000,
            hidden: false,
        },
        "gpt-5.3-codex-spark" => KnownCodexModel {
            display_name: Some("GPT-5.3 Codex Spark"),
            description: Some("Coding-optimized model with limited image support."),
            priority: Some(40),
            context_window: 272_000,
            hidden: false,
        },
        "gpt-5.2" => KnownCodexModel {
            display_name: Some("GPT-5.2"),
            description: Some("Optimized for professional work and long-running agents."),
            priority: Some(50),
            context_window: 272_000,
            hidden: false,
        },
        "codex-auto-review" => KnownCodexModel {
            display_name: Some("Codex Auto Review"),
            description: Some("Internal review model."),
            priority: Some(50_000),
            context_window: 272_000,
            hidden: true,
        },
        _ => KnownCodexModel {
            display_name: None,
            description: Some("Model served by the configured Codex upstream."),
            priority: None,
            context_window: 272_000,
            hidden: false,
        },
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

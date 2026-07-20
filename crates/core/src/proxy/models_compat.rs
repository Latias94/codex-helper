use std::collections::HashSet;
use std::io::Read;

use axum::body::Bytes;
use axum::http::{HeaderMap, header};
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use serde_json::Value;

use crate::provider_catalog::{ProviderCatalogEpoch, ProviderModelCapabilities};

const MAX_DECODED_MODELS_BYTES: usize = 8 * 1024 * 1024;

#[derive(serde::Serialize)]
struct CodexModelsResponse<'a> {
    models: &'a [Value],
}

#[derive(Debug, Clone, Copy)]
pub(super) enum ModelsTranslationScope<'a> {
    Disabled,
    Conservative,
    CapturedCatalog(&'a ProviderCatalogEpoch),
}

impl<'a> ModelsTranslationScope<'a> {
    pub(super) fn for_request(
        enabled: bool,
        provider_epoch: Option<&'a ProviderCatalogEpoch>,
    ) -> Self {
        if !enabled {
            Self::Disabled
        } else if let Some(provider_epoch) = provider_epoch {
            Self::CapturedCatalog(provider_epoch)
        } else {
            Self::Conservative
        }
    }
}

pub(super) fn codex_path_is_models(path: &str) -> bool {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .is_some_and(|segment| segment == "models")
}

pub(super) fn maybe_decode_models_response_body(
    service_name: &str,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
    translation: ModelsTranslationScope<'_>,
) -> Bytes {
    if service_name != "codex" || !codex_path_is_models(path) {
        return body;
    }

    let decoded = if looks_like_json(body.as_ref()) {
        body
    } else if let Some(decoded) = decode_from_content_encoding(headers, body.as_ref())
        .or_else(|| decode_from_signature(body.as_ref()))
    {
        Bytes::from(decoded)
    } else {
        body
    };

    match translation {
        ModelsTranslationScope::Disabled => decoded,
        ModelsTranslationScope::Conservative => {
            maybe_translate_openai_models_list(decoded.as_ref(), None).unwrap_or(decoded)
        }
        ModelsTranslationScope::CapturedCatalog(epoch) => {
            maybe_translate_openai_models_list(decoded.as_ref(), Some(epoch)).unwrap_or(decoded)
        }
    }
}

fn maybe_translate_openai_models_list(
    body: &[u8],
    provider_epoch: Option<&ProviderCatalogEpoch>,
) -> Option<Bytes> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    if value.get("models").is_some() {
        return None;
    }

    let data = value.get("data")?.as_array()?;
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for item in data {
        let Some(slug) = openai_model_id(item) else {
            continue;
        };
        if !seen.insert(slug.to_ascii_lowercase()) {
            continue;
        }
        let display_name = item
            .get("display_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(slug.as_str());
        let description = item
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        models.push(codex_model_info_json(
            slug.as_str(),
            display_name,
            description,
            models.len(),
            provider_epoch.and_then(|epoch| epoch.model(slug.as_str())),
        ));
    }
    if !data.is_empty() && models.is_empty() {
        return None;
    }

    serde_json::to_vec(&CodexModelsResponse { models: &models })
        .ok()
        .map(Bytes::from)
}

fn openai_model_id(item: &Value) -> Option<String> {
    let value = item
        .get("id")
        .or_else(|| item.get("name"))?
        .as_str()?
        .trim();
    let value = value.strip_prefix("models/").unwrap_or(value).trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn codex_model_info_json(
    slug: &str,
    upstream_display_name: &str,
    upstream_description: Option<&str>,
    fallback_priority: usize,
    known: Option<&ProviderModelCapabilities>,
) -> Value {
    if let Some(known) = known {
        return known_codex_model_info_json(known);
    }

    let priority_offset = i32::try_from(fallback_priority).unwrap_or(i32::MAX - 10_000);

    serde_json::json!({
        "slug": slug,
        "display_name": upstream_display_name,
        "description": upstream_description,
        "default_reasoning_level": null,
        "supported_reasoning_levels": [],
        "shell_type": "default",
        "visibility": "list",
        "supported_in_api": true,
        "priority": 10_000_i32.saturating_add(priority_offset),
        "additional_speed_tiers": [],
        "service_tiers": [],
        "default_service_tier": null,
        "availability_nux": null,
        "upgrade": null,
        "base_instructions": "You are Codex, a coding agent.",
        "model_messages": null,
        "supports_reasoning_summary_parameter": false,
        "default_reasoning_summary": "none",
        "support_verbosity": false,
        "default_verbosity": null,
        "apply_patch_tool_type": null,
        "web_search_tool_type": "text",
        "truncation_policy": {"mode": "tokens", "limit": 10_000},
        "supports_parallel_tool_calls": false,
        "supports_image_detail_original": false,
        "context_window": null,
        "max_context_window": null,
        "auto_compact_token_limit": null,
        "effective_context_window_percent": 95,
        "experimental_supported_tools": [],
        "input_modalities": ["text"],
        "supports_search_tool": false,
        "use_responses_lite": false,
    })
}

fn known_codex_model_info_json(known: &ProviderModelCapabilities) -> Value {
    let reasoning_levels = known
        .supported_reasoning_efforts()
        .iter()
        .map(|effort| reasoning_effort(effort.as_str(), reasoning_effort_description(*effort)))
        .collect::<Vec<_>>();
    let supports_fast = known.supports_priority_service_tier();

    serde_json::json!({
        "slug": known.slug(),
        "display_name": known.display_name(),
        "description": known.description(),
        "default_reasoning_level": known.default_reasoning_effort().as_str(),
        "supported_reasoning_levels": reasoning_levels,
        "shell_type": "shell_command",
        "visibility": "list",
        "supported_in_api": true,
        "priority": known.listing_priority(),
        "additional_speed_tiers": if supports_fast { vec!["fast"] } else { Vec::<&str>::new() },
        "service_tiers": service_tiers(supports_fast),
        "default_service_tier": null,
        "availability_nux": null,
        "upgrade": null,
        "base_instructions": "You are Codex, a coding agent based on GPT-5.",
        "model_messages": null,
        "supports_reasoning_summary_parameter": true,
        "default_reasoning_summary": "auto",
        "support_verbosity": true,
        "default_verbosity": null,
        "apply_patch_tool_type": "freeform",
        "web_search_tool_type": "text_and_image",
        "truncation_policy": {"mode": "tokens", "limit": 10_000},
        "supports_parallel_tool_calls": known.supports_parallel_tool_calls(),
        "supports_image_detail_original": known.supports_image_detail_original(),
        "context_window": known.context_window(),
        "max_context_window": known.max_context_window(),
        "auto_compact_token_limit": null,
        "comp_hash": known.comp_hash(),
        "effective_context_window_percent": 95,
        "experimental_supported_tools": [],
        "input_modalities": ["text", "image"],
        "supports_search_tool": true,
        "use_responses_lite": known.uses_responses_lite(),
        "tool_mode": known.tool_mode(),
        "multi_agent_version": known.multi_agent_version(),
    })
}

fn reasoning_effort(effort: &str, description: &str) -> Value {
    serde_json::json!({"effort": effort, "description": description})
}

fn reasoning_effort_description(
    effort: crate::provider_catalog::CatalogReasoningEffort,
) -> &'static str {
    use crate::provider_catalog::CatalogReasoningEffort;
    match effort {
        CatalogReasoningEffort::Low => "Fast responses with lighter reasoning",
        CatalogReasoningEffort::Medium => "Balances speed and reasoning depth for everyday tasks",
        CatalogReasoningEffort::High => "Greater reasoning depth for complex problems",
        CatalogReasoningEffort::Xhigh => "Extra high reasoning depth for complex problems",
        CatalogReasoningEffort::Max => "Maximum reasoning depth for the hardest problems",
        CatalogReasoningEffort::Ultra => "Maximum reasoning with automatic task delegation",
    }
}

fn service_tiers(supports_fast: bool) -> Vec<Value> {
    if supports_fast {
        vec![serde_json::json!({
            "id": "priority",
            "name": "Fast",
            "description": "Faster responses with increased usage",
        })]
    } else {
        Vec::new()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_catalog::{
        AccountFingerprint, ProviderAdapter, ProviderCatalogEpoch, ProviderCatalogScope,
    };
    use axum::http::HeaderValue;

    fn official_epoch() -> ProviderCatalogEpoch {
        let scope = ProviderCatalogScope::new(
            ProviderAdapter::OpenAiCodex,
            "https://api.openai.com/v1",
            "codex/test",
            AccountFingerprint::unscoped(),
            "test-runtime-revision",
        )
        .expect("official provider scope");
        ProviderCatalogEpoch::bundled_openai_codex(scope).expect("official provider epoch")
    }

    #[test]
    fn models_response_preserves_openai_data_list_shape() {
        let body = Bytes::from_static(
            br#"{
                "object": "list",
                "data": [{ "id": "gpt-5.6-sol", "object": "model" }]
            }"#,
        );

        let decoded = maybe_decode_models_response_body(
            "codex",
            "/models",
            &HeaderMap::new(),
            body.clone(),
            ModelsTranslationScope::Disabled,
        );

        assert_eq!(decoded, body);
    }

    #[test]
    fn models_response_translates_openai_data_list_when_enabled() {
        let body = Bytes::from_static(
            br#"{
                "object": "list",
                "data": [
                    {"id": "gpt-5.6-sol", "object": "model", "display_name": "GPT 5.6 Sol"},
                    {"name": "models/gpt-5.6-terra", "object": "model"},
                    {"id": "GPT-5.6-SOL", "object": "model"},
                    {"object": "model"}
                ]
            }"#,
        );

        let epoch = official_epoch();
        let translated = maybe_decode_models_response_body(
            "codex",
            "/v1/models",
            &HeaderMap::new(),
            body,
            ModelsTranslationScope::CapturedCatalog(&epoch),
        );
        let value: serde_json::Value =
            serde_json::from_slice(translated.as_ref()).expect("translated catalog");
        let models = value["models"].as_array().expect("models array");

        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["slug"].as_str(), Some("gpt-5.6-sol"));
        assert_eq!(models[0]["display_name"].as_str(), Some("GPT-5.6-Sol"));
        assert_eq!(models[1]["slug"].as_str(), Some("gpt-5.6-terra"));
        assert_eq!(models[1]["display_name"].as_str(), Some("GPT-5.6-Terra"));
        assert_eq!(models[0]["context_window"].as_i64(), Some(372_000));
        assert_eq!(models[0]["max_context_window"].as_i64(), Some(372_000));
        assert_eq!(models[0]["tool_mode"].as_str(), Some("code_mode_only"));
        assert_eq!(models[0]["use_responses_lite"].as_bool(), Some(true));
        let sol_efforts = models[0]["supported_reasoning_levels"]
            .as_array()
            .expect("Sol reasoning levels")
            .iter()
            .filter_map(|entry| entry["effort"].as_str())
            .collect::<Vec<_>>();
        assert!(sol_efforts.contains(&"max"));
        assert!(sol_efforts.contains(&"ultra"));
        for model in models {
            assert_eq!(model["shell_type"].as_str(), Some("shell_command"));
            assert_eq!(model["visibility"].as_str(), Some("list"));
            assert_eq!(model["supported_in_api"].as_bool(), Some(true));
            assert!(model["supported_reasoning_levels"].is_array());
            assert!(model["base_instructions"].is_string());
            assert!(model["truncation_policy"].is_object());
            assert!(model["experimental_supported_tools"].is_array());
            assert_eq!(
                model["input_modalities"],
                serde_json::json!(["text", "image"])
            );
        }
    }

    #[test]
    fn models_response_preserves_codex_catalog_byte_for_byte() {
        let body = Bytes::from_static(
            br#"{
                "models": [{
                    "slug": "gpt-5.6-sol",
                    "supports_reasoning_summaries": true,
                    "input_modalities": ["text", "image"]
                }]
            }"#,
        );

        let decoded = maybe_decode_models_response_body(
            "codex",
            "/models",
            &HeaderMap::new(),
            body.clone(),
            ModelsTranslationScope::Conservative,
        );

        assert_eq!(decoded, body);
    }

    #[test]
    fn codex_models_path_matches_supported_prefixes_and_trailing_slashes() {
        for path in [
            "/models",
            "/models/",
            "/v1/models",
            "/backend-api/codex/models/",
        ] {
            assert!(codex_path_is_models(path), "{path}");
        }
        for path in ["/model", "/models/item", "/v1/responses", "/"] {
            assert!(!codex_path_is_models(path), "{path}");
        }
    }

    #[test]
    fn models_response_decodes_declared_gzip() {
        let raw = br#"{"object":"list","data":[{"id":"gpt-5.6-sol"}]}"#;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, raw).expect("write gzip body");
        let compressed = encoder.finish().expect("finish gzip body");
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));

        let decoded = maybe_decode_models_response_body(
            "codex",
            "/models",
            &headers,
            Bytes::from(compressed),
            ModelsTranslationScope::Disabled,
        );

        assert_eq!(decoded.as_ref(), raw);
    }

    #[test]
    fn compatible_provider_translation_does_not_invent_official_capabilities() {
        let body = Bytes::from_static(
            br#"{"data":[{"id":"gpt-5.6-sol","display_name":"Relay Sol","description":"Relay-owned model"}]}"#,
        );

        let translated = maybe_decode_models_response_body(
            "codex",
            "/models",
            &HeaderMap::new(),
            body,
            ModelsTranslationScope::Conservative,
        );
        let value: Value = serde_json::from_slice(translated.as_ref()).expect("translated models");
        let model = &value["models"][0];

        assert_eq!(model["display_name"].as_str(), Some("Relay Sol"));
        assert_eq!(model["description"].as_str(), Some("Relay-owned model"));
        assert_eq!(model["shell_type"].as_str(), Some("default"));
        assert_eq!(
            model["supports_reasoning_summary_parameter"].as_bool(),
            Some(false)
        );
        assert_eq!(model["supports_parallel_tool_calls"].as_bool(), Some(false));
        assert_eq!(model["supports_search_tool"].as_bool(), Some(false));
        assert_eq!(model["input_modalities"], serde_json::json!(["text"]));
        assert!(model["context_window"].is_null());
        assert!(model["max_context_window"].is_null());
        assert!(model.get("tool_mode").is_none());
        assert!(model.get("multi_agent_version").is_none());
        assert!(model.get("comp_hash").is_none());
    }

    #[test]
    fn unknown_official_slug_uses_conservative_metadata() {
        let epoch = official_epoch();
        let body = Bytes::from_static(br#"{"data":[{"id":"gpt-5.999-codex"}]}"#);

        let translated = maybe_decode_models_response_body(
            "codex",
            "/models",
            &HeaderMap::new(),
            body,
            ModelsTranslationScope::CapturedCatalog(&epoch),
        );
        let value: Value = serde_json::from_slice(translated.as_ref()).expect("translated models");
        let model = &value["models"][0];

        assert_eq!(model["shell_type"].as_str(), Some("default"));
        assert_eq!(model["supported_reasoning_levels"], serde_json::json!([]));
        assert_eq!(model["supports_search_tool"].as_bool(), Some(false));
        assert!(model["context_window"].is_null());
    }
}

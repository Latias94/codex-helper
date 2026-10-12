use std::collections::HashMap;
use std::io::Read;
use std::sync::LazyLock;

use axum::body::Bytes;
use axum::http::{HeaderMap, header};
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use serde_json::Value;

use crate::basellm_metadata::BasellmOpenAiModelMetadata;

const MAX_DECODED_MODELS_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn merge_model_catalogs(catalogs: &[Value]) -> Option<Value> {
    let basellm_cache = crate::basellm_metadata::load_cached_basellm_metadata_cache();
    merge_model_catalogs_with_basellm_cache(catalogs, basellm_cache.as_ref())
}

fn merge_model_catalogs_with_basellm_cache(
    catalogs: &[Value],
    basellm_cache: Option<&crate::basellm_metadata::BasellmMetadataCache>,
) -> Option<Value> {
    struct CatalogModel<'a> {
        slug: String,
        first: &'a Value,
        native: Option<&'a Value>,
        reasoning_levels: Vec<&'a Value>,
    }

    let mut indices = HashMap::<String, usize>::new();
    let mut entries = Vec::<CatalogModel<'_>>::new();
    let mut has_catalog = false;
    for catalog in catalogs {
        for (key, is_native) in [("models", true), ("data", false)] {
            let Some(models) = catalog.get(key).and_then(Value::as_array) else {
                continue;
            };
            has_catalog = true;
            for model in models {
                // Restrictions apply to this source only; another upstream may expose it.
                if model.get("supported_in_api").and_then(Value::as_bool) == Some(false)
                    || model.get("visibility").and_then(Value::as_str) == Some("hide")
                {
                    continue;
                }
                let slug = catalog_model_id(model, is_native);
                let Some(slug) = slug else {
                    continue;
                };
                let next_index = entries.len();
                let index = *indices
                    .entry(slug.to_ascii_lowercase())
                    .or_insert(next_index);
                if index == next_index {
                    entries.push(CatalogModel {
                        slug,
                        first: model,
                        native: None,
                        reasoning_levels: Vec::new(),
                    });
                }
                let entry = &mut entries[index];
                if is_native && entry.native.is_none() {
                    entry.native = Some(model);
                }
                if let Some(levels) = model
                    .get("supported_reasoning_levels")
                    .and_then(Value::as_array)
                {
                    entry.reasoning_levels.extend(levels);
                }
            }
        }
    }
    if !has_catalog {
        return None;
    }

    let models = entries
        .into_iter()
        .enumerate()
        .map(|(priority, entry)| {
            let official = official_codex_model_info_json(&entry.slug);
            let is_official = official.is_some();
            let mut model = official.unwrap_or_else(|| {
                let metadata = basellm_cache
                    .and_then(|cache| cache.openai_models.get(&entry.slug.to_ascii_lowercase()));
                let display_name = openai_model_display_name(entry.first)
                    .unwrap_or_else(|| display_name(&entry.slug));
                let mut model =
                    codex_model_info_json(&entry.slug, &display_name, priority, metadata);
                if let Some(native) = entry.native.and_then(Value::as_object) {
                    // Fill missing ModelInfo fields without inventing native reasoning support.
                    model["supported_reasoning_levels"] = serde_json::json!([]);
                    model["default_reasoning_level"] = Value::Null;
                    if let Some(fields) = model.as_object_mut() {
                        fields.extend(
                            native
                                .iter()
                                .map(|(key, value)| (key.clone(), value.clone())),
                        );
                    }
                }
                model
            });
            model["slug"] = Value::String(entry.slug);
            let baseline_levels = model
                .get("supported_reasoning_levels")
                .and_then(Value::as_array);
            let levels = [
                "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
            ]
            .into_iter()
            .filter_map(|effort| {
                let mut level = baseline_levels
                    .into_iter()
                    .flatten()
                    .chain(entry.reasoning_levels.iter().copied())
                    .find(|level| level.get("effort").and_then(Value::as_str) == Some(effort))?
                    .clone();
                if level.get("description").and_then(Value::as_str).is_none() {
                    level["description"] = Value::String(format!("{effort} reasoning"));
                }
                Some(level)
            })
            .collect::<Vec<_>>();
            if !is_official
                && !levels.iter().any(|level| {
                    level.get("effort").and_then(Value::as_str)
                        == model.get("default_reasoning_level").and_then(Value::as_str)
                })
            {
                model["default_reasoning_level"] = levels
                    .first()
                    .and_then(|level| level.get("effort"))
                    .cloned()
                    .unwrap_or(Value::Null);
            }
            model["supported_reasoning_levels"] = Value::Array(levels);
            model
        })
        .collect::<Vec<_>>();
    Some(serde_json::json!({ "models": models }))
}

pub(super) fn maybe_decode_models_response_body(
    service_name: &str,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Bytes {
    if service_name != "codex" || path != "/models" {
        return body;
    }

    let body =
        maybe_decode_models_response_body_without_translation(service_name, path, headers, body);
    serde_json::from_slice::<Value>(body.as_ref())
        .ok()
        .and_then(|catalog| merge_model_catalogs(&[catalog]))
        .and_then(|catalog| serde_json::to_vec(&catalog).ok())
        .map(Bytes::from)
        .unwrap_or(body)
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

#[cfg(test)]
fn maybe_translate_openai_models_list(body: &[u8]) -> Option<Bytes> {
    let basellm_cache = crate::basellm_metadata::load_cached_basellm_metadata_cache();
    translate_openai_models_list_with_basellm_cache(body, basellm_cache.as_ref())
}

#[cfg(test)]
fn translate_openai_models_list_with_basellm_cache(
    body: &[u8],
    basellm_cache: Option<&crate::basellm_metadata::BasellmMetadataCache>,
) -> Option<Bytes> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    let catalog = merge_model_catalogs_with_basellm_cache(&[value], basellm_cache)?;
    serde_json::to_vec(&catalog).ok().map(Bytes::from)
}

pub(super) fn catalog_model_id(item: &Value, native: bool) -> Option<String> {
    if native {
        return item
            .get("slug")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|slug| !slug.is_empty())
            .map(ToOwned::to_owned);
    }
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

fn official_codex_model_info_json(slug: &str) -> Option<Value> {
    static CATALOG: LazyLock<Value> = LazyLock::new(|| {
        serde_json::from_str(include_str!("model_catalogs/openai-codex-0.162.1.json"))
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "Failed to parse bundled Codex model metadata");
                Value::Null
            })
    });

    let normalized = slug.trim().to_ascii_lowercase();
    let bedrock_alias = normalized.starts_with("openai.");
    let canonical_slug = normalized.strip_prefix("openai.").unwrap_or(&normalized);
    let mut model = CATALOG
        .get("models")?
        .as_array()?
        .iter()
        .find(|model| model.get("slug").and_then(Value::as_str) == Some(canonical_slug))?
        .clone();
    model["slug"] = Value::String(slug.to_string());
    if bedrock_alias {
        model["service_tiers"] = serde_json::json!([]);
        model["additional_speed_tiers"] = serde_json::json!([]);
        model["default_service_tier"] = Value::Null;
    }
    Some(model)
}

fn codex_model_info_json(
    slug: &str,
    display_name: &str,
    fallback_priority: usize,
    basellm_metadata: Option<&BasellmOpenAiModelMetadata>,
) -> Value {
    if let Some(model) = official_codex_model_info_json(slug) {
        return model;
    }

    let known = known_codex_model(slug, basellm_metadata);
    let caps = known.capabilities;
    let priority = known
        .priority
        .unwrap_or_else(|| 10_000 + i32::try_from(fallback_priority).unwrap_or(0));

    serde_json::json!({
        "slug": slug,
        "display_name": known.display_name.as_deref().unwrap_or(display_name),
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

    const fn bedrock_gpt_5_6() -> Self {
        Self {
            reasoning_levels: ReasoningLevels::ModernWithMax,
            ..Self::modern_gpt()
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

    fn with_basellm_metadata(
        mut self,
        metadata: &BasellmOpenAiModelMetadata,
        allow_input_modalities_overlay: bool,
    ) -> Self {
        if metadata.supports_fast_priority {
            self.fast_service_tier = true;
        }
        if allow_input_modalities_overlay && !metadata.input_modalities.is_empty() {
            self.text_input = metadata
                .input_modalities
                .iter()
                .any(|modality| modality.eq_ignore_ascii_case("text"));
            self.image_input = metadata
                .input_modalities
                .iter()
                .any(|modality| modality.eq_ignore_ascii_case("image"));
            if !self.image_input {
                self.supports_image_detail_original = false;
            }
        }
        if metadata.reasoning == Some(false) {
            self.reasoning_levels = ReasoningLevels::None;
            self.default_reasoning_level = None;
            self.supports_reasoning_summaries = false;
        }
        if metadata.tool_call == Some(false) {
            self.shell_type = "disabled";
            self.apply_patch_tool_type = None;
            self.supports_parallel_tool_calls = false;
        }
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
    ModernWithMax,
    Gpt52,
}

impl ReasoningLevels {
    fn json(self) -> Value {
        match self {
            Self::None => serde_json::json!([]),
            Self::Modern | Self::ModernWithMax => {
                let mut levels = vec![
                    serde_json::json!({
                        "effort": "low",
                        "description": "Fast responses with lighter reasoning"
                    }),
                    serde_json::json!({
                        "effort": "medium",
                        "description": "Balances speed and reasoning depth for everyday tasks"
                    }),
                    serde_json::json!({
                        "effort": "high",
                        "description": "Greater reasoning depth for complex problems"
                    }),
                    serde_json::json!({
                        "effort": "xhigh",
                        "description": "Extra high reasoning depth for complex problems"
                    }),
                ];
                if matches!(self, Self::ModernWithMax) {
                    levels.push(serde_json::json!({
                        "effort": "max",
                        "description": "Maximum reasoning depth for the hardest problems"
                    }));
                }
                serde_json::json!(levels)
            }
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

fn is_codex_text_only_slug(slug: &str) -> bool {
    slug == "gpt-5.3-codex-spark" || slug.contains("codex-spark")
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
    display_name: Option<String>,
    description: Option<String>,
    priority: Option<i32>,
    context_window: i64,
    max_context_window: i64,
    hidden: bool,
    capabilities: ModelCapabilities,
}

fn known_codex_model(
    slug: &str,
    basellm_metadata: Option<&BasellmOpenAiModelMetadata>,
) -> KnownCodexModel {
    let normalized = slug.trim().to_ascii_lowercase();
    let mut known = match normalized.as_str() {
        "gpt-5.5" => KnownCodexModel {
            display_name: Some("GPT-5.5".to_string()),
            description: Some(
                "Frontier model for complex coding, research, and real-world work.".to_string(),
            ),
            priority: Some(0),
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: false,
            capabilities: ModelCapabilities::modern_gpt().with_fast_service_tier(),
        },
        "gpt-5.4" => KnownCodexModel {
            display_name: Some("gpt-5.4".to_string()),
            description: Some("Strong model for everyday coding.".to_string()),
            priority: Some(10),
            context_window: 272_000,
            max_context_window: 1_000_000,
            hidden: false,
            capabilities: ModelCapabilities::modern_gpt().with_fast_service_tier(),
        },
        "gpt-5.4-mini" => KnownCodexModel {
            display_name: Some("GPT-5.4-Mini".to_string()),
            description: Some(
                "Small, fast, and cost-efficient model for simpler coding tasks.".to_string(),
            ),
            priority: Some(20),
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: false,
            capabilities: ModelCapabilities::modern_gpt()
                .with_default_verbosity("medium")
                .with_fast_service_tier(),
        },
        "gpt-5.3-codex" => KnownCodexModel {
            display_name: Some("GPT-5.3 Codex".to_string()),
            description: Some("Coding-optimized model.".to_string()),
            priority: Some(30),
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: false,
            capabilities: ModelCapabilities::modern_gpt()
                .with_web_search_type("text")
                .with_fast_service_tier(),
        },
        "gpt-5.3-codex-spark" => KnownCodexModel {
            display_name: Some("GPT-5.3 Codex Spark".to_string()),
            description: Some("Coding-optimized model with limited image support.".to_string()),
            priority: Some(40),
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: false,
            capabilities: ModelCapabilities::codex_spark(),
        },
        "openai.gpt-5.5" => KnownCodexModel {
            display_name: Some("GPT-5.5".to_string()),
            description: Some(
                "Frontier model for complex coding, research, and real-world work.".to_string(),
            ),
            priority: Some(0),
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: false,
            capabilities: ModelCapabilities::modern_gpt(),
        },
        "openai.gpt-5.4" => KnownCodexModel {
            display_name: Some("gpt-5.4".to_string()),
            description: Some("Strong model for everyday coding.".to_string()),
            priority: Some(1),
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: false,
            capabilities: ModelCapabilities::modern_gpt(),
        },
        "openai.gpt-5.6-sol" | "gpt-5.6-sol" => KnownCodexModel {
            display_name: Some("Sol".to_string()),
            description: Some(
                "Frontier model for complex coding, research, and real-world work.".to_string(),
            ),
            priority: Some(2),
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: false,
            capabilities: ModelCapabilities::bedrock_gpt_5_6(),
        },
        "openai.gpt-5.6-terra" | "gpt-5.6-terra" => KnownCodexModel {
            display_name: Some("Terra".to_string()),
            description: Some(
                "Frontier model for complex coding, research, and real-world work.".to_string(),
            ),
            priority: Some(3),
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: false,
            capabilities: ModelCapabilities::bedrock_gpt_5_6(),
        },
        "openai.gpt-5.6-luna" | "gpt-5.6-luna" => KnownCodexModel {
            display_name: Some("Luna".to_string()),
            description: Some(
                "Frontier model for complex coding, research, and real-world work.".to_string(),
            ),
            priority: Some(4),
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: false,
            capabilities: ModelCapabilities::bedrock_gpt_5_6(),
        },
        "gpt-5.2" => KnownCodexModel {
            display_name: Some("GPT-5.2".to_string()),
            description: Some(
                "Optimized for professional work and long-running agents.".to_string(),
            ),
            priority: Some(50),
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: false,
            capabilities: ModelCapabilities::gpt_5_2(),
        },
        "codex-auto-review" => KnownCodexModel {
            display_name: Some("Codex Auto Review".to_string()),
            description: Some("Internal review model.".to_string()),
            priority: Some(50_000),
            context_window: 272_000,
            max_context_window: 272_000,
            hidden: true,
            capabilities: ModelCapabilities::modern_gpt(),
        },
        _ if normalized.starts_with("gpt-image-") => KnownCodexModel {
            display_name: None,
            description: Some("Image model served by the configured Codex upstream.".to_string()),
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
                description: Some("Model served by the configured Codex upstream.".to_string()),
                priority: None,
                context_window: 272_000,
                max_context_window: 272_000,
                hidden: false,
                capabilities,
            }
        }
    };
    if let Some(metadata) = basellm_metadata {
        known = known.with_basellm_metadata(metadata);
    }
    known
}

impl KnownCodexModel {
    fn with_basellm_metadata(mut self, metadata: &BasellmOpenAiModelMetadata) -> Self {
        if self.display_name.is_none() {
            self.display_name = metadata
                .display_name
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
        }
        if self.description.is_none()
            || self.description.as_deref() == Some("Model served by the configured Codex upstream.")
        {
            self.description = metadata
                .description
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
        }
        let allow_input_modalities_overlay = !is_codex_text_only_slug(&metadata.model_id)
            && !metadata.model_id.starts_with("gpt-image-");
        self.capabilities = self
            .capabilities
            .with_basellm_metadata(metadata, allow_input_modalities_overlay);
        self
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
    use axum::http::HeaderValue;

    #[test]
    fn merged_catalog_combines_upstream_efforts_without_maximizing_other_fields() {
        let mut first = codex_model_info_json("custom-model", "First upstream", 0, None);
        first["context_window"] = serde_json::json!(32_000);
        first["max_context_window"] = serde_json::json!(64_000);
        first["default_reasoning_level"] = serde_json::json!("high");
        first["upstream_extension"] = serde_json::json!({ "source": "first" });
        first["supported_reasoning_levels"] = serde_json::json!([
            { "effort": "high", "description": "First high" },
            { "effort": "none", "description": "No reasoning" },
            { "effort": "high", "description": "Duplicate high" },
            { "effort": "future-effort", "description": "Unsupported effort" }
        ]);
        let mut second = first.clone();
        second["display_name"] = serde_json::json!("Second upstream");
        second["context_window"] = serde_json::json!(1_000_000);
        second["max_context_window"] = serde_json::json!(2_000_000);
        second["supports_parallel_tool_calls"] = serde_json::json!(true);
        second["input_modalities"] = serde_json::json!(["text", "image"]);
        second["supported_reasoning_levels"] = serde_json::json!([
            { "effort": "ultra", "description": "Ultra" },
            { "effort": "medium", "description": "Medium" },
            { "effort": "minimal", "description": "Minimal" },
            { "effort": "max", "description": "Max" },
            { "effort": "xhigh", "description": "Extra high" },
            { "effort": "low", "description": "Low" },
            { "effort": "high", "description": "Second high" },
            { "effort": "future-effort", "description": "Unsupported effort" },
            null,
            { "effort": 42 }
        ]);
        let merged = merge_model_catalogs(&[
            serde_json::json!({ "models": [first.clone()] }),
            serde_json::json!({ "models": [second] }),
        ])
        .expect("valid catalogs should merge");
        assert_eq!(merged["models"].as_array().unwrap().len(), 1);
        let model = model_by_slug(&merged, "custom-model");
        assert_reasoning_efforts(
            model,
            &[
                "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
            ],
        );
        assert_eq!(
            model["supported_reasoning_levels"][4]["description"],
            "First high"
        );
        first["supported_reasoning_levels"] = model["supported_reasoning_levels"].clone();
        assert_eq!(model, &first);
    }

    #[test]
    fn merged_catalog_prefers_unknown_native_record_over_raw_id_in_either_order() {
        let mut native = codex_model_info_json("gpt-7-custom", "Native custom", 0, None);
        native["default_reasoning_level"] = serde_json::json!("none");
        native["supported_reasoning_levels"] = serde_json::json!([
            { "effort": "none", "description": "Native no reasoning" }
        ]);
        native["context_window"] = serde_json::json!(16_000);
        native["max_context_window"] = serde_json::json!(16_000);
        native["supports_parallel_tool_calls"] = serde_json::json!(false);
        native["input_modalities"] = serde_json::json!(["text"]);
        native["native_extension"] = serde_json::json!({ "retain": true });
        let raw_catalog = serde_json::json!({ "data": [{ "id": "gpt-7-custom" }] });
        let native_catalog = serde_json::json!({ "models": [native.clone()] });
        for catalogs in [
            [raw_catalog.clone(), native_catalog.clone()],
            [native_catalog, raw_catalog],
        ] {
            let merged = merge_model_catalogs(&catalogs).expect("valid catalogs should merge");
            assert_eq!(merged, serde_json::json!({ "models": [native.clone()] }));
        }
    }

    #[test]
    fn merged_catalog_preserves_official_gpt_6_1_record_and_unions_reported_efforts() {
        let mut reported = codex_model_info_json("Gpt-6.1-Sol", "Upstream override", 0, None);
        reported["default_reasoning_level"] = serde_json::json!("high");
        reported["context_window"] = serde_json::json!(12_000);
        reported["supports_parallel_tool_calls"] = serde_json::json!(false);
        reported["supported_reasoning_levels"] = serde_json::json!([
            { "effort": "minimal", "description": "Upstream minimal" },
            { "effort": "high", "description": "Upstream high" },
            { "effort": "unsupported", "description": "Unsupported effort" }
        ]);
        let merged = merge_model_catalogs(&[
            serde_json::json!({ "models": [reported] }),
            serde_json::json!({ "data": [{ "id": "gpt-6.1-sol" }] }),
        ])
        .expect("valid catalogs should merge");
        let model = model_by_slug(&merged, "Gpt-6.1-Sol");
        assert_reasoning_efforts(
            model,
            &["minimal", "low", "medium", "high", "xhigh", "max", "ultra"],
        );
        assert_eq!(model["default_reasoning_level"], "low");
        let mut official = official_codex_model_info_json("Gpt-6.1-Sol").unwrap();
        assert_eq!(
            model["supported_reasoning_levels"][3],
            official["supported_reasoning_levels"][2]
        );
        official["supported_reasoning_levels"] = model["supported_reasoning_levels"].clone();
        assert_eq!(model, &official);
        assert_eq!(merged["models"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn merged_catalog_members_come_only_from_valid_visible_sources() {
        let filtered = serde_json::json!({ "models": [
            { "slug": "gpt-6-sol", "supported_in_api": false },
            { "slug": "gpt-6-luna", "visibility": "hide" },
            { "slug": "gpt-6.1-sol", "visibility": "hide" },
            { "slug": 42 }, null, { "slug": " " }
        ] });
        let raw = serde_json::json!({ "data": [
            { "id": "Gpt-6.1-Sol" },
            { "id": "gpt-image-1" },
            { "id": "gpt-6-astra", "supported_in_api": false },
            { "id": "gpt-5.5", "visibility": "hide" },
            { "id": "" }, { "id": 7 }, null
        ] });
        let merged = merge_model_catalogs(&[
            Value::Null,
            serde_json::json!({ "models": "broken", "data": {} }),
            filtered,
            raw,
        ])
        .expect("valid members should survive invalid catalogs and records");
        let models = merged["models"].as_array().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["slug"], "Gpt-6.1-Sol");
        assert_eq!(models[1]["slug"], "gpt-image-1");
        assert_eq!(models[1]["supported_in_api"], false);
        assert_eq!(models[1]["visibility"], "hide");
    }

    #[test]
    fn merged_catalog_completes_unknown_partial_native_without_inventing_efforts() {
        let merged = merge_model_catalogs(&[serde_json::json!({ "models": [
            { "slug": "gpt-7-partial", "supported_reasoning_levels": [
                { "effort": "minimal" }, { "effort": "provider-specific" }
            ], "default_reasoning_level": "provider-specific", "context_window": 12_345 },
            { "slug": "gpt-7-no-efforts", "default_reasoning_level": "high" }
        ] })])
        .expect("partial native models should merge");
        let model = model_by_slug(&merged, "gpt-7-partial");
        assert_eq!(model["context_window"], 12_345);
        assert_eq!(model["display_name"], "GPT 7 Partial");
        assert_eq!(model["shell_type"], "shell_command");
        assert_eq!(
            model["input_modalities"],
            serde_json::json!(["text", "image"])
        );
        assert!(model["base_instructions"].is_string());
        assert_reasoning_efforts(model, &["minimal"]);
        assert!(model["supported_reasoning_levels"][0]["description"].is_string());
        assert_eq!(model["default_reasoning_level"], "minimal");
        let no_efforts = model_by_slug(&merged, "gpt-7-no-efforts");
        assert_reasoning_efforts(no_efforts, &[]);
        assert_eq!(no_efforts["default_reasoning_level"], Value::Null);
    }

    #[test]
    fn merged_catalog_repairs_unknown_default_after_union_and_preserves_valid_default() {
        let first = serde_json::json!({ "models": [{ "slug": "custom-model", "default_reasoning_level": "ultra",
            "supported_reasoning_levels": [{ "effort": "none", "description": "None" }] }] });
        let second = serde_json::json!({ "models": [{ "slug": "custom-model", "supported_reasoning_levels": [
            { "effort": "minimal", "description": "Minimal" }
        ] }] });
        let merged =
            merge_model_catalogs(&[first.clone(), second]).expect("valid catalogs should merge");
        let model = model_by_slug(&merged, "custom-model");
        assert_reasoning_efforts(model, &["none", "minimal"]);
        assert_eq!(model["default_reasoning_level"], "none");

        let ultra_source = serde_json::json!({ "models": [{ "slug": "custom-model", "supported_reasoning_levels": [
            { "effort": "ultra", "description": "Ultra" }
        ] }] });
        let merged =
            merge_model_catalogs(&[first, ultra_source]).expect("valid catalogs should merge");
        let model = model_by_slug(&merged, "custom-model");
        assert_reasoning_efforts(model, &["none", "ultra"]);
        assert_eq!(model["default_reasoning_level"], "ultra");
    }

    #[test]
    fn merged_catalog_distinguishes_empty_catalogs_from_invalid_responses() {
        assert_eq!(merge_model_catalogs(&[]), None);
        assert_eq!(
            merge_model_catalogs(&[serde_json::json!({ "error": "unavailable" })]),
            None
        );
        assert_eq!(
            merge_model_catalogs(&[serde_json::json!({ "models": [] })]),
            Some(serde_json::json!({ "models": [] }))
        );
        assert_eq!(
            merge_model_catalogs(&[serde_json::json!({ "data": [] })]),
            Some(serde_json::json!({ "models": [] }))
        );
    }

    #[test]
    fn merged_catalog_preserves_bedrock_aliases_with_default_service_tier() {
        let merged = merge_model_catalogs(&[serde_json::json!({ "data": [
            { "id": "openai.gpt-6.1-sol" },
            { "id": "OpenAI.gpt-5.6-sol" },
            { "id": "gpt-6.1-sol" }
        ] })])
        .expect("valid catalog should merge");
        assert_eq!(merged["models"].as_array().unwrap().len(), 3);
        for slug in ["openai.gpt-6.1-sol", "OpenAI.gpt-5.6-sol"] {
            let model = model_by_slug(&merged, slug);
            assert_eq!(model["default_service_tier"], Value::Null);
            assert!(!model_has_fast_service_tier(model));
            assert!(!model_has_legacy_fast_speed_tier(model));
            assert_reasoning_efforts(model, &["low", "medium", "high", "xhigh", "max", "ultra"]);
            assert_eq!(model["default_reasoning_level"], "low");
        }
        assert!(model_has_fast_service_tier(model_by_slug(
            &merged,
            "gpt-6.1-sol"
        )));
    }

    #[test]
    fn bundled_catalog_contains_all_eleven_complete_official_records() {
        let catalog: Value =
            serde_json::from_str(include_str!("model_catalogs/openai-codex-0.162.1.json")).unwrap();
        let models = catalog["models"].as_array().unwrap();
        assert_eq!(models.len(), 11);
        for model in models {
            let slug = model["slug"].as_str().unwrap();
            assert_eq!(official_codex_model_info_json(slug), Some(model.clone()));
            let raw = serde_json::json!({ "data": [{ "id": slug }] });
            let merged = merge_model_catalogs(&[raw]).expect("valid catalog should merge");
            assert_eq!(model_by_slug(&merged, slug), model);
        }
        assert_eq!(
            model_by_slug(&catalog, "gpt-5.6-terra")["multi_agent_version"],
            "v2"
        );
        assert_eq!(
            model_by_slug(&catalog, "gpt-daybreak-red-latest")["context_window"],
            372_000
        );
        assert_reasoning_efforts(
            model_by_slug(&catalog, "codex-auto-review"),
            &["low", "medium", "high", "xhigh", "max"],
        );
    }

    #[test]
    fn decode_models_response_normalizes_openai_list_by_default() {
        let body = Bytes::from_static(
            br#"{
            "object": "list",
            "data": [
                { "id": "gpt-5.5", "object": "model", "display_name": "GPT-5.5" }
            ]
        }"#,
        );

        let decoded =
            maybe_decode_models_response_body("codex", "/models", &HeaderMap::new(), body);
        let value: Value = serde_json::from_slice(decoded.as_ref()).expect("decoded JSON");

        assert!(value.get("data").is_none());
        assert_eq!(
            model_by_slug(&value, "gpt-5.5")
                .get("context_window")
                .and_then(Value::as_i64),
            Some(272_000)
        );
    }

    #[test]
    fn decode_models_response_without_translation_preserves_compressed_raw_catalog() {
        let raw = br#"{"object":"list","data":[{"id":"gpt-5.5"}]}"#;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, raw).expect("write gzip body");
        let compressed = encoder.finish().expect("finish gzip body");
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));

        let decoded = maybe_decode_models_response_body_without_translation(
            "codex",
            "/models",
            &headers,
            Bytes::from(compressed),
        );
        let value: Value = serde_json::from_slice(decoded.as_ref()).expect("decoded JSON");

        assert_eq!(decoded.as_ref(), raw);
        assert!(value.get("models").is_none());
        assert!(value.get("data").and_then(Value::as_array).is_some());
    }

    #[test]
    fn decode_models_response_normalizes_compressed_native_catalog_by_default() {
        let raw = br#"{"models":[{"slug":"gpt-6.1-sol","default_reasoning_level":"high","supported_reasoning_levels":[{"effort":"minimal","description":"Minimal"}]}]}"#;
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
        );
        let catalog: Value = serde_json::from_slice(decoded.as_ref()).expect("normalized JSON");
        let model = model_by_slug(&catalog, "gpt-6.1-sol");
        assert_eq!(model["default_reasoning_level"], "low");
        assert_reasoning_efforts(
            model,
            &["minimal", "low", "medium", "high", "xhigh", "max", "ultra"],
        );
    }

    #[test]
    fn decode_models_response_preserves_unrelated_and_invalid_bodies() {
        let raw = Bytes::from_static(br#"{"data":[{"id":"gpt-6.1-sol"}]}"#);
        for (service_name, path) in [("claude", "/models"), ("codex", "/responses")] {
            assert_eq!(
                maybe_decode_models_response_body(
                    service_name,
                    path,
                    &HeaderMap::new(),
                    raw.clone()
                ),
                raw
            );
        }
        for raw in [
            Bytes::from_static(b"{broken JSON"),
            Bytes::from_static(br#"{"error":"unavailable"}"#),
        ] {
            assert_eq!(
                maybe_decode_models_response_body(
                    "codex",
                    "/models",
                    &HeaderMap::new(),
                    raw.clone()
                ),
                raw
            );
        }
    }

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
    fn translated_openai_models_list_supports_bedrock_gpt_5_6_variants() {
        let value = translated_models_value(&[
            "openai.gpt-5.6-sol",
            "openai.gpt-5.6-terra",
            "openai.gpt-5.6-luna",
        ]);

        for (slug, display_name, priority, default_effort, supports_ultra) in [
            ("openai.gpt-5.6-sol", "GPT-5.6-Sol", 5, "low", true),
            ("openai.gpt-5.6-terra", "GPT-5.6-Terra", 8, "medium", true),
            ("openai.gpt-5.6-luna", "GPT-5.6-Luna", 9, "medium", false),
        ] {
            let model = model_by_slug(&value, slug);
            assert_eq!(
                model.get("display_name").and_then(Value::as_str),
                Some(display_name)
            );
            assert_eq!(
                model.get("priority").and_then(Value::as_i64),
                Some(priority)
            );
            assert_eq!(
                model.get("context_window").and_then(Value::as_i64),
                Some(272_000)
            );
            assert_eq!(
                model.get("max_context_window").and_then(Value::as_i64),
                Some(872_000)
            );
            assert_eq!(model["default_reasoning_level"], default_effort);
            let mut efforts = vec!["low", "medium", "high", "xhigh", "max"];
            if supports_ultra {
                efforts.push("ultra");
            }
            assert_reasoning_efforts(model, &efforts);
            assert_input_modalities(model, &["text", "image"]);
            assert!(!model_has_fast_service_tier(model));
            assert!(!model_has_legacy_fast_speed_tier(model));
        }
    }

    #[test]
    fn translated_openai_models_list_keeps_bedrock_gpt_5_4_and_5_5_on_default_tier() {
        let value = translated_models_value(&["openai.gpt-5.5", "openai.gpt-5.4"]);

        for (slug, priority) in [("openai.gpt-5.5", 13), ("openai.gpt-5.4", 1)] {
            let model = model_by_slug(&value, slug);
            assert_eq!(
                model.get("priority").and_then(Value::as_i64),
                Some(priority)
            );
            assert_eq!(
                model.get("context_window").and_then(Value::as_i64),
                Some(272_000)
            );
            assert_eq!(
                model.get("max_context_window").and_then(Value::as_i64),
                Some(272_000)
            );
            assert_reasoning_efforts(model, &["low", "medium", "high", "xhigh"]);
            assert_input_modalities(model, &["text", "image"]);
            assert!(!model_has_fast_service_tier(model));
            assert!(!model_has_legacy_fast_speed_tier(model));
        }
    }

    #[test]
    fn translated_openai_models_list_uses_official_tiers_for_canonical_gpt_5_6_slug() {
        let value = translated_models_value(&["gpt-5.6-sol"]);
        let model = model_by_slug(&value, "gpt-5.6-sol");

        assert_eq!(
            model.get("display_name").and_then(Value::as_str),
            Some("GPT-5.6-Sol")
        );
        assert_reasoning_efforts(model, &["low", "medium", "high", "xhigh", "max", "ultra"]);
        assert!(model_has_fast_service_tier(model));
        assert!(model_has_legacy_fast_speed_tier(model));
    }

    #[test]
    fn translated_openai_models_list_preserves_gpt_6_1_sol_reasoning_capabilities() {
        for slug in ["gpt-6.1-sol", "openai.gpt-6.1-sol", "Gpt-6.1-Sol"] {
            let value = translated_models_value(&[slug]);
            let model = model_by_slug(&value, slug);

            assert_eq!(model["display_name"], "GPT-6.1-Sol");
            assert_eq!(model["default_reasoning_level"], "low");
            assert_reasoning_efforts(model, &["low", "medium", "high", "xhigh", "max", "ultra"]);
            assert_eq!(model["visibility"], "list");
            assert_eq!(model["supported_in_api"], true);
            assert_eq!(model["context_window"], 272_000);
            assert_eq!(model["max_context_window"], 872_000);
            assert_eq!(model["use_responses_lite"], true);
            assert_eq!(model["multi_agent_version"], "v2");
            assert_eq!(model["multi_agent_reasoning_effort"], "xhigh");
            assert_eq!(
                model_has_fast_service_tier(model),
                !slug.starts_with("openai.")
            );
        }
    }

    #[test]
    fn translated_openai_models_list_retains_gpt_6_peer_reasoning_limits() {
        let value = translated_models_value(&["gpt-6-astra", "gpt-6-sol", "gpt-6-luna"]);
        for (slug, name, default_effort, supports_ultra) in [
            ("gpt-6-astra", "GPT-6-Astra", "low", true),
            ("gpt-6-sol", "GPT-6-Sol", "medium", true),
            ("gpt-6-luna", "GPT-6-Luna", "medium", false),
        ] {
            let model = model_by_slug(&value, slug);
            assert_eq!(model["display_name"], name);
            assert_eq!(model["default_reasoning_level"], default_effort);
            let mut efforts = vec!["low", "medium", "high", "xhigh", "max"];
            if supports_ultra {
                efforts.push("ultra");
            }
            assert_reasoning_efforts(model, &efforts);

            let alias = format!("openai.{slug}");
            let alias_value = translated_models_value(&[alias.as_str()]);
            let alias_model = model_by_slug(&alias_value, &alias);
            assert_reasoning_efforts(alias_model, &efforts);
            assert_eq!(alias_model["default_service_tier"], Value::Null);
            assert!(!model_has_fast_service_tier(alias_model));
            assert!(!model_has_legacy_fast_speed_tier(alias_model));
        }
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

    #[test]
    fn translated_openai_models_list_uses_basellm_fast_metadata_overlay() {
        let body = br#"{
            "object": "list",
            "data": [
                { "id": "gpt-5.3-codex" }
            ]
        }"#;
        let basellm_cache = crate::basellm_metadata::parse_basellm_openai_metadata_json(
            r#"{
              "openai": {
                "models": {
                  "gpt-5.3-codex": {
                    "name": "GPT-5.3 Codex From BaseLLM",
                    "description": "Metadata sourced from BaseLLM.",
                    "limit": { "context": 400000, "input": 272000 },
                    "modalities": { "input": ["text", "image", "pdf"] },
                    "reasoning": true,
                    "tool_call": true,
                    "experimental": {
                      "modes": {
                        "fast": {
                          "provider": { "body": { "service_tier": "priority" } }
                        }
                      }
                    }
                  }
                }
              }
            }"#,
        )
        .expect("parse BaseLLM metadata");

        let translated =
            translate_openai_models_list_with_basellm_cache(body, Some(&basellm_cache))
                .expect("OpenAI models list should translate to Codex catalog");
        let value: serde_json::Value =
            serde_json::from_slice(translated.as_ref()).expect("translated JSON");
        let model = model_by_slug(&value, "gpt-5.3-codex");

        assert!(model_has_fast_service_tier(model));
        assert_eq!(
            model.get("context_window").and_then(Value::as_i64),
            Some(272_000)
        );
        assert_eq!(
            model.get("max_context_window").and_then(Value::as_i64),
            Some(272_000)
        );
        assert_input_modalities(model, &["text", "image"]);
    }

    #[test]
    fn basellm_overlay_does_not_override_known_codex_model_context_windows() {
        let body = br#"{
            "object": "list",
            "data": [
                { "id": "gpt-5.5" }
            ]
        }"#;
        let basellm_cache = crate::basellm_metadata::parse_basellm_openai_metadata_json(
            r#"{
              "openai": {
                "models": {
                  "gpt-5.5": {
                    "name": "GPT-5.5",
                    "description": "Metadata sourced from BaseLLM.",
                    "limit": { "context": 1050000, "input": 922000 },
                    "modalities": { "input": ["text", "image"] },
                    "reasoning": true,
                    "tool_call": true,
                    "experimental": {
                      "modes": {
                        "fast": {
                          "provider": { "body": { "service_tier": "priority" } }
                        }
                      }
                    }
                  }
                }
              }
            }"#,
        )
        .expect("parse BaseLLM metadata");

        let translated =
            translate_openai_models_list_with_basellm_cache(body, Some(&basellm_cache))
                .expect("OpenAI models list should translate to Codex catalog");
        let value: serde_json::Value =
            serde_json::from_slice(translated.as_ref()).expect("translated JSON");
        let model = model_by_slug(&value, "gpt-5.5");

        assert!(model_has_fast_service_tier(model));
        assert_eq!(
            model.get("context_window").and_then(Value::as_i64),
            Some(272_000)
        );
        assert_eq!(
            model.get("max_context_window").and_then(Value::as_i64),
            Some(272_000)
        );
        assert_input_modalities(model, &["text", "image"]);
    }

    #[test]
    fn translated_openai_models_list_can_discover_fast_for_unknown_basellm_gpt_model() {
        let body = br#"{
            "object": "list",
            "data": [
                { "id": "gpt-5.3-lab" }
            ]
        }"#;
        let basellm_cache = crate::basellm_metadata::parse_basellm_openai_metadata_json(
            r#"{
              "openai": {
                "models": {
                  "gpt-5.3-lab": {
                    "name": "GPT-5.3 Lab",
                    "limit": { "context": 128000, "input": 96000 },
                    "modalities": { "input": ["text"] },
                    "reasoning": true,
                    "tool_call": true,
                    "experimental": {
                      "modes": {
                        "fast": {
                          "provider": { "body": { "service_tier": "priority" } }
                        }
                      }
                    }
                  }
                }
              }
            }"#,
        )
        .expect("parse BaseLLM metadata");

        let translated =
            translate_openai_models_list_with_basellm_cache(body, Some(&basellm_cache))
                .expect("OpenAI models list should translate to Codex catalog");
        let value: serde_json::Value =
            serde_json::from_slice(translated.as_ref()).expect("translated JSON");
        let model = model_by_slug(&value, "gpt-5.3-lab");

        assert!(model_has_fast_service_tier(model));
        assert_eq!(
            model.get("display_name").and_then(Value::as_str),
            Some("GPT-5.3 Lab")
        );
        assert_eq!(
            model.get("context_window").and_then(Value::as_i64),
            Some(272_000)
        );
        assert_eq!(
            model.get("max_context_window").and_then(Value::as_i64),
            Some(272_000)
        );
        assert_input_modalities(model, &["text"]);
    }

    #[test]
    fn basellm_overlay_does_not_reenable_codex_spark_image_input() {
        let body = br#"{
            "object": "list",
            "data": [
                { "id": "gpt-5.3-codex-spark" }
            ]
        }"#;
        let basellm_cache = crate::basellm_metadata::parse_basellm_openai_metadata_json(
            r#"{
              "openai": {
                "models": {
                  "gpt-5.3-codex-spark": {
                    "name": "GPT-5.3 Codex Spark",
                    "limit": { "context": 128000, "input": 100000 },
                    "modalities": { "input": ["text", "image", "pdf"] },
                    "reasoning": true,
                    "tool_call": true
                  }
                }
              }
            }"#,
        )
        .expect("parse BaseLLM metadata");

        let translated =
            translate_openai_models_list_with_basellm_cache(body, Some(&basellm_cache))
                .expect("OpenAI models list should translate to Codex catalog");
        let value: serde_json::Value =
            serde_json::from_slice(translated.as_ref()).expect("translated JSON");
        let model = model_by_slug(&value, "gpt-5.3-codex-spark");

        assert_input_modalities(model, &["text"]);
        assert_eq!(
            model
                .get("supports_image_detail_original")
                .and_then(Value::as_bool),
            Some(false)
        );
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

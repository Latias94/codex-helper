use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::header::{
    ETAG, HeaderMap, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::fs;
use tracing::{debug, info, warn};

use crate::config::proxy_home_dir;
use crate::pricing::basellm_all_json_url;

const BASELLM_RESPONSE_BODY_LIMIT: usize = 16 * 1024 * 1024;
const BASELLM_CACHE_MAX_BYTES: u64 = 2 * 1024 * 1024;
const BASELLM_SYNC_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BasellmMetadataCache {
    pub source_url: String,
    pub fetched_at_unix: i64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub openai_models: BTreeMap<String, BasellmOpenAiModelMetadata>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BasellmOpenAiModelMetadata {
    pub model_id: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub context_window: Option<i64>,
    pub max_context_window: Option<i64>,
    pub input_modalities: Vec<String>,
    pub reasoning: Option<bool>,
    pub tool_call: Option<bool>,
    pub structured_output: Option<bool>,
    pub supports_fast_priority: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasellmMetadataSyncStatus {
    Updated,
    NotModified,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasellmMetadataSyncReport {
    pub status: BasellmMetadataSyncStatus,
    pub model_count: usize,
}

pub fn basellm_metadata_cache_path() -> PathBuf {
    proxy_home_dir()
        .join("model-metadata")
        .join("basellm-openai-cache.json")
}

pub async fn sync_basellm_metadata_cache_background() {
    if std::env::var("CODEX_HELPER_BASELLM_METADATA_SYNC")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
    {
        debug!("BaseLLM model metadata background sync disabled by env");
        return;
    }

    tokio::spawn(async {
        match sync_basellm_metadata_cache(false).await {
            Ok(report) => {
                debug!(
                    status = ?report.status,
                    model_count = report.model_count,
                    "BaseLLM model metadata background sync completed"
                );
            }
            Err(err) => {
                warn!("BaseLLM model metadata background sync failed: {err}");
            }
        }
    });
}

pub async fn sync_basellm_metadata_cache(force: bool) -> Result<BasellmMetadataSyncReport> {
    let cache_path = basellm_metadata_cache_path();
    let existing = if force {
        BasellmMetadataCache::default()
    } else {
        read_basellm_metadata_cache_from_path(&cache_path)
            .await
            .unwrap_or_default()
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(BASELLM_SYNC_TIMEOUT_SECS))
        .build()
        .context("build BaseLLM metadata HTTP client")?;

    let mut request = client
        .get(basellm_all_json_url())
        .header(reqwest::header::ACCEPT, "application/json");
    if !force {
        let headers = cache_headers(&existing);
        if !headers.is_empty() {
            request = request.headers(headers);
        }
    }

    let response = request
        .send()
        .await
        .context("request BaseLLM model metadata")?;

    if response.status() == reqwest::StatusCode::NOT_MODIFIED && !force {
        return Ok(BasellmMetadataSyncReport {
            status: BasellmMetadataSyncStatus::NotModified,
            model_count: existing.openai_models.len(),
        });
    }

    if !response.status().is_success() {
        return Ok(BasellmMetadataSyncReport {
            status: BasellmMetadataSyncStatus::Unavailable,
            model_count: existing.openai_models.len(),
        });
    }

    let etag = header_to_string(response.headers(), ETAG);
    let last_modified = header_to_string(response.headers(), LAST_MODIFIED);
    let body = read_limited_text(response, BASELLM_RESPONSE_BODY_LIMIT)
        .await
        .context("read BaseLLM model metadata response")?;
    let mut cache =
        parse_basellm_openai_metadata_json(&body).context("parse BaseLLM OpenAI model metadata")?;
    cache.source_url = basellm_all_json_url().to_string();
    cache.fetched_at_unix = unix_now();
    cache.etag = etag;
    cache.last_modified = last_modified;

    write_basellm_metadata_cache_to_path(&cache_path, &cache)
        .await
        .with_context(|| format!("write BaseLLM model metadata cache {:?}", cache_path))?;

    info!(
        model_count = cache.openai_models.len(),
        cache_path = %cache_path.display(),
        "BaseLLM OpenAI model metadata cache updated"
    );

    Ok(BasellmMetadataSyncReport {
        status: BasellmMetadataSyncStatus::Updated,
        model_count: cache.openai_models.len(),
    })
}

pub fn load_cached_openai_model_metadata(model_id: &str) -> Option<BasellmOpenAiModelMetadata> {
    let cache = load_cached_basellm_metadata_cache()?;
    cache
        .openai_models
        .get(&normalize_model_id(model_id))
        .cloned()
}

pub fn load_cached_basellm_metadata_cache() -> Option<BasellmMetadataCache> {
    read_basellm_metadata_cache_from_path_blocking(&basellm_metadata_cache_path()).ok()
}

pub fn parse_basellm_openai_metadata_json(text: &str) -> Result<BasellmMetadataCache> {
    let root: Value = serde_json::from_str(text).context("invalid BaseLLM metadata JSON")?;
    let Some(openai_models) = root
        .get("openai")
        .and_then(|provider| provider.get("models"))
        .and_then(Value::as_object)
    else {
        return Ok(BasellmMetadataCache::default());
    };

    let mut models = BTreeMap::new();
    for (model_id, model_value) in openai_models {
        let normalized = normalize_model_id(model_id);
        if normalized.is_empty() {
            continue;
        }

        let display_name = model_value
            .get("name")
            .and_then(json_scalar_to_string)
            .or_else(|| {
                model_value
                    .get("display_name")
                    .and_then(json_scalar_to_string)
            })
            .filter(|value| !value.eq_ignore_ascii_case(model_id));
        let description = model_value
            .get("description")
            .and_then(json_scalar_to_string);
        let input_modalities = parse_input_modalities(model_value);
        let context_window = model_value
            .get("limit")
            .and_then(|limit| limit.get("input").or_else(|| limit.get("context")))
            .and_then(json_i64);
        let max_context_window = model_value
            .get("limit")
            .and_then(|limit| limit.get("context").or_else(|| limit.get("input")))
            .and_then(json_i64);

        models.insert(
            normalized.clone(),
            BasellmOpenAiModelMetadata {
                model_id: model_id.to_string(),
                display_name,
                description,
                context_window,
                max_context_window,
                input_modalities,
                reasoning: model_value.get("reasoning").and_then(Value::as_bool),
                tool_call: model_value.get("tool_call").and_then(Value::as_bool),
                structured_output: model_value
                    .get("structured_output")
                    .and_then(Value::as_bool),
                supports_fast_priority: basellm_model_supports_fast_priority(model_value),
            },
        );
    }

    Ok(BasellmMetadataCache {
        source_url: basellm_all_json_url().to_string(),
        openai_models: models,
        ..BasellmMetadataCache::default()
    })
}

async fn read_basellm_metadata_cache_from_path(path: &Path) -> Result<BasellmMetadataCache> {
    let metadata = fs::metadata(path).await?;
    if metadata.len() > BASELLM_CACHE_MAX_BYTES {
        anyhow::bail!("BaseLLM metadata cache is too large");
    }
    let bytes = fs::read(path).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn read_basellm_metadata_cache_from_path_blocking(path: &Path) -> Result<BasellmMetadataCache> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > BASELLM_CACHE_MAX_BYTES {
        anyhow::bail!("BaseLLM metadata cache is too large");
    }
    let bytes = std::fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn write_basellm_metadata_cache_to_path(
    path: &Path,
    cache: &BasellmMetadataCache,
) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(cache)?;
    if bytes.len() as u64 > BASELLM_CACHE_MAX_BYTES {
        anyhow::bail!("BaseLLM metadata cache payload is too large");
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, bytes).await?;
    if fs::try_exists(path).await.unwrap_or(false) {
        let _ = fs::remove_file(path).await;
    }
    fs::rename(&tmp_path, path).await?;
    Ok(())
}

async fn read_limited_text(response: reqwest::Response, limit: usize) -> Result<String> {
    let bytes = response.bytes().await?;
    if bytes.len() > limit {
        anyhow::bail!("response body exceeds {limit} bytes");
    }
    Ok(String::from_utf8(bytes.to_vec())?)
}

fn cache_headers(cache: &BasellmMetadataCache) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(etag) = cache.etag.as_deref().and_then(header_value) {
        headers.insert(IF_NONE_MATCH, etag);
    }
    if let Some(last_modified) = cache.last_modified.as_deref().and_then(header_value) {
        headers.insert(IF_MODIFIED_SINCE, last_modified);
    }
    headers
}

fn header_value(value: &str) -> Option<HeaderValue> {
    HeaderValue::from_str(value).ok()
}

fn header_to_string(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

fn parse_input_modalities(model_value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(items) = model_value
        .get("modalities")
        .and_then(|modalities| modalities.get("input"))
        .and_then(Value::as_array)
    {
        for item in items {
            let Some(modality) = item
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let modality = match modality.to_ascii_lowercase().as_str() {
                "text" => "text",
                "image" => "image",
                // Codex catalog currently only models text/image input. Treat PDFs as attachments
                // rather than adding a modality unknown to older Codex clients.
                "pdf" => continue,
                _ => continue,
            };
            if !out.iter().any(|existing| existing == modality) {
                out.push(modality.to_string());
            }
        }
    }
    out
}

fn basellm_model_supports_fast_priority(model_value: &Value) -> bool {
    model_value
        .get("experimental")
        .and_then(|experimental| experimental.get("modes"))
        .and_then(|modes| modes.get("fast"))
        .and_then(|fast| fast.get("provider"))
        .and_then(|provider| provider.get("body"))
        .and_then(|body| body.get("service_tier"))
        .and_then(Value::as_str)
        .is_some_and(|service_tier| service_tier.eq_ignore_ascii_case("priority"))
}

fn json_scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        }
        _ => None,
    }
}

fn json_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
}

fn normalize_model_id(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fast_priority_and_capabilities_from_basellm_openai_models() {
        let text = r#"
{
  "openai": {
    "models": {
      "gpt-test": {
        "name": "GPT Test",
        "description": "Test model",
        "limit": { "context": 1050000, "input": 922000, "output": 128000 },
        "modalities": { "input": ["text", "image", "pdf"], "output": ["text"] },
        "reasoning": true,
        "tool_call": true,
        "structured_output": true,
        "experimental": {
          "modes": {
            "fast": {
              "cost": { "input": 10, "output": 20 },
              "provider": { "body": { "service_tier": "priority" } }
            }
          }
        }
      }
    }
  }
}
"#;

        let cache = parse_basellm_openai_metadata_json(text).expect("parse");
        let model = cache.openai_models.get("gpt-test").expect("gpt-test");

        assert_eq!(model.display_name.as_deref(), Some("GPT Test"));
        assert_eq!(model.description.as_deref(), Some("Test model"));
        assert_eq!(model.context_window, Some(922_000));
        assert_eq!(model.max_context_window, Some(1_050_000));
        assert_eq!(model.input_modalities, vec!["text", "image"]);
        assert_eq!(model.reasoning, Some(true));
        assert_eq!(model.tool_call, Some(true));
        assert_eq!(model.structured_output, Some(true));
        assert!(model.supports_fast_priority);
    }
}

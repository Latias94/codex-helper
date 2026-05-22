use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::fs;
use tracing::info;

const FAST_SERVICE_TIER_SLUGS: &[&str] = &["gpt-5.5", "gpt-5.4", "gpt-5.4-mini", "gpt-5.3-codex"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelsCacheInvalidation {
    Missing,
    Kept,
    Deleted,
}

pub async fn invalidate_stale_fast_service_tier_cache(
    cache_path: &Path,
) -> Result<ModelsCacheInvalidation> {
    let contents = match fs::read(cache_path).await {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ModelsCacheInvalidation::Missing);
        }
        Err(err) => return Err(err).with_context(|| format!("read {:?}", cache_path)),
    };

    let value = serde_json::from_slice::<Value>(&contents)
        .with_context(|| format!("parse {:?}", cache_path))?;
    let Some(stale_slug) = stale_fast_service_tier_slug(&value) else {
        return Ok(ModelsCacheInvalidation::Kept);
    };

    fs::remove_file(cache_path)
        .await
        .with_context(|| format!("remove stale Codex models cache {:?}", cache_path))?;
    info!(
        cache_path = %cache_path.display(),
        stale_slug,
        "removed Codex models cache because a known Fast model was cached without priority service_tiers"
    );
    Ok(ModelsCacheInvalidation::Deleted)
}

fn stale_fast_service_tier_slug(value: &Value) -> Option<&'static str> {
    let models = value.get("models")?.as_array()?;
    FAST_SERVICE_TIER_SLUGS
        .iter()
        .copied()
        .find(|slug| model_exists_without_fast_service_tier(models, slug))
}

fn model_exists_without_fast_service_tier(models: &[Value], slug: &str) -> bool {
    models
        .iter()
        .find(|model| model.get("slug").and_then(Value::as_str) == Some(slug))
        .is_some_and(|model| !model_has_fast_service_tier(model))
}

fn model_has_fast_service_tier(model: &Value) -> bool {
    model
        .get("service_tiers")
        .and_then(Value::as_array)
        .is_some_and(|service_tiers| {
            service_tiers.iter().any(|tier| {
                tier.get("id").and_then(Value::as_str) == Some("priority")
                    || tier.get("name").and_then(Value::as_str) == Some("Fast")
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cache_path() -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!(
                "codex-helper-models-cache-{}",
                uuid::Uuid::new_v4()
            ))
            .join("models_cache.json")
    }

    async fn write_cache(path: &Path, json: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.expect("create temp dir");
        }
        fs::write(path, json).await.expect("write cache");
    }

    #[tokio::test]
    async fn invalidates_known_fast_model_without_service_tier() {
        let path = temp_cache_path();
        write_cache(
            &path,
            r#"{
                "fetched_at": "2026-05-22T03:11:27Z",
                "client_version": "0.133.0",
                "models": [
                    { "slug": "gpt-5.5", "service_tiers": [] }
                ]
            }"#,
        )
        .await;

        let result = invalidate_stale_fast_service_tier_cache(&path)
            .await
            .expect("invalidate cache");

        assert_eq!(result, ModelsCacheInvalidation::Deleted);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn keeps_known_fast_model_with_priority_service_tier() {
        let path = temp_cache_path();
        write_cache(
            &path,
            r#"{
                "models": [
                    {
                        "slug": "gpt-5.5",
                        "service_tiers": [
                            { "id": "priority", "name": "Fast" }
                        ]
                    }
                ]
            }"#,
        )
        .await;

        let result = invalidate_stale_fast_service_tier_cache(&path)
            .await
            .expect("check cache");

        assert_eq!(result, ModelsCacheInvalidation::Kept);
        assert!(path.exists());
    }

    #[tokio::test]
    async fn keeps_cache_without_known_fast_models() {
        let path = temp_cache_path();
        write_cache(
            &path,
            r#"{
                "models": [
                    { "slug": "gpt-5.2", "service_tiers": [] }
                ]
            }"#,
        )
        .await;

        let result = invalidate_stale_fast_service_tier_cache(&path)
            .await
            .expect("check cache");

        assert_eq!(result, ModelsCacheInvalidation::Kept);
        assert!(path.exists());
    }

    #[tokio::test]
    async fn reports_missing_cache() {
        let path = temp_cache_path();

        let result = invalidate_stale_fast_service_tier_cache(&path)
            .await
            .expect("check missing cache");

        assert_eq!(result, ModelsCacheInvalidation::Missing);
    }
}

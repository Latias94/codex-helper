use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::basellm_catalog::{
    BasellmCatalogAttemptState, BasellmCatalogLkg, BasellmCatalogSyncOptions,
    BasellmCatalogSyncReport, BasellmSyncOutcome, basellm_catalog_snapshot,
    load_basellm_catalog_attempt_state, sync_basellm_catalog,
};
use crate::config::proxy_home_dir;
use crate::pricing::{
    EffectivePricingCatalogSnapshot, basellm_all_json_url, effective_pricing_catalog_snapshot,
    refresh_effective_pricing_catalog,
};

const BASELLM_CACHE_MAX_BYTES: u64 = 2 * 1024 * 1024;
const DEFAULT_BASELLM_CATALOG_SYNC_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const MAX_BASELLM_PERSISTED_RETRY_WAIT: Duration = Duration::from_secs(5 * 60);

type CatalogSyncFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
type CatalogSyncExecutor = dyn Fn() -> CatalogSyncFuture + Send + Sync + 'static;
type RetryNotBeforeLoader = dyn Fn() -> Option<i64> + Send + Sync + 'static;
type UnixClock = dyn Fn() -> i64 + Send + Sync + 'static;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BasellmMetadataCache {
    pub source_url: String,
    pub fetched_at_unix: i64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_hash: Option<String>,
    pub content_generation: Option<u64>,
    pub openai_models: BTreeMap<String, BasellmOpenAiModelMetadata>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
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

pub fn basellm_metadata_cache_path() -> PathBuf {
    proxy_home_dir()
        .join("model-metadata")
        .join("basellm-openai-cache.json")
}

#[derive(Default)]
struct LegacyMetadataSnapshotCache {
    snapshot: OnceLock<Option<Arc<BasellmMetadataCache>>>,
}

impl LegacyMetadataSnapshotCache {
    fn get_or_load(&self, path: &Path) -> Option<Arc<BasellmMetadataCache>> {
        self.snapshot
            .get_or_init(|| {
                read_basellm_metadata_cache_from_path_blocking(path)
                    .ok()
                    .map(Arc::new)
            })
            .clone()
    }
}

#[derive(Clone)]
pub(crate) enum BasellmOpenAiMetadataSnapshot {
    Catalog(Arc<BasellmCatalogLkg>),
    Legacy(Arc<BasellmMetadataCache>),
}

impl BasellmOpenAiMetadataSnapshot {
    pub(crate) fn model(&self, model_id: &str) -> Option<&BasellmOpenAiModelMetadata> {
        match self {
            Self::Catalog(snapshot) => snapshot
                .model("openai", model_id)
                .map(|model| &model.metadata),
            Self::Legacy(snapshot) => snapshot.openai_models.get(&normalize_model_id(model_id)),
        }
    }
}

static LEGACY_METADATA_SNAPSHOT: LegacyMetadataSnapshotCache = LegacyMetadataSnapshotCache {
    snapshot: OnceLock::new(),
};
static PUBLISHED_CATALOG_SNAPSHOT: OnceLock<RwLock<Option<Arc<BasellmCatalogLkg>>>> =
    OnceLock::new();

fn published_catalog_snapshot_slot() -> &'static RwLock<Option<Arc<BasellmCatalogLkg>>> {
    PUBLISHED_CATALOG_SNAPSHOT.get_or_init(|| RwLock::new(basellm_catalog_snapshot()))
}

fn legacy_metadata_snapshot() -> Option<Arc<BasellmMetadataCache>> {
    LEGACY_METADATA_SNAPSHOT.get_or_load(&basellm_metadata_cache_path())
}

pub(crate) fn basellm_openai_metadata_snapshot() -> Option<BasellmOpenAiMetadataSnapshot> {
    let effective = effective_pricing_catalog_snapshot();
    let Some(expected_revision) = effective.remote_content_revision.as_deref() else {
        return legacy_metadata_snapshot().map(BasellmOpenAiMetadataSnapshot::Legacy);
    };
    if let Some(snapshot) = basellm_catalog_snapshot()
        && snapshot.content_hash == expected_revision
    {
        return Some(BasellmOpenAiMetadataSnapshot::Catalog(snapshot));
    }
    let published = published_catalog_snapshot_slot()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    published
        .filter(|snapshot| snapshot.content_hash == expected_revision)
        .map(BasellmOpenAiMetadataSnapshot::Catalog)
}

pub(crate) fn initialize_basellm_runtime_state() -> Arc<EffectivePricingCatalogSnapshot> {
    let catalog = basellm_catalog_snapshot();
    if catalog.is_none() {
        let _ = legacy_metadata_snapshot();
    }
    let effective = refresh_effective_pricing_catalog();
    publish_catalog_for_effective_snapshot(&effective, catalog);
    effective
}

fn publish_catalog_for_effective_snapshot(
    effective: &EffectivePricingCatalogSnapshot,
    candidate: Option<Arc<BasellmCatalogLkg>>,
) {
    let Some(candidate) = candidate.filter(|candidate| {
        effective.remote_content_revision.as_deref() == Some(candidate.content_hash.as_str())
    }) else {
        return;
    };
    *published_catalog_snapshot_slot()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(candidate);
}

fn basellm_catalog_sync_enabled() -> bool {
    !std::env::var("CODEX_HELPER_BASELLM_METADATA_SYNC")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
}

pub(crate) struct BasellmCatalogSyncTask {
    interval: Duration,
    enabled: bool,
    sync: Box<CatalogSyncExecutor>,
    retry_not_before: Box<RetryNotBeforeLoader>,
    unix_now: Box<UnixClock>,
}

impl Default for BasellmCatalogSyncTask {
    fn default() -> Self {
        Self::new_with_schedule(
            DEFAULT_BASELLM_CATALOG_SYNC_INTERVAL,
            basellm_catalog_sync_enabled(),
            || async {
                let report = sync_basellm_catalog(BasellmCatalogSyncOptions::default()).await;
                apply_catalog_sync_report(&report);
                let model_count = report
                    .snapshot
                    .as_deref()
                    .and_then(|snapshot| snapshot.catalog.providers.get("openai"))
                    .map(|provider| provider.models.len())
                    .unwrap_or(0);
                match report.outcome {
                    BasellmSyncOutcome::Updated
                    | BasellmSyncOutcome::NotModified
                    | BasellmSyncOutcome::StaleResponse => {
                        debug!(
                            outcome = ?report.outcome,
                            model_count,
                            "BaseLLM catalog background sync completed"
                        );
                    }
                    BasellmSyncOutcome::Quarantined
                    | BasellmSyncOutcome::Unavailable
                    | BasellmSyncOutcome::ReadOnly => {
                        warn!(
                            outcome = ?report.outcome,
                            model_count,
                            "BaseLLM catalog background sync retained existing state"
                        );
                    }
                }
            },
            persisted_basellm_retry_after_unix,
            unix_now,
        )
    }
}

impl BasellmCatalogSyncTask {
    #[cfg(test)]
    pub(crate) fn new<F, Fut>(interval: Duration, sync: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Self::new_with_schedule(interval, true, sync, || None, unix_now)
    }

    fn new_with_schedule<F, Fut, R, N>(
        interval: Duration,
        enabled: bool,
        sync: F,
        retry_not_before: R,
        unix_now: N,
    ) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
        R: Fn() -> Option<i64> + Send + Sync + 'static,
        N: Fn() -> i64 + Send + Sync + 'static,
    {
        Self {
            interval,
            enabled,
            sync: Box::new(move || Box::pin(sync())),
            retry_not_before: Box::new(retry_not_before),
            unix_now: Box::new(unix_now),
        }
    }

    pub(crate) fn spawn(self, shutdown_rx: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(self.run(shutdown_rx))
    }

    async fn run(self, mut shutdown_rx: watch::Receiver<bool>) {
        if !self.enabled {
            debug!("BaseLLM catalog background sync disabled by env");
            wait_for_shutdown(&mut shutdown_rx).await;
            return;
        }

        loop {
            if *shutdown_rx.borrow() {
                return;
            }

            if let Some(delay) = retry_gate_delay((self.retry_not_before)(), (self.unix_now)()) {
                debug!(
                    retry_in_seconds = delay.as_secs(),
                    "BaseLLM catalog background sync is honoring persisted Retry-After"
                );
                tokio::select! {
                    biased;
                    _ = wait_for_shutdown(&mut shutdown_rx) => return,
                    _ = tokio::time::sleep(delay) => continue,
                }
            }

            // Detaching this child on supervisor cancellation keeps the complete sync future,
            // including its commit lock, alive until any blocking atomic replace has finished.
            let mut sync = tokio::spawn((self.sync)());
            tokio::select! {
                biased;
                _ = wait_for_shutdown(&mut shutdown_rx) => {
                    log_catalog_sync_join_result(sync.await);
                    return;
                },
                result = &mut sync => log_catalog_sync_join_result(result),
            }

            tokio::select! {
                biased;
                _ = wait_for_shutdown(&mut shutdown_rx) => return,
                _ = tokio::time::sleep(self.interval) => {}
            }
        }
    }
}

fn log_catalog_sync_join_result(result: Result<(), tokio::task::JoinError>) {
    if let Err(error) = result {
        warn!(%error, "BaseLLM catalog sync task failed to join");
    }
}

fn persisted_basellm_retry_after_unix() -> Option<i64> {
    let attempt = load_basellm_catalog_attempt_state()?;
    retry_after_for_default_source(&attempt)
}

fn retry_after_for_default_source(attempt: &BasellmCatalogAttemptState) -> Option<i64> {
    (attempt.source_url == basellm_all_json_url())
        .then_some(attempt.retry_after_unix)
        .flatten()
}

fn retry_gate_delay(retry_after_unix: Option<i64>, now_unix: i64) -> Option<Duration> {
    let remaining_seconds = retry_after_unix?.saturating_sub(now_unix);
    if remaining_seconds <= 0 {
        return None;
    }
    Some(
        Duration::from_secs(u64::try_from(remaining_seconds).unwrap_or(u64::MAX))
            .min(MAX_BASELLM_PERSISTED_RETRY_WAIT),
    )
}

async fn wait_for_shutdown(shutdown_rx: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown_rx.borrow() || shutdown_rx.changed().await.is_err() {
            return;
        }
    }
}

fn apply_catalog_sync_report(report: &BasellmCatalogSyncReport) {
    if !should_refresh_effective_pricing(report.outcome) {
        return;
    }
    let effective = refresh_effective_pricing_catalog();
    publish_catalog_for_effective_snapshot(&effective, report.snapshot.clone());
}

fn should_refresh_effective_pricing(outcome: BasellmSyncOutcome) -> bool {
    matches!(
        outcome,
        BasellmSyncOutcome::Updated
            | BasellmSyncOutcome::NotModified
            | BasellmSyncOutcome::StaleResponse
    )
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

        models.insert(
            normalized,
            parse_basellm_model_metadata(model_id, model_value),
        );
    }

    Ok(BasellmMetadataCache {
        source_url: basellm_all_json_url().to_string(),
        openai_models: models,
        ..BasellmMetadataCache::default()
    })
}

fn read_basellm_metadata_cache_from_path_blocking(path: &Path) -> Result<BasellmMetadataCache> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > BASELLM_CACHE_MAX_BYTES {
        anyhow::bail!("BaseLLM metadata cache is too large");
    }
    let bytes = std::fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub(crate) fn parse_basellm_model_metadata(
    model_id: &str,
    model_value: &Value,
) -> BasellmOpenAiModelMetadata {
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
    let context_window = model_value
        .get("limit")
        .and_then(|limit| limit.get("input").or_else(|| limit.get("context")))
        .and_then(json_i64);
    let max_context_window = model_value
        .get("limit")
        .and_then(|limit| limit.get("context").or_else(|| limit.get("input")))
        .and_then(json_i64);
    BasellmOpenAiModelMetadata {
        model_id: model_id.to_string(),
        display_name,
        description,
        context_window,
        max_context_window,
        input_modalities: parse_input_modalities(model_value),
        reasoning: model_value.get("reasoning").and_then(Value::as_bool),
        tool_call: model_value.get("tool_call").and_then(Value::as_bool),
        structured_output: model_value
            .get("structured_output")
            .and_then(Value::as_bool),
        supports_fast_priority: basellm_model_supports_fast_priority(model_value),
    }
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
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

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

    #[test]
    fn recovered_catalog_outcomes_refresh_the_effective_snapshot() {
        assert!(should_refresh_effective_pricing(
            BasellmSyncOutcome::Updated
        ));
        assert!(should_refresh_effective_pricing(
            BasellmSyncOutcome::NotModified
        ));
        assert!(should_refresh_effective_pricing(
            BasellmSyncOutcome::StaleResponse
        ));
        assert!(!should_refresh_effective_pricing(
            BasellmSyncOutcome::Quarantined
        ));
        assert!(!should_refresh_effective_pricing(
            BasellmSyncOutcome::Unavailable
        ));
        assert!(!should_refresh_effective_pricing(
            BasellmSyncOutcome::ReadOnly
        ));
    }

    #[test]
    fn legacy_metadata_is_loaded_once_into_an_immutable_snapshot() {
        let temp_dir = std::env::temp_dir().join(format!(
            "codex-helper-basellm-legacy-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&temp_dir).expect("create temp dir");
        let path = temp_dir.join("basellm-openai-cache.json");
        let expected = BasellmMetadataCache {
            openai_models: BTreeMap::from([(
                "gpt-legacy".to_string(),
                BasellmOpenAiModelMetadata {
                    model_id: "gpt-legacy".to_string(),
                    supports_fast_priority: true,
                    ..BasellmOpenAiModelMetadata::default()
                },
            )]),
            ..BasellmMetadataCache::default()
        };
        std::fs::write(
            &path,
            serde_json::to_vec(&expected).expect("serialize legacy cache"),
        )
        .expect("write legacy cache");
        let cache = LegacyMetadataSnapshotCache::default();

        let first = cache.get_or_load(&path).expect("first snapshot");
        std::fs::remove_file(&path).expect("remove legacy cache after startup");
        let second = cache.get_or_load(&path).expect("cached snapshot");

        assert!(Arc::ptr_eq(&first, &second));
        assert!(second.openai_models.contains_key("gpt-legacy"));
        std::fs::remove_dir(&temp_dir).expect("remove empty temp dir");
    }

    async fn wait_for_calls(calls: &AtomicUsize, expected: usize) {
        for _ in 0..32 {
            if calls.load(Ordering::SeqCst) == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), expected);
    }

    #[tokio::test(start_paused = true)]
    async fn catalog_sync_task_runs_immediately_and_on_its_interval() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_sync = calls.clone();
        let task = BasellmCatalogSyncTask::new(Duration::from_secs(60 * 60), move || {
            let calls = calls_for_sync.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
            }
        });
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let handle = task.spawn(shutdown_rx);

        wait_for_calls(&calls, 1).await;
        tokio::time::advance(Duration::from_secs(60 * 60 - 1)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(1)).await;
        wait_for_calls(&calls, 2).await;

        shutdown_tx.send(true).expect("send shutdown");
        handle.await.expect("join catalog sync task");
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_joins_an_in_flight_blocking_catalog_commit() {
        let entered = Arc::new(AtomicUsize::new(0));
        let committed = Arc::new(AtomicUsize::new(0));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let entered_for_sync = entered.clone();
        let committed_for_sync = committed.clone();
        let release_for_sync = release.clone();
        let task = BasellmCatalogSyncTask::new(Duration::from_secs(60 * 60), move || {
            let entered = entered_for_sync.clone();
            let committed = committed_for_sync.clone();
            let release = release_for_sync.clone();
            async move {
                tokio::task::spawn_blocking(move || {
                    entered.fetch_add(1, Ordering::SeqCst);
                    let (released, wake) = release.as_ref();
                    let mut released = released
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    while !*released {
                        released = wake
                            .wait(released)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                    committed.fetch_add(1, Ordering::SeqCst);
                })
                .await
                .expect("join blocking catalog commit");
            }
        });
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let handle = task.spawn(shutdown_rx);

        wait_for_calls(&entered, 1).await;
        shutdown_tx.send(true).expect("send shutdown");
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        let returned_before_commit = handle.is_finished();

        let (released, wake) = release.as_ref();
        *released
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        wake.notify_all();
        handle.await.expect("join catalog sync task");

        assert!(!returned_before_commit);
        assert_eq!(committed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn persisted_retry_after_is_honored_after_task_restart() {
        let temp_dir = std::env::temp_dir().join(format!(
            "codex-helper-basellm-retry-after-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&temp_dir).expect("create temp dir");
        let attempt_path = temp_dir.join("attempt.json");
        let attempt = BasellmCatalogAttemptState {
            schema_version: crate::basellm_catalog::BASELLM_CATALOG_SCHEMA_VERSION,
            check_generation: 1,
            source_url: basellm_all_json_url().to_string(),
            last_checked_at_unix: 10_000,
            outcome: BasellmSyncOutcome::Unavailable,
            last_error_category: Some(
                crate::basellm_catalog::BasellmSyncErrorCategory::RateLimited,
            ),
            content_hash: None,
            content_generation: None,
            quarantined_candidate_hash: None,
            read_only_schema_version: None,
            retry_after_unix: Some(10_120),
        };
        std::fs::write(
            &attempt_path,
            serde_json::to_vec(&attempt).expect("serialize attempt state"),
        )
        .expect("persist attempt state");

        let now = Arc::new(AtomicI64::new(10_000));
        let reads = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let build_task = |reads: Arc<AtomicUsize>, calls: Arc<AtomicUsize>| {
            let attempt_path = attempt_path.clone();
            let now_for_clock = now.clone();
            BasellmCatalogSyncTask::new_with_schedule(
                Duration::from_secs(60 * 60),
                true,
                move || {
                    let calls = calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                    }
                },
                move || {
                    reads.fetch_add(1, Ordering::SeqCst);
                    let attempt: BasellmCatalogAttemptState =
                        serde_json::from_slice(&std::fs::read(&attempt_path).ok()?).ok()?;
                    retry_after_for_default_source(&attempt)
                },
                move || now_for_clock.load(Ordering::SeqCst),
            )
        };

        let first = build_task(reads.clone(), calls.clone());
        let (first_shutdown_tx, first_shutdown_rx) = tokio::sync::watch::channel(false);
        let first_handle = first.spawn(first_shutdown_rx);
        wait_for_calls(&reads, 1).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        first_shutdown_tx.send(true).expect("send first shutdown");
        first_handle.await.expect("join first catalog sync task");

        let restarted = build_task(reads.clone(), calls.clone());
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let handle = restarted.spawn(shutdown_rx);
        wait_for_calls(&reads, 2).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        tokio::time::advance(Duration::from_secs(119)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        now.store(10_120, Ordering::SeqCst);
        tokio::time::advance(Duration::from_secs(1)).await;
        wait_for_calls(&calls, 1).await;

        shutdown_tx.send(true).expect("send shutdown");
        handle.await.expect("join restarted catalog sync task");
        std::fs::remove_file(&attempt_path).expect("remove attempt state");
        std::fs::remove_dir(&temp_dir).expect("remove empty temp dir");
    }
}

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::time::Instant;

use crate::control_plane_client::LocalOperatorClient;
use crate::dashboard_core::{
    LocalOperatorSessionMetadataResponse, OperatorLocalSessionMetadata, OperatorReadModel,
};

const NEGATIVE_LOOKUP_INITIAL: Duration = Duration::from_secs(30);
const NEGATIVE_LOOKUP_MAX: Duration = Duration::from_secs(30 * 60);
const ATTACHED_METADATA_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const METADATA_RETRY_INITIAL: Duration = Duration::from_secs(2);
const METADATA_RETRY_MAX: Duration = Duration::from_secs(30);
const SESSION_METADATA_READ_CONCURRENCY: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LocalSessionEnrichmentIssue {
    MetadataUnavailable,
    ServiceMismatch,
}

#[derive(Debug, Clone, Default)]
pub(super) struct AttachedLocalSessionEnrichmentResult {
    pub(super) sessions: HashMap<String, OperatorLocalSessionMetadata>,
    pub(super) issue: Option<LocalSessionEnrichmentIssue>,
}

#[derive(Default)]
pub(super) struct LocalSessionEnrichmentCache {
    source: HashMap<String, OperatorLocalSessionMetadata>,
    resolved: HashMap<String, OperatorLocalSessionMetadata>,
    locator_by_raw_session_id: HashMap<String, TranscriptLocatorCacheEntry>,
}

struct HostSessionLocation {
    transcript_path: String,
    cwd: Option<String>,
}

struct NegativeTranscriptLookup {
    attempts: u32,
    retry_not_before: Instant,
}

enum TranscriptLocatorCacheEntry {
    Located(HostSessionLocation),
    Missing(NegativeTranscriptLookup),
}

impl fmt::Debug for LocalSessionEnrichmentCache {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalSessionEnrichmentCache")
            .field("source_session_count", &self.source.len())
            .field("resolved_session_count", &self.resolved.len())
            .field(
                "located_session_count",
                &self
                    .locator_by_raw_session_id
                    .values()
                    .filter(|entry| matches!(entry, TranscriptLocatorCacheEntry::Located(_)))
                    .count(),
            )
            .field(
                "missing_session_count",
                &self
                    .locator_by_raw_session_id
                    .values()
                    .filter(|entry| matches!(entry, TranscriptLocatorCacheEntry::Missing(_)))
                    .count(),
            )
            .finish()
    }
}

impl LocalSessionEnrichmentCache {
    pub(super) async fn resolve(
        &mut self,
        source: HashMap<String, OperatorLocalSessionMetadata>,
    ) -> HashMap<String, OperatorLocalSessionMetadata> {
        self.resolve_with_locator(source, locate_host_sessions)
            .await
    }

    async fn resolve_with_locator<F, Fut>(
        &mut self,
        source: HashMap<String, OperatorLocalSessionMetadata>,
        locate: F,
    ) -> HashMap<String, OperatorLocalSessionMetadata>
    where
        F: FnOnce(Vec<String>) -> Fut,
        Fut: Future<Output = Result<HashMap<String, HostSessionLocation>, ()>>,
    {
        self.source = source;
        let active_raw_session_ids = self
            .source
            .values()
            .map(|session| session.raw_session_id.clone())
            .filter(|raw_session_id| !raw_session_id.trim().is_empty())
            .collect::<HashSet<_>>();
        self.locator_by_raw_session_id
            .retain(|raw_session_id, _| active_raw_session_ids.contains(raw_session_id));

        for session in self.source.values() {
            let Some(transcript_path) = session.host_local_transcript_path.as_ref() else {
                continue;
            };
            let raw_session_id = session.raw_session_id.as_str();
            if raw_session_id.trim().is_empty() {
                continue;
            }
            match self.locator_by_raw_session_id.get_mut(raw_session_id) {
                Some(TranscriptLocatorCacheEntry::Located(location)) => {
                    location.transcript_path.clone_from(transcript_path);
                    if session.cwd.is_some() {
                        location.cwd.clone_from(&session.cwd);
                    }
                }
                Some(entry) => {
                    *entry = TranscriptLocatorCacheEntry::Located(HostSessionLocation {
                        transcript_path: transcript_path.clone(),
                        cwd: session.cwd.clone(),
                    });
                }
                None => {
                    self.locator_by_raw_session_id.insert(
                        raw_session_id.to_string(),
                        TranscriptLocatorCacheEntry::Located(HostSessionLocation {
                            transcript_path: transcript_path.clone(),
                            cwd: session.cwd.clone(),
                        }),
                    );
                }
            }
        }

        let now = Instant::now();
        let mut lookup_ids = active_raw_session_ids
            .into_iter()
            .filter(|raw_session_id| {
                self.locator_by_raw_session_id
                    .get(raw_session_id)
                    .is_none_or(|entry| match entry {
                        TranscriptLocatorCacheEntry::Located(_) => false,
                        TranscriptLocatorCacheEntry::Missing(missing) => {
                            now >= missing.retry_not_before
                        }
                    })
            })
            .collect::<Vec<_>>();
        lookup_ids.sort();

        if !lookup_ids.is_empty() {
            let mut located = locate(lookup_ids.clone()).await.ok();
            let completed_at = Instant::now();
            for raw_session_id in lookup_ids {
                let location = located
                    .as_mut()
                    .and_then(|locations| locations.remove(&raw_session_id));
                if let Some(location) = location {
                    self.locator_by_raw_session_id.insert(
                        raw_session_id,
                        TranscriptLocatorCacheEntry::Located(location),
                    );
                } else {
                    self.record_negative_lookup(raw_session_id, completed_at);
                }
            }
        }

        self.resolved = self.source.clone();
        for session in self.resolved.values_mut() {
            let Some(TranscriptLocatorCacheEntry::Located(location)) = self
                .locator_by_raw_session_id
                .get(session.raw_session_id.as_str())
            else {
                continue;
            };
            if session.host_local_transcript_path.is_none() {
                session.host_local_transcript_path = Some(location.transcript_path.clone());
            }
            if session.cwd.is_none() {
                session.cwd.clone_from(&location.cwd);
            }
        }
        self.resolved.clone()
    }

    fn record_negative_lookup(&mut self, raw_session_id: String, now: Instant) {
        let attempts = self
            .locator_by_raw_session_id
            .get(&raw_session_id)
            .and_then(|entry| match entry {
                TranscriptLocatorCacheEntry::Located(_) => None,
                TranscriptLocatorCacheEntry::Missing(missing) => {
                    Some(missing.attempts.saturating_add(1))
                }
            })
            .unwrap_or(1);
        let shift = attempts.saturating_sub(1).min(6);
        let delay = NEGATIVE_LOOKUP_INITIAL
            .saturating_mul(1_u32 << shift)
            .min(NEGATIVE_LOOKUP_MAX);
        self.locator_by_raw_session_id.insert(
            raw_session_id,
            TranscriptLocatorCacheEntry::Missing(NegativeTranscriptLookup {
                attempts,
                retry_not_before: now + delay,
            }),
        );
    }

    pub(super) fn current(&self) -> HashMap<String, OperatorLocalSessionMetadata> {
        self.resolved.clone()
    }
}

#[derive(Default)]
pub(super) struct AttachedLocalSessionEnrichment {
    last_session_keys: Vec<String>,
    last_attempt_session_keys: Vec<String>,
    last_successful_fetch_at: Option<Instant>,
    retry_not_before: Option<Instant>,
    consecutive_failures: u32,
    last_issue: Option<LocalSessionEnrichmentIssue>,
    source: HashMap<String, OperatorLocalSessionMetadata>,
    local: LocalSessionEnrichmentCache,
}

impl fmt::Debug for AttachedLocalSessionEnrichment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AttachedLocalSessionEnrichment")
            .field("last_session_key_count", &self.last_session_keys.len())
            .field(
                "last_attempt_session_key_count",
                &self.last_attempt_session_keys.len(),
            )
            .field("last_successful_fetch_at", &self.last_successful_fetch_at)
            .field("retry_not_before", &self.retry_not_before)
            .field("consecutive_failures", &self.consecutive_failures)
            .field("last_issue", &self.last_issue)
            .field("source_session_count", &self.source.len())
            .field("local", &self.local)
            .finish()
    }
}

impl AttachedLocalSessionEnrichment {
    pub(super) async fn resolve(
        &mut self,
        client: &LocalOperatorClient,
        model: &OperatorReadModel,
    ) -> AttachedLocalSessionEnrichmentResult {
        let session_keys = model
            .data
            .as_ref()
            .map(|data| {
                data.summary
                    .sessions
                    .iter()
                    .map(|session| session.session_key.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.resolve_with(
            model.service_name.as_str(),
            session_keys,
            |session_keys| async move {
                client
                    .read_operator_session_metadata(session_keys)
                    .await
                    .map_err(|_| ())
            },
        )
        .await
    }

    pub(super) fn current(&self) -> AttachedLocalSessionEnrichmentResult {
        AttachedLocalSessionEnrichmentResult {
            sessions: self.local.current(),
            issue: self.last_issue,
        }
    }

    async fn resolve_with<F, Fut>(
        &mut self,
        service_name: &str,
        mut session_keys: Vec<String>,
        fetch: F,
    ) -> AttachedLocalSessionEnrichmentResult
    where
        F: FnOnce(Vec<String>) -> Fut,
        Fut: Future<Output = Result<LocalOperatorSessionMetadataResponse, ()>>,
    {
        session_keys.sort();
        session_keys.dedup();

        let now = Instant::now();
        if session_keys.is_empty() {
            self.last_session_keys.clear();
            self.last_attempt_session_keys.clear();
            self.last_successful_fetch_at = Some(now);
            self.retry_not_before = None;
            self.consecutive_failures = 0;
            self.last_issue = None;
            self.source.clear();
            return self.resolve_current().await;
        }

        let should_fetch = self.last_session_keys != session_keys
            || self
                .last_successful_fetch_at
                .is_none_or(|at| now.duration_since(at) >= ATTACHED_METADATA_REFRESH_INTERVAL);
        let retry_ready = self.last_attempt_session_keys != session_keys
            || self.retry_not_before.is_none_or(|deadline| now >= deadline);
        if should_fetch && retry_ready {
            self.last_attempt_session_keys = session_keys.clone();
            match fetch(session_keys.clone()).await {
                Ok(response) if response.service_name == service_name => {
                    self.last_session_keys = session_keys;
                    self.last_successful_fetch_at = Some(Instant::now());
                    self.retry_not_before = None;
                    self.consecutive_failures = 0;
                    self.last_issue = None;
                    self.source = response.sessions;
                }
                Ok(_) => self.record_failure(LocalSessionEnrichmentIssue::ServiceMismatch),
                Err(()) => self.record_failure(LocalSessionEnrichmentIssue::MetadataUnavailable),
            }
        }

        self.resolve_current().await
    }

    fn record_failure(&mut self, issue: LocalSessionEnrichmentIssue) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let shift = self.consecutive_failures.saturating_sub(1).min(4);
        let multiplier = 1_u32 << shift;
        let delay = METADATA_RETRY_INITIAL
            .saturating_mul(multiplier)
            .min(METADATA_RETRY_MAX);
        self.retry_not_before = Some(Instant::now() + delay);
        self.last_issue = Some(issue);
    }

    async fn resolve_current(&mut self) -> AttachedLocalSessionEnrichmentResult {
        AttachedLocalSessionEnrichmentResult {
            sessions: self.local.resolve(self.source.clone()).await,
            issue: self.last_issue,
        }
    }
}

async fn locate_host_sessions(
    raw_session_ids: Vec<String>,
) -> Result<HashMap<String, HostSessionLocation>, ()> {
    let found = crate::sessions::find_codex_session_files_by_ids(&raw_session_ids)
        .await
        .map_err(|_| ())?;

    let resolved = futures_util::stream::iter(found)
        .map(|(raw_session_id, path)| async move {
            let cwd = crate::sessions::read_codex_session_meta(&path)
                .await
                .ok()
                .flatten()
                .and_then(|metadata| metadata.cwd);
            (
                raw_session_id,
                HostSessionLocation {
                    transcript_path: path.to_string_lossy().to_string(),
                    cwd,
                },
            )
        })
        .buffer_unordered(SESSION_METADATA_READ_CONCURRENCY)
        .collect::<HashMap<_, _>>()
        .await;
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, MutexGuard};
    use std::time::{SystemTime, UNIX_EPOCH};

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use super::*;
    use crate::dashboard_core::{ApiV1OperatorSummary, OperatorReadData, OperatorSessionSummary};
    use crate::tui::Language;
    use crate::tui::model::{Palette, snapshot_from_operator_data};
    use crate::tui::state::UiState;
    use crate::tui::types::Page;

    static CODEX_HOME_LOCK: Mutex<()> = Mutex::new(());
    const RAW_SESSION_ID: &str = "11111111-2222-4333-8444-555555555555";
    const OPAQUE_SESSION_KEY: &str =
        "session:sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const SECRET_CANARY: &str = "local-session-secret-canary-do-not-log";

    struct TestCodexHome {
        root: PathBuf,
        previous: Option<OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl TestCodexHome {
        fn new() -> Self {
            let lock = CODEX_HOME_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock after Unix epoch")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "codex-helper-tui-session-enrichment-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).expect("create temporary CODEX_HOME");
            let previous = std::env::var_os("CODEX_HOME");
            // SAFETY: all CODEX_HOME mutations in this module are serialized and restored on drop.
            unsafe { std::env::set_var("CODEX_HOME", &root) };
            Self {
                root,
                previous,
                _lock: lock,
            }
        }

        fn day_dir(&self) -> PathBuf {
            self.root
                .join("sessions")
                .join("2026")
                .join("07")
                .join("20")
        }
    }

    impl Drop for TestCodexHome {
        fn drop(&mut self) {
            // SAFETY: the guard still exclusively owns this module's CODEX_HOME mutation window.
            unsafe {
                match &self.previous {
                    Some(previous) => std::env::set_var("CODEX_HOME", previous),
                    None => std::env::remove_var("CODEX_HOME"),
                }
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn test_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test runtime")
    }

    fn operator_session_metadata(
        raw_session_id: &str,
        client_name: Option<&str>,
    ) -> OperatorLocalSessionMetadata {
        OperatorLocalSessionMetadata {
            raw_session_id: raw_session_id.to_string(),
            cwd: None,
            last_client_name: client_name.map(ToOwned::to_owned),
            last_client_addr: Some("127.0.0.1:43123".to_string()),
            host_local_transcript_path: None,
        }
    }

    fn host_session_location(path: &str, cwd: Option<&str>) -> HostSessionLocation {
        HostSessionLocation {
            transcript_path: path.to_string(),
            cwd: cwd.map(ToOwned::to_owned),
        }
    }

    fn operator_data_with_session(session_key: &str) -> OperatorReadData {
        let session: OperatorSessionSummary = serde_json::from_value(serde_json::json!({
            "session_key": session_key,
            "active_count": 1,
            "last_status": 200,
            "last_model": "gpt-5.6"
        }))
        .expect("operator session fixture");
        OperatorReadData {
            summary: ApiV1OperatorSummary {
                api_version: 1,
                service_name: "codex".to_string(),
                runtime: Default::default(),
                counts: Default::default(),
                retry: Default::default(),
                credential_readiness: None,
                sessions: vec![session],
                profiles: Vec::new(),
                providers: Vec::new(),
            },
            routing: None,
            active_requests: Vec::new(),
            recent_requests: Vec::new(),
            usage_summaries: Vec::new(),
            usage_day: Default::default(),
            usage_rollup: Default::default(),
            quota_analytics: Default::default(),
            stats_5m: Default::default(),
            stats_1h: Default::default(),
            pricing_catalog: Default::default(),
            service_status: None,
            provider_balances: Vec::new(),
        }
    }

    fn buffer_text(buffer: &Buffer) -> String {
        let mut out = String::new();
        for y in buffer.area.y..buffer.area.y.saturating_add(buffer.area.height) {
            for x in buffer.area.x..buffer.area.x.saturating_add(buffer.area.width) {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn write_real_session_fixture(home: &TestCodexHome, cwd: &Path) -> PathBuf {
        let day_dir = home.day_dir();
        std::fs::create_dir_all(&day_dir).expect("create Codex session day directory");
        let path = day_dir.join(format!(
            "rollout-2026-07-20T12-00-00-{RAW_SESSION_ID}.jsonl"
        ));
        let lines = [
            serde_json::json!({
                "timestamp": "2026-07-20T12:00:00.000Z",
                "type": "session_meta",
                "payload": {
                    "id": RAW_SESSION_ID,
                    "cwd": cwd.to_string_lossy(),
                    "timestamp": "2026-07-20T12:00:00.000Z",
                    "originator": "codex_cli_rs"
                }
            })
            .to_string(),
            serde_json::json!({
                "timestamp": "2026-07-20T12:00:01.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "user_message",
                    "message": SECRET_CANARY,
                    "request_id": "request-correlated-with-operator-session"
                }
            })
            .to_string(),
            serde_json::json!({
                "timestamp": "2026-07-20T12:00:02.000Z",
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "ok"}]
                }
            })
            .to_string(),
        ]
        .join("\n");
        std::fs::write(&path, lines).expect("write Codex session fixture");
        path
    }

    fn write_oversized_malformed_session_fixture(home: &TestCodexHome) {
        let day_dir = home.day_dir();
        std::fs::create_dir_all(&day_dir).expect("create malformed session day directory");
        let path =
            day_dir.join("rollout-2026-07-20T13-00-00-aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee.jsonl");
        let mut lines = Vec::with_capacity(514);
        lines.push(format!(
            "{{malformed:{SECRET_CANARY}:{}",
            "x".repeat(64 * 1024)
        ));
        lines.extend((1..513).map(|index| format!("not-json-{index}")));
        lines.push(
            serde_json::json!({
                "timestamp": "2026-07-20T13:00:00.000Z",
                "type": "session_meta",
                "payload": {
                    "id": "metadata-after-bounded-header-scan",
                    "cwd": "/must/not/be/read"
                }
            })
            .to_string(),
        );
        std::fs::write(path, lines.join("\n")).expect("write malformed Codex session fixture");
    }

    #[test]
    fn empty_source_resolves_without_scanning() {
        let _home = TestCodexHome::new();
        let mut cache = LocalSessionEnrichmentCache::default();

        assert!(
            test_runtime()
                .block_on(cache.resolve(HashMap::new()))
                .is_empty()
        );
        assert!(cache.current().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn display_metadata_changes_reuse_positive_locator_without_scanning() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut cache = LocalSessionEnrichmentCache::default();
        let first_calls = Arc::clone(&calls);
        let first = cache
            .resolve_with_locator(
                HashMap::from([(
                    OPAQUE_SESSION_KEY.to_string(),
                    operator_session_metadata(RAW_SESSION_ID, Some("codex-cli/first")),
                )]),
                move |ids| {
                    first_calls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(ids, vec![RAW_SESSION_ID.to_string()]);
                    async {
                        Ok(HashMap::from([(
                            RAW_SESSION_ID.to_string(),
                            host_session_location("/sessions/first.jsonl", Some("/workspace")),
                        )]))
                    }
                },
            )
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            first
                .get(OPAQUE_SESSION_KEY)
                .and_then(|session| session.host_local_transcript_path.as_deref()),
            Some("/sessions/first.jsonl")
        );

        let second_calls = Arc::clone(&calls);
        let second = cache
            .resolve_with_locator(
                HashMap::from([(
                    OPAQUE_SESSION_KEY.to_string(),
                    operator_session_metadata(RAW_SESSION_ID, Some("codex-cli/second")),
                )]),
                move |_| {
                    second_calls.fetch_add(1, Ordering::SeqCst);
                    async { Err(()) }
                },
            )
            .await;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let session = second
            .get(OPAQUE_SESSION_KEY)
            .expect("updated display metadata");
        assert_eq!(
            session.last_client_name.as_deref(),
            Some("codex-cli/second")
        );
        assert_eq!(session.cwd.as_deref(), Some("/workspace"));
        assert_eq!(
            session.host_local_transcript_path.as_deref(),
            Some("/sessions/first.jsonl")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn newly_seen_raw_session_id_is_the_only_locator_query() {
        const SECOND_RAW_SESSION_ID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

        let calls = Arc::new(AtomicUsize::new(0));
        let mut cache = LocalSessionEnrichmentCache::default();
        let first_calls = Arc::clone(&calls);
        cache
            .resolve_with_locator(
                HashMap::from([(
                    OPAQUE_SESSION_KEY.to_string(),
                    operator_session_metadata(RAW_SESSION_ID, None),
                )]),
                move |ids| {
                    first_calls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(ids, vec![RAW_SESSION_ID.to_string()]);
                    async {
                        Ok(HashMap::from([(
                            RAW_SESSION_ID.to_string(),
                            host_session_location("/sessions/first.jsonl", None),
                        )]))
                    }
                },
            )
            .await;

        let second_calls = Arc::clone(&calls);
        let resolved = cache
            .resolve_with_locator(
                HashMap::from([
                    (
                        OPAQUE_SESSION_KEY.to_string(),
                        operator_session_metadata(RAW_SESSION_ID, None),
                    ),
                    (
                        "session:second".to_string(),
                        operator_session_metadata(SECOND_RAW_SESSION_ID, None),
                    ),
                ]),
                move |ids| {
                    second_calls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(ids, vec![SECOND_RAW_SESSION_ID.to_string()]);
                    async {
                        Ok(HashMap::from([(
                            SECOND_RAW_SESSION_ID.to_string(),
                            host_session_location("/sessions/second.jsonl", None),
                        )]))
                    }
                },
            )
            .await;

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            resolved
                .get(OPAQUE_SESSION_KEY)
                .and_then(|session| session.host_local_transcript_path.as_deref()),
            Some("/sessions/first.jsonl")
        );
        assert_eq!(
            resolved
                .get("session:second")
                .and_then(|session| session.host_local_transcript_path.as_deref()),
            Some("/sessions/second.jsonl")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn locator_io_failure_keeps_positive_lkg_and_backs_off_new_id() {
        const SECOND_RAW_SESSION_ID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

        let calls = Arc::new(AtomicUsize::new(0));
        let mut cache = LocalSessionEnrichmentCache::default();
        let first_calls = Arc::clone(&calls);
        cache
            .resolve_with_locator(
                HashMap::from([(
                    OPAQUE_SESSION_KEY.to_string(),
                    operator_session_metadata(RAW_SESSION_ID, None),
                )]),
                move |_| {
                    first_calls.fetch_add(1, Ordering::SeqCst);
                    async {
                        Ok(HashMap::from([(
                            RAW_SESSION_ID.to_string(),
                            host_session_location("/sessions/lkg.jsonl", Some("/lkg")),
                        )]))
                    }
                },
            )
            .await;

        let source = HashMap::from([
            (
                OPAQUE_SESSION_KEY.to_string(),
                operator_session_metadata(RAW_SESSION_ID, Some("updated")),
            ),
            (
                "session:second".to_string(),
                operator_session_metadata(SECOND_RAW_SESSION_ID, None),
            ),
        ]);
        let failed_calls = Arc::clone(&calls);
        let failed = cache
            .resolve_with_locator(source.clone(), move |ids| {
                failed_calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(ids, vec![SECOND_RAW_SESSION_ID.to_string()]);
                async { Err(()) }
            })
            .await;

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let lkg = failed
            .get(OPAQUE_SESSION_KEY)
            .expect("positive locator LKG");
        assert_eq!(lkg.last_client_name.as_deref(), Some("updated"));
        assert_eq!(lkg.cwd.as_deref(), Some("/lkg"));
        assert_eq!(
            lkg.host_local_transcript_path.as_deref(),
            Some("/sessions/lkg.jsonl")
        );
        assert!(
            failed
                .get("session:second")
                .is_some_and(|session| session.host_local_transcript_path.is_none())
        );

        let backed_off_calls = Arc::clone(&calls);
        cache
            .resolve_with_locator(source, move |_| {
                backed_off_calls.fetch_add(1, Ordering::SeqCst);
                async { Err(()) }
            })
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn negative_locator_lookups_back_off_and_recover() {
        let calls = Arc::new(AtomicUsize::new(0));
        let source = HashMap::from([(
            OPAQUE_SESSION_KEY.to_string(),
            operator_session_metadata(RAW_SESSION_ID, None),
        )]);
        let mut cache = LocalSessionEnrichmentCache::default();

        for expected_calls in [1, 1] {
            let lookup_calls = Arc::clone(&calls);
            cache
                .resolve_with_locator(source.clone(), move |_| {
                    lookup_calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok(HashMap::new()) }
                })
                .await;
            assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
        }

        tokio::time::advance(NEGATIVE_LOOKUP_INITIAL).await;
        let second_lookup_calls = Arc::clone(&calls);
        cache
            .resolve_with_locator(source.clone(), move |_| {
                second_lookup_calls.fetch_add(1, Ordering::SeqCst);
                async { Ok(HashMap::new()) }
            })
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        tokio::time::advance(NEGATIVE_LOOKUP_INITIAL).await;
        let early_calls = Arc::clone(&calls);
        cache
            .resolve_with_locator(source.clone(), move |_| {
                early_calls.fetch_add(1, Ordering::SeqCst);
                async { Ok(HashMap::new()) }
            })
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        tokio::time::advance(NEGATIVE_LOOKUP_INITIAL).await;
        let recovery_calls = Arc::clone(&calls);
        let recovered = cache
            .resolve_with_locator(source.clone(), move |_| {
                recovery_calls.fetch_add(1, Ordering::SeqCst);
                async {
                    Ok(HashMap::from([(
                        RAW_SESSION_ID.to_string(),
                        host_session_location("/sessions/recovered.jsonl", Some("/recovered")),
                    )]))
                }
            })
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            recovered
                .get(OPAQUE_SESSION_KEY)
                .and_then(|session| session.host_local_transcript_path.as_deref()),
            Some("/sessions/recovered.jsonl")
        );

        tokio::time::advance(NEGATIVE_LOOKUP_MAX).await;
        let positive_calls = Arc::clone(&calls);
        cache
            .resolve_with_locator(source, move |_| {
                positive_calls.fetch_add(1, Ordering::SeqCst);
                async { Err(()) }
            })
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn negative_locator_backoff_caps_at_thirty_minutes() {
        let mut cache = LocalSessionEnrichmentCache::default();
        let now = Instant::now();
        for _ in 0..10 {
            cache.record_negative_lookup(RAW_SESSION_ID.to_string(), now);
        }

        let Some(TranscriptLocatorCacheEntry::Missing(missing)) =
            cache.locator_by_raw_session_id.get(RAW_SESSION_ID)
        else {
            panic!("negative locator cache entry");
        };
        assert_eq!(
            missing.retry_not_before.duration_since(now),
            NEGATIVE_LOOKUP_MAX
        );
    }

    #[tokio::test(start_paused = true)]
    async fn deleting_source_clears_locator_and_readding_queries_again() {
        let calls = Arc::new(AtomicUsize::new(0));
        let source = HashMap::from([(
            OPAQUE_SESSION_KEY.to_string(),
            operator_session_metadata(RAW_SESSION_ID, None),
        )]);
        let mut cache = LocalSessionEnrichmentCache::default();
        let first_calls = Arc::clone(&calls);
        cache
            .resolve_with_locator(source.clone(), move |_| {
                first_calls.fetch_add(1, Ordering::SeqCst);
                async {
                    Ok(HashMap::from([(
                        RAW_SESSION_ID.to_string(),
                        host_session_location("/sessions/first.jsonl", None),
                    )]))
                }
            })
            .await;

        let empty_calls = Arc::clone(&calls);
        let empty = cache
            .resolve_with_locator(HashMap::new(), move |_| {
                empty_calls.fetch_add(1, Ordering::SeqCst);
                async { Err(()) }
            })
            .await;
        assert!(empty.is_empty());
        assert!(cache.locator_by_raw_session_id.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let second_calls = Arc::clone(&calls);
        let readded = cache
            .resolve_with_locator(source, move |ids| {
                second_calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(ids, vec![RAW_SESSION_ID.to_string()]);
                async {
                    Ok(HashMap::from([(
                        RAW_SESSION_ID.to_string(),
                        host_session_location("/sessions/readded.jsonl", None),
                    )]))
                }
            })
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            readded
                .get(OPAQUE_SESSION_KEY)
                .and_then(|session| session.host_local_transcript_path.as_deref()),
            Some("/sessions/readded.jsonl")
        );
    }

    #[test]
    fn opaque_operator_session_key_resolves_real_codex_session_metadata() {
        let home = TestCodexHome::new();
        let cwd = home.root.join("workspace").join("project");
        std::fs::create_dir_all(&cwd).expect("create fixture cwd");
        let transcript_path = write_real_session_fixture(&home, &cwd);
        let client_name = format!("codex-cli/{SECRET_CANARY}");
        let source = HashMap::from([(
            OPAQUE_SESSION_KEY.to_string(),
            operator_session_metadata(RAW_SESSION_ID, Some(&client_name)),
        )]);
        let mut cache = LocalSessionEnrichmentCache::default();

        let resolved = test_runtime().block_on(cache.resolve(source));

        assert_ne!(OPAQUE_SESSION_KEY, RAW_SESSION_ID);
        let session = resolved
            .get(OPAQUE_SESSION_KEY)
            .expect("opaque operator session should remain the lookup key");
        assert_eq!(session.raw_session_id, RAW_SESSION_ID);
        assert_eq!(session.cwd.as_deref().map(Path::new), Some(cwd.as_path()));
        assert_eq!(
            session.host_local_transcript_path.as_deref().map(Path::new),
            Some(transcript_path.as_path())
        );
        assert_eq!(
            session.last_client_name.as_deref(),
            Some(client_name.as_str())
        );
        assert_eq!(session.last_client_addr.as_deref(), Some("127.0.0.1:43123"));

        let debug = format!("{cache:?}");
        assert!(debug.contains("source_session_count: 1"), "{debug}");
        assert!(!debug.contains(SECRET_CANARY), "{debug}");
        assert!(!debug.contains(RAW_SESSION_ID), "{debug}");
        assert!(!debug.contains(cwd.to_string_lossy().as_ref()), "{debug}");
    }

    #[test]
    fn real_codex_session_cwd_reaches_snapshot_and_dashboard() {
        let home = TestCodexHome::new();
        let cwd = home.root.join("workspace").join("project");
        std::fs::create_dir_all(&cwd).expect("create fixture cwd");
        write_real_session_fixture(&home, &cwd);
        let source = HashMap::from([(
            OPAQUE_SESSION_KEY.to_string(),
            operator_session_metadata(RAW_SESSION_ID, Some("codex-cli")),
        )]);
        let mut cache = LocalSessionEnrichmentCache::default();
        let local_sessions = test_runtime().block_on(cache.resolve(source));
        let data = operator_data_with_session(OPAQUE_SESSION_KEY);
        let snapshot = snapshot_from_operator_data(&data, &local_sessions);

        let row = snapshot.rows.first().expect("projected session row");
        assert_eq!(row.local_session_id.as_deref(), Some(RAW_SESSION_ID));
        assert_eq!(row.cwd.as_deref().map(Path::new), Some(cwd.as_path()));
        assert_eq!(
            row.observation_scope,
            crate::state::SessionObservationScope::HostLocalEnriched
        );

        let backend = TestBackend::new(160, 45);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut ui = UiState {
            page: Page::Dashboard,
            language: Language::En,
            ..UiState::default()
        };
        let frame = terminal
            .draw(|frame| {
                crate::tui::view::render_app(
                    frame,
                    Palette::default(),
                    &mut ui,
                    &snapshot,
                    "codex",
                    3211,
                    &[],
                );
            })
            .expect("render enriched Dashboard");
        let text = buffer_text(frame.buffer);
        assert!(text.contains(RAW_SESSION_ID), "{text}");
        assert!(text.contains("workspace/proj"), "{text}");
        assert!(text.contains(" project "), "{text}");
        assert!(text.contains("host-local enriched"), "{text}");
    }

    #[test]
    fn remote_observer_without_local_operator_metadata_does_not_discover_host_sessions() {
        let home = TestCodexHome::new();
        let cwd = home.root.join("workspace").join("remote-must-not-read");
        std::fs::create_dir_all(&cwd).expect("create remote fixture cwd");
        write_real_session_fixture(&home, &cwd);
        let mut cache = LocalSessionEnrichmentCache::default();

        // A remote read model carries only opaque keys. Without the loopback-only metadata
        // response there is no raw id to use for host-local discovery.
        let resolved = test_runtime().block_on(cache.resolve(HashMap::new()));

        assert!(resolved.is_empty());
        assert!(cache.current().is_empty());
    }

    #[test]
    fn malformed_oversized_jsonl_fails_safe_without_retaining_file_contents() {
        let home = TestCodexHome::new();
        write_oversized_malformed_session_fixture(&home);
        let raw_session_id = "metadata-after-bounded-header-scan";
        let source = HashMap::from([(
            OPAQUE_SESSION_KEY.to_string(),
            operator_session_metadata(raw_session_id, Some(SECRET_CANARY)),
        )]);
        let mut cache = LocalSessionEnrichmentCache::default();

        let resolved = test_runtime().block_on(cache.resolve(source));

        let session = resolved
            .get(OPAQUE_SESSION_KEY)
            .expect("unresolved operator metadata remains available");
        assert_eq!(session.raw_session_id, raw_session_id);
        assert!(session.cwd.is_none());
        assert!(session.host_local_transcript_path.is_none());
        let debug = format!("{cache:?}");
        assert!(!debug.contains(SECRET_CANARY), "{debug}");
        assert!(!debug.contains(raw_session_id), "{debug}");
    }

    #[tokio::test(start_paused = true)]
    async fn attached_metadata_failure_keeps_lkg_and_retries_after_backoff() {
        let _home = TestCodexHome::new();
        let mut attached = AttachedLocalSessionEnrichment::default();
        let session_keys = vec![OPAQUE_SESSION_KEY.to_string()];
        let initial_metadata = operator_session_metadata(RAW_SESSION_ID, Some("codex-cli"));

        let initial = attached
            .resolve_with("codex", session_keys.clone(), |_| async {
                Ok(LocalOperatorSessionMetadataResponse {
                    service_name: "codex".to_string(),
                    sessions: HashMap::from([(OPAQUE_SESSION_KEY.to_string(), initial_metadata)]),
                })
            })
            .await;
        let initial_freshness = attached
            .last_successful_fetch_at
            .expect("successful metadata freshness");
        assert_eq!(initial.issue, None);
        assert!(initial.sessions.contains_key(OPAQUE_SESSION_KEY));

        tokio::time::advance(ATTACHED_METADATA_REFRESH_INTERVAL).await;
        let failed = attached
            .resolve_with("codex", session_keys.clone(), |_| async { Err(()) })
            .await;

        assert_eq!(
            failed.issue,
            Some(LocalSessionEnrichmentIssue::MetadataUnavailable)
        );
        assert!(failed.sessions.contains_key(OPAQUE_SESSION_KEY));
        assert_eq!(attached.last_successful_fetch_at, Some(initial_freshness));

        let calls = Arc::new(AtomicUsize::new(0));
        let retry_calls = calls.clone();
        let waiting = attached
            .resolve_with("codex", session_keys.clone(), move |_| {
                retry_calls.fetch_add(1, Ordering::SeqCst);
                async { Err(()) }
            })
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(waiting.issue, failed.issue);

        tokio::time::advance(METADATA_RETRY_INITIAL).await;
        let recovered = attached
            .resolve_with("codex", session_keys, |_| async {
                Ok(LocalOperatorSessionMetadataResponse {
                    service_name: "codex".to_string(),
                    sessions: HashMap::from([(
                        OPAQUE_SESSION_KEY.to_string(),
                        operator_session_metadata(RAW_SESSION_ID, Some("codex-cli/recovered")),
                    )]),
                })
            })
            .await;

        assert_eq!(recovered.issue, None);
        assert_eq!(attached.consecutive_failures, 0);
        assert!(attached.retry_not_before.is_none());
        assert!(attached.last_successful_fetch_at > Some(initial_freshness));
        assert_eq!(
            recovered
                .sessions
                .get(OPAQUE_SESSION_KEY)
                .and_then(|session| session.last_client_name.as_deref()),
            Some("codex-cli/recovered")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn attached_metadata_service_mismatch_is_safe_and_backed_off() {
        let _home = TestCodexHome::new();
        let mut attached = AttachedLocalSessionEnrichment::default();

        let result = attached
            .resolve_with("codex", vec![OPAQUE_SESSION_KEY.to_string()], |_| async {
                Ok(LocalOperatorSessionMetadataResponse {
                    service_name: "claude".to_string(),
                    sessions: HashMap::new(),
                })
            })
            .await;

        assert_eq!(
            result.issue,
            Some(LocalSessionEnrichmentIssue::ServiceMismatch)
        );
        assert!(attached.last_successful_fetch_at.is_none());
        assert!(attached.retry_not_before.is_some());
    }

    #[test]
    fn attached_cache_debug_redacts_session_keys_and_metadata() {
        let metadata = operator_session_metadata(SECRET_CANARY, Some(SECRET_CANARY));
        let attached = AttachedLocalSessionEnrichment {
            last_session_keys: vec![SECRET_CANARY.to_string()],
            source: HashMap::from([(SECRET_CANARY.to_string(), metadata.clone())]),
            local: LocalSessionEnrichmentCache {
                source: HashMap::from([(SECRET_CANARY.to_string(), metadata.clone())]),
                resolved: HashMap::from([(SECRET_CANARY.to_string(), metadata)]),
                locator_by_raw_session_id: HashMap::new(),
            },
            ..Default::default()
        };

        let debug = format!("{attached:?}");
        assert!(debug.contains("last_session_key_count: 1"), "{debug}");
        assert!(debug.contains("source_session_count: 1"), "{debug}");
        assert!(!debug.contains(SECRET_CANARY), "{debug}");
    }
}

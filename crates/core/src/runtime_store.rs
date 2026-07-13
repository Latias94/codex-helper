use std::cell::Cell;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, ErrorCode, OpenFlags, TransactionBehavior, params};
use thiserror::Error;
use uuid::Uuid;

use crate::runtime_identity::{ProviderEndpointKey, RuntimeUpstreamIdentity};
use crate::state::FinishedRequest;

mod affinity;
mod lifecycle;
mod metadata;
mod policy;

pub use affinity::{SessionAffinityIdentitySource, SessionAffinityLimit, SessionAffinityRecord};
pub use lifecycle::{
    AttemptId, AttemptOutcome, AttemptPendingEvidence, AttemptRecord, AttemptRouteEvidence,
    AttemptTerminal, AttemptTerminalRecord, BeginDisposition, EconomicsState,
    FrozenProviderCatalogScope, FrozenProviderEpochIdentity, FrozenProviderPriceKey,
    LogicalRequestId, LogicalRequestOutcome, LogicalRequestRecord, LogicalRequestTerminal,
    LogicalRequestTerminalPayload, LogicalRequestTerminalRecord, NewAttempt, NewLogicalRequest,
    NilLifecycleId, RecoveryReport, RecoveryRunId, RequestAccountingScope, TerminalDisposition,
    TerminalOrigin,
};
pub use metadata::{
    RuntimeDocument, RuntimeDocumentCommit, RuntimeDocumentKind, RuntimeDocumentWrite,
    RuntimeQuotaIdentity,
};
pub use policy::{
    ProviderAutomaticEligibility, ProviderEffectiveEligibility, ProviderEligibilityProjection,
    ProviderManualEligibility, ProviderObservation, ProviderObservationAuthority,
    ProviderObservationCommit, ProviderObservationDisposition, ProviderObservationHistoryEntry,
    ProviderObservationReservation, ProviderObservationScope, ProviderObservationScopeError,
    ProviderObservationTicket, ProviderPolicyActionRecord, ProviderPolicyEffect,
    ProviderPolicySnapshot,
};

const APPLICATION_ID: i32 = 0x4348_5354;
const SCHEMA_REVISION: i32 = 2;
const FIRST_MIGRATABLE_SCHEMA_REVISION: i32 = 1;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const APPLICATION_NAME: &str = "codex-helper";
const SCHEMA_NAME: &str = "canonical-relay-runtime";
const SQLITE_SYNCHRONOUS_FULL: i32 = 2;
const DEFAULT_COMMITTED_REQUEST_LIMIT: usize = 100;

/// Stable ownership identity read from the helper-owned database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeStoreIdentity {
    store_id: Uuid,
}

/// Opaque, restart-stable revision of runtime-origin request terminals.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OperatorLedgerRevision(String);

impl OperatorLedgerRevision {
    pub(crate) fn new(store_id: Uuid, terminal_count: u64) -> Self {
        Self(format!("operator-ledger-v1:{store_id}:{terminal_count}"))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for OperatorLedgerRevision {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Filters applied to decoded committed request payloads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommittedRequestFilter {
    pub service: Option<String>,
    pub trace_id: Option<String>,
    pub request_id: Option<u64>,
    pub session: Option<String>,
    pub model: Option<String>,
    pub provider_endpoint: Option<ProviderEndpointKey>,
    pub provider: Option<String>,
    pub path: Option<String>,
    pub signal_kind: Option<String>,
    pub policy_action_kind: Option<String>,
    pub status_min: Option<u64>,
    pub status_max: Option<u64>,
    pub fast: bool,
    pub retried: bool,
}

pub(crate) fn final_route_provider_id(request: &FinishedRequest) -> Option<&str> {
    request
        .route_decision
        .as_ref()?
        .provider_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(crate) fn final_route_provider_endpoint(
    request: &FinishedRequest,
) -> Option<ProviderEndpointKey> {
    let service_name = request.service.trim();
    if service_name.is_empty() {
        return None;
    }
    let decision = request.route_decision.as_ref()?;
    let provider_id = decision
        .provider_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let endpoint_id = decision
        .endpoint_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(ProviderEndpointKey::new(
        service_name,
        provider_id,
        endpoint_id,
    ))
}

/// Stable keyset cursor for committed request projections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommittedRequestCursor {
    pub terminal_at_unix_ms: u64,
    pub logical_request_id: LogicalRequestId,
}

/// A newest-first query over runtime-origin committed request terminals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedRequestQuery {
    pub limit: usize,
    pub cursor: Option<CommittedRequestCursor>,
    pub terminal_at_or_after_unix_ms: Option<u64>,
    pub filter: CommittedRequestFilter,
}

impl Default for CommittedRequestQuery {
    fn default() -> Self {
        Self {
            limit: DEFAULT_COMMITTED_REQUEST_LIMIT,
            cursor: None,
            terminal_at_or_after_unix_ms: None,
            filter: CommittedRequestFilter::default(),
        }
    }
}

/// Compact startup metadata that does not require decoding terminal JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommittedRequestProjectionMetadata {
    pub terminal_count: u64,
    pub max_numeric_request_id: Option<u64>,
}

/// Exact service-scoped identities used by the request-chain export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedRequestIdentityQuery {
    pub service: String,
    pub trace_id: Option<String>,
    pub request_id: Option<u64>,
    pub session_id: Option<String>,
    pub limit: usize,
}

/// One decoded terminal event suitable for accounting and operator projections.
#[derive(Debug, Clone, PartialEq)]
pub struct CommittedRequestProjection {
    pub logical_request_id: LogicalRequestId,
    pub begun_at_unix_ms: u64,
    pub outcome: LogicalRequestOutcome,
    pub terminal_at_unix_ms: u64,
    pub economics_state: EconomicsState,
    pub payload: LogicalRequestTerminalPayload,
}

/// One keyset-paginated page of committed request terminals.
#[derive(Debug, Clone, PartialEq)]
pub struct CommittedRequestPage {
    pub items: Vec<CommittedRequestProjection>,
    pub next_cursor: Option<CommittedRequestCursor>,
}

/// A logical request ID bound to the store that owns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LogicalRequestHandle {
    store_id: Uuid,
    id: LogicalRequestId,
}

impl LogicalRequestHandle {
    pub fn store_id(&self) -> Uuid {
        self.store_id
    }

    pub fn id(&self) -> LogicalRequestId {
        self.id
    }
}

/// An upstream attempt ID bound to the store that owns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AttemptHandle {
    store_id: Uuid,
    id: AttemptId,
}

impl AttemptHandle {
    pub fn store_id(&self) -> Uuid {
        self.store_id
    }

    pub fn id(&self) -> AttemptId {
        self.id
    }
}

/// The result of beginning a logical request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeginLogicalRequestResult {
    pub disposition: BeginDisposition,
    pub handle: LogicalRequestHandle,
}

/// The result of beginning an upstream attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeginAttemptResult {
    pub disposition: BeginDisposition,
    pub attempt_ordinal: u64,
    pub handle: AttemptHandle,
}

impl RuntimeStoreIdentity {
    pub fn store_id(&self) -> Uuid {
        self.store_id
    }

    pub fn application(&self) -> &'static str {
        APPLICATION_NAME
    }

    pub fn schema(&self) -> &'static str {
        SCHEMA_NAME
    }

    pub fn schema_revision(&self) -> i32 {
        SCHEMA_REVISION
    }
}

/// Failures that prevent a runtime store from becoming authoritative.
#[derive(Debug, Error)]
pub enum RuntimeStoreError {
    #[error("failed to create runtime store directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to open runtime store writer lease {path}: {source}")]
    OpenWriterLease {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("runtime store at {path} already has an active writer")]
    WriterAlreadyOwned { path: PathBuf },
    #[error("failed to acquire runtime store writer lease {path}: {source}")]
    AcquireWriterLease {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to secure runtime store path {path}: {source}")]
    SecurePermissions {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("runtime store writer lease {path} is unsafe: {detail}")]
    UnsafeWriterLease { path: PathBuf, detail: String },
    #[error("runtime store database path {path} is unsafe: {detail}")]
    UnsafeDatabasePath { path: PathBuf, detail: String },
    #[error("failed to inspect runtime store database path {path}: {source}")]
    InspectDatabasePath {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to open runtime store database {path}: {source}")]
    OpenDatabase {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("runtime store database does not exist at {path}")]
    DatabaseMissing { path: PathBuf },
    #[error("runtime store operation {operation} failed for {path}: {source}")]
    Sqlite {
        path: PathBuf,
        operation: &'static str,
        #[source]
        source: rusqlite::Error,
    },
    #[error(
        "runtime store at {path} belongs to application id {actual:#010x}, expected {expected:#010x}"
    )]
    ForeignApplication {
        path: PathBuf,
        expected: i32,
        actual: i32,
    },
    #[error("runtime store at {path} uses schema revision {actual}, expected {expected}")]
    UnsupportedSchemaRevision {
        path: PathBuf,
        expected: i32,
        actual: i32,
    },
    #[error("refusing to claim unidentified nonempty database at {path}")]
    UnidentifiedNonemptyDatabase { path: PathBuf },
    #[error("runtime store database at {path} is corrupt: {source}")]
    CorruptDatabase {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("runtime store database at {path} failed integrity checking: {detail}")]
    IntegrityCheckFailed { path: PathBuf, detail: String },
    #[error("runtime store metadata at {path} is invalid: {detail}")]
    InvalidMetadata { path: PathBuf, detail: String },
    #[error("runtime store at {path} reported unsupported {setting} value {actual}")]
    UnsupportedSetting {
        path: PathBuf,
        setting: &'static str,
        actual: String,
    },
    #[error("runtime store {component} path {path} no longer identifies the opened file")]
    PersistentFileReplaced {
        component: &'static str,
        path: PathBuf,
    },
    #[error("runtime store cannot determine stable identity for {component} at {path}")]
    PersistentFileIdentityUnavailable {
        component: &'static str,
        path: PathBuf,
    },
    #[error("runtime store is poisoned after persistent file identity changed")]
    PersistentFileIdentityPoisoned,
    #[error("runtime store connection is unavailable because its mutex was poisoned")]
    ConnectionPoisoned,
    #[error(
        "runtime store {entity} handle {id} belongs to store {actual}, expected store {expected}"
    )]
    ForeignStoreHandle {
        entity: &'static str,
        id: String,
        expected: Uuid,
        actual: Uuid,
    },
    #[error("system time is before the Unix epoch")]
    SystemTimeBeforeUnixEpoch,
    #[error("system time exceeds the supported SQLite integer range")]
    SystemTimeOverflow,
    #[error("runtime store {entity} {id} violates a lifecycle invariant: {detail}")]
    InvariantViolation {
        entity: &'static str,
        id: String,
        detail: String,
    },
    #[cfg(test)]
    #[error("runtime store injected failure at {operation}")]
    InjectedFailure { operation: &'static str },
}

/// Exclusive helper-owned SQLite authority for one runtime.
pub struct RuntimeStore {
    identity: RuntimeStoreIdentity,
    startup_recovery: RecoveryReport,
    _connection: Mutex<RuntimeStoreConnection>,
    backing: StoreBacking,
    #[cfg(test)]
    fail_next_logical_terminal_commit: AtomicBool,
    #[cfg(test)]
    fail_next_attempt_begin: AtomicBool,
    #[cfg(test)]
    fail_next_attempt_terminal_commit: AtomicBool,
    #[cfg(test)]
    fail_next_policy_commit: AtomicBool,
    #[cfg(test)]
    fail_next_provider_quota_commit: AtomicBool,
    #[cfg(test)]
    fail_next_affinity_commit: AtomicBool,
    #[cfg(test)]
    next_transaction_delay_ms: AtomicU64,
}

/// Read-only access to an existing helper runtime store.
pub struct RuntimeStoreReader {
    identity: RuntimeStoreIdentity,
    connection: Mutex<RuntimeStoreReaderConnection>,
    path: PathBuf,
    opened_database_identity: FileIdentity,
}

struct RuntimeStoreReaderConnection {
    connection: Connection,
    poisoned: bool,
}

struct RuntimeStoreConnection {
    connection: Connection,
    persistent_files: Option<PersistentFileSet>,
    poisoned: bool,
}

/// Typed lifecycle operations within one atomic store transaction.
pub struct RuntimeStoreTransaction<'transaction, 'connection> {
    inner: lifecycle::LifecycleTransaction<'transaction, 'connection>,
    failed: Cell<bool>,
    #[cfg(test)]
    fail_next_logical_terminal_commit: &'transaction AtomicBool,
    #[cfg(test)]
    fail_next_attempt_begin: &'transaction AtomicBool,
    #[cfg(test)]
    fail_next_attempt_terminal_commit: &'transaction AtomicBool,
}

impl RuntimeStoreTransaction<'_, '_> {
    pub fn begin_logical_request(
        &self,
        candidate: NewLogicalRequest,
    ) -> Result<BeginLogicalRequestResult, RuntimeStoreError> {
        let result = self
            .inner
            .begin_logical_request(candidate)
            .map(|disposition| BeginLogicalRequestResult {
                disposition,
                handle: LogicalRequestHandle {
                    store_id: self.inner.store_id(),
                    id: candidate.id,
                },
            });
        self.track(result)
    }

    pub fn begin_attempt(
        &self,
        logical_request: LogicalRequestHandle,
        candidate: NewAttempt,
    ) -> Result<BeginAttemptResult, RuntimeStoreError> {
        #[cfg(test)]
        if self.fail_next_attempt_begin.swap(false, Ordering::SeqCst) {
            return self.track(Err(RuntimeStoreError::InjectedFailure {
                operation: "begin upstream attempt",
            }));
        }
        if let Err(error) = self.require_logical_request_handle(logical_request) {
            return self.track(Err(error));
        }
        if candidate.logical_request_id != logical_request.id {
            return self.track(Err(RuntimeStoreError::InvariantViolation {
                entity: "upstream attempt",
                id: candidate.id.to_string(),
                detail: format!(
                    "candidate parent {} does not match handle {}",
                    candidate.logical_request_id, logical_request.id
                ),
            }));
        }
        let attempt_id = candidate.id;
        let result = self
            .inner
            .begin_attempt(candidate)
            .map(|result| BeginAttemptResult {
                disposition: result.disposition,
                attempt_ordinal: result.attempt_ordinal,
                handle: AttemptHandle {
                    store_id: self.inner.store_id(),
                    id: attempt_id,
                },
            });
        self.track(result)
    }

    pub fn commit_attempt_terminal(
        &self,
        attempt: AttemptHandle,
        terminal: AttemptTerminal,
    ) -> Result<TerminalDisposition, RuntimeStoreError> {
        #[cfg(test)]
        if self
            .fail_next_attempt_terminal_commit
            .swap(false, Ordering::SeqCst)
        {
            return self.track(Err(RuntimeStoreError::InjectedFailure {
                operation: "commit upstream attempt terminal",
            }));
        }
        if let Err(error) = self.require_attempt_handle(attempt) {
            return self.track(Err(error));
        }
        let result = self.inner.commit_attempt_terminal(attempt.id, terminal);
        self.track(result)
    }

    pub fn commit_logical_request_terminal(
        &self,
        logical_request: LogicalRequestHandle,
        terminal: LogicalRequestTerminal,
    ) -> Result<TerminalDisposition, RuntimeStoreError> {
        #[cfg(test)]
        if self
            .fail_next_logical_terminal_commit
            .swap(false, Ordering::SeqCst)
        {
            return self.track(Err(RuntimeStoreError::InjectedFailure {
                operation: "commit logical request terminal",
            }));
        }
        if let Err(error) = self.require_logical_request_handle(logical_request) {
            return self.track(Err(error));
        }
        let result = self
            .inner
            .commit_logical_request_terminal(logical_request.id, terminal);
        self.track(result)
    }

    fn require_logical_request_handle(
        &self,
        handle: LogicalRequestHandle,
    ) -> Result<(), RuntimeStoreError> {
        require_store_handle(
            self.inner.store_id(),
            handle.store_id,
            "logical request",
            handle.id,
        )
    }

    fn require_attempt_handle(&self, handle: AttemptHandle) -> Result<(), RuntimeStoreError> {
        require_store_handle(
            self.inner.store_id(),
            handle.store_id,
            "upstream attempt",
            handle.id,
        )
    }

    fn track<T>(&self, result: Result<T, RuntimeStoreError>) -> Result<T, RuntimeStoreError> {
        if result.is_err() {
            self.failed.set(true);
        }
        result
    }
}

impl std::ops::Deref for RuntimeStoreConnection {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

impl std::ops::DerefMut for RuntimeStoreConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.connection
    }
}

impl std::fmt::Debug for RuntimeStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeStore")
            .field("identity", &self.identity)
            .field("path", &self.path())
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for RuntimeStoreReader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeStoreReader")
            .field("identity", &self.identity)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct WriterLease {
    path: PathBuf,
    file: File,
    #[cfg(unix)]
    _directory_lock: File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume_serial_number: u32,
    #[cfg(windows)]
    file_index: u64,
    #[cfg(not(any(unix, windows)))]
    length: u64,
    #[cfg(not(any(unix, windows)))]
    modified: Option<std::time::SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WalIdentity {
    Unobserved,
    Present(FileIdentity),
}

struct PersistentFileSet {
    database: FileIdentity,
    writer_lease: FileIdentity,
    wal: WalIdentity,
}

impl FileIdentity {
    fn from_metadata(
        metadata: &std::fs::Metadata,
        component: &'static str,
        path: &Path,
    ) -> Result<Self, RuntimeStoreError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let _ = (component, path);
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            let volume_serial_number = metadata.volume_serial_number().ok_or_else(|| {
                RuntimeStoreError::PersistentFileIdentityUnavailable {
                    component,
                    path: path.to_path_buf(),
                }
            })?;
            let file_index = metadata.file_index().ok_or_else(|| {
                RuntimeStoreError::PersistentFileIdentityUnavailable {
                    component,
                    path: path.to_path_buf(),
                }
            })?;
            Ok(Self {
                volume_serial_number,
                file_index,
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            Ok(Self {
                length: metadata.len(),
                modified: metadata.modified().ok(),
            })
        }
    }
}

impl PersistentFileSet {
    fn capture(backing: &StoreBacking) -> Result<Option<Self>, RuntimeStoreError> {
        let StoreBacking::Persistent {
            path,
            opened_database_identity,
            _writer_lease,
        } = backing
        else {
            return Ok(None);
        };
        let database = required_runtime_file_identity(path, "database")?;
        if database != *opened_database_identity {
            return Err(RuntimeStoreError::PersistentFileReplaced {
                component: "database",
                path: path.clone(),
            });
        }
        let held_writer_lease = FileIdentity::from_metadata(
            &_writer_lease.file.metadata().map_err(|_| {
                RuntimeStoreError::PersistentFileReplaced {
                    component: "writer lease",
                    path: _writer_lease.path.clone(),
                }
            })?,
            "writer lease",
            &_writer_lease.path,
        )?;
        let writer_lease = required_runtime_file_identity(&_writer_lease.path, "writer lease")?;
        if held_writer_lease != writer_lease {
            return Err(RuntimeStoreError::PersistentFileReplaced {
                component: "writer lease",
                path: _writer_lease.path.clone(),
            });
        }
        let wal_path = database_sidecar_path(path, "-wal");
        let wal = match optional_runtime_file_identity(&wal_path, "WAL")? {
            Some(identity) => WalIdentity::Present(identity),
            None => WalIdentity::Unobserved,
        };
        Ok(Some(Self {
            database,
            writer_lease,
            wal,
        }))
    }

    fn validate(&mut self, backing: &StoreBacking) -> Result<(), RuntimeStoreError> {
        let StoreBacking::Persistent {
            path,
            _writer_lease,
            ..
        } = backing
        else {
            return Ok(());
        };
        ensure_runtime_file_identity(path, "database", self.database)?;
        ensure_runtime_file_identity(&_writer_lease.path, "writer lease", self.writer_lease)?;
        let held_writer_lease = FileIdentity::from_metadata(
            &_writer_lease.file.metadata().map_err(|_| {
                RuntimeStoreError::PersistentFileReplaced {
                    component: "writer lease",
                    path: _writer_lease.path.clone(),
                }
            })?,
            "writer lease",
            &_writer_lease.path,
        )?;
        if held_writer_lease != self.writer_lease {
            return Err(RuntimeStoreError::PersistentFileReplaced {
                component: "writer lease",
                path: _writer_lease.path.clone(),
            });
        }
        let wal_path = database_sidecar_path(path, "-wal");
        match (self.wal, optional_runtime_file_identity(&wal_path, "WAL")?) {
            (WalIdentity::Unobserved, Some(identity)) => self.wal = WalIdentity::Present(identity),
            (WalIdentity::Unobserved, None) => {}
            (WalIdentity::Present(expected), Some(actual)) if expected == actual => {}
            (WalIdentity::Present(_), _) => {
                return Err(RuntimeStoreError::PersistentFileReplaced {
                    component: "WAL",
                    path: wal_path,
                });
            }
        }
        Ok(())
    }
}

fn optional_runtime_file_identity(
    path: &Path,
    component: &'static str,
) -> Result<Option<FileIdentity>, RuntimeStoreError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(RuntimeStoreError::PersistentFileIdentityUnavailable {
                component,
                path: path.to_path_buf(),
            });
        }
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata_has_multiple_links(&metadata)
    {
        return Err(RuntimeStoreError::PersistentFileReplaced {
            component,
            path: path.to_path_buf(),
        });
    }
    FileIdentity::from_metadata(&metadata, component, path).map(Some)
}

fn required_runtime_file_identity(
    path: &Path,
    component: &'static str,
) -> Result<FileIdentity, RuntimeStoreError> {
    optional_runtime_file_identity(path, component)?.ok_or_else(|| {
        RuntimeStoreError::PersistentFileReplaced {
            component,
            path: path.to_path_buf(),
        }
    })
}

fn ensure_runtime_file_identity(
    path: &Path,
    component: &'static str,
    expected: FileIdentity,
) -> Result<(), RuntimeStoreError> {
    let actual = required_runtime_file_identity(path, component)?;
    if actual != expected {
        return Err(RuntimeStoreError::PersistentFileReplaced {
            component,
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

enum StoreBacking {
    Persistent {
        path: PathBuf,
        opened_database_identity: FileIdentity,
        _writer_lease: WriterLease,
    },
    Memory,
}

struct OpenResources {
    connection: Connection,
    backing: StoreBacking,
}

impl StoreBacking {
    fn path(&self) -> Option<&Path> {
        match self {
            Self::Persistent { path, .. } => Some(path),
            Self::Memory => None,
        }
    }

    fn is_persistent(&self) -> bool {
        matches!(self, Self::Persistent { .. })
    }
}

impl RuntimeStore {
    /// Opens the canonical database under the configured helper home.
    pub fn open_default() -> Result<Self, RuntimeStoreError> {
        Self::open(runtime_store_path())
    }

    /// Opens the canonical database under an explicit helper home.
    pub fn open_in_home(helper_home: impl AsRef<Path>) -> Result<Self, RuntimeStoreError> {
        Self::open(runtime_store_path_in(helper_home.as_ref()))
    }

    fn open(path: impl AsRef<Path>) -> Result<Self, RuntimeStoreError> {
        let requested_path = path.as_ref();
        let parent = requested_path
            .parent()
            .ok_or_else(|| RuntimeStoreError::CreateDirectory {
                path: requested_path.to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "runtime store path has no parent directory",
                ),
            })?;
        prepare_state_directory(parent)?;
        let canonical_parent =
            std::fs::canonicalize(parent).map_err(|source| RuntimeStoreError::CreateDirectory {
                path: parent.to_path_buf(),
                source,
            })?;
        let file_name =
            requested_path
                .file_name()
                .ok_or_else(|| RuntimeStoreError::CreateDirectory {
                    path: requested_path.to_path_buf(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "runtime store path has no file name",
                    ),
                })?;
        let path = canonical_parent.join(file_name);

        let identity_before_lease = validate_database_path_if_present(&path)?;
        if identity_before_lease.is_some() {
            probe_existing_database(&path)?;
        }
        let writer_lease = WriterLease::acquire(&path)?;
        let identity_with_lease = validate_database_path_if_present(&path)?;
        if identity_before_lease != identity_with_lease {
            return Err(RuntimeStoreError::PersistentFileReplaced {
                component: "database",
                path,
            });
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let connection = Connection::open_with_flags(&path, flags).map_err(|source| {
            RuntimeStoreError::OpenDatabase {
                path: path.clone(),
                source,
            }
        })?;
        let identity_after_open = validate_database_path_if_present(&path)?.ok_or_else(|| {
            RuntimeStoreError::PersistentFileReplaced {
                component: "database",
                path: path.clone(),
            }
        })?;
        if identity_with_lease.is_some_and(|identity| identity != identity_after_open) {
            return Err(RuntimeStoreError::PersistentFileReplaced {
                component: "database",
                path,
            });
        }
        Self::finish_open(OpenResources {
            connection,
            backing: StoreBacking::Persistent {
                path,
                opened_database_identity: identity_after_open,
                _writer_lease: writer_lease,
            },
        })
    }

    /// Creates an isolated in-memory store for tests and explicit ephemeral use.
    pub fn open_in_memory() -> Result<Self, RuntimeStoreError> {
        let connection =
            Connection::open_in_memory().map_err(|source| RuntimeStoreError::OpenDatabase {
                path: PathBuf::from(":memory:"),
                source,
            })?;
        Self::finish_open(OpenResources {
            connection,
            backing: StoreBacking::Memory,
        })
    }

    /// Returns the validated database identity.
    pub fn identity(&self) -> &RuntimeStoreIdentity {
        &self.identity
    }

    /// Returns the database path, or `None` for an in-memory store.
    pub fn path(&self) -> Option<&Path> {
        self.backing.path()
    }

    /// Loads the installation-local quota identity, creating it in this store when absent.
    pub fn load_or_create_quota_identity(&self) -> Result<RuntimeQuotaIdentity, RuntimeStoreError> {
        let created_at_unix_ms = current_unix_time_ms()?;
        self.write_store_transaction("load or create quota identity", |transaction, path| {
            metadata::load_or_create_quota_identity(
                transaction,
                path,
                self.identity.store_id,
                created_at_unix_ms,
            )
        })
    }

    /// Reads one versioned document from the canonical runtime database.
    pub fn read_runtime_document(
        &self,
        kind: RuntimeDocumentKind,
    ) -> Result<Option<RuntimeDocument>, RuntimeStoreError> {
        let connection = self
            ._connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        metadata::read_document(
            &connection.connection,
            &self.display_path(),
            self.identity.store_id,
            kind,
        )
    }

    /// Atomically commits one or more versioned helper-owned documents.
    pub fn write_runtime_documents(
        &self,
        writes: &[RuntimeDocumentWrite<'_>],
    ) -> Result<Vec<RuntimeDocument>, RuntimeStoreError> {
        let updated_at_unix_ms = current_unix_time_ms()?;
        self.write_store_transaction("write runtime documents", |transaction, path| {
            metadata::write_documents(
                transaction,
                path,
                self.identity.store_id,
                updated_at_unix_ms,
                writes,
            )
        })
    }

    /// Commits one document only when its current revision matches the observed revision.
    pub fn compare_and_write_runtime_document(
        &self,
        expected_revision: Option<u64>,
        write: RuntimeDocumentWrite<'_>,
    ) -> Result<RuntimeDocumentCommit, RuntimeStoreError> {
        let updated_at_unix_ms = current_unix_time_ms()?;
        self.write_store_transaction("compare and write runtime document", |transaction, path| {
            let current =
                metadata::read_document(transaction, path, self.identity.store_id, write.kind)?;
            if current.as_ref().map(|document| document.revision) != expected_revision {
                return Ok(RuntimeDocumentCommit::Stale(current));
            }

            let mut committed = metadata::write_documents(
                transaction,
                path,
                self.identity.store_id,
                updated_at_unix_ms,
                &[write],
            )?;
            let Some(document) = committed.pop() else {
                return Err(RuntimeStoreError::InvariantViolation {
                    entity: "runtime document",
                    id: "conditional commit".to_string(),
                    detail: "document write returned no committed revision".to_string(),
                });
            };
            Ok(RuntimeDocumentCommit::Committed(document))
        })
    }

    /// Returns the recovery transaction committed during this open.
    pub fn startup_recovery_report(&self) -> RecoveryReport {
        self.startup_recovery
    }

    /// Binds a known logical request ID to this store.
    pub fn logical_request_handle(&self, id: LogicalRequestId) -> LogicalRequestHandle {
        LogicalRequestHandle {
            store_id: self.identity.store_id,
            id,
        }
    }

    /// Binds a known upstream attempt ID to this store.
    pub fn attempt_handle(&self, id: AttemptId) -> AttemptHandle {
        AttemptHandle {
            store_id: self.identity.store_id,
            id,
        }
    }

    /// Runs typed lifecycle mutations in one immediate transaction.
    pub fn transaction<T>(
        &self,
        operation: impl FnOnce(&RuntimeStoreTransaction<'_, '_>) -> Result<T, RuntimeStoreError>,
    ) -> Result<T, RuntimeStoreError> {
        let mut connection = self
            ._connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        validate_connection_file_identity(&mut connection, &self.backing)?;
        #[cfg(test)]
        {
            let delay_ms = self.next_transaction_delay_ms.swap(0, Ordering::SeqCst);
            if delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
        }
        let display_path = self.display_path();
        let result = {
            let transaction = connection
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|source| {
                    sqlite_error(&display_path, "begin lifecycle transaction", source)
                })?;
            let (operation_result, lifecycle_failed) = {
                let transaction_api = RuntimeStoreTransaction {
                    inner: lifecycle::LifecycleTransaction::new(
                        &transaction,
                        &display_path,
                        self.identity.store_id,
                    ),
                    failed: Cell::new(false),
                    #[cfg(test)]
                    fail_next_logical_terminal_commit: &self.fail_next_logical_terminal_commit,
                    #[cfg(test)]
                    fail_next_attempt_begin: &self.fail_next_attempt_begin,
                    #[cfg(test)]
                    fail_next_attempt_terminal_commit: &self.fail_next_attempt_terminal_commit,
                };
                let result = operation(&transaction_api);
                (result, transaction_api.failed.get())
            };
            match operation_result {
                Ok(_) if lifecycle_failed => Err(RuntimeStoreError::InvariantViolation {
                    entity: "lifecycle transaction",
                    id: self.identity.store_id.to_string(),
                    detail: "a lifecycle error was ignored by the transaction callback".to_string(),
                }),
                Ok(value) => transaction
                    .commit()
                    .map_err(|source| {
                        sqlite_error(&display_path, "commit lifecycle transaction", source)
                    })
                    .map(|()| value),
                Err(error) => Err(error),
            }
        };
        let identity_result = validate_connection_file_identity(&mut connection, &self.backing);
        identity_result?;
        result
    }

    /// Reads one logical request from this store.
    pub fn read_logical_request(
        &self,
        handle: LogicalRequestHandle,
    ) -> Result<Option<LogicalRequestRecord>, RuntimeStoreError> {
        require_store_handle(
            self.identity.store_id,
            handle.store_id,
            "logical request",
            handle.id,
        )?;
        let mut connection = self
            ._connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        validate_connection_file_identity(&mut connection, &self.backing)?;
        lifecycle::read_logical_request(
            &connection.connection,
            &self.display_path(),
            self.identity.store_id,
            handle.id,
        )
    }

    /// Reads the most recently begun logical requests, newest first.
    pub fn read_recent_logical_requests(
        &self,
        limit: u64,
    ) -> Result<Vec<LogicalRequestRecord>, RuntimeStoreError> {
        let mut connection = self
            ._connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        validate_connection_file_identity(&mut connection, &self.backing)?;
        lifecycle::read_recent_logical_requests(
            &connection.connection,
            &self.display_path(),
            self.identity.store_id,
            limit,
        )
    }

    /// Reads runtime-origin committed request terminals in stable newest-first order.
    pub fn query_committed_requests(
        &self,
        query: &CommittedRequestQuery,
    ) -> Result<CommittedRequestPage, RuntimeStoreError> {
        let mut connection = self
            ._connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        validate_connection_file_identity(&mut connection, &self.backing)?;
        lifecycle::query_committed_requests(
            &connection.connection,
            &self.display_path(),
            self.identity.store_id,
            query,
        )
    }

    /// Reads exact service-scoped request identities through typed SQLite projections.
    pub fn query_committed_request_identities(
        &self,
        query: &CommittedRequestIdentityQuery,
    ) -> Result<Vec<CommittedRequestProjection>, RuntimeStoreError> {
        let mut connection = self
            ._connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        validate_connection_file_identity(&mut connection, &self.backing)?;
        lifecycle::query_committed_request_identities(
            &connection.connection,
            &self.display_path(),
            self.identity.store_id,
            query,
        )
    }

    pub(crate) fn committed_request_projection_metadata(
        &self,
    ) -> Result<CommittedRequestProjectionMetadata, RuntimeStoreError> {
        let mut connection = self
            ._connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        validate_connection_file_identity(&mut connection, &self.backing)?;
        lifecycle::committed_request_projection_metadata(
            &connection.connection,
            &self.display_path(),
            self.identity.store_id,
        )
    }

    /// Returns an opaque revision that survives reopening the same runtime store.
    #[cfg(test)]
    pub fn operator_ledger_revision(&self) -> Result<OperatorLedgerRevision, RuntimeStoreError> {
        let mut connection = self
            ._connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        validate_connection_file_identity(&mut connection, &self.backing)?;
        let terminal_count = lifecycle::count_committed_request_terminals(
            &connection.connection,
            &self.display_path(),
            self.identity.store_id,
        )?;
        Ok(OperatorLedgerRevision::new(
            self.identity.store_id,
            terminal_count,
        ))
    }

    /// Reserves the next monotonic observation generation before network I/O.
    pub fn reserve_provider_observation(
        &self,
        scope: ProviderObservationScope,
        reserved_at_unix_ms: u64,
    ) -> Result<ProviderObservationReservation, RuntimeStoreError> {
        self.write_store_transaction("reserve provider observation", |transaction, path| {
            policy::reserve_provider_observation(
                transaction,
                path,
                self.identity.store_id,
                scope,
                reserved_at_unix_ms,
            )
        })
    }

    /// Atomically records an observation, action history, projection, and revision.
    pub fn commit_provider_observation(
        &self,
        ticket: ProviderObservationTicket,
        observation: ProviderObservation,
    ) -> Result<ProviderObservationCommit, RuntimeStoreError> {
        self.write_store_transaction("commit provider observation", |transaction, path| {
            let result = policy::commit_provider_observation(
                transaction,
                path,
                self.identity.store_id,
                ticket,
                observation,
            );
            #[cfg(test)]
            if result.is_ok() && self.fail_next_policy_commit.swap(false, Ordering::SeqCst) {
                return Err(RuntimeStoreError::InjectedFailure {
                    operation: "commit provider observation",
                });
            }
            result
        })
    }

    /// Atomically commits one provider observation and its quota-registry projection.
    pub(crate) fn commit_provider_observation_and_quota_registry(
        &self,
        ticket: ProviderObservationTicket,
        observation: ProviderObservation,
        expected_quota_revision: Option<u64>,
        quota_registry: RuntimeDocumentWrite<'_>,
    ) -> Result<(ProviderObservationCommit, Option<RuntimeDocument>), RuntimeStoreError> {
        if quota_registry.kind != RuntimeDocumentKind::QuotaRegistry {
            return Err(RuntimeStoreError::InvariantViolation {
                entity: "runtime document",
                id: "quota_registry".to_string(),
                detail: "atomic provider observation requires a quota registry document"
                    .to_string(),
            });
        }
        let updated_at_unix_ms = current_unix_time_ms()?;
        self.write_store_transaction(
            "commit provider observation and quota registry",
            |transaction, path| {
                let committed = policy::commit_provider_observation(
                    transaction,
                    path,
                    self.identity.store_id,
                    ticket,
                    observation,
                )?;
                #[cfg(test)]
                if self.fail_next_policy_commit.swap(false, Ordering::SeqCst) {
                    return Err(RuntimeStoreError::InjectedFailure {
                        operation: "commit provider observation",
                    });
                }
                if committed.disposition != ProviderObservationDisposition::Accepted {
                    return Ok((committed, None));
                }

                let current = metadata::read_document(
                    transaction,
                    path,
                    self.identity.store_id,
                    RuntimeDocumentKind::QuotaRegistry,
                )?;
                if let Some(current) = current.as_ref()
                    && current.schema_version != quota_registry.schema_version
                {
                    return Err(RuntimeStoreError::InvariantViolation {
                        entity: "runtime document",
                        id: "quota_registry".to_string(),
                        detail: format!(
                            "refusing to replace schema {} with schema {}",
                            current.schema_version, quota_registry.schema_version
                        ),
                    });
                }
                let actual_revision = current.as_ref().map(|document| document.revision);
                if actual_revision != expected_quota_revision {
                    return Err(RuntimeStoreError::InvariantViolation {
                        entity: "runtime document",
                        id: "quota_registry".to_string(),
                        detail: format!(
                            "expected revision {expected_quota_revision:?}, found {actual_revision:?}"
                        ),
                    });
                }

                let mut documents = metadata::write_documents(
                    transaction,
                    path,
                    self.identity.store_id,
                    updated_at_unix_ms,
                    &[quota_registry],
                )?;
                let document = documents.pop().ok_or_else(|| {
                    RuntimeStoreError::InvariantViolation {
                        entity: "runtime document",
                        id: "quota_registry".to_string(),
                        detail: "quota registry write returned no committed document".to_string(),
                    }
                })?;
                #[cfg(test)]
                if self
                    .fail_next_provider_quota_commit
                    .swap(false, Ordering::SeqCst)
                {
                    return Err(RuntimeStoreError::InjectedFailure {
                        operation: "commit provider observation and quota registry",
                    });
                }
                Ok((committed, Some(document)))
            },
        )
    }

    /// Invalidates automatic policy tied to removed or replaced upstream identities.
    pub fn reconcile_runtime_upstream_identities(
        &self,
        identities: &[RuntimeUpstreamIdentity],
        updated_at_unix_ms: u64,
    ) -> Result<ProviderPolicySnapshot, RuntimeStoreError> {
        self.write_store_transaction(
            "reconcile runtime upstream identities",
            |transaction, path| {
                let result = policy::reconcile_runtime_upstream_identities(
                    transaction,
                    path,
                    self.identity.store_id,
                    identities,
                    updated_at_unix_ms,
                );
                #[cfg(test)]
                if result.is_ok() && self.fail_next_policy_commit.swap(false, Ordering::SeqCst) {
                    return Err(RuntimeStoreError::InjectedFailure {
                        operation: "reconcile runtime upstream identities",
                    });
                }
                result
            },
        )
    }

    /// Sets the manual eligibility layer without changing automatic state.
    pub fn set_provider_manual_eligibility(
        &self,
        provider_endpoint: crate::runtime_identity::ProviderEndpointKey,
        manual: ProviderManualEligibility,
        reason: Option<String>,
        updated_at_unix_ms: u64,
    ) -> Result<ProviderEligibilityProjection, RuntimeStoreError> {
        self.write_store_transaction("set provider manual eligibility", |transaction, path| {
            let result = policy::set_provider_manual_eligibility(
                transaction,
                path,
                self.identity.store_id,
                provider_endpoint,
                manual,
                reason,
                updated_at_unix_ms,
            );
            #[cfg(test)]
            if result.is_ok() && self.fail_next_policy_commit.swap(false, Ordering::SeqCst) {
                return Err(RuntimeStoreError::InjectedFailure {
                    operation: "set provider manual eligibility",
                });
            }
            result
        })
    }

    /// Reads the latest committed policy projection bundle.
    pub fn provider_policy_snapshot(&self) -> Result<ProviderPolicySnapshot, RuntimeStoreError> {
        let mut connection = self
            ._connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        validate_connection_file_identity(&mut connection, &self.backing)?;
        policy::provider_policy_snapshot(
            &connection.connection,
            &self.display_path(),
            self.identity.store_id,
        )
    }

    /// Reads durable observation history in incarnation/generation order.
    pub fn read_provider_observation_history(
        &self,
        provider_endpoint: &crate::runtime_identity::ProviderEndpointKey,
        limit: u64,
    ) -> Result<Vec<ProviderObservationHistoryEntry>, RuntimeStoreError> {
        let mut connection = self
            ._connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        validate_connection_file_identity(&mut connection, &self.backing)?;
        policy::read_provider_observation_history(
            &connection.connection,
            &self.display_path(),
            self.identity.store_id,
            provider_endpoint,
            limit,
        )
    }

    /// Atomically inserts or replaces one session affinity and enforces capacity.
    pub fn upsert_session_affinity(
        &self,
        record: SessionAffinityRecord,
        limit: SessionAffinityLimit,
    ) -> Result<(), RuntimeStoreError> {
        self.write_store_transaction("upsert session affinity", |transaction, path| {
            let result = affinity::upsert_session_affinity(
                transaction,
                path,
                self.identity.store_id,
                &record,
                limit,
            );
            #[cfg(test)]
            if result.is_ok() && self.fail_next_affinity_commit.swap(false, Ordering::SeqCst) {
                return Err(RuntimeStoreError::InjectedFailure {
                    operation: "upsert session affinity",
                });
            }
            result
        })
    }

    /// Reads one unexpired session affinity without deleting expired state.
    pub fn get_session_affinity(
        &self,
        session_id: &str,
        now_unix_ms: u64,
        ttl_ms: u64,
    ) -> Result<Option<SessionAffinityRecord>, RuntimeStoreError> {
        let mut connection = self
            ._connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        validate_connection_file_identity(&mut connection, &self.backing)?;
        affinity::get_session_affinity(
            &connection.connection,
            &self.display_path(),
            self.identity.store_id,
            session_id,
            now_unix_ms,
            ttl_ms,
        )
    }

    /// Lists unexpired session affinities in stable least-recently-used order.
    pub fn list_session_affinities(
        &self,
        now_unix_ms: u64,
        ttl_ms: u64,
    ) -> Result<Vec<SessionAffinityRecord>, RuntimeStoreError> {
        let mut connection = self
            ._connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        validate_connection_file_identity(&mut connection, &self.backing)?;
        affinity::list_session_affinities(
            &connection.connection,
            &self.display_path(),
            self.identity.store_id,
            now_unix_ms,
            ttl_ms,
        )
    }

    /// Returns the durable row count, including expired rows not yet pruned.
    pub fn count_session_affinities(&self) -> Result<u64, RuntimeStoreError> {
        let mut connection = self
            ._connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        validate_connection_file_identity(&mut connection, &self.backing)?;
        affinity::count_session_affinities(
            &connection.connection,
            &self.display_path(),
            self.identity.store_id,
        )
    }

    /// Explicitly prunes expired rows and then enforces the requested capacity.
    pub fn prune_session_affinities(
        &self,
        now_unix_ms: u64,
        ttl_ms: u64,
        limit: SessionAffinityLimit,
    ) -> Result<u64, RuntimeStoreError> {
        self.write_store_transaction("prune session affinities", |transaction, path| {
            let result = affinity::prune_session_affinities(
                transaction,
                path,
                self.identity.store_id,
                now_unix_ms,
                ttl_ms,
                limit,
            );
            #[cfg(test)]
            if result.is_ok() && self.fail_next_affinity_commit.swap(false, Ordering::SeqCst) {
                return Err(RuntimeStoreError::InjectedFailure {
                    operation: "prune session affinities",
                });
            }
            result
        })
    }

    #[cfg(test)]
    pub(crate) fn fail_next_logical_terminal_commit_for_test(&self) {
        self.fail_next_logical_terminal_commit
            .store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_attempt_begin_for_test(&self) {
        self.fail_next_attempt_begin.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_attempt_terminal_commit_for_test(&self) {
        self.fail_next_attempt_terminal_commit
            .store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_policy_commit_for_test(&self) {
        self.fail_next_policy_commit.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_provider_quota_commit_for_test(&self) {
        self.fail_next_provider_quota_commit
            .store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_affinity_commit_for_test(&self) {
        self.fail_next_affinity_commit.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn delay_next_transaction_for_test(&self, duration: Duration) {
        let delay_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        self.next_transaction_delay_ms
            .store(delay_ms, Ordering::SeqCst);
    }

    /// Reads one upstream attempt from this store.
    pub fn read_attempt(
        &self,
        handle: AttemptHandle,
    ) -> Result<Option<AttemptRecord>, RuntimeStoreError> {
        require_store_handle(
            self.identity.store_id,
            handle.store_id,
            "upstream attempt",
            handle.id,
        )?;
        let mut connection = self
            ._connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        validate_connection_file_identity(&mut connection, &self.backing)?;
        lifecycle::read_attempt(
            &connection.connection,
            &self.display_path(),
            self.identity.store_id,
            handle.id,
        )
    }

    /// Reads all upstream attempts for one logical request in attempt order.
    pub fn read_attempts_for_logical_request(
        &self,
        logical_request: LogicalRequestHandle,
    ) -> Result<Vec<AttemptRecord>, RuntimeStoreError> {
        require_store_handle(
            self.identity.store_id,
            logical_request.store_id,
            "logical request",
            logical_request.id,
        )?;
        let mut connection = self
            ._connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        validate_connection_file_identity(&mut connection, &self.backing)?;
        lifecycle::read_attempts_for_logical_request(
            &connection.connection,
            &self.display_path(),
            self.identity.store_id,
            logical_request.id,
        )
    }

    /// Reads the latest committed startup recovery report.
    pub fn latest_recovery_report(&self) -> Result<Option<RecoveryReport>, RuntimeStoreError> {
        let mut connection = self
            ._connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        validate_connection_file_identity(&mut connection, &self.backing)?;
        lifecycle::read_latest_recovery_report(
            &connection.connection,
            &self.display_path(),
            self.identity.store_id,
        )
    }

    fn display_path(&self) -> PathBuf {
        self.backing
            .path()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(":memory:"))
    }

    fn write_store_transaction<T>(
        &self,
        operation_name: &'static str,
        operation: impl FnOnce(&rusqlite::Transaction<'_>, &Path) -> Result<T, RuntimeStoreError>,
    ) -> Result<T, RuntimeStoreError> {
        let mut connection = self
            ._connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        validate_connection_file_identity(&mut connection, &self.backing)?;
        let display_path = self.display_path();
        let result = {
            let transaction = connection
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|source| sqlite_error(&display_path, operation_name, source))?;
            match operation(&transaction, &display_path) {
                Ok(value) => transaction
                    .commit()
                    .map_err(|source| sqlite_error(&display_path, operation_name, source))
                    .map(|()| value),
                Err(error) => Err(error),
            }
        };
        validate_connection_file_identity(&mut connection, &self.backing)?;
        result
    }

    fn finish_open(mut resources: OpenResources) -> Result<Self, RuntimeStoreError> {
        let display_path = resources
            .backing
            .path()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(":memory:"));
        let persistent = resources.backing.is_persistent();
        let mut persistent_files = PersistentFileSet::capture(&resources.backing)?;
        validate_open_file_identity(&mut persistent_files, &resources.backing)?;
        configure_connection_basics(&resources.connection, &display_path)?;
        let database_state = inspect_database_state(&resources.connection, &display_path)?;

        let identity = match database_state {
            DatabaseState::Uninitialized => {
                validate_open_file_identity(&mut persistent_files, &resources.backing)?;
                configure_durability(&resources.connection, &display_path, persistent)?;
                validate_open_file_identity(&mut persistent_files, &resources.backing)?;
                let identity = initialize_database(&mut resources.connection, &display_path)?;
                validate_open_file_identity(&mut persistent_files, &resources.backing)?;
                identity
            }
            DatabaseState::Initialized => {
                let identity = read_identity(&resources.connection, &display_path)?;
                validate_open_file_identity(&mut persistent_files, &resources.backing)?;
                configure_durability(&resources.connection, &display_path, persistent)?;
                validate_open_file_identity(&mut persistent_files, &resources.backing)?;
                identity
            }
            DatabaseState::MigrationRequired => {
                validate_open_file_identity(&mut persistent_files, &resources.backing)?;
                configure_durability(&resources.connection, &display_path, persistent)?;
                validate_open_file_identity(&mut persistent_files, &resources.backing)?;
                let identity = migrate_database(&mut resources.connection, &display_path)?;
                validate_open_file_identity(&mut persistent_files, &resources.backing)?;
                identity
            }
        };
        if persistent {
            secure_helper_owned_database_files(&display_path)?;
            validate_open_file_identity(&mut persistent_files, &resources.backing)?;
        }

        let startup_recovery = run_startup_recovery(
            &mut resources.connection,
            &resources.backing,
            persistent_files.as_mut(),
            &display_path,
            identity.store_id,
        )?;

        Ok(Self {
            identity,
            startup_recovery,
            _connection: Mutex::new(RuntimeStoreConnection {
                connection: resources.connection,
                persistent_files,
                poisoned: false,
            }),
            backing: resources.backing,
            #[cfg(test)]
            fail_next_logical_terminal_commit: AtomicBool::new(false),
            #[cfg(test)]
            fail_next_attempt_begin: AtomicBool::new(false),
            #[cfg(test)]
            fail_next_attempt_terminal_commit: AtomicBool::new(false),
            #[cfg(test)]
            fail_next_policy_commit: AtomicBool::new(false),
            #[cfg(test)]
            fail_next_provider_quota_commit: AtomicBool::new(false),
            #[cfg(test)]
            fail_next_affinity_commit: AtomicBool::new(false),
            #[cfg(test)]
            next_transaction_delay_ms: AtomicU64::new(0),
        })
    }
}

impl RuntimeStoreReader {
    /// Opens the existing canonical database under the configured helper home.
    pub fn open_default() -> Result<Self, RuntimeStoreError> {
        Self::open(runtime_store_path())
    }

    /// Opens the existing canonical database under an explicit helper home.
    pub fn open_in_home(helper_home: impl AsRef<Path>) -> Result<Self, RuntimeStoreError> {
        Self::open(runtime_store_path_in(helper_home.as_ref()))
    }

    fn open(requested_path: impl AsRef<Path>) -> Result<Self, RuntimeStoreError> {
        let requested_path = requested_path.as_ref();
        let parent = requested_path
            .parent()
            .ok_or_else(|| RuntimeStoreError::DatabaseMissing {
                path: requested_path.to_path_buf(),
            })?;
        if !parent.exists() {
            return Err(RuntimeStoreError::DatabaseMissing {
                path: requested_path.to_path_buf(),
            });
        }
        let canonical_parent = std::fs::canonicalize(parent).map_err(|source| {
            RuntimeStoreError::InspectDatabasePath {
                path: parent.to_path_buf(),
                source,
            }
        })?;
        let file_name =
            requested_path
                .file_name()
                .ok_or_else(|| RuntimeStoreError::DatabaseMissing {
                    path: requested_path.to_path_buf(),
                })?;
        let path = canonical_parent.join(file_name);
        let opened_database_identity = validate_database_path_if_present(&path)?
            .ok_or_else(|| RuntimeStoreError::DatabaseMissing { path: path.clone() })?;
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let connection = Connection::open_with_flags(&path, flags).map_err(|source| {
            RuntimeStoreError::OpenDatabase {
                path: path.clone(),
                source,
            }
        })?;
        configure_connection_basics(&connection, &path)?;
        connection
            .pragma_update(None, "query_only", true)
            .map_err(|source| sqlite_error(&path, "enable read-only query mode", source))?;
        match inspect_database_state(&connection, &path)? {
            DatabaseState::Initialized => {}
            DatabaseState::MigrationRequired => {
                return Err(RuntimeStoreError::UnsupportedSchemaRevision {
                    path,
                    expected: SCHEMA_REVISION,
                    actual: FIRST_MIGRATABLE_SCHEMA_REVISION,
                });
            }
            DatabaseState::Uninitialized => {
                return Err(invalid_metadata(
                    &path,
                    "read-only runtime store is uninitialized",
                ));
            }
        }
        let identity = read_identity(&connection, &path)?;
        ensure_runtime_file_identity(&path, "database", opened_database_identity)?;

        Ok(Self {
            identity,
            connection: Mutex::new(RuntimeStoreReaderConnection {
                connection,
                poisoned: false,
            }),
            path,
            opened_database_identity,
        })
    }

    /// Returns the validated database identity.
    pub fn identity(&self) -> &RuntimeStoreIdentity {
        &self.identity
    }

    /// Returns the existing database path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads one versioned document without acquiring writer ownership.
    pub fn read_runtime_document(
        &self,
        kind: RuntimeDocumentKind,
    ) -> Result<Option<RuntimeDocument>, RuntimeStoreError> {
        self.with_connection(|connection| {
            metadata::read_document(connection, &self.path, self.identity.store_id, kind)
        })
    }

    /// Reads the latest committed startup recovery report without running recovery.
    pub fn latest_recovery_report(&self) -> Result<Option<RecoveryReport>, RuntimeStoreError> {
        self.with_connection(|connection| {
            lifecycle::read_latest_recovery_report(connection, &self.path, self.identity.store_id)
        })
    }

    /// Reads runtime-origin committed request terminals without acquiring writer ownership.
    pub fn query_committed_requests(
        &self,
        query: &CommittedRequestQuery,
    ) -> Result<CommittedRequestPage, RuntimeStoreError> {
        self.with_connection(|connection| {
            lifecycle::query_committed_requests(
                connection,
                &self.path,
                self.identity.store_id,
                query,
            )
        })
    }

    /// Reads exact service-scoped request identities without acquiring writer ownership.
    pub fn query_committed_request_identities(
        &self,
        query: &CommittedRequestIdentityQuery,
    ) -> Result<Vec<CommittedRequestProjection>, RuntimeStoreError> {
        self.with_connection(|connection| {
            lifecycle::query_committed_request_identities(
                connection,
                &self.path,
                self.identity.store_id,
                query,
            )
        })
    }

    /// Reads the latest committed policy projection without running recovery.
    pub fn provider_policy_snapshot(&self) -> Result<ProviderPolicySnapshot, RuntimeStoreError> {
        self.with_connection(|connection| {
            policy::provider_policy_snapshot(connection, &self.path, self.identity.store_id)
        })
    }

    /// Reads durable provider observation history without mutating the store.
    pub fn read_provider_observation_history(
        &self,
        provider_endpoint: &crate::runtime_identity::ProviderEndpointKey,
        limit: u64,
    ) -> Result<Vec<ProviderObservationHistoryEntry>, RuntimeStoreError> {
        self.with_connection(|connection| {
            policy::read_provider_observation_history(
                connection,
                &self.path,
                self.identity.store_id,
                provider_endpoint,
                limit,
            )
        })
    }

    /// Reads one unexpired session affinity without implicit pruning.
    pub fn get_session_affinity(
        &self,
        session_id: &str,
        now_unix_ms: u64,
        ttl_ms: u64,
    ) -> Result<Option<SessionAffinityRecord>, RuntimeStoreError> {
        self.with_connection(|connection| {
            affinity::get_session_affinity(
                connection,
                &self.path,
                self.identity.store_id,
                session_id,
                now_unix_ms,
                ttl_ms,
            )
        })
    }

    /// Lists unexpired session affinities without implicit pruning.
    pub fn list_session_affinities(
        &self,
        now_unix_ms: u64,
        ttl_ms: u64,
    ) -> Result<Vec<SessionAffinityRecord>, RuntimeStoreError> {
        self.with_connection(|connection| {
            affinity::list_session_affinities(
                connection,
                &self.path,
                self.identity.store_id,
                now_unix_ms,
                ttl_ms,
            )
        })
    }

    /// Returns the durable affinity row count without pruning.
    pub fn count_session_affinities(&self) -> Result<u64, RuntimeStoreError> {
        self.with_connection(|connection| {
            affinity::count_session_affinities(connection, &self.path, self.identity.store_id)
        })
    }

    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, RuntimeStoreError>,
    ) -> Result<T, RuntimeStoreError> {
        let mut reader = self
            .connection
            .lock()
            .map_err(|_| RuntimeStoreError::ConnectionPoisoned)?;
        if reader.poisoned {
            return Err(RuntimeStoreError::PersistentFileIdentityPoisoned);
        }
        if let Err(error) =
            ensure_runtime_file_identity(&self.path, "database", self.opened_database_identity)
        {
            reader.poisoned = true;
            return Err(error);
        }
        let result = operation(&reader.connection);
        if let Err(error) =
            ensure_runtime_file_identity(&self.path, "database", self.opened_database_identity)
        {
            reader.poisoned = true;
            return Err(error);
        }
        result
    }
}

fn require_store_handle(
    expected: Uuid,
    actual: Uuid,
    entity: &'static str,
    id: impl std::fmt::Display,
) -> Result<(), RuntimeStoreError> {
    if expected != actual {
        return Err(RuntimeStoreError::ForeignStoreHandle {
            entity,
            id: id.to_string(),
            expected,
            actual,
        });
    }
    Ok(())
}

fn validate_connection_file_identity(
    connection: &mut RuntimeStoreConnection,
    backing: &StoreBacking,
) -> Result<(), RuntimeStoreError> {
    if connection.poisoned {
        return Err(RuntimeStoreError::PersistentFileIdentityPoisoned);
    }
    let Some(persistent_files) = connection.persistent_files.as_mut() else {
        return Ok(());
    };
    if let Err(error) = persistent_files.validate(backing) {
        connection.poisoned = true;
        return Err(error);
    }
    Ok(())
}

fn validate_open_file_identity(
    persistent_files: &mut Option<PersistentFileSet>,
    backing: &StoreBacking,
) -> Result<(), RuntimeStoreError> {
    if let Some(files) = persistent_files.as_mut() {
        files.validate(backing)?;
    }
    Ok(())
}

fn run_startup_recovery(
    connection: &mut Connection,
    backing: &StoreBacking,
    mut persistent_files: Option<&mut PersistentFileSet>,
    path: &Path,
    store_id: Uuid,
) -> Result<RecoveryReport, RuntimeStoreError> {
    if let Some(files) = persistent_files.as_deref_mut() {
        files.validate(backing)?;
    }
    let run_id = RecoveryRunId::new();
    let recovered_at_unix_ms = current_unix_time_ms()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| sqlite_error(path, "begin startup recovery transaction", source))?;
    let report = lifecycle::LifecycleTransaction::new(&transaction, path, store_id)
        .recover_startup(run_id, recovered_at_unix_ms)?;
    transaction
        .commit()
        .map_err(|source| sqlite_error(path, "commit startup recovery transaction", source))?;
    if let Some(files) = persistent_files {
        files.validate(backing)?;
    }
    Ok(report)
}

fn current_unix_time_ms() -> Result<u64, RuntimeStoreError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RuntimeStoreError::SystemTimeBeforeUnixEpoch)?;
    u64::try_from(duration.as_millis()).map_err(|_| RuntimeStoreError::SystemTimeOverflow)
}

impl WriterLease {
    fn acquire(database_path: &Path) -> Result<Self, RuntimeStoreError> {
        let lease_path = writer_lease_path(database_path);
        #[cfg(unix)]
        let directory_lock = lock_writer_directory(database_path, &lease_path)?;
        let file = open_writer_lease_file(&lease_path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(RuntimeStoreError::WriterAlreadyOwned {
                    path: database_path.to_path_buf(),
                });
            }
            Err(TryLockError::Error(source)) => {
                return Err(RuntimeStoreError::AcquireWriterLease {
                    path: lease_path,
                    source,
                });
            }
        }

        Ok(Self {
            path: lease_path,
            file,
            #[cfg(unix)]
            _directory_lock: directory_lock,
        })
    }
}

#[cfg(unix)]
fn lock_writer_directory(
    database_path: &Path,
    lease_path: &Path,
) -> Result<File, RuntimeStoreError> {
    let directory_path =
        database_path
            .parent()
            .ok_or_else(|| RuntimeStoreError::OpenWriterLease {
                path: lease_path.to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "runtime store path has no parent directory",
                ),
            })?;
    let directory =
        File::open(directory_path).map_err(|source| RuntimeStoreError::OpenWriterLease {
            path: directory_path.to_path_buf(),
            source,
        })?;
    match directory.try_lock() {
        Ok(()) => Ok(directory),
        Err(TryLockError::WouldBlock) => Err(RuntimeStoreError::WriterAlreadyOwned {
            path: database_path.to_path_buf(),
        }),
        Err(TryLockError::Error(source)) => Err(RuntimeStoreError::AcquireWriterLease {
            path: directory_path.to_path_buf(),
            source,
        }),
    }
}

fn open_writer_lease_file(path: &Path) -> Result<File, RuntimeStoreError> {
    let mut create_options = OpenOptions::new();
    create_options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        create_options.mode(0o600);
    }

    match create_options.open(path) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            validate_writer_lease_path(path)?;
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .truncate(false)
                .open(path)
                .map_err(|source| RuntimeStoreError::OpenWriterLease {
                    path: path.to_path_buf(),
                    source,
                })?;
            validate_writer_lease_path(path)?;
            if !file
                .metadata()
                .map_err(|source| RuntimeStoreError::OpenWriterLease {
                    path: path.to_path_buf(),
                    source,
                })?
                .is_file()
            {
                return Err(unsafe_writer_lease(
                    path,
                    "opened handle is not a regular file",
                ));
            }
            Ok(file)
        }
        Err(source) => Err(RuntimeStoreError::OpenWriterLease {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn validate_writer_lease_path(path: &Path) -> Result<(), RuntimeStoreError> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|source| RuntimeStoreError::OpenWriterLease {
            path: path.to_path_buf(),
            source,
        })?;
    if metadata.file_type().is_symlink() {
        return Err(unsafe_writer_lease(path, "symbolic links are not allowed"));
    }
    if !metadata.is_file() {
        return Err(unsafe_writer_lease(
            path,
            "lease path is not a regular file",
        ));
    }
    if metadata_has_multiple_links(&metadata) {
        return Err(unsafe_writer_lease(
            path,
            "hard-linked lease files are not allowed",
        ));
    }
    Ok(())
}

fn unsafe_writer_lease(path: &Path, detail: impl Into<String>) -> RuntimeStoreError {
    RuntimeStoreError::UnsafeWriterLease {
        path: path.to_path_buf(),
        detail: detail.into(),
    }
}

fn validate_database_path_if_present(
    path: &Path,
) -> Result<Option<FileIdentity>, RuntimeStoreError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(RuntimeStoreError::InspectDatabasePath {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let detail = if metadata.file_type().is_symlink() {
        Some("symbolic links are not allowed")
    } else if !metadata.is_file() {
        Some("database path is not a regular file")
    } else if metadata_has_multiple_links(&metadata) {
        Some("hard-linked databases are not allowed")
    } else {
        None
    };
    if let Some(detail) = detail {
        return Err(RuntimeStoreError::UnsafeDatabasePath {
            path: path.to_path_buf(),
            detail: detail.to_string(),
        });
    }
    FileIdentity::from_metadata(&metadata, "database", path).map(Some)
}

fn probe_existing_database(path: &Path) -> Result<(), RuntimeStoreError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let connection = Connection::open_with_flags(path, flags).map_err(|source| {
        probe_ownership_error(
            path,
            RuntimeStoreError::OpenDatabase {
                path: path.to_path_buf(),
                source,
            },
        )
    })?;
    configure_connection_basics(&connection, path)?;
    connection
        .busy_timeout(Duration::ZERO)
        .map_err(|source| sqlite_error(path, "disable ownership probe wait", source))?;
    let result = (|| {
        if inspect_database_state(&connection, path)? == DatabaseState::Initialized {
            read_identity(&connection, path)?;
        }
        Ok(())
    })();
    result.map_err(|error| probe_ownership_error(path, error))
}

fn probe_ownership_error(path: &Path, error: RuntimeStoreError) -> RuntimeStoreError {
    let source = match &error {
        RuntimeStoreError::OpenDatabase { source, .. }
        | RuntimeStoreError::Sqlite { source, .. }
        | RuntimeStoreError::CorruptDatabase { source, .. } => Some(source),
        _ => None,
    };
    if source.is_some_and(sqlite_error_is_busy) {
        RuntimeStoreError::WriterAlreadyOwned {
            path: path.to_path_buf(),
        }
    } else {
        error
    }
}

fn sqlite_error_is_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(error, _)
            if matches!(error.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

#[cfg(unix)]
fn metadata_has_multiple_links(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    metadata.nlink() != 1
}

#[cfg(windows)]
fn metadata_has_multiple_links(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    metadata.number_of_links().is_some_and(|links| links != 1)
}

#[cfg(not(any(unix, windows)))]
fn metadata_has_multiple_links(_metadata: &std::fs::Metadata) -> bool {
    false
}

fn prepare_state_directory(path: &Path) -> Result<(), RuntimeStoreError> {
    std::fs::create_dir_all(path).map_err(|source| RuntimeStoreError::CreateDirectory {
        path: path.to_path_buf(),
        source,
    })?;
    let metadata =
        std::fs::symlink_metadata(path).map_err(|source| RuntimeStoreError::CreateDirectory {
            path: path.to_path_buf(),
            source,
        })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RuntimeStoreError::CreateDirectory {
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "runtime state path must be a real directory",
            ),
        });
    }
    secure_private_directory(path)
}

#[cfg(unix)]
fn secure_private_directory(path: &Path) -> Result<(), RuntimeStoreError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|source| {
        RuntimeStoreError::SecurePermissions {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(not(unix))]
fn secure_private_directory(_path: &Path) -> Result<(), RuntimeStoreError> {
    Ok(())
}

#[cfg(unix)]
fn secure_private_file(path: &Path) -> Result<(), RuntimeStoreError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|source| {
        RuntimeStoreError::SecurePermissions {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn secure_helper_owned_database_files(path: &Path) -> Result<(), RuntimeStoreError> {
    secure_private_file(path)?;
    secure_private_file(&writer_lease_path(path))?;
    for suffix in ["-wal", "-shm"] {
        let sidecar = database_sidecar_path(path, suffix);
        if sidecar.exists() {
            secure_private_file(&sidecar)?;
        }
    }
    Ok(())
}

fn database_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    PathBuf::from(sidecar)
}

#[cfg(not(unix))]
fn secure_private_file(_path: &Path) -> Result<(), RuntimeStoreError> {
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DatabaseState {
    Uninitialized,
    MigrationRequired,
    Initialized,
}

fn configure_connection_basics(
    connection: &Connection,
    path: &Path,
) -> Result<(), RuntimeStoreError> {
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(|source| sqlite_error(path, "set busy timeout", source))?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(|source| sqlite_error(path, "enable foreign keys", source))?;
    Ok(())
}

fn inspect_database_state(
    connection: &Connection,
    path: &Path,
) -> Result<DatabaseState, RuntimeStoreError> {
    let quick_check: String = connection
        .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
        .map_err(|source| sqlite_error(path, "check database integrity", source))?;
    if !quick_check.eq_ignore_ascii_case("ok") {
        return Err(RuntimeStoreError::IntegrityCheckFailed {
            path: path.to_path_buf(),
            detail: quick_check,
        });
    }

    let application_id = pragma_i32(connection, path, "application_id")?;
    let schema_revision = pragma_i32(connection, path, "user_version")?;
    if application_id != 0 && application_id != APPLICATION_ID {
        return Err(RuntimeStoreError::ForeignApplication {
            path: path.to_path_buf(),
            expected: APPLICATION_ID,
            actual: application_id,
        });
    }
    if application_id == APPLICATION_ID
        && schema_revision != SCHEMA_REVISION
        && schema_revision != FIRST_MIGRATABLE_SCHEMA_REVISION
    {
        return Err(RuntimeStoreError::UnsupportedSchemaRevision {
            path: path.to_path_buf(),
            expected: SCHEMA_REVISION,
            actual: schema_revision,
        });
    }
    if application_id == 0 && schema_revision != 0 {
        return Err(RuntimeStoreError::ForeignApplication {
            path: path.to_path_buf(),
            expected: APPLICATION_ID,
            actual: application_id,
        });
    }

    if application_id == APPLICATION_ID {
        if schema_revision == FIRST_MIGRATABLE_SCHEMA_REVISION {
            validate_revision_one_schema(connection, path)?;
            return Ok(DatabaseState::MigrationRequired);
        }
        validate_schema(connection, path)?;
        return Ok(DatabaseState::Initialized);
    }

    let object_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(|source| sqlite_error(path, "inspect schema objects", source))?;
    if object_count == 0 {
        Ok(DatabaseState::Uninitialized)
    } else {
        Err(RuntimeStoreError::UnidentifiedNonemptyDatabase {
            path: path.to_path_buf(),
        })
    }
}

fn initialize_database(
    connection: &mut Connection,
    path: &Path,
) -> Result<RuntimeStoreIdentity, RuntimeStoreError> {
    let store_id = Uuid::new_v4();
    let schema_sql = lifecycle::schema_sql();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| sqlite_error(path, "begin schema transaction", source))?;
    transaction
        .execute_batch(&schema_sql)
        .map_err(|source| sqlite_error(path, "create metadata schema", source))?;
    transaction
        .execute(
            "INSERT INTO store_meta (
                singleton,
                application,
                schema,
                schema_revision,
                store_id
            ) VALUES (1, ?1, ?2, ?3, ?4)",
            params![
                APPLICATION_NAME,
                SCHEMA_NAME,
                SCHEMA_REVISION,
                store_id.to_string()
            ],
        )
        .map_err(|source| sqlite_error(path, "write metadata identity", source))?;
    transaction
        .pragma_update(None, "application_id", APPLICATION_ID)
        .map_err(|source| sqlite_error(path, "set application id", source))?;
    transaction
        .pragma_update(None, "user_version", SCHEMA_REVISION)
        .map_err(|source| sqlite_error(path, "set schema revision", source))?;
    transaction
        .commit()
        .map_err(|source| sqlite_error(path, "commit schema transaction", source))?;

    match inspect_database_state(connection, path)? {
        DatabaseState::Initialized => read_identity(connection, path),
        DatabaseState::MigrationRequired => Err(invalid_metadata(
            path,
            "new runtime schema unexpectedly requires migration",
        )),
        DatabaseState::Uninitialized => Err(invalid_metadata(
            path,
            format!("schema transaction did not persist store identity {store_id}"),
        )),
    }
}

fn migrate_database(
    connection: &mut Connection,
    path: &Path,
) -> Result<RuntimeStoreIdentity, RuntimeStoreError> {
    migrate_database_with_sql(connection, path, &metadata::migration_sql())
}

fn migrate_database_with_sql(
    connection: &mut Connection,
    path: &Path,
    migration_sql: &str,
) -> Result<RuntimeStoreIdentity, RuntimeStoreError> {
    validate_revision_one_schema(connection, path)?;
    let identity = read_identity_for_revision(connection, path, FIRST_MIGRATABLE_SCHEMA_REVISION)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| sqlite_error(path, "begin runtime schema migration", source))?;
    transaction
        .execute_batch(migration_sql)
        .map_err(|source| sqlite_error(path, "create runtime metadata tables", source))?;
    transaction
        .execute(
            "UPDATE store_meta SET schema_revision = ?1 WHERE singleton = 1 AND schema_revision = ?2",
            params![SCHEMA_REVISION, FIRST_MIGRATABLE_SCHEMA_REVISION],
        )
        .map_err(|source| sqlite_error(path, "update runtime schema identity", source))?;
    if transaction.changes() != 1 {
        return Err(invalid_metadata(
            path,
            "runtime schema identity changed before migration commit",
        ));
    }
    transaction
        .pragma_update(None, "user_version", SCHEMA_REVISION)
        .map_err(|source| sqlite_error(path, "update runtime schema revision", source))?;
    transaction
        .commit()
        .map_err(|source| sqlite_error(path, "commit runtime schema migration", source))?;

    match inspect_database_state(connection, path)? {
        DatabaseState::Initialized => {
            let migrated = read_identity(connection, path)?;
            if migrated != identity {
                return Err(invalid_metadata(
                    path,
                    "runtime store identity changed during schema migration",
                ));
            }
            Ok(migrated)
        }
        DatabaseState::MigrationRequired | DatabaseState::Uninitialized => Err(invalid_metadata(
            path,
            "runtime schema migration did not publish revision two",
        )),
    }
}

fn read_identity(
    connection: &Connection,
    path: &Path,
) -> Result<RuntimeStoreIdentity, RuntimeStoreError> {
    read_identity_for_revision(connection, path, SCHEMA_REVISION)
}

fn read_identity_for_revision(
    connection: &Connection,
    path: &Path,
    expected_schema_revision: i32,
) -> Result<RuntimeStoreIdentity, RuntimeStoreError> {
    let row_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM store_meta", [], |row| row.get(0))
        .map_err(|source| sqlite_error(path, "count metadata identity rows", source))?;
    if row_count != 1 {
        return Err(invalid_metadata(
            path,
            format!("store_meta contains {row_count} rows, expected exactly one"),
        ));
    }

    let (singleton, application, schema, schema_revision, store_id) = connection
        .query_row(
            "SELECT singleton, application, schema, schema_revision, store_id FROM store_meta",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i32>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .map_err(|source| match source {
            rusqlite::Error::QueryReturnedNoRows => {
                invalid_metadata(path, "store_meta identity row is missing")
            }
            source => sqlite_error(path, "read metadata identity", source),
        })?;
    if singleton != 1 {
        return Err(invalid_metadata(
            path,
            format!("singleton is {singleton}, expected 1"),
        ));
    }
    if application != APPLICATION_NAME {
        return Err(invalid_metadata(
            path,
            format!("application is {application:?}, expected {APPLICATION_NAME:?}"),
        ));
    }
    if schema != SCHEMA_NAME {
        return Err(invalid_metadata(
            path,
            format!("schema is {schema:?}, expected {SCHEMA_NAME:?}"),
        ));
    }
    if schema_revision != expected_schema_revision {
        return Err(RuntimeStoreError::UnsupportedSchemaRevision {
            path: path.to_path_buf(),
            expected: expected_schema_revision,
            actual: schema_revision,
        });
    }
    let store_id = store_id
        .parse::<Uuid>()
        .map_err(|_| invalid_metadata(path, "store_id is not a UUID"))?;

    Ok(RuntimeStoreIdentity { store_id })
}

fn validate_schema(connection: &Connection, path: &Path) -> Result<(), RuntimeStoreError> {
    lifecycle::validate_expected_schema_objects(connection, path)?;
    validate_foreign_keys(connection, path)
}

fn validate_revision_one_schema(
    connection: &Connection,
    path: &Path,
) -> Result<(), RuntimeStoreError> {
    lifecycle::validate_revision_one_schema_objects(connection, path)?;
    read_identity_for_revision(connection, path, FIRST_MIGRATABLE_SCHEMA_REVISION)?;
    validate_foreign_keys(connection, path)
}

fn validate_foreign_keys(connection: &Connection, path: &Path) -> Result<(), RuntimeStoreError> {
    let mut statement = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(|source| sqlite_error(path, "prepare foreign key check", source))?;
    if statement
        .exists([])
        .map_err(|source| sqlite_error(path, "check foreign key integrity", source))?
    {
        return Err(RuntimeStoreError::IntegrityCheckFailed {
            path: path.to_path_buf(),
            detail: "foreign key violation".to_string(),
        });
    }
    Ok(())
}

fn configure_durability(
    connection: &Connection,
    path: &Path,
    persistent: bool,
) -> Result<(), RuntimeStoreError> {
    let journal_mode: String = if persistent {
        connection
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .map_err(|source| sqlite_error(path, "enable WAL journal mode", source))?
    } else {
        connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(|source| sqlite_error(path, "read journal mode", source))?
    };
    let expected_journal_mode = if persistent { "wal" } else { "memory" };
    if !journal_mode.eq_ignore_ascii_case(expected_journal_mode) {
        return Err(RuntimeStoreError::UnsupportedSetting {
            path: path.to_path_buf(),
            setting: "journal_mode",
            actual: journal_mode,
        });
    }
    if persistent {
        let locking_mode: String = connection
            .query_row("PRAGMA locking_mode = NORMAL", [], |row| row.get(0))
            .map_err(|source| sqlite_error(path, "enable shared-reader locking mode", source))?;
        if !locking_mode.eq_ignore_ascii_case("normal") {
            return Err(RuntimeStoreError::UnsupportedSetting {
                path: path.to_path_buf(),
                setting: "locking_mode",
                actual: locking_mode,
            });
        }
    }

    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(|source| sqlite_error(path, "enable full synchronous mode", source))?;
    let synchronous = pragma_i32(connection, path, "synchronous")?;
    if synchronous != SQLITE_SYNCHRONOUS_FULL {
        return Err(RuntimeStoreError::UnsupportedSetting {
            path: path.to_path_buf(),
            setting: "synchronous",
            actual: synchronous.to_string(),
        });
    }
    let foreign_keys = pragma_i32(connection, path, "foreign_keys")?;
    if foreign_keys != 1 {
        return Err(RuntimeStoreError::UnsupportedSetting {
            path: path.to_path_buf(),
            setting: "foreign_keys",
            actual: foreign_keys.to_string(),
        });
    }
    let busy_timeout_ms = pragma_i32(connection, path, "busy_timeout")?;
    let expected_busy_timeout_ms = i32::try_from(BUSY_TIMEOUT.as_millis()).unwrap_or(i32::MAX);
    if busy_timeout_ms != expected_busy_timeout_ms {
        return Err(RuntimeStoreError::UnsupportedSetting {
            path: path.to_path_buf(),
            setting: "busy_timeout",
            actual: busy_timeout_ms.to_string(),
        });
    }

    Ok(())
}

fn pragma_i32(
    connection: &Connection,
    path: &Path,
    pragma: &'static str,
) -> Result<i32, RuntimeStoreError> {
    connection
        .pragma_query_value(None, pragma, |row| row.get(0))
        .map_err(|source| sqlite_error(path, "read SQLite pragma", source))
}

fn invalid_metadata(path: &Path, detail: impl Into<String>) -> RuntimeStoreError {
    RuntimeStoreError::InvalidMetadata {
        path: path.to_path_buf(),
        detail: detail.into(),
    }
}

fn sqlite_error(
    path: &Path,
    operation: &'static str,
    source: rusqlite::Error,
) -> RuntimeStoreError {
    if matches!(
        &source,
        rusqlite::Error::SqliteFailure(error, _)
            if matches!(error.code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
    ) {
        RuntimeStoreError::CorruptDatabase {
            path: path.to_path_buf(),
            source,
        }
    } else {
        RuntimeStoreError::Sqlite {
            path: path.to_path_buf(),
            operation,
            source,
        }
    }
}

fn writer_lease_path(database_path: &Path) -> PathBuf {
    database_path.with_added_extension("writer.lock")
}

/// Returns the canonical runtime database path for the configured helper home.
pub fn runtime_store_path() -> PathBuf {
    runtime_store_path_in(&crate::config::proxy_home_dir())
}

/// Returns the canonical runtime database path within an explicit helper home.
pub fn runtime_store_path_in(helper_home: &Path) -> PathBuf {
    helper_home.join("state").join("state.sqlite")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;
    use std::thread;
    use std::time::Instant;

    use rusqlite::Connection;

    use super::*;

    const EXPECTED_APPLICATION: &str = "codex-helper";
    const EXPECTED_SCHEMA: &str = "canonical-relay-runtime";
    const LOCK_CHILD_PATH_ENV: &str = "CODEX_HELPER_TEST_RUNTIME_STORE_LOCK_CHILD_PATH";
    const LOCK_CHILD_READY_ENV: &str = "CODEX_HELPER_TEST_RUNTIME_STORE_LOCK_CHILD_READY";

    fn create_revision_one_store(home: &Path) -> Uuid {
        let path = runtime_store_path_in(home);
        let store = RuntimeStore::open_in_home(home).expect("create current runtime store");
        let store_id = store.identity().store_id();
        drop(store);

        let connection = Connection::open(&path).expect("open runtime store for downgrade");
        connection
            .execute_batch(
                "DROP TABLE runtime_documents;
                 DROP TABLE runtime_private_keys;
                 UPDATE store_meta SET schema_revision = 1 WHERE singleton = 1;
                 PRAGMA user_version = 1;",
            )
            .expect("downgrade runtime schema fixture to revision one");
        drop(connection);
        store_id
    }

    fn test_logical_terminal_payload() -> LogicalRequestTerminalPayload {
        LogicalRequestTerminalPayload {
            finished_request: crate::state::FinishedRequest {
                id: 1,
                trace_id: None,
                session_id: None,
                session_identity_source: None,
                client_name: None,
                client_addr: None,
                cwd: None,
                model: Some("gpt-5".to_string()),
                reasoning_effort: None,
                service_tier: None,
                provider_id: None,
                route_decision: None,
                usage: None,
                cost: crate::pricing::CostBreakdown::unknown(),
                accounting: Default::default(),
                retry: None,
                provider_signals: Vec::new(),
                policy_actions: Vec::new(),
                observability: crate::state::RequestObservability::default(),
                service: "codex".to_string(),
                method: "POST".to_string(),
                path: "/v1/responses".to_string(),
                status_code: 200,
                duration_ms: 24,
                ttfb_ms: Some(8),
                streaming: false,
                ended_at_ms: 34,
            },
            winning_attempt_id: None,
            runtime_revision: 1,
            runtime_digest: "test-runtime".to_string(),
            policy_revision: None,
            provider_epoch: None,
            provider_price_key: None,
            requested_model: Some("gpt-5".to_string()),
            mapped_model: Some("gpt-5".to_string()),
            reported_model: None,
            pricing_model: Some("gpt-5".to_string()),
            requested_service_tier: None,
            effective_service_tier: None,
            actual_service_tier: None,
            pricing_service_tier: None,
            cache_accounting_convention: crate::usage::CacheAccountingConvention::SEPARATE,
            billable_usage: None,
            accounting_scope: RequestAccountingScope::Economic,
        }
    }

    #[test]
    fn runtime_document_compare_and_write_rejects_stale_revision() {
        let store = RuntimeStore::open_in_memory().expect("open runtime store");
        let first_payload = r#"{"generation":1}"#;
        let first = store
            .compare_and_write_runtime_document(
                None,
                RuntimeDocumentWrite {
                    kind: RuntimeDocumentKind::BasellmCatalog,
                    schema_version: 1,
                    payload_json: first_payload,
                },
            )
            .expect("commit absent document");
        let RuntimeDocumentCommit::Committed(first) = first else {
            panic!("absent document should commit");
        };
        assert_eq!(first.revision, 1);

        let stale = store
            .compare_and_write_runtime_document(
                None,
                RuntimeDocumentWrite {
                    kind: RuntimeDocumentKind::BasellmCatalog,
                    schema_version: 1,
                    payload_json: r#"{"generation":0}"#,
                },
            )
            .expect("reject stale document");
        let RuntimeDocumentCommit::Stale(Some(current)) = stale else {
            panic!("stale absent observation should return the current document");
        };
        assert_eq!(current.revision, 1);
        assert_eq!(current.payload_json, first_payload);

        let second = store
            .compare_and_write_runtime_document(
                Some(first.revision),
                RuntimeDocumentWrite {
                    kind: RuntimeDocumentKind::BasellmCatalog,
                    schema_version: 1,
                    payload_json: r#"{"generation":2}"#,
                },
            )
            .expect("commit matching revision");
        let RuntimeDocumentCommit::Committed(second) = second else {
            panic!("matching revision should commit");
        };
        assert_eq!(second.revision, 2);
    }

    fn test_attempt_evidence() -> AttemptPendingEvidence {
        AttemptPendingEvidence::new(
            1,
            "test-runtime",
            AttemptRouteEvidence {
                provider_endpoint_key: Some("codex/test/0".to_string()),
                provider_id: Some("test-provider".to_string()),
                endpoint_id: Some("0".to_string()),
                route_path: vec!["test".to_string(), "endpoint-0".to_string()],
                upstream_base_url: Some("https://example.test/v1".to_string()),
                mapped_model: Some("gpt-5".to_string()),
            },
        )
        .with_provider_epoch(FrozenProviderEpochIdentity {
            scope: FrozenProviderCatalogScope {
                adapter: crate::provider_catalog::ProviderAdapter::OpenAiCodex,
                endpoint_origin: "https://example.test".to_string(),
                route_scope: "test".to_string(),
                account_fingerprint: "sha256:account-test".to_string(),
                config_revision: "test-runtime".to_string(),
            },
            catalog_revision: Some("catalog-test".to_string()),
            pricing_revision: Some("pricing-test".to_string()),
        })
    }

    fn apply_test_attempt_economics(payload: &mut LogicalRequestTerminalPayload) {
        let evidence = test_attempt_evidence();
        let route = evidence.route;
        payload.provider_epoch = evidence.provider_epoch;
        payload.provider_price_key =
            payload
                .provider_epoch
                .clone()
                .map(|epoch| FrozenProviderPriceKey {
                    epoch,
                    model: "gpt-5".to_string(),
                    tier: crate::provider_catalog::ProviderPricingTier::Standard,
                });
        payload.mapped_model = route.mapped_model.clone();
        payload.pricing_model = route.mapped_model;
        payload.actual_service_tier = Some("standard".to_string());
        payload.pricing_service_tier = Some("standard".to_string());
    }

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "codex-helper-runtime-store-{label}-{}-{}",
                std::process::id(),
                Uuid::new_v4()
            ));
            fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn commit_projection_fixture(
        store: &RuntimeStore,
        begun_at_unix_ms: u64,
        terminal_at_unix_ms: u64,
        mut payload: LogicalRequestTerminalPayload,
    ) -> LogicalRequestId {
        let request = NewLogicalRequest {
            id: LogicalRequestId::new(),
            begun_at_unix_ms,
        };
        payload.finished_request.ended_at_ms = terminal_at_unix_ms;
        let handle = store
            .transaction(|transaction| transaction.begin_logical_request(request))
            .expect("begin projection fixture")
            .handle;
        store
            .transaction(|transaction| {
                transaction.commit_logical_request_terminal(
                    handle,
                    LogicalRequestTerminal {
                        outcome: LogicalRequestOutcome::Succeeded,
                        terminal_at_unix_ms,
                        economics_state: EconomicsState::Unknown,
                        payload: Some(payload),
                    },
                )
            })
            .expect("commit projection fixture");
        request.id
    }

    #[test]
    fn operator_ledger_revision_is_stable_across_reopen_and_tracks_unique_terminals() {
        let home = TestDir::new("operator-ledger-revision");
        let store = RuntimeStore::open_in_home(home.path()).expect("open runtime store");
        let initial = store
            .operator_ledger_revision()
            .expect("read initial operator ledger revision");

        let first_id = commit_projection_fixture(&store, 1, 10, test_logical_terminal_payload());
        let first = store
            .operator_ledger_revision()
            .expect("read first operator ledger revision");
        assert_ne!(
            first, initial,
            "a new runtime terminal must advance revision"
        );

        let first_terminal = store
            .read_logical_request(store.logical_request_handle(first_id))
            .expect("read first logical request")
            .expect("first logical request exists")
            .terminal
            .expect("first logical request is terminal")
            .terminal;
        assert_eq!(
            store
                .transaction(|transaction| {
                    transaction.commit_logical_request_terminal(
                        store.logical_request_handle(first_id),
                        first_terminal,
                    )
                })
                .expect("repeat identical terminal"),
            TerminalDisposition::AlreadyIdentical
        );
        assert_eq!(
            store
                .operator_ledger_revision()
                .expect("read revision after identical terminal"),
            first,
            "an identical terminal must not advance revision"
        );

        drop(store);
        let reopened = RuntimeStore::open_in_home(home.path()).expect("reopen runtime store");
        assert_eq!(
            reopened
                .operator_ledger_revision()
                .expect("read reopened operator ledger revision"),
            first,
            "the same durable store must retain its revision across restart"
        );

        commit_projection_fixture(&reopened, 2, 20, test_logical_terminal_payload());
        assert_ne!(
            reopened
                .operator_ledger_revision()
                .expect("read second operator ledger revision"),
            first,
            "a second unique runtime terminal must advance revision"
        );
    }

    fn projection_filter_payload() -> LogicalRequestTerminalPayload {
        let mut payload = test_logical_terminal_payload();
        payload.finished_request.status_code = 429;
        payload.finished_request.service_tier = Some("priority".to_string());
        payload.finished_request.provider_id = Some("nested-provider".to_string());
        payload.finished_request.route_decision = Some(crate::state::RouteDecisionProvenance {
            provider_id: Some("nested-provider".to_string()),
            endpoint_id: Some("primary".to_string()),
            ..crate::state::RouteDecisionProvenance::default()
        });
        payload.finished_request.retry = Some(crate::logging::RetryInfo {
            attempts: 2,
            route_attempts: vec![crate::logging::RouteAttemptLog {
                attempt_index: 0,
                provider_id: Some("nested-provider".to_string()),
                endpoint_id: Some("primary".to_string()),
                provider_endpoint_key: Some("codex/nested-provider/primary".to_string()),
                decision: "failed_status".to_string(),
                ..crate::logging::RouteAttemptLog::default()
            }],
        });
        payload
    }

    fn projection_filter_signal(
        kind: crate::provider_signals::ProviderSignalKind,
    ) -> crate::provider_signals::ProviderSignal {
        use crate::provider_signals::{ProviderSignal, ProviderSignalSource, ProviderSignalTarget};
        use crate::runtime_identity::ProviderEndpointKey;

        let mut signal = ProviderSignal::high_confidence_route_facing(
            kind,
            ProviderSignalSource::UpstreamResponse,
            ProviderSignalTarget::ProviderEndpoint {
                provider_endpoint_key: ProviderEndpointKey::new(
                    "codex",
                    "nested-provider",
                    "primary",
                ),
            },
            20,
        );
        signal.retry_after_secs = Some(30);
        signal
    }

    fn projection_filter_action(
        signal: &crate::provider_signals::ProviderSignal,
    ) -> crate::policy_actions::PolicyAction {
        crate::policy_actions::PolicyAction::cooldown_from_signal(signal.clone(), 20, 0, 2)
            .expect("rate-limit signal creates cooldown")
    }

    fn projection_filter_retry_attempt(
        payload: &mut LogicalRequestTerminalPayload,
    ) -> &mut crate::logging::RouteAttemptLog {
        payload
            .finished_request
            .retry
            .as_mut()
            .and_then(|retry| retry.route_attempts.first_mut())
            .expect("projection fixture has a retry attempt")
    }

    fn assert_projection_filter_skips_newer_decoy(
        matcher: &str,
        filter: CommittedRequestFilter,
        matching: LogicalRequestTerminalPayload,
        mutate_decoy: impl FnOnce(&mut LogicalRequestTerminalPayload),
    ) {
        let store = RuntimeStore::open_in_memory().expect("open store");
        let matching_id = commit_projection_fixture(&store, 1, 30, matching.clone());
        let mut newer_decoy = matching;
        mutate_decoy(&mut newer_decoy);
        let decoy_id = commit_projection_fixture(&store, 2, 40, newer_decoy);

        let newest = store
            .query_committed_requests(&CommittedRequestQuery {
                limit: 1,
                ..CommittedRequestQuery::default()
            })
            .expect("query newest committed request");
        assert_eq!(
            newest.items[0].logical_request_id, decoy_id,
            "{matcher} decoy must be the newest committed request"
        );
        assert_eq!(
            newest.items[0].terminal_at_unix_ms, 40,
            "{matcher} decoy must have the updated terminal time"
        );
        assert_eq!(
            newest.items[0].payload.finished_request.ended_at_ms, 40,
            "{matcher} decoy must have the updated request end time"
        );

        let page = store
            .query_committed_requests(&CommittedRequestQuery {
                limit: 1,
                cursor: None,
                terminal_at_or_after_unix_ms: None,
                filter,
            })
            .expect("query committed request filter");
        assert_eq!(page.items.len(), 1, "{matcher} result count");
        assert_eq!(
            page.items[0].logical_request_id, matching_id,
            "{matcher} must reject only the newer decoy"
        );
        assert_eq!(page.next_cursor, None, "{matcher} filtered cursor");
    }

    #[test]
    fn committed_request_projection_filters_before_limit_and_pages_by_terminal_identity() {
        let store = RuntimeStore::open_in_memory().expect("open store");
        let mut nonmatching = test_logical_terminal_payload();
        nonmatching.finished_request.service = "claude".to_string();
        commit_projection_fixture(&store, 1, 40, nonmatching);

        let mut first = test_logical_terminal_payload();
        first.finished_request.service = "codex".to_string();
        let first_id = commit_projection_fixture(&store, 2, 30, first);
        let mut second = test_logical_terminal_payload();
        second.finished_request.service = "codex".to_string();
        let second_id = commit_projection_fixture(&store, 3, 30, second);
        let mut third = test_logical_terminal_payload();
        third.finished_request.service = "codex".to_string();
        let third_id = commit_projection_fixture(&store, 4, 20, third);

        let query = CommittedRequestQuery {
            limit: 2,
            cursor: None,
            terminal_at_or_after_unix_ms: None,
            filter: CommittedRequestFilter {
                service: Some("codex".to_string()),
                ..CommittedRequestFilter::default()
            },
        };
        let first_page = store
            .query_committed_requests(&query)
            .expect("query first projection page");
        let mut same_time_ids = vec![first_id, second_id];
        same_time_ids.sort_by(|left, right| right.as_uuid().cmp(left.as_uuid()));
        assert_eq!(
            first_page
                .items
                .iter()
                .map(|item| item.logical_request_id)
                .collect::<Vec<_>>(),
            same_time_ids
        );
        assert_eq!(
            first_page.next_cursor,
            Some(CommittedRequestCursor {
                terminal_at_unix_ms: 30,
                logical_request_id: *same_time_ids.last().expect("second same-time request"),
            })
        );

        let second_page = store
            .query_committed_requests(&CommittedRequestQuery {
                cursor: first_page.next_cursor,
                ..query
            })
            .expect("query second projection page");
        assert_eq!(second_page.items.len(), 1);
        assert_eq!(second_page.items[0].logical_request_id, third_id);
        assert_eq!(second_page.items[0].terminal_at_unix_ms, 20);
        assert_eq!(second_page.next_cursor, None);
    }

    #[test]
    fn filtered_projection_cursor_tracks_last_scanned_row_across_deep_decoys() {
        let store = RuntimeStore::open_in_memory().expect("open store");
        let mut matching = test_logical_terminal_payload();
        matching.finished_request.service = "target".to_string();
        let matching_id = commit_projection_fixture(&store, 1, 1, matching);
        for index in 0..1_030_u64 {
            let mut decoy = test_logical_terminal_payload();
            decoy.finished_request.service = "decoy".to_string();
            commit_projection_fixture(&store, index + 2, index + 2, decoy);
        }

        let query = CommittedRequestQuery {
            limit: 1,
            cursor: None,
            terminal_at_or_after_unix_ms: None,
            filter: CommittedRequestFilter {
                service: Some("target".to_string()),
                ..CommittedRequestFilter::default()
            },
        };
        let first_page = store
            .query_committed_requests(&query)
            .expect("scan first bounded projection page");
        assert!(first_page.items.is_empty());
        assert!(first_page.next_cursor.is_some());

        let second_page = store
            .query_committed_requests(&CommittedRequestQuery {
                cursor: first_page.next_cursor,
                ..query
            })
            .expect("continue from the last scanned projection row");
        assert_eq!(second_page.items.len(), 1);
        assert_eq!(second_page.items[0].logical_request_id, matching_id);
        assert_eq!(second_page.next_cursor, None);
    }

    #[test]
    fn exact_identity_projection_reaches_old_match_after_deep_decoys() {
        let store = RuntimeStore::open_in_memory().expect("open store");
        let mut matching = test_logical_terminal_payload();
        matching.finished_request.id = 7;
        matching.finished_request.trace_id = Some("trace-7".to_string());
        matching.finished_request.session_id = Some("sid-1".to_string());
        let matching_id = commit_projection_fixture(&store, 1, 1, matching);
        for index in 0..1_100_u64 {
            let mut decoy = test_logical_terminal_payload();
            decoy.finished_request.id = index + 100;
            decoy.finished_request.trace_id = Some(format!("trace-decoy-{index}"));
            decoy.finished_request.session_id = Some(format!("sid-decoy-{index}"));
            commit_projection_fixture(&store, index + 2, index + 2, decoy);
        }

        let items = store
            .query_committed_request_identities(&CommittedRequestIdentityQuery {
                service: "codex".to_string(),
                trace_id: Some("trace-7".to_string()),
                request_id: Some(7),
                session_id: Some("sid-1".to_string()),
                limit: 2,
            })
            .expect("query old exact identity through typed indexes");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].logical_request_id, matching_id);
    }

    #[test]
    fn committed_request_projection_unfiltered_cursor_pages_without_gaps_or_duplicates() {
        let store = RuntimeStore::open_in_memory().expect("open store");
        let mut expected = vec![
            (
                commit_projection_fixture(&store, 1, 40, test_logical_terminal_payload()),
                40,
            ),
            (
                commit_projection_fixture(&store, 2, 30, test_logical_terminal_payload()),
                30,
            ),
            (
                commit_projection_fixture(&store, 3, 30, test_logical_terminal_payload()),
                30,
            ),
            (
                commit_projection_fixture(&store, 4, 20, test_logical_terminal_payload()),
                20,
            ),
        ];
        expected.sort_by(|(left_id, left_time), (right_id, right_time)| {
            right_time
                .cmp(left_time)
                .then_with(|| right_id.as_uuid().cmp(left_id.as_uuid()))
        });

        let first_page = store
            .query_committed_requests(&CommittedRequestQuery {
                limit: 2,
                ..CommittedRequestQuery::default()
            })
            .expect("query first unfiltered projection page");
        let second_page = store
            .query_committed_requests(&CommittedRequestQuery {
                limit: 2,
                cursor: first_page.next_cursor,
                ..CommittedRequestQuery::default()
            })
            .expect("query second unfiltered projection page");

        assert!(first_page.next_cursor.is_some());
        assert_eq!(second_page.next_cursor, None);
        assert_eq!(
            first_page
                .items
                .iter()
                .chain(&second_page.items)
                .map(|item| (item.logical_request_id, item.terminal_at_unix_ms))
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn committed_request_projection_rejects_limit_without_lookahead_capacity() {
        let store = RuntimeStore::open_in_memory().expect("open store");

        let error = store
            .query_committed_requests(&CommittedRequestQuery {
                limit: usize::MAX,
                ..CommittedRequestQuery::default()
            })
            .expect_err("lookahead limit must not wrap or saturate");

        assert!(matches!(
            error,
            RuntimeStoreError::InvariantViolation { detail, .. }
                if detail == "limit exceeds SQLite integer range"
        ));
    }

    #[test]
    fn committed_request_projection_filters_canonical_provider_without_fallback() {
        assert_projection_filter_skips_newer_decoy(
            "provider",
            CommittedRequestFilter {
                provider: Some("nested-provider".to_string()),
                ..CommittedRequestFilter::default()
            },
            projection_filter_payload(),
            |payload| {
                payload
                    .finished_request
                    .route_decision
                    .as_mut()
                    .expect("projection fixture has final route identity")
                    .provider_id = Some("other-provider".to_string());
            },
        );
    }

    #[test]
    fn committed_request_projection_filters_typed_provider_endpoint_without_fallback() {
        use crate::runtime_identity::ProviderEndpointKey;

        assert_projection_filter_skips_newer_decoy(
            "provider endpoint",
            CommittedRequestFilter {
                provider_endpoint: Some(ProviderEndpointKey::new(
                    "codex",
                    "nested-provider",
                    "primary",
                )),
                ..CommittedRequestFilter::default()
            },
            projection_filter_payload(),
            |payload| {
                payload
                    .finished_request
                    .route_decision
                    .as_mut()
                    .expect("projection fixture has final route identity")
                    .endpoint_id = Some("secondary".to_string());
            },
        );
    }

    #[test]
    fn committed_request_projection_filters_terminal_signal_kind_before_limit() {
        use crate::provider_signals::ProviderSignalKind;

        let mut matching = projection_filter_payload();
        matching.finished_request.provider_signals =
            vec![projection_filter_signal(ProviderSignalKind::RateLimit)];
        assert_projection_filter_skips_newer_decoy(
            "terminal signal kind",
            CommittedRequestFilter {
                signal_kind: Some("RATE_LIMIT".to_string()),
                ..CommittedRequestFilter::default()
            },
            matching,
            |payload| {
                let signal = payload
                    .finished_request
                    .provider_signals
                    .first_mut()
                    .expect("projection fixture has a terminal signal");
                signal.kind = ProviderSignalKind::Capacity;
                signal.code = Some(ProviderSignalKind::Capacity.code().to_string());
            },
        );
    }

    #[test]
    fn committed_request_projection_filters_retry_attempt_signal_kind_before_limit() {
        use crate::provider_signals::ProviderSignalKind;

        let mut matching = projection_filter_payload();
        projection_filter_retry_attempt(&mut matching).provider_signals =
            vec![projection_filter_signal(ProviderSignalKind::RateLimit)];
        assert_projection_filter_skips_newer_decoy(
            "retry-attempt signal kind",
            CommittedRequestFilter {
                signal_kind: Some("rate_limit".to_string()),
                ..CommittedRequestFilter::default()
            },
            matching,
            |payload| {
                let signal = projection_filter_retry_attempt(payload)
                    .provider_signals
                    .first_mut()
                    .expect("projection fixture has a retry-attempt signal");
                signal.kind = ProviderSignalKind::Capacity;
                signal.code = Some(ProviderSignalKind::Capacity.code().to_string());
            },
        );
    }

    #[test]
    fn committed_request_projection_filters_terminal_policy_action_kind_before_limit() {
        use crate::policy_actions::PolicyActionKind;
        use crate::provider_signals::ProviderSignalKind;

        let signal = projection_filter_signal(ProviderSignalKind::RateLimit);
        let mut matching = projection_filter_payload();
        matching.finished_request.policy_actions = vec![projection_filter_action(&signal)];
        assert_projection_filter_skips_newer_decoy(
            "terminal policy-action kind",
            CommittedRequestFilter {
                policy_action_kind: Some("COOLDOWN".to_string()),
                ..CommittedRequestFilter::default()
            },
            matching,
            |payload| {
                let action = payload
                    .finished_request
                    .policy_actions
                    .first_mut()
                    .expect("projection fixture has a terminal policy action");
                action.kind = PolicyActionKind::Unknown;
                action.code = Some(PolicyActionKind::Unknown.code().to_string());
            },
        );
    }

    #[test]
    fn committed_request_projection_filters_retry_attempt_policy_action_kind_before_limit() {
        use crate::policy_actions::PolicyActionKind;
        use crate::provider_signals::ProviderSignalKind;

        let signal = projection_filter_signal(ProviderSignalKind::RateLimit);
        let mut matching = projection_filter_payload();
        projection_filter_retry_attempt(&mut matching).policy_actions =
            vec![projection_filter_action(&signal)];
        assert_projection_filter_skips_newer_decoy(
            "retry-attempt policy-action kind",
            CommittedRequestFilter {
                policy_action_kind: Some("cooldown".to_string()),
                ..CommittedRequestFilter::default()
            },
            matching,
            |payload| {
                let action = projection_filter_retry_attempt(payload)
                    .policy_actions
                    .first_mut()
                    .expect("projection fixture has a retry-attempt policy action");
                action.kind = PolicyActionKind::Unknown;
                action.code = Some(PolicyActionKind::Unknown.code().to_string());
            },
        );
    }

    #[test]
    fn committed_request_projection_filters_status_minimum_before_limit() {
        assert_projection_filter_skips_newer_decoy(
            "minimum status",
            CommittedRequestFilter {
                status_min: Some(429),
                ..CommittedRequestFilter::default()
            },
            projection_filter_payload(),
            |payload| payload.finished_request.status_code = 428,
        );
    }

    #[test]
    fn committed_request_projection_filters_status_maximum_before_limit() {
        assert_projection_filter_skips_newer_decoy(
            "maximum status",
            CommittedRequestFilter {
                status_max: Some(429),
                ..CommittedRequestFilter::default()
            },
            projection_filter_payload(),
            |payload| payload.finished_request.status_code = 430,
        );
    }

    #[test]
    fn committed_request_projection_filters_fast_mode_before_limit() {
        assert_projection_filter_skips_newer_decoy(
            "fast mode",
            CommittedRequestFilter {
                fast: true,
                ..CommittedRequestFilter::default()
            },
            projection_filter_payload(),
            |payload| payload.finished_request.service_tier = Some("standard".to_string()),
        );
    }

    #[test]
    fn committed_request_projection_filters_retried_requests_before_limit() {
        assert_projection_filter_skips_newer_decoy(
            "retried request",
            CommittedRequestFilter {
                retried: true,
                ..CommittedRequestFilter::default()
            },
            projection_filter_payload(),
            |payload| {
                payload
                    .finished_request
                    .retry
                    .as_mut()
                    .expect("projection fixture has retry metadata")
                    .attempts = 1;
            },
        );
    }

    #[test]
    fn committed_request_projection_round_trips_typed_accounting_scope_across_reopen() {
        let home = TestDir::new("projection-accounting-scope");
        let store = RuntimeStore::open_in_home(home.path()).expect("create store");
        let economic_id = commit_projection_fixture(&store, 1, 10, {
            let mut payload = test_logical_terminal_payload();
            payload.accounting_scope = RequestAccountingScope::Economic;
            payload
        });
        let non_economic_id = commit_projection_fixture(&store, 2, 20, {
            let mut payload = test_logical_terminal_payload();
            payload.accounting_scope = RequestAccountingScope::NonEconomic;
            payload
        });
        drop(store);

        let reopened = RuntimeStore::open_in_home(home.path()).expect("reopen store");
        let page = reopened
            .query_committed_requests(&CommittedRequestQuery {
                limit: 10,
                ..CommittedRequestQuery::default()
            })
            .expect("read reopened projection");
        assert_eq!(
            page.items
                .iter()
                .map(|item| (item.logical_request_id, item.payload.accounting_scope))
                .collect::<Vec<_>>(),
            vec![
                (non_economic_id, RequestAccountingScope::NonEconomic),
                (economic_id, RequestAccountingScope::Economic),
            ]
        );
    }

    #[test]
    fn read_only_runtime_store_reader_coexists_with_writer_without_recovery() {
        let home = TestDir::new("read-only-reader");
        let writer = RuntimeStore::open_in_home(home.path()).expect("open writer");
        commit_projection_fixture(&writer, 1, 10, test_logical_terminal_payload());
        let recovery_before = writer
            .latest_recovery_report()
            .expect("read writer recovery report");

        let reader = RuntimeStoreReader::open_in_home(home.path()).expect("open read-only reader");

        assert_eq!(reader.identity(), writer.identity());
        assert_eq!(
            reader
                .latest_recovery_report()
                .expect("read reader recovery report"),
            recovery_before
        );
        assert_eq!(
            reader
                .query_committed_requests(&CommittedRequestQuery {
                    limit: 10,
                    ..CommittedRequestQuery::default()
                })
                .expect("query through read-only reader")
                .items
                .len(),
            1
        );
        writer
            .transaction(|_| Ok(()))
            .expect("writer remains usable while reader is open");
    }

    #[test]
    fn read_only_runtime_store_reader_does_not_create_missing_state() {
        let home = TestDir::new("read-only-reader-missing");
        let path = runtime_store_path_in(home.path());

        let error = RuntimeStoreReader::open_in_home(home.path())
            .expect_err("read-only access requires an existing store");

        assert!(matches!(error, RuntimeStoreError::DatabaseMissing { .. }));
        assert!(!path.exists());
        assert!(!path.parent().expect("state directory parent").exists());
    }

    #[test]
    fn non_economic_terminal_rejects_billable_usage_and_known_cost() {
        let store = RuntimeStore::open_in_memory().expect("open store");
        let request = NewLogicalRequest {
            id: LogicalRequestId::new(),
            begun_at_unix_ms: 1,
        };
        let handle = store
            .transaction(|transaction| transaction.begin_logical_request(request))
            .expect("begin request")
            .handle;

        let mut with_billable_usage = test_logical_terminal_payload();
        with_billable_usage.accounting_scope = RequestAccountingScope::NonEconomic;
        with_billable_usage.billable_usage = Some(crate::usage::CanonicalUsageBuckets::default());
        let mut with_known_cost = test_logical_terminal_payload();
        with_known_cost.accounting_scope = RequestAccountingScope::NonEconomic;
        with_known_cost.finished_request.cost.confidence = crate::pricing::CostConfidence::Exact;

        for payload in [with_billable_usage, with_known_cost] {
            let error = store
                .transaction(|transaction| {
                    transaction.commit_logical_request_terminal(
                        handle,
                        LogicalRequestTerminal {
                            outcome: LogicalRequestOutcome::Succeeded,
                            terminal_at_unix_ms: 10,
                            economics_state: EconomicsState::Unknown,
                            payload: Some(payload),
                        },
                    )
                })
                .expect_err("reject invalid non-economic terminal");
            assert!(matches!(
                error,
                RuntimeStoreError::InvariantViolation { .. }
            ));
        }
    }

    #[test]
    fn committed_request_projection_limit_zero_is_empty_without_cursor() {
        let store = RuntimeStore::open_in_memory().expect("open store");
        commit_projection_fixture(&store, 1, 10, test_logical_terminal_payload());

        let page = store
            .query_committed_requests(&CommittedRequestQuery {
                limit: 0,
                ..CommittedRequestQuery::default()
            })
            .expect("query zero-size page");

        assert!(page.items.is_empty());
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn persistent_store_creates_and_reopens_with_stable_identity() {
        let home = TestDir::new("reopen");
        let lexical_path = home.path().join("state").join("state.sqlite");

        let first = RuntimeStore::open_in_home(home.path()).expect("create store");
        let expected_path = fs::canonicalize(lexical_path.parent().expect("database parent"))
            .expect("canonical state directory")
            .join("state.sqlite");
        let store_id = first.identity().store_id();
        assert_eq!(first.path(), Some(expected_path.as_path()));
        assert_eq!(first.identity().application(), EXPECTED_APPLICATION);
        assert_eq!(first.identity().schema(), EXPECTED_SCHEMA);
        assert_eq!(first.identity().schema_revision(), SCHEMA_REVISION);
        drop(first);

        let reopened = RuntimeStore::open_in_home(home.path()).expect("reopen store");
        assert_eq!(reopened.identity().store_id(), store_id);
        assert_eq!(reopened.path(), Some(expected_path.as_path()));
    }

    #[test]
    fn persistent_store_migrates_revision_one_atomically() {
        let home = TestDir::new("migrate-revision-one");
        let store_id = create_revision_one_store(home.path());

        let migrated = RuntimeStore::open_in_home(home.path()).expect("migrate runtime store");

        assert_eq!(migrated.identity().store_id(), store_id);
        assert_eq!(migrated.identity().schema_revision(), SCHEMA_REVISION);
        assert!(
            migrated
                .read_runtime_document(RuntimeDocumentKind::BasellmCatalog)
                .expect("read migrated document table")
                .is_none()
        );
        migrated
            .load_or_create_quota_identity()
            .expect("write migrated private-key table");
    }

    #[test]
    fn reader_rejects_revision_one_until_writer_migrates_it() {
        let home = TestDir::new("reader-before-migration");
        create_revision_one_store(home.path());

        let error = RuntimeStoreReader::open_in_home(home.path())
            .expect_err("reader must not observe a partially compatible schema");

        assert!(matches!(
            error,
            RuntimeStoreError::UnsupportedSchemaRevision {
                expected: SCHEMA_REVISION,
                actual: FIRST_MIGRATABLE_SCHEMA_REVISION,
                ..
            }
        ));
    }

    #[test]
    fn failed_revision_one_migration_rolls_back_all_schema_changes() {
        let home = TestDir::new("migration-rollback");
        let path = runtime_store_path_in(home.path());
        create_revision_one_store(home.path());
        let mut connection = Connection::open(&path).expect("open revision-one store");
        configure_connection_basics(&connection, &path).expect("configure migration connection");
        let failing_sql = format!(
            "{}; SELECT * FROM runtime_migration_failure_injection;",
            metadata::RUNTIME_PRIVATE_KEYS_SQL
        );

        migrate_database_with_sql(&mut connection, &path, &failing_sql)
            .expect_err("injected migration must fail");

        assert_eq!(
            pragma_i32(&connection, &path, "user_version").expect("read rolled-back revision"),
            FIRST_MIGRATABLE_SCHEMA_REVISION
        );
        let schema_revision: i32 = connection
            .query_row(
                "SELECT schema_revision FROM store_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("read rolled-back metadata revision");
        assert_eq!(schema_revision, FIRST_MIGRATABLE_SCHEMA_REVISION);
        let metadata_table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table' AND name IN ('runtime_private_keys', 'runtime_documents')",
                [],
                |row| row.get(0),
            )
            .expect("count rolled-back metadata tables");
        assert_eq!(metadata_table_count, 0);
        drop(connection);

        RuntimeStore::open_in_home(home.path()).expect("migrate normally after rollback");
    }

    #[test]
    fn persistent_store_configures_durable_sqlite_settings() {
        let home = TestDir::new("settings");
        let store = RuntimeStore::open_in_home(home.path()).expect("create store");
        assert_eq!(store.startup_recovery_report().recovery_ordinal, 1);

        let connection = store._connection.lock().expect("lock store connection");
        let journal_mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("read journal mode");
        assert!(journal_mode.eq_ignore_ascii_case("wal"));
        let locking_mode: String = connection
            .query_row("PRAGMA locking_mode", [], |row| row.get(0))
            .expect("read locking mode");
        assert!(locking_mode.eq_ignore_ascii_case("normal"));
        assert_eq!(
            pragma_i32(
                &connection,
                store.path().expect("persistent path"),
                "application_id"
            )
            .expect("read application id"),
            APPLICATION_ID
        );
        assert_eq!(
            pragma_i32(
                &connection,
                store.path().expect("persistent path"),
                "user_version"
            )
            .expect("read schema revision"),
            SCHEMA_REVISION
        );
        assert_eq!(
            pragma_i32(
                &connection,
                store.path().expect("persistent path"),
                "synchronous"
            )
            .expect("read synchronous mode"),
            SQLITE_SYNCHRONOUS_FULL
        );
        assert_eq!(
            pragma_i32(
                &connection,
                store.path().expect("persistent path"),
                "foreign_keys"
            )
            .expect("read foreign keys"),
            1
        );
        assert_eq!(
            pragma_i32(
                &connection,
                store.path().expect("persistent path"),
                "busy_timeout"
            )
            .expect("read busy timeout"),
            i32::try_from(BUSY_TIMEOUT.as_millis()).expect("busy timeout fits i32")
        );
    }

    #[test]
    fn in_memory_stores_are_explicit_and_isolated() {
        let first = RuntimeStore::open_in_memory().expect("open first in-memory store");
        let second = RuntimeStore::open_in_memory().expect("open second in-memory store");

        assert_eq!(first.path(), None);
        assert_eq!(second.path(), None);
        let connection = first._connection.lock().expect("lock in-memory connection");
        let journal_mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("read in-memory journal mode");
        assert!(journal_mode.eq_ignore_ascii_case("memory"));
        assert_ne!(first.identity().store_id(), second.identity().store_id());
    }

    #[test]
    fn lifecycle_transactions_are_idempotent_and_reject_conflicts() {
        let store = RuntimeStore::open_in_memory().expect("open in-memory store");
        let request = NewLogicalRequest {
            id: LogicalRequestId::new(),
            begun_at_unix_ms: 10,
        };

        let first_request = store
            .transaction(|transaction| transaction.begin_logical_request(request))
            .expect("begin logical request");
        assert_eq!(first_request.disposition, BeginDisposition::Inserted);
        assert_eq!(
            store
                .transaction(|transaction| transaction.begin_logical_request(request))
                .expect("repeat logical request begin")
                .disposition,
            BeginDisposition::AlreadyExists
        );
        let request_conflict = store
            .transaction(|transaction| {
                transaction.begin_logical_request(NewLogicalRequest {
                    begun_at_unix_ms: 11,
                    ..request
                })
            })
            .expect_err("reject conflicting logical request begin");
        assert!(matches!(
            request_conflict,
            RuntimeStoreError::InvariantViolation { .. }
        ));

        let attempt = NewAttempt {
            id: AttemptId::new(),
            logical_request_id: request.id,
            begun_at_unix_ms: 20,
            evidence: test_attempt_evidence(),
        };
        let first_attempt = store
            .transaction(|transaction| {
                transaction.begin_attempt(first_request.handle, attempt.clone())
            })
            .expect("begin upstream attempt");
        assert_eq!(first_attempt.disposition, BeginDisposition::Inserted);
        assert_eq!(first_attempt.attempt_ordinal, 1);
        let repeated_attempt = store
            .transaction(|transaction| {
                transaction.begin_attempt(first_request.handle, attempt.clone())
            })
            .expect("repeat upstream attempt begin");
        assert_eq!(
            repeated_attempt.disposition,
            BeginDisposition::AlreadyExists
        );
        assert_eq!(repeated_attempt.attempt_ordinal, 1);
        let attempt_begin_conflict = store
            .transaction(|transaction| {
                transaction.begin_attempt(
                    first_request.handle,
                    NewAttempt {
                        begun_at_unix_ms: 21,
                        ..attempt.clone()
                    },
                )
            })
            .expect_err("reject conflicting upstream attempt begin");
        assert!(matches!(
            attempt_begin_conflict,
            RuntimeStoreError::InvariantViolation { .. }
        ));
        let attempt_evidence_conflict = store
            .transaction(|transaction| {
                let mut conflicting_evidence = attempt.evidence.clone();
                conflicting_evidence.route.mapped_model = Some("gpt-5-conflict".to_string());
                transaction.begin_attempt(
                    first_request.handle,
                    NewAttempt {
                        evidence: conflicting_evidence,
                        ..attempt.clone()
                    },
                )
            })
            .expect_err("reject conflicting upstream attempt evidence");
        assert!(matches!(
            attempt_evidence_conflict,
            RuntimeStoreError::InvariantViolation { .. }
        ));

        let logical_while_attempt_pending = store
            .transaction(|transaction| {
                transaction.commit_logical_request_terminal(
                    first_request.handle,
                    LogicalRequestTerminal {
                        outcome: LogicalRequestOutcome::Succeeded,
                        terminal_at_unix_ms: 30,
                        economics_state: EconomicsState::Known,
                        payload: Some(test_logical_terminal_payload()),
                    },
                )
            })
            .expect_err("pending attempt blocks logical terminal");
        assert!(matches!(
            logical_while_attempt_pending,
            RuntimeStoreError::InvariantViolation { .. }
        ));

        let attempt_terminal = AttemptTerminal {
            outcome: AttemptOutcome::Succeeded,
            terminal_at_unix_ms: 31,
            economics_state: EconomicsState::Known,
        };
        assert_eq!(
            store
                .transaction(|transaction| {
                    transaction.commit_attempt_terminal(first_attempt.handle, attempt_terminal)
                })
                .expect("commit attempt terminal"),
            TerminalDisposition::Committed
        );
        assert_eq!(
            store
                .transaction(|transaction| {
                    transaction.commit_attempt_terminal(first_attempt.handle, attempt_terminal)
                })
                .expect("repeat identical attempt terminal"),
            TerminalDisposition::AlreadyIdentical
        );
        let attempt_conflict = store
            .transaction(|transaction| {
                transaction.commit_attempt_terminal(
                    first_attempt.handle,
                    AttemptTerminal {
                        economics_state: EconomicsState::Partial,
                        ..attempt_terminal
                    },
                )
            })
            .expect_err("reject conflicting attempt terminal");
        assert!(matches!(
            attempt_conflict,
            RuntimeStoreError::InvariantViolation { .. }
        ));

        let second_attempt = store
            .transaction(|transaction| {
                transaction.begin_attempt(
                    first_request.handle,
                    NewAttempt {
                        id: AttemptId::new(),
                        logical_request_id: request.id,
                        begun_at_unix_ms: 32,
                        evidence: test_attempt_evidence(),
                    },
                )
            })
            .expect("begin second attempt");
        assert_eq!(second_attempt.attempt_ordinal, 2);
        store
            .transaction(|transaction| {
                transaction.commit_attempt_terminal(
                    second_attempt.handle,
                    AttemptTerminal {
                        outcome: AttemptOutcome::Succeeded,
                        terminal_at_unix_ms: 33,
                        economics_state: EconomicsState::Known,
                    },
                )
            })
            .expect("commit second attempt terminal");

        let mut logical_terminal_payload = test_logical_terminal_payload();
        logical_terminal_payload.winning_attempt_id = Some(second_attempt.handle.id());
        apply_test_attempt_economics(&mut logical_terminal_payload);
        let logical_terminal = LogicalRequestTerminal {
            outcome: LogicalRequestOutcome::Succeeded,
            terminal_at_unix_ms: 34,
            economics_state: EconomicsState::Known,
            payload: Some(logical_terminal_payload),
        };
        assert_eq!(
            store
                .transaction(|transaction| {
                    transaction.commit_logical_request_terminal(
                        first_request.handle,
                        logical_terminal.clone(),
                    )
                })
                .expect("commit logical terminal"),
            TerminalDisposition::Committed
        );
        assert_eq!(
            store
                .transaction(|transaction| {
                    transaction.commit_logical_request_terminal(
                        first_request.handle,
                        logical_terminal.clone(),
                    )
                })
                .expect("repeat identical logical terminal"),
            TerminalDisposition::AlreadyIdentical
        );
        let logical_record = store
            .read_logical_request(first_request.handle)
            .expect("read logical request")
            .expect("logical request exists");
        assert_eq!(
            logical_record.terminal.expect("logical terminal").terminal,
            logical_terminal
        );
    }

    #[test]
    fn logical_success_requires_a_succeeded_attempt_from_the_same_request() {
        let store = RuntimeStore::open_in_memory().expect("open in-memory store");
        let first_request = NewLogicalRequest {
            id: LogicalRequestId::new(),
            begun_at_unix_ms: 10,
        };
        let second_request = NewLogicalRequest {
            id: LogicalRequestId::new(),
            begun_at_unix_ms: 11,
        };
        let first_request_handle = store
            .transaction(|transaction| transaction.begin_logical_request(first_request))
            .expect("begin first request")
            .handle;
        let second_request_handle = store
            .transaction(|transaction| transaction.begin_logical_request(second_request))
            .expect("begin second request")
            .handle;

        let failed_attempt = store
            .transaction(|transaction| {
                transaction.begin_attempt(
                    first_request_handle,
                    NewAttempt {
                        id: AttemptId::new(),
                        logical_request_id: first_request.id,
                        begun_at_unix_ms: 20,
                        evidence: test_attempt_evidence(),
                    },
                )
            })
            .expect("begin failed attempt");
        store
            .transaction(|transaction| {
                transaction.commit_attempt_terminal(
                    failed_attempt.handle,
                    AttemptTerminal {
                        outcome: AttemptOutcome::Failed,
                        terminal_at_unix_ms: 21,
                        economics_state: EconomicsState::Unknown,
                    },
                )
            })
            .expect("finish failed attempt");

        let foreign_attempt = store
            .transaction(|transaction| {
                transaction.begin_attempt(
                    second_request_handle,
                    NewAttempt {
                        id: AttemptId::new(),
                        logical_request_id: second_request.id,
                        begun_at_unix_ms: 22,
                        evidence: test_attempt_evidence(),
                    },
                )
            })
            .expect("begin foreign attempt");
        store
            .transaction(|transaction| {
                transaction.commit_attempt_terminal(
                    foreign_attempt.handle,
                    AttemptTerminal {
                        outcome: AttemptOutcome::Succeeded,
                        terminal_at_unix_ms: 23,
                        economics_state: EconomicsState::Known,
                    },
                )
            })
            .expect("finish foreign attempt");

        let mut missing_payload = test_logical_terminal_payload();
        missing_payload.winning_attempt_id = None;
        let missing_winner = store
            .transaction(|transaction| {
                transaction.commit_logical_request_terminal(
                    first_request_handle,
                    LogicalRequestTerminal {
                        outcome: LogicalRequestOutcome::Succeeded,
                        terminal_at_unix_ms: 30,
                        economics_state: EconomicsState::Known,
                        payload: Some(missing_payload),
                    },
                )
            })
            .expect_err("request with attempts requires a winning attempt");
        assert!(matches!(
            missing_winner,
            RuntimeStoreError::InvariantViolation { .. }
        ));

        for invalid_winner in [failed_attempt.handle.id(), foreign_attempt.handle.id()] {
            let mut payload = test_logical_terminal_payload();
            payload.winning_attempt_id = Some(invalid_winner);
            let error = store
                .transaction(|transaction| {
                    transaction.commit_logical_request_terminal(
                        first_request_handle,
                        LogicalRequestTerminal {
                            outcome: LogicalRequestOutcome::Succeeded,
                            terminal_at_unix_ms: 30,
                            economics_state: EconomicsState::Known,
                            payload: Some(payload),
                        },
                    )
                })
                .expect_err("invalid winning attempt must be rejected");
            assert!(matches!(
                error,
                RuntimeStoreError::InvariantViolation { .. }
            ));
        }

        let succeeded_attempt = store
            .transaction(|transaction| {
                transaction.begin_attempt(
                    first_request_handle,
                    NewAttempt {
                        id: AttemptId::new(),
                        logical_request_id: first_request.id,
                        begun_at_unix_ms: 24,
                        evidence: test_attempt_evidence(),
                    },
                )
            })
            .expect("begin succeeded attempt");
        store
            .transaction(|transaction| {
                transaction.commit_attempt_terminal(
                    succeeded_attempt.handle,
                    AttemptTerminal {
                        outcome: AttemptOutcome::Succeeded,
                        terminal_at_unix_ms: 25,
                        economics_state: EconomicsState::Known,
                    },
                )
            })
            .expect("finish succeeded attempt");

        let mut payload = test_logical_terminal_payload();
        payload.winning_attempt_id = Some(succeeded_attempt.handle.id());
        apply_test_attempt_economics(&mut payload);
        store
            .transaction(|transaction| {
                transaction.commit_logical_request_terminal(
                    first_request_handle,
                    LogicalRequestTerminal {
                        outcome: LogicalRequestOutcome::Succeeded,
                        terminal_at_unix_ms: 30,
                        economics_state: EconomicsState::Known,
                        payload: Some(payload),
                    },
                )
            })
            .expect("commit logical success with valid winner");

        let record = store
            .read_logical_request(first_request_handle)
            .expect("read logical request")
            .expect("logical request exists");
        assert_eq!(
            record
                .terminal
                .and_then(|terminal| terminal.terminal.payload)
                .and_then(|payload| payload.winning_attempt_id),
            Some(succeeded_attempt.handle.id())
        );
    }

    #[test]
    fn logical_terminal_rejects_provider_epoch_conflicting_with_winning_attempt() {
        let store = RuntimeStore::open_in_memory().expect("open in-memory store");
        let request = NewLogicalRequest {
            id: LogicalRequestId::new(),
            begun_at_unix_ms: 10,
        };
        let request_handle = store
            .transaction(|transaction| transaction.begin_logical_request(request))
            .expect("begin request")
            .handle;
        let attempt = store
            .transaction(|transaction| {
                transaction.begin_attempt(
                    request_handle,
                    NewAttempt {
                        id: AttemptId::new(),
                        logical_request_id: request.id,
                        begun_at_unix_ms: 20,
                        evidence: test_attempt_evidence(),
                    },
                )
            })
            .expect("begin attempt");
        store
            .transaction(|transaction| {
                transaction.commit_attempt_terminal(
                    attempt.handle,
                    AttemptTerminal {
                        outcome: AttemptOutcome::Succeeded,
                        terminal_at_unix_ms: 25,
                        economics_state: EconomicsState::Known,
                    },
                )
            })
            .expect("finish attempt");

        let mut payload = test_logical_terminal_payload();
        payload.winning_attempt_id = Some(attempt.handle.id());
        apply_test_attempt_economics(&mut payload);
        payload
            .provider_epoch
            .as_mut()
            .expect("provider epoch")
            .catalog_revision = Some("catalog-conflict".to_string());
        let error = store
            .transaction(|transaction| {
                transaction.commit_logical_request_terminal(
                    request_handle,
                    LogicalRequestTerminal {
                        outcome: LogicalRequestOutcome::Succeeded,
                        terminal_at_unix_ms: 30,
                        economics_state: EconomicsState::Known,
                        payload: Some(payload),
                    },
                )
            })
            .expect_err("winner provider epoch conflict must be rejected");

        assert!(matches!(
            error,
            RuntimeStoreError::InvariantViolation { .. }
        ));

        for conflict in [
            "scope",
            "config_revision",
            "pricing_revision",
            "mapped_model",
            "price_key_epoch",
            "price_key_model",
            "price_key_tier",
        ] {
            let mut payload = test_logical_terminal_payload();
            payload.winning_attempt_id = Some(attempt.handle.id());
            apply_test_attempt_economics(&mut payload);
            match conflict {
                "scope" => {
                    payload
                        .provider_epoch
                        .as_mut()
                        .expect("provider epoch")
                        .scope
                        .endpoint_origin = "https://other.example.test".to_string();
                }
                "config_revision" => {
                    payload
                        .provider_epoch
                        .as_mut()
                        .expect("provider epoch")
                        .scope
                        .config_revision = "other-runtime".to_string();
                }
                "pricing_revision" => {
                    payload
                        .provider_epoch
                        .as_mut()
                        .expect("provider epoch")
                        .pricing_revision = Some("pricing-conflict".to_string());
                }
                "mapped_model" => {
                    payload.mapped_model = Some("gpt-conflict".to_string());
                }
                "price_key_epoch" => {
                    payload
                        .provider_price_key
                        .as_mut()
                        .expect("provider price key")
                        .epoch
                        .scope
                        .route_scope = "other-route".to_string();
                }
                "price_key_model" => {
                    payload
                        .provider_price_key
                        .as_mut()
                        .expect("provider price key")
                        .model = "gpt-conflict".to_string();
                }
                "price_key_tier" => {
                    payload
                        .provider_price_key
                        .as_mut()
                        .expect("provider price key")
                        .tier = crate::provider_catalog::ProviderPricingTier::Priority;
                }
                _ => unreachable!(),
            }
            let error = store
                .transaction(|transaction| {
                    transaction.commit_logical_request_terminal(
                        request_handle,
                        LogicalRequestTerminal {
                            outcome: LogicalRequestOutcome::Succeeded,
                            terminal_at_unix_ms: 30,
                            economics_state: EconomicsState::Known,
                            payload: Some(payload),
                        },
                    )
                })
                .expect_err("winner economic evidence conflict must be rejected");
            assert!(
                matches!(error, RuntimeStoreError::InvariantViolation { .. }),
                "conflict: {conflict}"
            );
        }
    }

    #[test]
    fn logical_terminal_payload_round_trips_priced_retry_evidence_across_reopen() {
        let home = TestDir::new("terminal-payload-reopen");
        let store = RuntimeStore::open_in_home(home.path()).expect("create store");
        let request = NewLogicalRequest {
            id: LogicalRequestId::new(),
            begun_at_unix_ms: 10,
        };
        let handle = store
            .transaction(|transaction| transaction.begin_logical_request(request))
            .expect("begin logical request")
            .handle;

        let usage = crate::usage::UsageMetrics {
            input_tokens: 1_000,
            output_tokens: 50,
            total_tokens: 1_050,
            cache_read_input_tokens: 100,
            cache_creation_input_tokens: 200,
            ..crate::usage::UsageMetrics::default()
        };
        let convention = crate::usage::CacheAccountingConvention::INCLUDED_IN_INPUT;
        let price = crate::pricing::ModelPrice::from_per_million_usd(
            "gpt-5.6-sol",
            None,
            "2.5",
            "15",
            Some("0.25"),
            Some("3.125"),
            "test-price",
        )
        .expect("valid test price");
        let cost = crate::pricing::estimate_usage_cost_with_convention(
            &usage,
            &price,
            crate::pricing::CostAdjustments::default(),
            convention,
        );
        let expected_femto_usd = cost
            .total_cost_femto_usd()
            .expect("priced request has exact femto-USD cache");

        let mut payload = test_logical_terminal_payload();
        payload.finished_request.model = Some("gpt-5.6-sol".to_string());
        payload.finished_request.usage = Some(usage.clone());
        payload.finished_request.cost = cost;
        payload.finished_request.retry = Some(crate::logging::RetryInfo {
            attempts: 2,
            route_attempts: vec![
                crate::logging::RouteAttemptLog {
                    attempt_index: 0,
                    provider_id: Some("primary".to_string()),
                    endpoint_id: Some("endpoint-3".to_string()),
                    provider_endpoint_key: Some("codex/primary/endpoint-3".to_string()),
                    decision: "failed_status".to_string(),
                    ..crate::logging::RouteAttemptLog::default()
                },
                crate::logging::RouteAttemptLog {
                    attempt_index: 1,
                    provider_id: Some("backup".to_string()),
                    endpoint_id: Some("endpoint-7".to_string()),
                    provider_endpoint_key: Some("codex/backup/endpoint-7".to_string()),
                    decision: "completed".to_string(),
                    ..crate::logging::RouteAttemptLog::default()
                },
            ],
        });
        payload.requested_model = Some("gpt-5.6-sol".to_string());
        payload.mapped_model = Some("gpt-5.6-sol".to_string());
        payload.pricing_model = Some("gpt-5.6-sol".to_string());
        payload.cache_accounting_convention = convention;
        payload.billable_usage = Some(usage.canonical_usage_buckets(convention));
        let terminal = LogicalRequestTerminal {
            outcome: LogicalRequestOutcome::Succeeded,
            terminal_at_unix_ms: 34,
            economics_state: EconomicsState::Known,
            payload: Some(payload),
        };
        store
            .transaction(|transaction| {
                transaction.commit_logical_request_terminal(handle, terminal.clone())
            })
            .expect("commit logical terminal");
        drop(store);

        let reopened = RuntimeStore::open_in_home(home.path()).expect("reopen store");
        let reopened_handle = reopened.logical_request_handle(request.id);
        let persisted = reopened
            .read_logical_request(reopened_handle)
            .expect("read logical request")
            .expect("logical request exists")
            .terminal
            .expect("logical terminal")
            .terminal;
        assert_eq!(persisted, terminal);
        assert_eq!(
            persisted
                .payload
                .as_ref()
                .expect("runtime payload")
                .finished_request
                .cost
                .total_cost_femto_usd(),
            Some(expected_femto_usd)
        );
        assert_eq!(
            reopened
                .transaction(|transaction| {
                    transaction.commit_logical_request_terminal(reopened_handle, terminal.clone())
                })
                .expect("repeat identical terminal after reopen"),
            TerminalDisposition::AlreadyIdentical
        );

        let mut conflicting_model = terminal.clone();
        conflicting_model
            .payload
            .as_mut()
            .expect("runtime payload")
            .mapped_model = Some("gpt-5.6-terra".to_string());
        assert!(matches!(
            reopened
                .transaction(|transaction| {
                    transaction.commit_logical_request_terminal(reopened_handle, conflicting_model)
                })
                .expect_err("reject conflicting durable payload"),
            RuntimeStoreError::InvariantViolation { .. }
        ));

        let mut conflicting_cost = terminal.clone();
        conflicting_cost
            .payload
            .as_mut()
            .expect("runtime payload")
            .finished_request
            .cost
            .total_cost_usd = Some("999".to_string());
        assert!(matches!(
            reopened
                .transaction(|transaction| {
                    transaction.commit_logical_request_terminal(reopened_handle, conflicting_cost)
                })
                .expect_err("reject conflicting economic payload"),
            RuntimeStoreError::InvariantViolation { .. }
        ));

        let conflicting_envelope = LogicalRequestTerminal {
            outcome: LogicalRequestOutcome::Failed,
            ..terminal
        };
        assert!(matches!(
            reopened
                .transaction(|transaction| {
                    transaction
                        .commit_logical_request_terminal(reopened_handle, conflicting_envelope)
                })
                .expect_err("reject conflicting terminal envelope"),
            RuntimeStoreError::InvariantViolation { .. }
        ));
    }

    #[test]
    fn lifecycle_handles_reject_cross_store_use() {
        let first = RuntimeStore::open_in_memory().expect("open first store");
        let second = RuntimeStore::open_in_memory().expect("open second store");
        let request = NewLogicalRequest {
            id: LogicalRequestId::new(),
            begun_at_unix_ms: 1,
        };
        let handle = first
            .transaction(|transaction| transaction.begin_logical_request(request))
            .expect("begin request in first store")
            .handle;
        let attempt = NewAttempt {
            id: AttemptId::new(),
            logical_request_id: request.id,
            begun_at_unix_ms: 2,
            evidence: test_attempt_evidence(),
        };

        let error = second
            .transaction(|transaction| transaction.begin_attempt(handle, attempt))
            .expect_err("reject first store handle in second store");

        assert!(matches!(
            error,
            RuntimeStoreError::ForeignStoreHandle { .. }
        ));
    }

    #[test]
    fn terminal_attempt_evidence_round_trips_across_reopen() {
        let home = TestDir::new("attempt-evidence-reopen");
        let request = NewLogicalRequest {
            id: LogicalRequestId::new(),
            begun_at_unix_ms: 1,
        };
        let attempt = NewAttempt {
            id: AttemptId::new(),
            logical_request_id: request.id,
            begun_at_unix_ms: 2,
            evidence: test_attempt_evidence(),
        };

        let store = RuntimeStore::open_in_home(home.path()).expect("create runtime store");
        let request_handle = store
            .transaction(|transaction| transaction.begin_logical_request(request))
            .expect("begin logical request")
            .handle;
        let attempt_handle = store
            .transaction(|transaction| transaction.begin_attempt(request_handle, attempt.clone()))
            .expect("begin upstream attempt")
            .handle;
        store
            .transaction(|transaction| {
                transaction.commit_attempt_terminal(
                    attempt_handle,
                    AttemptTerminal {
                        outcome: AttemptOutcome::Succeeded,
                        terminal_at_unix_ms: 3,
                        economics_state: EconomicsState::Known,
                    },
                )
            })
            .expect("commit upstream attempt terminal");
        store
            .transaction(|transaction| {
                let mut payload = test_logical_terminal_payload();
                payload.winning_attempt_id = Some(attempt_handle.id());
                apply_test_attempt_economics(&mut payload);
                transaction.commit_logical_request_terminal(
                    request_handle,
                    LogicalRequestTerminal {
                        outcome: LogicalRequestOutcome::Succeeded,
                        terminal_at_unix_ms: 4,
                        economics_state: EconomicsState::Known,
                        payload: Some(payload),
                    },
                )
            })
            .expect("commit logical request terminal");
        drop(store);

        let reopened = RuntimeStore::open_in_home(home.path()).expect("reopen runtime store");
        let record = reopened
            .read_attempt(attempt_handle)
            .expect("read upstream attempt")
            .expect("upstream attempt exists");
        assert_eq!(record.attempt, attempt);
        assert_eq!(record.attempt_ordinal, 1);
        assert_eq!(
            record
                .terminal
                .expect("upstream attempt is terminal")
                .terminal,
            AttemptTerminal {
                outcome: AttemptOutcome::Succeeded,
                terminal_at_unix_ms: 3,
                economics_state: EconomicsState::Known,
            }
        );
        assert_eq!(
            reopened.startup_recovery_report().interrupted_attempt_count,
            0
        );
    }

    #[test]
    fn ignored_lifecycle_error_rolls_back_the_entire_transaction() {
        let store = RuntimeStore::open_in_memory().expect("open store");
        let existing = NewLogicalRequest {
            id: LogicalRequestId::new(),
            begun_at_unix_ms: 1,
        };
        store
            .transaction(|transaction| transaction.begin_logical_request(existing))
            .expect("begin existing request");
        let should_roll_back = NewLogicalRequest {
            id: LogicalRequestId::new(),
            begun_at_unix_ms: 2,
        };

        let error = store
            .transaction(|transaction| {
                let _ignored = transaction.begin_logical_request(NewLogicalRequest {
                    begun_at_unix_ms: 999,
                    ..existing
                });
                transaction
                    .begin_logical_request(should_roll_back)
                    .expect("later mutation is locally valid");
                Ok(())
            })
            .expect_err("ignored invariant failure must poison the transaction");

        assert!(matches!(
            error,
            RuntimeStoreError::InvariantViolation { .. }
        ));
        assert!(
            store
                .read_logical_request(store.logical_request_handle(should_roll_back.id))
                .expect("read rolled back request")
                .is_none()
        );
    }

    #[test]
    fn persistent_store_allows_only_one_writer_owner() {
        let home = TestDir::new("writer-owner");
        let path = runtime_store_path_in(home.path());
        let first = RuntimeStore::open(&path).expect("open first writer");

        let error = RuntimeStore::open(&path).expect_err("reject second writer");
        assert!(matches!(
            error,
            RuntimeStoreError::WriterAlreadyOwned { .. }
        ));

        drop(first);
        RuntimeStore::open(&path).expect("writer lease is released on drop");
    }

    #[test]
    fn startup_recovery_interrupts_stranded_rows_once() {
        let home = TestDir::new("startup-recovery");
        let request = NewLogicalRequest {
            id: LogicalRequestId::new(),
            begun_at_unix_ms: 100,
        };
        let attempt = NewAttempt {
            id: AttemptId::new(),
            logical_request_id: request.id,
            begun_at_unix_ms: 110,
            evidence: test_attempt_evidence(),
        };
        let expected_attempt = attempt.clone();

        let store = RuntimeStore::open_in_home(home.path()).expect("create store");
        assert_eq!(store.startup_recovery_report().recovery_ordinal, 1);
        assert_eq!(store.startup_recovery_report().interrupted_logical_count, 0);
        assert_eq!(store.startup_recovery_report().interrupted_attempt_count, 0);
        let request_handle = store
            .transaction(|transaction| transaction.begin_logical_request(request))
            .expect("begin logical request")
            .handle;
        let attempt_handle = store
            .transaction(|transaction| transaction.begin_attempt(request_handle, attempt))
            .expect("begin attempt")
            .handle;
        drop(store);

        let recovered = RuntimeStore::open_in_home(home.path()).expect("recover stranded rows");
        let first_report = recovered.startup_recovery_report();
        assert_eq!(first_report.recovery_ordinal, 2);
        assert_eq!(first_report.interrupted_logical_count, 1);
        assert_eq!(first_report.interrupted_attempt_count, 1);
        let logical_terminal = recovered
            .read_logical_request(request_handle)
            .expect("read recovered logical request")
            .expect("recovered logical request exists")
            .terminal
            .expect("logical request is terminal");
        assert_eq!(
            logical_terminal.terminal.outcome,
            LogicalRequestOutcome::Interrupted
        );
        assert_eq!(
            logical_terminal.terminal.economics_state,
            EconomicsState::Unknown
        );
        assert_eq!(logical_terminal.origin, TerminalOrigin::StartupRecovery);
        assert_eq!(logical_terminal.recovery_run_id, Some(first_report.run_id));
        assert!(logical_terminal.terminal.payload.is_none());
        let recovered_attempt = recovered
            .read_attempt(attempt_handle)
            .expect("read recovered attempt")
            .expect("recovered attempt exists");
        assert_eq!(recovered_attempt.attempt, expected_attempt);
        let attempt_terminal = recovered_attempt.terminal.expect("attempt is terminal");
        assert_eq!(
            attempt_terminal.terminal.outcome,
            AttemptOutcome::Interrupted
        );
        assert_eq!(
            attempt_terminal.terminal.economics_state,
            EconomicsState::Unknown
        );
        assert_eq!(attempt_terminal.origin, TerminalOrigin::StartupRecovery);
        assert_eq!(attempt_terminal.recovery_run_id, Some(first_report.run_id));
        drop(recovered);

        let clean_restart =
            RuntimeStore::open_in_home(home.path()).expect("restart recovered store");
        let second_report = clean_restart.startup_recovery_report();
        assert_eq!(second_report.recovery_ordinal, 3);
        assert_eq!(second_report.interrupted_logical_count, 0);
        assert_eq!(second_report.interrupted_attempt_count, 0);
        assert_ne!(second_report.run_id, first_report.run_id);
        assert_eq!(
            clean_restart
                .latest_recovery_report()
                .expect("read latest recovery report"),
            Some(second_report)
        );
    }

    #[test]
    fn crash_gate_recovers_pending_boundaries_once() {
        #[derive(Clone, Copy)]
        enum CrashBoundary {
            BeforeWrite,
            BeforeTerminal,
        }

        for (label, boundary, expected_interrupted_attempts) in [
            ("before-write", CrashBoundary::BeforeWrite, 1),
            ("before-terminal", CrashBoundary::BeforeTerminal, 0),
        ] {
            let home = TestDir::new(label);
            let request = NewLogicalRequest {
                id: LogicalRequestId::new(),
                begun_at_unix_ms: 100,
            };
            let attempt = NewAttempt {
                id: AttemptId::new(),
                logical_request_id: request.id,
                begun_at_unix_ms: 110,
                evidence: test_attempt_evidence(),
            };
            let expected_attempt = attempt.clone();
            let runtime_attempt_terminal = AttemptTerminal {
                outcome: AttemptOutcome::Succeeded,
                terminal_at_unix_ms: 120,
                economics_state: EconomicsState::Known,
            };

            let store = RuntimeStore::open_in_home(home.path()).expect("create crash-gate store");
            let empty_revision = store
                .operator_ledger_revision()
                .expect("read empty operator revision");
            let request_handle = store
                .transaction(|transaction| transaction.begin_logical_request(request))
                .expect("begin crash-gate logical request")
                .handle;
            let attempt_handle = store
                .transaction(|transaction| transaction.begin_attempt(request_handle, attempt))
                .expect("begin crash-gate attempt")
                .handle;
            if matches!(boundary, CrashBoundary::BeforeTerminal) {
                store
                    .transaction(|transaction| {
                        transaction
                            .commit_attempt_terminal(attempt_handle, runtime_attempt_terminal)
                    })
                    .expect("commit attempt before logical terminal");
            }
            drop(store);

            let recovered = RuntimeStore::open_in_home(home.path()).expect("recover crash gate");
            let first_report = recovered.startup_recovery_report();
            assert_eq!(first_report.recovery_ordinal, 2, "boundary={label}");
            assert_eq!(
                first_report.interrupted_logical_count, 1,
                "boundary={label}"
            );
            assert_eq!(
                first_report.interrupted_attempt_count, expected_interrupted_attempts,
                "boundary={label}"
            );

            let logical_terminal = recovered
                .read_logical_request(request_handle)
                .expect("read recovered logical request")
                .expect("recovered logical request exists")
                .terminal
                .expect("recovered logical request is terminal");
            assert_eq!(
                logical_terminal.terminal.outcome,
                LogicalRequestOutcome::Interrupted,
                "boundary={label}"
            );
            assert_eq!(
                logical_terminal.terminal.economics_state,
                EconomicsState::Unknown,
                "boundary={label}"
            );
            assert!(
                logical_terminal.terminal.payload.is_none(),
                "boundary={label}"
            );
            assert_eq!(
                logical_terminal.origin,
                TerminalOrigin::StartupRecovery,
                "boundary={label}"
            );
            assert_eq!(
                logical_terminal.recovery_run_id,
                Some(first_report.run_id),
                "boundary={label}"
            );

            let recovered_attempt = recovered
                .read_attempt(attempt_handle)
                .expect("read recovered attempt")
                .expect("recovered attempt exists");
            assert_eq!(
                recovered_attempt.attempt, expected_attempt,
                "boundary={label}"
            );
            assert_eq!(recovered_attempt.attempt_ordinal, 1, "boundary={label}");
            let attempt_terminal = recovered_attempt
                .terminal
                .expect("recovered attempt is terminal");
            match boundary {
                CrashBoundary::BeforeWrite => {
                    assert_eq!(
                        attempt_terminal.terminal.outcome,
                        AttemptOutcome::Interrupted,
                        "boundary={label}"
                    );
                    assert_eq!(
                        attempt_terminal.terminal.economics_state,
                        EconomicsState::Unknown,
                        "boundary={label}"
                    );
                    assert_eq!(
                        attempt_terminal.origin,
                        TerminalOrigin::StartupRecovery,
                        "boundary={label}"
                    );
                    assert_eq!(
                        attempt_terminal.recovery_run_id,
                        Some(first_report.run_id),
                        "boundary={label}"
                    );
                }
                CrashBoundary::BeforeTerminal => {
                    assert_eq!(attempt_terminal.terminal, runtime_attempt_terminal);
                    assert_eq!(attempt_terminal.origin, TerminalOrigin::Runtime);
                    assert_eq!(attempt_terminal.recovery_run_id, None);
                }
            }
            assert!(
                recovered
                    .query_committed_requests(&CommittedRequestQuery::default())
                    .expect("query recovered projections")
                    .items
                    .is_empty(),
                "startup recovery must not publish a runtime projection; boundary={label}"
            );
            assert_eq!(
                recovered
                    .operator_ledger_revision()
                    .expect("read recovered operator revision"),
                empty_revision,
                "boundary={label}"
            );
            drop(recovered);

            let clean_restart =
                RuntimeStore::open_in_home(home.path()).expect("restart recovered crash gate");
            let second_report = clean_restart.startup_recovery_report();
            assert_eq!(second_report.recovery_ordinal, 3, "boundary={label}");
            assert_eq!(
                second_report.interrupted_logical_count, 0,
                "boundary={label}"
            );
            assert_eq!(
                second_report.interrupted_attempt_count, 0,
                "boundary={label}"
            );
            assert_eq!(
                clean_restart
                    .read_logical_request(request_handle)
                    .expect("read logical request after clean restart")
                    .expect("logical request survives clean restart")
                    .terminal,
                Some(logical_terminal),
                "boundary={label}"
            );
            assert_eq!(
                clean_restart
                    .read_attempt(attempt_handle)
                    .expect("read attempt after clean restart")
                    .expect("attempt survives clean restart")
                    .terminal,
                Some(attempt_terminal),
                "boundary={label}"
            );
            assert!(
                clean_restart
                    .query_committed_requests(&CommittedRequestQuery::default())
                    .expect("query projections after clean restart")
                    .items
                    .is_empty(),
                "clean restart must not duplicate a recovery projection; boundary={label}"
            );
            assert_eq!(
                clean_restart
                    .operator_ledger_revision()
                    .expect("read operator revision after clean restart"),
                empty_revision,
                "boundary={label}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn persistent_store_rejects_second_writer_after_lease_path_is_unlinked() {
        let home = TestDir::new("unlinked-writer-lease");
        let path = runtime_store_path_in(home.path());
        let first = RuntimeStore::open(&path).expect("open first writer");
        fs::remove_file(writer_lease_path(&path)).expect("unlink held writer lease path");

        let error = RuntimeStore::open(&path)
            .expect_err("unlinked lease path must not allow a second writer");
        assert!(matches!(
            error,
            RuntimeStoreError::WriterAlreadyOwned { .. }
        ));

        drop(first);
        RuntimeStore::open(&path).expect("first writer drop releases all kernel leases");
    }

    #[test]
    fn persistent_store_writer_lease_is_cross_process_and_crash_recoverable() {
        let home = TestDir::new("cross-process-writer");
        let path = runtime_store_path_in(home.path());
        let ready_path = home.path().join("child-ready");
        let mut child = Command::new(std::env::current_exe().expect("current test executable"))
            .arg("runtime_store::tests::runtime_store_writer_lease_child")
            .arg("--exact")
            .env(LOCK_CHILD_PATH_ENV, &path)
            .env(LOCK_CHILD_READY_ENV, &ready_path)
            .spawn()
            .expect("spawn writer lease child");

        if !wait_for_path(&ready_path, Duration::from_secs(10)) {
            let _ = child.kill();
            let status = child.wait().expect("wait for failed writer child");
            panic!("writer child did not acquire lease; status={status}");
        }

        let second_writer = RuntimeStore::open(&path);
        child
            .kill()
            .expect("terminate writer child without cleanup");
        let status = child.wait().expect("wait for writer child");

        assert!(!status.success(), "writer child should be terminated");
        assert!(matches!(
            second_writer,
            Err(RuntimeStoreError::WriterAlreadyOwned { .. })
        ));
        RuntimeStore::open(&path).expect("child exit releases kernel writer lease");
    }

    #[cfg(unix)]
    #[test]
    fn persistent_store_database_lock_survives_cross_process_lease_unlink() {
        let home = TestDir::new("cross-process-unlinked-lease");
        let path = runtime_store_path_in(home.path());
        let ready_path = home.path().join("child-ready");
        let mut child = Command::new(std::env::current_exe().expect("current test executable"))
            .arg("runtime_store::tests::runtime_store_writer_lease_child")
            .arg("--exact")
            .env(LOCK_CHILD_PATH_ENV, &path)
            .env(LOCK_CHILD_READY_ENV, &ready_path)
            .spawn()
            .expect("spawn writer lease child");

        if !wait_for_path(&ready_path, Duration::from_secs(10)) {
            let _ = child.kill();
            let status = child.wait().expect("wait for failed writer child");
            panic!("writer child did not acquire lease; status={status}");
        }

        fs::remove_file(writer_lease_path(&path)).expect("unlink child writer lease path");
        let second_writer = RuntimeStore::open(&path);
        child
            .kill()
            .expect("terminate writer child without cleanup");
        let status = child.wait().expect("wait for writer child");

        assert!(!status.success(), "writer child should be terminated");
        assert!(matches!(
            second_writer,
            Err(RuntimeStoreError::WriterAlreadyOwned { .. })
        ));
        RuntimeStore::open(&path).expect("child exit releases SQLite database ownership");
    }

    #[cfg(unix)]
    #[test]
    fn write_transactions_fail_closed_after_database_wal_and_lease_replacement() {
        let home = TestDir::new("replaced-persistent-files");
        let path = runtime_store_path_in(home.path());
        let store = RuntimeStore::open(&path).expect("open persistent store");
        let wal_path = database_sidecar_path(&path, "-wal");
        let lease_path = writer_lease_path(&path);
        assert!(wal_path.exists(), "startup recovery should materialize WAL");

        let original_path = path.with_added_extension("original");
        let original_wal_path = wal_path.with_added_extension("original");
        let original_lease_path = lease_path.with_added_extension("original");
        fs::rename(&path, &original_path).expect("move opened database");
        fs::rename(&wal_path, &original_wal_path).expect("move opened WAL");
        fs::rename(&lease_path, &original_lease_path).expect("move held writer lease");
        fs::write(&path, b"replacement database").expect("replace database path");
        fs::write(&wal_path, b"replacement WAL").expect("replace WAL path");
        fs::write(&lease_path, b"replacement lease").expect("replace lease path");

        let first_error = store
            .transaction(|transaction| {
                transaction.begin_logical_request(NewLogicalRequest {
                    id: LogicalRequestId::new(),
                    begun_at_unix_ms: 1,
                })
            })
            .expect_err("replacement must stop writes");
        assert!(matches!(
            first_error,
            RuntimeStoreError::PersistentFileReplaced { .. }
        ));

        fs::remove_file(&path).expect("remove replacement database");
        fs::remove_file(&wal_path).expect("remove replacement WAL");
        fs::remove_file(&lease_path).expect("remove replacement lease");
        fs::rename(&original_path, &path).expect("restore opened database path");
        fs::rename(&original_wal_path, &wal_path).expect("restore opened WAL path");
        fs::rename(&original_lease_path, &lease_path).expect("restore writer lease path");

        let second_error = store
            .transaction(|transaction| {
                transaction.begin_logical_request(NewLogicalRequest {
                    id: LogicalRequestId::new(),
                    begun_at_unix_ms: 2,
                })
            })
            .expect_err("identity failure permanently poisons the store");
        assert!(matches!(
            second_error,
            RuntimeStoreError::PersistentFileIdentityPoisoned
        ));

        let memory = RuntimeStore::open_in_memory().expect("open in-memory store");
        memory
            .transaction(|transaction| {
                transaction.begin_logical_request(NewLogicalRequest {
                    id: LogicalRequestId::new(),
                    begun_at_unix_ms: 3,
                })
            })
            .expect("in-memory stores do not require file identity");
    }

    #[cfg(unix)]
    #[test]
    fn write_transactions_detect_wal_path_replacement() {
        let home = TestDir::new("replaced-wal");
        let path = runtime_store_path_in(home.path());
        let store = RuntimeStore::open(&path).expect("open persistent store");
        let wal_path = database_sidecar_path(&path, "-wal");
        let original_wal_path = wal_path.with_added_extension("original");
        fs::rename(&wal_path, &original_wal_path).expect("move opened WAL");
        fs::write(&wal_path, b"replacement WAL").expect("replace WAL path");

        let error = store
            .transaction(|transaction| {
                transaction.begin_logical_request(NewLogicalRequest {
                    id: LogicalRequestId::new(),
                    begun_at_unix_ms: 1,
                })
            })
            .expect_err("WAL replacement must stop writes");
        assert!(matches!(
            error,
            RuntimeStoreError::PersistentFileReplaced {
                component: "WAL",
                ..
            }
        ));

        fs::remove_file(&wal_path).expect("remove replacement WAL");
        fs::rename(&original_wal_path, &wal_path).expect("restore opened WAL");
    }

    #[cfg(unix)]
    #[test]
    fn write_transactions_detect_writer_lease_path_replacement() {
        let home = TestDir::new("replaced-writer-lease");
        let path = runtime_store_path_in(home.path());
        let store = RuntimeStore::open(&path).expect("open persistent store");
        let lease_path = writer_lease_path(&path);
        let original_lease_path = lease_path.with_added_extension("original");
        fs::rename(&lease_path, &original_lease_path).expect("move held writer lease");
        fs::write(&lease_path, b"replacement lease").expect("replace writer lease path");

        let error = store
            .transaction(|transaction| {
                transaction.begin_logical_request(NewLogicalRequest {
                    id: LogicalRequestId::new(),
                    begun_at_unix_ms: 1,
                })
            })
            .expect_err("writer lease replacement must stop writes");
        assert!(matches!(
            error,
            RuntimeStoreError::PersistentFileReplaced {
                component: "writer lease",
                ..
            }
        ));

        fs::remove_file(&lease_path).expect("remove replacement lease");
        fs::rename(&original_lease_path, &lease_path).expect("restore held writer lease");
    }

    #[cfg(unix)]
    #[test]
    fn persistent_store_rejects_symbolic_link_database_without_touching_target() {
        use std::os::unix::fs::symlink;

        let home = TestDir::new("database-symlink");
        let path = runtime_store_path_in(home.path());
        fs::create_dir_all(path.parent().expect("database parent")).expect("create state dir");
        let target = home.path().join("database-target");
        fs::write(&target, b"target-must-remain-unchanged").expect("write database target");
        symlink(&target, &path).expect("create database symlink");

        let error = RuntimeStore::open(&path).expect_err("reject database symlink");

        assert!(matches!(
            error,
            RuntimeStoreError::UnsafeDatabasePath { .. }
        ));
        assert_eq!(
            fs::read(&target).expect("read database target"),
            b"target-must-remain-unchanged"
        );
    }

    #[cfg(unix)]
    #[test]
    fn persistent_store_rejects_hard_linked_database_without_touching_target() {
        let home = TestDir::new("database-hardlink");
        let path = runtime_store_path_in(home.path());
        fs::create_dir_all(path.parent().expect("database parent")).expect("create state dir");
        let target = home.path().join("database-target.sqlite");
        let connection = Connection::open(&target).expect("create database target");
        connection
            .execute_batch("CREATE TABLE scratch (value TEXT); DROP TABLE scratch;")
            .expect("materialize empty database target");
        drop(connection);
        let original = fs::read(&target).expect("read original database target");
        fs::hard_link(&target, &path).expect("create database hard link");

        let result = RuntimeStore::open(&path);

        assert!(result.is_err(), "hard-linked database must be rejected");
        assert_eq!(
            fs::read(&target).expect("read database target"),
            original,
            "rejecting the hard link must not claim the target database"
        );
    }

    #[cfg(unix)]
    #[test]
    fn persistent_store_rejects_symbolic_link_writer_lease_without_touching_target() {
        use std::os::unix::fs::symlink;

        let home = TestDir::new("lease-symlink");
        let path = runtime_store_path_in(home.path());
        fs::create_dir_all(path.parent().expect("database parent")).expect("create state dir");
        let target = home.path().join("lease-target");
        fs::write(&target, b"target-must-remain-unchanged").expect("write lease target");
        symlink(&target, writer_lease_path(&path)).expect("create lease symlink");

        let error = RuntimeStore::open(&path).expect_err("reject writer lease symlink");

        assert!(matches!(error, RuntimeStoreError::UnsafeWriterLease { .. }));
        assert_eq!(
            fs::read(&target).expect("read lease target"),
            b"target-must-remain-unchanged"
        );
    }

    #[cfg(unix)]
    #[test]
    fn persistent_store_rejects_hard_linked_writer_lease_without_touching_target() {
        let home = TestDir::new("lease-hardlink");
        let path = runtime_store_path_in(home.path());
        fs::create_dir_all(path.parent().expect("database parent")).expect("create state dir");
        let target = home.path().join("lease-target");
        fs::write(&target, b"target-must-remain-unchanged").expect("write lease target");
        fs::hard_link(&target, writer_lease_path(&path)).expect("create lease hard link");

        let error = RuntimeStore::open(&path).expect_err("reject hard-linked writer lease");

        assert!(matches!(error, RuntimeStoreError::UnsafeWriterLease { .. }));
        assert_eq!(
            fs::read(&target).expect("read lease target"),
            b"target-must-remain-unchanged"
        );
    }

    #[cfg(unix)]
    #[test]
    fn persistent_store_secures_state_directory_and_files() {
        use std::os::unix::fs::PermissionsExt;

        let home = TestDir::new("permissions");
        let store = RuntimeStore::open_in_home(home.path()).expect("create store");
        let state_dir = store
            .path()
            .expect("persistent path")
            .parent()
            .expect("state directory");

        assert_eq!(
            fs::metadata(state_dir)
                .expect("state directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for entry in fs::read_dir(state_dir).expect("read state directory") {
            let entry = entry.expect("state directory entry");
            let metadata = entry.metadata().expect("state file metadata");
            assert_eq!(
                metadata.permissions().mode() & 0o077,
                0,
                "{} must not grant group or other permissions",
                entry.path().display()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn reopening_valid_helper_store_secures_database_and_writer_lease() {
        use std::os::unix::fs::PermissionsExt;

        let home = TestDir::new("reopen-permissions");
        let path = runtime_store_path_in(home.path());
        drop(RuntimeStore::open(&path).expect("create helper store"));
        let lease_path = writer_lease_path(&path);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .expect("relax helper database permissions");
        fs::set_permissions(&lease_path, fs::Permissions::from_mode(0o644))
            .expect("relax helper writer lease permissions");

        let reopened = RuntimeStore::open(&path).expect("reopen valid helper store");

        for secured_path in [path, lease_path] {
            assert_eq!(
                fs::metadata(&secured_path)
                    .expect("secured helper file metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600,
                "{} should be helper-private",
                secured_path.display()
            );
        }
        drop(reopened);
    }

    #[cfg(unix)]
    #[test]
    fn persistent_store_directory_alias_cannot_bypass_writer_lease() {
        use std::os::unix::fs::symlink;

        let home = TestDir::new("directory-alias");
        let alias = home.path().with_extension("alias");
        symlink(home.path(), &alias).expect("create helper home alias");
        let first = RuntimeStore::open_in_home(home.path()).expect("open canonical helper home");

        let second = RuntimeStore::open_in_home(&alias).expect_err("reject aliased second writer");

        assert!(matches!(
            second,
            RuntimeStoreError::WriterAlreadyOwned { .. }
        ));
        drop(first);
        fs::remove_file(alias).expect("remove helper home alias");
    }

    #[test]
    fn runtime_store_writer_lease_child() {
        let Some(path) = std::env::var_os(LOCK_CHILD_PATH_ENV).map(PathBuf::from) else {
            return;
        };
        let ready_path =
            PathBuf::from(std::env::var_os(LOCK_CHILD_READY_ENV).expect("writer child ready path"));
        let _store = RuntimeStore::open(path).expect("writer child acquires store");
        fs::write(&ready_path, b"ready").expect("signal writer child ready");
        thread::sleep(Duration::from_secs(30));
        panic!("writer child was not terminated by parent test");
    }

    #[test]
    fn persistent_store_rejects_foreign_application_identity() {
        let home = TestDir::new("foreign-application");
        let path = runtime_store_path_in(home.path());
        fs::create_dir_all(path.parent().expect("database parent")).expect("create state dir");
        let connection = Connection::open(&path).expect("create foreign database");
        connection
            .pragma_update(None, "application_id", 123_456_i32)
            .expect("set foreign application id");
        drop(connection);

        let error = RuntimeStore::open(&path).expect_err("reject foreign database");
        assert!(matches!(
            error,
            RuntimeStoreError::ForeignApplication {
                expected: APPLICATION_ID,
                actual: 123_456,
                ..
            }
        ));

        WriterLease::acquire(&path).expect("failed open releases writer lease");
    }

    #[cfg(unix)]
    #[test]
    fn rejecting_foreign_database_does_not_change_its_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let home = TestDir::new("foreign-permissions");
        let path = runtime_store_path_in(home.path());
        fs::create_dir_all(path.parent().expect("database parent")).expect("create state dir");
        let connection = Connection::open(&path).expect("create foreign database");
        connection
            .pragma_update(None, "application_id", 123_456_i32)
            .expect("set foreign application id");
        drop(connection);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .expect("set foreign database permissions");
        let original = fs::read(&path).expect("read foreign database before rejection");
        let original_modified = fs::metadata(&path)
            .expect("foreign database metadata before rejection")
            .modified()
            .expect("foreign database modified time");
        let sentinel = path.with_added_extension("sentinel");
        fs::write(&sentinel, b"foreign sentinel").expect("write foreign sentinel");

        let error = RuntimeStore::open(&path).expect_err("reject foreign database");

        assert!(matches!(
            error,
            RuntimeStoreError::ForeignApplication { .. }
        ));
        assert_eq!(
            fs::metadata(&path)
                .expect("foreign database metadata")
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
        assert_eq!(
            fs::read(&path).expect("read rejected foreign database"),
            original
        );
        assert_eq!(
            fs::metadata(&path)
                .expect("foreign database metadata after rejection")
                .modified()
                .expect("foreign database modified time"),
            original_modified
        );
        assert_eq!(
            fs::read(&sentinel).expect("read foreign sentinel"),
            b"foreign sentinel"
        );
        assert!(!writer_lease_path(&path).exists());
    }

    #[test]
    fn persistent_store_rejects_unknown_schema_revision() {
        let home = TestDir::new("schema-revision");
        let path = runtime_store_path_in(home.path());
        fs::create_dir_all(path.parent().expect("database parent")).expect("create state dir");
        let connection = Connection::open(&path).expect("create database");
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .expect("set application id");
        connection
            .pragma_update(None, "user_version", 99_i32)
            .expect("set schema revision");
        drop(connection);

        let error = RuntimeStore::open(&path).expect_err("reject unknown schema revision");
        assert!(matches!(
            error,
            RuntimeStoreError::UnsupportedSchemaRevision {
                expected: SCHEMA_REVISION,
                actual: 99,
                ..
            }
        ));
    }

    #[test]
    fn persistent_store_classifies_missing_current_schema_as_invalid_metadata() {
        let home = TestDir::new("missing-current-schema");
        let path = runtime_store_path_in(home.path());
        fs::create_dir_all(path.parent().expect("database parent")).expect("create state dir");
        let connection = Connection::open(&path).expect("create database");
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .expect("set application id");
        connection
            .pragma_update(None, "user_version", SCHEMA_REVISION)
            .expect("set schema revision");
        drop(connection);

        let error = RuntimeStore::open(&path).expect_err("reject missing current schema");
        assert!(matches!(error, RuntimeStoreError::InvalidMetadata { .. }));
    }

    #[test]
    fn persistent_store_rejects_logically_incompatible_current_schema() {
        let home = TestDir::new("incompatible-current-schema");
        let path = runtime_store_path_in(home.path());
        fs::create_dir_all(path.parent().expect("database parent")).expect("create state dir");
        let connection = Connection::open(&path).expect("create database");
        connection
            .execute_batch(
                "CREATE TABLE store_meta (
                    singleton INTEGER,
                    application TEXT,
                    schema TEXT,
                    schema_revision INTEGER,
                    store_id TEXT
                );",
            )
            .expect("create incompatible metadata table");
        connection
            .execute(
                "INSERT INTO store_meta VALUES (1, ?1, ?2, ?3, ?4)",
                params![
                    APPLICATION_NAME,
                    SCHEMA_NAME,
                    SCHEMA_REVISION,
                    Uuid::new_v4().to_string()
                ],
            )
            .expect("insert plausible metadata");
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .expect("set application id");
        connection
            .pragma_update(None, "user_version", SCHEMA_REVISION)
            .expect("set schema revision");
        drop(connection);

        let error = RuntimeStore::open(&path).expect_err("reject incompatible current schema");
        assert!(matches!(error, RuntimeStoreError::InvalidMetadata { .. }));
    }

    #[test]
    fn persistent_store_refuses_to_claim_an_unidentified_nonempty_database() {
        let home = TestDir::new("unclaimed");
        let path = runtime_store_path_in(home.path());
        fs::create_dir_all(path.parent().expect("database parent")).expect("create state dir");
        let connection = Connection::open(&path).expect("create database");
        connection
            .execute("CREATE TABLE foreign_data (value TEXT NOT NULL)", [])
            .expect("create foreign table");
        drop(connection);

        let error = RuntimeStore::open(&path).expect_err("reject unidentified database");
        assert!(matches!(
            error,
            RuntimeStoreError::UnidentifiedNonemptyDatabase { .. }
        ));
    }

    #[test]
    fn persistent_store_rejects_corrupt_database() {
        let home = TestDir::new("corrupt");
        let path = runtime_store_path_in(home.path());
        fs::create_dir_all(path.parent().expect("database parent")).expect("create state dir");
        fs::write(&path, b"not a sqlite database").expect("write corrupt database");

        let error = RuntimeStore::open(&path).expect_err("reject corrupt database");
        assert!(matches!(error, RuntimeStoreError::CorruptDatabase { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn rejecting_corrupt_database_does_not_change_its_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let home = TestDir::new("corrupt-permissions");
        let path = runtime_store_path_in(home.path());
        fs::create_dir_all(path.parent().expect("database parent")).expect("create state dir");
        fs::write(&path, b"not a sqlite database").expect("write corrupt database");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .expect("set corrupt database permissions");
        let original = fs::read(&path).expect("read corrupt database before rejection");
        let original_modified = fs::metadata(&path)
            .expect("corrupt database metadata before rejection")
            .modified()
            .expect("corrupt database modified time");
        let sentinel = path.with_added_extension("sentinel");
        fs::write(&sentinel, b"corrupt sentinel").expect("write corrupt sentinel");

        let error = RuntimeStore::open(&path).expect_err("reject corrupt database");

        assert!(matches!(error, RuntimeStoreError::CorruptDatabase { .. }));
        assert_eq!(
            fs::metadata(&path)
                .expect("corrupt database metadata")
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
        assert_eq!(
            fs::read(&path).expect("read rejected corrupt database"),
            original
        );
        assert_eq!(
            fs::metadata(&path)
                .expect("corrupt database metadata after rejection")
                .modified()
                .expect("corrupt database modified time"),
            original_modified
        );
        assert_eq!(
            fs::read(&sentinel).expect("read corrupt sentinel"),
            b"corrupt sentinel"
        );
        assert!(!writer_lease_path(&path).exists());
    }

    #[test]
    fn persistent_store_rejects_tampered_helper_metadata() {
        let home = TestDir::new("tampered-metadata");
        let path = runtime_store_path_in(home.path());
        drop(RuntimeStore::open(&path).expect("create store"));

        let connection = Connection::open(&path).expect("open database for tampering");
        connection
            .execute("UPDATE store_meta SET schema_revision = 99", [])
            .expect("tamper store identity metadata");
        drop(connection);

        let error = RuntimeStore::open(&path).expect_err("reject tampered metadata");
        assert!(matches!(
            error,
            RuntimeStoreError::UnsupportedSchemaRevision {
                expected: SCHEMA_REVISION,
                actual: 99,
                ..
            }
        ));
    }

    #[test]
    fn startup_recovery_rejects_terminal_request_with_pending_attempt_atomically() {
        let home = TestDir::new("invalid-recovery-state");
        let path = runtime_store_path_in(home.path());
        let request = NewLogicalRequest {
            id: LogicalRequestId::new(),
            begun_at_unix_ms: 10,
        };
        let attempt = NewAttempt {
            id: AttemptId::new(),
            logical_request_id: request.id,
            begun_at_unix_ms: 20,
            evidence: test_attempt_evidence(),
        };
        let store = RuntimeStore::open(&path).expect("create store");
        let request_handle = store
            .transaction(|transaction| transaction.begin_logical_request(request))
            .expect("begin logical request")
            .handle;
        store
            .transaction(|transaction| transaction.begin_attempt(request_handle, attempt))
            .expect("begin upstream attempt");
        drop(store);

        let connection = Connection::open(&path).expect("open store for fault injection");
        connection
            .pragma_update(None, "ignore_check_constraints", true)
            .expect("temporarily disable CHECK constraints for fault injection");
        connection
            .execute(
                "UPDATE logical_requests
                 SET terminal_outcome = 'failed',
                     terminal_at_unix_ms = 30,
                     economics_state = 'known',
                     terminal_origin = 'runtime',
                     terminal_payload_json = '{}',
                     service_name = 'codex',
                     numeric_request_id = 1
                 WHERE logical_request_id = ?1",
                params![request.id.as_uuid().as_bytes().as_slice()],
            )
            .expect("inject impossible lifecycle state");
        connection
            .pragma_update(None, "ignore_check_constraints", false)
            .expect("restore CHECK constraint enforcement");
        let recovery_count_before: i64 = connection
            .query_row("SELECT COUNT(*) FROM recovery_runs", [], |row| row.get(0))
            .expect("count recovery runs before failed open");
        drop(connection);

        let error = RuntimeStore::open(&path).expect_err("invalid recovery state blocks open");
        assert!(matches!(
            error,
            RuntimeStoreError::InvariantViolation { .. }
        ));

        let connection = Connection::open(&path).expect("inspect store after failed recovery");
        let recovery_count_after: i64 = connection
            .query_row("SELECT COUNT(*) FROM recovery_runs", [], |row| row.get(0))
            .expect("count recovery runs after failed open");
        assert_eq!(recovery_count_after, recovery_count_before);
        let pending_attempts: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM upstream_attempts WHERE terminal_outcome IS NULL",
                [],
                |row| row.get(0),
            )
            .expect("count pending attempts after failed recovery");
        assert_eq!(pending_attempts, 1);
    }

    #[test]
    fn persistent_store_reports_state_directory_creation_failure() {
        let home = TestDir::new("state-directory-error");
        fs::write(home.path().join("state"), b"not a directory").expect("write state blocker");

        let error = RuntimeStore::open_in_home(home.path()).expect_err("reject invalid state path");
        assert!(matches!(error, RuntimeStoreError::CreateDirectory { .. }));
    }

    #[test]
    fn runtime_store_is_safe_to_inject_behind_an_arc() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RuntimeStore>();
    }

    fn wait_for_path(path: &Path, timeout: Duration) -> bool {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if path.exists() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        path.exists()
    }
}

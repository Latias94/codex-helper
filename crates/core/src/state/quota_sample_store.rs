//! Schema-versioned persistence for the semantic quota registry checkpoint.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::file_replace::{
    AtomicWriteError, recover_uncertain_candidate, write_bytes_file_validated,
};
use crate::quota_pool::{
    DEFAULT_MAX_SAMPLES_PER_POOL, DEFAULT_SAMPLE_RETENTION_MS, PoolMembership,
    QUOTA_CHECKPOINT_SCHEMA_VERSION, QuotaObservation, QuotaPoolRegistry, QuotaPoolState,
    QuotaRegistryCheckpoint,
};

const LOCK_WAIT: Duration = Duration::from_secs(5);
const LOCK_RETRY: Duration = Duration::from_millis(5);
const STALE_LOCK_AGE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaSampleLoad {
    Missing,
    Corrupt { message: String },
    Unsupported { schema_version: u64 },
    Valid(QuotaRegistryCheckpoint),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaSampleSave {
    Committed,
    Unchanged,
}

#[derive(Debug, Error)]
pub enum QuotaSampleStoreError {
    #[error(
        "quota sample store is read-only because schema version {schema_version} is newer than supported version {supported_version}"
    )]
    UnsupportedSchema {
        schema_version: u64,
        supported_version: u32,
    },
    #[error("refusing stale quota checkpoint generation {attempted}; latest is {latest}")]
    StaleGeneration { attempted: u64, latest: u64 },
    #[error("quota checkpoint changed concurrently since it was loaded")]
    ConcurrentContent,
    #[error("quota checkpoint generation {generation} has conflicting content")]
    GenerationConflict { generation: u64 },
    #[error("timed out acquiring quota checkpoint advisory lock {path:?}")]
    LockTimeout { path: PathBuf },
    #[error("quota atomic write failed before commit: {detail}")]
    AtomicBeforeCommit { detail: String },
    #[error(
        "quota atomic write commit state remains uncertain after recovery; destination generation is {destination_generation:?}: {detail}"
    )]
    AtomicCommitStateUnknown {
        destination_generation: Option<u64>,
        detail: String,
    },
    #[error("quota sample I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("quota sample serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiskBaseline {
    digest: Option<[u8; 32]>,
    generation: u64,
}

#[derive(serde::Deserialize)]
struct CheckpointSchemaEnvelope {
    #[serde(default)]
    schema_version: Option<u64>,
}

#[derive(serde::Serialize)]
struct CheckpointWireRef<'a> {
    schema_version: u32,
    generation: u64,
    pools: &'a BTreeMap<String, QuotaPoolState>,
    memberships: Vec<&'a PoolMembership>,
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct CheckpointWireOwned {
    schema_version: u32,
    generation: u64,
    pools: BTreeMap<String, QuotaPoolState>,
    memberships: CheckpointMemberships,
}

#[derive(Default)]
struct CheckpointMemberships(
    BTreeMap<crate::runtime_identity::ProviderEndpointKey, PoolMembership>,
);

impl<'de> serde::Deserialize<'de> for CheckpointMemberships {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct MembershipVisitor;

        impl<'de> serde::de::Visitor<'de> for MembershipVisitor {
            type Value = CheckpointMemberships;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a membership array or a legacy empty object")
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut memberships = BTreeMap::new();
                while let Some(membership) = sequence.next_element::<PoolMembership>()? {
                    let endpoint = membership.endpoint.clone();
                    if let Some(existing) = memberships.insert(endpoint.clone(), membership.clone())
                        && existing != membership
                    {
                        return Err(serde::de::Error::custom(format!(
                            "conflicting duplicate membership for endpoint {endpoint}"
                        )));
                    }
                }
                Ok(CheckpointMemberships(memberships))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                if map
                    .next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?
                    .is_some()
                {
                    return Err(serde::de::Error::custom(
                        "legacy membership object must be empty",
                    ));
                }
                Ok(CheckpointMemberships::default())
            }
        }

        deserializer.deserialize_any(MembershipVisitor)
    }
}

#[derive(Debug)]
enum DiskState {
    Missing,
    Corrupt {
        bytes: Vec<u8>,
        message: String,
    },
    Unsupported {
        bytes: Vec<u8>,
        schema_version: u64,
    },
    Valid {
        bytes: Vec<u8>,
        checkpoint: QuotaRegistryCheckpoint,
    },
}

impl DiskState {
    fn baseline(&self) -> DiskBaseline {
        match self {
            Self::Missing => DiskBaseline {
                digest: None,
                generation: 0,
            },
            Self::Corrupt { bytes, .. } | Self::Unsupported { bytes, .. } => DiskBaseline {
                digest: Some(sha256(bytes)),
                generation: 0,
            },
            Self::Valid { bytes, checkpoint } => DiskBaseline {
                digest: Some(sha256(bytes)),
                generation: checkpoint.generation,
            },
        }
    }

    fn generation(&self) -> u64 {
        match self {
            Self::Valid { checkpoint, .. } => checkpoint.generation,
            _ => 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct QuotaSampleStore {
    path: PathBuf,
    baseline: Option<DiskBaseline>,
    unsupported_schema: Option<u64>,
}

impl QuotaSampleStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            baseline: None,
            unsupported_schema: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_read_only(&self) -> bool {
        self.unsupported_schema.is_some()
    }

    pub fn committed_generation(&self) -> u64 {
        self.baseline.as_ref().map_or(0, |state| state.generation)
    }

    pub fn load(&mut self) -> QuotaSampleLoad {
        match read_disk_state(&self.path) {
            Ok(state) => self.adopt_loaded_state(state),
            Err(error) => {
                self.baseline = None;
                QuotaSampleLoad::Corrupt {
                    message: error.to_string(),
                }
            }
        }
    }

    pub fn save(
        &mut self,
        checkpoint: &QuotaRegistryCheckpoint,
    ) -> Result<QuotaSampleSave, QuotaSampleStoreError> {
        if let Some(schema_version) = self.unsupported_schema {
            return Err(QuotaSampleStoreError::UnsupportedSchema {
                schema_version,
                supported_version: QUOTA_CHECKPOINT_SCHEMA_VERSION,
            });
        }

        let _lock = AdvisoryFileLock::acquire(lock_path_for(&self.path))?;
        let disk = read_disk_state(&self.path)?;
        if let DiskState::Unsupported { schema_version, .. } = &disk {
            self.unsupported_schema = Some(*schema_version);
            return Err(QuotaSampleStoreError::UnsupportedSchema {
                schema_version: *schema_version,
                supported_version: QUOTA_CHECKPOINT_SCHEMA_VERSION,
            });
        }

        if let Some(baseline) = self.baseline.as_ref() {
            if &disk.baseline() != baseline && !matches!(disk, DiskState::Valid { .. }) {
                return Err(QuotaSampleStoreError::ConcurrentContent);
            }
        } else {
            // A caller may save without an explicit load.  Establish the CAS baseline only after
            // the cross-process lock is held so no writer can change it between read and commit.
            self.baseline = Some(disk.baseline());
        }

        let disk_generation = disk.generation();
        let mut normalized = checkpoint.clone();
        normalized.schema_version = QUOTA_CHECKPOINT_SCHEMA_VERSION;
        if let DiskState::Valid {
            checkpoint: disk_checkpoint,
            ..
        } = &disk
        {
            normalized = merge_checkpoints(disk_checkpoint, &normalized)?;
            if checkpoint_content_eq(&normalized, disk_checkpoint) {
                self.baseline = Some(disk.baseline());
                return Ok(QuotaSampleSave::Unchanged);
            }
            let next_generation = disk_generation.checked_add(1).ok_or(
                QuotaSampleStoreError::GenerationConflict {
                    generation: disk_generation,
                },
            )?;
            normalized.generation = normalized.generation.max(next_generation);
        }
        let candidate = serialize_checkpoint(&normalized)?;

        let old_baseline = disk.baseline();
        match write_bytes_file_validated(&self.path, &candidate, validate_checkpoint_bytes) {
            Ok(()) => self.finish_commit(candidate, normalized.generation),
            Err(error @ AtomicWriteError::BeforeCommit { .. }) => {
                Err(QuotaSampleStoreError::AtomicBeforeCommit {
                    detail: error.to_string(),
                })
            }
            Err(error @ AtomicWriteError::CommitStateUnknown { .. }) => {
                match recover_uncertain_candidate(
                    &self.path,
                    &candidate,
                    error,
                    validate_checkpoint_bytes,
                ) {
                    Ok(()) => self.finish_commit(candidate, normalized.generation),
                    Err(recovery_error) => {
                        let recovered = read_disk_state(&self.path).ok();
                        if recovered
                            .as_ref()
                            .is_some_and(|state| state.baseline() == old_baseline)
                        {
                            self.baseline = Some(old_baseline);
                        }
                        Err(QuotaSampleStoreError::AtomicCommitStateUnknown {
                            destination_generation: recovered.as_ref().map(DiskState::generation),
                            detail: recovery_error.to_string(),
                        })
                    }
                }
            }
        }
    }

    fn finish_commit(
        &mut self,
        candidate: Vec<u8>,
        generation: u64,
    ) -> Result<QuotaSampleSave, QuotaSampleStoreError> {
        self.baseline = Some(DiskBaseline {
            digest: Some(sha256(&candidate)),
            generation,
        });
        self.unsupported_schema = None;
        Ok(QuotaSampleSave::Committed)
    }

    fn adopt_loaded_state(&mut self, state: DiskState) -> QuotaSampleLoad {
        self.baseline = Some(state.baseline());
        self.unsupported_schema = None;
        match state {
            DiskState::Missing => QuotaSampleLoad::Missing,
            DiskState::Corrupt { message, .. } => QuotaSampleLoad::Corrupt { message },
            DiskState::Unsupported { schema_version, .. } => {
                self.unsupported_schema = Some(schema_version);
                QuotaSampleLoad::Unsupported { schema_version }
            }
            DiskState::Valid { checkpoint, .. } => QuotaSampleLoad::Valid(checkpoint),
        }
    }
}

fn merge_checkpoints(
    disk: &QuotaRegistryCheckpoint,
    submitted: &QuotaRegistryCheckpoint,
) -> Result<QuotaRegistryCheckpoint, serde_json::Error> {
    let mut merged = disk.clone();
    merged.schema_version = QUOTA_CHECKPOINT_SCHEMA_VERSION;
    merged.generation = merged.generation.max(submitted.generation);

    for (pool_key, submitted_pool) in &submitted.pools {
        match merged.pools.get_mut(pool_key) {
            Some(disk_pool) => merge_pool_state(disk_pool, submitted_pool)?,
            None => {
                merged
                    .pools
                    .insert(pool_key.clone(), submitted_pool.clone());
            }
        }
    }
    for (endpoint, submitted_membership) in &submitted.memberships {
        match merged.memberships.get_mut(endpoint) {
            Some(disk_membership) => {
                *disk_membership = merge_membership(disk_membership, submitted_membership)?;
            }
            None => {
                merged
                    .memberships
                    .insert(endpoint.clone(), submitted_membership.clone());
            }
        }
    }

    let reference_ms = merged
        .pools
        .values()
        .filter_map(|pool| pool.last_attempt_at_ms)
        .max()
        .unwrap_or(0);
    if let Some(registry) = QuotaPoolRegistry::from_checkpoint_at(
        merged.clone(),
        DEFAULT_MAX_SAMPLES_PER_POOL,
        DEFAULT_SAMPLE_RETENTION_MS,
        reference_ms,
    ) {
        merged = registry.checkpoint();
    }
    Ok(merged)
}

fn merge_pool_state(
    disk: &mut QuotaPoolState,
    submitted: &QuotaPoolState,
) -> Result<(), serde_json::Error> {
    let submitted_is_newer = submitted.identity.revision > disk.identity.revision
        || submitted.identity.revision == disk.identity.revision
            && submitted.last_attempt_at_ms > disk.last_attempt_at_ms
        || submitted.identity.revision == disk.identity.revision
            && submitted.last_attempt_at_ms == disk.last_attempt_at_ms
            && serialized_rank(&submitted.identity)? > serialized_rank(&disk.identity)?;
    if submitted_is_newer {
        disk.identity = submitted.identity.clone();
    }
    disk.last_success_at_ms = disk.last_success_at_ms.max(submitted.last_success_at_ms);
    disk.last_attempt_at_ms = disk.last_attempt_at_ms.max(submitted.last_attempt_at_ms);
    disk.adjustment_revision = disk.adjustment_revision.max(submitted.adjustment_revision);

    let mut samples = BTreeMap::<u64, QuotaObservation>::new();
    for sample in disk.samples.iter().chain(&submitted.samples) {
        match samples.get_mut(&sample.observed_at_ms) {
            Some(existing) if serialized_rank(sample)? > serialized_rank(existing)? => {
                *existing = sample.clone();
            }
            Some(_) => {}
            None => {
                samples.insert(sample.observed_at_ms, sample.clone());
            }
        }
    }
    disk.samples = samples.into_values().collect::<VecDeque<_>>();
    Ok(())
}

fn merge_membership(
    disk: &PoolMembership,
    submitted: &PoolMembership,
) -> Result<PoolMembership, serde_json::Error> {
    if disk.pool.key == submitted.pool.key && disk.pool.revision == submitted.pool.revision {
        let mut merged = if serialized_rank(&submitted.pool)? > serialized_rank(&disk.pool)? {
            submitted.clone()
        } else {
            disk.clone()
        };
        merged.since_ms = disk.since_ms.min(submitted.since_ms);
        return Ok(merged);
    }

    let disk_rank = (disk.pool.revision, disk.since_ms, serialized_rank(disk)?);
    let submitted_rank = (
        submitted.pool.revision,
        submitted.since_ms,
        serialized_rank(submitted)?,
    );
    Ok(if submitted_rank > disk_rank {
        submitted.clone()
    } else {
        disk.clone()
    })
}

fn serialized_rank(value: &impl serde::Serialize) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(value)
}

fn checkpoint_content_eq(left: &QuotaRegistryCheckpoint, right: &QuotaRegistryCheckpoint) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left.schema_version = QUOTA_CHECKPOINT_SCHEMA_VERSION;
    right.schema_version = QUOTA_CHECKPOINT_SCHEMA_VERSION;
    left.generation = 0;
    right.generation = 0;
    left == right
}

fn serialize_checkpoint(
    checkpoint: &QuotaRegistryCheckpoint,
) -> Result<Vec<u8>, serde_json::Error> {
    let wire = CheckpointWireRef {
        schema_version: checkpoint.schema_version,
        generation: checkpoint.generation,
        pools: &checkpoint.pools,
        memberships: checkpoint.memberships.values().collect(),
    };
    serde_json::to_vec_pretty(&wire)
}

fn deserialize_checkpoint(bytes: &[u8]) -> Result<QuotaRegistryCheckpoint, serde_json::Error> {
    let wire: CheckpointWireOwned = serde_json::from_slice(bytes)?;
    Ok(QuotaRegistryCheckpoint {
        schema_version: wire.schema_version,
        generation: wire.generation,
        pools: wire.pools,
        memberships: wire.memberships.0,
    })
}

fn read_disk_state(path: &Path) -> Result<DiskState, io::Error> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(DiskState::Missing),
        Err(error) => return Err(error),
    };
    let envelope: CheckpointSchemaEnvelope = match serde_json::from_slice(&bytes) {
        Ok(envelope) => envelope,
        Err(error) => {
            return Ok(DiskState::Corrupt {
                bytes,
                message: error.to_string(),
            });
        }
    };
    let Some(schema_version) = envelope.schema_version else {
        return Ok(DiskState::Corrupt {
            bytes,
            message: "missing schema_version".to_string(),
        });
    };
    if schema_version > u64::from(QUOTA_CHECKPOINT_SCHEMA_VERSION) {
        return Ok(DiskState::Unsupported {
            bytes,
            schema_version,
        });
    }
    if schema_version != u64::from(QUOTA_CHECKPOINT_SCHEMA_VERSION) {
        return Ok(DiskState::Corrupt {
            bytes,
            message: format!("unsupported legacy schema_version {schema_version}"),
        });
    }
    match deserialize_checkpoint(&bytes) {
        Ok(checkpoint) => Ok(DiskState::Valid { bytes, checkpoint }),
        Err(error) => Ok(DiskState::Corrupt {
            bytes,
            message: error.to_string(),
        }),
    }
}

fn validate_checkpoint_bytes(bytes: &[u8]) -> io::Result<()> {
    let checkpoint = deserialize_checkpoint(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if checkpoint.schema_version != QUOTA_CHECKPOINT_SCHEMA_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "quota checkpoint schema changed while staging",
        ));
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn lock_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("quota-samples.json");
    path.with_file_name(format!(".{file_name}.lock"))
}

struct AdvisoryFileLock {
    path: PathBuf,
    token: String,
}

impl AdvisoryFileLock {
    fn acquire(path: PathBuf) -> Result<Self, QuotaSampleStoreError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let token = format!("{}:{}", std::process::id(), uuid::Uuid::new_v4());
        let started = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    file.write_all(token.as_bytes())?;
                    file.sync_all()?;
                    return Ok(Self { path, token });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(&path) {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    if started.elapsed() >= LOCK_WAIT {
                        return Err(QuotaSampleStoreError::LockTimeout { path });
                    }
                    thread::sleep(LOCK_RETRY);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for AdvisoryFileLock {
    fn drop(&mut self) {
        if fs::read_to_string(&self.path).ok().as_deref() == Some(self.token.as_str()) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn lock_is_stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age >= STALE_LOCK_AGE)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;
    use crate::quota_pool::{
        NormalizationSignature, PoolIdentity, QuotaPoolRegistry, QuotaQuantity, QuotaScope,
        QuotaUnit,
    };

    fn temp_path() -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "codex-helper-quota-sample-{}",
                uuid::Uuid::new_v4()
            ))
            .join("quota.json")
    }

    fn checkpoint_for_writer(
        endpoint_id: &str,
        samples: &[(u64, i128)],
        generation: u64,
    ) -> QuotaRegistryCheckpoint {
        let identity = PoolIdentity::resolve(
            "https://relay.example",
            QuotaScope::Account,
            None,
            Some("shared-account"),
            None,
            None,
            0,
        );
        let endpoint =
            crate::runtime_identity::ProviderEndpointKey::new("codex", endpoint_id, "default");
        let observations = samples
            .iter()
            .map(|(observed_at_ms, value)| QuotaObservation {
                pool: identity.clone(),
                endpoint: Some(endpoint.clone()),
                observed_at_ms: *observed_at_ms,
                source: "test".to_string(),
                status: "ok".to_string(),
                fresh: true,
                used: Some(QuotaQuantity::from_integer(*value, QuotaUnit::Usd)),
                signature: NormalizationSignature {
                    pool_key: identity.key.clone(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .collect::<VecDeque<_>>();
        let last_observed_at_ms = observations.back().map(|sample| sample.observed_at_ms);
        QuotaRegistryCheckpoint {
            generation,
            pools: BTreeMap::from([(
                identity.key.clone(),
                QuotaPoolState {
                    identity: identity.clone(),
                    samples: observations,
                    last_success_at_ms: last_observed_at_ms,
                    last_attempt_at_ms: last_observed_at_ms,
                    ..Default::default()
                },
            )]),
            memberships: BTreeMap::from([(
                endpoint.clone(),
                PoolMembership {
                    pool: identity,
                    endpoint,
                    since_ms: samples
                        .first()
                        .map_or(0, |(observed_at_ms, _)| *observed_at_ms),
                },
            )]),
            ..Default::default()
        }
    }

    #[test]
    fn missing_and_valid_state_are_distinct() {
        let path = temp_path();
        let mut store = QuotaSampleStore::new(&path);
        assert_eq!(store.load(), QuotaSampleLoad::Missing);
        let checkpoint = QuotaPoolRegistry::default().checkpoint();
        assert_eq!(
            store.save(&checkpoint).expect("save"),
            QuotaSampleSave::Committed
        );
        let mut restored = QuotaSampleStore::new(&path);
        assert_eq!(restored.load(), QuotaSampleLoad::Valid(checkpoint));
    }

    #[test]
    fn future_schema_remains_byte_identical_and_read_only() {
        let path = temp_path();
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        let bytes = br#"{"schema_version":999,"future":340282366920938463463374607431768211455}"#;
        fs::write(&path, bytes).expect("write future state");
        let mut store = QuotaSampleStore::new(&path);
        assert_eq!(
            store.load(),
            QuotaSampleLoad::Unsupported {
                schema_version: 999
            }
        );
        assert!(matches!(
            store.save(&QuotaRegistryCheckpoint::default()),
            Err(QuotaSampleStoreError::UnsupportedSchema { .. })
        ));
        assert_eq!(fs::read(&path).expect("read future state"), bytes);
    }

    #[test]
    fn legacy_empty_membership_object_remains_readable() {
        let path = temp_path();
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        let bytes = br#"{"schema_version":2,"generation":7,"pools":{},"memberships":{}}"#;
        fs::write(&path, bytes).expect("write legacy checkpoint");

        let mut store = QuotaSampleStore::new(&path);
        assert_eq!(
            store.load(),
            QuotaSampleLoad::Valid(QuotaRegistryCheckpoint {
                generation: 7,
                ..Default::default()
            })
        );
    }

    #[test]
    fn legacy_non_empty_membership_object_is_not_silently_discarded() {
        let path = temp_path();
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        let bytes = br#"{"schema_version":2,"generation":7,"pools":{},"memberships":{"codex/input/default":{}}}"#;
        fs::write(&path, bytes).expect("write invalid legacy checkpoint");

        let mut store = QuotaSampleStore::new(&path);
        let QuotaSampleLoad::Corrupt { message } = store.load() else {
            panic!("non-empty legacy membership object must be rejected");
        };
        assert!(message.contains("legacy membership object must be empty"));
        assert_eq!(fs::read(&path).expect("read invalid checkpoint"), bytes);
    }

    #[test]
    fn checkpoint_round_trips_positive_and_negative_i128_beyond_64_bits() {
        let path = temp_path();
        let positive = i128::from(u64::MAX) + 17;
        let negative = i128::from(i64::MIN) - 17;
        let checkpoint =
            checkpoint_for_writer("input-a", &[(1_000, positive), (2_000, negative)], 1);
        let mut store = QuotaSampleStore::new(&path);

        assert_eq!(
            store.save(&checkpoint).expect("save i128 checkpoint"),
            QuotaSampleSave::Committed
        );

        let mut restored = QuotaSampleStore::new(&path);
        let QuotaSampleLoad::Valid(restored) = restored.load() else {
            panic!("checkpoint should remain valid");
        };
        let pool_key = checkpoint.pools.keys().next().expect("pool");
        let values = restored.pools[pool_key]
            .samples
            .iter()
            .map(|sample| sample.used.as_ref().expect("used").value)
            .collect::<Vec<_>>();
        assert_eq!(values, vec![positive, negative]);
        assert_eq!(restored.memberships, checkpoint.memberships);
    }

    #[test]
    fn corrupt_competitors_use_exact_byte_cas() {
        let path = temp_path();
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(&path, b"corrupt-a").expect("write corrupt state");
        let mut first = QuotaSampleStore::new(&path);
        let mut second = QuotaSampleStore::new(&path);
        assert!(matches!(first.load(), QuotaSampleLoad::Corrupt { .. }));
        assert!(matches!(second.load(), QuotaSampleLoad::Corrupt { .. }));
        fs::write(&path, b"corrupt-b").expect("concurrent corrupt mutation");
        let checkpoint = QuotaRegistryCheckpoint {
            generation: 1,
            ..Default::default()
        };
        assert!(matches!(
            first.save(&checkpoint),
            Err(QuotaSampleStoreError::ConcurrentContent)
        ));
        assert!(matches!(
            second.save(&checkpoint),
            Err(QuotaSampleStoreError::ConcurrentContent)
        ));
    }

    #[test]
    fn real_barrier_writers_reconcile_a_missing_baseline() {
        let path = temp_path();
        let mut first = QuotaSampleStore::new(&path);
        let mut second = QuotaSampleStore::new(&path);
        assert_eq!(first.load(), QuotaSampleLoad::Missing);
        assert_eq!(second.load(), QuotaSampleLoad::Missing);
        let barrier = Arc::new(Barrier::new(2));
        let handles = [first, second]
            .into_iter()
            .enumerate()
            .map(|(index, mut store)| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let checkpoint = QuotaRegistryCheckpoint {
                        generation: (index + 1) as u64,
                        ..Default::default()
                    };
                    barrier.wait();
                    store.save(&checkpoint)
                })
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().expect("writer"))
            .collect::<Vec<_>>();
        assert!(results.iter().all(Result::is_ok));
    }

    #[test]
    fn alternating_writers_preserve_both_observation_and_membership_streams() {
        let path = temp_path();
        let mut first = QuotaSampleStore::new(&path);
        let mut second = QuotaSampleStore::new(&path);
        assert_eq!(first.load(), QuotaSampleLoad::Missing);
        assert_eq!(second.load(), QuotaSampleLoad::Missing);

        first
            .save(&checkpoint_for_writer(
                "input-a",
                &[(1_000, 1), (2_000, 2)],
                2,
            ))
            .expect("first initial save");
        second
            .save(&checkpoint_for_writer(
                "input-b",
                &[(1_000, 1), (3_000, 3)],
                2,
            ))
            .expect("second reconciled save");
        first
            .save(&checkpoint_for_writer(
                "input-a",
                &[(1_000, 1), (2_000, 2), (4_000, 4)],
                3,
            ))
            .expect("first alternating save");
        second
            .save(&checkpoint_for_writer(
                "input-b",
                &[(1_000, 1), (3_000, 3), (5_000, 5)],
                3,
            ))
            .expect("second alternating save");

        let mut restored = QuotaSampleStore::new(&path);
        let QuotaSampleLoad::Valid(restored) = restored.load() else {
            panic!("merged checkpoint should be valid");
        };
        let pool = restored.pools.values().next().expect("shared pool");
        assert_eq!(
            pool.samples
                .iter()
                .map(|sample| sample.observed_at_ms)
                .collect::<Vec<_>>(),
            vec![1_000, 2_000, 3_000, 4_000, 5_000]
        );
        assert_eq!(restored.memberships.len(), 2);
        assert!(
            restored
                .memberships
                .keys()
                .any(|endpoint| endpoint.provider_id == "input-a")
        );
        assert!(
            restored
                .memberships
                .keys()
                .any(|endpoint| endpoint.provider_id == "input-b")
        );
    }

    #[test]
    fn checkpoint_has_no_credential_or_install_key_field() {
        let mut checkpoint = QuotaRegistryCheckpoint::default();
        checkpoint.pools.insert(
            "pool".to_string(),
            crate::quota_pool::QuotaPoolState {
                identity: PoolIdentity::resolve(
                    "https://relay.example",
                    QuotaScope::Account,
                    None,
                    None,
                    Some(b"raw-secret"),
                    Some(&[4_u8; 32]),
                    0,
                ),
                ..Default::default()
            },
        );
        let text = serde_json::to_string(&checkpoint).expect("serialize checkpoint");
        assert!(!text.contains("raw-secret"));
        assert!(!text.contains("install_key"));
        let _ = QuotaUnit::Usd;
    }
}

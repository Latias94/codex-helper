use std::fmt;
use std::path::Path;

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Transaction, params, params_from_iter};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::provider_catalog::{ProviderAdapter, ProviderPricingTier};

use super::{
    CommittedRequestCursor, CommittedRequestIdentityQuery, CommittedRequestPage,
    CommittedRequestProjection, CommittedRequestProjectionMetadata, CommittedRequestQuery,
    RuntimeStoreError, affinity, invalid_metadata, metadata, policy, sqlite_error,
};

const STORE_META_SQL: &str = "CREATE TABLE store_meta (singleton INTEGER PRIMARY KEY CHECK (singleton = 1), application TEXT NOT NULL CHECK (application = 'codex-helper'), schema TEXT NOT NULL CHECK (schema = 'canonical-relay-runtime'), schema_revision INTEGER NOT NULL CHECK (schema_revision >= 1), store_id TEXT NOT NULL UNIQUE CHECK (typeof(store_id) = 'text' AND length(store_id) = 36)) STRICT";
const RECOVERY_RUNS_SQL: &str = "CREATE TABLE recovery_runs (store_id TEXT NOT NULL, recovery_run_id BLOB NOT NULL CHECK (typeof(recovery_run_id) = 'blob' AND length(recovery_run_id) = 16 AND recovery_run_id <> zeroblob(16)), recovery_ordinal INTEGER NOT NULL CHECK (recovery_ordinal >= 1), recovered_at_unix_ms INTEGER NOT NULL CHECK (recovered_at_unix_ms >= 0), interrupted_logical_count INTEGER NOT NULL CHECK (interrupted_logical_count >= 0), interrupted_attempt_count INTEGER NOT NULL CHECK (interrupted_attempt_count >= 0), PRIMARY KEY (store_id, recovery_run_id), UNIQUE (store_id, recovery_ordinal), FOREIGN KEY (store_id) REFERENCES store_meta(store_id) ON UPDATE RESTRICT ON DELETE RESTRICT) STRICT, WITHOUT ROWID";
const LOGICAL_REQUESTS_SQL: &str = concat!(
    "CREATE TABLE logical_requests (",
    "store_id TEXT NOT NULL, ",
    "logical_request_id BLOB NOT NULL CHECK (typeof(logical_request_id) = 'blob' AND length(logical_request_id) = 16 AND logical_request_id <> zeroblob(16)), ",
    "begun_at_unix_ms INTEGER NOT NULL CHECK (begun_at_unix_ms >= 0), ",
    "terminal_outcome TEXT, terminal_at_unix_ms INTEGER, economics_state TEXT, terminal_origin TEXT, recovery_run_id BLOB, terminal_payload_json TEXT, ",
    "service_name TEXT, numeric_request_id INTEGER, trace_id TEXT, session_id TEXT, ",
    "PRIMARY KEY (store_id, logical_request_id), ",
    "FOREIGN KEY (store_id) REFERENCES store_meta(store_id) ON UPDATE RESTRICT ON DELETE RESTRICT, ",
    "FOREIGN KEY (store_id, recovery_run_id) REFERENCES recovery_runs(store_id, recovery_run_id) ON UPDATE RESTRICT ON DELETE RESTRICT, ",
    "CHECK (recovery_run_id IS NULL OR (typeof(recovery_run_id) = 'blob' AND length(recovery_run_id) = 16 AND recovery_run_id <> zeroblob(16))), ",
    "CHECK (",
    "(terminal_outcome IS NULL AND terminal_at_unix_ms IS NULL AND economics_state IS NULL AND terminal_origin IS NULL AND recovery_run_id IS NULL AND terminal_payload_json IS NULL AND service_name IS NULL AND numeric_request_id IS NULL AND trace_id IS NULL AND session_id IS NULL) ",
    "OR (terminal_outcome IN ('succeeded', 'failed', 'interrupted') AND terminal_at_unix_ms IS NOT NULL AND terminal_at_unix_ms >= 0 AND economics_state IS NOT NULL AND economics_state IN ('known', 'partial', 'unknown') AND terminal_origin IS NOT NULL AND (",
    "(terminal_origin = 'runtime' AND recovery_run_id IS NULL AND typeof(terminal_payload_json) = 'text' AND length(terminal_payload_json) > 0 AND typeof(service_name) = 'text' AND length(service_name) > 0 AND typeof(numeric_request_id) = 'integer' AND numeric_request_id >= 0 AND (trace_id IS NULL OR typeof(trace_id) = 'text') AND (session_id IS NULL OR typeof(session_id) = 'text')) ",
    "OR (terminal_origin = 'startup_recovery' AND terminal_outcome = 'interrupted' AND economics_state = 'unknown' AND recovery_run_id IS NOT NULL AND terminal_payload_json IS NULL AND service_name IS NULL AND numeric_request_id IS NULL AND trace_id IS NULL AND session_id IS NULL)",
    ")))) STRICT, WITHOUT ROWID"
);
const UPSTREAM_ATTEMPTS_SQL: &str = "CREATE TABLE upstream_attempts (store_id TEXT NOT NULL, attempt_id BLOB NOT NULL CHECK (typeof(attempt_id) = 'blob' AND length(attempt_id) = 16 AND attempt_id <> zeroblob(16)), logical_request_id BLOB NOT NULL CHECK (typeof(logical_request_id) = 'blob' AND length(logical_request_id) = 16 AND logical_request_id <> zeroblob(16)), attempt_ordinal INTEGER NOT NULL CHECK (attempt_ordinal >= 1), begun_at_unix_ms INTEGER NOT NULL CHECK (begun_at_unix_ms >= 0), pending_evidence_json TEXT NOT NULL CHECK (typeof(pending_evidence_json) = 'text' AND length(pending_evidence_json) > 0), terminal_outcome TEXT, terminal_at_unix_ms INTEGER, economics_state TEXT, terminal_origin TEXT, recovery_run_id BLOB, PRIMARY KEY (store_id, attempt_id), FOREIGN KEY (store_id, logical_request_id) REFERENCES logical_requests(store_id, logical_request_id) ON UPDATE RESTRICT ON DELETE RESTRICT, FOREIGN KEY (store_id, recovery_run_id) REFERENCES recovery_runs(store_id, recovery_run_id) ON UPDATE RESTRICT ON DELETE RESTRICT, CHECK (recovery_run_id IS NULL OR (typeof(recovery_run_id) = 'blob' AND length(recovery_run_id) = 16 AND recovery_run_id <> zeroblob(16))), CHECK ((terminal_outcome IS NULL AND terminal_at_unix_ms IS NULL AND economics_state IS NULL AND terminal_origin IS NULL AND recovery_run_id IS NULL) OR (terminal_outcome IN ('succeeded', 'failed', 'interrupted') AND terminal_at_unix_ms IS NOT NULL AND terminal_at_unix_ms >= 0 AND economics_state IS NOT NULL AND economics_state IN ('known', 'partial', 'unknown') AND terminal_origin IS NOT NULL AND ((terminal_origin = 'runtime' AND recovery_run_id IS NULL) OR (terminal_origin = 'startup_recovery' AND terminal_outcome = 'interrupted' AND economics_state = 'unknown' AND recovery_run_id IS NOT NULL))))) STRICT, WITHOUT ROWID";
const LOGICAL_REQUESTS_PENDING_SQL: &str = "CREATE INDEX logical_requests_pending ON logical_requests(store_id, logical_request_id) WHERE terminal_outcome IS NULL";
const LOGICAL_REQUESTS_RUNTIME_TERMINAL_ORDER_SQL: &str = "CREATE INDEX logical_requests_runtime_terminal_order ON logical_requests(store_id, terminal_at_unix_ms DESC, logical_request_id DESC) WHERE terminal_origin = 'runtime' AND terminal_payload_json IS NOT NULL";
const LOGICAL_REQUESTS_RUNTIME_SERVICE_REQUEST_SQL: &str = "CREATE INDEX logical_requests_runtime_service_request ON logical_requests(store_id, service_name, numeric_request_id, terminal_at_unix_ms DESC, logical_request_id DESC) WHERE terminal_origin = 'runtime' AND terminal_payload_json IS NOT NULL";
const LOGICAL_REQUESTS_RUNTIME_SERVICE_TRACE_SQL: &str = "CREATE INDEX logical_requests_runtime_service_trace ON logical_requests(store_id, service_name, trace_id, terminal_at_unix_ms DESC, logical_request_id DESC) WHERE terminal_origin = 'runtime' AND terminal_payload_json IS NOT NULL";
const LOGICAL_REQUESTS_RUNTIME_SERVICE_SESSION_SQL: &str = "CREATE INDEX logical_requests_runtime_service_session ON logical_requests(store_id, service_name, session_id, terminal_at_unix_ms DESC, logical_request_id DESC) WHERE terminal_origin = 'runtime' AND terminal_payload_json IS NOT NULL";
const UPSTREAM_ATTEMPTS_ORDINAL_SQL: &str = "CREATE UNIQUE INDEX upstream_attempts_logical_ordinal ON upstream_attempts(store_id, logical_request_id, attempt_ordinal)";
const UPSTREAM_ATTEMPTS_ONE_PENDING_SQL: &str = "CREATE UNIQUE INDEX upstream_attempts_one_pending ON upstream_attempts(store_id, logical_request_id) WHERE terminal_outcome IS NULL";
const DEFAULT_PROJECTION_CAPACITY: usize = 256;
const FILTERED_PROJECTION_SCAN_ROWS: usize = 1_024;
const COMMITTED_REQUESTS_FIRST_PAGE_SQL: &str = "SELECT logical_request_id,
            begun_at_unix_ms,
            terminal_outcome,
            terminal_at_unix_ms,
            economics_state,
            terminal_payload_json
     FROM logical_requests
     WHERE store_id = ?1
       AND terminal_origin = 'runtime'
       AND terminal_payload_json IS NOT NULL
     ORDER BY terminal_at_unix_ms DESC, logical_request_id DESC
     LIMIT ?2";
const COMMITTED_REQUESTS_AFTER_CURSOR_SQL: &str = "SELECT logical_request_id,
            begun_at_unix_ms,
            terminal_outcome,
            terminal_at_unix_ms,
            economics_state,
            terminal_payload_json
     FROM logical_requests
     WHERE store_id = ?1
       AND terminal_origin = 'runtime'
       AND terminal_payload_json IS NOT NULL
       AND (terminal_at_unix_ms, logical_request_id) < (?2, ?3)
     ORDER BY terminal_at_unix_ms DESC, logical_request_id DESC
     LIMIT ?4";
const COMMITTED_REQUESTS_SINCE_FIRST_PAGE_SQL: &str = "SELECT logical_request_id,
            begun_at_unix_ms,
            terminal_outcome,
            terminal_at_unix_ms,
            economics_state,
            terminal_payload_json
     FROM logical_requests
     WHERE store_id = ?1
       AND terminal_origin = 'runtime'
       AND terminal_payload_json IS NOT NULL
       AND terminal_at_unix_ms >= ?2
     ORDER BY terminal_at_unix_ms DESC, logical_request_id DESC
     LIMIT ?3";
const COMMITTED_REQUESTS_SINCE_AFTER_CURSOR_SQL: &str = "SELECT logical_request_id,
            begun_at_unix_ms,
            terminal_outcome,
            terminal_at_unix_ms,
            economics_state,
            terminal_payload_json
     FROM logical_requests
     WHERE store_id = ?1
       AND terminal_origin = 'runtime'
       AND terminal_payload_json IS NOT NULL
       AND terminal_at_unix_ms >= ?2
       AND (terminal_at_unix_ms, logical_request_id) < (?3, ?4)
     ORDER BY terminal_at_unix_ms DESC, logical_request_id DESC
     LIMIT ?5";

#[derive(Debug, Clone, Copy)]
struct ExpectedSchemaObject {
    object_type: &'static str,
    name: &'static str,
    table_name: &'static str,
    sql: &'static str,
}

const EXPECTED_SCHEMA_OBJECTS: &[ExpectedSchemaObject] = &[
    ExpectedSchemaObject {
        object_type: "table",
        name: "store_meta",
        table_name: "store_meta",
        sql: STORE_META_SQL,
    },
    ExpectedSchemaObject {
        object_type: "table",
        name: "recovery_runs",
        table_name: "recovery_runs",
        sql: RECOVERY_RUNS_SQL,
    },
    ExpectedSchemaObject {
        object_type: "table",
        name: "logical_requests",
        table_name: "logical_requests",
        sql: LOGICAL_REQUESTS_SQL,
    },
    ExpectedSchemaObject {
        object_type: "table",
        name: "upstream_attempts",
        table_name: "upstream_attempts",
        sql: UPSTREAM_ATTEMPTS_SQL,
    },
    ExpectedSchemaObject {
        object_type: "index",
        name: "logical_requests_pending",
        table_name: "logical_requests",
        sql: LOGICAL_REQUESTS_PENDING_SQL,
    },
    ExpectedSchemaObject {
        object_type: "index",
        name: "logical_requests_runtime_terminal_order",
        table_name: "logical_requests",
        sql: LOGICAL_REQUESTS_RUNTIME_TERMINAL_ORDER_SQL,
    },
    ExpectedSchemaObject {
        object_type: "index",
        name: "logical_requests_runtime_service_request",
        table_name: "logical_requests",
        sql: LOGICAL_REQUESTS_RUNTIME_SERVICE_REQUEST_SQL,
    },
    ExpectedSchemaObject {
        object_type: "index",
        name: "logical_requests_runtime_service_trace",
        table_name: "logical_requests",
        sql: LOGICAL_REQUESTS_RUNTIME_SERVICE_TRACE_SQL,
    },
    ExpectedSchemaObject {
        object_type: "index",
        name: "logical_requests_runtime_service_session",
        table_name: "logical_requests",
        sql: LOGICAL_REQUESTS_RUNTIME_SERVICE_SESSION_SQL,
    },
    ExpectedSchemaObject {
        object_type: "index",
        name: "upstream_attempts_logical_ordinal",
        table_name: "upstream_attempts",
        sql: UPSTREAM_ATTEMPTS_ORDINAL_SQL,
    },
    ExpectedSchemaObject {
        object_type: "index",
        name: "upstream_attempts_one_pending",
        table_name: "upstream_attempts",
        sql: UPSTREAM_ATTEMPTS_ONE_PENDING_SQL,
    },
    ExpectedSchemaObject {
        object_type: "table",
        name: "runtime_revisions",
        table_name: "runtime_revisions",
        sql: policy::RUNTIME_REVISIONS_SQL,
    },
    ExpectedSchemaObject {
        object_type: "table",
        name: "runtime_identity_authority",
        table_name: "runtime_identity_authority",
        sql: policy::RUNTIME_IDENTITY_AUTHORITY_SQL,
    },
    ExpectedSchemaObject {
        object_type: "table",
        name: "runtime_upstream_identities",
        table_name: "runtime_upstream_identities",
        sql: policy::RUNTIME_UPSTREAM_IDENTITIES_SQL,
    },
    ExpectedSchemaObject {
        object_type: "table",
        name: "provider_incarnations",
        table_name: "provider_incarnations",
        sql: policy::PROVIDER_INCARNATIONS_SQL,
    },
    ExpectedSchemaObject {
        object_type: "index",
        name: "provider_incarnations_endpoint_history",
        table_name: "provider_incarnations",
        sql: policy::PROVIDER_INCARNATIONS_ENDPOINT_SQL,
    },
    ExpectedSchemaObject {
        object_type: "table",
        name: "provider_endpoint_heads",
        table_name: "provider_endpoint_heads",
        sql: policy::PROVIDER_ENDPOINT_HEADS_SQL,
    },
    ExpectedSchemaObject {
        object_type: "table",
        name: "provider_observations",
        table_name: "provider_observations",
        sql: policy::PROVIDER_OBSERVATIONS_SQL,
    },
    ExpectedSchemaObject {
        object_type: "index",
        name: "provider_observations_endpoint_history",
        table_name: "provider_observations",
        sql: policy::PROVIDER_OBSERVATIONS_HISTORY_SQL,
    },
    ExpectedSchemaObject {
        object_type: "table",
        name: "provider_policy_actions",
        table_name: "provider_policy_actions",
        sql: policy::PROVIDER_POLICY_ACTIONS_SQL,
    },
    ExpectedSchemaObject {
        object_type: "index",
        name: "provider_policy_actions_active",
        table_name: "provider_policy_actions",
        sql: policy::PROVIDER_POLICY_ACTIONS_ACTIVE_SQL,
    },
    ExpectedSchemaObject {
        object_type: "index",
        name: "provider_policy_actions_history",
        table_name: "provider_policy_actions",
        sql: policy::PROVIDER_POLICY_ACTIONS_HISTORY_SQL,
    },
    ExpectedSchemaObject {
        object_type: "table",
        name: "provider_manual_eligibility",
        table_name: "provider_manual_eligibility",
        sql: policy::PROVIDER_MANUAL_ELIGIBILITY_SQL,
    },
    ExpectedSchemaObject {
        object_type: "table",
        name: "provider_eligibility",
        table_name: "provider_eligibility",
        sql: policy::PROVIDER_ELIGIBILITY_SQL,
    },
    ExpectedSchemaObject {
        object_type: "table",
        name: "runtime_private_keys",
        table_name: "runtime_private_keys",
        sql: metadata::RUNTIME_PRIVATE_KEYS_SQL,
    },
    ExpectedSchemaObject {
        object_type: "table",
        name: "runtime_documents",
        table_name: "runtime_documents",
        sql: metadata::RUNTIME_DOCUMENTS_SQL,
    },
    ExpectedSchemaObject {
        object_type: "table",
        name: "session_route_affinities",
        table_name: "session_route_affinities",
        sql: affinity::SESSION_ROUTE_AFFINITIES_SQL,
    },
    ExpectedSchemaObject {
        object_type: "index",
        name: "session_route_affinities_lru",
        table_name: "session_route_affinities",
        sql: affinity::SESSION_ROUTE_AFFINITIES_LRU_SQL,
    },
];

/// A UUID constructor rejected the nil UUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NilLifecycleId {
    kind: &'static str,
}

impl fmt::Display for NilLifecycleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} cannot be the nil UUID", self.kind)
    }
}

impl std::error::Error for NilLifecycleId {}

macro_rules! lifecycle_id {
    ($name:ident, $kind:literal) => {
        #[doc = concat!("A durable ", $kind, " UUID.")]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Uuid);

        #[allow(clippy::new_without_default)]
        impl $name {
            /// Generates a new random UUID.
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Creates the typed ID, rejecting the nil UUID.
            pub fn from_uuid(value: Uuid) -> Result<Self, NilLifecycleId> {
                if value.is_nil() {
                    Err(NilLifecycleId { kind: $kind })
                } else {
                    Ok(Self(value))
                }
            }

            /// Returns the underlying UUID.
            pub fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                let uuid = Uuid::parse_str(&value).map_err(serde::de::Error::custom)?;
                Self::from_uuid(uuid).map_err(serde::de::Error::custom)
            }
        }
    };
}

lifecycle_id!(LogicalRequestId, "logical request ID");
lifecycle_id!(AttemptId, "attempt ID");
lifecycle_id!(RecoveryRunId, "recovery run ID");

/// A logical request's terminal result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalRequestOutcome {
    Succeeded,
    Failed,
    Interrupted,
}

/// An upstream attempt's terminal result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    Succeeded,
    Failed,
    Interrupted,
}

/// The confidence available for terminal economic facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EconomicsState {
    Known,
    Partial,
    Unknown,
}

/// How a terminal lifecycle record was produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalOrigin {
    Runtime,
    StartupRecovery,
}

/// The result of an idempotent begin operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeginDisposition {
    Inserted,
    AlreadyExists,
}

/// The result of a conditional terminal operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalDisposition {
    Committed,
    AlreadyIdentical,
}

/// Whether a committed request participates in economic projections.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RequestAccountingScope {
    Economic,
    NonEconomic,
}

/// Immutable facts written when a logical request begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewLogicalRequest {
    pub id: LogicalRequestId,
    pub begun_at_unix_ms: u64,
}

/// Immutable facts written when an upstream attempt begins.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct AttemptRouteEvidence {
    pub provider_endpoint_key: Option<String>,
    pub provider_id: Option<String>,
    pub endpoint_id: Option<String>,
    pub route_path: Vec<String>,
    pub upstream_base_url: Option<String>,
    pub mapped_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttemptPendingEvidence {
    pub runtime_revision: u64,
    pub runtime_digest: String,
    pub route: AttemptRouteEvidence,
    #[serde(default)]
    pub provider_epoch: Option<FrozenProviderEpochIdentity>,
}

impl AttemptPendingEvidence {
    pub fn new(
        runtime_revision: u64,
        runtime_digest: impl Into<String>,
        route: AttemptRouteEvidence,
    ) -> Self {
        Self {
            runtime_revision,
            runtime_digest: runtime_digest.into(),
            route,
            provider_epoch: None,
        }
    }

    pub fn with_provider_epoch(mut self, provider_epoch: FrozenProviderEpochIdentity) -> Self {
        self.provider_epoch = Some(provider_epoch);
        self
    }

    fn validate(&self) -> Result<(), String> {
        if self.runtime_revision == 0 {
            return Err("runtime revision must be positive".to_string());
        }
        if self.runtime_digest.trim().is_empty() {
            return Err("runtime digest is empty".to_string());
        }
        if let Some(epoch) = self.provider_epoch.as_ref() {
            epoch.validate()?;
            if epoch.scope.config_revision != self.runtime_digest {
                return Err(
                    "provider epoch config revision conflicts with runtime digest".to_string(),
                );
            }
        }
        Ok(())
    }
}

/// Immutable facts written when an upstream attempt begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAttempt {
    pub id: AttemptId,
    pub logical_request_id: LogicalRequestId,
    pub begun_at_unix_ms: u64,
    pub evidence: AttemptPendingEvidence,
}

/// Provider identity dimensions captured after final request headers are known.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrozenProviderCatalogScope {
    pub adapter: ProviderAdapter,
    pub endpoint_origin: String,
    pub route_scope: String,
    pub account_fingerprint: String,
    pub config_revision: String,
}

/// Provider catalog and pricing revisions frozen before an upstream write.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrozenProviderEpochIdentity {
    pub scope: FrozenProviderCatalogScope,
    pub catalog_revision: Option<String>,
    pub pricing_revision: Option<String>,
}

impl FrozenProviderEpochIdentity {
    fn validate(&self) -> Result<(), String> {
        if self.scope.endpoint_origin.trim().is_empty() {
            return Err("provider endpoint origin is empty".to_string());
        }
        if self.scope.route_scope.trim().is_empty() {
            return Err("provider route scope is empty".to_string());
        }
        if self.scope.account_fingerprint.trim().is_empty() {
            return Err("provider account fingerprint is empty".to_string());
        }
        if self.scope.config_revision.trim().is_empty() {
            return Err("provider config revision is empty".to_string());
        }
        if self.catalog_revision.is_some() != self.pricing_revision.is_some() {
            return Err(
                "provider catalog and pricing revisions must be captured together".to_string(),
            );
        }
        Ok(())
    }
}

/// A provider-scoped pricing identity selected from actual response evidence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrozenProviderPriceKey {
    pub epoch: FrozenProviderEpochIdentity,
    pub model: String,
    pub tier: ProviderPricingTier,
}

/// Immutable request and economic evidence stored with a runtime terminal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogicalRequestTerminalPayload {
    pub finished_request: crate::state::FinishedRequest,
    pub winning_attempt_id: Option<AttemptId>,
    pub runtime_revision: u64,
    pub runtime_digest: String,
    pub policy_revision: Option<u64>,
    pub provider_epoch: Option<FrozenProviderEpochIdentity>,
    pub provider_price_key: Option<FrozenProviderPriceKey>,
    pub requested_model: Option<String>,
    pub mapped_model: Option<String>,
    pub reported_model: Option<String>,
    pub pricing_model: Option<String>,
    pub requested_service_tier: Option<String>,
    pub effective_service_tier: Option<String>,
    pub actual_service_tier: Option<String>,
    pub pricing_service_tier: Option<String>,
    pub cache_accounting_convention: crate::usage::CacheAccountingConvention,
    pub billable_usage: Option<crate::usage::CanonicalUsageBuckets>,
    pub accounting_scope: RequestAccountingScope,
}

impl LogicalRequestTerminalPayload {
    fn validate_for_write(&self) -> Result<(), String> {
        if self.runtime_digest.trim().is_empty() {
            return Err("runtime digest is empty".to_string());
        }
        if let Some(epoch) = self.provider_epoch.as_ref() {
            epoch.validate()?;
            if epoch.scope.config_revision != self.runtime_digest {
                return Err("terminal provider epoch conflicts with runtime digest".to_string());
            }
        }
        if let Some(key) = self.provider_price_key.as_ref() {
            if Some(&key.epoch) != self.provider_epoch.as_ref() {
                return Err("provider price key conflicts with terminal epoch".to_string());
            }
            if key.model.trim().is_empty() {
                return Err("provider price key model is empty".to_string());
            }
            if self.pricing_model.as_deref() != Some(key.model.as_str()) {
                return Err("provider price key model conflicts with pricing model".to_string());
            }
            if ProviderPricingTier::from_actual_service_tier(self.actual_service_tier.as_deref())
                != key.tier
            {
                return Err(
                    "provider price key tier conflicts with actual service tier".to_string()
                );
            }
        }
        let model_conflict = self
            .mapped_model
            .as_deref()
            .zip(self.reported_model.as_deref())
            .is_some_and(|(mapped, reported)| !mapped.trim().eq_ignore_ascii_case(reported.trim()));
        if model_conflict
            && (self.pricing_model.is_some()
                || self.provider_price_key.is_some()
                || !self.finished_request.cost.is_unknown())
        {
            return Err("reported model conflicts with mapped model economics".to_string());
        }
        if self.accounting_scope == RequestAccountingScope::NonEconomic
            && (self.billable_usage.is_some() || !self.finished_request.cost.is_unknown())
        {
            return Err("non-economic terminal contains billable usage or known cost".to_string());
        }
        Ok(())
    }
}

/// A runtime terminal candidate for a logical request.
#[derive(Debug, Clone, PartialEq)]
pub struct LogicalRequestTerminal {
    pub outcome: LogicalRequestOutcome,
    pub terminal_at_unix_ms: u64,
    pub economics_state: EconomicsState,
    pub payload: Option<LogicalRequestTerminalPayload>,
}

/// A runtime terminal candidate for an upstream attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttemptTerminal {
    pub outcome: AttemptOutcome,
    pub terminal_at_unix_ms: u64,
    pub economics_state: EconomicsState,
}

/// The persisted terminal envelope for a logical request.
#[derive(Debug, Clone, PartialEq)]
pub struct LogicalRequestTerminalRecord {
    pub terminal: LogicalRequestTerminal,
    pub origin: TerminalOrigin,
    pub recovery_run_id: Option<RecoveryRunId>,
}

/// The persisted terminal envelope for an upstream attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttemptTerminalRecord {
    pub terminal: AttemptTerminal,
    pub origin: TerminalOrigin,
    pub recovery_run_id: Option<RecoveryRunId>,
}

/// A complete logical request lifecycle row.
#[derive(Debug, Clone, PartialEq)]
pub struct LogicalRequestRecord {
    pub store_id: Uuid,
    pub request: NewLogicalRequest,
    pub terminal: Option<LogicalRequestTerminalRecord>,
}

/// A complete upstream attempt lifecycle row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptRecord {
    pub store_id: Uuid,
    pub attempt: NewAttempt,
    pub attempt_ordinal: u64,
    pub terminal: Option<AttemptTerminalRecord>,
}

/// The result of beginning an upstream attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeginAttemptResult {
    pub disposition: BeginDisposition,
    pub attempt_ordinal: u64,
}

/// A committed startup recovery run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryReport {
    pub store_id: Uuid,
    pub run_id: RecoveryRunId,
    pub recovery_ordinal: u64,
    pub recovered_at_unix_ms: u64,
    pub interrupted_logical_count: u64,
    pub interrupted_attempt_count: u64,
}

impl LogicalRequestOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }

    fn parse(path: &Path, value: &str) -> Result<Self, RuntimeStoreError> {
        match value {
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            other => Err(invalid_metadata(
                path,
                format!("logical request has invalid terminal outcome {other:?}"),
            )),
        }
    }
}

impl AttemptOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }

    fn parse(path: &Path, value: &str) -> Result<Self, RuntimeStoreError> {
        match value {
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            other => Err(invalid_metadata(
                path,
                format!("upstream attempt has invalid terminal outcome {other:?}"),
            )),
        }
    }
}

impl EconomicsState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Known => "known",
            Self::Partial => "partial",
            Self::Unknown => "unknown",
        }
    }

    fn parse(path: &Path, value: &str) -> Result<Self, RuntimeStoreError> {
        match value {
            "known" => Ok(Self::Known),
            "partial" => Ok(Self::Partial),
            "unknown" => Ok(Self::Unknown),
            other => Err(invalid_metadata(
                path,
                format!("lifecycle row has invalid economics state {other:?}"),
            )),
        }
    }
}

impl TerminalOrigin {
    fn parse(path: &Path, value: &str) -> Result<Self, RuntimeStoreError> {
        match value {
            "runtime" => Ok(Self::Runtime),
            "startup_recovery" => Ok(Self::StartupRecovery),
            other => Err(invalid_metadata(
                path,
                format!("lifecycle row has invalid terminal origin {other:?}"),
            )),
        }
    }
}

/// Returns the complete schema initialization batch for revision one.
pub(super) fn schema_sql() -> String {
    let mut sql = String::new();
    for object in EXPECTED_SCHEMA_OBJECTS {
        sql.push_str(object.sql);
        sql.push_str(";\n");
    }
    sql
}

/// Validates the exact non-internal SQLite schema manifest.
pub(super) fn validate_expected_schema_objects(
    connection: &Connection,
    path: &Path,
) -> Result<(), RuntimeStoreError> {
    validate_schema_objects(connection, path, EXPECTED_SCHEMA_OBJECTS)
}

pub(super) fn validate_revision_one_schema_objects(
    connection: &Connection,
    path: &Path,
) -> Result<(), RuntimeStoreError> {
    let expected = EXPECTED_SCHEMA_OBJECTS
        .iter()
        .copied()
        .filter(|object| !matches!(object.name, "runtime_private_keys" | "runtime_documents"))
        .collect::<Vec<_>>();
    validate_schema_objects(connection, path, &expected)
}

fn validate_schema_objects(
    connection: &Connection,
    path: &Path,
    expected_objects: &[ExpectedSchemaObject],
) -> Result<(), RuntimeStoreError> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name, sql
             FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%'",
        )
        .map_err(|source| sqlite_error(path, "prepare runtime schema manifest", source))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|source| sqlite_error(path, "read runtime schema manifest", source))?;
    let mut actual = Vec::new();
    for row in rows {
        actual.push(
            row.map_err(|source| sqlite_error(path, "decode runtime schema manifest", source))?,
        );
    }

    if actual.len() != expected_objects.len() {
        return Err(invalid_metadata(
            path,
            format!(
                "runtime schema contains {} objects, expected {}",
                actual.len(),
                expected_objects.len()
            ),
        ));
    }

    for expected in expected_objects {
        let Some((object_type, _, table_name, sql)) =
            actual.iter().find(|(_, name, _, _)| name == expected.name)
        else {
            return Err(invalid_metadata(
                path,
                format!("runtime schema object {:?} is missing", expected.name),
            ));
        };
        if object_type != expected.object_type
            || table_name != expected.table_name
            || sql != expected.sql
        {
            return Err(invalid_metadata(
                path,
                format!(
                    "runtime schema object {:?} does not match revision one",
                    expected.name
                ),
            ));
        }
    }
    Ok(())
}

/// Typed lifecycle operations over one caller-owned SQLite transaction.
pub(super) struct LifecycleTransaction<'transaction, 'connection> {
    transaction: &'transaction Transaction<'connection>,
    path: &'transaction Path,
    store_id: Uuid,
}

impl<'transaction, 'connection> LifecycleTransaction<'transaction, 'connection> {
    pub(super) fn new(
        transaction: &'transaction Transaction<'connection>,
        path: &'transaction Path,
        store_id: Uuid,
    ) -> Self {
        Self {
            transaction,
            path,
            store_id,
        }
    }

    pub(super) fn store_id(&self) -> Uuid {
        self.store_id
    }

    pub(super) fn sqlite_transaction(&self) -> &Transaction<'connection> {
        self.transaction
    }

    pub(super) fn path(&self) -> &Path {
        self.path
    }

    pub(super) fn begin_logical_request(
        &self,
        candidate: NewLogicalRequest,
    ) -> Result<BeginDisposition, RuntimeStoreError> {
        let begun_at = checked_sql_integer(
            candidate.begun_at_unix_ms,
            "logical request",
            candidate.id.to_string(),
            "begun_at_unix_ms",
        )?;
        let store_id = self.store_id.to_string();
        let inserted = self
            .transaction
            .execute(
                "INSERT INTO logical_requests (
                    store_id, logical_request_id, begun_at_unix_ms
                 ) VALUES (?1, ?2, ?3)
                 ON CONFLICT(store_id, logical_request_id) DO NOTHING",
                params![
                    store_id,
                    candidate.id.as_uuid().as_bytes().as_slice(),
                    begun_at
                ],
            )
            .map_err(|source| sqlite_error(self.path, "begin logical request", source))?;
        if inserted == 1 {
            return Ok(BeginDisposition::Inserted);
        }

        let existing =
            read_logical_request(self.transaction, self.path, self.store_id, candidate.id)?
                .ok_or_else(|| {
                    invariant(
                        "logical request",
                        candidate.id,
                        "conflict was reported but the row is missing",
                    )
                })?;
        if existing.request == candidate {
            Ok(BeginDisposition::AlreadyExists)
        } else {
            Err(invariant(
                "logical request",
                candidate.id,
                format!(
                    "begin facts conflict: existing={:?}, proposed={candidate:?}",
                    existing.request
                ),
            ))
        }
    }

    pub(super) fn begin_attempt(
        &self,
        candidate: NewAttempt,
    ) -> Result<BeginAttemptResult, RuntimeStoreError> {
        let evidence_json =
            encode_attempt_pending_evidence(&candidate.evidence).map_err(|error| {
                invariant(
                    "upstream attempt",
                    candidate.id,
                    format!("pending evidence is invalid: {error}"),
                )
            })?;
        if let Some(existing) =
            read_attempt(self.transaction, self.path, self.store_id, candidate.id)?
        {
            if existing.attempt == candidate {
                return Ok(BeginAttemptResult {
                    disposition: BeginDisposition::AlreadyExists,
                    attempt_ordinal: existing.attempt_ordinal,
                });
            }
            return Err(invariant(
                "upstream attempt",
                candidate.id,
                format!(
                    "begin facts conflict: existing={:?}, proposed={candidate:?}",
                    existing.attempt
                ),
            ));
        }

        let logical = read_logical_request(
            self.transaction,
            self.path,
            self.store_id,
            candidate.logical_request_id,
        )?
        .ok_or_else(|| {
            invariant(
                "logical request",
                candidate.logical_request_id,
                format!("parent of attempt {} is missing", candidate.id),
            )
        })?;
        if logical.terminal.is_some() {
            return Err(invariant(
                "logical request",
                candidate.logical_request_id,
                format!("cannot begin attempt {} after terminal", candidate.id),
            ));
        }

        let store_id = self.store_id.to_string();
        let pending_attempt: Option<Vec<u8>> = self
            .transaction
            .query_row(
                "SELECT attempt_id
                 FROM upstream_attempts
                 WHERE store_id = ?1
                   AND logical_request_id = ?2
                   AND terminal_outcome IS NULL
                 LIMIT 1",
                params![
                    store_id,
                    candidate.logical_request_id.as_uuid().as_bytes().as_slice()
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| sqlite_error(self.path, "find pending upstream attempt", source))?;
        if let Some(bytes) = pending_attempt {
            let pending_id = decode_attempt_id(self.path, bytes, "pending attempt ID")?;
            return Err(invariant(
                "logical request",
                candidate.logical_request_id,
                format!("attempt {pending_id} is already pending"),
            ));
        }

        let previous_ordinal: i64 = self
            .transaction
            .query_row(
                "SELECT COALESCE(MAX(attempt_ordinal), 0)
                 FROM upstream_attempts
                 WHERE store_id = ?1 AND logical_request_id = ?2",
                params![
                    store_id,
                    candidate.logical_request_id.as_uuid().as_bytes().as_slice()
                ],
                |row| row.get(0),
            )
            .map_err(|source| sqlite_error(self.path, "read latest attempt ordinal", source))?;
        let attempt_ordinal = previous_ordinal.checked_add(1).ok_or_else(|| {
            invariant(
                "logical request",
                candidate.logical_request_id,
                "attempt ordinal overflow",
            )
        })?;
        let begun_at = checked_sql_integer(
            candidate.begun_at_unix_ms,
            "upstream attempt",
            candidate.id.to_string(),
            "begun_at_unix_ms",
        )?;
        let inserted = self
            .transaction
            .execute(
                "INSERT INTO upstream_attempts (
                    store_id,
                    attempt_id,
                    logical_request_id,
                    attempt_ordinal,
                    begun_at_unix_ms,
                    pending_evidence_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(store_id, attempt_id) DO NOTHING",
                params![
                    store_id,
                    candidate.id.as_uuid().as_bytes().as_slice(),
                    candidate.logical_request_id.as_uuid().as_bytes().as_slice(),
                    attempt_ordinal,
                    begun_at,
                    evidence_json
                ],
            )
            .map_err(|source| sqlite_error(self.path, "begin upstream attempt", source))?;

        if inserted == 0 {
            let existing = read_attempt(self.transaction, self.path, self.store_id, candidate.id)?
                .ok_or_else(|| {
                    invariant(
                        "upstream attempt",
                        candidate.id,
                        "conflict was reported but the row is missing",
                    )
                })?;
            if existing.attempt == candidate {
                return Ok(BeginAttemptResult {
                    disposition: BeginDisposition::AlreadyExists,
                    attempt_ordinal: existing.attempt_ordinal,
                });
            }
            return Err(invariant(
                "upstream attempt",
                candidate.id,
                format!(
                    "begin facts conflict: existing={:?}, proposed={candidate:?}",
                    existing.attempt
                ),
            ));
        }
        if inserted != 1 {
            return Err(invariant(
                "upstream attempt",
                candidate.id,
                format!("begin inserted an unexpected {inserted} rows"),
            ));
        }

        Ok(BeginAttemptResult {
            disposition: BeginDisposition::Inserted,
            attempt_ordinal: u64::try_from(attempt_ordinal).map_err(|_| {
                invariant(
                    "upstream attempt",
                    candidate.id,
                    "persisted attempt ordinal is negative",
                )
            })?,
        })
    }

    pub(super) fn commit_attempt_terminal(
        &self,
        attempt_id: AttemptId,
        terminal: AttemptTerminal,
    ) -> Result<TerminalDisposition, RuntimeStoreError> {
        let terminal_at = checked_sql_integer(
            terminal.terminal_at_unix_ms,
            "upstream attempt",
            attempt_id.to_string(),
            "terminal_at_unix_ms",
        )?;
        let store_id = self.store_id.to_string();
        let updated = self
            .transaction
            .execute(
                "UPDATE upstream_attempts
                 SET terminal_outcome = ?3,
                     terminal_at_unix_ms = ?4,
                     economics_state = ?5,
                     terminal_origin = 'runtime',
                     recovery_run_id = NULL
                 WHERE store_id = ?1
                   AND attempt_id = ?2
                   AND terminal_outcome IS NULL
                   AND EXISTS (
                       SELECT 1
                       FROM logical_requests
                       WHERE logical_requests.store_id = upstream_attempts.store_id
                         AND logical_requests.logical_request_id = upstream_attempts.logical_request_id
                         AND logical_requests.terminal_outcome IS NULL
                   )",
                params![
                    store_id,
                    attempt_id.as_uuid().as_bytes().as_slice(),
                    terminal.outcome.as_str(),
                    terminal_at,
                    terminal.economics_state.as_str()
                ],
            )
            .map_err(|source| sqlite_error(self.path, "commit upstream attempt terminal", source))?;
        if updated == 1 {
            return Ok(TerminalDisposition::Committed);
        }

        let existing = read_attempt(self.transaction, self.path, self.store_id, attempt_id)?
            .ok_or_else(|| invariant("upstream attempt", attempt_id, "attempt is missing"))?;
        let expected = AttemptTerminalRecord {
            terminal,
            origin: TerminalOrigin::Runtime,
            recovery_run_id: None,
        };
        match existing.terminal {
            Some(actual) if actual == expected => Ok(TerminalDisposition::AlreadyIdentical),
            Some(actual) => Err(invariant(
                "upstream attempt",
                attempt_id,
                format!("conflicting terminal: existing={actual:?}, proposed={expected:?}"),
            )),
            None => Err(invariant(
                "upstream attempt",
                attempt_id,
                format!(
                    "attempt is pending but logical request {} is terminal",
                    existing.attempt.logical_request_id
                ),
            )),
        }
    }

    pub(super) fn commit_logical_request_terminal(
        &self,
        logical_request_id: LogicalRequestId,
        terminal: LogicalRequestTerminal,
    ) -> Result<TerminalDisposition, RuntimeStoreError> {
        let payload = terminal.payload.as_ref().ok_or_else(|| {
            invariant(
                "logical request",
                logical_request_id,
                "runtime terminal payload is required",
            )
        })?;
        self.validate_winning_attempt(logical_request_id, &terminal, payload)?;
        let payload_json = encode_logical_terminal_payload(payload).map_err(|error| {
            invariant(
                "logical request",
                logical_request_id,
                format!("terminal payload is invalid: {error}"),
            )
        })?;
        let terminal_at = checked_sql_integer(
            terminal.terminal_at_unix_ms,
            "logical request",
            logical_request_id.to_string(),
            "terminal_at_unix_ms",
        )?;
        let request_id = checked_sql_integer(
            payload.finished_request.id,
            "logical request",
            logical_request_id.to_string(),
            "numeric request ID",
        )?;
        let service_name = payload.finished_request.service.trim();
        if service_name.is_empty() {
            return Err(invariant(
                "logical request",
                logical_request_id,
                "terminal service name is empty",
            ));
        }
        let store_id = self.store_id.to_string();
        let updated = self
            .transaction
            .execute(
                "UPDATE logical_requests
                 SET terminal_outcome = ?3,
                     terminal_at_unix_ms = ?4,
                     economics_state = ?5,
                     terminal_origin = 'runtime',
                     recovery_run_id = NULL,
                     terminal_payload_json = ?6,
                     service_name = ?7,
                     numeric_request_id = ?8,
                     trace_id = ?9,
                     session_id = ?10
                 WHERE store_id = ?1
                   AND logical_request_id = ?2
                   AND terminal_outcome IS NULL
                   AND NOT EXISTS (
                       SELECT 1
                       FROM upstream_attempts
                       WHERE upstream_attempts.store_id = logical_requests.store_id
                         AND upstream_attempts.logical_request_id = logical_requests.logical_request_id
                         AND upstream_attempts.terminal_outcome IS NULL
                   )",
                params![
                    store_id,
                    logical_request_id.as_uuid().as_bytes().as_slice(),
                    terminal.outcome.as_str(),
                    terminal_at,
                    terminal.economics_state.as_str(),
                    payload_json,
                    service_name,
                    request_id,
                    payload.finished_request.trace_id.as_deref(),
                    payload.finished_request.session_id.as_deref()
                ],
            )
            .map_err(|source| sqlite_error(self.path, "commit logical request terminal", source))?;
        if updated == 1 {
            return Ok(TerminalDisposition::Committed);
        }

        let existing = read_logical_request(
            self.transaction,
            self.path,
            self.store_id,
            logical_request_id,
        )?
        .ok_or_else(|| invariant("logical request", logical_request_id, "request is missing"))?;
        let existing_payload_json: Option<String> = self
            .transaction
            .query_row(
                "SELECT terminal_payload_json
                 FROM logical_requests
                 WHERE store_id = ?1 AND logical_request_id = ?2",
                params![store_id, logical_request_id.as_uuid().as_bytes().as_slice()],
                |row| row.get(0),
            )
            .map_err(|source| {
                sqlite_error(self.path, "read existing logical terminal payload", source)
            })?;
        match existing.terminal {
            Some(actual) => {
                let envelope_identical = actual.origin == TerminalOrigin::Runtime
                    && actual.recovery_run_id.is_none()
                    && actual.terminal.outcome == terminal.outcome
                    && actual.terminal.terminal_at_unix_ms == terminal.terminal_at_unix_ms
                    && actual.terminal.economics_state == terminal.economics_state;
                if envelope_identical
                    && existing_payload_json.as_deref() == Some(payload_json.as_str())
                {
                    Ok(TerminalDisposition::AlreadyIdentical)
                } else {
                    let detail = if envelope_identical {
                        "conflicting terminal payload"
                    } else {
                        "conflicting terminal envelope"
                    };
                    Err(invariant("logical request", logical_request_id, detail))
                }
            }
            None => {
                let pending_attempt: Option<Vec<u8>> = self
                    .transaction
                    .query_row(
                        "SELECT attempt_id
                         FROM upstream_attempts
                         WHERE store_id = ?1
                           AND logical_request_id = ?2
                           AND terminal_outcome IS NULL
                         LIMIT 1",
                        params![store_id, logical_request_id.as_uuid().as_bytes().as_slice()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|source| {
                        sqlite_error(self.path, "find blocking pending attempt", source)
                    })?;
                if let Some(bytes) = pending_attempt {
                    let attempt_id = decode_attempt_id(self.path, bytes, "pending attempt ID")?;
                    Err(invariant(
                        "logical request",
                        logical_request_id,
                        format!("attempt {attempt_id} is still pending"),
                    ))
                } else {
                    Err(invariant(
                        "logical request",
                        logical_request_id,
                        "conditional terminal update did not change a pending row",
                    ))
                }
            }
        }
    }

    fn validate_winning_attempt(
        &self,
        logical_request_id: LogicalRequestId,
        terminal: &LogicalRequestTerminal,
        payload: &LogicalRequestTerminalPayload,
    ) -> Result<(), RuntimeStoreError> {
        let store_id = self.store_id.to_string();
        let attempt_count: i64 = self
            .transaction
            .query_row(
                "SELECT COUNT(*)
                 FROM upstream_attempts
                 WHERE store_id = ?1 AND logical_request_id = ?2",
                params![store_id, logical_request_id.as_uuid().as_bytes().as_slice()],
                |row| row.get(0),
            )
            .map_err(|source| {
                sqlite_error(self.path, "count attempts before logical terminal", source)
            })?;

        let Some(winning_attempt_id) = payload.winning_attempt_id else {
            if terminal.outcome == LogicalRequestOutcome::Succeeded && attempt_count > 0 {
                return Err(invariant(
                    "logical request",
                    logical_request_id,
                    "successful request with upstream attempts is missing winning_attempt_id",
                ));
            }
            return Ok(());
        };
        if terminal.outcome != LogicalRequestOutcome::Succeeded {
            return Err(invariant(
                "logical request",
                logical_request_id,
                "non-successful request must not carry winning_attempt_id",
            ));
        }

        let attempt = read_attempt(
            self.transaction,
            self.path,
            self.store_id,
            winning_attempt_id,
        )?
        .ok_or_else(|| {
            invariant(
                "logical request",
                logical_request_id,
                format!("winning attempt {winning_attempt_id} is missing"),
            )
        })?;
        if attempt.attempt.logical_request_id != logical_request_id {
            return Err(invariant(
                "logical request",
                logical_request_id,
                format!(
                    "winning attempt {winning_attempt_id} belongs to logical request {}",
                    attempt.attempt.logical_request_id
                ),
            ));
        }
        if i64::try_from(attempt.attempt_ordinal).ok() != Some(attempt_count) {
            return Err(invariant(
                "logical request",
                logical_request_id,
                format!(
                    "winning attempt {winning_attempt_id} has ordinal {}, expected final ordinal {attempt_count}",
                    attempt.attempt_ordinal
                ),
            ));
        }
        if payload.runtime_revision != attempt.attempt.evidence.runtime_revision
            || payload.runtime_digest != attempt.attempt.evidence.runtime_digest
        {
            return Err(invariant(
                "logical request",
                logical_request_id,
                format!(
                    "winning attempt {winning_attempt_id} runtime evidence conflicts with terminal payload"
                ),
            ));
        }
        let route = &attempt.attempt.evidence.route;
        if payload.provider_epoch != attempt.attempt.evidence.provider_epoch
            || payload.mapped_model != route.mapped_model
        {
            return Err(invariant(
                "logical request",
                logical_request_id,
                format!(
                    "winning attempt {winning_attempt_id} provider epoch evidence conflicts with terminal payload"
                ),
            ));
        }
        let attempt_terminal = attempt.terminal.ok_or_else(|| {
            invariant(
                "logical request",
                logical_request_id,
                format!("winning attempt {winning_attempt_id} is still pending"),
            )
        })?;
        if attempt_terminal.terminal.outcome != AttemptOutcome::Succeeded {
            return Err(invariant(
                "logical request",
                logical_request_id,
                format!("winning attempt {winning_attempt_id} did not succeed"),
            ));
        }
        Ok(())
    }

    pub(super) fn recover_startup(
        &self,
        run_id: RecoveryRunId,
        recovered_at_unix_ms: u64,
    ) -> Result<RecoveryReport, RuntimeStoreError> {
        if let Some(existing) =
            read_recovery_report(self.transaction, self.path, self.store_id, run_id)?
        {
            if existing.recovered_at_unix_ms != recovered_at_unix_ms {
                return Err(invariant(
                    "recovery run",
                    run_id,
                    format!(
                        "recovery timestamp conflicts: existing={}, proposed={recovered_at_unix_ms}",
                        existing.recovered_at_unix_ms
                    ),
                ));
            }
            if self.has_pending_lifecycle()? {
                return Err(invariant(
                    "recovery run",
                    run_id,
                    "an already committed recovery run cannot be reused while rows are pending",
                ));
            }
            return Ok(existing);
        }

        self.reject_terminal_request_with_pending_attempt()?;

        let recovered_at = checked_sql_integer(
            recovered_at_unix_ms,
            "recovery run",
            run_id.to_string(),
            "recovered_at_unix_ms",
        )?;
        let store_id = self.store_id.to_string();
        let previous_ordinal: i64 = self
            .transaction
            .query_row(
                "SELECT COALESCE(MAX(recovery_ordinal), 0)
                 FROM recovery_runs
                 WHERE store_id = ?1",
                params![store_id],
                |row| row.get(0),
            )
            .map_err(|source| sqlite_error(self.path, "read latest recovery ordinal", source))?;
        let recovery_ordinal = previous_ordinal
            .checked_add(1)
            .ok_or_else(|| invariant("recovery run", run_id, "recovery ordinal overflow"))?;
        self.transaction
            .execute(
                "INSERT INTO recovery_runs (
                    store_id,
                    recovery_run_id,
                    recovery_ordinal,
                    recovered_at_unix_ms,
                    interrupted_logical_count,
                    interrupted_attempt_count
                 ) VALUES (?1, ?2, ?3, ?4, 0, 0)",
                params![
                    store_id,
                    run_id.as_uuid().as_bytes().as_slice(),
                    recovery_ordinal,
                    recovered_at
                ],
            )
            .map_err(|source| sqlite_error(self.path, "begin startup recovery run", source))?;

        let interrupted_attempt_count = self
            .transaction
            .execute(
                "UPDATE upstream_attempts
                 SET terminal_outcome = 'interrupted',
                     terminal_at_unix_ms = ?3,
                     economics_state = 'unknown',
                     terminal_origin = 'startup_recovery',
                     recovery_run_id = ?2
                 WHERE store_id = ?1 AND terminal_outcome IS NULL",
                params![
                    store_id,
                    run_id.as_uuid().as_bytes().as_slice(),
                    recovered_at
                ],
            )
            .map_err(|source| sqlite_error(self.path, "interrupt stranded attempts", source))?;
        let interrupted_logical_count = self
            .transaction
            .execute(
                "UPDATE logical_requests
                 SET terminal_outcome = 'interrupted',
                     terminal_at_unix_ms = ?3,
                     economics_state = 'unknown',
                     terminal_origin = 'startup_recovery',
                     recovery_run_id = ?2,
                     terminal_payload_json = NULL
                 WHERE store_id = ?1 AND terminal_outcome IS NULL",
                params![
                    store_id,
                    run_id.as_uuid().as_bytes().as_slice(),
                    recovered_at
                ],
            )
            .map_err(|source| {
                sqlite_error(self.path, "interrupt stranded logical requests", source)
            })?;

        let interrupted_attempt_count_i64 =
            i64::try_from(interrupted_attempt_count).map_err(|_| {
                invariant(
                    "recovery run",
                    run_id,
                    "interrupted attempt count exceeds SQLite integer range",
                )
            })?;
        let interrupted_logical_count_i64 =
            i64::try_from(interrupted_logical_count).map_err(|_| {
                invariant(
                    "recovery run",
                    run_id,
                    "interrupted logical count exceeds SQLite integer range",
                )
            })?;
        let updated = self
            .transaction
            .execute(
                "UPDATE recovery_runs
                 SET interrupted_logical_count = ?3,
                     interrupted_attempt_count = ?4
                 WHERE store_id = ?1 AND recovery_run_id = ?2",
                params![
                    store_id,
                    run_id.as_uuid().as_bytes().as_slice(),
                    interrupted_logical_count_i64,
                    interrupted_attempt_count_i64
                ],
            )
            .map_err(|source| sqlite_error(self.path, "finish startup recovery run", source))?;
        if updated != 1 || self.has_pending_lifecycle()? {
            return Err(invariant(
                "recovery run",
                run_id,
                "startup recovery did not reach a terminal lifecycle state",
            ));
        }

        Ok(RecoveryReport {
            store_id: self.store_id,
            run_id,
            recovery_ordinal: u64::try_from(recovery_ordinal).map_err(|_| {
                invariant(
                    "recovery run",
                    run_id,
                    "persisted recovery ordinal is not positive",
                )
            })?,
            recovered_at_unix_ms,
            interrupted_logical_count: interrupted_logical_count as u64,
            interrupted_attempt_count: interrupted_attempt_count as u64,
        })
    }

    fn reject_terminal_request_with_pending_attempt(&self) -> Result<(), RuntimeStoreError> {
        let store_id = self.store_id.to_string();
        let invalid: Option<(Vec<u8>, Vec<u8>)> = self
            .transaction
            .query_row(
                "SELECT attempts.attempt_id, attempts.logical_request_id
                 FROM upstream_attempts AS attempts
                 JOIN logical_requests AS requests
                   ON requests.store_id = attempts.store_id
                  AND requests.logical_request_id = attempts.logical_request_id
                 WHERE attempts.store_id = ?1
                   AND attempts.terminal_outcome IS NULL
                   AND requests.terminal_outcome IS NOT NULL
                 LIMIT 1",
                params![store_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|source| {
                sqlite_error(self.path, "check startup lifecycle invariants", source)
            })?;
        if let Some((attempt_bytes, logical_bytes)) = invalid {
            let attempt_id = decode_attempt_id(self.path, attempt_bytes, "attempt ID")?;
            let logical_request_id =
                decode_logical_request_id(self.path, logical_bytes, "logical request ID")?;
            return Err(invariant(
                "upstream attempt",
                attempt_id,
                format!("is pending after logical request {logical_request_id} became terminal"),
            ));
        }
        Ok(())
    }

    fn has_pending_lifecycle(&self) -> Result<bool, RuntimeStoreError> {
        let store_id = self.store_id.to_string();
        self.transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM logical_requests
                    WHERE store_id = ?1 AND terminal_outcome IS NULL
                 ) OR EXISTS(
                    SELECT 1 FROM upstream_attempts
                    WHERE store_id = ?1 AND terminal_outcome IS NULL
                 )",
                params![store_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|source| sqlite_error(self.path, "check pending lifecycle rows", source))
    }
}

/// Reads one logical request lifecycle record.
pub(super) fn read_logical_request(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
    logical_request_id: LogicalRequestId,
) -> Result<Option<LogicalRequestRecord>, RuntimeStoreError> {
    let store_id_text = store_id.to_string();
    let raw = connection
        .query_row(
            "SELECT begun_at_unix_ms,
                    terminal_outcome,
                    terminal_at_unix_ms,
                    economics_state,
                    terminal_origin,
                    recovery_run_id,
                    terminal_payload_json
             FROM logical_requests
             WHERE store_id = ?1 AND logical_request_id = ?2",
            params![
                store_id_text,
                logical_request_id.as_uuid().as_bytes().as_slice()
            ],
            |row| {
                Ok(RawTerminalRow {
                    begun_at_unix_ms: row.get(0)?,
                    terminal_outcome: row.get(1)?,
                    terminal_at_unix_ms: row.get(2)?,
                    economics_state: row.get(3)?,
                    terminal_origin: row.get(4)?,
                    recovery_run_id: row.get(5)?,
                    terminal_payload_json: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|source| sqlite_error(path, "read logical request lifecycle", source))?;
    raw.map(|raw| decode_logical_request(path, store_id, logical_request_id, raw))
        .transpose()
}

/// Reads the most recently begun logical request lifecycle records.
pub(super) fn read_recent_logical_requests(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
    limit: u64,
) -> Result<Vec<LogicalRequestRecord>, RuntimeStoreError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let limit = checked_sql_integer(
        limit,
        "logical request query",
        store_id.to_string(),
        "limit",
    )?;
    let store_id_text = store_id.to_string();
    let mut statement = connection
        .prepare(
            "SELECT logical_request_id,
                    begun_at_unix_ms,
                    terminal_outcome,
                    terminal_at_unix_ms,
                    economics_state,
                    terminal_origin,
                    recovery_run_id,
                    terminal_payload_json
             FROM logical_requests
             WHERE store_id = ?1
             ORDER BY begun_at_unix_ms DESC, logical_request_id DESC
             LIMIT ?2",
        )
        .map_err(|source| sqlite_error(path, "prepare recent logical requests", source))?;
    let rows = statement
        .query_map(params![store_id_text, limit], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                RawTerminalRow {
                    begun_at_unix_ms: row.get(1)?,
                    terminal_outcome: row.get(2)?,
                    terminal_at_unix_ms: row.get(3)?,
                    economics_state: row.get(4)?,
                    terminal_origin: row.get(5)?,
                    recovery_run_id: row.get(6)?,
                    terminal_payload_json: row.get(7)?,
                },
            ))
        })
        .map_err(|source| sqlite_error(path, "read recent logical requests", source))?;
    let mut records = Vec::new();
    for row in rows {
        let (logical_request_id, raw) =
            row.map_err(|source| sqlite_error(path, "decode recent logical request row", source))?;
        let logical_request_id =
            decode_logical_request_id(path, logical_request_id, "logical request ID")?;
        records.push(decode_logical_request(
            path,
            store_id,
            logical_request_id,
            raw,
        )?);
    }
    Ok(records)
}

/// Reads decoded runtime terminals in stable keyset order.
pub(super) fn query_committed_requests(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
    query: &CommittedRequestQuery,
) -> Result<CommittedRequestPage, RuntimeStoreError> {
    if query.limit == 0 {
        return Ok(CommittedRequestPage {
            items: Vec::new(),
            next_cursor: None,
        });
    }

    let cursor = match query.cursor {
        Some(cursor) => Some((
            checked_sql_integer(
                cursor.terminal_at_unix_ms,
                "committed request query",
                store_id.to_string(),
                "cursor terminal_at_unix_ms",
            )?,
            cursor.logical_request_id.as_uuid().as_bytes().to_vec(),
        )),
        None => None,
    };
    let cutoff = query
        .terminal_at_or_after_unix_ms
        .map(|cutoff| {
            checked_sql_integer(
                cutoff,
                "committed request query",
                store_id.to_string(),
                "terminal cutoff",
            )
        })
        .transpose()?;
    let store_id_text = store_id.to_string();
    let scan_budget = if query.filter == super::CommittedRequestFilter::default() {
        query.limit
    } else {
        FILTERED_PROJECTION_SCAN_ROWS
    };
    let sql_limit = scan_budget
        .checked_add(1)
        .and_then(|limit| i64::try_from(limit).ok())
        .ok_or_else(|| RuntimeStoreError::InvariantViolation {
            entity: "committed request query",
            id: store_id.to_string(),
            detail: "limit exceeds SQLite integer range".to_string(),
        })?;
    let sql = match (cursor.is_some(), cutoff.is_some()) {
        (false, false) => COMMITTED_REQUESTS_FIRST_PAGE_SQL,
        (true, false) => COMMITTED_REQUESTS_AFTER_CURSOR_SQL,
        (false, true) => COMMITTED_REQUESTS_SINCE_FIRST_PAGE_SQL,
        (true, true) => COMMITTED_REQUESTS_SINCE_AFTER_CURSOR_SQL,
    };
    let mut statement = connection
        .prepare(sql)
        .map_err(|source| sqlite_error(path, "prepare committed request projection", source))?;
    let mut rows = match (cursor.as_ref(), cutoff) {
        (Some((cursor_time, cursor_id)), None) => {
            statement.query(params![store_id_text, cursor_time, cursor_id, sql_limit])
        }
        (None, None) => statement.query(params![store_id_text, sql_limit]),
        (None, Some(cutoff)) => statement.query(params![store_id_text, cutoff, sql_limit]),
        (Some((cursor_time, cursor_id)), Some(cutoff)) => statement.query(params![
            store_id_text,
            cutoff,
            cursor_time,
            cursor_id,
            sql_limit
        ]),
    }
    .map_err(|source| sqlite_error(path, "read committed request projection", source))?;

    let mut items = Vec::with_capacity(query.limit.min(DEFAULT_PROJECTION_CAPACITY));
    let mut has_more = false;
    let mut scanned_rows = 0_usize;
    let mut last_scanned = None;
    while let Some(row) = rows
        .next()
        .map_err(|source| sqlite_error(path, "read committed request projection", source))?
    {
        if scanned_rows == scan_budget || items.len() == query.limit {
            has_more = true;
            break;
        }
        let (logical_request_id, begun_at, outcome, terminal_at, economics_state, payload_json) =
            read_committed_request_projection_row(row).map_err(|source| {
                sqlite_error(path, "decode committed request projection row", source)
            })?;
        let logical_request_id =
            decode_logical_request_id(path, logical_request_id, "logical request ID")?;
        let projection = decode_committed_request_projection(
            path,
            logical_request_id,
            begun_at,
            outcome,
            terminal_at,
            economics_state,
            payload_json,
        )?;
        scanned_rows = scanned_rows.saturating_add(1);
        last_scanned = Some(CommittedRequestCursor {
            terminal_at_unix_ms: projection.terminal_at_unix_ms,
            logical_request_id,
        });
        if !committed_payload_matches_filter(&projection.payload, &query.filter) {
            continue;
        }
        items.push(projection);
    }

    let next_cursor = if has_more { last_scanned } else { None };
    Ok(CommittedRequestPage { items, next_cursor })
}

/// Reads a request chain through exact typed identities rather than terminal JSON scans.
pub(super) fn query_committed_request_identities(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
    query: &CommittedRequestIdentityQuery,
) -> Result<Vec<CommittedRequestProjection>, RuntimeStoreError> {
    if query.limit == 0 {
        return Ok(Vec::new());
    }
    let service = query.service.trim();
    if service.is_empty() {
        return Err(invariant(
            "committed request identity query",
            store_id,
            "service is empty",
        ));
    }
    if query.trace_id.is_none() && query.request_id.is_none() && query.session_id.is_none() {
        return Err(invariant(
            "committed request identity query",
            store_id,
            "trace ID, request ID, or session ID is required",
        ));
    }
    let limit = i64::try_from(query.limit).map_err(|_| RuntimeStoreError::InvariantViolation {
        entity: "committed request identity query",
        id: store_id.to_string(),
        detail: "limit exceeds SQLite integer range".to_string(),
    })?;

    let mut sql = String::from(
        "SELECT logical_request_id,
                begun_at_unix_ms,
                terminal_outcome,
                terminal_at_unix_ms,
                economics_state,
                terminal_payload_json
         FROM logical_requests
         WHERE store_id = ?1
           AND terminal_origin = 'runtime'
           AND terminal_payload_json IS NOT NULL
           AND service_name = ?2",
    );
    let mut parameters = vec![
        Value::Text(store_id.to_string()),
        Value::Text(service.to_string()),
    ];
    if let Some(trace_id) = query.trace_id.as_ref() {
        parameters.push(Value::Text(trace_id.clone()));
        sql.push_str(&format!(" AND trace_id = ?{}", parameters.len()));
    }
    if let Some(request_id) = query.request_id {
        parameters.push(Value::Integer(checked_sql_integer(
            request_id,
            "committed request identity query",
            store_id.to_string(),
            "request ID",
        )?));
        sql.push_str(&format!(" AND numeric_request_id = ?{}", parameters.len()));
    }
    if let Some(session_id) = query.session_id.as_ref() {
        parameters.push(Value::Text(session_id.clone()));
        sql.push_str(&format!(" AND session_id = ?{}", parameters.len()));
    }
    parameters.push(Value::Integer(limit));
    sql.push_str(&format!(
        " ORDER BY terminal_at_unix_ms DESC, logical_request_id DESC LIMIT ?{}",
        parameters.len()
    ));

    let mut statement = connection
        .prepare(&sql)
        .map_err(|source| sqlite_error(path, "prepare committed request identity query", source))?;
    let mut rows = statement
        .query(params_from_iter(parameters.iter()))
        .map_err(|source| sqlite_error(path, "read committed request identity query", source))?;
    let mut items = Vec::with_capacity(query.limit.min(DEFAULT_PROJECTION_CAPACITY));
    while let Some(row) = rows
        .next()
        .map_err(|source| sqlite_error(path, "read committed request identity row", source))?
    {
        let (logical_request_id, begun_at, outcome, terminal_at, economics_state, payload_json) =
            read_committed_request_projection_row(row).map_err(|source| {
                sqlite_error(path, "decode committed request identity row", source)
            })?;
        let logical_request_id =
            decode_logical_request_id(path, logical_request_id, "logical request ID")?;
        items.push(decode_committed_request_projection(
            path,
            logical_request_id,
            begun_at,
            outcome,
            terminal_at,
            economics_state,
            payload_json,
        )?);
    }
    Ok(items)
}

fn read_committed_request_projection_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(Vec<u8>, i64, String, i64, String, String)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

#[allow(clippy::too_many_arguments)]
fn decode_committed_request_projection(
    path: &Path,
    logical_request_id: LogicalRequestId,
    begun_at: i64,
    outcome: String,
    terminal_at: i64,
    economics_state: String,
    payload_json: String,
) -> Result<CommittedRequestProjection, RuntimeStoreError> {
    let payload = decode_logical_terminal_payload(
        path,
        TerminalOrigin::Runtime,
        Some(payload_json.as_str()),
    )?
    .ok_or_else(|| invalid_metadata(path, "runtime terminal payload is missing"))?;
    Ok(CommittedRequestProjection {
        logical_request_id,
        begun_at_unix_ms: decode_nonnegative(path, begun_at, "logical request begun_at_unix_ms")?,
        outcome: LogicalRequestOutcome::parse(path, &outcome)?,
        terminal_at_unix_ms: decode_nonnegative(path, terminal_at, "terminal_at_unix_ms")?,
        economics_state: EconomicsState::parse(path, &economics_state)?,
        payload,
    })
}

#[cfg(test)]
pub(super) fn count_committed_request_terminals(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
) -> Result<u64, RuntimeStoreError> {
    Ok(committed_request_projection_metadata(connection, path, store_id)?.terminal_count)
}

pub(super) fn committed_request_projection_metadata(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
) -> Result<CommittedRequestProjectionMetadata, RuntimeStoreError> {
    let (count, max_numeric_request_id) = connection
        .query_row(
            "SELECT COUNT(*), MAX(numeric_request_id)
             FROM logical_requests
             WHERE store_id = ?1
               AND terminal_origin = 'runtime'
               AND terminal_payload_json IS NOT NULL",
            [store_id.to_string()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .map_err(|source| sqlite_error(path, "read committed request metadata", source))?;
    Ok(CommittedRequestProjectionMetadata {
        terminal_count: decode_nonnegative(path, count, "committed request terminal count")?,
        max_numeric_request_id: max_numeric_request_id
            .map(|request_id| decode_nonnegative(path, request_id, "maximum numeric request ID"))
            .transpose()?,
    })
}

fn committed_payload_matches_filter(
    payload: &LogicalRequestTerminalPayload,
    filter: &super::CommittedRequestFilter,
) -> bool {
    let request = &payload.finished_request;
    if filter
        .service
        .as_ref()
        .is_some_and(|expected| request.service != *expected)
    {
        return false;
    }
    if filter
        .trace_id
        .as_deref()
        .is_some_and(|expected| request.trace_id.as_deref() != Some(expected))
    {
        return false;
    }
    if filter
        .request_id
        .is_some_and(|expected| request.id != expected)
    {
        return false;
    }
    if filter
        .session
        .as_deref()
        .is_some_and(|expected| !filter_text_contains(request.session_id.as_deref(), expected))
    {
        return false;
    }
    if filter.model.as_deref().is_some_and(|expected| {
        ![
            request.model.as_deref(),
            payload.requested_model.as_deref(),
            payload.mapped_model.as_deref(),
            payload.reported_model.as_deref(),
            payload.pricing_model.as_deref(),
        ]
        .into_iter()
        .any(|value| filter_text_contains(value, expected))
    }) {
        return false;
    }
    let route_attempts = request
        .retry
        .as_ref()
        .map(|retry| retry.route_attempts.as_slice())
        .unwrap_or_default();
    if filter.provider_endpoint.as_ref().is_some_and(|expected| {
        super::final_route_provider_endpoint(request).as_ref() != Some(expected)
    }) {
        return false;
    }
    if filter.provider.as_deref().is_some_and(|expected| {
        !filter_text_contains(super::final_route_provider_id(request), expected)
    }) {
        return false;
    }
    if filter
        .path
        .as_deref()
        .is_some_and(|expected| !filter_text_contains(Some(&request.path), expected))
    {
        return false;
    }
    if filter.signal_kind.as_deref().is_some_and(|expected| {
        !request.provider_signals.iter().any(|signal| {
            filter_text_contains(Some(signal.stable_code()), expected)
                || filter_text_contains(Some(signal.kind.code()), expected)
        }) && !route_attempts.iter().any(|attempt| {
            attempt.provider_signals.iter().any(|signal| {
                filter_text_contains(Some(signal.stable_code()), expected)
                    || filter_text_contains(Some(signal.kind.code()), expected)
            })
        })
    }) {
        return false;
    }
    if filter
        .policy_action_kind
        .as_deref()
        .is_some_and(|expected| {
            !request.policy_actions.iter().any(|action| {
                filter_text_contains(Some(action.stable_code()), expected)
                    || filter_text_contains(Some(action.kind.code()), expected)
            }) && !route_attempts.iter().any(|attempt| {
                attempt.policy_actions.iter().any(|action| {
                    filter_text_contains(Some(action.stable_code()), expected)
                        || filter_text_contains(Some(action.kind.code()), expected)
                })
            })
        })
    {
        return false;
    }
    if filter
        .status_min
        .is_some_and(|minimum| u64::from(request.status_code) < minimum)
        || filter
            .status_max
            .is_some_and(|maximum| u64::from(request.status_code) > maximum)
    {
        return false;
    }
    if filter.fast && !request.observability_view().fast_mode {
        return false;
    }
    if filter.retried && !request.observability_view().retried {
        return false;
    }
    true
}

fn filter_text_contains(value: Option<&str>, expected: &str) -> bool {
    let expected = expected.trim();
    if expected.is_empty() {
        return true;
    }
    value.is_some_and(|value| {
        value
            .to_ascii_lowercase()
            .contains(&expected.to_ascii_lowercase())
    })
}

/// Reads one upstream attempt lifecycle record.
pub(super) fn read_attempt(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
    attempt_id: AttemptId,
) -> Result<Option<AttemptRecord>, RuntimeStoreError> {
    let store_id_text = store_id.to_string();
    let raw = connection
        .query_row(
            "SELECT logical_request_id,
                    attempt_ordinal,
                    begun_at_unix_ms,
                    pending_evidence_json,
                    terminal_outcome,
                    terminal_at_unix_ms,
                    economics_state,
                    terminal_origin,
                    recovery_run_id
             FROM upstream_attempts
             WHERE store_id = ?1 AND attempt_id = ?2",
            params![store_id_text, attempt_id.as_uuid().as_bytes().as_slice()],
            |row| {
                Ok(RawAttemptRow {
                    logical_request_id: row.get(0)?,
                    attempt_ordinal: row.get(1)?,
                    pending_evidence_json: row.get(3)?,
                    terminal: RawTerminalRow {
                        begun_at_unix_ms: row.get(2)?,
                        terminal_outcome: row.get(4)?,
                        terminal_at_unix_ms: row.get(5)?,
                        economics_state: row.get(6)?,
                        terminal_origin: row.get(7)?,
                        recovery_run_id: row.get(8)?,
                        terminal_payload_json: None,
                    },
                })
            },
        )
        .optional()
        .map_err(|source| sqlite_error(path, "read upstream attempt lifecycle", source))?;
    raw.map(|raw| decode_attempt(path, store_id, attempt_id, raw))
        .transpose()
}

/// Reads all upstream attempts for one logical request in attempt order.
pub(super) fn read_attempts_for_logical_request(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
    logical_request_id: LogicalRequestId,
) -> Result<Vec<AttemptRecord>, RuntimeStoreError> {
    let store_id_text = store_id.to_string();
    let mut statement = connection
        .prepare(
            "SELECT attempt_id,
                    logical_request_id,
                    attempt_ordinal,
                    begun_at_unix_ms,
                    pending_evidence_json,
                    terminal_outcome,
                    terminal_at_unix_ms,
                    economics_state,
                    terminal_origin,
                    recovery_run_id
             FROM upstream_attempts
             WHERE store_id = ?1 AND logical_request_id = ?2
             ORDER BY attempt_ordinal ASC",
        )
        .map_err(|source| sqlite_error(path, "prepare attempts for logical request", source))?;
    let rows = statement
        .query_map(
            params![
                store_id_text,
                logical_request_id.as_uuid().as_bytes().as_slice()
            ],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    RawAttemptRow {
                        logical_request_id: row.get(1)?,
                        attempt_ordinal: row.get(2)?,
                        pending_evidence_json: row.get(4)?,
                        terminal: RawTerminalRow {
                            begun_at_unix_ms: row.get(3)?,
                            terminal_outcome: row.get(5)?,
                            terminal_at_unix_ms: row.get(6)?,
                            economics_state: row.get(7)?,
                            terminal_origin: row.get(8)?,
                            recovery_run_id: row.get(9)?,
                            terminal_payload_json: None,
                        },
                    },
                ))
            },
        )
        .map_err(|source| sqlite_error(path, "read attempts for logical request", source))?;
    let mut records = Vec::new();
    for row in rows {
        let (attempt_id, raw) =
            row.map_err(|source| sqlite_error(path, "decode logical request attempt row", source))?;
        let attempt_id = decode_attempt_id(path, attempt_id, "attempt ID")?;
        records.push(decode_attempt(path, store_id, attempt_id, raw)?);
    }
    Ok(records)
}

/// Reads the most recent committed recovery report for this store.
pub(super) fn read_latest_recovery_report(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
) -> Result<Option<RecoveryReport>, RuntimeStoreError> {
    let store_id_text = store_id.to_string();
    let raw = connection
        .query_row(
            "SELECT recovery_run_id,
                    recovery_ordinal,
                    recovered_at_unix_ms,
                    interrupted_logical_count,
                    interrupted_attempt_count
             FROM recovery_runs
             WHERE store_id = ?1
             ORDER BY recovery_ordinal DESC
             LIMIT 1",
            params![store_id_text],
            |row| {
                Ok(RawRecoveryReport {
                    run_id: row.get(0)?,
                    recovery_ordinal: row.get(1)?,
                    recovered_at_unix_ms: row.get(2)?,
                    interrupted_logical_count: row.get(3)?,
                    interrupted_attempt_count: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(|source| sqlite_error(path, "read latest recovery report", source))?;
    raw.map(|raw| decode_recovery_report(path, store_id, raw))
        .transpose()
}

fn read_recovery_report(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
    run_id: RecoveryRunId,
) -> Result<Option<RecoveryReport>, RuntimeStoreError> {
    let store_id_text = store_id.to_string();
    let raw = connection
        .query_row(
            "SELECT recovery_run_id,
                    recovery_ordinal,
                    recovered_at_unix_ms,
                    interrupted_logical_count,
                    interrupted_attempt_count
             FROM recovery_runs
             WHERE store_id = ?1 AND recovery_run_id = ?2",
            params![store_id_text, run_id.as_uuid().as_bytes().as_slice()],
            |row| {
                Ok(RawRecoveryReport {
                    run_id: row.get(0)?,
                    recovery_ordinal: row.get(1)?,
                    recovered_at_unix_ms: row.get(2)?,
                    interrupted_logical_count: row.get(3)?,
                    interrupted_attempt_count: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(|source| sqlite_error(path, "read recovery report", source))?;
    raw.map(|raw| decode_recovery_report(path, store_id, raw))
        .transpose()
}

#[derive(Debug)]
struct RawTerminalRow {
    begun_at_unix_ms: i64,
    terminal_outcome: Option<String>,
    terminal_at_unix_ms: Option<i64>,
    economics_state: Option<String>,
    terminal_origin: Option<String>,
    recovery_run_id: Option<Vec<u8>>,
    terminal_payload_json: Option<String>,
}

#[derive(Debug)]
struct RawAttemptRow {
    logical_request_id: Vec<u8>,
    attempt_ordinal: i64,
    pending_evidence_json: String,
    terminal: RawTerminalRow,
}

#[derive(Debug)]
struct RawRecoveryReport {
    run_id: Vec<u8>,
    recovery_ordinal: i64,
    recovered_at_unix_ms: i64,
    interrupted_logical_count: i64,
    interrupted_attempt_count: i64,
}

fn decode_logical_request(
    path: &Path,
    store_id: Uuid,
    logical_request_id: LogicalRequestId,
    raw: RawTerminalRow,
) -> Result<LogicalRequestRecord, RuntimeStoreError> {
    let begun_at_unix_ms = decode_nonnegative(
        path,
        raw.begun_at_unix_ms,
        "logical request begun_at_unix_ms",
    )?;
    let terminal = decode_logical_terminal(path, &raw)?;
    Ok(LogicalRequestRecord {
        store_id,
        request: NewLogicalRequest {
            id: logical_request_id,
            begun_at_unix_ms,
        },
        terminal,
    })
}

fn decode_attempt(
    path: &Path,
    store_id: Uuid,
    attempt_id: AttemptId,
    raw: RawAttemptRow,
) -> Result<AttemptRecord, RuntimeStoreError> {
    let logical_request_id =
        decode_logical_request_id(path, raw.logical_request_id, "logical request ID")?;
    let attempt_ordinal = decode_nonnegative(path, raw.attempt_ordinal, "attempt ordinal")?;
    if attempt_ordinal == 0 {
        return Err(invalid_metadata(path, "attempt ordinal must be positive"));
    }
    let begun_at_unix_ms = decode_nonnegative(
        path,
        raw.terminal.begun_at_unix_ms,
        "attempt begun_at_unix_ms",
    )?;
    let evidence = decode_attempt_pending_evidence(path, &raw.pending_evidence_json)?;
    let terminal = decode_attempt_terminal(path, &raw.terminal)?;
    Ok(AttemptRecord {
        store_id,
        attempt: NewAttempt {
            id: attempt_id,
            logical_request_id,
            begun_at_unix_ms,
            evidence,
        },
        attempt_ordinal,
        terminal,
    })
}

fn decode_logical_terminal(
    path: &Path,
    raw: &RawTerminalRow,
) -> Result<Option<LogicalRequestTerminalRecord>, RuntimeStoreError> {
    let Some(outcome) = raw.terminal_outcome.as_deref() else {
        ensure_pending_envelope(path, raw)?;
        return Ok(None);
    };
    let terminal_at_unix_ms = terminal_time(path, raw)?;
    let economics_state = terminal_economics(path, raw)?;
    let origin = terminal_origin(path, raw)?;
    let recovery_run_id = optional_recovery_run_id(path, raw.recovery_run_id.as_deref())?;
    let payload =
        decode_logical_terminal_payload(path, origin, raw.terminal_payload_json.as_deref())?;
    Ok(Some(LogicalRequestTerminalRecord {
        terminal: LogicalRequestTerminal {
            outcome: LogicalRequestOutcome::parse(path, outcome)?,
            terminal_at_unix_ms,
            economics_state,
            payload,
        },
        origin,
        recovery_run_id,
    }))
}

fn decode_attempt_terminal(
    path: &Path,
    raw: &RawTerminalRow,
) -> Result<Option<AttemptTerminalRecord>, RuntimeStoreError> {
    let Some(outcome) = raw.terminal_outcome.as_deref() else {
        ensure_pending_envelope(path, raw)?;
        return Ok(None);
    };
    let terminal_at_unix_ms = terminal_time(path, raw)?;
    let economics_state = terminal_economics(path, raw)?;
    let origin = terminal_origin(path, raw)?;
    let recovery_run_id = optional_recovery_run_id(path, raw.recovery_run_id.as_deref())?;
    Ok(Some(AttemptTerminalRecord {
        terminal: AttemptTerminal {
            outcome: AttemptOutcome::parse(path, outcome)?,
            terminal_at_unix_ms,
            economics_state,
        },
        origin,
        recovery_run_id,
    }))
}

fn decode_logical_terminal_payload(
    path: &Path,
    origin: TerminalOrigin,
    payload_json: Option<&str>,
) -> Result<Option<LogicalRequestTerminalPayload>, RuntimeStoreError> {
    match (origin, payload_json) {
        (TerminalOrigin::Runtime, Some(payload_json)) => {
            let payload = serde_json::from_str::<LogicalRequestTerminalPayload>(payload_json)
                .map_err(|error| {
                    invalid_metadata(
                        path,
                        format!("logical terminal payload is invalid: {error}"),
                    )
                })?;
            payload.validate_for_write().map_err(|error| {
                invalid_metadata(
                    path,
                    format!("logical terminal payload is invalid: {error}"),
                )
            })?;
            Ok(Some(payload))
        }
        (TerminalOrigin::Runtime, None) => Err(invalid_metadata(
            path,
            "runtime logical terminal payload is missing",
        )),
        (TerminalOrigin::StartupRecovery, None) => Ok(None),
        (TerminalOrigin::StartupRecovery, Some(_)) => Err(invalid_metadata(
            path,
            "startup recovery logical terminal cannot contain a runtime payload",
        )),
    }
}

fn encode_logical_terminal_payload(
    payload: &LogicalRequestTerminalPayload,
) -> Result<String, String> {
    payload.validate_for_write()?;
    canonical_json_string(payload)
}

fn encode_attempt_pending_evidence(evidence: &AttemptPendingEvidence) -> Result<String, String> {
    evidence.validate()?;
    canonical_json_string(evidence)
}

fn decode_attempt_pending_evidence(
    path: &Path,
    evidence_json: &str,
) -> Result<AttemptPendingEvidence, RuntimeStoreError> {
    let evidence =
        serde_json::from_str::<AttemptPendingEvidence>(evidence_json).map_err(|error| {
            invalid_metadata(
                path,
                format!("attempt pending evidence is invalid: {error}"),
            )
        })?;
    evidence.validate().map_err(|error| {
        invalid_metadata(
            path,
            format!("attempt pending evidence is invalid: {error}"),
        )
    })?;
    Ok(evidence)
}

fn canonical_json_string(value: &impl Serialize) -> Result<String, String> {
    let mut value = serde_json::to_value(value).map_err(|error| error.to_string())?;
    canonicalize_json_value(&mut value);
    serde_json::to_string(&value).map_err(|error| error.to_string())
}

fn canonicalize_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                canonicalize_json_value(value);
            }
        }
        serde_json::Value::Object(object) => {
            let mut entries = std::mem::take(object).into_iter().collect::<Vec<_>>();
            for (_, value) in &mut entries {
                canonicalize_json_value(value);
            }
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            object.extend(entries);
        }
        _ => {}
    }
}

fn decode_recovery_report(
    path: &Path,
    store_id: Uuid,
    raw: RawRecoveryReport,
) -> Result<RecoveryReport, RuntimeStoreError> {
    Ok(RecoveryReport {
        store_id,
        run_id: decode_recovery_run_id(path, raw.run_id, "recovery run ID")?,
        recovery_ordinal: decode_positive(path, raw.recovery_ordinal, "recovery_ordinal")?,
        recovered_at_unix_ms: decode_nonnegative(
            path,
            raw.recovered_at_unix_ms,
            "recovered_at_unix_ms",
        )?,
        interrupted_logical_count: decode_nonnegative(
            path,
            raw.interrupted_logical_count,
            "interrupted_logical_count",
        )?,
        interrupted_attempt_count: decode_nonnegative(
            path,
            raw.interrupted_attempt_count,
            "interrupted_attempt_count",
        )?,
    })
}

fn ensure_pending_envelope(path: &Path, raw: &RawTerminalRow) -> Result<(), RuntimeStoreError> {
    if raw.terminal_at_unix_ms.is_some()
        || raw.economics_state.is_some()
        || raw.terminal_origin.is_some()
        || raw.recovery_run_id.is_some()
        || raw.terminal_payload_json.is_some()
    {
        Err(invalid_metadata(
            path,
            "pending lifecycle row has partial terminal fields",
        ))
    } else {
        Ok(())
    }
}

fn terminal_time(path: &Path, raw: &RawTerminalRow) -> Result<u64, RuntimeStoreError> {
    let value = raw
        .terminal_at_unix_ms
        .ok_or_else(|| invalid_metadata(path, "terminal timestamp is missing"))?;
    decode_nonnegative(path, value, "terminal_at_unix_ms")
}

fn terminal_economics(
    path: &Path,
    raw: &RawTerminalRow,
) -> Result<EconomicsState, RuntimeStoreError> {
    let value = raw
        .economics_state
        .as_deref()
        .ok_or_else(|| invalid_metadata(path, "terminal economics state is missing"))?;
    EconomicsState::parse(path, value)
}

fn terminal_origin(path: &Path, raw: &RawTerminalRow) -> Result<TerminalOrigin, RuntimeStoreError> {
    let value = raw
        .terminal_origin
        .as_deref()
        .ok_or_else(|| invalid_metadata(path, "terminal origin is missing"))?;
    TerminalOrigin::parse(path, value)
}

fn optional_recovery_run_id(
    path: &Path,
    bytes: Option<&[u8]>,
) -> Result<Option<RecoveryRunId>, RuntimeStoreError> {
    bytes
        .map(|bytes| decode_recovery_run_id(path, bytes.to_vec(), "recovery run ID"))
        .transpose()
}

fn decode_logical_request_id(
    path: &Path,
    bytes: Vec<u8>,
    field: &'static str,
) -> Result<LogicalRequestId, RuntimeStoreError> {
    let uuid = decode_uuid(path, bytes, field)?;
    LogicalRequestId::from_uuid(uuid).map_err(|_| invalid_metadata(path, format!("{field} is nil")))
}

fn decode_attempt_id(
    path: &Path,
    bytes: Vec<u8>,
    field: &'static str,
) -> Result<AttemptId, RuntimeStoreError> {
    let uuid = decode_uuid(path, bytes, field)?;
    AttemptId::from_uuid(uuid).map_err(|_| invalid_metadata(path, format!("{field} is nil")))
}

fn decode_recovery_run_id(
    path: &Path,
    bytes: Vec<u8>,
    field: &'static str,
) -> Result<RecoveryRunId, RuntimeStoreError> {
    let uuid = decode_uuid(path, bytes, field)?;
    RecoveryRunId::from_uuid(uuid).map_err(|_| invalid_metadata(path, format!("{field} is nil")))
}

fn decode_uuid(
    path: &Path,
    bytes: Vec<u8>,
    field: &'static str,
) -> Result<Uuid, RuntimeStoreError> {
    Uuid::from_slice(&bytes)
        .map_err(|_| invalid_metadata(path, format!("{field} is not UUID BLOB16")))
}

fn decode_nonnegative(
    path: &Path,
    value: i64,
    field: &'static str,
) -> Result<u64, RuntimeStoreError> {
    u64::try_from(value).map_err(|_| invalid_metadata(path, format!("{field} is negative")))
}

fn decode_positive(path: &Path, value: i64, field: &'static str) -> Result<u64, RuntimeStoreError> {
    let value = decode_nonnegative(path, value, field)?;
    if value == 0 {
        Err(invalid_metadata(path, format!("{field} must be positive")))
    } else {
        Ok(value)
    }
}

fn checked_sql_integer(
    value: u64,
    entity: &'static str,
    id: String,
    field: &'static str,
) -> Result<i64, RuntimeStoreError> {
    i64::try_from(value).map_err(|_| RuntimeStoreError::InvariantViolation {
        entity,
        id,
        detail: format!("{field} exceeds SQLite integer range"),
    })
}

fn invariant(
    entity: &'static str,
    id: impl fmt::Display,
    detail: impl Into<String>,
) -> RuntimeStoreError {
    RuntimeStoreError::InvariantViolation {
        entity,
        id: id.to_string(),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_request_projection_query_plan_uses_compound_partial_index_without_sorting() {
        let connection = Connection::open_in_memory().expect("open query-plan database");
        connection
            .execute_batch(&schema_sql())
            .expect("create runtime schema");

        let index_sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_schema
                 WHERE type = 'index'
                   AND name = 'logical_requests_runtime_terminal_order'",
                [],
                |row| row.get(0),
            )
            .expect("read runtime terminal index");
        assert_eq!(index_sql, LOGICAL_REQUESTS_RUNTIME_TERMINAL_ORDER_SQL);

        let first_page_plan = explain_committed_request_projection(&connection, None);
        assert_uses_runtime_terminal_order_index(&first_page_plan);

        let after_cursor_plan = explain_committed_request_projection(
            &connection,
            Some((1, Uuid::new_v4().as_bytes().to_vec())),
        );
        assert_uses_runtime_terminal_order_index(&after_cursor_plan);
        assert!(
            after_cursor_plan
                .iter()
                .any(|detail| detail.contains("(terminal_at_unix_ms,logical_request_id)<(?,?)")),
            "cursor plan must use the compound keyset range: {after_cursor_plan:?}"
        );
    }

    #[test]
    fn exact_identity_query_plans_use_service_scoped_partial_indexes_without_sorting() {
        let connection = Connection::open_in_memory().expect("open query-plan database");
        connection
            .execute_batch(&schema_sql())
            .expect("create runtime schema");

        for (column, index, identity) in [
            (
                "numeric_request_id",
                "logical_requests_runtime_service_request",
                Value::Integer(7),
            ),
            (
                "trace_id",
                "logical_requests_runtime_service_trace",
                Value::Text("trace-7".to_string()),
            ),
            (
                "session_id",
                "logical_requests_runtime_service_session",
                Value::Text("sid-7".to_string()),
            ),
        ] {
            let query = format!(
                "EXPLAIN QUERY PLAN
                 SELECT logical_request_id
                 FROM logical_requests
                 WHERE store_id = ?1
                   AND terminal_origin = 'runtime'
                   AND terminal_payload_json IS NOT NULL
                   AND service_name = ?2
                   AND {column} = ?3
                 ORDER BY terminal_at_unix_ms DESC, logical_request_id DESC
                 LIMIT ?4"
            );
            let parameters = [
                Value::Text(Uuid::new_v4().to_string()),
                Value::Text("codex".to_string()),
                identity,
                Value::Integer(2),
            ];
            let mut statement = connection
                .prepare(&query)
                .expect("prepare identity query plan");
            let mut rows = statement
                .query(params_from_iter(parameters.iter()))
                .expect("read identity query plan");
            let mut details = Vec::new();
            while let Some(row) = rows.next().expect("advance identity query plan") {
                details.push(row.get::<_, String>(3).expect("decode query plan detail"));
            }
            assert!(
                details
                    .iter()
                    .any(|detail| detail.contains(&format!("USING INDEX {index}"))),
                "{column} query must use {index}: {details:?}"
            );
            assert!(
                details.iter().all(|detail| !detail.contains("TEMP B-TREE")),
                "{column} query must not allocate a temporary sort B-tree: {details:?}"
            );
        }
    }

    fn explain_committed_request_projection(
        connection: &Connection,
        cursor: Option<(i64, Vec<u8>)>,
    ) -> Vec<String> {
        let query = if cursor.is_some() {
            COMMITTED_REQUESTS_AFTER_CURSOR_SQL
        } else {
            COMMITTED_REQUESTS_FIRST_PAGE_SQL
        };
        let mut statement = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {query}"))
            .expect("prepare committed request query plan");
        let store_id = Uuid::new_v4().to_string();
        let mut rows = match cursor.as_ref() {
            Some((cursor_time, cursor_id)) => {
                statement.query(params![store_id, cursor_time, cursor_id, 2_i64])
            }
            None => statement.query(params![store_id, 2_i64]),
        }
        .expect("read committed request query plan");
        let mut details = Vec::new();
        while let Some(row) = rows.next().expect("advance committed request query plan") {
            details.push(
                row.get(3)
                    .expect("decode committed request query-plan detail"),
            );
        }
        details
    }

    fn assert_uses_runtime_terminal_order_index(plan: &[String]) {
        assert!(
            plan.iter().any(
                |detail| detail.contains("USING INDEX logical_requests_runtime_terminal_order")
            ),
            "query plan must use the runtime terminal order index: {plan:?}"
        );
        assert!(
            plan.iter().all(|detail| !detail.contains("TEMP B-TREE")),
            "query plan must not allocate a temporary sort B-tree: {plan:?}"
        );
    }
}

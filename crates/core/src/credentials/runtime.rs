use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::future::join_all;
use tokio::sync::watch;

use crate::auth_resolution::{
    CredentialResolution, is_valid_environment_variable_name,
    resolve_environment_credential_for_runtime, resolve_service_credential_for_runtime,
};
use crate::config::{CredentialRef, UpstreamAuth};
use crate::runtime_identity::ProviderEndpointKey;
#[cfg(test)]
use crate::runtime_identity::RuntimeUpstreamIdentity;
use crate::runtime_store::{RuntimeQuotaIdentity, RuntimeStore};

#[cfg(test)]
use super::generation::CapturedUpstreamCredential;
use super::generation::{
    CredentialCatalog, CredentialGeneration, CredentialHandle, CredentialLoadFailure,
    CredentialLoadResult, CredentialSourceSpec, CredentialSourceState, EndpointCredentialBinding,
    NATIVE_HARD_EXPIRY, NamedCredentialLookup, NamedCredentialReference, RuntimeCredentialKind,
    generation_digest, preserve_last_known_good,
};
use super::{
    CredentialErrorCode, CredentialName, CredentialReadinessCode, CredentialReadinessDetail,
    CredentialSourceCapabilities, CredentialSourceKind, InstallationIdentity,
    NativeCredentialDaemon, SecretValue, read_secret_file,
};

const NATIVE_READ_TIMEOUT: Duration = Duration::from_secs(5);
const NATIVE_READ_PENDING: u8 = 0;
const NATIVE_READ_EXPIRED: u8 = 1;
const NATIVE_READ_COMPLETED: u8 = 2;

#[cfg(test)]
impl CapturedUpstreamCredential {
    pub(crate) fn from_config_for_test(service_name: &str, auth: &UpstreamAuth) -> Self {
        Self::runtime_binding_from_config_for_test(
            &ProviderEndpointKey::new(service_name, "test", "default"),
            "https://example.test/v1",
            None,
            auth,
        )
        .0
    }

    pub(crate) fn runtime_binding_from_config_for_test(
        endpoint: &ProviderEndpointKey,
        base_url: &str,
        continuity_domain: Option<String>,
        auth: &UpstreamAuth,
    ) -> (Self, RuntimeUpstreamIdentity) {
        let runtime_store =
            Arc::new(RuntimeStore::open_in_memory().expect("open credential test store"));
        let runtime = CredentialRuntime::from_runtime_store(
            CredentialSourceCapabilities::server(),
            runtime_store.as_ref(),
        )
        .expect("build credential test runtime");
        let generation = runtime
            .build_generation([CredentialCandidateInput {
                provider_endpoint: endpoint.clone(),
                auth,
            }])
            .expect("capture configured test credential");
        let credential = generation
            .capture_bound(endpoint)
            .expect("capture registered test credential");
        let identity = generation
            .bind_upstream_identity(endpoint.clone(), base_url.to_string(), continuity_domain)
            .expect("bind configured test credential identity");
        (credential, identity)
    }
}

pub(crate) struct CredentialCandidateInput<'a> {
    pub(crate) provider_endpoint: ProviderEndpointKey,
    pub(crate) auth: &'a UpstreamAuth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CredentialEndpointReadiness {
    pub(crate) code: CredentialReadinessCode,
    pub(crate) details: Vec<CredentialReadinessDetail>,
    pub(crate) configured_contract: bool,
    pub(crate) allow_anonymous: bool,
}

#[derive(Clone)]
pub(crate) struct CredentialReadinessEvaluator {
    runtime: CredentialRuntime,
}

struct EvaluatedCredentialCatalog {
    catalog: CredentialCatalog,
    states: BTreeMap<CredentialHandle, CredentialSourceState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CredentialRuntimeRefreshCause {
    Scheduled,
    AuthenticationFailure,
    ExplicitRefresh,
    ExplicitDelete,
}

#[derive(Clone)]
pub(crate) struct CredentialRuntime {
    inner: Arc<CredentialRuntimeInner>,
    scope_identity: Option<RuntimeQuotaIdentity>,
}

struct CredentialRuntimeInner {
    native: NativeCredentialDaemon,
    inflight: Mutex<BTreeMap<CredentialHandle, Arc<NativeReadFlight>>>,
    read_timeout: Duration,
}

struct NativeReadFlight {
    receiver: watch::Receiver<Option<CredentialLoadResult>>,
    blocking_result: Mutex<Option<CredentialLoadResult>>,
    blocking_ready: Condvar,
    state: AtomicU8,
    deadline: Instant,
}

#[cfg(test)]
struct TestBlockingNativeStore {
    value: Mutex<Option<SecretValue>>,
    reads: AtomicUsize,
    blocked: Mutex<bool>,
    released: Condvar,
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct TestBlockingNativeReadControl {
    backend: Arc<TestBlockingNativeStore>,
}

#[cfg(test)]
impl TestBlockingNativeReadControl {
    pub(crate) fn block(&self) {
        *self.backend.blocked.lock().expect("native block lock") = true;
    }

    pub(crate) fn release(&self) {
        *self.backend.blocked.lock().expect("native block lock") = false;
        self.backend.released.notify_all();
    }

    pub(crate) fn read_count(&self) -> usize {
        self.backend.reads.load(Ordering::SeqCst)
    }

    pub(crate) fn set_value(&self, value: SecretValue) {
        *self.backend.value.lock().expect("native value lock") = Some(value);
    }
}

#[cfg(test)]
impl super::capabilities::NativeCredentialStore for TestBlockingNativeStore {
    fn create(
        &self,
        _locator: &super::native::NativeCredentialLocator,
        _value: &SecretValue,
    ) -> std::result::Result<(), super::capabilities::NativeStoreError> {
        unreachable!()
    }

    fn set(
        &self,
        _locator: &super::native::NativeCredentialLocator,
        _value: &SecretValue,
    ) -> std::result::Result<(), super::capabilities::NativeStoreError> {
        unreachable!()
    }

    fn read(
        &self,
        _locator: &super::native::NativeCredentialLocator,
    ) -> std::result::Result<SecretValue, super::capabilities::NativeStoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let mut blocked = self.blocked.lock().expect("native block lock");
        while *blocked {
            blocked = self
                .released
                .wait(blocked)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        drop(blocked);
        self.value
            .lock()
            .expect("native value lock")
            .clone()
            .ok_or_else(|| super::capabilities::NativeStoreError::new(CredentialErrorCode::Missing))
    }

    fn delete(
        &self,
        _locator: &super::native::NativeCredentialLocator,
    ) -> std::result::Result<(), super::capabilities::NativeStoreError> {
        unreachable!()
    }
}

impl fmt::Debug for CredentialRuntimeInner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialRuntime")
            .field("native", &self.native)
            .field(
                "inflight_count",
                &self
                    .inflight
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .len(),
            )
            .finish()
    }
}

impl CredentialRuntime {
    #[cfg(test)]
    pub(crate) fn test_blocking_sources(
        initial: SecretValue,
    ) -> (CredentialSourceCapabilities, TestBlockingNativeReadControl) {
        let backend = Arc::new(TestBlockingNativeStore {
            value: Mutex::new(Some(initial)),
            reads: AtomicUsize::new(0),
            blocked: Mutex::new(false),
            released: Condvar::new(),
        });
        (
            CredentialSourceCapabilities::from_backend(Arc::clone(&backend)),
            TestBlockingNativeReadControl { backend },
        )
    }

    pub(crate) fn from_runtime_store(
        capabilities: CredentialSourceCapabilities,
        runtime_store: &RuntimeStore,
    ) -> Result<Self> {
        let installation = InstallationIdentity::from_runtime_store(runtime_store);
        let scope_identity = runtime_store
            .load_or_create_quota_identity()
            .context("load runtime credential scope identity")?;
        Ok(Self::from_installation(
            capabilities,
            installation,
            Some(scope_identity),
        ))
    }

    fn from_installation(
        capabilities: CredentialSourceCapabilities,
        installation: InstallationIdentity,
        scope_identity: Option<RuntimeQuotaIdentity>,
    ) -> Self {
        Self {
            inner: Arc::new(CredentialRuntimeInner {
                native: capabilities.daemon(installation),
                inflight: Mutex::new(BTreeMap::new()),
                read_timeout: NATIVE_READ_TIMEOUT,
            }),
            scope_identity,
        }
    }

    fn without_runtime_store(capabilities: CredentialSourceCapabilities) -> Result<Self> {
        let native = capabilities.forbidden_daemon().context(
            "runtime-store-free credential evaluation requires native credentials to be forbidden",
        )?;
        Ok(Self {
            inner: Arc::new(CredentialRuntimeInner {
                native,
                inflight: Mutex::new(BTreeMap::new()),
                read_timeout: NATIVE_READ_TIMEOUT,
            }),
            scope_identity: None,
        })
    }

    #[cfg(test)]
    fn set_read_timeout_for_test(&mut self, timeout: Duration) {
        Arc::get_mut(&mut self.inner)
            .expect("test credential runtime must be uniquely owned")
            .read_timeout = timeout;
    }

    #[cfg(test)]
    pub(crate) fn native_inflight_owner_count_for_test(&self) -> usize {
        self.inner
            .inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .map(Arc::strong_count)
            .max()
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub(crate) fn build_generation<'a>(
        &self,
        candidates: impl IntoIterator<Item = CredentialCandidateInput<'a>>,
    ) -> Result<Arc<CredentialGeneration>> {
        self.build_generation_with_previous(candidates, std::iter::empty(), "", None)
    }

    pub(crate) fn build_generation_with_named<'a>(
        &self,
        candidates: impl IntoIterator<Item = CredentialCandidateInput<'a>>,
        named: impl IntoIterator<Item = NamedCredentialReference>,
        named_catalog_revision: &str,
    ) -> Result<Arc<CredentialGeneration>> {
        self.build_generation_with_previous(candidates, named, named_catalog_revision, None)
    }

    #[cfg(test)]
    pub(crate) fn build_generation_from_previous<'a>(
        &self,
        candidates: impl IntoIterator<Item = CredentialCandidateInput<'a>>,
        previous: &CredentialGeneration,
    ) -> Result<Arc<CredentialGeneration>> {
        self.build_generation_with_previous(candidates, std::iter::empty(), "", Some(previous))
    }

    pub(crate) fn build_generation_from_previous_with_named<'a>(
        &self,
        candidates: impl IntoIterator<Item = CredentialCandidateInput<'a>>,
        named: impl IntoIterator<Item = NamedCredentialReference>,
        named_catalog_revision: &str,
        previous: &CredentialGeneration,
    ) -> Result<Arc<CredentialGeneration>> {
        self.build_generation_with_previous(
            candidates,
            named,
            named_catalog_revision,
            Some(previous),
        )
    }

    fn build_generation_with_previous<'a>(
        &self,
        candidates: impl IntoIterator<Item = CredentialCandidateInput<'a>>,
        named: impl IntoIterator<Item = NamedCredentialReference>,
        named_catalog_revision: &str,
        previous: Option<&CredentialGeneration>,
    ) -> Result<Arc<CredentialGeneration>> {
        let evaluated = self.evaluate_catalog_with_previous(
            candidates,
            named,
            named_catalog_revision,
            previous,
        )?;
        let previous_revision = previous.map_or(0, |generation| generation.revision);
        let mut next = self.finish_generation(
            previous_revision,
            Arc::new(evaluated.catalog),
            evaluated.states,
        )?;
        if previous.is_some_and(|generation| generation.digest != next.digest) {
            Arc::get_mut(&mut next)
                .expect("fresh credential generation is uniquely owned")
                .revision = previous_revision.saturating_add(1);
        }
        Ok(next)
    }

    fn evaluate_catalog_with_previous<'a>(
        &self,
        candidates: impl IntoIterator<Item = CredentialCandidateInput<'a>>,
        named: impl IntoIterator<Item = NamedCredentialReference>,
        named_catalog_revision: &str,
        previous: Option<&CredentialGeneration>,
    ) -> Result<EvaluatedCredentialCatalog> {
        let now = Instant::now();
        let mut catalog = CredentialCatalog {
            named_catalog_revision: Arc::from(named_catalog_revision),
            ..CredentialCatalog::default()
        };
        let mut initial = BTreeMap::<CredentialHandle, CredentialLoadResult>::new();

        for candidate in candidates {
            let service_name = candidate.provider_endpoint.service_name.as_str();
            let bearer = self.register_part(
                &mut catalog,
                &mut initial,
                &candidate.provider_endpoint,
                service_name,
                RuntimeCredentialKind::Bearer,
                candidate.auth.auth_token_ref.as_ref(),
                candidate.auth.auth_token.as_deref(),
                candidate.auth.auth_token_env.as_deref(),
            )?;
            let api_key = self.register_part(
                &mut catalog,
                &mut initial,
                &candidate.provider_endpoint,
                service_name,
                RuntimeCredentialKind::ApiKey,
                candidate.auth.api_key_ref.as_ref(),
                candidate.auth.api_key.as_deref(),
                candidate.auth.api_key_env.as_deref(),
            )?;
            let configured_contract = bearer.is_some() || api_key.is_some();
            let binding = EndpointCredentialBinding {
                auth_token: bearer,
                api_key,
                configured_contract,
                allow_anonymous: candidate.auth.allow_anonymous == Some(true),
            };
            if let Some(previous) = catalog
                .endpoints
                .insert(candidate.provider_endpoint.clone(), binding.clone())
                && previous != binding
            {
                anyhow::bail!(
                    "runtime credential catalog has conflicting auth for {}",
                    candidate.provider_endpoint
                );
            }
        }

        for reference in named {
            self.register_named(&mut catalog, &mut initial, reference)?;
        }

        let states = initial
            .into_iter()
            .map(|(handle, result)| {
                let state = match result {
                    Ok(value) => CredentialSourceState::Ready {
                        value,
                        loaded_at: now,
                    },
                    Err(failure) => previous
                        .and_then(|previous| {
                            preserve_last_known_good(previous, &catalog, &handle, now, &failure)
                        })
                        .unwrap_or(CredentialSourceState::Unavailable {
                            attempted_at: now,
                            failure,
                        }),
                };
                (handle, state)
            })
            .collect();
        Ok(EvaluatedCredentialCatalog { catalog, states })
    }

    fn register_named(
        &self,
        catalog: &mut CredentialCatalog,
        initial: &mut BTreeMap<CredentialHandle, CredentialLoadResult>,
        reference: NamedCredentialReference,
    ) -> Result<()> {
        let service_name = reference.service_name.trim();
        let name = reference.name.trim();
        if service_name.is_empty() || name.is_empty() {
            anyhow::bail!("named credential references require a service and name");
        }
        if !is_valid_environment_variable_name(name) {
            anyhow::bail!("named credential reference has an invalid environment variable name");
        }
        let reference = NamedCredentialReference {
            service_name: service_name.to_string(),
            name: name.to_string(),
            lookup: reference.lookup,
        };
        let descriptor_ref = match reference.lookup {
            NamedCredentialLookup::ServiceCredential => {
                format!("{}/{}", reference.service_name, reference.name)
            }
            NamedCredentialLookup::EnvironmentOnly => reference.name.clone(),
        };
        let handle = CredentialHandle::for_descriptor(&[
            reference.lookup.descriptor_kind(),
            descriptor_ref.as_bytes(),
        ]);
        let spec = CredentialSourceSpec::Static {
            source_kind: "environment",
            reference: reference.name.clone(),
        };
        catalog.sources.entry(handle.clone()).or_insert(spec);
        catalog.named.insert(reference.clone(), handle.clone());
        if initial.contains_key(&handle) {
            return Ok(());
        }
        let resolution = match reference.lookup {
            NamedCredentialLookup::ServiceCredential => resolve_service_credential_for_runtime(
                reference.service_name.as_str(),
                None,
                Some(reference.name.as_str()),
            ),
            NamedCredentialLookup::EnvironmentOnly => {
                resolve_environment_credential_for_runtime(reference.name.as_str())
            }
        };
        initial.insert(
            handle,
            map_runtime_resolution(resolution, "environment", reference.name),
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn register_part<'a>(
        &self,
        catalog: &mut CredentialCatalog,
        initial: &mut BTreeMap<CredentialHandle, CredentialLoadResult>,
        endpoint: &ProviderEndpointKey,
        service_name: &'a str,
        kind: RuntimeCredentialKind,
        reference: Option<&'a CredentialRef>,
        inline: Option<&'a str>,
        env_name: Option<&'a str>,
    ) -> Result<Option<CredentialHandle>> {
        let Some((handle, spec, load)) =
            self.prepare_source(endpoint, service_name, kind, reference, inline, env_name)?
        else {
            return Ok(None);
        };
        catalog.sources.entry(handle.clone()).or_insert(spec);
        initial.entry(handle.clone()).or_insert_with(load);
        Ok(Some(handle))
    }

    fn prepare_source<'a>(
        &self,
        endpoint: &ProviderEndpointKey,
        service_name: &'a str,
        kind: RuntimeCredentialKind,
        reference: Option<&'a CredentialRef>,
        inline: Option<&'a str>,
        env_name: Option<&'a str>,
    ) -> Result<Option<PreparedCredentialSource<'a>>> {
        if let Some(reference) = reference {
            return match reference {
                CredentialRef::Native { name } => {
                    let name = CredentialName::parse(name.clone())
                        .context("validated native credential name became invalid")?;
                    let handle =
                        CredentialHandle::for_descriptor(&[b"native", name.as_str().as_bytes()]);
                    let spec = CredentialSourceSpec::Native { name: name.clone() };
                    let runtime = self.clone();
                    let load_handle = handle.clone();
                    let load_source = spec.clone();
                    Ok(Some((
                        handle,
                        spec,
                        Box::new(move || {
                            runtime.read_native_singleflight_blocking(load_handle, load_source)
                        }),
                    )))
                }
                CredentialRef::SecretFile { path } => {
                    let path = PathBuf::from(path);
                    let path_text = path.to_string_lossy().into_owned();
                    let handle =
                        CredentialHandle::for_descriptor(&[b"secret-file", path_text.as_bytes()]);
                    let spec = CredentialSourceSpec::Static {
                        source_kind: CredentialSourceKind::SecretFile.as_str(),
                        reference: path_text.clone(),
                    };
                    Ok(Some((
                        handle,
                        spec,
                        Box::new(move || {
                            read_secret_file(&path).map_err(|error| CredentialLoadFailure {
                                code: error.code(),
                                source_kind: CredentialSourceKind::SecretFile.as_str(),
                                reference: path_text,
                            })
                        }),
                    )))
                }
            };
        }

        let inline = inline.filter(|value| !value.trim().is_empty());
        let env_name = env_name.map(str::trim).filter(|value| !value.is_empty());
        if inline.is_none() && env_name.is_none() {
            return Ok(None);
        }
        let (descriptor_kind, descriptor_ref, source_kind, display_ref) = if inline.is_some() {
            (
                b"inline".as_slice(),
                format!(
                    "{}/{}/{}",
                    endpoint.service_name,
                    endpoint.provider_id,
                    std::str::from_utf8(kind.as_bytes()).unwrap_or("credential")
                ),
                "inline",
                "inline".to_string(),
            )
        } else {
            let name = env_name.expect("checked environment reference");
            (
                NamedCredentialLookup::ServiceCredential.descriptor_kind(),
                format!("{service_name}/{name}"),
                "environment",
                name.to_string(),
            )
        };
        let handle =
            CredentialHandle::for_descriptor(&[descriptor_kind, descriptor_ref.as_bytes()]);
        let spec = CredentialSourceSpec::Static {
            source_kind,
            reference: display_ref.clone(),
        };
        Ok(Some((
            handle,
            spec,
            Box::new(move || {
                let resolution =
                    resolve_service_credential_for_runtime(service_name, inline, env_name);
                map_runtime_resolution(resolution, source_kind, display_ref)
            }),
        )))
    }

    fn finish_generation(
        &self,
        previous_revision: u64,
        catalog: Arc<CredentialCatalog>,
        states: BTreeMap<CredentialHandle, CredentialSourceState>,
    ) -> Result<Arc<CredentialGeneration>> {
        let scope_identity = self
            .scope_identity
            .as_ref()
            .context("credential generation requires a runtime scope identity")?;
        let now = Instant::now();
        let mut scopes = BTreeMap::new();
        for (endpoint, binding) in &catalog.endpoints {
            let bearer = binding
                .auth_token
                .as_ref()
                .and_then(|handle| states.get(handle))
                .and_then(|state| state.value_at(now));
            let api_key = binding
                .api_key
                .as_ref()
                .and_then(|handle| states.get(handle))
                .and_then(|state| state.value_at(now));
            let scope = scope_identity.derive_credential_scope(
                bearer.map(SecretValue::expose),
                api_key.map(SecretValue::expose),
            );
            scopes.insert(endpoint.clone(), scope);
        }
        let named_scopes = catalog
            .named
            .iter()
            .map(|(reference, handle)| {
                let value = states
                    .get(handle)
                    .and_then(|state| state.value_at(now))
                    .map(SecretValue::expose);
                (
                    reference.clone(),
                    scope_identity.derive_credential_scope(value, None),
                )
            })
            .collect();
        let digest = generation_digest(&catalog, &states, &scopes, &named_scopes);
        Ok(Arc::new(CredentialGeneration {
            revision: previous_revision,
            digest,
            catalog,
            sources: Arc::new(states),
            scopes: Arc::new(scopes),
        }))
    }

    pub(crate) async fn refresh_generation(
        &self,
        previous: Arc<CredentialGeneration>,
        requested: Option<Arc<[CredentialHandle]>>,
        cause: CredentialRuntimeRefreshCause,
    ) -> Result<Arc<CredentialGeneration>> {
        let now = Instant::now();
        let requested = requested
            .as_deref()
            .map(|handles| handles.iter().cloned().collect::<BTreeSet<_>>());
        let handles = previous
            .catalog
            .sources
            .iter()
            .filter(|(handle, source)| {
                if !source.is_native() {
                    return false;
                }
                if let Some(requested) = requested.as_ref() {
                    return requested.contains(*handle);
                }
                match cause {
                    CredentialRuntimeRefreshCause::Scheduled => previous
                        .sources
                        .get(*handle)
                        .is_some_and(|state| state.next_deadline() <= now),
                    CredentialRuntimeRefreshCause::AuthenticationFailure
                    | CredentialRuntimeRefreshCause::ExplicitRefresh
                    | CredentialRuntimeRefreshCause::ExplicitDelete => true,
                }
            })
            .map(|(handle, source)| (handle.clone(), source.clone()))
            .collect::<Vec<_>>();
        if handles.is_empty() {
            return Ok(previous);
        }

        let loads = join_all(handles.iter().map(|(handle, source)| {
            let runtime = self.clone();
            let handle = handle.clone();
            let source = source.clone();
            async move {
                let result = if cause == CredentialRuntimeRefreshCause::ExplicitDelete {
                    Err(CredentialLoadFailure {
                        code: CredentialErrorCode::Missing,
                        source_kind: source.source_kind(),
                        reference: source.reference().to_string(),
                    })
                } else {
                    runtime
                        .read_native_singleflight(handle.clone(), source)
                        .await
                };
                (handle, result)
            }
        }))
        .await;

        let completed_at = Instant::now();
        let mut states = previous.sources.as_ref().clone();
        for (handle, result) in loads {
            let next = match result {
                Ok(value) => CredentialSourceState::Ready {
                    value,
                    loaded_at: completed_at,
                },
                Err(failure) if cause == CredentialRuntimeRefreshCause::ExplicitDelete => {
                    CredentialSourceState::Unavailable {
                        attempted_at: completed_at,
                        failure,
                    }
                }
                Err(failure) => match states.get(&handle) {
                    Some(CredentialSourceState::Ready { value, loaded_at })
                    | Some(CredentialSourceState::Stale {
                        value, loaded_at, ..
                    }) if completed_at.saturating_duration_since(*loaded_at)
                        < NATIVE_HARD_EXPIRY =>
                    {
                        CredentialSourceState::Stale {
                            value: value.clone(),
                            loaded_at: *loaded_at,
                            attempted_at: completed_at,
                            failure,
                        }
                    }
                    _ => CredentialSourceState::Unavailable {
                        attempted_at: completed_at,
                        failure,
                    },
                },
            };
            states.insert(handle, next);
        }
        let mut next =
            self.finish_generation(previous.revision, Arc::clone(&previous.catalog), states)?;
        if next.digest != previous.digest {
            Arc::get_mut(&mut next)
                .expect("fresh credential generation is uniquely owned")
                .revision = previous.revision.saturating_add(1);
        }
        Ok(next)
    }

    async fn read_native_singleflight(
        &self,
        handle: CredentialHandle,
        source: CredentialSourceSpec,
    ) -> CredentialLoadResult {
        let failure_reference = source.reference().to_string();
        let flight = self.native_read_flight(handle, source);
        let mut receiver = flight.receiver.clone();

        loop {
            if let Some(result) = receiver.borrow().clone() {
                return result;
            }
            if flight.state.load(Ordering::Acquire) == NATIVE_READ_EXPIRED {
                return Err(native_refresh_failure(failure_reference));
            }
            tokio::select! {
                biased;
                () = tokio::time::sleep_until(tokio::time::Instant::from_std(flight.deadline)) => {
                    match flight
                        .state
                        .compare_exchange(
                            NATIVE_READ_PENDING,
                            NATIVE_READ_EXPIRED,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                    {
                        Ok(_) | Err(NATIVE_READ_EXPIRED) => {
                            return Err(native_refresh_failure(failure_reference));
                        }
                        Err(NATIVE_READ_COMPLETED) => {
                            if receiver.changed().await.is_err() {
                                return Err(native_refresh_failure(failure_reference));
                            }
                        }
                        Err(_) => {}
                    }
                }
                changed = receiver.changed() => {
                    if changed.is_err() {
                        return Err(native_refresh_failure(failure_reference));
                    }
                }
            }
        }
    }

    fn read_native_singleflight_blocking(
        &self,
        handle: CredentialHandle,
        source: CredentialSourceSpec,
    ) -> CredentialLoadResult {
        let failure_reference = source.reference().to_string();
        let flight = self.native_read_flight(handle, source);
        let mut result = flight
            .blocking_result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(result) = result.clone() {
                return result;
            }
            let state = flight.state.load(Ordering::Acquire);
            if state == NATIVE_READ_EXPIRED {
                return Err(native_refresh_failure(failure_reference));
            }
            let now = Instant::now();
            let Some(remaining) = flight.deadline.checked_duration_since(now) else {
                if state == NATIVE_READ_COMPLETED {
                    result = flight
                        .blocking_ready
                        .wait(result)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    continue;
                }
                if flight
                    .state
                    .compare_exchange(
                        NATIVE_READ_PENDING,
                        NATIVE_READ_EXPIRED,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    return Err(native_refresh_failure(failure_reference));
                }
                continue;
            };
            let (next_result, timeout) = flight
                .blocking_ready
                .wait_timeout(result, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            result = next_result;
            if timeout.timed_out()
                && result.is_none()
                && flight
                    .state
                    .compare_exchange(
                        NATIVE_READ_PENDING,
                        NATIVE_READ_EXPIRED,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
            {
                return Err(native_refresh_failure(failure_reference));
            }
        }
    }

    fn native_read_flight(
        &self,
        handle: CredentialHandle,
        source: CredentialSourceSpec,
    ) -> Arc<NativeReadFlight> {
        let mut inflight = self
            .inner
            .inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(flight) = inflight.get(&handle) {
            return Arc::clone(flight);
        }

        let (sender, receiver) = watch::channel(None);
        let flight = Arc::new(NativeReadFlight {
            receiver,
            blocking_result: Mutex::new(None),
            blocking_ready: Condvar::new(),
            state: AtomicU8::new(NATIVE_READ_PENDING),
            deadline: Instant::now() + self.inner.read_timeout,
        });
        inflight.insert(handle.clone(), Arc::clone(&flight));
        drop(inflight);

        let weak = Arc::downgrade(&self.inner);
        let cleanup_weak = weak.clone();
        let native = self.inner.native.clone();
        let task_handle = handle.clone();
        let task_flight = Arc::clone(&flight);
        let spawn_result = std::thread::Builder::new()
            .name("codex-helper-native-credential-read".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    read_native_source(&native, source)
                }))
                .unwrap_or_else(|_| Err(native_refresh_failure("native-refresh".to_string())));
                let completed = Instant::now() < task_flight.deadline
                    && task_flight
                        .state
                        .compare_exchange(
                            NATIVE_READ_PENDING,
                            NATIVE_READ_COMPLETED,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok();
                if completed {
                    *task_flight
                        .blocking_result
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result.clone());
                }
                remove_inflight(weak, &task_handle, &task_flight);
                if completed {
                    task_flight.blocking_ready.notify_all();
                    let _ = sender.send(Some(result));
                }
            });
        if spawn_result.is_err() {
            flight.state.store(NATIVE_READ_EXPIRED, Ordering::Release);
            flight.blocking_ready.notify_all();
            remove_inflight(cleanup_weak, &handle, &flight);
        }
        flight
    }
}

impl CredentialReadinessEvaluator {
    pub(crate) fn new(
        capabilities: CredentialSourceCapabilities,
        installation: InstallationIdentity,
    ) -> Self {
        Self {
            runtime: CredentialRuntime::from_installation(capabilities, installation, None),
        }
    }

    pub(crate) fn without_runtime_store(
        capabilities: CredentialSourceCapabilities,
    ) -> Result<Self> {
        Ok(Self {
            runtime: CredentialRuntime::without_runtime_store(capabilities)?,
        })
    }

    pub(crate) fn evaluate<'a>(
        &self,
        candidates: impl IntoIterator<Item = CredentialCandidateInput<'a>>,
    ) -> Result<BTreeMap<ProviderEndpointKey, CredentialEndpointReadiness>> {
        let evaluated = self.runtime.evaluate_catalog_with_previous(
            candidates,
            std::iter::empty(),
            "",
            None,
        )?;
        let now = Instant::now();
        Ok(evaluated
            .catalog
            .endpoints
            .iter()
            .map(|(provider_endpoint, binding)| {
                let details = [
                    (binding.auth_token.as_ref(), RuntimeCredentialKind::Bearer),
                    (binding.api_key.as_ref(), RuntimeCredentialKind::ApiKey),
                ]
                .into_iter()
                .filter_map(|(handle, kind)| {
                    evaluated_part_readiness_detail(
                        &evaluated.catalog,
                        &evaluated.states,
                        handle,
                        kind,
                        now,
                    )
                })
                .collect::<Vec<_>>();
                let code = CredentialReadinessCode::from_binding_codes(
                    details.iter().map(|detail| detail.code),
                );
                (
                    provider_endpoint.clone(),
                    CredentialEndpointReadiness {
                        code,
                        details,
                        configured_contract: binding.configured_contract,
                        allow_anonymous: binding.allow_anonymous,
                    },
                )
            })
            .collect())
    }
}

type PreparedCredentialSource<'a> = (
    CredentialHandle,
    CredentialSourceSpec,
    Box<dyn FnOnce() -> CredentialLoadResult + 'a>,
);

fn evaluated_part_readiness_detail(
    catalog: &CredentialCatalog,
    states: &BTreeMap<CredentialHandle, CredentialSourceState>,
    handle: Option<&CredentialHandle>,
    kind: RuntimeCredentialKind,
    now: Instant,
) -> Option<CredentialReadinessDetail> {
    let handle = handle?;
    let Some(spec) = catalog.sources.get(handle) else {
        return Some(CredentialReadinessDetail {
            kind: Some(kind.binding_kind()),
            code: CredentialReadinessCode::Invalid,
            stale_cause: None,
            source_kind: Some("runtime".to_string()),
            reference: None,
        });
    };
    let Some(state) = states.get(handle) else {
        return Some(CredentialReadinessDetail {
            kind: Some(kind.binding_kind()),
            code: CredentialReadinessCode::Invalid,
            stale_cause: None,
            source_kind: Some("runtime".to_string()),
            reference: None,
        });
    };
    let (code, stale_cause, source_kind, reference) = match state {
        CredentialSourceState::Ready { .. } => (
            CredentialReadinessCode::Ready,
            None,
            spec.source_kind(),
            spec.reference(),
        ),
        CredentialSourceState::Stale { failure, .. } if state.value_at(now).is_some() => (
            CredentialReadinessCode::Stale,
            Some(failure.code.into()),
            failure.source_kind,
            failure.reference.as_str(),
        ),
        CredentialSourceState::Stale { failure, .. }
        | CredentialSourceState::Unavailable { failure, .. } => (
            failure.code.into(),
            None,
            failure.source_kind,
            failure.reference.as_str(),
        ),
    };
    Some(CredentialReadinessDetail {
        kind: Some(kind.binding_kind()),
        code,
        stale_cause,
        source_kind: Some(source_kind.to_string()),
        reference: Some(reference.to_string()),
    })
}

fn native_refresh_failure(reference: String) -> CredentialLoadFailure {
    CredentialLoadFailure {
        code: CredentialErrorCode::BackendUnavailable,
        source_kind: CredentialSourceKind::Native.as_str(),
        reference,
    }
}

fn read_native_source(
    native: &NativeCredentialDaemon,
    source: CredentialSourceSpec,
) -> CredentialLoadResult {
    match source {
        CredentialSourceSpec::Native { name } => {
            native.read(&name).map_err(|error| CredentialLoadFailure {
                code: error.code(),
                source_kind: CredentialSourceKind::Native.as_str(),
                reference: name.as_str().to_string(),
            })
        }
        CredentialSourceSpec::Static {
            source_kind,
            reference,
        } => Err(CredentialLoadFailure {
            code: CredentialErrorCode::Invalid,
            source_kind,
            reference,
        }),
    }
}

fn remove_inflight(
    inner: Weak<CredentialRuntimeInner>,
    handle: &CredentialHandle,
    flight: &Arc<NativeReadFlight>,
) {
    let Some(inner) = inner.upgrade() else {
        return;
    };
    let mut inflight = inner
        .inflight
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if inflight
        .get(handle)
        .is_some_and(|current| Arc::ptr_eq(current, flight))
    {
        inflight.remove(handle);
    }
}

fn map_runtime_resolution(
    resolution: CredentialResolution,
    fallback_source_kind: &'static str,
    fallback_reference: String,
) -> CredentialLoadResult {
    match resolution {
        CredentialResolution::Resolved { value, .. } => SecretValue::new(value.as_bytes().to_vec())
            .map_err(|_| CredentialLoadFailure {
                code: CredentialErrorCode::Invalid,
                source_kind: fallback_source_kind,
                reference: fallback_reference,
            }),
        CredentialResolution::MissingReference { name } => Err(CredentialLoadFailure {
            code: CredentialErrorCode::Missing,
            source_kind: fallback_source_kind,
            reference: name,
        }),
        CredentialResolution::InvalidValue { source } => Err(CredentialLoadFailure {
            code: CredentialErrorCode::Invalid,
            source_kind: fallback_source_kind,
            reference: source.label(),
        }),
        CredentialResolution::Unconfigured => Err(CredentialLoadFailure {
            code: CredentialErrorCode::Missing,
            source_kind: fallback_source_kind,
            reference: fallback_reference,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::capabilities::{
        NativeCredentialStore, NativeStoreError, NativeStoreErrorCode,
    };
    use crate::credentials::native::NativeCredentialLocator;
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ScopedEnvironment {
        name: &'static str,
        previous: Option<OsString>,
    }

    impl ScopedEnvironment {
        fn set(name: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(name);
            // SAFETY: this test owns a unique environment variable name.
            unsafe { std::env::set_var(name, value) };
            Self { name, previous }
        }

        fn replace(&self, value: &str) {
            // SAFETY: this test owns a unique environment variable name.
            unsafe { std::env::set_var(self.name, value) };
        }
    }

    impl Drop for ScopedEnvironment {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => {
                    // SAFETY: this test owns a unique environment variable name.
                    unsafe { std::env::set_var(self.name, value) };
                }
                None => {
                    // SAFETY: this test owns a unique environment variable name.
                    unsafe { std::env::remove_var(self.name) };
                }
            }
        }
    }

    #[derive(Default)]
    struct CountingNativeStore {
        value: Mutex<Option<SecretValue>>,
        reads: AtomicUsize,
    }

    impl NativeCredentialStore for CountingNativeStore {
        fn create(
            &self,
            _locator: &NativeCredentialLocator,
            _value: &SecretValue,
        ) -> std::result::Result<(), NativeStoreError> {
            unreachable!()
        }

        fn set(
            &self,
            _locator: &NativeCredentialLocator,
            _value: &SecretValue,
        ) -> std::result::Result<(), NativeStoreError> {
            unreachable!()
        }

        fn read(
            &self,
            _locator: &NativeCredentialLocator,
        ) -> std::result::Result<SecretValue, NativeStoreError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(30));
            self.value
                .lock()
                .expect("native value lock")
                .clone()
                .ok_or_else(|| NativeStoreError::new(NativeStoreErrorCode::Missing))
        }

        fn delete(
            &self,
            _locator: &NativeCredentialLocator,
        ) -> std::result::Result<(), NativeStoreError> {
            unreachable!()
        }
    }

    #[derive(Default)]
    struct BlockingNativeStore {
        value: Mutex<Option<SecretValue>>,
        reads: AtomicUsize,
        blocked: Mutex<bool>,
        released: Condvar,
    }

    impl BlockingNativeStore {
        fn block(&self) {
            *self.blocked.lock().expect("native block lock") = true;
        }

        fn release(&self) {
            *self.blocked.lock().expect("native block lock") = false;
            self.released.notify_all();
        }

        fn set_value(&self, value: SecretValue) {
            *self.value.lock().expect("native value lock") = Some(value);
        }
    }

    impl NativeCredentialStore for BlockingNativeStore {
        fn create(
            &self,
            _locator: &NativeCredentialLocator,
            _value: &SecretValue,
        ) -> std::result::Result<(), NativeStoreError> {
            unreachable!()
        }

        fn set(
            &self,
            _locator: &NativeCredentialLocator,
            _value: &SecretValue,
        ) -> std::result::Result<(), NativeStoreError> {
            unreachable!()
        }

        fn read(
            &self,
            _locator: &NativeCredentialLocator,
        ) -> std::result::Result<SecretValue, NativeStoreError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            let mut blocked = self.blocked.lock().expect("native block lock");
            while *blocked {
                blocked = self
                    .released
                    .wait(blocked)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            drop(blocked);
            self.value
                .lock()
                .expect("native value lock")
                .clone()
                .ok_or_else(|| NativeStoreError::new(NativeStoreErrorCode::Missing))
        }

        fn delete(
            &self,
            _locator: &NativeCredentialLocator,
        ) -> std::result::Result<(), NativeStoreError> {
            unreachable!()
        }
    }

    async fn wait_for_read_count(backend: &BlockingNativeStore, expected: usize) {
        while backend.reads.load(Ordering::SeqCst) < expected {
            tokio::task::yield_now().await;
        }
    }

    async fn wait_for_no_inflight(runtime: &CredentialRuntime) {
        loop {
            if runtime
                .inner
                .inflight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    }

    async fn wait_for_singleflight_waiter(runtime: &CredentialRuntime) {
        loop {
            let waiter_joined = runtime
                .inner
                .inflight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values()
                .next()
                .is_some_and(|flight| Arc::strong_count(flight) >= 3);
            if waiter_joined {
                return;
            }
            tokio::task::yield_now().await;
        }
    }

    fn endpoint() -> ProviderEndpointKey {
        ProviderEndpointKey::new("codex", "relay", "default")
    }

    fn native_auth() -> UpstreamAuth {
        UpstreamAuth {
            auth_token_ref: Some(CredentialRef::Native {
                name: "relay.primary".to_string(),
            }),
            ..UpstreamAuth::default()
        }
    }

    #[test]
    fn identical_credentials_have_installation_local_scopes() {
        let first_store = RuntimeStore::open_in_memory().expect("open first runtime store");
        let second_store = RuntimeStore::open_in_memory().expect("open second runtime store");
        let first_runtime = CredentialRuntime::from_runtime_store(
            CredentialSourceCapabilities::server(),
            &first_store,
        )
        .expect("build first credential runtime");
        let second_runtime = CredentialRuntime::from_runtime_store(
            CredentialSourceCapabilities::server(),
            &second_store,
        )
        .expect("build second credential runtime");
        let endpoint = endpoint();
        let auth = UpstreamAuth {
            auth_token: Some("shared-upstream-credential".to_string().into()),
            ..UpstreamAuth::default()
        };

        let first = first_runtime
            .build_generation([CredentialCandidateInput {
                provider_endpoint: endpoint.clone(),
                auth: &auth,
            }])
            .expect("build first credential generation");
        let second = second_runtime
            .build_generation([CredentialCandidateInput {
                provider_endpoint: endpoint.clone(),
                auth: &auth,
            }])
            .expect("build second credential generation");

        assert_ne!(
            first
                .credential_scope_for_route_digest(&endpoint)
                .expect("first credential scope"),
            second
                .credential_scope_for_route_digest(&endpoint)
                .expect("second credential scope")
        );
    }

    #[test]
    fn generation_captures_sensitive_headers_without_debugging_values() {
        let store = RuntimeStore::open_in_memory().expect("open runtime store");
        let runtime =
            CredentialRuntime::from_runtime_store(CredentialSourceCapabilities::server(), &store)
                .expect("build credential runtime");
        let auth = UpstreamAuth {
            auth_token: Some("bearer-canary".to_string().into()),
            api_key: Some("api-canary".to_string().into()),
            ..UpstreamAuth::default()
        };
        let endpoint = endpoint();
        let generation = runtime
            .build_generation([CredentialCandidateInput {
                provider_endpoint: endpoint.clone(),
                auth: &auth,
            }])
            .expect("build generation");
        let captured = generation.capture(&endpoint);

        let bearer = captured.bearer_header().expect("bearer header");
        let api_key = captured.api_key_header().expect("API key header");
        assert_eq!(bearer.as_bytes(), b"Bearer bearer-canary");
        assert_eq!(api_key.as_bytes(), b"api-canary");
        assert!(bearer.is_sensitive());
        assert!(api_key.is_sensitive());
        let rendered = format!("{generation:?} {captured:?}");
        assert!(!rendered.contains("bearer-canary"));
        assert!(!rendered.contains("api-canary"));
    }

    #[test]
    fn credential_debug_surfaces_redact_logical_references() {
        const REFERENCE_CANARY: &str = "relay.reference-canary-7d4e";
        let store = RuntimeStore::open_in_memory().expect("open runtime store");
        let runtime =
            CredentialRuntime::from_runtime_store(CredentialSourceCapabilities::server(), &store)
                .expect("build credential runtime");
        let endpoint = endpoint();
        let auth = UpstreamAuth {
            auth_token_ref: Some(CredentialRef::Native {
                name: REFERENCE_CANARY.to_string(),
            }),
            ..UpstreamAuth::default()
        };
        let generation = runtime
            .build_generation([CredentialCandidateInput {
                provider_endpoint: endpoint.clone(),
                auth: &auth,
            }])
            .expect("build unavailable native generation");
        let captured = generation.capture(&endpoint);

        assert_eq!(
            captured.readiness_code(),
            CredentialReadinessCode::Unsupported
        );
        assert_eq!(
            captured.readiness_details()[0].reference.as_deref(),
            Some(REFERENCE_CANARY)
        );
        for rendered in [
            format!("{:?}", generation.catalog),
            format!("{:?}", generation.sources),
            format!("{:?}", captured.readiness_details()),
            format!("{captured:?}"),
        ] {
            assert!(!rendered.contains(REFERENCE_CANARY), "{rendered}");
        }
    }

    #[test]
    fn named_credentials_change_only_when_a_new_generation_is_built() {
        const ENV_NAME: &str = "CODEX_HELPER_TEST_NAMED_GENERATION_TOKEN_7D433498";
        let environment = ScopedEnvironment::set(ENV_NAME, "named-generation-a");
        let store = RuntimeStore::open_in_memory().expect("open runtime store");
        let runtime =
            CredentialRuntime::from_runtime_store(CredentialSourceCapabilities::server(), &store)
                .expect("build credential runtime");
        let endpoint = endpoint();
        let auth = UpstreamAuth::default();
        let named = || {
            [NamedCredentialReference {
                service_name: "codex".to_string(),
                name: ENV_NAME.to_string(),
                lookup: NamedCredentialLookup::ServiceCredential,
            }]
        };
        let candidates = || {
            [CredentialCandidateInput {
                provider_endpoint: endpoint.clone(),
                auth: &auth,
            }]
        };

        let first = runtime
            .build_generation_with_named(candidates(), named(), "test:named:v1")
            .expect("build first named generation");
        let captured_first = first.capture(&endpoint);
        assert_eq!(
            captured_first
                .named_credential(NamedCredentialLookup::ServiceCredential, ENV_NAME)
                .expect("captured first named credential")
                .expose(),
            b"named-generation-a"
        );

        environment.replace("named-generation-b");
        assert_eq!(
            captured_first
                .named_credential(NamedCredentialLookup::ServiceCredential, ENV_NAME)
                .expect("old generation remains captured")
                .expose(),
            b"named-generation-a"
        );

        let second = runtime
            .build_generation_from_previous_with_named(
                candidates(),
                named(),
                "test:named:v1",
                first.as_ref(),
            )
            .expect("build reloaded named generation");
        assert_eq!(second.revision(), first.revision() + 1);
        assert_ne!(second.digest(), first.digest());
        assert_eq!(
            second
                .capture(&endpoint)
                .named_credential(NamedCredentialLookup::ServiceCredential, ENV_NAME)
                .expect("captured reloaded named credential")
                .expose(),
            b"named-generation-b"
        );

        let unchanged = runtime
            .build_generation_from_previous_with_named(
                candidates(),
                named(),
                "test:named:v1",
                second.as_ref(),
            )
            .expect("build unchanged named generation");
        assert_eq!(unchanged.revision(), second.revision());
        assert_eq!(unchanged.digest(), second.digest());
        let catalog_changed = runtime
            .build_generation_from_previous_with_named(
                candidates(),
                named(),
                "test:named:v2",
                unchanged.as_ref(),
            )
            .expect("build generation with changed named catalog revision");
        assert_eq!(catalog_changed.revision(), unchanged.revision() + 1);
        assert_ne!(catalog_changed.digest(), unchanged.digest());
        assert_eq!(
            catalog_changed.capture(&endpoint).named_catalog_revision(),
            "test:named:v2"
        );
        let rendered = format!("{first:?} {captured_first:?} {second:?}");
        assert!(!rendered.contains("named-generation-a"));
        assert!(!rendered.contains("named-generation-b"));
    }

    #[test]
    fn endpoint_and_named_service_credentials_share_one_source() {
        const ENV_NAME: &str = "CODEX_HELPER_TEST_SHARED_GENERATION_TOKEN_82A9485F";
        let _environment = ScopedEnvironment::set(ENV_NAME, "shared-generation-token");
        let store = RuntimeStore::open_in_memory().expect("open runtime store");
        let runtime =
            CredentialRuntime::from_runtime_store(CredentialSourceCapabilities::server(), &store)
                .expect("build credential runtime");
        let endpoint = endpoint();
        let auth = UpstreamAuth {
            auth_token_env: Some(ENV_NAME.to_string()),
            ..UpstreamAuth::default()
        };

        let generation = runtime
            .build_generation_with_named(
                [CredentialCandidateInput {
                    provider_endpoint: endpoint.clone(),
                    auth: &auth,
                }],
                [NamedCredentialReference {
                    service_name: "codex".to_string(),
                    name: ENV_NAME.to_string(),
                    lookup: NamedCredentialLookup::ServiceCredential,
                }],
                "test:shared:v1",
            )
            .expect("build generation with shared source");

        assert_eq!(generation.catalog.sources.len(), 1);
        let captured = generation.capture(&endpoint);
        assert_eq!(
            captured
                .named_credential(NamedCredentialLookup::ServiceCredential, ENV_NAME)
                .expect("captured named credential")
                .expose(),
            b"shared-generation-token"
        );
        assert_eq!(
            captured
                .bearer_header()
                .expect("captured endpoint credential")
                .as_bytes(),
            b"Bearer shared-generation-token"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn simultaneous_refreshes_share_one_native_read() {
        let backend = Arc::new(BlockingNativeStore::default());
        backend.set_value(SecretValue::new(b"generation-a".to_vec()).expect("valid credential"));
        let store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let runtime = CredentialRuntime::from_runtime_store(
            CredentialSourceCapabilities::from_backend(Arc::clone(&backend)),
            store.as_ref(),
        )
        .expect("build credential runtime");
        let auth = native_auth();
        let endpoint = endpoint();
        let initial = runtime
            .build_generation([CredentialCandidateInput {
                provider_endpoint: endpoint,
                auth: &auth,
            }])
            .expect("build initial generation");
        assert_eq!(backend.reads.load(Ordering::SeqCst), 1);

        backend.block();
        let first_runtime = runtime.clone();
        let first_generation = Arc::clone(&initial);
        let first = tokio::spawn(async move {
            first_runtime
                .refresh_generation(
                    first_generation,
                    None,
                    CredentialRuntimeRefreshCause::AuthenticationFailure,
                )
                .await
        });
        let second_runtime = runtime.clone();
        let second = tokio::spawn(async move {
            second_runtime
                .refresh_generation(
                    initial,
                    None,
                    CredentialRuntimeRefreshCause::AuthenticationFailure,
                )
                .await
        });
        wait_for_read_count(backend.as_ref(), 2).await;
        assert_eq!(backend.reads.load(Ordering::SeqCst), 2);
        backend.release();
        first
            .await
            .expect("join first refresh")
            .expect("first refresh");
        second
            .await
            .expect("join second refresh")
            .expect("second refresh");
        assert_eq!(backend.reads.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_refresh_waiter_does_not_release_the_singleflight_read() {
        let backend = Arc::new(BlockingNativeStore::default());
        backend.set_value(SecretValue::new(b"generation-a".to_vec()).expect("valid credential"));
        let store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let runtime = CredentialRuntime::from_runtime_store(
            CredentialSourceCapabilities::from_backend(Arc::clone(&backend)),
            store.as_ref(),
        )
        .expect("build credential runtime");
        let auth = native_auth();
        let initial = runtime
            .build_generation([CredentialCandidateInput {
                provider_endpoint: endpoint(),
                auth: &auth,
            }])
            .expect("build initial generation");

        backend.block();
        let cancelled_runtime = runtime.clone();
        let cancelled_generation = Arc::clone(&initial);
        let cancelled = tokio::spawn(async move {
            cancelled_runtime
                .refresh_generation(
                    cancelled_generation,
                    None,
                    CredentialRuntimeRefreshCause::AuthenticationFailure,
                )
                .await
        });
        wait_for_read_count(backend.as_ref(), 2).await;
        cancelled.abort();
        assert!(
            cancelled
                .await
                .expect_err("cancelled waiter must stop")
                .is_cancelled()
        );

        let remaining_runtime = runtime.clone();
        let remaining = tokio::spawn(async move {
            remaining_runtime
                .refresh_generation(
                    initial,
                    None,
                    CredentialRuntimeRefreshCause::AuthenticationFailure,
                )
                .await
        });
        wait_for_singleflight_waiter(&runtime).await;
        assert_eq!(backend.reads.load(Ordering::SeqCst), 2);
        backend.release();
        remaining
            .await
            .expect("join remaining waiter")
            .expect("remaining waiter receives shared result");
        assert_eq!(backend.reads.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn timed_out_native_read_stays_singleflight_and_discards_late_result() {
        let backend = Arc::new(BlockingNativeStore::default());
        backend.set_value(SecretValue::new(b"generation-a".to_vec()).expect("valid credential"));
        let store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let mut runtime = CredentialRuntime::from_runtime_store(
            CredentialSourceCapabilities::from_backend(Arc::clone(&backend)),
            store.as_ref(),
        )
        .expect("build credential runtime");
        runtime.set_read_timeout_for_test(Duration::from_secs(60));
        let auth = native_auth();
        let endpoint = endpoint();
        let initial = runtime
            .build_generation([CredentialCandidateInput {
                provider_endpoint: endpoint.clone(),
                auth: &auth,
            }])
            .expect("build initial generation")
            .aged_for_test(NATIVE_HARD_EXPIRY + Duration::from_secs(1));

        backend.block();
        let refresh_runtime = runtime.clone();
        let refresh = tokio::spawn(async move {
            refresh_runtime
                .refresh_generation(
                    initial,
                    None,
                    CredentialRuntimeRefreshCause::AuthenticationFailure,
                )
                .await
        });
        wait_for_read_count(backend.as_ref(), 2).await;
        tokio::time::advance(Duration::from_secs(60)).await;
        let expired = refresh
            .await
            .expect("join timed out refresh")
            .expect("publish timed out refresh");
        assert!(!expired.capture(&endpoint).is_available());

        let second = runtime
            .refresh_generation(
                expired,
                None,
                CredentialRuntimeRefreshCause::AuthenticationFailure,
            )
            .await
            .expect("reuse expired singleflight");
        assert!(!second.capture(&endpoint).is_available());
        assert_eq!(backend.reads.load(Ordering::SeqCst), 2);

        backend.set_value(SecretValue::new(b"generation-b".to_vec()).expect("valid credential"));
        backend.release();
        wait_for_no_inflight(&runtime).await;
        let recovered = runtime
            .build_generation_from_previous(
                [CredentialCandidateInput {
                    provider_endpoint: endpoint.clone(),
                    auth: &auth,
                }],
                second.as_ref(),
            )
            .expect("read after expired flight completes");
        assert_eq!(backend.reads.load(Ordering::SeqCst), 3);
        assert_eq!(
            recovered
                .capture(&endpoint)
                .bearer_header()
                .expect("recovered bearer")
                .as_bytes(),
            b"Bearer generation-b"
        );
    }

    #[test]
    fn reload_baseline_preserves_unexpired_native_lkg_and_reads_shared_handle_once() {
        let backend = Arc::new(CountingNativeStore::default());
        *backend.value.lock().expect("native value lock") =
            Some(SecretValue::new(b"generation-a".to_vec()).expect("valid credential"));
        let store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let runtime = CredentialRuntime::from_runtime_store(
            CredentialSourceCapabilities::from_backend(Arc::clone(&backend)),
            store.as_ref(),
        )
        .expect("build credential runtime");
        let auth = native_auth();
        let first_endpoint = endpoint();
        let second_endpoint = ProviderEndpointKey::new("codex", "backup", "default");
        let initial = runtime
            .build_generation([
                CredentialCandidateInput {
                    provider_endpoint: first_endpoint.clone(),
                    auth: &auth,
                },
                CredentialCandidateInput {
                    provider_endpoint: second_endpoint.clone(),
                    auth: &auth,
                },
            ])
            .expect("build shared-handle generation");
        assert_eq!(backend.reads.load(Ordering::SeqCst), 1);

        *backend.value.lock().expect("native value lock") = None;
        let reloaded = runtime
            .build_generation_from_previous(
                [
                    CredentialCandidateInput {
                        provider_endpoint: first_endpoint.clone(),
                        auth: &auth,
                    },
                    CredentialCandidateInput {
                        provider_endpoint: second_endpoint.clone(),
                        auth: &auth,
                    },
                ],
                initial.as_ref(),
            )
            .expect("rebuild from current generation");
        assert_eq!(backend.reads.load(Ordering::SeqCst), 2);
        assert!(reloaded.capture(&first_endpoint).is_available());
        assert!(reloaded.capture(&second_endpoint).is_available());

        let expired = reloaded.aged_for_test(NATIVE_HARD_EXPIRY + Duration::from_secs(1));
        let hard_expired = runtime
            .build_generation_from_previous(
                [CredentialCandidateInput {
                    provider_endpoint: first_endpoint.clone(),
                    auth: &auth,
                }],
                expired.as_ref(),
            )
            .expect("rebuild expired generation");
        assert!(!hard_expired.capture(&first_endpoint).is_available());
    }

    #[tokio::test]
    async fn native_refresh_uses_bounded_stale_value_then_hard_expires() {
        let backend = Arc::new(CountingNativeStore::default());
        *backend.value.lock().expect("native value lock") =
            Some(SecretValue::new(b"generation-a".to_vec()).expect("valid credential"));
        let store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let runtime = CredentialRuntime::from_runtime_store(
            CredentialSourceCapabilities::from_backend(Arc::clone(&backend)),
            store.as_ref(),
        )
        .expect("build credential runtime");
        let auth = native_auth();
        let endpoint = endpoint();
        let initial = runtime
            .build_generation([CredentialCandidateInput {
                provider_endpoint: endpoint.clone(),
                auth: &auth,
            }])
            .expect("build initial generation");
        *backend.value.lock().expect("native value lock") = None;

        let stale = runtime
            .refresh_generation(
                initial,
                None,
                CredentialRuntimeRefreshCause::AuthenticationFailure,
            )
            .await
            .expect("publish stale generation");
        let captured_stale = stale.capture(&endpoint);
        assert!(captured_stale.is_available());
        assert_eq!(
            captured_stale.readiness_code(),
            CredentialReadinessCode::Stale
        );
        assert_eq!(
            captured_stale.readiness_details()[0].stale_cause,
            Some(CredentialReadinessCode::Missing)
        );
        assert_eq!(
            captured_stale
                .bearer_header()
                .expect("stale bearer")
                .as_bytes(),
            b"Bearer generation-a"
        );

        let expired_source = stale.aged_for_test(NATIVE_HARD_EXPIRY + Duration::from_secs(1));
        assert!(!expired_source.capture(&endpoint).is_available());
        let expired = runtime
            .refresh_generation(
                expired_source,
                None,
                CredentialRuntimeRefreshCause::AuthenticationFailure,
            )
            .await
            .expect("publish hard expiry");
        assert!(!expired.capture(&endpoint).is_available());
        assert!(expired.capture(&endpoint).bearer_header().is_none());
    }

    #[tokio::test]
    async fn unchanged_native_refresh_advances_freshness_without_generation_churn() {
        let backend = Arc::new(CountingNativeStore::default());
        *backend.value.lock().expect("native value lock") =
            Some(SecretValue::new(b"generation-a".to_vec()).expect("valid credential"));
        let store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let runtime = CredentialRuntime::from_runtime_store(
            CredentialSourceCapabilities::from_backend(Arc::clone(&backend)),
            store.as_ref(),
        )
        .expect("build credential runtime");
        let auth = native_auth();
        let endpoint = endpoint();
        let initial = runtime
            .build_generation([CredentialCandidateInput {
                provider_endpoint: endpoint.clone(),
                auth: &auth,
            }])
            .expect("build initial generation");
        let initial_revision = initial.revision();
        let initial_digest = initial.digest().to_string();
        let initial_scope = initial
            .credential_scope_for_route_digest(&endpoint)
            .expect("initial credential scope")
            .map(str::to_string);
        let initial_deadline = initial
            .next_native_deadline()
            .expect("initial native freshness deadline");

        tokio::time::sleep(Duration::from_millis(2)).await;
        let refreshed = runtime
            .refresh_generation(
                Arc::clone(&initial),
                None,
                CredentialRuntimeRefreshCause::ExplicitRefresh,
            )
            .await
            .expect("refresh unchanged native credential");

        assert_eq!(backend.reads.load(Ordering::SeqCst), 2);
        assert_eq!(refreshed.revision(), initial_revision);
        assert_eq!(refreshed.digest(), initial_digest);
        assert!(initial.marker().matches(refreshed.as_ref()));
        assert_eq!(
            refreshed
                .credential_scope_for_route_digest(&endpoint)
                .expect("refreshed credential scope"),
            initial_scope.as_deref()
        );
        assert!(
            refreshed
                .next_native_deadline()
                .expect("refreshed native freshness deadline")
                > initial_deadline
        );
        assert_eq!(
            refreshed
                .capture(&endpoint)
                .bearer_header()
                .expect("refreshed bearer")
                .as_bytes(),
            b"Bearer generation-a"
        );
    }

    #[tokio::test]
    async fn explicit_delete_invalidates_then_allows_a_new_generation() {
        let backend = Arc::new(CountingNativeStore::default());
        *backend.value.lock().expect("native value lock") =
            Some(SecretValue::new(b"generation-a".to_vec()).expect("valid credential"));
        let store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let runtime = CredentialRuntime::from_runtime_store(
            CredentialSourceCapabilities::from_backend(Arc::clone(&backend)),
            store.as_ref(),
        )
        .expect("build credential runtime");
        let auth = native_auth();
        let endpoint = endpoint();
        let initial = runtime
            .build_generation([CredentialCandidateInput {
                provider_endpoint: endpoint.clone(),
                auth: &auth,
            }])
            .expect("build initial generation");
        let deleted = runtime
            .refresh_generation(initial, None, CredentialRuntimeRefreshCause::ExplicitDelete)
            .await
            .expect("publish delete");

        let captured = deleted.capture(&endpoint);
        assert!(!captured.is_available());
        assert!(captured.bearer_header().is_none());

        *backend.value.lock().expect("native value lock") =
            Some(SecretValue::new(b"generation-b".to_vec()).expect("valid credential"));
        let recreated = runtime
            .refresh_generation(
                deleted,
                None,
                CredentialRuntimeRefreshCause::AuthenticationFailure,
            )
            .await
            .expect("publish recreated credential");
        assert_eq!(
            recreated
                .capture(&endpoint)
                .bearer_header()
                .expect("recreated bearer")
                .as_bytes(),
            b"Bearer generation-b"
        );
    }
}

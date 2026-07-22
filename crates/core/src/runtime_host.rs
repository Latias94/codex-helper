use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::Router;
use thiserror::Error;
use tokio::sync::watch;
use tokio::task::{AbortHandle, JoinHandle};

use crate::basellm_catalog::{
    BasellmCatalogSyncOptions, install_basellm_catalog_runtime_state,
    load_basellm_catalog_runtime_state, sync_basellm_catalog,
};
use crate::config::{HelperConfig, LoadedConfig, ServiceKind, load_config_with_source};
use crate::credentials::CredentialSourceCapabilities;
use crate::proxy::{
    ProxyService, admin_listener_router, admin_loopback_addr_for_proxy_port, proxy_only_router,
};
use crate::quota_sampler::{QuotaSampler, QuotaSamplerConfig, QuotaSamplerRefreshOutcome};
use crate::runtime_store::RuntimeStore;
use crate::service_target::ServiceRuntimeIdentity;
use crate::state::ProxyState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyLifetimeMode {
    Ephemeral,
    Resident,
}

impl ProxyLifetimeMode {
    pub fn is_resident(self) -> bool {
        matches!(self, Self::Resident)
    }
}

pub struct ProxyRuntime {
    pub service_name: &'static str,
    pub host: IpAddr,
    pub port: u16,
    pub admin_addr: SocketAddr,
    pub config: Arc<HelperConfig>,
    pub proxy: ProxyService,
    pub state: Arc<ProxyState>,
    pub shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    runtime_store: Arc<RuntimeStore>,
    listener: Option<tokio::net::TcpListener>,
    admin_listener: Option<tokio::net::TcpListener>,
    app: Option<Router>,
    admin_app: Option<Router>,
}

pub struct RunningProxyRuntime {
    pub service_name: &'static str,
    pub host: IpAddr,
    pub port: u16,
    pub admin_addr: SocketAddr,
    pub shutdown_tx: watch::Sender<bool>,
    runtime_join_handle: JoinHandle<RuntimeTaskJoinResults>,
    server_abort_handle: AbortHandle,
    quota_sampler_abort_handle: AbortHandle,
}

struct RuntimeTaskJoinResults {
    server: Result<Result<()>, tokio::task::JoinError>,
    quota_sampler: Result<(), tokio::task::JoinError>,
    runtime_config_driver: Result<(), tokio::task::JoinError>,
    runtime_config_driver_exited_before_shutdown: bool,
    basellm_sync: Result<(), tokio::task::JoinError>,
}

#[derive(Debug, Clone)]
pub struct ProxyRuntimeOptions {
    pub admin_addr: SocketAddr,
    pub credential_sources: CredentialSourceCapabilities,
    pub service_runtime_identity: Option<ServiceRuntimeIdentity>,
    pub local_runtime_shutdown_policy: crate::local_operator::LocalRuntimeShutdownPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyListenerKind {
    Proxy,
    Admin,
}

impl std::fmt::Display for ProxyListenerKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Proxy => "proxy",
            Self::Admin => "admin",
        })
    }
}

#[derive(Debug, Error)]
#[error("failed to bind {kind} listener on {addr}: {source}")]
pub struct ProxyListenerBindError {
    kind: ProxyListenerKind,
    addr: SocketAddr,
    #[source]
    source: std::io::Error,
}

impl ProxyListenerBindError {
    pub fn kind(&self) -> ProxyListenerKind {
        self.kind
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn source_error(&self) -> &std::io::Error {
        &self.source
    }
}

impl ProxyRuntimeOptions {
    pub fn for_proxy_port(port: u16) -> Self {
        let admin_addr = admin_loopback_addr_for_proxy_port(port);
        Self {
            admin_addr,
            credential_sources: CredentialSourceCapabilities::server(),
            service_runtime_identity: None,
            local_runtime_shutdown_policy:
                crate::local_operator::LocalRuntimeShutdownPolicy::ForegroundProcess,
        }
    }

    pub fn with_admin_addr(mut self, admin_addr: SocketAddr) -> Self {
        self.admin_addr = admin_addr;
        self
    }

    pub fn with_credential_sources(
        mut self,
        credential_sources: CredentialSourceCapabilities,
    ) -> Self {
        self.credential_sources = credential_sources;
        self
    }

    pub fn with_service_runtime_identity(
        mut self,
        service_runtime_identity: Option<ServiceRuntimeIdentity>,
    ) -> Self {
        self.service_runtime_identity = service_runtime_identity;
        self
    }

    pub fn with_local_runtime_shutdown_policy(
        mut self,
        policy: crate::local_operator::LocalRuntimeShutdownPolicy,
    ) -> Self {
        self.local_runtime_shutdown_policy = policy;
        self
    }
}

impl ProxyRuntime {
    pub fn shutdown_receiver(&self) -> watch::Receiver<bool> {
        self.shutdown_rx.clone()
    }

    pub fn start(mut self) -> RunningProxyRuntime {
        let listener = self
            .listener
            .take()
            .expect("proxy runtime listener should exist before start");
        let admin_listener = self
            .admin_listener
            .take()
            .expect("proxy runtime admin listener should exist before start");
        let app = self
            .app
            .take()
            .expect("proxy runtime app should exist before start");
        let admin_app = self
            .admin_app
            .take()
            .expect("proxy runtime admin app should exist before start");
        let shutdown_rx = self.shutdown_rx.clone();
        let sampler_proxy = self.proxy.clone();
        let sampler = QuotaSampler::new_with_outcome(QuotaSamplerConfig::default(), move || {
            let proxy = sampler_proxy.clone();
            async move {
                let response = match proxy.refresh_provider_balances(None, None, false).await {
                    Ok(response) => response,
                    Err(error) => {
                        return QuotaSamplerRefreshOutcome::Failed(format!(
                            "quota provider refresh failed: {error}"
                        ));
                    }
                };
                quota_sampler_refresh_result(&response.refresh)
            }
        });
        let quota_sampler_handle = sampler.spawn(self.shutdown_rx.clone());
        let runtime_config_driver_handle = self
            .proxy
            .spawn_runtime_config_driver(self.shutdown_rx.clone());
        let basellm_sync_handle = spawn_basellm_catalog_sync(
            Arc::clone(&self.runtime_store),
            self.proxy.clone(),
            self.shutdown_rx.clone(),
        );
        let server_handle =
            spawn_proxy_runtime_servers(listener, admin_listener, app, admin_app, shutdown_rx);
        let server_abort_handle = server_handle.abort_handle();
        let quota_sampler_abort_handle = quota_sampler_handle.abort_handle();
        let runtime_join_handle = spawn_runtime_task_joiner(
            self.shutdown_tx.clone(),
            server_handle,
            quota_sampler_handle,
            runtime_config_driver_handle,
            basellm_sync_handle,
        );

        RunningProxyRuntime {
            service_name: self.service_name,
            host: self.host,
            port: self.port,
            admin_addr: self.admin_addr,
            shutdown_tx: self.shutdown_tx,
            runtime_join_handle,
            server_abort_handle,
            quota_sampler_abort_handle,
        }
    }
}

fn quota_sampler_refresh_result(
    summary: &crate::usage_providers::UsageProviderRefreshSummary,
) -> QuotaSamplerRefreshOutcome {
    if summary.failed > 0 && summary.refreshed == 0 {
        return QuotaSamplerRefreshOutcome::Failed(format!(
            "{} of {} quota provider refresh attempts failed",
            summary.failed, summary.attempted
        ));
    }
    if summary.suppressed > 0
        && let Some(next_retry_at_ms) = summary.next_retry_at_ms
    {
        let delay = Duration::from_millis(next_retry_at_ms.saturating_sub(unix_now_ms()));
        return QuotaSamplerRefreshOutcome::Suppressed {
            wake_at: tokio::time::Instant::now() + delay,
        };
    }
    QuotaSamplerRefreshOutcome::Refreshed
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

impl RunningProxyRuntime {
    pub async fn wait(&mut self) -> Result<()> {
        let results = (&mut self.runtime_join_handle)
            .await
            .map_err(|error| anyhow::anyhow!("runtime task coordinator join error: {error}"))?;
        results.graceful_result()
    }

    /// Stops request-serving tasks immediately while allowing an in-flight catalog sync to finish.
    pub fn abort(&self) {
        let _ = self.shutdown_tx.send(true);
        self.server_abort_handle.abort();
        self.quota_sampler_abort_handle.abort();
    }

    pub async fn abort_and_wait(&mut self) -> Result<()> {
        self.abort();
        let results = (&mut self.runtime_join_handle)
            .await
            .map_err(|error| anyhow::anyhow!("runtime task coordinator join error: {error}"))?;
        results.aborted_result()
    }
}

impl RuntimeTaskJoinResults {
    fn graceful_result(self) -> Result<()> {
        self.ensure_runtime_config_driver_remained_supervised()?;
        match self.server {
            Ok(result) => result?,
            Err(error) => return Err(anyhow::anyhow!("server task join error: {error}")),
        }
        self.quota_sampler
            .map_err(|error| anyhow::anyhow!("quota sampler task join error: {error}"))?;
        self.runtime_config_driver
            .map_err(|error| anyhow::anyhow!("runtime config driver join error: {error}"))?;
        self.basellm_sync
            .map_err(|error| anyhow::anyhow!("BaseLLM sync task join error: {error}"))?;
        Ok(())
    }

    fn aborted_result(self) -> Result<()> {
        self.ensure_runtime_config_driver_remained_supervised()?;
        match self.server {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(error) if error.is_cancelled() => {}
            Err(error) => return Err(anyhow::anyhow!("server task join error: {error}")),
        }
        match self.quota_sampler {
            Ok(()) => {}
            Err(error) if error.is_cancelled() => {}
            Err(error) => return Err(anyhow::anyhow!("quota sampler task join error: {error}")),
        }
        match self.runtime_config_driver {
            Ok(()) => {}
            Err(error) if error.is_cancelled() => {}
            Err(error) => {
                return Err(anyhow::anyhow!("runtime config driver join error: {error}"));
            }
        }
        self.basellm_sync
            .map_err(|error| anyhow::anyhow!("BaseLLM sync task join error: {error}"))
    }

    fn ensure_runtime_config_driver_remained_supervised(&self) -> Result<()> {
        if !self.runtime_config_driver_exited_before_shutdown {
            return Ok(());
        }

        match &self.runtime_config_driver {
            Ok(()) => Err(anyhow::anyhow!(
                "runtime config driver exited before runtime shutdown"
            )),
            Err(error) => Err(anyhow::anyhow!("runtime config driver join error: {error}")),
        }
    }
}

fn spawn_runtime_task_joiner(
    shutdown_tx: watch::Sender<bool>,
    server_handle: JoinHandle<Result<()>>,
    quota_sampler_handle: JoinHandle<()>,
    runtime_config_driver_handle: JoinHandle<()>,
    basellm_sync_handle: JoinHandle<()>,
) -> JoinHandle<RuntimeTaskJoinResults> {
    tokio::spawn(async move {
        let mut server_handle = server_handle;
        let mut runtime_config_driver_handle = runtime_config_driver_handle;
        let shutdown_rx = shutdown_tx.subscribe();
        let (server, runtime_config_driver, runtime_config_driver_exited_before_shutdown) = tokio::select! {
            server = &mut server_handle => {
                let _ = shutdown_tx.send(true);
                let runtime_config_driver = runtime_config_driver_handle.await;
                (server, runtime_config_driver, false)
            }
            runtime_config_driver = &mut runtime_config_driver_handle => {
                let exited_before_shutdown = !*shutdown_rx.borrow();
                if exited_before_shutdown {
                    let _ = shutdown_tx.send(true);
                    server_handle.abort();
                }
                let server = server_handle.await;
                (server, runtime_config_driver, exited_before_shutdown)
            }
        };
        let (quota_sampler, basellm_sync) = tokio::join!(quota_sampler_handle, basellm_sync_handle);
        RuntimeTaskJoinResults {
            server,
            quota_sampler,
            runtime_config_driver,
            runtime_config_driver_exited_before_shutdown,
            basellm_sync,
        }
    })
}

impl Drop for RunningProxyRuntime {
    fn drop(&mut self) {
        self.abort();
    }
}

pub async fn build_proxy_runtime(
    service_kind: ServiceKind,
    host: IpAddr,
    port: u16,
) -> Result<ProxyRuntime> {
    let service_name = service_name_for_kind(service_kind);
    let loaded = load_runtime_config().await?;
    build_proxy_runtime_from_loaded(service_name, host, port, loaded).await
}

async fn load_runtime_config() -> Result<LoadedConfig> {
    load_config_with_source().await
}

async fn open_default_runtime_store() -> Result<Arc<RuntimeStore>> {
    let runtime_store = tokio::task::spawn_blocking(RuntimeStore::open_default)
        .await
        .context("runtime store initialization task failed")?
        .context("open canonical runtime store")?;
    Ok(Arc::new(runtime_store))
}

pub async fn build_proxy_runtime_from_loaded(
    service_name: &'static str,
    host: IpAddr,
    port: u16,
    loaded: LoadedConfig,
) -> Result<ProxyRuntime> {
    build_proxy_runtime_from_loaded_with_options(
        service_name,
        host,
        port,
        ProxyRuntimeOptions::for_proxy_port(port),
        loaded,
    )
    .await
}

pub async fn build_proxy_runtime_from_loaded_with_admin_addr(
    service_name: &'static str,
    host: IpAddr,
    port: u16,
    admin_addr: SocketAddr,
    loaded: LoadedConfig,
) -> Result<ProxyRuntime> {
    build_proxy_runtime_from_loaded_with_options(
        service_name,
        host,
        port,
        ProxyRuntimeOptions::for_proxy_port(port).with_admin_addr(admin_addr),
        loaded,
    )
    .await
}

pub async fn build_proxy_runtime_from_loaded_with_options(
    service_name: &'static str,
    host: IpAddr,
    port: u16,
    options: ProxyRuntimeOptions,
    loaded: LoadedConfig,
) -> Result<ProxyRuntime> {
    let runtime_store = open_default_runtime_store().await?;
    build_proxy_runtime_from_loaded_with_options_and_runtime_store(
        service_name,
        host,
        port,
        options,
        loaded,
        runtime_store,
    )
    .await
}

async fn build_proxy_runtime_from_loaded_with_options_and_runtime_store(
    service_name: &'static str,
    host: IpAddr,
    port: u16,
    options: ProxyRuntimeOptions,
    loaded: LoadedConfig,
    runtime_store: Arc<RuntimeStore>,
) -> Result<ProxyRuntime> {
    initialize_basellm_catalog(Arc::clone(&runtime_store)).await?;
    let ProxyRuntimeOptions {
        admin_addr,
        credential_sources,
        service_runtime_identity,
        local_runtime_shutdown_policy,
    } = options;
    validate_service_has_upstream(service_name, &loaded.source)?;
    let client = crate::proxy::upstream_http_client_builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .tcp_keepalive(std::time::Duration::from_secs(30))
        .pool_idle_timeout(std::time::Duration::from_secs(30))
        .build()?;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let local_runtime_shutdown_tx = shutdown_tx.clone();

    let proxy_config = Arc::new(loaded.source);
    let proxy_store = Arc::clone(&runtime_store);
    let proxy = tokio::task::spawn_blocking(move || {
        ProxyService::new_with_runtime_store_and_credential_sources(
            client,
            proxy_config,
            service_name,
            proxy_store,
            credential_sources,
        )
        .map(|proxy| {
            proxy
                .with_service_runtime_identity(service_runtime_identity)
                .with_local_runtime_shutdown(
                    port,
                    local_runtime_shutdown_policy,
                    local_runtime_shutdown_tx,
                )
        })
    })
    .await
    .context("join initial credential and runtime snapshot builder")??;
    let runtime_config = proxy.captured_runtime_config().await;
    let state = proxy.state_handle();
    let app = proxy_only_router(proxy.clone());
    let admin_app = admin_listener_router(proxy.clone());
    let addr: SocketAddr = SocketAddr::from((host, port));
    let listener = bind_listener(addr, ProxyListenerKind::Proxy).await?;
    let admin_listener = bind_listener(admin_addr, ProxyListenerKind::Admin).await?;

    Ok(ProxyRuntime {
        service_name,
        host,
        port,
        admin_addr,
        config: runtime_config,
        proxy,
        state,
        shutdown_tx,
        shutdown_rx,
        runtime_store,
        listener: Some(listener),
        admin_listener: Some(admin_listener),
        app: Some(app),
        admin_app: Some(admin_app),
    })
}

async fn initialize_basellm_catalog(runtime_store: Arc<RuntimeStore>) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let state = load_basellm_catalog_runtime_state(runtime_store.as_ref())
            .context("load BaseLLM catalog from canonical runtime store")?;
        install_basellm_catalog_runtime_state(&state);
        crate::pricing::refresh_effective_pricing_catalog();
        Ok::<(), anyhow::Error>(())
    })
    .await
    .context("join BaseLLM catalog initialization")??;
    Ok(())
}

fn spawn_basellm_catalog_sync(
    runtime_store: Arc<RuntimeStore>,
    proxy: ProxyService,
    mut shutdown_rx: watch::Receiver<bool>,
) -> JoinHandle<()> {
    const SYNC_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

    tokio::spawn(async move {
        loop {
            if *shutdown_rx.borrow() {
                return;
            }
            let sync = sync_basellm_catalog(
                Arc::clone(&runtime_store),
                BasellmCatalogSyncOptions::default(),
            );
            let (sync_result, shutdown_observed) =
                join_in_flight_on_shutdown(sync, wait_for_runtime_shutdown(&mut shutdown_rx)).await;
            let report = match sync_result {
                Ok(report) => report,
                Err(error) => {
                    tracing::warn!(%error, "BaseLLM catalog sync task failed to join");
                    return;
                }
            };
            if shutdown_observed || *shutdown_rx.borrow() {
                return;
            }
            if let Err(error) = proxy.publish_operator_pricing_catalog().await {
                tracing::warn!(error = %error, "failed to publish refreshed BaseLLM pricing catalog");
            }

            let delay = report
                .attempt
                .retry_after_unix
                .and_then(|retry_at| retry_at.checked_sub(unix_now_secs()))
                .and_then(|seconds| u64::try_from(seconds).ok())
                .map(Duration::from_secs)
                .unwrap_or(SYNC_INTERVAL);
            tokio::select! {
                biased;
                () = wait_for_runtime_shutdown(&mut shutdown_rx) => return,
                () = tokio::time::sleep(delay) => {}
            }
        }
    })
}

async fn join_in_flight_on_shutdown<T>(
    task: impl Future<Output = T> + Send + 'static,
    shutdown: impl Future<Output = ()>,
) -> (Result<T, tokio::task::JoinError>, bool)
where
    T: Send + 'static,
{
    let mut task = tokio::spawn(task);
    tokio::select! {
        biased;
        () = shutdown => (task.await, true),
        result = &mut task => (result, false),
    }
}

async fn wait_for_runtime_shutdown(shutdown_rx: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown_rx.borrow() || shutdown_rx.changed().await.is_err() {
            return;
        }
    }
}

fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

async fn bind_listener(
    addr: SocketAddr,
    kind: ProxyListenerKind,
) -> Result<tokio::net::TcpListener> {
    tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|source| ProxyListenerBindError { kind, addr, source }.into())
}

pub fn service_name_for_kind(service_kind: ServiceKind) -> &'static str {
    match service_kind {
        ServiceKind::Codex => "codex",
        ServiceKind::Claude => "claude",
    }
}

pub fn service_kind_for_name(service_name: &str) -> ServiceKind {
    match service_name {
        "claude" => ServiceKind::Claude,
        _ => ServiceKind::Codex,
    }
}

pub fn validate_service_has_upstream(service_name: &str, cfg: &HelperConfig) -> Result<()> {
    let (view, display_name, section) = match service_name {
        "claude" => (&cfg.claude, "Claude", "claude"),
        _ => (&cfg.codex, "Codex", "codex"),
    };
    let plan = crate::routing_ir::compile_route_handshake_plan(service_name, view)?;
    if plan.candidates.is_empty() {
        anyhow::bail!(
            "未找到任何可用的 {display_name} 上游配置，请在 ~/.codex-helper/config.toml 的 `{section}` 路由图中配置 provider 与 route"
        );
    }
    Ok(())
}

fn spawn_proxy_runtime_servers(
    listener: tokio::net::TcpListener,
    admin_listener: tokio::net::TcpListener,
    app: Router,
    admin_app: Router,
    shutdown_rx: watch::Receiver<bool>,
) -> JoinHandle<Result<()>> {
    let proxy_server_shutdown = {
        let mut rx = shutdown_rx.clone();
        async move {
            let _ = rx.changed().await;
        }
    };
    let admin_server_shutdown = {
        let mut rx = shutdown_rx;
        async move {
            let _ = rx.changed().await;
        }
    };

    tokio::spawn(async move {
        tokio::try_join!(
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(proxy_server_shutdown),
            axum::serve(
                admin_listener,
                admin_app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(admin_server_shutdown),
        )?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::{Condvar, Mutex as StdMutex, OnceLock};

    struct TaskDropSignal(Option<tokio::sync::oneshot::Sender<()>>);

    impl Drop for TaskDropSignal {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    struct BlockingGate {
        state: Arc<(StdMutex<bool>, Condvar)>,
    }

    impl BlockingGate {
        fn new() -> Self {
            Self {
                state: Arc::new((StdMutex::new(false), Condvar::new())),
            }
        }

        fn waiter(&self) -> Arc<(StdMutex<bool>, Condvar)> {
            Arc::clone(&self.state)
        }

        fn release(&self) {
            let (released, wake) = self.state.as_ref();
            *released
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            wake.notify_all();
        }
    }

    impl Drop for BlockingGate {
        fn drop(&mut self) {
            self.release();
        }
    }

    struct ScopedEnv {
        saved: Vec<(String, Option<String>)>,
    }

    impl ScopedEnv {
        fn new() -> Self {
            Self { saved: Vec::new() }
        }

        unsafe fn set_path(&mut self, key: &str, value: &Path) {
            self.saved.push((key.to_string(), std::env::var(key).ok()));
            unsafe { std::env::set_var(key, value) };
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            for (key, old) in self.saved.drain(..).rev() {
                unsafe {
                    match old {
                        Some(value) => std::env::set_var(&key, value),
                        None => std::env::remove_var(&key),
                    }
                }
            }
        }
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
        match LOCK.get_or_init(|| StdMutex::new(())).lock() {
            Ok(guard) => guard,
            Err(error) => error.into_inner(),
        }
    }

    #[test]
    fn quota_sampler_failure_dominates_a_suppressed_target() {
        let summary = crate::usage_providers::UsageProviderRefreshSummary {
            attempted: 2,
            failed: 1,
            suppressed: 1,
            next_retry_at_ms: Some(unix_now_ms().saturating_add(30_000)),
            ..Default::default()
        };

        let outcome = quota_sampler_refresh_result(&summary);

        assert!(
            matches!(outcome, QuotaSamplerRefreshOutcome::Failed(_)),
            "an actual failed attempt must retain sampler backoff even when another target is suppressed: {outcome:?}"
        );
    }

    #[test]
    fn quota_sampler_partial_success_does_not_back_off_healthy_targets() {
        let summary = crate::usage_providers::UsageProviderRefreshSummary {
            attempted: 2,
            refreshed: 1,
            failed: 1,
            ..Default::default()
        };

        assert_eq!(
            quota_sampler_refresh_result(&summary),
            QuotaSamplerRefreshOutcome::Refreshed
        );
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create test directory");
        }
        std::fs::write(path, contents).expect("write test file");
    }

    fn loaded_test_config(source: crate::config::HelperConfig) -> LoadedConfig {
        LoadedConfig { source }
    }

    fn loaded_runtime_test_config() -> LoadedConfig {
        let provider_id = "test".to_string();
        loaded_test_config(crate::config::HelperConfig {
            codex: crate::config::ServiceRouteConfig {
                providers: std::collections::BTreeMap::from([(
                    provider_id.clone(),
                    crate::config::ProviderConfig {
                        base_url: Some("http://127.0.0.1:9/v1".to_string()),
                        ..crate::config::ProviderConfig::default()
                    },
                )]),
                routing: Some(crate::config::RouteGraphConfig::ordered_failover(vec![
                    provider_id,
                ])),
                ..crate::config::ServiceRouteConfig::default()
            },
            ..crate::config::HelperConfig::default()
        })
    }

    fn empty_loaded_test_config() -> LoadedConfig {
        loaded_test_config(crate::config::HelperConfig::default())
    }

    fn assert_listener_bind_error(
        error: &anyhow::Error,
        expected_kind: ProxyListenerKind,
        expected_addr: SocketAddr,
    ) {
        let bind_error = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<ProxyListenerBindError>())
            .expect("runtime builder should preserve a structured listener bind error");
        assert_eq!(bind_error.kind(), expected_kind);
        assert_eq!(bind_error.addr(), expected_addr);
        assert_eq!(
            bind_error.source_error().kind(),
            std::io::ErrorKind::AddrInUse
        );
    }

    #[test]
    fn missing_upstream_errors_point_to_the_current_config_contract() {
        let config = HelperConfig::default();

        for (service_name, section) in [("codex", "`codex`"), ("claude", "`claude`")] {
            let error = validate_service_has_upstream(service_name, &config)
                .expect_err("missing upstream must be rejected");
            let message = error.to_string();

            assert!(message.contains("~/.codex-helper/config.toml"));
            assert!(message.contains(section));
            assert!(!message.contains("config.json"));
            assert!(!message.contains("auth.json"));
        }
    }

    #[test]
    fn runtime_config_load_does_not_import_codex_owned_files() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-runtime-host-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join(".codex-helper");
        let codex_home = root.join(".codex");
        std::fs::create_dir_all(&helper_home).expect("create helper home");
        std::fs::create_dir_all(&codex_home).expect("create Codex home");

        let mut env = ScopedEnv::new();
        unsafe {
            env.set_path("CODEX_HELPER_HOME", &helper_home);
            env.set_path("CODEX_HOME", &codex_home);
            env.set_path("HOME", &root);
            env.set_path("USERPROFILE", &root);
        }

        let codex_config_path = codex_home.join("config.toml");
        let codex_auth_path = codex_home.join("auth.json");
        let codex_config = r#"model_provider = "external"

[model_providers.external]
name = "external"
base_url = "https://external.example.com/v1"
env_key = "EXTERNAL_API_KEY"
"#;
        let codex_auth = r#"{"EXTERNAL_API_KEY":"test-only"}"#;
        write_file(&codex_config_path, codex_config);
        write_file(&codex_auth_path, codex_auth);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        let loaded = runtime
            .block_on(load_runtime_config())
            .expect("strictly load runtime config");

        assert!(loaded.source.codex.providers.is_empty());
        assert!(!helper_home.join("config.toml").exists());
        assert!(!helper_home.join("config.json").exists());
        assert_eq!(
            std::fs::read_to_string(&codex_config_path).expect("read Codex config"),
            codex_config
        );
        assert_eq!(
            std::fs::read_to_string(&codex_auth_path).expect("read Codex auth"),
            codex_auth
        );

        drop(env);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_store_failure_happens_before_listener_bind() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-runtime-store-order-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join(".codex-helper");
        let state_dir = helper_home.join("state");
        std::fs::create_dir_all(&state_dir).expect("create helper state directory");
        std::fs::write(state_dir.join("state.sqlite"), b"not a sqlite database")
            .expect("write corrupt runtime store");

        let mut env = ScopedEnv::new();
        unsafe {
            env.set_path("CODEX_HELPER_HOME", &helper_home);
        }

        let occupied_proxy =
            std::net::TcpListener::bind("127.0.0.1:0").expect("reserve occupied proxy address");
        let proxy_addr = occupied_proxy.local_addr().expect("occupied proxy address");
        let admin_listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("reserve temporary admin address");
        let admin_addr = admin_listener
            .local_addr()
            .expect("temporary admin address");
        drop(admin_listener);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        let result = runtime.block_on(build_proxy_runtime_from_loaded_with_options(
            "codex",
            proxy_addr.ip(),
            proxy_addr.port(),
            ProxyRuntimeOptions::for_proxy_port(proxy_addr.port()).with_admin_addr(admin_addr),
            empty_loaded_test_config(),
        ));
        let error = match result {
            Ok(_) => panic!("corrupt runtime store must prevent startup"),
            Err(error) => error,
        };

        assert!(error.chain().any(|cause| {
            matches!(
                cause.downcast_ref::<crate::runtime_store::RuntimeStoreError>(),
                Some(crate::runtime_store::RuntimeStoreError::CorruptDatabase { .. })
            )
        }));

        drop(occupied_proxy);
        drop(env);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_store_recovery_invariant_happens_before_listener_bind() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-runtime-recovery-order-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join(".codex-helper");
        let mut env = ScopedEnv::new();
        unsafe {
            env.set_path("CODEX_HELPER_HOME", &helper_home);
        }

        let request = crate::runtime_store::NewLogicalRequest {
            id: crate::runtime_store::LogicalRequestId::new(),
            begun_at_unix_ms: 1,
        };
        let attempt = crate::runtime_store::NewAttempt {
            id: crate::runtime_store::AttemptId::new(),
            logical_request_id: request.id,
            begun_at_unix_ms: 2,
            evidence: crate::runtime_store::AttemptPendingEvidence::new(
                1,
                "test-runtime",
                crate::runtime_store::AttemptRouteEvidence::default(),
            ),
        };
        let store = RuntimeStore::open_in_home(&helper_home).expect("create runtime store");
        let request_handle = store
            .transaction(|transaction| transaction.begin_logical_request(request))
            .expect("begin logical request")
            .handle;
        store
            .transaction(|transaction| transaction.begin_attempt(request_handle, attempt))
            .expect("begin upstream attempt");
        drop(store);

        let store_path = crate::runtime_store::runtime_store_path_in(&helper_home);
        let connection = rusqlite::Connection::open(&store_path).expect("open store for tampering");
        connection
            .pragma_update(None, "ignore_check_constraints", true)
            .expect("temporarily disable CHECK constraints for fault injection");
        assert_eq!(
            connection
                .pragma_query_value(None, "ignore_check_constraints", |row| row.get::<_, i64>(0))
                .expect("read CHECK constraint override"),
            1
        );
        connection
            .execute(
                "UPDATE logical_requests
                 SET terminal_outcome = 'failed',
                     terminal_at_unix_ms = 3,
                     economics_state = 'known',
                     terminal_origin = 'runtime',
                     terminal_payload_json = '{}',
                     service_name = 'codex',
                     numeric_request_id = 1
                 WHERE logical_request_id = ?1",
                rusqlite::params![request.id.as_uuid().as_bytes().as_slice()],
            )
            .expect("inject impossible lifecycle state");
        connection
            .pragma_update(None, "ignore_check_constraints", false)
            .expect("restore CHECK constraint enforcement");
        assert_eq!(
            connection
                .pragma_query_value(None, "ignore_check_constraints", |row| row.get::<_, i64>(0))
                .expect("read restored CHECK constraint setting"),
            0
        );
        drop(connection);

        let occupied_proxy =
            std::net::TcpListener::bind("127.0.0.1:0").expect("reserve occupied proxy address");
        let proxy_addr = occupied_proxy.local_addr().expect("occupied proxy address");
        let admin_listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("reserve temporary admin address");
        let admin_addr = admin_listener
            .local_addr()
            .expect("temporary admin address");
        drop(admin_listener);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        let result = runtime.block_on(build_proxy_runtime_from_loaded_with_options(
            "codex",
            proxy_addr.ip(),
            proxy_addr.port(),
            ProxyRuntimeOptions::for_proxy_port(proxy_addr.port()).with_admin_addr(admin_addr),
            empty_loaded_test_config(),
        ));
        let error = match result {
            Ok(_) => panic!("invalid recovery state must prevent startup"),
            Err(error) => error,
        };

        assert!(error.chain().any(|cause| {
            matches!(
                cause.downcast_ref::<crate::runtime_store::RuntimeStoreError>(),
                Some(crate::runtime_store::RuntimeStoreError::InvariantViolation { .. })
            )
        }));

        drop(occupied_proxy);
        drop(env);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn shutdown_releases_runtime_store_in_same_tokio_runtime() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-runtime-store-shutdown-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join(".codex-helper");
        let mut env = ScopedEnv::new();
        unsafe {
            env.set_path("CODEX_HELPER_HOME", &helper_home);
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        runtime.block_on(async {
            let proxy_runtime = build_proxy_runtime_from_loaded_with_options(
                "codex",
                IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                0,
                ProxyRuntimeOptions::for_proxy_port(0)
                    .with_admin_addr(SocketAddr::from(([127, 0, 0, 1], 0))),
                loaded_runtime_test_config(),
            )
            .await
            .expect("build proxy runtime");
            let shutdown_tx = proxy_runtime.shutdown_tx.clone();
            let mut running = proxy_runtime.start();
            shutdown_tx.send(true).expect("request runtime shutdown");
            running.wait().await.expect("wait for runtime shutdown");
            drop(shutdown_tx);

            let reopened = open_default_runtime_store()
                .await
                .expect("runtime shutdown should release the store writer lease");
            drop(reopened);
        });

        drop(env);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn abort_and_wait_joins_in_flight_basellm_runtime_document_commit() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-basellm-shutdown-commit-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join(".codex-helper");
        let release_commit = BlockingGate::new();
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            run_in_flight_basellm_shutdown_case(&helper_home, &release_commit),
        )
        .await;
        release_commit.release();
        result.expect("in-flight BaseLLM shutdown case timed out");
        let _ = std::fs::remove_dir_all(root);
    }

    async fn run_in_flight_basellm_shutdown_case(
        helper_home: &Path,
        release_commit: &BlockingGate,
    ) {
        let runtime_store = Arc::new(
            RuntimeStore::open_in_home(helper_home).expect("open runtime store for catalog sync"),
        );
        let release_commit_for_sync = release_commit.waiter();
        let runtime_store_for_sync = Arc::clone(&runtime_store);
        let (commit_started_tx, commit_started_rx) = tokio::sync::oneshot::channel();
        let (shutdown_observed_tx, shutdown_observed_rx) = tokio::sync::oneshot::channel();
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        let basellm_sync_handle = tokio::spawn(async move {
            let commit = async move {
                tokio::task::spawn_blocking(move || {
                    let _ = commit_started_tx.send(());
                    let (released, wake) = release_commit_for_sync.as_ref();
                    let mut released = released
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    while !*released {
                        released = wake
                            .wait(released)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                    runtime_store_for_sync.compare_and_write_runtime_document(
                        None,
                        crate::runtime_store::RuntimeDocumentWrite {
                            kind: crate::runtime_store::RuntimeDocumentKind::BasellmCatalog,
                            schema_version: crate::basellm_catalog::BASELLM_CATALOG_SCHEMA_VERSION,
                            payload_json: r#"{"schema_version":1,"attempt":null}"#,
                        },
                    )
                })
                .await
                .expect("join blocking BaseLLM catalog commit")
            };
            let shutdown = async {
                wait_for_runtime_shutdown(&mut shutdown_rx).await;
                let _ = shutdown_observed_tx.send(());
            };
            let (commit_result, shutdown_observed) =
                join_in_flight_on_shutdown(commit, shutdown).await;

            assert!(shutdown_observed);
            let commit = commit_result
                .expect("join supervised BaseLLM sync")
                .expect("commit BaseLLM runtime document");
            assert!(matches!(
                commit,
                crate::runtime_store::RuntimeDocumentCommit::Committed(_)
            ));
        });
        drop(runtime_store);

        let (server_started_tx, server_started_rx) = tokio::sync::oneshot::channel();
        let (server_stopped_tx, server_stopped_rx) = tokio::sync::oneshot::channel();
        let server_handle = tokio::spawn(async move {
            let _stopped = TaskDropSignal(Some(server_stopped_tx));
            let _ = server_started_tx.send(());
            std::future::pending::<Result<()>>().await
        });
        let (quota_sampler_started_tx, quota_sampler_started_rx) = tokio::sync::oneshot::channel();
        let (quota_sampler_stopped_tx, quota_sampler_stopped_rx) = tokio::sync::oneshot::channel();
        let quota_sampler_handle = tokio::spawn(async move {
            let _stopped = TaskDropSignal(Some(quota_sampler_stopped_tx));
            let _ = quota_sampler_started_tx.send(());
            std::future::pending::<()>().await;
        });
        let server_abort_handle = server_handle.abort_handle();
        let quota_sampler_abort_handle = quota_sampler_handle.abort_handle();
        let mut runtime_config_shutdown_rx = shutdown_tx.subscribe();
        let runtime_config_driver_handle = tokio::spawn(async move {
            wait_for_runtime_shutdown(&mut runtime_config_shutdown_rx).await;
        });
        let runtime_join_handle = spawn_runtime_task_joiner(
            shutdown_tx.clone(),
            server_handle,
            quota_sampler_handle,
            runtime_config_driver_handle,
            basellm_sync_handle,
        );
        let mut running = RunningProxyRuntime {
            service_name: "codex",
            host: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port: 0,
            admin_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            shutdown_tx,
            runtime_join_handle,
            server_abort_handle,
            quota_sampler_abort_handle,
        };

        commit_started_rx
            .await
            .expect("blocking catalog commit started");
        server_started_rx.await.expect("server task started");
        quota_sampler_started_rx
            .await
            .expect("quota sampler task started");
        running.abort();
        server_stopped_rx.await.expect("server task was cancelled");
        quota_sampler_stopped_rx
            .await
            .expect("quota sampler task was cancelled");
        shutdown_observed_rx
            .await
            .expect("BaseLLM supervisor observed shutdown");
        assert!(
            !running.runtime_join_handle.is_finished(),
            "shutdown must join the already-started catalog commit"
        );

        let mut graceful_wait = Box::pin(running.wait());
        let wait_poll = std::future::poll_fn(|context| {
            std::task::Poll::Ready(graceful_wait.as_mut().poll(context))
        })
        .await;
        assert!(
            wait_poll.is_pending(),
            "runtime wait must remain pending while the catalog commit is blocked"
        );
        drop(graceful_wait);

        let (wait_started_tx, wait_started_rx) = tokio::sync::oneshot::channel();
        let wait = tokio::spawn(async move {
            let _ = wait_started_tx.send(());
            running.abort_and_wait().await
        });
        wait_started_rx.await.expect("runtime wait started");
        assert!(
            !wait.is_finished(),
            "abort_and_wait must remain pending while the catalog commit is blocked"
        );

        release_commit.release();
        wait.await
            .expect("join runtime shutdown waiter")
            .expect("abort runtime and wait for catalog commit");

        let reopened = RuntimeStore::open_in_home(helper_home)
            .expect("runtime shutdown releases the writer lease after catalog commit");
        let document = reopened
            .read_runtime_document(crate::runtime_store::RuntimeDocumentKind::BasellmCatalog)
            .expect("read committed BaseLLM runtime document")
            .expect("BaseLLM runtime document was committed");
        assert_eq!(document.revision, 1);
        drop(reopened);
    }

    #[test]
    fn runtime_snapshot_failure_happens_before_listener_bind() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-runtime-snapshot-order-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join(".codex-helper");
        let mut env = ScopedEnv::new();
        unsafe {
            env.set_path("CODEX_HELPER_HOME", &helper_home);
        }

        let occupied_proxy =
            std::net::TcpListener::bind("127.0.0.1:0").expect("reserve occupied proxy address");
        let proxy_addr = occupied_proxy.local_addr().expect("occupied proxy address");
        let mut loaded = loaded_runtime_test_config();
        loaded.source.codex.routing =
            Some(crate::config::RouteGraphConfig::ordered_failover(vec![
                "missing-provider".to_string(),
            ]));

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        let result = runtime.block_on(build_proxy_runtime_from_loaded_with_options(
            "codex",
            proxy_addr.ip(),
            proxy_addr.port(),
            ProxyRuntimeOptions::for_proxy_port(proxy_addr.port())
                .with_admin_addr(SocketAddr::from(([127, 0, 0, 1], 0))),
            loaded,
        ));
        let error = match result {
            Ok(_) => panic!("invalid runtime snapshot must prevent startup"),
            Err(error) => error,
        };

        assert!(
            error
                .chain()
                .all(|cause| cause.downcast_ref::<ProxyListenerBindError>().is_none()),
            "listener bind must not run before snapshot construction: {error:#}"
        );
        assert!(
            format!("{error:#}").contains("routing references missing route or provider"),
            "unexpected snapshot build error: {error:#}"
        );

        drop(occupied_proxy);
        drop(env);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_builder_reports_proxy_and_admin_bind_addresses() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-runtime-bind-error-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join(".codex-helper");
        let mut env = ScopedEnv::new();
        unsafe {
            env.set_path("CODEX_HELPER_HOME", &helper_home);
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        runtime.block_on(async {
            let occupied_proxy =
                std::net::TcpListener::bind("127.0.0.1:0").expect("reserve occupied proxy address");
            let proxy_addr = occupied_proxy.local_addr().expect("occupied proxy address");
            let proxy_result = build_proxy_runtime_from_loaded_with_options(
                "codex",
                proxy_addr.ip(),
                proxy_addr.port(),
                ProxyRuntimeOptions::for_proxy_port(proxy_addr.port())
                    .with_admin_addr(SocketAddr::from(([127, 0, 0, 1], 0))),
                loaded_runtime_test_config(),
            )
            .await;
            let proxy_error = match proxy_result {
                Ok(_) => panic!("occupied proxy listener must fail"),
                Err(error) => error,
            };
            assert_listener_bind_error(&proxy_error, ProxyListenerKind::Proxy, proxy_addr);
            drop(occupied_proxy);

            let occupied_admin =
                std::net::TcpListener::bind("127.0.0.1:0").expect("reserve occupied admin address");
            let admin_addr = occupied_admin.local_addr().expect("occupied admin address");
            let admin_result = build_proxy_runtime_from_loaded_with_options(
                "codex",
                IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                0,
                ProxyRuntimeOptions::for_proxy_port(0).with_admin_addr(admin_addr),
                loaded_runtime_test_config(),
            )
            .await;
            let admin_error = match admin_result {
                Ok(_) => panic!("occupied admin listener must fail"),
                Err(error) => error,
            };
            assert_listener_bind_error(&admin_error, ProxyListenerKind::Admin, admin_addr);
            drop(occupied_admin);
        });

        drop(env);
        let _ = std::fs::remove_dir_all(root);
    }

    async fn run_runtime_config_driver_early_exit_case(
        runtime_config_driver_handle: JoinHandle<()>,
    ) -> anyhow::Error {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (server_stopped_tx, server_stopped_rx) = tokio::sync::oneshot::channel();
        let joiner = spawn_runtime_task_joiner(
            shutdown_tx,
            tokio::spawn(async move {
                let _stopped = TaskDropSignal(Some(server_stopped_tx));
                std::future::pending::<Result<()>>().await
            }),
            tokio::spawn(async {}),
            runtime_config_driver_handle,
            tokio::spawn(async {}),
        );

        let results = joiner.await.expect("join runtime task coordinator");
        let error = results
            .graceful_result()
            .expect_err("early runtime config driver exit must fail runtime wait");
        server_stopped_rx
            .await
            .expect("runtime config driver failure must terminate the server task");
        assert!(
            *shutdown_rx.borrow(),
            "runtime config driver failure must broadcast runtime shutdown"
        );
        error
    }

    #[tokio::test]
    async fn runtime_config_driver_panic_backtrace_does_not_render_credential_canary() {
        const CANARY: &str = "runtime-driver-panic-canary-1a467d90f28c4b35";
        let credential = crate::credentials::SecretValue::new(CANARY.as_bytes().to_vec())
            .expect("valid panic-path credential canary");
        let error = run_runtime_config_driver_early_exit_case(tokio::spawn(async move {
            std::hint::black_box(&credential);
            panic!("injected runtime config driver panic")
        }))
        .await;
        let rendered = format!("{error:#?}");
        assert!(rendered.contains("runtime config driver join error"));
        let bearer = format!("Bearer {CANARY}");
        for forbidden in [CANARY, &CANARY[..20], bearer.as_str()] {
            assert!(
                !rendered.contains(forbidden),
                "runtime panic surface leaked credential material: {rendered}"
            );
        }
    }

    #[tokio::test]
    async fn runtime_config_driver_normal_early_exit_stops_the_running_server() {
        let error = run_runtime_config_driver_early_exit_case(tokio::spawn(async {})).await;
        assert!(
            error
                .to_string()
                .contains("runtime config driver exited before runtime shutdown")
        );
    }
}

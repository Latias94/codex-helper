use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Router;
use thiserror::Error;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::{HelperConfig, LoadedConfig, ServiceKind, load_config_with_source};
use crate::proxy::{
    ProxyService, admin_listener_router, admin_loopback_addr_for_proxy_port, proxy_only_router,
};
use crate::runtime_store::RuntimeStore;
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
    server_handle: JoinHandle<Result<()>>,
    initial_balance_refresh_handle: JoinHandle<()>,
}

#[derive(Debug, Clone)]
pub struct ProxyRuntimeOptions {
    pub admin_addr: SocketAddr,
}

struct PreparedProxyRuntimeResources {
    runtime_store: Arc<RuntimeStore>,
    listener: tokio::net::TcpListener,
    admin_listener: tokio::net::TcpListener,
}

impl PreparedProxyRuntimeResources {
    fn new(
        runtime_store: Arc<RuntimeStore>,
        listener: tokio::net::TcpListener,
        admin_listener: tokio::net::TcpListener,
    ) -> Self {
        Self {
            runtime_store,
            listener,
            admin_listener,
        }
    }
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
        Self { admin_addr }
    }

    pub fn with_admin_addr(mut self, admin_addr: SocketAddr) -> Self {
        self.admin_addr = admin_addr;
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
        let initial_balance_refresh_handle = self
            .proxy
            .spawn_initial_balance_refresh(self.shutdown_rx.clone());
        let server_handle =
            spawn_proxy_runtime_servers(listener, admin_listener, app, admin_app, shutdown_rx);

        RunningProxyRuntime {
            service_name: self.service_name,
            host: self.host,
            port: self.port,
            admin_addr: self.admin_addr,
            shutdown_tx: self.shutdown_tx,
            server_handle,
            initial_balance_refresh_handle,
        }
    }
}

impl RunningProxyRuntime {
    pub async fn wait(&mut self) -> Result<()> {
        let server_result = (&mut self.server_handle)
            .await
            .map_err(|error| anyhow::anyhow!("server task join error: {error}"))?;
        let _ = self.shutdown_tx.send(true);
        (&mut self.initial_balance_refresh_handle)
            .await
            .map_err(|error| anyhow::anyhow!("initial balance refresh task join error: {error}"))?;
        server_result
    }

    pub fn abort(&self) {
        self.server_handle.abort();
        self.initial_balance_refresh_handle.abort();
    }

    pub async fn abort_and_wait(&mut self) -> Result<()> {
        self.abort();
        match (&mut self.server_handle).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(error) if error.is_cancelled() => {}
            Err(error) => return Err(anyhow::anyhow!("server task join error: {error}")),
        }
        match (&mut self.initial_balance_refresh_handle).await {
            Ok(()) => Ok(()),
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(anyhow::anyhow!(
                "initial balance refresh task join error: {error}"
            )),
        }
    }
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
    let addr: SocketAddr = SocketAddr::from((host, port));
    let listener = bind_listener(addr, ProxyListenerKind::Proxy).await?;
    let admin_listener = bind_listener(options.admin_addr, ProxyListenerKind::Admin).await?;
    let resources = PreparedProxyRuntimeResources::new(runtime_store, listener, admin_listener);
    build_proxy_runtime_from_bound_listeners_with_options(
        service_name,
        host,
        port,
        options,
        loaded,
        resources,
    )
    .await
}

async fn build_proxy_runtime_from_bound_listeners_with_options(
    service_name: &'static str,
    host: IpAddr,
    port: u16,
    options: ProxyRuntimeOptions,
    loaded: LoadedConfig,
    resources: PreparedProxyRuntimeResources,
) -> Result<ProxyRuntime> {
    let PreparedProxyRuntimeResources {
        runtime_store,
        listener,
        admin_listener,
    } = resources;
    validate_service_has_upstream(service_name, &loaded.source)?;
    let config_source = Arc::new(loaded.source);

    let client = crate::proxy::upstream_http_client_builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .tcp_keepalive(std::time::Duration::from_secs(30))
        .pool_idle_timeout(std::time::Duration::from_secs(30))
        .build()?;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let proxy = ProxyService::new_with_runtime_store(
        client,
        config_source.clone(),
        service_name,
        runtime_store,
    )?;
    let state = proxy.state_handle();
    let app = proxy_only_router(proxy.clone());
    let admin_app = admin_listener_router(proxy.clone());

    Ok(ProxyRuntime {
        service_name,
        host,
        port,
        admin_addr: options.admin_addr,
        config: config_source,
        proxy,
        state,
        shutdown_tx,
        shutdown_rx,
        listener: Some(listener),
        admin_listener: Some(admin_listener),
        app: Some(app),
        admin_app: Some(admin_app),
    })
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
    use std::sync::{Mutex as StdMutex, OnceLock};

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
}

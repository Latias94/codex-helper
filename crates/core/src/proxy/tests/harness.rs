use super::*;

#[derive(Debug)]
pub(super) struct TestProxyServer {
    pub addr: std::net::SocketAddr,
    pub handle: tokio::task::JoinHandle<()>,
}

impl TestProxyServer {
    pub(super) fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path_with_leading_slash(path))
    }

    pub(super) fn responses_url(&self) -> String {
        self.url("/v1/responses")
    }

    pub(super) fn compact_url(&self) -> String {
        self.url("/responses/compact")
    }

    pub(super) fn images_generations_url(&self) -> String {
        self.url("/v1/images/generations")
    }
}

impl Drop for TestProxyServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub(super) struct TestUpstreamServer {
    pub addr: std::net::SocketAddr,
    pub handle: tokio::task::JoinHandle<()>,
}

impl TestUpstreamServer {
    pub(super) fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    pub(super) fn upstream_config(&self) -> UpstreamConfig {
        upstream_config(self.base_url())
    }
}

impl Drop for TestUpstreamServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub(super) fn spawn_test_upstream(app: axum::Router) -> TestUpstreamServer {
    let (addr, handle) = spawn_axum_server(app);
    TestUpstreamServer { addr, handle }
}

pub(super) fn spawn_test_proxy(config: ProxyConfig) -> TestProxyServer {
    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(config),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    spawn_proxy_service(proxy)
}

pub(super) fn spawn_proxy_service(proxy: ProxyService) -> TestProxyServer {
    let app = crate::proxy::router(proxy);
    let (addr, handle) = spawn_axum_server(app);
    TestProxyServer { addr, handle }
}

pub(super) fn proxy_service(config: ProxyConfig) -> ProxyService {
    ProxyService::new(
        Client::new(),
        Arc::new(config),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    )
}

pub(super) fn upstream_config(base_url: impl Into<String>) -> UpstreamConfig {
    UpstreamConfig {
        base_url: base_url.into(),
        auth: UpstreamAuth::default(),
        tags: HashMap::new(),
        supported_models: HashMap::new(),
        model_mapping: HashMap::new(),
    }
}

pub(super) async fn post_responses_json(
    client: &reqwest::Client,
    proxy: &TestProxyServer,
    body: impl Into<reqwest::Body>,
) -> reqwest::Response {
    client
        .post(proxy.responses_url())
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .expect("send responses request")
}

pub(super) async fn post_compact_json(
    client: &reqwest::Client,
    proxy: &TestProxyServer,
    body: impl Into<reqwest::Body>,
) -> reqwest::Response {
    client
        .post(proxy.compact_url())
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .expect("send compact request")
}

pub(super) async fn post_images_generations_json(
    client: &reqwest::Client,
    proxy: &TestProxyServer,
    body: impl Into<reqwest::Body>,
) -> reqwest::Response {
    client
        .post(proxy.images_generations_url())
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .expect("send images generations request")
}

pub(super) async fn find_finished_request(
    state: &crate::state::ProxyState,
    limit: usize,
    matches: impl Fn(&crate::state::FinishedRequest) -> bool,
) -> Option<crate::state::FinishedRequest> {
    for _ in 0..100 {
        let finished = state.list_recent_finished(limit).await;
        if let Some(request) = finished.into_iter().find(|request| matches(request)) {
            return Some(request);
        }
        sleep(Duration::from_millis(20)).await;
    }
    None
}

#[derive(Debug)]
pub(super) struct BeginRequestTestBuilder<'a> {
    state: &'a crate::state::ProxyState,
    service: &'static str,
    method: &'static str,
    path: &'static str,
    session_id: Option<String>,
    session_identity_source: Option<crate::state::SessionIdentitySource>,
    client_name: Option<String>,
    client_addr: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    service_tier: Option<String>,
    started_at_ms: u64,
}

impl<'a> BeginRequestTestBuilder<'a> {
    pub(super) fn new(state: &'a crate::state::ProxyState) -> Self {
        Self {
            state,
            service: "codex",
            method: "POST",
            path: "/v1/responses",
            session_id: None,
            session_identity_source: None,
            client_name: None,
            client_addr: None,
            cwd: None,
            model: None,
            reasoning_effort: None,
            service_tier: None,
            started_at_ms: 0,
        }
    }

    pub(super) fn session_id(mut self, value: impl Into<String>) -> Self {
        self.session_id = Some(value.into());
        self
    }

    pub(super) fn client_name(mut self, value: impl Into<String>) -> Self {
        self.client_name = Some(value.into());
        self
    }

    pub(super) fn client_addr(mut self, value: impl Into<String>) -> Self {
        self.client_addr = Some(value.into());
        self
    }

    pub(super) fn cwd(mut self, value: impl Into<String>) -> Self {
        self.cwd = Some(value.into());
        self
    }

    pub(super) fn model(mut self, value: impl Into<String>) -> Self {
        self.model = Some(value.into());
        self
    }

    pub(super) fn reasoning_effort(mut self, value: impl Into<String>) -> Self {
        self.reasoning_effort = Some(value.into());
        self
    }

    pub(super) fn service_tier(mut self, value: impl Into<String>) -> Self {
        self.service_tier = Some(value.into());
        self
    }

    pub(super) fn started_at_ms(mut self, value: u64) -> Self {
        self.started_at_ms = value;
        self
    }

    pub(super) async fn begin(self) -> u64 {
        self.state
            .begin_request(
                self.service,
                self.method,
                self.path,
                self.session_id,
                self.session_identity_source,
                self.client_name,
                self.client_addr,
                self.cwd,
                self.model,
                self.reasoning_effort,
                self.service_tier,
                self.started_at_ms,
            )
            .await
    }
}

fn path_with_leading_slash(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

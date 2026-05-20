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

fn path_with_leading_slash(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

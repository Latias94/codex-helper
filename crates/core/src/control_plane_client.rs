use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use reqwest::header::{CONTENT_TYPE, HeaderValue};
use reqwest::{Client, Url};
use thiserror::Error;

use crate::dashboard_core::{
    LOCAL_OPERATOR_SESSION_METADATA_BATCH_MAX, LocalOperatorSessionMetadataRequest,
    LocalOperatorSessionMetadataResponse, OperatorReadModel, OperatorReadStatus,
};
use crate::local_operator::{
    LocalOperatorSessionRequest, LocalOperatorSessionResponse, LocalRuntimeShutdownRequest,
    LocalRuntimeShutdownResponse, local_operator_client_proof, local_operator_request_signature,
    new_local_operator_nonce, unix_time_ms, verify_local_operator_server_proof,
};
use crate::proxy::{
    ADMIN_TOKEN_ENV_VAR, ADMIN_TOKEN_HEADER, CodexRelayCapabilitiesRequest,
    CodexRelayCapabilitiesResponse, CodexRelayLiveSmokeRequest, CodexRelayLiveSmokeResponse,
    LOCAL_OPERATOR_NONCE_HEADER, LOCAL_OPERATOR_SESSION_HEADER, LOCAL_OPERATOR_SIGNATURE_HEADER,
    LOCAL_OPERATOR_TIMESTAMP_HEADER, LOCAL_V1_BALANCE_REFRESH, LOCAL_V1_CREDENTIAL_REFRESH,
    LOCAL_V1_DEFAULT_PROFILE_MUTATION, LOCAL_V1_OPERATOR_SESSION, LOCAL_V1_RELAY_CAPABILITIES,
    LOCAL_V1_RELAY_LIVE_SMOKE, LOCAL_V1_ROUTING_MUTATION, LOCAL_V1_RUNTIME_RELOAD,
    LOCAL_V1_RUNTIME_SHUTDOWN, LOCAL_V1_SERVICE_RUNTIME_READ, LOCAL_V1_SESSION_AFFINITY_MUTATION,
    LOCAL_V1_SESSION_BINDING_MUTATION, LOCAL_V1_SESSION_METADATA_READ,
    OperatorDefaultProfileMutationRequest, OperatorDefaultProfileMutationResponse,
    OperatorRoutingMutationRequest, OperatorRoutingMutationResponse, OperatorRuntimeReloadRequest,
    OperatorRuntimeReloadResponse, OperatorSessionAffinityMutationRequest,
    OperatorSessionAffinityMutationResponse, OperatorSessionBindingMutationRequest,
    OperatorSessionBindingMutationResponse, ProviderBalanceRefreshResponse,
};
use crate::request_chain::{RequestChainExport, RequestChainSelector};
use crate::service_target::{
    LocalCredentialRefreshRequest, LocalCredentialRefreshResponse, LocalServiceRuntimeReadRequest,
    LocalServiceRuntimeReadResponse,
};
const MAX_HTTP_ERROR_BODY_BYTES: usize = 4 * 1024;
const LOCAL_OPERATOR_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const LOCAL_OPERATOR_BALANCE_REFRESH_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlPlaneEndpoint {
    admin_base_url: String,
    admin_token_env: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ControlPlaneClient {
    endpoint: ControlPlaneEndpoint,
    client: Client,
    admin_token: Option<HeaderValue>,
}

#[derive(Clone)]
pub struct LocalOperatorClient {
    endpoint: ControlPlaneEndpoint,
    client: Client,
    admin_token: Option<HeaderValue>,
    local_operator_token: String,
}

impl std::fmt::Debug for LocalOperatorClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalOperatorClient")
            .field("endpoint", &self.endpoint)
            .field("local_operator_token", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

struct EstablishedLocalOperatorSession {
    client_nonce: String,
    response: LocalOperatorSessionResponse,
}

#[derive(Debug, serde::Serialize)]
struct LocalBalanceRefreshRequest {
    force: bool,
}

#[derive(Debug, Error)]
pub enum ControlPlaneError {
    #[error("admin API request path is not trusted: {reason}")]
    UntrustedRequestPath { reason: String },
    #[error("admin API not reachable at {base_url}: {source}")]
    Transport {
        base_url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("admin API returned {status}: {body_excerpt}")]
    HttpStatus { status: u16, body_excerpt: String },
    #[error("admin API response is not valid JSON: {source}")]
    Decode {
        #[source]
        source: reqwest::Error,
    },
    #[error("admin API response violates the operator read-model contract: {reason}")]
    InvalidPayload { reason: String },
}

impl ControlPlaneEndpoint {
    pub fn new(
        admin_base_url: impl Into<String>,
        admin_token_env: Option<impl Into<String>>,
    ) -> Result<Self> {
        let admin_base_url = normalize_control_plane_base_url(&admin_base_url.into())?;
        let admin_token_env = admin_token_env.map(Into::into);
        let admin_token_env = normalize_admin_token_env(admin_token_env.as_deref())?;
        require_remote_admin_token(&admin_base_url, admin_token_env.as_deref())?;
        Ok(Self {
            admin_base_url,
            admin_token_env,
        })
    }

    pub fn admin_base_url(&self) -> &str {
        &self.admin_base_url
    }

    pub fn admin_token_env(&self) -> Option<&str> {
        self.admin_token_env.as_deref()
    }
}

impl ControlPlaneClient {
    pub fn new(endpoint: ControlPlaneEndpoint) -> Result<Self> {
        Self::new_with_timeout(endpoint, Duration::from_millis(1200))
    }

    pub(crate) fn new_with_timeout(
        endpoint: ControlPlaneEndpoint,
        timeout: Duration,
    ) -> Result<Self> {
        let admin_token = load_admin_token(&endpoint)?;
        let client = control_plane_http_client(timeout)?;
        Ok(Self {
            endpoint,
            client,
            admin_token,
        })
    }

    pub fn endpoint(&self) -> &ControlPlaneEndpoint {
        &self.endpoint
    }

    async fn fetch_json<T>(&self, path: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        self.fetch_json_classified(path).await.map_err(Into::into)
    }

    async fn fetch_json_classified<T>(&self, path: &str) -> Result<T, ControlPlaneError>
    where
        T: serde::de::DeserializeOwned,
    {
        let url =
            control_plane_request_url(&self.endpoint.admin_base_url, path).map_err(|error| {
                ControlPlaneError::UntrustedRequestPath {
                    reason: error.to_string(),
                }
            })?;
        let mut request = self.client.get(url);
        if let Some(token) = self.admin_token() {
            request = request.header(ADMIN_TOKEN_HEADER, token.clone());
        }

        let response = request
            .send()
            .await
            .map_err(|source| ControlPlaneError::Transport {
                base_url: self.endpoint.admin_base_url.clone(),
                source,
            })?;
        let status = response.status();
        if !status.is_success() {
            let body_excerpt = bounded_http_error_body(response).await;
            return Err(ControlPlaneError::HttpStatus {
                status: status.as_u16(),
                body_excerpt,
            });
        }
        response
            .json::<T>()
            .await
            .map_err(|source| ControlPlaneError::Decode { source })
    }

    pub async fn operator_read_model(&self) -> Result<OperatorReadModel, ControlPlaneError> {
        let model: OperatorReadModel = self
            .fetch_json_classified("/__codex_helper/api/v1/operator/read-model")
            .await?;
        model
            .validate()
            .map_err(|reason| ControlPlaneError::InvalidPayload {
                reason: reason.to_string(),
            })?;
        if model.status != OperatorReadStatus::Ready {
            return Err(ControlPlaneError::InvalidPayload {
                reason: "operator read-model endpoint returned a non-ready client state"
                    .to_string(),
            });
        }
        Ok(model)
    }

    pub async fn refresh_operator_read_model(
        &self,
        service_name: &str,
        previous: Option<&OperatorReadModel>,
    ) -> OperatorReadModel {
        match self.operator_read_model().await {
            Ok(model) if model.service_name == service_name => model,
            Ok(_) => OperatorReadModel::disconnected(service_name),
            Err(ControlPlaneError::HttpStatus { status, .. }) if status == 401 || status == 403 => {
                OperatorReadModel::auth_required(service_name)
            }
            Err(ControlPlaneError::Decode { .. } | ControlPlaneError::InvalidPayload { .. }) => {
                OperatorReadModel::disconnected(service_name)
            }
            Err(_) => previous
                .map(OperatorReadModel::stale_from)
                .unwrap_or_else(|| OperatorReadModel::disconnected(service_name)),
        }
    }

    pub async fn request_chain(
        &self,
        selector: RequestChainSelector,
        limit: usize,
    ) -> Result<RequestChainExport> {
        self.fetch_json(&request_chain_path(selector, limit)).await
    }

    fn admin_token(&self) -> Option<&HeaderValue> {
        self.admin_token.as_ref()
    }
}

impl LocalOperatorClient {
    pub fn new(endpoint: ControlPlaneEndpoint, local_operator_token: &str) -> Result<Self> {
        if !is_loopback_control_plane_base_url(endpoint.admin_base_url()) {
            anyhow::bail!("local operator client requires a loopback admin URL");
        }
        if local_operator_token.trim().is_empty() {
            anyhow::bail!("local operator token is empty");
        }
        let admin_token = load_admin_token(&endpoint)?;
        let client = control_plane_http_client(LOCAL_OPERATOR_REQUEST_TIMEOUT)?;
        Ok(Self {
            endpoint,
            client,
            admin_token,
            local_operator_token: local_operator_token.to_string(),
        })
    }

    pub fn from_helper_home(
        endpoint: ControlPlaneEndpoint,
        helper_home: impl AsRef<Path>,
    ) -> Result<Self> {
        let token = crate::local_operator::read_local_operator_token_from(helper_home)?
            .ok_or_else(|| anyhow!("local operator capability is unavailable"))?;
        Self::new(endpoint, &token)
    }

    pub fn endpoint(&self) -> &ControlPlaneEndpoint {
        &self.endpoint
    }

    pub async fn refresh_provider_balances(
        &self,
        force: bool,
    ) -> Result<ProviderBalanceRefreshResponse, ControlPlaneError> {
        self.post_json_classified(
            LOCAL_V1_BALANCE_REFRESH,
            &LocalBalanceRefreshRequest { force },
        )
        .await
    }

    pub async fn refresh_native_credential(
        &self,
        request: &LocalCredentialRefreshRequest,
    ) -> Result<LocalCredentialRefreshResponse, ControlPlaneError> {
        let response = self
            .post_json_classified::<_, LocalCredentialRefreshResponse>(
                LOCAL_V1_CREDENTIAL_REFRESH,
                request,
            )
            .await?;
        if response.service != request.service
            || response.install_generation != request.install_generation
        {
            return Err(ControlPlaneError::InvalidPayload {
                reason: "local operator daemon returned a different service generation".to_string(),
            });
        }
        Ok(response)
    }

    pub async fn read_service_runtime(
        &self,
        request: &LocalServiceRuntimeReadRequest,
    ) -> Result<LocalServiceRuntimeReadResponse, ControlPlaneError> {
        let response = self
            .post_json_classified::<_, LocalServiceRuntimeReadResponse>(
                LOCAL_V1_SERVICE_RUNTIME_READ,
                request,
            )
            .await?;
        if response.identity.service != request.service
            || response.identity.install_generation != request.install_generation
            || response.operator.service_name
                != crate::runtime_host::service_name_for_kind(request.service)
        {
            return Err(ControlPlaneError::InvalidPayload {
                reason: "local operator daemon returned a different service runtime".to_string(),
            });
        }
        Ok(response)
    }

    pub async fn read_operator_session_metadata(
        &self,
        session_keys: Vec<String>,
    ) -> Result<LocalOperatorSessionMetadataResponse, ControlPlaneError> {
        let mut seen = std::collections::HashSet::with_capacity(session_keys.len());
        let session_keys = session_keys
            .into_iter()
            .filter(|key| seen.insert(key.clone()))
            .collect::<Vec<_>>();
        if session_keys.is_empty() {
            return self.read_operator_session_metadata_batch(Vec::new()).await;
        }

        let mut service_name = None::<String>;
        let mut sessions = std::collections::HashMap::new();
        for batch in session_keys.chunks(LOCAL_OPERATOR_SESSION_METADATA_BATCH_MAX) {
            let response = self
                .read_operator_session_metadata_batch(batch.to_vec())
                .await?;
            if service_name
                .as_deref()
                .is_some_and(|expected| expected != response.service_name)
            {
                return Err(ControlPlaneError::InvalidPayload {
                    reason:
                        "local operator daemon changed service identity between metadata batches"
                            .to_string(),
                });
            }
            service_name.get_or_insert_with(|| response.service_name.clone());
            for (key, session) in response.sessions {
                if sessions.insert(key, session).is_some() {
                    return Err(ControlPlaneError::InvalidPayload {
                        reason: "local operator daemon duplicated session metadata between batches"
                            .to_string(),
                    });
                }
            }
        }

        Ok(LocalOperatorSessionMetadataResponse {
            service_name: service_name.unwrap_or_default(),
            sessions,
        })
    }

    async fn read_operator_session_metadata_batch(
        &self,
        session_keys: Vec<String>,
    ) -> Result<LocalOperatorSessionMetadataResponse, ControlPlaneError> {
        let requested = session_keys
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        let response = self
            .post_json_classified::<_, LocalOperatorSessionMetadataResponse>(
                LOCAL_V1_SESSION_METADATA_READ,
                &LocalOperatorSessionMetadataRequest { session_keys },
            )
            .await?;
        if response.sessions.iter().any(|(key, session)| {
            !requested.contains(key) || session.raw_session_id.trim().is_empty()
        }) {
            return Err(ControlPlaneError::InvalidPayload {
                reason: "local operator daemon returned invalid session metadata".to_string(),
            });
        }
        Ok(response)
    }

    pub async fn mutate_operator_routing(
        &self,
        request: &OperatorRoutingMutationRequest,
    ) -> Result<OperatorRoutingMutationResponse, ControlPlaneError> {
        self.post_json_classified(LOCAL_V1_ROUTING_MUTATION, request)
            .await
    }

    pub async fn mutate_operator_session_affinity(
        &self,
        request: &OperatorSessionAffinityMutationRequest,
    ) -> Result<OperatorSessionAffinityMutationResponse, ControlPlaneError> {
        self.post_json_classified(LOCAL_V1_SESSION_AFFINITY_MUTATION, request)
            .await
    }

    pub async fn mutate_operator_session_binding(
        &self,
        request: &OperatorSessionBindingMutationRequest,
    ) -> Result<OperatorSessionBindingMutationResponse, ControlPlaneError> {
        self.post_json_classified(LOCAL_V1_SESSION_BINDING_MUTATION, request)
            .await
    }

    pub async fn mutate_operator_default_profile(
        &self,
        request: &OperatorDefaultProfileMutationRequest,
    ) -> Result<OperatorDefaultProfileMutationResponse, ControlPlaneError> {
        self.post_json_classified(LOCAL_V1_DEFAULT_PROFILE_MUTATION, request)
            .await
    }

    pub async fn reload_runtime(&self) -> Result<OperatorRuntimeReloadResponse, ControlPlaneError> {
        self.post_json_classified(LOCAL_V1_RUNTIME_RELOAD, &OperatorRuntimeReloadRequest {})
            .await
    }

    pub async fn shutdown_runtime(
        &self,
        request: &LocalRuntimeShutdownRequest,
    ) -> Result<LocalRuntimeShutdownResponse, ControlPlaneError> {
        let response = self
            .post_json_classified::<_, LocalRuntimeShutdownResponse>(
                LOCAL_V1_RUNTIME_SHUTDOWN,
                request,
            )
            .await?;
        if !response.accepted
            || response.service_name != request.service_name
            || response.proxy_port != request.proxy_port
        {
            return Err(ControlPlaneError::InvalidPayload {
                reason: "local operator daemon returned a different shutdown target".to_string(),
            });
        }
        Ok(response)
    }

    pub async fn inspect_relay_capabilities(
        &self,
        request: &CodexRelayCapabilitiesRequest,
    ) -> Result<CodexRelayCapabilitiesResponse, ControlPlaneError> {
        self.post_json_classified(LOCAL_V1_RELAY_CAPABILITIES, request)
            .await
    }

    pub async fn run_relay_live_smoke(
        &self,
        request: &CodexRelayLiveSmokeRequest,
    ) -> Result<CodexRelayLiveSmokeResponse, ControlPlaneError> {
        self.post_json_classified(LOCAL_V1_RELAY_LIVE_SMOKE, request)
            .await
    }

    async fn post_json_classified<RequestBody, ResponseBody>(
        &self,
        path: &str,
        body: &RequestBody,
    ) -> Result<ResponseBody, ControlPlaneError>
    where
        RequestBody: serde::Serialize + ?Sized,
        ResponseBody: serde::de::DeserializeOwned,
    {
        let body = serde_json::to_vec(body).map_err(|error| ControlPlaneError::InvalidPayload {
            reason: format!("local operator request cannot be serialized: {error}"),
        })?;
        let session = self.begin_operator_session().await?;
        let request_nonce = new_local_operator_nonce();
        let timestamp_ms = unix_time_ms();
        if timestamp_ms > session.response.expires_at_ms {
            return Err(ControlPlaneError::InvalidPayload {
                reason: "local operator session expired before use".to_string(),
            });
        }
        let signature = local_operator_request_signature(
            &self.local_operator_token,
            &session.client_nonce,
            &session.response.session_id,
            &request_nonce,
            timestamp_ms,
            path,
            &body,
        )
        .map_err(|error| ControlPlaneError::InvalidPayload {
            reason: format!("local operator request cannot be signed: {error}"),
        })?;
        let url =
            control_plane_request_url(self.endpoint.admin_base_url(), path).map_err(|error| {
                ControlPlaneError::UntrustedRequestPath {
                    reason: error.to_string(),
                }
            })?;
        let mut request = self
            .client
            .post(url)
            .timeout(local_operator_request_timeout(path))
            .header(
                LOCAL_OPERATOR_SESSION_HEADER,
                session.response.session_id.as_str(),
            )
            .header(LOCAL_OPERATOR_NONCE_HEADER, request_nonce)
            .header(LOCAL_OPERATOR_TIMESTAMP_HEADER, timestamp_ms.to_string())
            .header(LOCAL_OPERATOR_SIGNATURE_HEADER, signature)
            .header(CONTENT_TYPE, "application/json")
            .body(body);
        if let Some(token) = self.admin_token.as_ref() {
            request = request.header(ADMIN_TOKEN_HEADER, token.clone());
        }
        let response = request
            .send()
            .await
            .map_err(|source| ControlPlaneError::Transport {
                base_url: self.endpoint.admin_base_url().to_string(),
                source,
            })?;
        let status = response.status();
        if !status.is_success() {
            let body_excerpt = bounded_http_error_body(response).await;
            return Err(ControlPlaneError::HttpStatus {
                status: status.as_u16(),
                body_excerpt,
            });
        }
        response
            .json::<ResponseBody>()
            .await
            .map_err(|source| ControlPlaneError::Decode { source })
    }

    async fn begin_operator_session(
        &self,
    ) -> Result<EstablishedLocalOperatorSession, ControlPlaneError> {
        let client_nonce = new_local_operator_nonce();
        let timestamp_ms = unix_time_ms();
        let proof =
            local_operator_client_proof(&self.local_operator_token, &client_nonce, timestamp_ms)
                .map_err(|error| ControlPlaneError::InvalidPayload {
                    reason: format!("local operator session request cannot be signed: {error}"),
                })?;
        let url =
            control_plane_request_url(self.endpoint.admin_base_url(), LOCAL_V1_OPERATOR_SESSION)
                .map_err(|error| ControlPlaneError::UntrustedRequestPath {
                    reason: error.to_string(),
                })?;
        let mut request = self.client.post(url).json(&LocalOperatorSessionRequest {
            client_nonce: client_nonce.clone(),
            timestamp_ms,
            proof,
        });
        if let Some(token) = self.admin_token.as_ref() {
            request = request.header(ADMIN_TOKEN_HEADER, token.clone());
        }
        let response = request
            .send()
            .await
            .map_err(|source| ControlPlaneError::Transport {
                base_url: self.endpoint.admin_base_url().to_string(),
                source,
            })?;
        let status = response.status();
        if !status.is_success() {
            let body_excerpt = bounded_http_error_body(response).await;
            return Err(ControlPlaneError::HttpStatus {
                status: status.as_u16(),
                body_excerpt,
            });
        }
        let response = response
            .json::<LocalOperatorSessionResponse>()
            .await
            .map_err(|source| ControlPlaneError::Decode { source })?;
        verify_local_operator_server_proof(&self.local_operator_token, &client_nonce, &response)
            .map_err(|error| ControlPlaneError::InvalidPayload {
                reason: format!("local operator daemon proof failed: {error}"),
            })?;
        Ok(EstablishedLocalOperatorSession {
            client_nonce,
            response,
        })
    }
}

fn local_operator_request_timeout(path: &str) -> Duration {
    if matches!(path, LOCAL_V1_BALANCE_REFRESH | LOCAL_V1_RELAY_LIVE_SMOKE) {
        LOCAL_OPERATOR_BALANCE_REFRESH_TIMEOUT
    } else {
        LOCAL_OPERATOR_REQUEST_TIMEOUT
    }
}

fn load_admin_token(endpoint: &ControlPlaneEndpoint) -> Result<Option<HeaderValue>> {
    let Some(env_name) = endpoint.admin_token_env() else {
        return Ok(None);
    };
    let value = std::env::var(env_name).with_context(|| {
        format!("admin token environment variable {env_name} is missing or not valid Unicode")
    })?;
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("admin token environment variable {env_name} is empty");
    }
    validate_admin_token_header_value(value)
        .with_context(|| format!("admin token environment variable {env_name} is invalid"))?;
    HeaderValue::from_str(value)
        .map(Some)
        .with_context(|| format!("admin token environment variable {env_name} is invalid"))
}

pub fn configured_local_admin_token_env() -> Option<&'static str> {
    std::env::var(ADMIN_TOKEN_ENV_VAR)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|_| ADMIN_TOKEN_ENV_VAR)
}

pub fn validate_admin_token_header_value(value: &str) -> Result<()> {
    HeaderValue::from_str(value)
        .map(|_| ())
        .context("admin token cannot be encoded as an HTTP header value")
}

async fn bounded_http_error_body(response: reqwest::Response) -> String {
    let mut body = Vec::with_capacity(MAX_HTTP_ERROR_BODY_BYTES);
    let mut stream = response.bytes_stream();
    while body.len() < MAX_HTTP_ERROR_BODY_BYTES {
        let Some(chunk) = stream.next().await else {
            break;
        };
        let Ok(chunk) = chunk else {
            break;
        };
        let remaining = MAX_HTTP_ERROR_BODY_BYTES.saturating_sub(body.len());
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    String::from_utf8(body)
        .map(|body| body.trim().to_string())
        .unwrap_or_else(|_| "<non-UTF-8 response body>".to_string())
}

pub fn control_plane_http_client(timeout: Duration) -> Result<Client> {
    Client::builder()
        .no_proxy()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("failed to build control-plane HTTP client")
}

pub fn normalize_base_url(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() {
        return None;
    }
    let url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url.to_string().trim_end_matches('/').to_string())
}

pub fn normalize_control_plane_base_url(value: &str) -> Result<String> {
    let normalized = normalize_base_url(value).ok_or_else(|| {
        anyhow!(
            "admin base URL must use http:// or https:// and must not include userinfo, query, or fragment credentials"
        )
    })?;
    let url = Url::parse(&normalized).context("admin base URL is invalid")?;
    if url.path() != "/" {
        anyhow::bail!("admin base URL must not include a path");
    }
    if url.scheme() == "http" && !is_loopback_url(&url) {
        anyhow::bail!("remote admin base URL must use HTTPS; HTTP is allowed only for loopback");
    }
    Ok(normalized)
}

pub(crate) fn normalize_admin_token_env(value: Option<&str>) -> Result<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let mut chars = value.chars();
    let valid = chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_uppercase())
        && chars.all(|ch| ch == '_' || ch.is_ascii_uppercase() || ch.is_ascii_digit());
    if !valid {
        anyhow::bail!(
            "admin_token_env must be a valid environment variable name containing only ASCII uppercase letters, digits, or underscores"
        );
    }
    Ok(Some(value.to_string()))
}

pub(crate) fn require_remote_admin_token(
    admin_base_url: &str,
    admin_token_env: Option<&str>,
) -> Result<()> {
    if !is_loopback_control_plane_base_url(admin_base_url) && admin_token_env.is_none() {
        anyhow::bail!("remote relay admin URL requires admin_token_env");
    }
    Ok(())
}

pub fn is_loopback_control_plane_base_url(value: &str) -> bool {
    normalize_control_plane_base_url(value)
        .ok()
        .and_then(|value| Url::parse(&value).ok())
        .is_some_and(|url| is_loopback_url(&url))
}

pub fn control_plane_request_url(admin_base_url: &str, path: &str) -> Result<Url> {
    let admin_base_url = normalize_control_plane_base_url(admin_base_url)?;
    let base_url = Url::parse(&admin_base_url).context("admin base URL is invalid")?;

    if path.is_empty() || !path.starts_with('/') {
        anyhow::bail!("admin API path must be root-relative");
    }
    if path.starts_with("//") {
        anyhow::bail!("admin API path must not contain an authority");
    }
    if path.contains('\\') {
        anyhow::bail!("admin API path must not contain backslashes");
    }

    let url = base_url.join(path).context("admin API path is invalid")?;
    if url.fragment().is_some() {
        anyhow::bail!("admin API path must not contain a fragment");
    }
    if url.origin() != base_url.origin() {
        anyhow::bail!("admin API path changed the configured origin");
    }
    Ok(url)
}

fn is_loopback_url(url: &Url) -> bool {
    url.host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("localhost"))
        || ip_host(url).is_some_and(|addr| addr.is_loopback())
}

fn ip_host(url: &Url) -> Option<IpAddr> {
    let address = url
        .host_str()?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .ok()?;
    match address {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .or(Some(IpAddr::V6(address))),
        address => Some(address),
    }
}

fn request_chain_path(selector: RequestChainSelector, limit: usize) -> String {
    let selector = selector.normalized();
    let mut url =
        Url::parse("http://localhost/__codex_helper/api/v1/request-ledger/chain").expect("url");
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("limit", &limit.to_string());
        if let Some(trace_id) = selector.trace_id {
            pairs.append_pair("trace_id", &trace_id);
        }
        if let Some(request_id) = selector.request_id {
            pairs.append_pair("request_id", &request_id.to_string());
        }
        if let Some(session_id) = selector.session_id {
            pairs.append_pair("session", &session_id);
        }
    }
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};

    use axum::Router;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::Redirect;
    use axum::routing::{get, post};
    use serde_json::json;
    use tokio::net::TcpListener;

    use crate::dashboard_core::{
        ApiV1OperatorSummary, OperatorReadData, OperatorReadIssue, OperatorReadStatus,
        OperatorRevisionBundle,
    };

    use super::*;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn long_running_local_operator_actions_have_a_longer_timeout() {
        assert_eq!(
            local_operator_request_timeout(LOCAL_V1_BALANCE_REFRESH),
            LOCAL_OPERATOR_BALANCE_REFRESH_TIMEOUT
        );
        assert_eq!(
            local_operator_request_timeout(LOCAL_V1_RELAY_LIVE_SMOKE),
            LOCAL_OPERATOR_BALANCE_REFRESH_TIMEOUT
        );
        assert!(
            LOCAL_OPERATOR_BALANCE_REFRESH_TIMEOUT > LOCAL_OPERATOR_REQUEST_TIMEOUT,
            "provider sweeps and relay smoke tests can exceed the default local action timeout"
        );
        assert_eq!(
            local_operator_request_timeout(LOCAL_V1_ROUTING_MUTATION),
            LOCAL_OPERATOR_REQUEST_TIMEOUT
        );
    }

    struct ScopedEnv {
        _lock: MutexGuard<'static, ()>,
        key: &'static str,
        previous: Option<OsString>,
    }

    impl ScopedEnv {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous = std::env::var_os(key);
            // SAFETY: this guard serializes this module's test-only environment mutations and
            // holds the lock until the original value has been restored.
            unsafe { std::env::set_var(key, value) };
            Self {
                _lock: lock,
                key,
                previous,
            }
        }

        fn remove(key: &'static str) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous = std::env::var_os(key);
            // SAFETY: this guard serializes this module's test-only environment mutations and
            // holds the lock until the original value has been restored.
            unsafe { std::env::remove_var(key) };
            Self {
                _lock: lock,
                key,
                previous,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: the environment lock is still held while the original value is restored.
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    async fn spawn_server(app: Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local address");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        (addr, handle)
    }

    fn ready_operator_model(service_name: &str, revision: u64) -> OperatorReadModel {
        let service_name = service_name.to_string();
        OperatorReadModel::ready(
            service_name.clone(),
            1_700_000_000_000 + revision,
            OperatorRevisionBundle {
                runtime_revision: revision,
                runtime_digest: format!("runtime-{revision}"),
                route_digest: format!("route-{revision}"),
                catalog_revision: format!("catalog-{revision}"),
                pricing_revision: format!("pricing-{revision}"),
                operator_pricing_revision: format!("operator-pricing-{revision}"),
                policy_revision: revision + 1,
                ledger_revision: format!("operator-ledger-v1:test-store:{}", revision + 2),
            },
            OperatorReadData {
                summary: ApiV1OperatorSummary {
                    api_version: 1,
                    service_name,
                    runtime: Default::default(),
                    counts: Default::default(),
                    retry: Default::default(),
                    credential_readiness: None,
                    sessions: Vec::new(),
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
            },
        )
    }

    #[tokio::test]
    async fn fake_loopback_daemon_never_receives_the_reusable_operator_token() {
        let token = format!("codex-helper-local-v1-{}", "a".repeat(64));
        let captured = Arc::new(Mutex::new(Vec::<String>::new()));
        let route_capture = captured.clone();
        let app = Router::new().route(
            LOCAL_V1_OPERATOR_SESSION,
            post(move |headers: HeaderMap, body: String| {
                let route_capture = route_capture.clone();
                async move {
                    let mut values = headers
                        .values()
                        .filter_map(|value| value.to_str().ok())
                        .map(str::to_string)
                        .collect::<Vec<_>>();
                    values.push(body);
                    route_capture
                        .lock()
                        .expect("capture request")
                        .extend(values);
                    axum::Json(LocalOperatorSessionResponse {
                        session_id: "b".repeat(64),
                        expires_at_ms: unix_time_ms().saturating_add(30_000),
                        proof: "0".repeat(64),
                    })
                }
            }),
        );
        let (addr, handle) = spawn_server(app).await;
        let endpoint = ControlPlaneEndpoint::new(format!("http://{addr}"), None::<String>)
            .expect("loopback endpoint");
        let client = LocalOperatorClient::new(endpoint, &token).expect("operator client");

        let error = client
            .refresh_provider_balances(true)
            .await
            .expect_err("fake daemon cannot prove token possession");

        assert!(matches!(error, ControlPlaneError::InvalidPayload { .. }));
        assert!(
            captured
                .lock()
                .expect("captured request")
                .iter()
                .all(|value| !value.contains(&token)),
            "the reusable token must never cross loopback HTTP"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn refresh_operator_read_model_returns_valid_ready_bundle() {
        let expected = ready_operator_model("codex", 41);
        let response = expected.clone();
        let hits = Arc::new(AtomicUsize::new(0));
        let route_hits = hits.clone();
        let app = Router::new().route(
            "/__codex_helper/api/v1/operator/read-model",
            get(move || {
                let response = response.clone();
                let route_hits = route_hits.clone();
                async move {
                    route_hits.fetch_add(1, Ordering::SeqCst);
                    axum::Json(response)
                }
            }),
        );
        let (addr, handle) = spawn_server(app).await;
        let endpoint = ControlPlaneEndpoint::new(format!("http://{addr}"), None::<String>)
            .expect("loopback endpoint");
        let client = ControlPlaneClient::new(endpoint).expect("control-plane client");

        let actual = client.refresh_operator_read_model("codex", None).await;

        assert_eq!(actual, expected);
        assert!(actual.can_use_runtime_actions());
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        handle.abort();
    }

    #[tokio::test]
    async fn refresh_operator_read_model_preserves_previous_bundle_when_refresh_fails() {
        let previous = ready_operator_model("codex", 52);
        let previous_revisions = previous.revisions.clone();
        let previous_data = previous.data.clone();
        let app = Router::new().route(
            "/__codex_helper/api/v1/operator/read-model",
            get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        );
        let (addr, handle) = spawn_server(app).await;
        let endpoint = ControlPlaneEndpoint::new(format!("http://{addr}"), None::<String>)
            .expect("loopback endpoint");
        let client = ControlPlaneClient::new(endpoint).expect("control-plane client");

        let stale = client
            .refresh_operator_read_model("codex", Some(&previous))
            .await;

        assert_eq!(stale.status, OperatorReadStatus::Stale);
        assert_eq!(stale.issue, Some(OperatorReadIssue::RefreshFailed));
        assert_eq!(stale.captured_at_ms, previous.captured_at_ms);
        assert_eq!(stale.revisions, previous_revisions);
        assert_eq!(stale.data, previous_data);
        assert!(!stale.can_use_runtime_actions());
        handle.abort();
    }

    #[tokio::test]
    async fn refresh_operator_read_model_drops_previous_bundle_when_auth_is_required() {
        for status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
            let previous = ready_operator_model("codex", u64::from(status.as_u16()));
            let app = Router::new().route(
                "/__codex_helper/api/v1/operator/read-model",
                get(move || async move { status }),
            );
            let (addr, handle) = spawn_server(app).await;
            let endpoint = ControlPlaneEndpoint::new(format!("http://{addr}"), None::<String>)
                .expect("loopback endpoint");
            let client = ControlPlaneClient::new(endpoint).expect("control-plane client");

            let unavailable = client
                .refresh_operator_read_model("codex", Some(&previous))
                .await;

            assert_eq!(unavailable.status, OperatorReadStatus::AuthRequired);
            assert_eq!(unavailable.issue, Some(OperatorReadIssue::AuthRequired));
            assert!(unavailable.revisions.is_none());
            assert!(unavailable.data.is_none());
            assert!(!unavailable.can_use_runtime_actions());
            handle.abort();
        }
    }

    #[tokio::test]
    async fn refresh_operator_read_model_classifies_disconnected_and_invalid_payload() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind unused port");
        let disconnected_addr = listener.local_addr().expect("unused address");
        drop(listener);
        let endpoint =
            ControlPlaneEndpoint::new(format!("http://{disconnected_addr}"), None::<String>)
                .expect("loopback endpoint");
        let client = ControlPlaneClient::new(endpoint).expect("control-plane client");

        let disconnected = client.refresh_operator_read_model("codex", None).await;

        assert_eq!(disconnected.status, OperatorReadStatus::Disconnected);
        assert_eq!(disconnected.issue, Some(OperatorReadIssue::Disconnected));
        assert!(disconnected.revisions.is_none());
        assert!(disconnected.data.is_none());

        let app = Router::new().route(
            "/__codex_helper/api/v1/operator/read-model",
            get(|| async { "not an operator read model" }),
        );
        let (addr, handle) = spawn_server(app).await;
        let endpoint = ControlPlaneEndpoint::new(format!("http://{addr}"), None::<String>)
            .expect("loopback endpoint");
        let client = ControlPlaneClient::new(endpoint).expect("control-plane client");

        let invalid = client.refresh_operator_read_model("codex", None).await;

        assert_eq!(invalid.status, OperatorReadStatus::Disconnected);
        assert_eq!(invalid.issue, Some(OperatorReadIssue::Disconnected));
        assert!(invalid.revisions.is_none());
        assert!(invalid.data.is_none());
        handle.abort();
    }

    #[tokio::test]
    async fn refresh_operator_read_model_rejects_non_ready_server_payload() {
        let server_payload = OperatorReadModel::stale_from(&ready_operator_model("codex", 63));
        let app = Router::new().route(
            "/__codex_helper/api/v1/operator/read-model",
            get(move || {
                let server_payload = server_payload.clone();
                async move { axum::Json(server_payload) }
            }),
        );
        let (addr, handle) = spawn_server(app).await;
        let endpoint = ControlPlaneEndpoint::new(format!("http://{addr}"), None::<String>)
            .expect("loopback endpoint");
        let client = ControlPlaneClient::new(endpoint).expect("control-plane client");

        let invalid = client.refresh_operator_read_model("codex", None).await;

        assert_eq!(invalid.status, OperatorReadStatus::Disconnected);
        assert_eq!(invalid.issue, Some(OperatorReadIssue::Disconnected));
        assert!(invalid.revisions.is_none());
        assert!(invalid.data.is_none());
        handle.abort();
    }

    #[test]
    fn control_plane_endpoint_normalizes_admin_base_url() {
        let endpoint = ControlPlaneEndpoint::new(" https://nas.example:4211/ ", Some("TOKEN_ENV"))
            .expect("endpoint");

        assert_eq!(endpoint.admin_base_url(), "https://nas.example:4211");
        assert_eq!(endpoint.admin_token_env(), Some("TOKEN_ENV"));
    }

    #[test]
    fn control_plane_endpoint_accepts_loopback_http() {
        let endpoint = ControlPlaneEndpoint::new("http://127.0.0.1:4211/", None::<String>)
            .expect("loopback endpoint");

        assert_eq!(endpoint.admin_base_url(), "http://127.0.0.1:4211");
    }

    #[test]
    fn control_plane_endpoint_rejects_remote_without_token_env() {
        let error = ControlPlaneEndpoint::new("https://relay.example:4211", None::<String>)
            .expect_err("remote endpoint must name a token environment variable");

        assert!(
            error.to_string().contains("admin_token_env"),
            "unexpected: {error}"
        );
    }

    #[test]
    fn control_plane_endpoint_rejects_invalid_token_env_name() {
        let error =
            ControlPlaneEndpoint::new("https://relay.example:4211", Some("token with spaces"))
                .expect_err("token environment variable name must be validated");

        assert!(
            error.to_string().contains("environment variable name"),
            "unexpected: {error}"
        );
    }

    #[test]
    fn control_plane_client_rejects_missing_empty_and_invalid_header_tokens_before_io() {
        const MISSING_TOKEN_ENV: &str = "CODEX_HELPER_TEST_MISSING_CONTROL_TOKEN";
        const EMPTY_TOKEN_ENV: &str = "CODEX_HELPER_TEST_EMPTY_CONTROL_TOKEN";
        const INVALID_TOKEN_ENV: &str = "CODEX_HELPER_TEST_INVALID_CONTROL_TOKEN";

        let _missing = ScopedEnv::remove(MISSING_TOKEN_ENV);
        let endpoint =
            ControlPlaneEndpoint::new("https://relay.example:4211", Some(MISSING_TOKEN_ENV))
                .expect("remote endpoint config");
        let error = ControlPlaneClient::new(endpoint)
            .expect_err("missing token must fail before any request");
        assert!(error.to_string().contains(MISSING_TOKEN_ENV));
        drop(_missing);

        let _empty = ScopedEnv::set(EMPTY_TOKEN_ENV, "   ");
        let endpoint =
            ControlPlaneEndpoint::new("https://relay.example:4211", Some(EMPTY_TOKEN_ENV))
                .expect("remote endpoint config");
        let error = ControlPlaneClient::new(endpoint)
            .expect_err("empty token must fail before any request");
        assert!(error.to_string().contains(EMPTY_TOKEN_ENV));
        drop(_empty);

        let _invalid = ScopedEnv::set(INVALID_TOKEN_ENV, "invalid\nheader");
        let endpoint =
            ControlPlaneEndpoint::new("https://relay.example:4211", Some(INVALID_TOKEN_ENV))
                .expect("remote endpoint config");
        let error = ControlPlaneClient::new(endpoint)
            .expect_err("invalid header token must fail before any request");
        assert!(error.to_string().contains(INVALID_TOKEN_ENV));
    }

    #[test]
    fn control_plane_client_none_does_not_fall_back_to_global_admin_token_env() {
        let _token = ScopedEnv::set(
            crate::proxy::ADMIN_TOKEN_ENV_VAR,
            "must-not-be-selected-implicitly",
        );
        let endpoint = ControlPlaneEndpoint::new("http://127.0.0.1:4211", None::<String>)
            .expect("loopback endpoint");
        let client = ControlPlaneClient::new(endpoint).expect("control-plane client");

        assert_eq!(client.admin_token(), None);
    }

    #[test]
    fn configured_local_admin_token_env_requires_a_non_empty_unicode_value() {
        {
            let _missing = ScopedEnv::remove(ADMIN_TOKEN_ENV_VAR);
            assert_eq!(configured_local_admin_token_env(), None);
        }
        {
            let _empty = ScopedEnv::set(ADMIN_TOKEN_ENV_VAR, "  \t  ");
            assert_eq!(configured_local_admin_token_env(), None);
        }
        {
            let _configured = ScopedEnv::set(ADMIN_TOKEN_ENV_VAR, "local-admin-token");
            assert_eq!(
                configured_local_admin_token_env(),
                Some(ADMIN_TOKEN_ENV_VAR)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn configured_local_admin_token_env_rejects_non_unicode_values() {
        use std::os::unix::ffi::OsStringExt;

        let _invalid = ScopedEnv::set(
            ADMIN_TOKEN_ENV_VAR,
            OsString::from_vec(vec![b't', b'o', b'k', b'e', b'n', 0xff]),
        );

        assert_eq!(configured_local_admin_token_env(), None);
    }

    #[test]
    fn configured_local_admin_token_env_explicitly_enables_loopback_client_auth() {
        let _token = ScopedEnv::set(ADMIN_TOKEN_ENV_VAR, "local-admin-token");
        let endpoint =
            ControlPlaneEndpoint::new("http://127.0.0.1:4211", configured_local_admin_token_env())
                .expect("loopback endpoint");
        let client = ControlPlaneClient::new(endpoint).expect("control-plane client");

        assert_eq!(
            client.endpoint().admin_token_env(),
            Some(ADMIN_TOKEN_ENV_VAR)
        );
        assert_eq!(
            client.admin_token().and_then(|value| value.to_str().ok()),
            Some("local-admin-token")
        );
    }

    #[tokio::test]
    async fn control_plane_client_loopback_without_token_env_sends_no_token() {
        let _token = ScopedEnv::set(
            crate::proxy::ADMIN_TOKEN_ENV_VAR,
            "must-not-reach-loopback-server",
        );
        let received_tokens = Arc::new(Mutex::new(Vec::<String>::new()));
        let tokens = received_tokens.clone();
        let server = Router::new().route(
            "/status",
            get(move |headers: axum::http::HeaderMap| {
                let tokens = tokens.clone();
                async move {
                    if let Some(token) = headers
                        .get(ADMIN_TOKEN_HEADER)
                        .and_then(|value| value.to_str().ok())
                    {
                        tokens
                            .lock()
                            .expect("received token lock")
                            .push(token.to_string());
                    }
                    axum::Json(json!({ "ok": true }))
                }
            }),
        );
        let (server_addr, server_handle) = spawn_server(server).await;
        let endpoint = ControlPlaneEndpoint::new(format!("http://{server_addr}"), None::<String>)
            .expect("loopback endpoint");
        let client = ControlPlaneClient::new(endpoint).expect("control-plane client");

        let body = client
            .fetch_json::<serde_json::Value>("/status")
            .await
            .expect("loopback control-plane response");

        assert_eq!(body, json!({ "ok": true }));
        assert!(
            received_tokens
                .lock()
                .expect("received token lock")
                .is_empty()
        );
        server_handle.abort();
    }

    #[test]
    fn control_plane_endpoint_accepts_ipv4_mapped_loopback_http() {
        let endpoint = ControlPlaneEndpoint::new("http://[::ffff:127.0.0.1]:4211/", None::<String>)
            .expect("IPv4-mapped loopback endpoint");

        assert_eq!(endpoint.admin_base_url(), "http://[::ffff:7f00:1]:4211");
    }

    #[test]
    fn control_plane_endpoint_rejects_remote_http_even_with_token_env() {
        let err = ControlPlaneEndpoint::new("http://nas.example:4211", Some("NAS_TOKEN"))
            .expect_err("remote HTTP must not be approved by token presence");

        assert!(err.to_string().contains("HTTPS"), "unexpected: {err}");
    }

    #[test]
    fn control_plane_endpoint_rejects_credential_bearing_urls() {
        for value in [
            "https://user:secret@nas.example:4211",
            "https://nas.example:4211?token=secret",
            "https://nas.example:4211/#token=secret",
        ] {
            let err = ControlPlaneEndpoint::new(value, Some("NAS_TOKEN"))
                .expect_err("credential-bearing admin URL must fail");
            assert!(
                err.to_string().contains("userinfo")
                    || err.to_string().contains("query")
                    || err.to_string().contains("fragment"),
                "unexpected error for {value}: {err}"
            );
        }
    }

    #[test]
    fn control_plane_endpoint_rejects_non_http_url() {
        let err = ControlPlaneEndpoint::new("file:///tmp/socket", None::<String>)
            .expect_err("non-http admin url should fail");

        assert!(err.to_string().contains("admin base URL"));
    }

    #[test]
    fn control_plane_request_url_accepts_root_relative_path_and_query() {
        let url = control_plane_request_url(
            "https://relay.example:4211",
            "/__codex_helper/api/v1/request-ledger/chain?limit=40&session=test",
        )
        .expect("root-relative admin path should remain on the configured origin");

        assert_eq!(
            url.as_str(),
            "https://relay.example:4211/__codex_helper/api/v1/request-ledger/chain?limit=40&session=test"
        );
    }

    #[test]
    fn control_plane_request_url_rejects_untrusted_path_forms() {
        for path in [
            "//attacker.example/steal",
            "https://attacker.example/steal",
            "/\\\\attacker.example/steal",
            "/__codex_helper/api/v1/operator/read-model#secret",
        ] {
            let error = control_plane_request_url("https://relay.example:4211", path)
                .expect_err("admin request path must not change or obscure its authority");

            assert!(
                error.to_string().contains("path"),
                "unexpected error for {path}: {error}"
            );
        }
    }

    #[tokio::test]
    async fn control_plane_client_does_not_follow_redirects() {
        let target_hits = Arc::new(AtomicUsize::new(0));
        let hits = target_hits.clone();
        let target = Router::new().route(
            "/target",
            get(move || {
                let hits = hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    axum::Json(json!({ "ok": true }))
                }
            }),
        );
        let (target_addr, target_handle) = spawn_server(target).await;

        let target_url = format!("http://{target_addr}/target");
        let source = Router::new().route(
            "/redirect",
            get(move || {
                let target_url = target_url.clone();
                async move { Redirect::temporary(&target_url) }
            }),
        );
        let (source_addr, source_handle) = spawn_server(source).await;

        let endpoint = ControlPlaneEndpoint::new(format!("http://{source_addr}"), None::<String>)
            .expect("loopback endpoint");
        let client = ControlPlaneClient::new(endpoint).expect("client");
        let err = client
            .fetch_json::<serde_json::Value>("/redirect")
            .await
            .expect_err("redirect must not be followed");

        assert!(err.to_string().contains("307"), "unexpected: {err}");
        assert_eq!(target_hits.load(Ordering::SeqCst), 0);
        source_handle.abort();
        target_handle.abort();
    }

    #[tokio::test]
    async fn control_plane_client_bounds_http_error_body() {
        let body = "x".repeat(MAX_HTTP_ERROR_BODY_BYTES * 2);
        let app = Router::new().route(
            "/error",
            get(move || {
                let body = body.clone();
                async move { (StatusCode::BAD_GATEWAY, body) }
            }),
        );
        let (addr, handle) = spawn_server(app).await;
        let endpoint = ControlPlaneEndpoint::new(format!("http://{addr}"), None::<String>)
            .expect("loopback endpoint");
        let client = ControlPlaneClient::new(endpoint).expect("control-plane client");

        let error = client
            .fetch_json_classified::<serde_json::Value>("/error")
            .await
            .expect_err("error response must remain classified");

        let ControlPlaneError::HttpStatus { body_excerpt, .. } = error else {
            panic!("expected HTTP status error");
        };
        assert_eq!(body_excerpt.len(), MAX_HTTP_ERROR_BODY_BYTES);
        handle.abort();
    }

    #[tokio::test]
    async fn control_plane_client_sends_token_only_to_pinned_origin() {
        const TOKEN_ENV: &str = "CODEX_HELPER_TEST_PINNED_ORIGIN_TOKEN";
        const TOKEN: &str = "pinned-origin-token";
        let _token = ScopedEnv::set(TOKEN_ENV, TOKEN);

        let source_tokens = Arc::new(Mutex::new(Vec::<String>::new()));
        let target_hits = Arc::new(AtomicUsize::new(0));
        let token_hits = Arc::new(AtomicUsize::new(0));
        let hits = target_hits.clone();
        let tokens = token_hits.clone();
        let target = Router::new().route(
            "/target",
            get(move |headers: axum::http::HeaderMap| {
                let hits = hits.clone();
                let tokens = tokens.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    if headers.contains_key(ADMIN_TOKEN_HEADER) {
                        tokens.fetch_add(1, Ordering::SeqCst);
                    }
                    axum::Json(json!({ "ok": true }))
                }
            }),
        );
        let (target_addr, target_handle) = spawn_server(target).await;

        let target_url = format!("http://{target_addr}/target");
        let tokens = source_tokens.clone();
        let source = Router::new().route(
            "/redirect",
            get(move |headers: axum::http::HeaderMap| {
                let target_url = target_url.clone();
                let tokens = tokens.clone();
                async move {
                    if let Some(token) = headers
                        .get(ADMIN_TOKEN_HEADER)
                        .and_then(|value| value.to_str().ok())
                    {
                        tokens
                            .lock()
                            .expect("source token lock")
                            .push(token.to_string());
                    }
                    Redirect::temporary(&target_url)
                }
            }),
        );
        let (source_addr, source_handle) = spawn_server(source).await;

        let endpoint = ControlPlaneEndpoint::new(format!("http://{source_addr}"), Some(TOKEN_ENV))
            .expect("source endpoint");
        let client = ControlPlaneClient::new(endpoint).expect("client");
        let error = client
            .fetch_json::<serde_json::Value>("/redirect")
            .await
            .expect_err("redirect must not be followed");

        assert!(error.to_string().contains("307"), "unexpected: {error}");
        assert_eq!(
            source_tokens.lock().expect("source token lock").as_slice(),
            [TOKEN]
        );
        assert_eq!(target_hits.load(Ordering::SeqCst), 0);
        assert_eq!(token_hits.load(Ordering::SeqCst), 0);
        source_handle.abort();
        target_handle.abort();
    }

    #[tokio::test]
    async fn control_plane_client_bypasses_system_proxy_before_sending_token() {
        const TOKEN_ENV: &str = "CODEX_HELPER_TEST_NO_PROXY_TOKEN";
        const TOKEN: &str = "direct-control-plane-token";
        const SOURCE_ENV: &str = "CODEX_HELPER_TEST_NO_PROXY_SOURCE_URL";

        let source_tokens = Arc::new(Mutex::new(Vec::<String>::new()));
        let tokens = source_tokens.clone();
        let source = Router::new().route(
            "/status",
            get(move |headers: axum::http::HeaderMap| {
                let tokens = tokens.clone();
                async move {
                    if let Some(token) = headers
                        .get(ADMIN_TOKEN_HEADER)
                        .and_then(|value| value.to_str().ok())
                    {
                        tokens
                            .lock()
                            .expect("source token lock")
                            .push(token.to_string());
                    }
                    axum::Json(json!({ "source": true }))
                }
            }),
        );
        let (source_addr, source_handle) = spawn_server(source).await;

        let proxy_hits = Arc::new(AtomicUsize::new(0));
        let proxy_token_hits = Arc::new(AtomicUsize::new(0));
        let hits = proxy_hits.clone();
        let token_hits = proxy_token_hits.clone();
        let proxy = Router::new().fallback(move |headers: axum::http::HeaderMap| {
            let hits = hits.clone();
            let token_hits = token_hits.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                if headers.contains_key(ADMIN_TOKEN_HEADER) {
                    token_hits.fetch_add(1, Ordering::SeqCst);
                }
                axum::Json(json!({ "source": false }))
            }
        });
        let (proxy_addr, proxy_handle) = spawn_server(proxy).await;
        let proxy_url = format!("http://{proxy_addr}");
        let source_url = format!("http://{source_addr}");
        let test_exe = std::env::current_exe().expect("current test executable");
        let output = tokio::task::spawn_blocking(move || {
            std::process::Command::new(test_exe)
                .args([
                    "--exact",
                    "control_plane_client::tests::control_plane_client_no_proxy_subprocess",
                    "--ignored",
                    "--nocapture",
                ])
                .env(SOURCE_ENV, source_url)
                .env(TOKEN_ENV, TOKEN)
                .env("HTTP_PROXY", &proxy_url)
                .env("HTTPS_PROXY", &proxy_url)
                .env("ALL_PROXY", &proxy_url)
                .env("NO_PROXY", "")
                .env_remove("REQUEST_METHOD")
                .output()
        })
        .await
        .expect("join no-proxy subprocess")
        .expect("run no-proxy subprocess");
        let child_stdout = String::from_utf8_lossy(&output.stdout);
        let child_stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(
            proxy_hits.load(Ordering::SeqCst),
            0,
            "system proxy unexpectedly received the request\nstdout: {child_stdout}\nstderr: {child_stderr}"
        );
        assert_eq!(
            proxy_token_hits.load(Ordering::SeqCst),
            0,
            "system proxy unexpectedly received the admin token"
        );
        assert!(
            output.status.success(),
            "no-proxy subprocess failed\nstdout: {child_stdout}\nstderr: {child_stderr}"
        );
        assert_eq!(
            source_tokens.lock().expect("source token lock").as_slice(),
            [TOKEN]
        );
        source_handle.abort();
        proxy_handle.abort();
    }

    #[tokio::test]
    #[ignore = "subprocess helper for the system-proxy isolation test"]
    async fn control_plane_client_no_proxy_subprocess() {
        const TOKEN_ENV: &str = "CODEX_HELPER_TEST_NO_PROXY_TOKEN";
        const SOURCE_ENV: &str = "CODEX_HELPER_TEST_NO_PROXY_SOURCE_URL";

        let Ok(source_url) = std::env::var(SOURCE_ENV) else {
            return;
        };
        let endpoint = ControlPlaneEndpoint::new(source_url, Some(TOKEN_ENV))
            .expect("source control-plane endpoint");
        let client = ControlPlaneClient::new(endpoint).expect("control-plane client");
        let body = client
            .fetch_json::<serde_json::Value>("/status")
            .await
            .expect("control-plane status response");

        assert_eq!(body, json!({ "source": true }));
    }

    #[tokio::test]
    async fn control_plane_client_rejects_authority_rewriting_path_before_sending_token() {
        const TOKEN_ENV: &str = "CODEX_HELPER_TEST_HOSTILE_PATH_TOKEN";
        let _token = ScopedEnv::set(TOKEN_ENV, "must-not-leak");
        let target_hits = Arc::new(AtomicUsize::new(0));
        let target_token_hits = Arc::new(AtomicUsize::new(0));
        let hits = target_hits.clone();
        let tokens = target_token_hits.clone();
        let target = Router::new().route(
            "/target",
            get(move |headers: axum::http::HeaderMap| {
                let hits = hits.clone();
                let tokens = tokens.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    if headers.contains_key(ADMIN_TOKEN_HEADER) {
                        tokens.fetch_add(1, Ordering::SeqCst);
                    }
                    axum::Json(json!({ "ok": true }))
                }
            }),
        );
        let (target_addr, target_handle) = spawn_server(target).await;
        let (source_addr, source_handle) = spawn_server(Router::new()).await;
        let endpoint = ControlPlaneEndpoint::new(format!("http://{source_addr}"), Some(TOKEN_ENV))
            .expect("source endpoint");
        let client = ControlPlaneClient::new(endpoint).expect("client");

        let error = client
            .fetch_json::<serde_json::Value>(&format!("@{target_addr}/target"))
            .await
            .expect_err("authority-rewriting path must fail before I/O");

        assert!(error.to_string().contains("trusted"), "unexpected: {error}");
        assert_eq!(target_hits.load(Ordering::SeqCst), 0);
        assert_eq!(target_token_hits.load(Ordering::SeqCst), 0);
        source_handle.abort();
        target_handle.abort();
    }

    #[test]
    fn request_chain_path_encodes_selector_query_values() {
        let path = request_chain_path(
            RequestChainSelector {
                trace_id: Some("trace/with space".to_string()),
                request_id: Some(42),
                session_id: Some("session a".to_string()),
            },
            20,
        );

        assert_eq!(
            path,
            "/__codex_helper/api/v1/request-ledger/chain?limit=20&trace_id=trace%2Fwith+space&request_id=42&session=session+a"
        );
    }
}

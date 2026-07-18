use std::fmt;
use std::sync::Arc;

use http::HeaderValue;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

const MAX_CREDENTIAL_NAME_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("credential name must match [a-z0-9][a-z0-9._-]{{0,127}}")]
pub struct CredentialNameError;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CredentialName(String);

impl CredentialName {
    pub fn parse(value: impl Into<String>) -> Result<Self, CredentialNameError> {
        let value = value.into();
        let bytes = value.as_bytes();
        let first_is_valid = bytes
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
        let rest_is_valid = bytes.iter().skip(1).all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(*byte, b'.' | b'_' | b'-')
        });
        if !(1..=MAX_CREDENTIAL_NAME_BYTES).contains(&bytes.len())
            || !first_is_valid
            || !rest_is_valid
        {
            return Err(CredentialNameError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for CredentialName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for CredentialName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CredentialName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CredentialSourceKind {
    Native,
    SecretFile,
}

impl CredentialSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::SecretFile => "secret_file",
        }
    }
}

impl fmt::Display for CredentialSourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CredentialErrorCode {
    AlreadyExists,
    Missing,
    Invalid,
    Locked,
    PermissionDenied,
    InteractionRequired,
    BackendUnavailable,
    Ambiguous,
    Unsupported,
}

impl CredentialErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyExists => "already_exists",
            Self::Missing => "missing",
            Self::Invalid => "invalid",
            Self::Locked => "locked",
            Self::PermissionDenied => "permission_denied",
            Self::InteractionRequired => "interaction_required",
            Self::BackendUnavailable => "backend_unavailable",
            Self::Ambiguous => "ambiguous",
            Self::Unsupported => "unsupported",
        }
    }
}

impl fmt::Display for CredentialErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Redacted credential readiness shared by routing and trusted operator surfaces.
///
/// Management-only and structurally ambiguous failures collapse to `invalid`; they
/// are not distinct runtime availability states.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CredentialReadinessCode {
    #[default]
    Ready,
    Stale,
    Missing,
    Invalid,
    Locked,
    PermissionDenied,
    InteractionRequired,
    BackendUnavailable,
    Unsupported,
}

impl CredentialReadinessCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Stale => "stale",
            Self::Missing => "missing",
            Self::Invalid => "invalid",
            Self::Locked => "locked",
            Self::PermissionDenied => "permission_denied",
            Self::InteractionRequired => "interaction_required",
            Self::BackendUnavailable => "backend_unavailable",
            Self::Unsupported => "unsupported",
        }
    }

    pub fn is_routable(self) -> bool {
        matches!(self, Self::Ready | Self::Stale)
    }

    pub fn from_binding_codes(codes: impl IntoIterator<Item = CredentialReadinessCode>) -> Self {
        let mut routable = Self::Ready;
        for code in codes {
            if !code.is_routable() {
                return code;
            }
            if code == Self::Stale {
                routable = Self::Stale;
            }
        }
        routable
    }
}

impl From<CredentialErrorCode> for CredentialReadinessCode {
    fn from(code: CredentialErrorCode) -> Self {
        match code {
            CredentialErrorCode::Missing => Self::Missing,
            CredentialErrorCode::Invalid
            | CredentialErrorCode::AlreadyExists
            | CredentialErrorCode::Ambiguous => Self::Invalid,
            CredentialErrorCode::Locked => Self::Locked,
            CredentialErrorCode::PermissionDenied => Self::PermissionDenied,
            CredentialErrorCode::InteractionRequired => Self::InteractionRequired,
            CredentialErrorCode::BackendUnavailable => Self::BackendUnavailable,
            CredentialErrorCode::Unsupported => Self::Unsupported,
        }
    }
}

impl fmt::Display for CredentialReadinessCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CredentialBindingKind {
    Bearer,
    ApiKey,
}

impl CredentialBindingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bearer => "bearer",
            Self::ApiKey => "api_key",
        }
    }
}

/// Trusted, redacted detail for one configured credential binding.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialReadinessDetail {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<CredentialBindingKind>,
    pub code: CredentialReadinessCode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_cause: Option<CredentialReadinessCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

impl fmt::Debug for CredentialReadinessDetail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialReadinessDetail")
            .field("kind", &self.kind)
            .field("code", &self.code)
            .field("stale_cause", &self.stale_cause)
            .field("source_kind", &self.source_kind)
            .field("reference", &self.reference.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CredentialAggregateReadiness {
    #[default]
    Ready,
    Degraded,
    Blocked,
}

impl CredentialAggregateReadiness {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Degraded => "degraded",
            Self::Blocked => "blocked",
        }
    }

    pub fn from_endpoint_codes(codes: impl IntoIterator<Item = CredentialReadinessCode>) -> Self {
        let mut has_routable = false;
        let mut has_degraded = false;
        let mut has_endpoint = false;
        for code in codes {
            has_endpoint = true;
            has_routable |= code.is_routable();
            has_degraded |= code != CredentialReadinessCode::Ready;
        }
        if !has_endpoint || !has_routable {
            Self::Blocked
        } else if has_degraded {
            Self::Degraded
        } else {
            Self::Ready
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct CredentialError {
    code: CredentialErrorCode,
    source_kind: CredentialSourceKind,
    reference: String,
}

impl CredentialError {
    pub(crate) fn new(
        code: CredentialErrorCode,
        source_kind: CredentialSourceKind,
        reference: impl Into<String>,
    ) -> Self {
        Self {
            code,
            source_kind,
            reference: reference.into(),
        }
    }

    pub fn code(&self) -> CredentialErrorCode {
        self.code
    }

    pub fn source_kind(&self) -> CredentialSourceKind {
        self.source_kind
    }

    pub fn reference(&self) -> &str {
        self.reference.as_str()
    }
}

impl fmt::Debug for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialError")
            .field("code", &self.code)
            .field("source_kind", &self.source_kind)
            .field("reference", &self.reference)
            .finish()
    }
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "credential {}:{} is {}",
            self.source_kind, self.reference, self.code
        )
    }
}

impl std::error::Error for CredentialError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum CredentialValueError {
    #[error("credential value is empty")]
    Empty,
    #[error("credential value is not valid UTF-8")]
    InvalidUtf8,
    #[error("credential value contains a NUL byte")]
    Nul,
    #[error("credential value contains a line break")]
    LineBreak,
    #[error("credential value is not valid in an HTTP header")]
    InvalidHeader,
}

#[derive(Clone)]
pub struct SecretValue(Arc<SecretInner>);

struct SecretInner {
    bytes: Zeroizing<Vec<u8>>,
    #[cfg(test)]
    drop_observer: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl SecretValue {
    pub fn new(bytes: Vec<u8>) -> Result<Self, CredentialValueError> {
        Self::from_zeroizing_with_drop_observer(Zeroizing::new(bytes), None)
    }

    #[cfg(all(feature = "native-credentials", target_os = "macos"))]
    pub(crate) fn from_zeroizing(bytes: Zeroizing<Vec<u8>>) -> Result<Self, CredentialValueError> {
        Self::from_zeroizing_with_drop_observer(bytes, None)
    }

    fn from_zeroizing_with_drop_observer(
        bytes: Zeroizing<Vec<u8>>,
        #[cfg(test)] drop_observer: Option<Arc<std::sync::atomic::AtomicBool>>,
        #[cfg(not(test))] _drop_observer: Option<()>,
    ) -> Result<Self, CredentialValueError> {
        validate_secret_value(bytes.as_slice())?;
        Ok(Self(Arc::new(SecretInner {
            bytes,
            #[cfg(test)]
            drop_observer,
        })))
    }

    pub fn sensitive_header_value(&self) -> HeaderValue {
        let mut value = HeaderValue::from_bytes(self.0.bytes.as_slice())
            .expect("SecretValue validates HTTP header bytes at construction");
        value.set_sensitive(true);
        value
    }

    pub fn len(&self) -> usize {
        self.0.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.bytes.is_empty()
    }

    pub(crate) fn sensitive_bearer_header_value(&self) -> HeaderValue {
        let mut bytes = Zeroizing::new(Vec::with_capacity(7 + self.0.bytes.len()));
        bytes.extend_from_slice(b"Bearer ");
        bytes.extend_from_slice(self.0.bytes.as_slice());
        let mut value = HeaderValue::from_bytes(bytes.as_slice())
            .expect("SecretValue validates bearer header bytes at construction");
        value.set_sensitive(true);
        value
    }

    pub(crate) fn expose(&self) -> &[u8] {
        self.0.bytes.as_slice()
    }

    #[cfg(test)]
    pub(super) fn expose_for_test(&self) -> &[u8] {
        self.expose()
    }

    #[cfg(test)]
    pub(super) fn from_bytes_with_drop_observer(
        bytes: Vec<u8>,
        observer: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Self, CredentialValueError> {
        Self::from_zeroizing_with_drop_observer(Zeroizing::new(bytes), Some(observer))
    }
}

impl Drop for SecretInner {
    fn drop(&mut self) {
        self.bytes.zeroize();
        #[cfg(test)]
        if let Some(observer) = &self.drop_observer {
            observer.store(
                self.bytes.iter().all(|byte| *byte == 0),
                std::sync::atomic::Ordering::SeqCst,
            );
        }
    }
}

fn validate_secret_value(bytes: &[u8]) -> Result<(), CredentialValueError> {
    if bytes.is_empty() {
        return Err(CredentialValueError::Empty);
    }
    if std::str::from_utf8(bytes).is_err() {
        return Err(CredentialValueError::InvalidUtf8);
    }
    if bytes.contains(&0) {
        return Err(CredentialValueError::Nul);
    }
    if bytes.iter().any(|byte| matches!(*byte, b'\r' | b'\n')) {
        return Err(CredentialValueError::LineBreak);
    }
    HeaderValue::from_bytes(bytes)
        .map(|_| ())
        .map_err(|_| CredentialValueError::InvalidHeader)
}

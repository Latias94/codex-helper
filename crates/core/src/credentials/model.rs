use std::fmt;
use std::sync::Arc;

use http::HeaderValue;
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

const MAX_CREDENTIAL_NAME_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("credential name must match [a-z0-9][a-z0-9._-]{{0,127}}")]
pub struct CredentialNameError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

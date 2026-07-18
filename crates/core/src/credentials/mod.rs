mod capabilities;
mod generation;
mod installation_identity;
mod model;
mod native;
mod runtime;
mod secret_file;

#[cfg(test)]
pub(crate) use capabilities::TestNativeCredentialControl;
pub use capabilities::{
    CredentialSourceCapabilities, NATIVE_CREDENTIAL_MAX_BYTES, NativeCredentialDaemon,
    NativeCredentialManager,
};
pub(crate) use generation::{
    CapturedUpstreamCredential, CredentialGeneration, CredentialGenerationMarker, CredentialHandle,
    NamedCredentialLookup, NamedCredentialReference,
};
pub use installation_identity::{
    InstallationIdentity, InstallationIdentityError, InstallationIdentityErrorCode,
};
pub use model::{
    CredentialAggregateReadiness, CredentialBindingKind, CredentialError, CredentialErrorCode,
    CredentialName, CredentialNameError, CredentialReadinessCode, CredentialReadinessDetail,
    CredentialSourceKind, CredentialValueError, SecretValue,
};
pub(crate) use runtime::{
    CredentialCandidateInput, CredentialReadinessEvaluator, CredentialRuntime,
    CredentialRuntimeRefreshCause,
};
pub use secret_file::read_secret_file;

#[cfg(test)]
mod tests;

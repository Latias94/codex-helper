mod capabilities;
mod generation;
mod installation_identity;
mod model;
mod native;
mod runtime;
mod secret_file;

pub use capabilities::{
    CredentialSourceCapabilities, NativeCredentialDaemon, NativeCredentialManager,
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
    CredentialCandidateInput, CredentialRuntime, CredentialRuntimeRefreshCause,
};
pub use secret_file::read_secret_file;

#[cfg(test)]
mod tests;

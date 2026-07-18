use std::path::{Component, Path, PathBuf};

use codex_helper_core::config::ServiceKind;
use codex_helper_core::control_plane_client::{
    is_loopback_control_plane_base_url, normalize_control_plane_base_url,
};
use codex_helper_core::service_target::ServiceInstallGeneration;
use codex_helper_core::{
    ManagedFileSnapshot, ManagedFileTransaction, ManagedFileTransactionError,
    read_managed_file_snapshot,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const SERVICE_RECEIPT_SCHEMA_VERSION: u32 = 1;
const SERVICE_RECEIPT_FILE_NAME: &str = "service-install-receipt.json";
const MAX_SERVICE_RECEIPT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServicePlatformBackend {
    WindowsScheduledTask,
    MacosLaunchAgent,
    LinuxSystemdUser,
}

impl ServicePlatformBackend {
    pub(crate) fn current() -> Option<Self> {
        if cfg!(windows) {
            Some(Self::WindowsScheduledTask)
        } else if cfg!(target_os = "macos") {
            Some(Self::MacosLaunchAgent)
        } else if cfg!(target_os = "linux") {
            Some(Self::LinuxSystemdUser)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServiceReceipt {
    schema_version: u32,
    service: ServiceKind,
    helper_home: PathBuf,
    client_home: PathBuf,
    admin_base_url: String,
    platform_backend: ServicePlatformBackend,
    install_generation: ServiceInstallGeneration,
}

impl ServiceReceipt {
    // The service lifecycle introduced in U6 is the production writer; U9 owns the format first.
    #[allow(dead_code)]
    pub(crate) fn new(
        service: ServiceKind,
        helper_home: PathBuf,
        client_home: PathBuf,
        admin_base_url: impl Into<String>,
        platform_backend: ServicePlatformBackend,
        install_generation: ServiceInstallGeneration,
    ) -> Result<Self, ServiceReceiptError> {
        let admin_base_url = normalize_receipt_admin_url(&admin_base_url.into())?;
        let receipt = Self {
            schema_version: SERVICE_RECEIPT_SCHEMA_VERSION,
            service,
            helper_home,
            client_home,
            admin_base_url,
            platform_backend,
            install_generation,
        };
        receipt.validate_fields()?;
        Ok(receipt)
    }

    pub(crate) fn service(&self) -> ServiceKind {
        self.service
    }

    pub(crate) fn helper_home(&self) -> &Path {
        &self.helper_home
    }

    #[allow(dead_code)]
    pub(crate) fn client_home(&self) -> &Path {
        &self.client_home
    }

    pub(crate) fn admin_base_url(&self) -> &str {
        self.admin_base_url.as_str()
    }

    pub(crate) fn platform_backend(&self) -> ServicePlatformBackend {
        self.platform_backend
    }

    pub(crate) fn install_generation(&self) -> &ServiceInstallGeneration {
        &self.install_generation
    }

    fn validate_for_selected_home(&self, selected_home: &Path) -> Result<(), ServiceReceiptError> {
        self.validate_fields()?;
        if !same_existing_directory(&self.helper_home, selected_home)? {
            return Err(ServiceReceiptError::ForeignHelperHome);
        }
        Ok(())
    }

    fn validate_fields(&self) -> Result<(), ServiceReceiptError> {
        if self.schema_version != SERVICE_RECEIPT_SCHEMA_VERSION {
            return Err(ServiceReceiptError::LegacySchema {
                schema_version: Some(u64::from(self.schema_version)),
            });
        }
        validate_absolute_home(&self.helper_home, "helper_home")?;
        validate_absolute_home(&self.client_home, "client_home")?;
        let normalized = normalize_receipt_admin_url(&self.admin_base_url)?;
        if normalized != self.admin_base_url {
            return Err(ServiceReceiptError::Invalid {
                reason: "admin_base_url is not canonical",
            });
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub(crate) enum ServiceReceiptError {
    #[error("service receipt is absent")]
    Missing,
    #[error("service receipt uses an unsupported legacy schema version: {schema_version:?}")]
    LegacySchema { schema_version: Option<u64> },
    #[error("service receipt is invalid: {reason}")]
    Invalid { reason: &'static str },
    #[error("service receipt belongs to a different helper home")]
    ForeignHelperHome,
    #[error("service receipt transaction failed: {0}")]
    Transaction(#[from] ManagedFileTransactionError),
}

// U6 composes this receipt transaction with the platform service-definition transaction.
#[allow(dead_code)]
pub(crate) struct ServiceReceiptTransaction {
    helper_home: PathBuf,
    inner: ManagedFileTransaction,
}

impl std::fmt::Debug for ServiceReceiptTransaction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServiceReceiptTransaction")
            .field("helper_home", &self.helper_home)
            .field("inner", &self.inner)
            .finish()
    }
}

#[allow(dead_code)]
impl ServiceReceiptTransaction {
    pub(crate) fn begin(helper_home: impl Into<PathBuf>) -> Result<Self, ServiceReceiptError> {
        let helper_home = helper_home.into();
        validate_absolute_home(&helper_home, "helper_home")?;
        let inner = ManagedFileTransaction::begin(
            service_receipt_path(&helper_home),
            MAX_SERVICE_RECEIPT_BYTES,
        )?;
        Ok(Self { helper_home, inner })
    }

    pub(crate) fn current(&self) -> Result<Option<ServiceReceipt>, ServiceReceiptError> {
        parse_service_receipt_snapshot(self.inner.current(), &self.helper_home)
    }

    pub(crate) fn replace(&mut self, receipt: &ServiceReceipt) -> Result<(), ServiceReceiptError> {
        receipt.validate_for_selected_home(&self.helper_home)?;
        let mut bytes =
            serde_json::to_vec_pretty(receipt).map_err(|_| ServiceReceiptError::Invalid {
                reason: "receipt cannot be serialized",
            })?;
        bytes.push(b'\n');
        if bytes.len() > MAX_SERVICE_RECEIPT_BYTES {
            return Err(ServiceReceiptError::Invalid {
                reason: "receipt exceeds the maximum size",
            });
        }
        self.inner.replace(&bytes)?;
        Ok(())
    }

    pub(crate) fn remove(&mut self) -> Result<(), ServiceReceiptError> {
        self.inner.remove()?;
        Ok(())
    }

    pub(crate) fn rollback(&mut self) -> Result<(), ServiceReceiptError> {
        self.inner.rollback()?;
        Ok(())
    }
}

pub(crate) fn read_service_receipt(
    helper_home: impl AsRef<Path>,
) -> Result<ServiceReceipt, ServiceReceiptError> {
    let helper_home = helper_home.as_ref();
    validate_absolute_home(helper_home, "helper_home")?;
    let snapshot =
        read_managed_file_snapshot(service_receipt_path(helper_home), MAX_SERVICE_RECEIPT_BYTES)?;
    parse_service_receipt_snapshot(&snapshot, helper_home)?.ok_or(ServiceReceiptError::Missing)
}

pub(crate) fn service_receipt_path(helper_home: impl AsRef<Path>) -> PathBuf {
    helper_home.as_ref().join(SERVICE_RECEIPT_FILE_NAME)
}

fn parse_service_receipt_snapshot(
    snapshot: &ManagedFileSnapshot,
    selected_home: &Path,
) -> Result<Option<ServiceReceipt>, ServiceReceiptError> {
    let Some(bytes) = snapshot.bytes() else {
        return Ok(None);
    };
    if bytes.len() > MAX_SERVICE_RECEIPT_BYTES {
        return Err(ServiceReceiptError::Invalid {
            reason: "receipt exceeds the maximum size",
        });
    }
    let value = serde_json::from_slice::<serde_json::Value>(bytes).map_err(|_| {
        ServiceReceiptError::Invalid {
            reason: "receipt is not valid JSON",
        }
    })?;
    let schema_version = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64);
    if schema_version != Some(u64::from(SERVICE_RECEIPT_SCHEMA_VERSION)) {
        return Err(ServiceReceiptError::LegacySchema { schema_version });
    }
    let receipt = serde_json::from_value::<ServiceReceipt>(value).map_err(|_| {
        ServiceReceiptError::Invalid {
            reason: "receipt does not match the current schema",
        }
    })?;
    receipt.validate_for_selected_home(selected_home)?;
    Ok(Some(receipt))
}

fn normalize_receipt_admin_url(value: &str) -> Result<String, ServiceReceiptError> {
    let normalized =
        normalize_control_plane_base_url(value).map_err(|_| ServiceReceiptError::Invalid {
            reason: "admin_base_url is not a valid control-plane authority",
        })?;
    if !is_loopback_control_plane_base_url(&normalized) {
        return Err(ServiceReceiptError::Invalid {
            reason: "admin_base_url must be loopback",
        });
    }
    Ok(normalized)
}

fn validate_absolute_home(path: &Path, field: &'static str) -> Result<(), ServiceReceiptError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(ServiceReceiptError::Invalid { reason: field });
    }
    Ok(())
}

fn same_existing_directory(left: &Path, right: &Path) -> Result<bool, ServiceReceiptError> {
    let left = std::fs::canonicalize(left).map_err(|_| ServiceReceiptError::Invalid {
        reason: "helper_home cannot be resolved",
    })?;
    let right = std::fs::canonicalize(right).map_err(|_| ServiceReceiptError::Invalid {
        reason: "selected helper home cannot be resolved",
    })?;
    Ok(left == right)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHome(PathBuf);

    impl TestHome {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "codex-helper-service-receipt-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&path).expect("create test helper home");
            Self(path)
        }

        fn client_home(&self) -> PathBuf {
            let path = self.0.join("client");
            std::fs::create_dir_all(&path).expect("create test client home");
            path
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn receipt(home: &TestHome, generation: ServiceInstallGeneration) -> ServiceReceipt {
        ServiceReceipt::new(
            ServiceKind::Codex,
            home.0.clone(),
            home.client_home(),
            "http://127.0.0.1:4211",
            ServicePlatformBackend::MacosLaunchAgent,
            generation,
        )
        .expect("build receipt")
    }

    #[test]
    fn receipt_transaction_creates_replaces_removes_and_rolls_back() {
        let home = TestHome::new();
        let first = receipt(&home, ServiceInstallGeneration::generate());
        let second = receipt(&home, ServiceInstallGeneration::generate());
        let mut transaction =
            ServiceReceiptTransaction::begin(home.0.clone()).expect("begin transaction");
        assert!(matches!(
            ServiceReceiptTransaction::begin(home.0.clone()),
            Err(ServiceReceiptError::Transaction(
                ManagedFileTransactionError::Busy { .. }
            ))
        ));
        assert!(transaction.current().expect("missing receipt").is_none());

        transaction.replace(&first).expect("create receipt");
        assert_eq!(transaction.current().expect("current receipt"), Some(first));
        transaction.replace(&second).expect("replace receipt");
        assert_eq!(
            transaction.current().expect("current receipt"),
            Some(second)
        );
        transaction.remove().expect("remove receipt");
        assert!(transaction.current().expect("removed receipt").is_none());
        transaction.rollback().expect("roll back transaction");
        assert!(
            transaction
                .current()
                .expect("rolled back receipt")
                .is_none()
        );
    }

    #[test]
    fn replacing_existing_receipt_can_be_rolled_back() {
        let home = TestHome::new();
        let first = receipt(&home, ServiceInstallGeneration::generate());
        let second = receipt(&home, ServiceInstallGeneration::generate());
        {
            let mut create =
                ServiceReceiptTransaction::begin(home.0.clone()).expect("begin create");
            create.replace(&first).expect("create receipt");
        }

        let mut replace = ServiceReceiptTransaction::begin(home.0.clone()).expect("begin replace");
        replace.replace(&second).expect("replace receipt");
        replace.rollback().expect("roll back receipt");
        assert_eq!(
            read_service_receipt(&home.0).expect("read restored receipt"),
            first
        );
    }

    #[test]
    fn receipt_rejects_corrupt_legacy_foreign_and_concurrent_changes() {
        let home = TestHome::new();
        let foreign = TestHome::new();
        let path = service_receipt_path(&home.0);
        std::fs::write(&path, b"not-json").expect("write corrupt receipt");
        assert!(matches!(
            read_service_receipt(&home.0),
            Err(ServiceReceiptError::Invalid { .. })
        ));

        std::fs::write(&path, br#"{"schema_version":0}"#).expect("write legacy receipt");
        assert!(matches!(
            read_service_receipt(&home.0),
            Err(ServiceReceiptError::LegacySchema {
                schema_version: Some(0)
            })
        ));

        let foreign_receipt = receipt(&foreign, ServiceInstallGeneration::generate());
        std::fs::write(
            &path,
            serde_json::to_vec(&foreign_receipt).expect("serialize foreign receipt"),
        )
        .expect("write foreign receipt");
        assert!(matches!(
            read_service_receipt(&home.0),
            Err(ServiceReceiptError::ForeignHelperHome)
        ));

        std::fs::remove_file(&path).expect("remove foreign receipt");
        let mut transaction =
            ServiceReceiptTransaction::begin(home.0.clone()).expect("begin transaction");
        std::fs::write(&path, b"external writer").expect("write concurrent receipt");
        let error = transaction
            .replace(&receipt(&home, ServiceInstallGeneration::generate()))
            .expect_err("concurrent change must be rejected");
        assert!(matches!(
            error,
            ServiceReceiptError::Transaction(ManagedFileTransactionError::ConcurrentChange { .. })
        ));
    }

    #[test]
    fn receipt_bytes_are_non_secret_and_schema_is_closed() {
        const CANARY: &str = "credential-canary-9b7b485fcfcd4d8bbaccc9d092c8e0a2";

        let home = TestHome::new();
        let receipt = receipt(&home, ServiceInstallGeneration::generate());
        let mut transaction =
            ServiceReceiptTransaction::begin(home.0.clone()).expect("begin transaction");
        transaction.replace(&receipt).expect("write receipt");
        let bytes = std::fs::read(service_receipt_path(&home.0)).expect("read receipt bytes");
        let text = String::from_utf8(bytes).expect("receipt UTF-8");

        assert!(!text.contains(CANARY));
        assert!(!text.contains("credential"));
        assert!(!text.contains("fingerprint"));
        assert!(!text.contains("secret_file"));

        let mut value = serde_json::to_value(&receipt).expect("serialize receipt");
        value["unexpected"] = serde_json::Value::String(CANARY.to_string());
        std::fs::write(
            service_receipt_path(&home.0),
            serde_json::to_vec(&value).expect("serialize unexpected field"),
        )
        .expect("write unexpected field");
        assert!(matches!(
            read_service_receipt(&home.0),
            Err(ServiceReceiptError::Invalid { .. })
        ));
    }
}

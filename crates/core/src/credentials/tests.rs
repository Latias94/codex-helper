use std::collections::BTreeMap;
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use static_assertions::assert_not_impl_any;
use uuid::Uuid;

use super::capabilities::{NativeCredentialStore, NativeStoreError, NativeStoreErrorCode};
use super::installation_identity::{InstallationIdentity, InstallationIdentityErrorCode};
use super::model::{
    CredentialAggregateReadiness, CredentialErrorCode, CredentialName, CredentialReadinessCode,
    SecretValue,
};
use super::native::{NativeCredentialLocator, NativeCredentialNamespace};
use super::{CredentialSourceCapabilities, read_secret_file};

assert_not_impl_any!(SecretValue: std::fmt::Debug, serde::Serialize);

#[test]
fn credential_errors_map_to_stable_runtime_readiness_codes() {
    let cases = [
        (
            CredentialErrorCode::AlreadyExists,
            CredentialReadinessCode::Invalid,
        ),
        (
            CredentialErrorCode::Missing,
            CredentialReadinessCode::Missing,
        ),
        (
            CredentialErrorCode::Invalid,
            CredentialReadinessCode::Invalid,
        ),
        (CredentialErrorCode::Locked, CredentialReadinessCode::Locked),
        (
            CredentialErrorCode::PermissionDenied,
            CredentialReadinessCode::PermissionDenied,
        ),
        (
            CredentialErrorCode::InteractionRequired,
            CredentialReadinessCode::InteractionRequired,
        ),
        (
            CredentialErrorCode::BackendUnavailable,
            CredentialReadinessCode::BackendUnavailable,
        ),
        (
            CredentialErrorCode::Ambiguous,
            CredentialReadinessCode::Invalid,
        ),
        (
            CredentialErrorCode::Unsupported,
            CredentialReadinessCode::Unsupported,
        ),
    ];

    for (error, expected) in cases {
        let readiness = CredentialReadinessCode::from(error);
        assert_eq!(readiness, expected);
        assert_eq!(
            serde_json::from_str::<CredentialReadinessCode>(
                &serde_json::to_string(&readiness).expect("serialize readiness")
            )
            .expect("deserialize readiness"),
            readiness
        );
    }
}

#[test]
fn aggregate_readiness_distinguishes_ready_degraded_and_blocked_routes() {
    assert_eq!(
        CredentialAggregateReadiness::from_endpoint_codes([CredentialReadinessCode::Ready]),
        CredentialAggregateReadiness::Ready
    );
    assert_eq!(
        CredentialAggregateReadiness::from_endpoint_codes([
            CredentialReadinessCode::Ready,
            CredentialReadinessCode::Missing,
        ]),
        CredentialAggregateReadiness::Degraded
    );
    assert_eq!(
        CredentialAggregateReadiness::from_endpoint_codes([CredentialReadinessCode::Stale]),
        CredentialAggregateReadiness::Degraded
    );
    assert_eq!(
        CredentialAggregateReadiness::from_endpoint_codes([
            CredentialReadinessCode::Missing,
            CredentialReadinessCode::Unsupported,
        ]),
        CredentialAggregateReadiness::Blocked
    );
}

#[test]
fn binding_readiness_prioritizes_unavailable_then_stale() {
    assert_eq!(
        CredentialReadinessCode::from_binding_codes([]),
        CredentialReadinessCode::Ready
    );
    assert_eq!(
        CredentialReadinessCode::from_binding_codes([
            CredentialReadinessCode::Ready,
            CredentialReadinessCode::Stale,
        ]),
        CredentialReadinessCode::Stale
    );
    assert_eq!(
        CredentialReadinessCode::from_binding_codes([
            CredentialReadinessCode::Stale,
            CredentialReadinessCode::PermissionDenied,
        ]),
        CredentialReadinessCode::PermissionDenied
    );
}

#[derive(Default)]
struct FakeNativeStore {
    values: Mutex<BTreeMap<String, SecretValue>>,
    next_error: Mutex<Option<NativeStoreErrorCode>>,
    calls: AtomicUsize,
}

impl FakeNativeStore {
    fn fail_next(&self, code: NativeStoreErrorCode) {
        *self.next_error.lock().expect("fake error lock") = Some(code);
    }

    fn begin_operation(&self) -> Result<(), NativeStoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.next_error.lock().expect("fake error lock").take() {
            Some(code) => Err(NativeStoreError::new(code)),
            None => Ok(()),
        }
    }
}

impl NativeCredentialStore for FakeNativeStore {
    fn create(
        &self,
        locator: &NativeCredentialLocator,
        value: &SecretValue,
    ) -> Result<(), NativeStoreError> {
        self.begin_operation()?;
        let mut values = self.values.lock().expect("fake values lock");
        if values.contains_key(locator.as_str()) {
            return Err(NativeStoreError::new(NativeStoreErrorCode::AlreadyExists));
        }
        values.insert(locator.as_str().to_owned(), value.clone());
        Ok(())
    }

    fn set(
        &self,
        locator: &NativeCredentialLocator,
        value: &SecretValue,
    ) -> Result<(), NativeStoreError> {
        self.begin_operation()?;
        self.values
            .lock()
            .expect("fake values lock")
            .insert(locator.as_str().to_owned(), value.clone());
        Ok(())
    }

    fn read(&self, locator: &NativeCredentialLocator) -> Result<SecretValue, NativeStoreError> {
        self.begin_operation()?;
        self.values
            .lock()
            .expect("fake values lock")
            .get(locator.as_str())
            .cloned()
            .ok_or_else(|| NativeStoreError::new(NativeStoreErrorCode::Missing))
    }

    fn delete(&self, locator: &NativeCredentialLocator) -> Result<(), NativeStoreError> {
        self.begin_operation()?;
        if self
            .values
            .lock()
            .expect("fake values lock")
            .remove(locator.as_str())
            .is_none()
        {
            return Err(NativeStoreError::new(NativeStoreErrorCode::Missing));
        }
        Ok(())
    }
}

fn name(value: &str) -> CredentialName {
    CredentialName::parse(value).expect("valid credential name")
}

fn secret(value: &str) -> SecretValue {
    SecretValue::new(value.as_bytes().to_vec()).expect("valid secret")
}

fn test_identity(byte: u8) -> InstallationIdentity {
    InstallationIdentity::from_uuid(Uuid::from_bytes([byte; 16]))
}

#[test]
fn fake_backend_obeys_management_and_daemon_contract() {
    let backend = Arc::new(FakeNativeStore::default());
    let capabilities = CredentialSourceCapabilities::from_backend(backend.clone());
    let manager = capabilities.manager(test_identity(1));
    let daemon = capabilities.daemon(test_identity(1));
    let reference = name("relay.primary");

    manager
        .create(&reference, &secret("first"))
        .expect("create credential");
    assert_eq!(
        daemon
            .read(&reference)
            .expect("read created credential")
            .expose_for_test(),
        b"first"
    );

    let duplicate = manager
        .create(&reference, &secret("second"))
        .expect_err("create must not overwrite");
    assert_eq!(duplicate.code(), CredentialErrorCode::AlreadyExists);

    manager
        .set(&reference, &secret("second"))
        .expect("replace existing credential");
    assert_eq!(
        daemon
            .read(&reference)
            .expect("read replaced credential")
            .expose_for_test(),
        b"second"
    );

    manager.delete(&reference).expect("delete credential");
    assert_eq!(
        daemon
            .read(&reference)
            .err()
            .expect("deleted credential must be missing")
            .code(),
        CredentialErrorCode::Missing
    );
    assert_eq!(
        manager
            .delete(&reference)
            .expect_err("delete missing must fail")
            .code(),
        CredentialErrorCode::Missing
    );

    backend.fail_next(NativeStoreErrorCode::Ambiguous);
    assert_eq!(
        daemon
            .read(&reference)
            .err()
            .expect("ambiguous lookup must fail")
            .code(),
        CredentialErrorCode::Ambiguous
    );
    backend.fail_next(NativeStoreErrorCode::BackendUnavailable);
    assert_eq!(
        daemon
            .read(&reference)
            .err()
            .expect("unavailable backend must fail")
            .code(),
        CredentialErrorCode::BackendUnavailable
    );
}

#[test]
fn set_upserts_a_missing_native_credential() {
    let backend = Arc::new(FakeNativeStore::default());
    let capabilities = CredentialSourceCapabilities::from_backend(backend);
    let manager = capabilities.manager(test_identity(11));
    let daemon = capabilities.daemon(test_identity(11));
    let reference = name("relay.upsert");

    manager
        .set(&reference, &secret("created-by-set"))
        .expect("set must create a missing credential");
    assert_eq!(
        daemon
            .read(&reference)
            .expect("read upserted credential")
            .expose_for_test(),
        b"created-by-set"
    );
}

#[test]
fn manager_enforces_portable_native_credential_size_before_backend_io() {
    let backend = Arc::new(FakeNativeStore::default());
    let capabilities = CredentialSourceCapabilities::from_backend(backend.clone());
    let manager = capabilities.manager(test_identity(9));
    let below = name("relay.below-limit");
    let boundary = name("relay.at-limit");

    manager
        .create(
            &below,
            &SecretValue::new(vec![b'a'; 2_559]).expect("2,559-byte secret"),
        )
        .expect("2,559-byte native credential must be accepted");
    manager
        .create(
            &boundary,
            &SecretValue::new(vec![b'b'; 2_560]).expect("2,560-byte secret"),
        )
        .expect("2,560-byte native credential must be accepted");
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);

    let oversized = SecretValue::new(vec![b'c'; 2_561]).expect("2,561-byte test secret");
    assert_eq!(
        manager
            .create(&name("relay.over-limit"), &oversized)
            .expect_err("2,561-byte native credential must be rejected")
            .code(),
        CredentialErrorCode::Invalid
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);

    manager
        .set(
            &boundary,
            &SecretValue::new(vec![b'd'; 2_560]).expect("2,560-byte replacement"),
        )
        .expect("2,560-byte replacement must be accepted");
    assert_eq!(backend.calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        manager
            .set(&boundary, &oversized)
            .expect_err("2,561-byte replacement must be rejected")
            .code(),
        CredentialErrorCode::Invalid
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 3);
}

#[test]
fn daemon_rejects_oversized_values_inserted_outside_the_manager() {
    let backend = Arc::new(FakeNativeStore::default());
    let capabilities = CredentialSourceCapabilities::from_backend(backend.clone());
    let identity = test_identity(10);
    let reference = name("relay.external-oversized");
    let locator = NativeCredentialNamespace::new(identity).locator(&reference);
    backend.values.lock().expect("fake values lock").insert(
        locator.as_str().to_string(),
        SecretValue::new(vec![b'x'; 2_561]).expect("oversized external test value"),
    );

    let error = capabilities
        .daemon(identity)
        .read(&reference)
        .err()
        .expect("daemon must reject an oversized backend value");
    assert_eq!(error.code(), CredentialErrorCode::Invalid);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn backend_errors_never_format_secret_payloads() {
    const CANARY: &str = "ch-secret-canary-8fce6b3158614baf";
    let backend = Arc::new(FakeNativeStore::default());
    backend.fail_next(NativeStoreErrorCode::BackendUnavailable);
    let capabilities = CredentialSourceCapabilities::from_backend(backend);
    let error = capabilities
        .manager(test_identity(2))
        .create(&name("relay.canary"), &secret(CANARY))
        .expect_err("injected backend failure");

    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains(CANARY));
    assert_eq!(error.reference(), "relay.canary");
}

#[test]
fn forbidden_native_capability_fails_without_invoking_an_adapter() {
    let capabilities = CredentialSourceCapabilities::server();
    let daemon = capabilities.daemon(test_identity(3));

    assert!(!capabilities.native_supported());
    assert_eq!(
        daemon
            .read(&name("relay.server"))
            .err()
            .expect("server native access must be forbidden")
            .code(),
        CredentialErrorCode::Unsupported
    );
}

#[test]
fn locator_is_stable_opaque_and_installation_scoped() {
    let reference = name("relay.primary");
    let first = NativeCredentialNamespace::new(test_identity(4)).locator(&reference);
    let repeated = NativeCredentialNamespace::new(test_identity(4)).locator(&reference);
    let other_installation = NativeCredentialNamespace::new(test_identity(5)).locator(&reference);

    assert_eq!(first, repeated);
    assert_ne!(first, other_installation);
    assert!(!first.as_str().contains(reference.as_str()));
    assert!(
        !first
            .as_str()
            .contains(&test_identity(4).uuid().to_string())
    );
    assert!(CredentialName::parse("Relay.Primary").is_err());
}

#[test]
fn secret_zeroizes_only_after_the_final_clone_drops() {
    let zeroized = Arc::new(AtomicBool::new(false));
    let value = SecretValue::from_bytes_with_drop_observer(
        b"sensitive-value".to_vec(),
        Arc::clone(&zeroized),
    )
    .expect("valid observed secret");
    let last = value.clone();

    drop(value);
    assert!(!zeroized.load(Ordering::SeqCst));
    drop(last);
    assert!(zeroized.load(Ordering::SeqCst));
}

#[test]
fn secret_header_values_are_always_sensitive() {
    let value = secret("header-secret").sensitive_header_value();
    assert!(value.is_sensitive());
    assert_eq!(value.as_bytes(), b"header-secret");
}

fn secret_file_error(path: &std::path::Path) -> CredentialErrorCode {
    read_secret_file(path)
        .err()
        .expect("secret-file read must fail")
        .code()
}

#[test]
fn secret_file_enforces_content_and_size_boundaries() {
    let home = tempfile::tempdir().expect("temporary directory");
    let path = home.path().join("credential");

    assert_eq!(secret_file_error(&path), CredentialErrorCode::Missing);

    fs::write(&path, []).expect("write empty credential");
    assert_eq!(secret_file_error(&path), CredentialErrorCode::Invalid);

    fs::write(&path, vec![b'a'; 64 * 1024]).expect("write boundary credential");
    assert_eq!(
        read_secret_file(&path)
            .expect("64 KiB credential must be accepted")
            .expose_for_test()
            .len(),
        64 * 1024
    );

    fs::write(&path, vec![b'a'; 64 * 1024 + 1]).expect("write oversized credential");
    assert_eq!(secret_file_error(&path), CredentialErrorCode::Invalid);

    for invalid in [
        vec![0xff],
        b"contains\0nul".to_vec(),
        b"two\nlines".to_vec(),
        b"two\rlines".to_vec(),
        b"extra-terminal-lines\n\n".to_vec(),
        vec![0x01],
    ] {
        fs::write(&path, invalid).expect("write invalid credential");
        assert_eq!(secret_file_error(&path), CredentialErrorCode::Invalid);
    }

    fs::write(&path, b"one-line\n").expect("write LF credential");
    assert_eq!(
        read_secret_file(&path)
            .expect("one LF must be removed")
            .expose_for_test(),
        b"one-line"
    );

    fs::write(&path, b"one-line\r\n").expect("write CRLF credential");
    assert_eq!(
        read_secret_file(&path)
            .expect("one CRLF must be removed")
            .expose_for_test(),
        b"one-line"
    );

    assert_eq!(
        read_secret_file("relative-secret")
            .err()
            .expect("relative path must fail")
            .code(),
        CredentialErrorCode::Invalid
    );
}

#[cfg(unix)]
#[test]
fn secret_file_rejects_special_files_without_blocking_and_allows_regular_symlinks() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    use std::os::unix::net::UnixListener;

    let home = tempfile::tempdir().expect("temporary directory");
    let regular = home.path().join("regular");
    fs::write(&regular, b"symlink-value").expect("write regular credential");
    let link = home.path().join("link");
    symlink(&regular, &link).expect("create regular-file symlink");
    assert_eq!(
        read_secret_file(&link)
            .expect("regular-file symlink must be accepted")
            .expose_for_test(),
        b"symlink-value"
    );

    assert_eq!(secret_file_error(home.path()), CredentialErrorCode::Invalid);
    assert_eq!(
        secret_file_error(std::path::Path::new("/dev/null")),
        CredentialErrorCode::Invalid
    );

    let fifo = home.path().join("fifo");
    let fifo_path = CString::new(fifo.as_os_str().as_bytes()).expect("FIFO path CString");
    let result = unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) };
    assert_eq!(
        result,
        0,
        "create FIFO: {}",
        std::io::Error::last_os_error()
    );
    assert_eq!(secret_file_error(&fifo), CredentialErrorCode::Invalid);

    let socket = home.path().join("socket");
    let _listener = UnixListener::bind(&socket).expect("bind Unix socket");
    assert_eq!(secret_file_error(&socket), CredentialErrorCode::Invalid);

    let unreadable = home.path().join("unreadable");
    fs::write(&unreadable, b"permission-value").expect("write unreadable credential");
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000))
        .expect("remove credential permissions");
    let permission_result = read_secret_file(&unreadable);
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o600))
        .expect("restore credential permissions");
    if let Err(error) = permission_result {
        assert_eq!(error.code(), CredentialErrorCode::PermissionDenied);
    }
}

#[test]
fn installation_identity_initializes_once_and_reads_while_writer_is_active() {
    let home = tempfile::tempdir().expect("temporary helper home");
    let initialized = InstallationIdentity::resolve_in_home(home.path())
        .expect("initialize installation identity");
    let stopped = InstallationIdentity::resolve_in_home(home.path())
        .expect("read stopped installation identity");
    assert_eq!(initialized, stopped);

    let writer = crate::runtime_store::RuntimeStore::open_in_home(home.path())
        .expect("open active runtime writer");
    let active = InstallationIdentity::resolve_in_home(home.path())
        .expect("read identity without competing for writer lease");
    assert_eq!(active.uuid(), writer.identity().store_id());
}

#[test]
fn corrupt_installation_metadata_is_not_replaced() {
    let home = tempfile::tempdir().expect("temporary helper home");
    let state = home.path().join("state");
    fs::create_dir_all(&state).expect("create state directory");
    let database = state.join("state.sqlite");
    let corrupt = b"not-a-runtime-database";
    fs::write(&database, corrupt).expect("write corrupt database");

    let error = InstallationIdentity::resolve_in_home(home.path())
        .expect_err("corrupt identity must fail closed");
    assert_eq!(error.code(), InstallationIdentityErrorCode::Invalid);
    assert_eq!(fs::read(&database).expect("read corrupt database"), corrupt);
}

#[test]
fn invalid_identity_under_an_active_writer_never_mints_a_second_namespace() {
    let home = tempfile::tempdir().expect("temporary helper home");
    let writer = crate::runtime_store::RuntimeStore::open_in_home(home.path())
        .expect("open active runtime writer");
    let original = writer.identity().store_id();
    let database = crate::runtime_store::runtime_store_path_in(home.path());
    let connection = rusqlite::Connection::open(&database).expect("open mutation connection");
    connection
        .pragma_update(None, "foreign_keys", false)
        .expect("disable foreign keys for corruption fixture");
    connection
        .execute(
            "UPDATE store_meta SET store_id = 'zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz' WHERE store_id = ?1",
            [original.to_string()],
        )
        .expect("corrupt installation metadata");

    let error = InstallationIdentity::resolve_in_home(home.path())
        .expect_err("invalid active identity must fail closed");
    assert_eq!(error.code(), InstallationIdentityErrorCode::Invalid);
    let persisted: String = connection
        .query_row("SELECT store_id FROM store_meta", [], |row| row.get(0))
        .expect("read invalid identity");
    assert_eq!(persisted, "zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz");
}

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read as _, Write as _};

use codex_helper_core::credentials::{
    CredentialError, CredentialErrorCode, CredentialName, CredentialReadinessCode,
    CredentialSourceCapabilities, InstallationIdentity, NATIVE_CREDENTIAL_MAX_BYTES,
    NativeCredentialDaemon, NativeCredentialManager, SecretValue,
};
use codex_helper_core::dashboard_core::OperatorReadModel;
use codex_helper_core::service_target::{
    LocalCredentialRefreshAction, LocalCredentialRefreshResponse, LocalCredentialRefreshStatus,
};
use serde::Serialize;
use zeroize::Zeroizing;

use crate::cli_types::{CliError, CliResult, CredentialCommand};
use crate::config::{CredentialRef, HelperConfig, ServiceRouteConfig, storage::load_config};

const CREDENTIAL_STATUS_SCHEMA_VERSION: u32 = 1;

pub async fn handle_credential_cmd(cmd: CredentialCommand) -> CliResult<()> {
    let input = ProcessCredentialInput;
    match cmd {
        CredentialCommand::Create { name, stdin } => {
            let name = parse_credential_name(name)?;
            prepare_and_write_native_credential(
                NativeWriteMode::Create,
                &name,
                stdin,
                &input,
                NativeCredentialAccess::open,
            )?;
            finish_committed_mutation(&name, LocalCredentialRefreshAction::Upsert, "created")
                .await?;
        }
        CredentialCommand::Set { name, stdin } => {
            let name = parse_credential_name(name)?;
            prepare_and_write_native_credential(
                NativeWriteMode::Set,
                &name,
                stdin,
                &input,
                NativeCredentialAccess::open,
            )?;
            finish_committed_mutation(&name, LocalCredentialRefreshAction::Upsert, "set").await?;
        }
        CredentialCommand::Import { name, from_env } => {
            let name = parse_credential_name(name)?;
            import_native_credential_from_environment(
                &name,
                &from_env,
                |key| std::env::var(key),
                NativeCredentialAccess::open,
            )?;
            finish_committed_mutation(&name, LocalCredentialRefreshAction::Upsert, "imported")
                .await?;
        }
        CredentialCommand::Status { name, json } => {
            handle_status(name, json).await?;
        }
        CredentialCommand::Delete {
            name,
            yes,
            if_exists,
        } => {
            let name = parse_credential_name(name)?;
            let config = load_config()
                .await
                .map_err(|error| CliError::Configuration(error.to_string()))?;
            let consumers = configured_consumers(&config, name.as_str());
            print_delete_scope(&name, &consumers);
            match delete_native_credential(
                &name,
                yes,
                if_exists,
                &input,
                NativeCredentialAccess::open,
            )? {
                NativeDeleteOutcome::AlreadyAbsent => {
                    println!("Native credential '{}' is already absent.", name);
                    return Ok(());
                }
                NativeDeleteOutcome::Deleted => {}
            }
            finish_committed_mutation(&name, LocalCredentialRefreshAction::Delete, "deleted")
                .await?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum NativeWriteMode {
    Create,
    Set,
}

trait CredentialAccess {
    fn create(&self, name: &CredentialName, value: &SecretValue) -> Result<(), CredentialError>;
    fn set(&self, name: &CredentialName, value: &SecretValue) -> Result<(), CredentialError>;
    fn readiness(&self, name: &CredentialName) -> CredentialReadinessCode;
    fn delete(&self, name: &CredentialName) -> Result<(), CredentialError>;
}

struct NativeCredentialAccess {
    manager: NativeCredentialManager,
    daemon: NativeCredentialDaemon,
}

impl NativeCredentialAccess {
    fn open() -> CliResult<Self> {
        let installation = InstallationIdentity::resolve_default().map_err(|error| {
            CliError::Other(format!(
                "native credential installation identity is {}",
                error.code()
            ))
        })?;
        let capabilities = CredentialSourceCapabilities::platform_native();
        Ok(Self {
            manager: capabilities.manager(installation),
            daemon: capabilities.daemon(installation),
        })
    }
}

impl CredentialAccess for NativeCredentialAccess {
    fn create(&self, name: &CredentialName, value: &SecretValue) -> Result<(), CredentialError> {
        self.manager.create(name, value)
    }

    fn set(&self, name: &CredentialName, value: &SecretValue) -> Result<(), CredentialError> {
        self.manager.set(name, value)
    }

    fn readiness(&self, name: &CredentialName) -> CredentialReadinessCode {
        self.daemon.read(name).map_or_else(
            |error| CredentialReadinessCode::from(error.code()),
            |_| CredentialReadinessCode::Ready,
        )
    }

    fn delete(&self, name: &CredentialName) -> Result<(), CredentialError> {
        self.manager.delete(name)
    }
}

trait CredentialInput {
    fn is_interactive(&self) -> bool;
    fn read_masked(&self, prompt: &str) -> io::Result<Vec<u8>>;
    fn read_stdin(&self, max_bytes: usize) -> io::Result<Vec<u8>>;
    fn confirm_delete(&self, prompt: &str) -> io::Result<bool>;
}

struct ProcessCredentialInput;

impl CredentialInput for ProcessCredentialInput {
    fn is_interactive(&self) -> bool {
        atty::is(atty::Stream::Stdin) && atty::is(atty::Stream::Stderr)
    }

    fn read_masked(&self, prompt: &str) -> io::Result<Vec<u8>> {
        rpassword::prompt_password(prompt).map(String::into_bytes)
    }

    fn read_stdin(&self, max_bytes: usize) -> io::Result<Vec<u8>> {
        let mut bytes = Zeroizing::new(Vec::with_capacity(max_bytes.min(4 * 1024)));
        io::stdin()
            .lock()
            .take((max_bytes + 3) as u64)
            .read_to_end(&mut bytes)?;
        Ok(std::mem::take(&mut *bytes))
    }

    fn confirm_delete(&self, prompt: &str) -> io::Result<bool> {
        eprint!("{prompt}");
        io::stderr().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        Ok(matches!(
            answer.trim().to_ascii_lowercase().as_str(),
            "y" | "yes"
        ))
    }
}

fn prepare_and_write_native_credential<A: CredentialAccess>(
    mode: NativeWriteMode,
    name: &CredentialName,
    stdin: bool,
    input: &dyn CredentialInput,
    open_access: impl FnOnce() -> CliResult<A>,
) -> CliResult<()> {
    let value = read_secret_input(name, stdin, input)?;
    let access = open_access()?;
    match mode {
        NativeWriteMode::Create => access.create(name, &value),
        NativeWriteMode::Set => access.set(name, &value),
    }
    .map_err(credential_store_error)
}

fn import_native_credential_from_environment<A: CredentialAccess>(
    name: &CredentialName,
    environment: &str,
    read_environment: impl FnOnce(&str) -> Result<String, std::env::VarError>,
    open_access: impl FnOnce() -> CliResult<A>,
) -> CliResult<()> {
    validate_environment_name(environment)?;
    let raw = read_environment(environment).map_err(|error| {
        let reason = match error {
            std::env::VarError::NotPresent => "is not set",
            std::env::VarError::NotUnicode(_) => "is not valid Unicode",
        };
        CliError::Other(format!("environment variable {environment} {reason}"))
    })?;
    let value = validate_secret_bytes(raw.into_bytes(), true)?;
    open_access()?
        .set(name, &value)
        .map_err(credential_store_error)
}

fn read_secret_input(
    name: &CredentialName,
    stdin: bool,
    input: &dyn CredentialInput,
) -> CliResult<SecretValue> {
    if stdin {
        let bytes = input
            .read_stdin(NATIVE_CREDENTIAL_MAX_BYTES)
            .map_err(|error| CliError::Other(format!("read credential from stdin: {error}")))?;
        return validate_secret_bytes(bytes, true);
    }
    if !input.is_interactive() {
        return Err(CliError::Other(
            "credential input requires an interactive TTY; pass --stdin or use `credential import --from-env ENV`"
                .to_string(),
        ));
    }
    let mut bytes = Zeroizing::new(
        input
            .read_masked(&format!("Credential value for {name}: "))
            .map_err(|error| CliError::Other(format!("read masked credential: {error}")))?,
    );
    let confirmation = Zeroizing::new(
        input
            .read_masked(&format!("Confirm credential value for {name}: "))
            .map_err(|error| {
                CliError::Other(format!("read masked credential confirmation: {error}"))
            })?,
    );
    if bytes.as_slice() != confirmation.as_slice() {
        return Err(CliError::Other(
            "credential confirmation does not match".to_string(),
        ));
    }
    validate_secret_bytes(std::mem::take(&mut *bytes), false)
}

fn validate_secret_bytes(
    mut bytes: Vec<u8>,
    strip_terminal_line_ending: bool,
) -> CliResult<SecretValue> {
    if strip_terminal_line_ending {
        if bytes.ends_with(b"\r\n") {
            bytes.truncate(bytes.len() - 2);
        } else if bytes.ends_with(b"\n") {
            bytes.truncate(bytes.len() - 1);
        }
    }
    let value = SecretValue::new(bytes)
        .map_err(|error| CliError::Other(format!("invalid credential value: {error}")))?;
    if value.len() > NATIVE_CREDENTIAL_MAX_BYTES {
        return Err(CliError::Other(format!(
            "invalid credential value: exceeds {NATIVE_CREDENTIAL_MAX_BYTES} bytes"
        )));
    }
    Ok(value)
}

fn validate_environment_name(name: &str) -> CliResult<()> {
    if name.is_empty() || name.contains('=') || name.chars().any(char::is_control) {
        return Err(CliError::Other(
            "environment variable name is invalid".to_string(),
        ));
    }
    Ok(())
}

fn parse_credential_name(name: String) -> CliResult<CredentialName> {
    CredentialName::parse(name).map_err(|error| CliError::Other(error.to_string()))
}

fn credential_store_error(error: CredentialError) -> CliError {
    CliError::Other(error.to_string())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
struct CredentialConsumer {
    service: String,
    provider: String,
    kind: String,
}

#[derive(Debug, Clone, Serialize)]
struct CredentialStatusRecord {
    backend: &'static str,
    reference: String,
    readiness: CredentialReadinessCode,
    consumers: Vec<CredentialConsumer>,
    refresh: CredentialRefreshView,
}

#[derive(Debug, Clone, Serialize)]
struct CredentialStatusPayload {
    schema_version: u32,
    credentials: Vec<CredentialStatusRecord>,
}

#[derive(Debug, Clone, Serialize)]
struct CredentialRefreshView {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_revision: Option<u64>,
}

async fn handle_status(requested_name: Option<String>, json: bool) -> CliResult<()> {
    let config = load_config()
        .await
        .map_err(|error| CliError::Configuration(error.to_string()))?;
    let names = match requested_name {
        Some(name) => BTreeSet::from([parse_credential_name(name)?]),
        None => configured_native_names(&config),
    };
    if names.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&CredentialStatusPayload {
                    schema_version: CREDENTIAL_STATUS_SCHEMA_VERSION,
                    credentials: Vec::new(),
                })
                .map_err(|error| CliError::Other(error.to_string()))?
            );
        } else {
            print_credential_status(&[]);
        }
        return Ok(());
    }
    let access = NativeCredentialAccess::open()?;
    let runtime = match crate::cli_app::read_resident_operator_model().await {
        Ok(model) => RuntimeCredentialProjection::from_operator_model(&model),
        Err(_) => RuntimeCredentialProjection::Unavailable,
    };
    let records = names
        .iter()
        .map(|name| credential_status_record(&access, &config, &runtime, name))
        .collect::<Vec<_>>();
    if json {
        let payload = CredentialStatusPayload {
            schema_version: CREDENTIAL_STATUS_SCHEMA_VERSION,
            credentials: records,
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&payload)
                .map_err(|error| CliError::Other(error.to_string()))?
        );
    } else {
        print_credential_status(&records);
    }
    Ok(())
}

fn credential_status_record(
    access: &dyn CredentialAccess,
    config: &HelperConfig,
    runtime: &RuntimeCredentialProjection,
    name: &CredentialName,
) -> CredentialStatusRecord {
    CredentialStatusRecord {
        backend: "native",
        reference: format!("native:{name}"),
        readiness: access.readiness(name),
        consumers: configured_consumers(config, name.as_str()),
        refresh: runtime.refresh_view(name.as_str()),
    }
}

enum RuntimeCredentialProjection {
    Available {
        service: String,
        runtime_revision: Option<u64>,
        readiness: BTreeMap<String, Vec<CredentialReadinessCode>>,
    },
    Unavailable,
}

impl RuntimeCredentialProjection {
    fn from_operator_model(model: &OperatorReadModel) -> Self {
        let mut readiness = BTreeMap::<String, Vec<CredentialReadinessCode>>::new();
        if let Some(data) = model.data.as_ref() {
            for provider in &data.summary.providers {
                for endpoint in &provider.endpoints {
                    for detail in &endpoint.credential_details {
                        if detail.source_kind.as_deref() != Some("native") {
                            continue;
                        }
                        let Some(reference) = detail.reference.as_ref() else {
                            continue;
                        };
                        readiness
                            .entry(reference.clone())
                            .or_default()
                            .push(detail.code);
                    }
                }
            }
        }
        for states in readiness.values_mut() {
            states.sort_by_key(|state| state.as_str());
            states.dedup();
        }
        Self::Available {
            service: model.service_name.clone(),
            runtime_revision: model
                .revisions
                .as_ref()
                .map(|revisions| revisions.runtime_revision),
            readiness,
        }
    }

    fn refresh_view(&self, name: &str) -> CredentialRefreshView {
        let Self::Available {
            service,
            runtime_revision,
            readiness,
        } = self
        else {
            return CredentialRefreshView {
                status: "target_unavailable".to_string(),
                service: None,
                runtime_revision: None,
            };
        };
        let Some(states) = readiness.get(name) else {
            return CredentialRefreshView {
                status: "not_referenced".to_string(),
                service: Some(service.clone()),
                runtime_revision: *runtime_revision,
            };
        };
        let code = CredentialReadinessCode::from_binding_codes(states.iter().copied());
        CredentialRefreshView {
            status: format!("runtime_{}", code.as_str()),
            service: Some(service.clone()),
            runtime_revision: *runtime_revision,
        }
    }
}

fn configured_native_names(config: &HelperConfig) -> BTreeSet<CredentialName> {
    configured_consumer_map(config)
        .into_keys()
        .filter_map(|name| CredentialName::parse(name).ok())
        .collect()
}

fn configured_consumers(config: &HelperConfig, name: &str) -> Vec<CredentialConsumer> {
    configured_consumer_map(config)
        .remove(name)
        .unwrap_or_default()
}

fn configured_consumer_map(config: &HelperConfig) -> BTreeMap<String, Vec<CredentialConsumer>> {
    let mut consumers = BTreeMap::<String, Vec<CredentialConsumer>>::new();
    collect_service_consumers(&mut consumers, "codex", &config.codex);
    collect_service_consumers(&mut consumers, "claude", &config.claude);
    for entries in consumers.values_mut() {
        entries.sort();
        entries.dedup();
    }
    consumers
}

fn collect_service_consumers(
    output: &mut BTreeMap<String, Vec<CredentialConsumer>>,
    service: &str,
    config: &ServiceRouteConfig,
) {
    for (provider_name, provider) in &config.providers {
        let auth = provider.effective_auth();
        collect_native_consumer(
            output,
            service,
            provider_name,
            "bearer",
            auth.auth_token_ref.as_ref(),
        );
        collect_native_consumer(
            output,
            service,
            provider_name,
            "api_key",
            auth.api_key_ref.as_ref(),
        );
    }
}

fn collect_native_consumer(
    output: &mut BTreeMap<String, Vec<CredentialConsumer>>,
    service: &str,
    provider: &str,
    kind: &str,
    reference: Option<&CredentialRef>,
) {
    let Some(CredentialRef::Native { name }) = reference else {
        return;
    };
    output
        .entry(name.clone())
        .or_default()
        .push(CredentialConsumer {
            service: service.to_string(),
            provider: provider.to_string(),
            kind: kind.to_string(),
        });
}

fn print_credential_status(records: &[CredentialStatusRecord]) {
    if records.is_empty() {
        println!("No native credentials are referenced by the current configuration.");
        return;
    }
    for record in records {
        println!("Credential: {}", record.reference);
        println!("  backend: {}", record.backend);
        println!("  readiness: {}", record.readiness);
        println!("  consumers: {}", format_consumers(&record.consumers));
        println!("  refresh: {}", record.refresh.status);
    }
}

fn format_consumers(consumers: &[CredentialConsumer]) -> String {
    if consumers.is_empty() {
        return "<none>".to_string();
    }
    consumers
        .iter()
        .map(|consumer| {
            format!(
                "{}/{}/{}",
                consumer.service, consumer.provider, consumer.kind
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_delete_scope(name: &CredentialName, consumers: &[CredentialConsumer]) {
    println!("Delete native credential '{name}'.");
    println!("Configured consumers: {}", format_consumers(consumers));
    println!("Provider configuration will not be changed.");
}

fn require_delete_confirmation(
    name: &CredentialName,
    yes: bool,
    input: &dyn CredentialInput,
) -> CliResult<()> {
    if yes {
        return Ok(());
    }
    if !input.is_interactive() {
        return Err(CliError::Other(format!(
            "deleting native credential '{name}' requires an interactive TTY or --yes"
        )));
    }
    if input
        .confirm_delete(&format!("Delete native credential '{name}'? [y/N] "))
        .map_err(|error| CliError::Other(format!("read delete confirmation: {error}")))?
    {
        Ok(())
    } else {
        Err(CliError::Other("credential deletion cancelled".to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeDeleteOutcome {
    Deleted,
    AlreadyAbsent,
}

fn delete_native_credential<A: CredentialAccess>(
    name: &CredentialName,
    yes: bool,
    if_exists: bool,
    input: &dyn CredentialInput,
    open_access: impl FnOnce() -> CliResult<A>,
) -> CliResult<NativeDeleteOutcome> {
    if if_exists {
        let access = open_access()?;
        if access.readiness(name) == CredentialReadinessCode::Missing {
            return Ok(NativeDeleteOutcome::AlreadyAbsent);
        }
        require_delete_confirmation(name, yes, input)?;
        return classify_delete_result(access.delete(name), true);
    }

    require_delete_confirmation(name, yes, input)?;
    classify_delete_result(open_access()?.delete(name), false)
}

fn classify_delete_result(
    result: Result<(), CredentialError>,
    if_exists: bool,
) -> CliResult<NativeDeleteOutcome> {
    match result {
        Ok(()) => Ok(NativeDeleteOutcome::Deleted),
        Err(error) if if_exists && error.code() == CredentialErrorCode::Missing => {
            Ok(NativeDeleteOutcome::AlreadyAbsent)
        }
        Err(error) => Err(credential_store_error(error)),
    }
}

async fn finish_committed_mutation(
    name: &CredentialName,
    action: LocalCredentialRefreshAction,
    operation: &str,
) -> CliResult<()> {
    let response = crate::cli_app::refresh_resident_credential(name.clone(), action)
        .await
        .map_err(|error| committed_refresh_error(name, operation, &error.to_string()))?;
    print_committed_mutation(name, operation, &response);
    Ok(())
}

fn committed_refresh_error(name: &CredentialName, operation: &str, reason: &str) -> CliError {
    CliError::Other(format!(
        "store_committed_runtime_refresh_failed: native credential '{name}' was {operation}, but the matching resident runtime was not refreshed: {reason}. The store change was not rolled back; restart the intended codex-helper service before relying on the new credential state"
    ))
}

fn print_committed_mutation(
    name: &CredentialName,
    operation: &str,
    response: &LocalCredentialRefreshResponse,
) {
    let refresh = match response.status {
        LocalCredentialRefreshStatus::Published => "published",
        LocalCredentialRefreshStatus::Unchanged => "unchanged",
        LocalCredentialRefreshStatus::NotReferenced => "not_referenced",
    };
    println!(
        "Native credential '{name}' {operation}; runtime_refresh={refresh} service={} revision={}",
        service_name(response.service),
        response.runtime_revision
    );
}

fn service_name(service: crate::config::ServiceKind) -> &'static str {
    match service {
        crate::config::ServiceKind::Codex => "codex",
        crate::config::ServiceKind::Claude => "claude",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::TempTestDir;
    use std::cell::Cell;
    use std::fs;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeAccess {
        values: Mutex<BTreeSet<String>>,
        calls: Cell<usize>,
    }

    impl CredentialAccess for FakeAccess {
        fn create(
            &self,
            name: &CredentialName,
            _value: &SecretValue,
        ) -> Result<(), CredentialError> {
            self.calls.set(self.calls.get() + 1);
            self.values
                .lock()
                .expect("fake values")
                .insert(name.to_string());
            Ok(())
        }

        fn set(&self, name: &CredentialName, _value: &SecretValue) -> Result<(), CredentialError> {
            self.calls.set(self.calls.get() + 1);
            self.values
                .lock()
                .expect("fake values")
                .insert(name.to_string());
            Ok(())
        }

        fn readiness(&self, name: &CredentialName) -> CredentialReadinessCode {
            self.calls.set(self.calls.get() + 1);
            if self
                .values
                .lock()
                .expect("fake values")
                .contains(name.as_str())
            {
                CredentialReadinessCode::Ready
            } else {
                CredentialReadinessCode::Missing
            }
        }

        fn delete(&self, name: &CredentialName) -> Result<(), CredentialError> {
            self.calls.set(self.calls.get() + 1);
            self.values
                .lock()
                .expect("fake values")
                .remove(name.as_str());
            Ok(())
        }
    }

    struct FakeInput {
        interactive: bool,
        masked: Vec<u8>,
        masked_confirmation: Option<Vec<u8>>,
        stdin: Vec<u8>,
        confirmed: bool,
        masked_calls: Cell<usize>,
    }

    impl CredentialInput for FakeInput {
        fn is_interactive(&self) -> bool {
            self.interactive
        }

        fn read_masked(&self, _prompt: &str) -> io::Result<Vec<u8>> {
            let call = self.masked_calls.get();
            self.masked_calls.set(call + 1);
            Ok(if call == 1 {
                self.masked_confirmation
                    .clone()
                    .unwrap_or_else(|| self.masked.clone())
            } else {
                self.masked.clone()
            })
        }

        fn read_stdin(&self, _max_bytes: usize) -> io::Result<Vec<u8>> {
            Ok(self.stdin.clone())
        }

        fn confirm_delete(&self, _prompt: &str) -> io::Result<bool> {
            Ok(self.confirmed)
        }
    }

    fn credential_name() -> CredentialName {
        CredentialName::parse("relay.primary").expect("credential name")
    }

    #[test]
    fn masked_tty_is_the_default_and_non_tty_fails_before_opening_store() {
        let interactive = FakeInput {
            interactive: true,
            masked: b"masked-value".to_vec(),
            masked_confirmation: None,
            stdin: Vec::new(),
            confirmed: false,
            masked_calls: Cell::new(0),
        };
        prepare_and_write_native_credential(
            NativeWriteMode::Create,
            &credential_name(),
            false,
            &interactive,
            || Ok(FakeAccess::default()),
        )
        .expect("masked create");
        assert_eq!(interactive.masked_calls.get(), 2);

        let non_tty = FakeInput {
            interactive: false,
            masked: b"must-not-read".to_vec(),
            masked_confirmation: None,
            stdin: Vec::new(),
            confirmed: false,
            masked_calls: Cell::new(0),
        };
        let opened = Cell::new(false);
        let result = prepare_and_write_native_credential(
            NativeWriteMode::Create,
            &credential_name(),
            false,
            &non_tty,
            || {
                opened.set(true);
                Ok(FakeAccess::default())
            },
        );
        assert!(result.is_err());
        assert!(!opened.get());
        assert_eq!(non_tty.masked_calls.get(), 0);

        let mismatch = FakeInput {
            interactive: true,
            masked: b"first-value".to_vec(),
            masked_confirmation: Some(b"second-value".to_vec()),
            stdin: Vec::new(),
            confirmed: false,
            masked_calls: Cell::new(0),
        };
        let opened = Cell::new(false);
        let result = prepare_and_write_native_credential(
            NativeWriteMode::Set,
            &credential_name(),
            false,
            &mismatch,
            || {
                opened.set(true);
                Ok(FakeAccess::default())
            },
        );
        assert!(result.is_err());
        assert!(!opened.get());
    }

    #[test]
    fn stdin_strips_one_line_ending_and_rejects_invalid_or_oversized_values() {
        assert!(validate_secret_bytes(b"one-line\n".to_vec(), true).is_ok());
        assert!(validate_secret_bytes(b"one-line\r\n".to_vec(), true).is_ok());
        for invalid in [
            b"\n".to_vec(),
            b"\r\n".to_vec(),
            b"two\nlines\n".to_vec(),
            b"two\rlines\n".to_vec(),
            b"contains\0nul\n".to_vec(),
            vec![0x01, b'\n'],
            vec![b'x'; NATIVE_CREDENTIAL_MAX_BYTES + 1],
        ] {
            assert!(validate_secret_bytes(invalid, true).is_err());
        }
        assert!(validate_secret_bytes(vec![b'x'; NATIVE_CREDENTIAL_MAX_BYTES], true).is_ok());
    }

    #[test]
    fn environment_import_does_not_mutate_or_remove_the_source() {
        let source = String::from("imported-value\n");
        let observed = Cell::new(false);
        import_native_credential_from_environment(
            &credential_name(),
            "RELAY_TOKEN",
            |name| {
                assert_eq!(name, "RELAY_TOKEN");
                observed.set(true);
                Ok(source.clone())
            },
            || Ok(FakeAccess::default()),
        )
        .expect("import credential");
        assert!(observed.get());
        assert_eq!(source, "imported-value\n");

        let opened = Cell::new(false);
        let invalid = import_native_credential_from_environment(
            &credential_name(),
            "RELAY_TOKEN",
            |_| Ok("two\nlines\n".to_string()),
            || {
                opened.set(true);
                Ok(FakeAccess::default())
            },
        );
        assert!(invalid.is_err());
        assert!(!opened.get());
    }

    #[test]
    fn consumers_include_only_effective_native_references() {
        let mut config = HelperConfig::default();
        config.codex.providers.insert(
            "relay".to_string(),
            crate::config::ProviderConfig {
                auth: crate::config::UpstreamAuth {
                    auth_token_ref: Some(CredentialRef::Native {
                        name: "shadowed".to_string(),
                    }),
                    ..crate::config::UpstreamAuth::default()
                },
                inline_auth: crate::config::UpstreamAuth {
                    auth_token_ref: Some(CredentialRef::Native {
                        name: "relay.primary".to_string(),
                    }),
                    ..crate::config::UpstreamAuth::default()
                },
                ..crate::config::ProviderConfig::default()
            },
        );

        assert!(configured_consumers(&config, "shadowed").is_empty());
        assert_eq!(configured_consumers(&config, "relay.primary").len(), 1);
        assert_eq!(configured_native_names(&config).len(), 1);
    }

    #[test]
    fn runtime_projection_distinguishes_publication_state_from_store_readiness() {
        let projection = RuntimeCredentialProjection::Available {
            service: "codex".to_string(),
            runtime_revision: Some(42),
            readiness: BTreeMap::from([(
                "relay.primary".to_string(),
                vec![
                    CredentialReadinessCode::Ready,
                    CredentialReadinessCode::Stale,
                ],
            )]),
        };
        let stale = projection.refresh_view("relay.primary");
        assert_eq!(stale.status, "runtime_stale");
        assert_eq!(stale.service.as_deref(), Some("codex"));
        assert_eq!(stale.runtime_revision, Some(42));

        assert_eq!(
            projection.refresh_view("unreferenced").status,
            "not_referenced"
        );
        assert_eq!(
            RuntimeCredentialProjection::Unavailable
                .refresh_view("relay.primary")
                .status,
            "target_unavailable"
        );
    }

    #[test]
    fn delete_confirmation_and_partial_refresh_never_rewrite_or_rollback_state() {
        let name = credential_name();
        let access = FakeAccess::default();
        access
            .set(
                &name,
                &SecretValue::new(b"secret-canary".to_vec()).expect("secret"),
            )
            .expect("set fake credential");
        let config = HelperConfig::default();
        let before = toml::to_string(&config).expect("serialize config before delete");

        let non_interactive = FakeInput {
            interactive: false,
            masked: Vec::new(),
            masked_confirmation: None,
            stdin: Vec::new(),
            confirmed: false,
            masked_calls: Cell::new(0),
        };
        assert!(require_delete_confirmation(&name, false, &non_interactive).is_err());
        assert!(
            access
                .values
                .lock()
                .expect("fake values")
                .contains(name.as_str())
        );

        let partial = committed_refresh_error(&name, "set", "admin unavailable");
        assert!(partial.to_string().contains("restart"));
        assert!(
            access
                .values
                .lock()
                .expect("fake values")
                .contains(name.as_str())
        );
        assert_eq!(
            toml::to_string(&config).expect("serialize config after partial refresh"),
            before
        );

        require_delete_confirmation(&name, true, &non_interactive).expect("--yes confirms delete");
        access.delete(&name).expect("delete fake credential");
        assert!(
            !access
                .values
                .lock()
                .expect("fake values")
                .contains(name.as_str())
        );
        assert_eq!(
            toml::to_string(&config).expect("serialize config after delete"),
            before
        );

        let opened = Cell::new(0);
        let absent = delete_native_credential(&name, false, true, &non_interactive, || {
            opened.set(opened.get() + 1);
            Ok(FakeAccess::default())
        })
        .expect("--if-exists succeeds without confirmation when absent");
        assert_eq!(absent, NativeDeleteOutcome::AlreadyAbsent);
        assert_eq!(opened.get(), 1);
    }

    #[test]
    fn status_and_partial_error_never_render_secret_values() {
        const CANARY: &str = "credential-canary-do-not-render";
        let mut access = FakeAccess::default();
        access
            .values
            .get_mut()
            .expect("fake values")
            .insert("relay.primary".to_string());
        let record = credential_status_record(
            &access,
            &HelperConfig::default(),
            &RuntimeCredentialProjection::Unavailable,
            &credential_name(),
        );
        let json = serde_json::to_string(&record).expect("status JSON");
        let error = committed_refresh_error(&credential_name(), "set", "target unavailable");
        let rendered = format!("{json} {error}");
        assert!(!rendered.contains(CANARY));
        assert!(!rendered.contains("fingerprint"));
        assert!(!rendered.contains("value"));
        assert!(rendered.contains("store_committed_runtime_refresh_failed"));
    }

    #[test]
    fn imported_value_never_enters_cli_or_helper_owned_artifacts() {
        const CANARY: &str = "credential-import-canary-917264ab";
        let home = TempTestDir::new("codex-helper-cli-test-credential-canary");
        let logs = home.path().join("logs");
        let state = home.path().join("state");
        fs::create_dir_all(&logs).expect("create log directory");
        fs::create_dir_all(&state).expect("create state directory");
        let artifacts = [
            home.path().join("config.toml"),
            home.path().join("config.toml.bak"),
            logs.join("codex-helper.log"),
            state.join("state.sqlite"),
        ];
        for (index, path) in artifacts.iter().enumerate() {
            fs::write(path, format!("artifact-{index}")).expect("write artifact fixture");
        }

        import_native_credential_from_environment(
            &credential_name(),
            "RELAY_TOKEN",
            |_| Ok(CANARY.to_string()),
            || Ok(FakeAccess::default()),
        )
        .expect("import canary through explicit environment source");

        let argv_capture = serde_json::to_string(&[
            "codex-helper",
            "credential",
            "import",
            "relay.primary",
            "--from-env",
            "RELAY_TOKEN",
        ])
        .expect("serialize argv capture");
        let access = FakeAccess::default();
        let status = serde_json::to_string(&credential_status_record(
            &access,
            &HelperConfig::default(),
            &RuntimeCredentialProjection::Unavailable,
            &credential_name(),
        ))
        .expect("serialize status output");
        let stderr = committed_refresh_error(
            &credential_name(),
            "imported",
            "resident target unavailable",
        )
        .to_string();

        assert!(!argv_capture.contains(CANARY));
        assert!(!status.contains(CANARY));
        assert!(!stderr.contains(CANARY));
        for path in artifacts {
            let contents = fs::read(path).expect("read helper-owned artifact");
            assert!(
                !contents
                    .windows(CANARY.len())
                    .any(|window| window == CANARY.as_bytes())
            );
        }
    }
}

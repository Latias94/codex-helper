use std::ffi::OsString;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[cfg(any(windows, test))]
use serde::Deserialize;
use serde::Serialize;

use crate::cli_types::{CliError, CliResult, ServiceCommand};
use crate::config::proxy_home_dir;
use crate::service_receipt::{
    ServicePlatformBackend, ServiceReceipt, ServiceReceiptError, ServiceReceiptTransaction,
    read_service_receipt,
};

#[cfg(windows)]
const WINDOWS_SERVICE_NAME: &str = "codex-helper";
#[cfg(any(windows, test))]
const WINDOWS_TASK_BASENAME: &str = "codex-helper";
#[cfg(windows)]
const WINDOWS_TASK_DEFINITION_FILE: &str = "windows-task.xml";
#[cfg(target_os = "macos")]
const MACOS_LABEL: &str = "io.github.latias94.codex-helper";
#[cfg(target_os = "linux")]
const LINUX_UNIT_NAME: &str = "codex-helper.service";
const MAX_SERVICE_DEFINITION_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServicePlatform {
    Windows,
    Macos,
    Linux,
    Unsupported,
}

impl ServicePlatform {
    fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Unsupported
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub(crate) enum ServiceRuntimeState {
    Running,
    Stopped,
    Starting,
    Stopping,
    Installed,
    NotInstalled,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServiceReceiptState {
    Absent,
    Current,
    Legacy,
    Unsupported,
    Invalid,
    Foreign,
    PlatformMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServiceCredentialContext {
    Unverified,
    Ready,
    Degraded,
    Blocked,
    RuntimeUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ServiceStatus {
    platform: ServicePlatform,
    service_name: String,
    state: ServiceRuntimeState,
    installed: bool,
    autostart: bool,
    service_definition: Option<PathBuf>,
    log_directory: PathBuf,
    detail: Option<String>,
    receipt_state: ServiceReceiptState,
    credential_context: ServiceCredentialContext,
    runtime_identity_verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    install_generation: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ServiceInstallOptions {
    pub(crate) service_name: &'static str,
    pub(crate) host: IpAddr,
    pub(crate) port: u16,
    pub(crate) start: bool,
    pub(crate) helper_home: PathBuf,
    pub(crate) client_home: PathBuf,
    pub(crate) install_generation: codex_helper_core::service_target::ServiceInstallGeneration,
}

pub(crate) async fn handle_service_command(command: ServiceCommand) -> CliResult<()> {
    match command {
        ServiceCommand::Install {
            codex,
            claude,
            host,
            port,
            no_start,
        } => {
            let service_name = service_name_from_flags(codex, claude)?;
            let port = port.unwrap_or_else(|| default_proxy_port(service_name));
            let install_generation =
                codex_helper_core::service_target::ServiceInstallGeneration::generate();
            let options = ServiceInstallOptions {
                service_name,
                host,
                port,
                start: !no_start,
                helper_home: proxy_home_dir(),
                client_home: service_client_home(service_name),
                install_generation,
            };
            preflight_service_credentials(service_kind(service_name)?, &options.helper_home)
                .await?;
            ensure_service_operator_token()?;
            install(options)?;
            if no_start {
                println!(
                    "Credential readiness is valid in the installer context but remains unverified in the installed service context until start."
                );
            } else {
                verify_started_service_runtime().await?;
            }
            print_status(&service_status().await?, false)?;
        }
        ServiceCommand::Uninstall { keep_running } => uninstall_with_receipt(!keep_running)?,
        ServiceCommand::Start => {
            preflight_installed_service_credentials().await?;
            ensure_service_operator_token()?;
            start()?;
            verify_started_service_runtime().await?;
        }
        ServiceCommand::Stop => stop()?,
        ServiceCommand::Restart => {
            preflight_installed_service_credentials().await?;
            ensure_service_operator_token()?;
            stop()?;
            start()?;
            verify_started_service_runtime().await?;
        }
        ServiceCommand::Status { json } => print_status(&service_status().await?, json)?,
        ServiceCommand::Logs => print_logs(),
        ServiceCommand::Run {
            service_name,
            host,
            port,
            helper_home,
            client_home,
        } => {
            let service_name = service_name_from_value(&service_name)?;
            let helper_home = helper_home.unwrap_or_else(proxy_home_dir);
            let client_home = client_home
                .unwrap_or_else(|| legacy_service_client_home(service_name, &helper_home));
            let install_generation = service_process_install_generation()?;
            configure_service_process(
                service_name,
                &helper_home,
                &client_home,
                &install_generation,
            );
            run_service_dispatcher(ServiceInstallOptions {
                service_name,
                host,
                port: port.unwrap_or_else(|| default_proxy_port(service_name)),
                start: false,
                helper_home,
                client_home,
                install_generation,
            })?;
        }
        ServiceCommand::TaskRun {
            service_name,
            host,
            port,
            helper_home,
            client_home,
            install_generation,
        } => {
            let service_name = service_name_from_value(&service_name)?;
            let helper_home = helper_home.unwrap_or_else(proxy_home_dir);
            let client_home = client_home
                .unwrap_or_else(|| legacy_service_client_home(service_name, &helper_home));
            let install_generation = command_install_generation(install_generation.as_deref())?;
            configure_service_process(
                service_name,
                &helper_home,
                &client_home,
                &install_generation,
            );
            crate::cli_app::run_service_managed_server(
                service_name,
                host,
                port.unwrap_or_else(|| default_proxy_port(service_name)),
            )
            .await?;
        }
    }
    Ok(())
}

pub(crate) fn configure_service_command_environment(command: &ServiceCommand) -> CliResult<()> {
    let (service_name, helper_home, client_home, install_generation) = match command {
        ServiceCommand::Run {
            service_name,
            helper_home,
            client_home,
            ..
        } => (service_name, helper_home, client_home, None),
        ServiceCommand::TaskRun {
            service_name,
            helper_home,
            client_home,
            install_generation,
            ..
        } => (
            service_name,
            helper_home,
            client_home,
            install_generation.as_deref(),
        ),
        _ => return Ok(()),
    };
    let service_name = service_name_from_value(service_name)?;
    let helper_home = helper_home.clone().unwrap_or_else(proxy_home_dir);
    let client_home = client_home
        .clone()
        .unwrap_or_else(|| legacy_service_client_home(service_name, &helper_home));
    let install_generation = command_install_generation(install_generation)?;
    configure_service_process(
        service_name,
        &helper_home,
        &client_home,
        &install_generation,
    );
    Ok(())
}

fn service_name_from_value(value: &str) -> CliResult<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "codex" => Ok("codex"),
        "claude" => Ok("claude"),
        _ => Err(CliError::Other(format!(
            "unsupported service name '{value}'; expected codex or claude"
        ))),
    }
}

fn command_install_generation(
    value: Option<&str>,
) -> CliResult<codex_helper_core::service_target::ServiceInstallGeneration> {
    if let Some(value) = value {
        return codex_helper_core::service_target::ServiceInstallGeneration::parse(value)
            .map_err(|error| CliError::Other(error.to_string()));
    }
    service_process_install_generation()
}

fn service_process_install_generation()
-> CliResult<codex_helper_core::service_target::ServiceInstallGeneration> {
    codex_helper_core::service_target::ServiceInstallGeneration::from_process_env()
        .map_err(|error| CliError::Other(error.to_string()))
        .map(|generation| {
            generation.unwrap_or_else(
                codex_helper_core::service_target::ServiceInstallGeneration::generate,
            )
        })
}

fn configure_service_process(
    service_name: &str,
    helper_home: &Path,
    client_home: &Path,
    install_generation: &codex_helper_core::service_target::ServiceInstallGeneration,
) {
    unsafe {
        std::env::set_var("CODEX_HELPER_HOME", helper_home);
        std::env::set_var(service_client_home_env(service_name), client_home);
        std::env::set_var(
            codex_helper_core::service_target::SERVICE_INSTALL_GENERATION_ENV_VAR,
            install_generation.as_str(),
        );
    }
}

fn service_client_home(service_name: &str) -> PathBuf {
    if service_name == "claude" {
        crate::config::claude_home()
    } else {
        crate::config::codex_home()
    }
}

fn service_client_home_env(service_name: &str) -> &'static str {
    if service_name == "claude" {
        "CLAUDE_HOME"
    } else {
        "CODEX_HOME"
    }
}

fn legacy_service_client_home(service_name: &str, helper_home: &Path) -> PathBuf {
    let default_name = if service_name == "claude" {
        ".claude"
    } else {
        ".codex"
    };
    let is_default_helper_home = helper_home
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(".codex-helper"));
    if is_default_helper_home && let Some(parent) = helper_home.parent() {
        return parent.join(default_name);
    }
    service_client_home(service_name)
}

#[cfg(any(windows, test))]
fn service_task_arguments(options: &ServiceInstallOptions) -> Vec<OsString> {
    vec![
        OsString::from("service"),
        OsString::from("task-run"),
        OsString::from("--service-name"),
        OsString::from(options.service_name),
        OsString::from("--host"),
        OsString::from(options.host.to_string()),
        OsString::from("--port"),
        OsString::from(options.port.to_string()),
        OsString::from("--helper-home"),
        options.helper_home.clone().into_os_string(),
        OsString::from("--client-home"),
        options.client_home.clone().into_os_string(),
        OsString::from("--install-generation"),
        OsString::from(options.install_generation.as_str()),
    ]
}

#[cfg(any(windows, test))]
fn quote_windows_argument(value: &str) -> String {
    if !value.is_empty()
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character == '"')
    {
        return value.to_string();
    }

    let mut quoted = String::from("\"");
    let mut backslashes = 0_usize;
    for character in value.chars() {
        match character {
            '\\' => backslashes = backslashes.saturating_add(1),
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', backslashes.saturating_mul(2) + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push(character);
                backslashes = 0;
            }
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes.saturating_mul(2)));
    quoted.push('"');
    quoted
}

#[cfg(any(windows, test))]
fn render_windows_task_definition(
    executable: &Path,
    options: &ServiceInstallOptions,
    user_sid: &str,
) -> String {
    let arguments = service_task_arguments(options)
        .iter()
        .map(|argument| quote_windows_argument(argument.to_string_lossy().as_ref()))
        .collect::<Vec<_>>()
        .join(" ");
    let working_directory = executable.parent().unwrap_or_else(|| Path::new("."));
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n<RegistrationInfo><Description>Resident codex-helper relay for the current user.</Description></RegistrationInfo>\n<Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{}</UserId></LogonTrigger></Triggers>\n<Principals><Principal id=\"Author\"><UserId>{}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>\n<Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><AllowHardTerminate>true</AllowHardTerminate><StartWhenAvailable>true</StartWhenAvailable><RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable><AllowStartOnDemand>true</AllowStartOnDemand><Enabled>true</Enabled><Hidden>false</Hidden><RunOnlyIfIdle>false</RunOnlyIfIdle><WakeToRun>false</WakeToRun><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><Priority>7</Priority><RestartOnFailure><Interval>PT10S</Interval><Count>10</Count></RestartOnFailure></Settings>\n<Actions Context=\"Author\"><Exec><Command>{}</Command><Arguments>{}</Arguments><WorkingDirectory>{}</WorkingDirectory></Exec></Actions>\n</Task>\n",
        xml_escape(user_sid),
        xml_escape(user_sid),
        xml_escape(executable.to_string_lossy().as_ref()),
        xml_escape(&arguments),
        xml_escape(working_directory.to_string_lossy().as_ref()),
    )
}

#[cfg(any(windows, test))]
fn windows_task_name_for_sid(user_sid: &str) -> CliResult<String> {
    let components = user_sid.split('-').collect::<Vec<_>>();
    let valid = components.len() >= 4
        && components[0].eq_ignore_ascii_case("s")
        && components[1..].iter().all(|component| {
            !component.is_empty() && component.bytes().all(|byte| byte.is_ascii_digit())
        });
    if !valid || user_sid.len() > 184 {
        return Err(CliError::Other(
            "the current Windows principal returned an invalid SID".to_string(),
        ));
    }
    Ok(format!("{WINDOWS_TASK_BASENAME}-{user_sid}"))
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct WindowsTaskRecord {
    task_name: String,
    task_path: String,
    owner_sid: String,
    state: u8,
    enabled: bool,
    action_count: usize,
    execute: String,
    arguments: String,
    working_directory: String,
    logon_type: String,
    run_level: String,
    trigger_count: usize,
    trigger_enabled: bool,
    trigger_type: String,
    trigger_user_sid: String,
}

#[cfg(any(windows, test))]
#[derive(Debug, Deserialize)]
struct WindowsTaskProbe {
    found: bool,
    #[serde(default)]
    record: Option<WindowsTaskRecord>,
}

#[cfg(any(windows, test))]
fn windows_task_owner_matches(record: &WindowsTaskRecord, expected_sid: &str) -> bool {
    record.owner_sid.eq_ignore_ascii_case(expected_sid)
}

#[cfg(any(windows, test))]
fn parse_windows_task_probe(output: &str) -> CliResult<Option<WindowsTaskRecord>> {
    let probe = serde_json::from_str::<WindowsTaskProbe>(output).map_err(|error| {
        CliError::Other(format!(
            "parse the Windows Scheduled Task probe response: {error}"
        ))
    })?;
    match (probe.found, probe.record) {
        (false, None) => Ok(None),
        (true, Some(record)) => Ok(Some(record)),
        (false, Some(_)) | (true, None) => Err(CliError::Other(
            "the Windows Scheduled Task probe returned an inconsistent response".to_string(),
        )),
    }
}

#[cfg(any(windows, test))]
fn windows_path_text<'a>(path: &'a Path, description: &str) -> CliResult<&'a str> {
    path.to_str().ok_or_else(|| {
        CliError::Other(format!(
            "{description} is not valid Unicode and cannot be stored in a Windows task definition"
        ))
    })
}

#[cfg(any(windows, test))]
fn windows_paths_equal(left: &str, right: &str) -> bool {
    let normalize = |value: &str| {
        value
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    normalize(left) == normalize(right)
}

#[cfg(any(windows, test))]
fn verify_windows_task_record(
    record: &WindowsTaskRecord,
    task_name: &str,
    user_sid: &str,
    executable: &Path,
    options: &ServiceInstallOptions,
) -> CliResult<()> {
    let executable = windows_path_text(executable, "the codex-helper executable path")?;
    let expected_arguments = service_task_arguments(options)
        .iter()
        .map(|argument| quote_windows_argument(argument.to_string_lossy().as_ref()))
        .collect::<Vec<_>>()
        .join(" ");
    let working_directory = windows_path_text(
        Path::new(executable)
            .parent()
            .unwrap_or_else(|| Path::new(".")),
        "the codex-helper working directory",
    )?;
    let run_level_is_limited = record.run_level.eq_ignore_ascii_case("limited")
        || record.run_level.eq_ignore_ascii_case("leastprivilege");
    let logon_type_is_interactive = record.logon_type.eq_ignore_ascii_case("interactive")
        || record.logon_type.eq_ignore_ascii_case("interactivetoken");
    let valid = record.task_name == task_name
        && record.task_path == "\\"
        && windows_task_owner_matches(record, user_sid)
        && matches!(record.state, 2..=4)
        && record.enabled
        && record.action_count == 1
        && windows_paths_equal(&record.execute, executable)
        && record.arguments == expected_arguments
        && windows_paths_equal(&record.working_directory, working_directory)
        && logon_type_is_interactive
        && run_level_is_limited
        && record.trigger_count == 1
        && record.trigger_enabled
        && record
            .trigger_type
            .eq_ignore_ascii_case("MSFT_TaskLogonTrigger")
        && record.trigger_user_sid.eq_ignore_ascii_case(user_sid);
    if valid {
        Ok(())
    } else {
        Err(CliError::Other(format!(
            "the registered Windows task '{task_name}' did not match the verified SID, action, trigger, or least-privilege definition"
        )))
    }
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsServiceProbeClassification {
    Missing,
    Error,
}

#[cfg(any(windows, test))]
fn classify_windows_service_probe_error(
    raw_os_error: Option<i32>,
) -> WindowsServiceProbeClassification {
    const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;
    if raw_os_error == Some(ERROR_SERVICE_DOES_NOT_EXIST) {
        WindowsServiceProbeClassification::Missing
    } else {
        WindowsServiceProbeClassification::Error
    }
}

#[cfg(any(windows, test))]
trait WindowsInstallTransactionBackend {
    fn preflight(&mut self) -> CliResult<()>;
    fn register_scoped_task(&mut self) -> CliResult<()>;
    fn verify_scoped_task(&mut self) -> CliResult<()>;
    fn publish_receipt(&mut self) -> CliResult<()>;
    fn retire_owned_fixed_task(&mut self) -> CliResult<()>;
    fn retire_legacy_scm(&mut self) -> CliResult<()>;
    fn rollback_receipt(&mut self) -> CliResult<()>;
    fn rollback(&mut self) -> CliResult<()>;
    fn start_scoped_task(&mut self) -> CliResult<()>;
}

#[cfg(any(windows, test))]
fn run_windows_install_transaction(
    backend: &mut impl WindowsInstallTransactionBackend,
    start: bool,
) -> CliResult<()> {
    backend.preflight()?;
    let mut new_task_verified = false;
    let migration = (|| {
        backend.register_scoped_task()?;
        backend.verify_scoped_task()?;
        new_task_verified = true;
        backend.publish_receipt()?;
        backend.retire_owned_fixed_task()?;
        backend.retire_legacy_scm()
    })();
    if let Err(primary) = migration {
        let receipt_rollback = backend.rollback_receipt();
        let platform_rollback = backend.rollback();
        return match (receipt_rollback, platform_rollback) {
            (Ok(()), Ok(())) => Err(CliError::Other(format!(
                "Windows service migration failed and the previous runnable installation was restored: {primary}"
            ))),
            (receipt_rollback, platform_rollback) => {
                let fallback = if new_task_verified {
                    "The verified SID-scoped task was left installed when required to avoid removing the last runnable installation"
                } else {
                    "The legacy installations were not retired; inspect and remove any partially registered SID-scoped task only after verifying its Principal SID"
                };
                let failures = [receipt_rollback.err(), platform_rollback.err()]
                    .into_iter()
                    .flatten()
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("; ");
                Err(CliError::Other(format!(
                    "Windows service migration failed: {primary}; rollback also failed: {failures}. {fallback}"
                )))
            }
        };
    }
    if start {
        backend.start_scoped_task().map_err(|error| {
            CliError::Other(format!(
                "the SID-scoped Windows task was installed and the legacy installation was retired, but starting the new task failed: {error}"
            ))
        })?;
    }
    Ok(())
}

fn service_name_from_flags(codex: bool, claude: bool) -> CliResult<&'static str> {
    match (codex, claude) {
        (true, true) => Err(CliError::Other(
            "--codex and --claude are mutually exclusive".to_string(),
        )),
        (_, true) => Ok("claude"),
        _ => Ok("codex"),
    }
}

fn service_kind(service_name: &str) -> CliResult<codex_helper_core::config::ServiceKind> {
    match service_name {
        "codex" => Ok(codex_helper_core::config::ServiceKind::Codex),
        "claude" => Ok(codex_helper_core::config::ServiceKind::Claude),
        _ => Err(CliError::Other(format!(
            "unsupported service name '{service_name}'; expected codex or claude"
        ))),
    }
}

fn service_receipt(options: &ServiceInstallOptions) -> CliResult<ServiceReceipt> {
    let platform_backend = ServicePlatformBackend::current().ok_or_else(|| {
        CliError::Other("system service management is unsupported on this platform".to_string())
    })?;
    ServiceReceipt::new(
        service_kind(options.service_name)?,
        options.helper_home.clone(),
        options.client_home.clone(),
        codex_helper_core::proxy::local_admin_base_url_for_proxy_port(options.port),
        platform_backend,
        options.install_generation.clone(),
    )
    .map_err(|error| CliError::Other(format!("build service install receipt: {error}")))
}

fn begin_service_receipt_transaction(
    options: &ServiceInstallOptions,
) -> CliResult<(ServiceReceiptTransaction, ServiceReceipt)> {
    let receipt = service_receipt(options)?;
    let transaction = ServiceReceiptTransaction::begin_install_replacement(
        options.helper_home.clone(),
    )
    .map_err(|error| {
        CliError::Other(format!(
            "begin service receipt install replacement: {error}"
        ))
    })?;
    Ok((transaction, receipt))
}

async fn preflight_installed_service_credentials()
-> CliResult<codex_helper_core::service_readiness::ServiceCredentialReadinessReport> {
    let helper_home = proxy_home_dir();
    let receipt = read_service_receipt(&helper_home).map_err(|error| {
        CliError::Other(format!(
            "cannot preflight the installed service without a current receipt: {error}; run `codex-helper service install`"
        ))
    })?;
    if ServicePlatformBackend::current() != Some(receipt.platform_backend()) {
        return Err(CliError::Other(
            "the installed service receipt targets a different platform backend; run `codex-helper service install`"
                .to_string(),
        ));
    }
    preflight_service_credentials(receipt.service(), receipt.helper_home()).await
}

async fn preflight_service_credentials(
    service: codex_helper_core::config::ServiceKind,
    helper_home: &Path,
) -> CliResult<codex_helper_core::service_readiness::ServiceCredentialReadinessReport> {
    let config = codex_helper_core::config::load_config()
        .await
        .map_err(|error| CliError::Configuration(error.to_string()))?;
    let helper_home = helper_home.to_path_buf();
    let report = tokio::task::spawn_blocking(move || {
        codex_helper_core::service_readiness::evaluate_service_credential_readiness(
            &config,
            service,
            codex_helper_core::credentials::CredentialSourceCapabilities::platform_native(),
            helper_home,
        )
    })
    .await
    .map_err(|error| CliError::Other(format!("credential preflight task failed: {error}")))?
    .map_err(|error| CliError::Other(format!("credential preflight failed: {error}")))?;
    let unavailable = report
        .endpoints
        .iter()
        .filter(|endpoint| !endpoint.code.is_routable())
        .map(|endpoint| {
            format!(
                "{}/{}={}",
                endpoint.provider_id,
                endpoint.endpoint_id,
                endpoint.code.as_str()
            )
        })
        .collect::<Vec<_>>();
    match report.aggregate {
        codex_helper_core::credentials::CredentialAggregateReadiness::Ready => {}
        codex_helper_core::credentials::CredentialAggregateReadiness::Degraded => {
            eprintln!(
                "Credential preflight is degraded; unavailable endpoints: {}",
                unavailable.join(", ")
            );
        }
        codex_helper_core::credentials::CredentialAggregateReadiness::Blocked => {
            return Err(CliError::Other(format!(
                "credential preflight is blocked; no route is usable ({})",
                unavailable.join(", ")
            )));
        }
    }
    Ok(report)
}

fn ensure_service_operator_token() -> CliResult<()> {
    codex_helper_core::local_operator::ensure_local_operator_token()
        .map(|_| ())
        .map_err(|error| CliError::Other(format!("prepare local service operator token: {error}")))
}

async fn verify_started_service_runtime() -> CliResult<()> {
    const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
    const POLL_INTERVAL: Duration = Duration::from_millis(200);
    const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

    let receipt = read_service_receipt(proxy_home_dir()).map_err(|error| {
        CliError::Other(format!(
            "cannot verify the started service without its committed receipt: {error}"
        ))
    })?;
    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
    loop {
        match read_service_runtime_with_timeout(receipt.clone(), PROBE_TIMEOUT).await {
            Ok(runtime) => {
                return ensure_started_service_credential_readiness(runtime.credential_readiness);
            }
            Err(error) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(CliError::Other(format!(
                        "installed service did not publish its matching runtime identity within {} seconds; the service remains installed for diagnosis: {error}",
                        STARTUP_TIMEOUT.as_secs(),
                    )));
                }
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn ensure_started_service_credential_readiness(
    readiness: codex_helper_core::credentials::CredentialAggregateReadiness,
) -> CliResult<()> {
    match readiness {
        codex_helper_core::credentials::CredentialAggregateReadiness::Ready => Ok(()),
        codex_helper_core::credentials::CredentialAggregateReadiness::Degraded => {
            eprintln!(
                "Installed service is running with degraded credential readiness; inspect `codex-helper service status`."
            );
            Ok(())
        }
        codex_helper_core::credentials::CredentialAggregateReadiness::Blocked => {
            Err(CliError::Other(
                "installed service is running but credential readiness is blocked; the local admin endpoint remains available for diagnosis"
                    .to_string(),
            ))
        }
    }
}

async fn read_service_runtime_with_timeout(
    receipt: ServiceReceipt,
    timeout: Duration,
) -> Result<codex_helper_core::service_target::LocalServiceRuntimeReadResponse, String> {
    tokio::time::timeout(
        timeout,
        crate::cli_app::read_service_runtime_for_receipt(receipt),
    )
    .await
    .map_err(|_| {
        format!(
            "local service runtime probe exceeded {}ms",
            timeout.as_millis()
        )
    })?
    .map_err(|error| error.to_string())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn rollback_error(primary: CliError, failures: Vec<String>) -> CliError {
    if failures.is_empty() {
        CliError::Other(format!(
            "service installation failed and the previous installation was restored: {primary}"
        ))
    } else {
        CliError::Other(format!(
            "service installation failed: {primary}; rollback also failed: {}",
            failures.join("; ")
        ))
    }
}

fn default_proxy_port(service_name: &str) -> u16 {
    if service_name == "claude" { 3210 } else { 3211 }
}

fn print_status(status: &ServiceStatus, json: bool) -> CliResult<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(status)
                .map_err(|error| CliError::Other(error.to_string()))?
        );
        return Ok(());
    }
    println!("codex-helper service");
    println!("  platform: {:?}", status.platform);
    println!("  installed: {}", status.installed);
    println!("  autostart: {}", status.autostart);
    println!("  state: {:?}", status.state);
    println!("  receipt: {:?}", status.receipt_state);
    println!("  credential context: {:?}", status.credential_context);
    println!(
        "  runtime identity verified: {}",
        status.runtime_identity_verified
    );
    if let Some(generation) = status.install_generation.as_deref() {
        println!("  install generation: {generation}");
    }
    if let Some(path) = status.service_definition.as_ref() {
        println!("  definition: {}", path.display());
    }
    println!("  logs: {}", status.log_directory.display());
    if let Some(detail) = status.detail.as_deref() {
        println!("  detail: {detail}");
    }
    Ok(())
}

fn print_logs() {
    let log_dir = service_log_dir();
    println!("{}", log_dir.display());
    for file_name in ["runtime.log", "service.stdout.log", "service.stderr.log"] {
        let path = log_dir.join(file_name);
        if path.exists() {
            println!("  {}", path.display());
        }
    }
}

fn uninstall_with_receipt(stop_first: bool) -> CliResult<()> {
    let mut receipt = ServiceReceiptTransaction::begin(proxy_home_dir())
        .map_err(|error| CliError::Other(format!("begin service receipt removal: {error}")))?;
    receipt
        .remove()
        .map_err(|error| CliError::Other(format!("remove service receipt: {error}")))?;
    if let Err(primary) = uninstall(stop_first) {
        return match receipt.rollback() {
            Ok(()) => Err(CliError::Other(format!(
                "service uninstall failed and its receipt was restored: {primary}"
            ))),
            Err(rollback) => Err(CliError::Other(format!(
                "service uninstall failed: {primary}; restoring its receipt also failed: {rollback}"
            ))),
        };
    }
    Ok(())
}

fn service_log_dir() -> PathBuf {
    proxy_home_dir().join("logs")
}

fn current_executable() -> CliResult<PathBuf> {
    std::env::current_exe()
        .map_err(|error| CliError::Other(format!("resolve current executable: {error}")))
}

fn ensure_service_log_dir() -> CliResult<PathBuf> {
    let path = service_log_dir();
    std::fs::create_dir_all(&path).map_err(|error| {
        CliError::Other(format!(
            "create service log directory {}: {error}",
            path.display()
        ))
    })?;
    Ok(path)
}

fn run_command(program: &str, args: &[OsString]) -> CliResult<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| CliError::Other(format!("run {program}: {error}")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(CliError::Other(format!(
        "{program} failed with {}: {detail}",
        output.status
    )))
}

#[cfg(windows)]
fn install(options: ServiceInstallOptions) -> CliResult<()> {
    windows::install(options)
}

#[cfg(target_os = "macos")]
fn install(options: ServiceInstallOptions) -> CliResult<()> {
    macos::install(options)
}

#[cfg(target_os = "linux")]
fn install(options: ServiceInstallOptions) -> CliResult<()> {
    linux::install(options)
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn install(_options: ServiceInstallOptions) -> CliResult<()> {
    Err(unsupported_platform())
}

#[cfg(windows)]
fn uninstall(stop_first: bool) -> CliResult<()> {
    windows::uninstall(stop_first)
}

#[cfg(target_os = "macos")]
fn uninstall(stop_first: bool) -> CliResult<()> {
    macos::uninstall(stop_first)
}

#[cfg(target_os = "linux")]
fn uninstall(stop_first: bool) -> CliResult<()> {
    linux::uninstall(stop_first)
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn uninstall(_stop_first: bool) -> CliResult<()> {
    Err(unsupported_platform())
}

#[cfg(windows)]
fn start() -> CliResult<()> {
    windows::start()
}

#[cfg(target_os = "macos")]
fn start() -> CliResult<()> {
    macos::start()
}

#[cfg(target_os = "linux")]
fn start() -> CliResult<()> {
    linux::start()
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn start() -> CliResult<()> {
    Err(unsupported_platform())
}

#[cfg(windows)]
fn stop() -> CliResult<()> {
    windows::stop()
}

#[cfg(target_os = "macos")]
fn stop() -> CliResult<()> {
    macos::stop()
}

#[cfg(target_os = "linux")]
fn stop() -> CliResult<()> {
    linux::stop()
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn stop() -> CliResult<()> {
    Err(unsupported_platform())
}

#[cfg(windows)]
fn status() -> CliResult<ServiceStatus> {
    windows::status()
}

#[cfg(target_os = "macos")]
fn status() -> CliResult<ServiceStatus> {
    macos::status()
}

#[cfg(target_os = "linux")]
fn status() -> CliResult<ServiceStatus> {
    linux::status()
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn status() -> CliResult<ServiceStatus> {
    Err(unsupported_platform())
}

async fn service_status() -> CliResult<ServiceStatus> {
    Ok(enrich_service_status(status()?, &proxy_home_dir()).await)
}

async fn enrich_service_status(
    mut service_status: ServiceStatus,
    helper_home: &Path,
) -> ServiceStatus {
    let receipt = match read_service_receipt(helper_home) {
        Ok(receipt) => receipt,
        Err(ServiceReceiptError::Missing) => {
            service_status.receipt_state = ServiceReceiptState::Absent;
            return service_status;
        }
        Err(ServiceReceiptError::LegacySchema { .. }) => {
            service_status.receipt_state = ServiceReceiptState::Legacy;
            append_status_detail(
                &mut service_status,
                "service receipt uses an unsupported legacy schema; reinstall the service",
            );
            return service_status;
        }
        Err(ServiceReceiptError::UnsupportedSchema { .. }) => {
            service_status.receipt_state = ServiceReceiptState::Unsupported;
            append_status_detail(
                &mut service_status,
                "service receipt uses a newer unsupported schema; upgrade codex-helper",
            );
            return service_status;
        }
        Err(ServiceReceiptError::ForeignHelperHome) => {
            service_status.receipt_state = ServiceReceiptState::Foreign;
            append_status_detail(
                &mut service_status,
                "service receipt belongs to a different helper home",
            );
            return service_status;
        }
        Err(error) => {
            service_status.receipt_state = ServiceReceiptState::Invalid;
            append_status_detail(
                &mut service_status,
                &format!("service receipt is invalid: {error}"),
            );
            return service_status;
        }
    };
    if ServicePlatformBackend::current() != Some(receipt.platform_backend()) {
        service_status.receipt_state = ServiceReceiptState::PlatformMismatch;
        append_status_detail(
            &mut service_status,
            "service receipt targets a different platform backend",
        );
        return service_status;
    }
    service_status.receipt_state = ServiceReceiptState::Current;
    service_status.install_generation = Some(receipt.install_generation().to_string());
    if !matches!(
        service_status.state,
        ServiceRuntimeState::Running | ServiceRuntimeState::Starting
    ) {
        return service_status;
    }
    match read_service_runtime_with_timeout(receipt, Duration::from_secs(2)).await {
        Ok(runtime) => {
            service_status.runtime_identity_verified = true;
            service_status.credential_context = match runtime.credential_readiness {
                codex_helper_core::credentials::CredentialAggregateReadiness::Ready => {
                    ServiceCredentialContext::Ready
                }
                codex_helper_core::credentials::CredentialAggregateReadiness::Degraded => {
                    ServiceCredentialContext::Degraded
                }
                codex_helper_core::credentials::CredentialAggregateReadiness::Blocked => {
                    ServiceCredentialContext::Blocked
                }
            };
        }
        Err(error) => {
            service_status.credential_context = ServiceCredentialContext::RuntimeUnavailable;
            append_status_detail(
                &mut service_status,
                &format!("service runtime identity/readiness is unavailable: {error}"),
            );
        }
    }
    service_status
}

fn append_status_detail(status: &mut ServiceStatus, detail: &str) {
    status.detail = Some(match status.detail.take() {
        Some(existing) if !existing.trim().is_empty() => format!("{existing}; {detail}"),
        Some(_) | None => detail.to_string(),
    });
}

#[cfg(windows)]
fn run_service_dispatcher(options: ServiceInstallOptions) -> CliResult<()> {
    windows::run_dispatcher(options)
}

#[cfg(not(windows))]
fn run_service_dispatcher(_options: ServiceInstallOptions) -> CliResult<()> {
    Err(CliError::Other(
        "service run is an internal Windows Service Control Manager entrypoint".to_string(),
    ))
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn unsupported_platform() -> CliError {
    CliError::Other("system service management is unsupported on this platform".to_string())
}

#[cfg(windows)]
mod windows {
    use std::ffi::{OsStr, OsString};
    use std::sync::{OnceLock, mpsc};

    use windows_service::define_windows_service;
    use windows_service::service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceStartType,
        ServiceState, ServiceStatus as WindowsStatus, ServiceType,
    };
    use windows_service::service_control_handler::{
        self, ServiceControlHandlerResult, ServiceStatusHandle,
    };
    use windows_service::service_dispatcher;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    use super::*;

    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;
    static SERVICE_OPTIONS: OnceLock<ServiceInstallOptions> = OnceLock::new();

    define_windows_service!(service_entry, service_main);

    #[derive(Debug, Clone)]
    struct OwnedTaskSnapshot {
        record: WindowsTaskRecord,
        definition: Vec<u8>,
    }

    #[derive(Debug, Clone, Copy)]
    struct LegacyScmSnapshot {
        was_running: bool,
    }

    struct WindowsInstallContext {
        executable: PathBuf,
        user_sid: String,
        scoped_task_name: String,
        definition_path: PathBuf,
        definition_document: Vec<u8>,
        scoped_snapshot: Option<OwnedTaskSnapshot>,
        fixed_snapshot: Option<OwnedTaskSnapshot>,
        legacy_scm: Option<LegacyScmSnapshot>,
    }

    struct NativeWindowsInstallBackend {
        options: ServiceInstallOptions,
        context: Option<WindowsInstallContext>,
        scoped_task_changed: bool,
        fixed_task_changed: bool,
        legacy_scm_stopped: bool,
        preserve_scoped_task: bool,
        definition_transaction: Option<codex_helper_core::ManagedFileTransaction>,
        receipt_transaction: Option<ServiceReceiptTransaction>,
        receipt: Option<ServiceReceipt>,
    }

    impl NativeWindowsInstallBackend {
        fn new(options: ServiceInstallOptions) -> Self {
            Self {
                options,
                context: None,
                scoped_task_changed: false,
                fixed_task_changed: false,
                legacy_scm_stopped: false,
                preserve_scoped_task: false,
                definition_transaction: None,
                receipt_transaction: None,
                receipt: None,
            }
        }

        fn context(&self) -> CliResult<&WindowsInstallContext> {
            self.context.as_ref().ok_or_else(|| {
                CliError::Other("Windows service installation was not preflighted".to_string())
            })
        }

        fn definition_transaction_mut(
            &mut self,
        ) -> CliResult<&mut codex_helper_core::ManagedFileTransaction> {
            self.definition_transaction.as_mut().ok_or_else(|| {
                CliError::Other("Windows definition transaction is unavailable".to_string())
            })
        }

        fn receipt_transaction_mut(&mut self) -> CliResult<&mut ServiceReceiptTransaction> {
            self.receipt_transaction.as_mut().ok_or_else(|| {
                CliError::Other("Windows receipt transaction is unavailable".to_string())
            })
        }
    }

    impl WindowsInstallTransactionBackend for NativeWindowsInstallBackend {
        fn preflight(&mut self) -> CliResult<()> {
            ensure_service_log_dir()?;
            let executable = current_executable()?;
            validate_windows_install_paths(&executable, &self.options)?;
            preflight_windows_commands(&executable)?;
            let user_sid = codex_helper_core::local_operator::current_windows_user_sid_string()
                .map_err(|error| {
                    CliError::Other(format!("resolve current Windows user SID: {error}"))
                })?;
            let scoped_task_name = windows_task_name_for_sid(&user_sid)?;
            let definition_path = task_definition_path(&self.options.helper_home);
            let definition_document =
                render_windows_task_definition(&executable, &self.options, &user_sid).into_bytes();

            let scoped_snapshot = match query_scheduled_task(&scoped_task_name)? {
                Some(record) => {
                    require_task_owner(&record, &user_sid, "SID-scoped")?;
                    let _ = scheduled_task_requires_end(&record)?;
                    Some(snapshot_owned_task(record)?)
                }
                None => None,
            };
            let fixed_snapshot = match query_scheduled_task(WINDOWS_TASK_BASENAME)? {
                Some(record) if windows_task_owner_matches(&record, &user_sid) => {
                    let _ = scheduled_task_requires_end(&record)?;
                    Some(snapshot_owned_task(record)?)
                }
                Some(_) | None => None,
            };
            let legacy_scm = probe_legacy_scm_for_migration()?;
            let definition_transaction = codex_helper_core::ManagedFileTransaction::begin(
                definition_path.clone(),
                MAX_SERVICE_DEFINITION_BYTES,
            )
            .map_err(|error| {
                CliError::Other(format!("begin Windows definition transaction: {error}"))
            })?;
            let (receipt_transaction, receipt) = begin_service_receipt_transaction(&self.options)?;
            self.context = Some(WindowsInstallContext {
                executable,
                user_sid,
                scoped_task_name,
                definition_path,
                definition_document,
                scoped_snapshot,
                fixed_snapshot,
                legacy_scm,
            });
            self.definition_transaction = Some(definition_transaction);
            self.receipt_transaction = Some(receipt_transaction);
            self.receipt = Some(receipt);
            Ok(())
        }

        fn register_scoped_task(&mut self) -> CliResult<()> {
            let context = self.context()?;
            if let Some(record) = query_scheduled_task(&context.scoped_task_name)? {
                require_task_owner(&record, &context.user_sid, "SID-scoped")?;
            }
            let task_name = context.scoped_task_name.clone();
            let definition_path = context.definition_path.clone();
            let definition_document = context.definition_document.clone();
            self.definition_transaction_mut()?
                .replace(&definition_document)
                .map_err(|error| {
                    CliError::Other(format!("publish Windows task definition: {error}"))
                })?;
            if self.definition_transaction_mut()?.current().bytes()
                != Some(definition_document.as_slice())
            {
                return Err(CliError::Other(format!(
                    "Windows task definition {} failed transaction read-back verification",
                    definition_path.display()
                )));
            }
            self.scoped_task_changed = true;
            register_task_from_file(&task_name, &definition_path)
        }

        fn verify_scoped_task(&mut self) -> CliResult<()> {
            let context = self.context()?;
            let record = query_scheduled_task(&context.scoped_task_name)?.ok_or_else(|| {
                CliError::Other(format!(
                    "the newly registered Windows task '{}' was not found during verification",
                    context.scoped_task_name
                ))
            })?;
            require_task_owner(&record, &context.user_sid, "new SID-scoped")?;
            verify_windows_task_record(
                &record,
                &context.scoped_task_name,
                &context.user_sid,
                &context.executable,
                &self.options,
            )
        }

        fn publish_receipt(&mut self) -> CliResult<()> {
            let receipt = self.receipt.clone().ok_or_else(|| {
                CliError::Other("Windows service receipt is unavailable".to_string())
            })?;
            self.receipt_transaction_mut()?
                .replace(&receipt)
                .map_err(|error| {
                    CliError::Other(format!("publish Windows service receipt: {error}"))
                })
        }

        fn retire_owned_fixed_task(&mut self) -> CliResult<()> {
            let Some(snapshot) = self.context()?.fixed_snapshot.clone() else {
                return Ok(());
            };
            self.fixed_task_changed = true;
            if scheduled_task_requires_end(&snapshot.record)? {
                end_owned_scheduled_task(&snapshot.record.task_name, &snapshot.record.owner_sid)?;
            }
            delete_owned_scheduled_task(&snapshot.record.task_name, &snapshot.record.owner_sid)
        }

        fn retire_legacy_scm(&mut self) -> CliResult<()> {
            let Some(snapshot) = self.context()?.legacy_scm else {
                return Ok(());
            };
            let result = retire_legacy_scm_service(snapshot, &mut self.legacy_scm_stopped);
            if let Err(primary) = result {
                if snapshot.was_running && self.legacy_scm_stopped {
                    if let Err(rollback) = start_legacy_scm_service() {
                        self.preserve_scoped_task = true;
                        return Err(CliError::Other(format!(
                            "{primary}; restarting the legacy SCM service also failed: {rollback}"
                        )));
                    }
                    self.legacy_scm_stopped = false;
                }
                return Err(primary);
            }
            Ok(())
        }

        fn rollback_receipt(&mut self) -> CliResult<()> {
            self.receipt_transaction_mut()?.rollback().map_err(|error| {
                CliError::Other(format!("restore previous Windows service receipt: {error}"))
            })
        }

        fn rollback(&mut self) -> CliResult<()> {
            let mut failures = Vec::new();
            let fixed_snapshot = self.context()?.fixed_snapshot.clone();
            let legacy_snapshot = self.context()?.legacy_scm;
            if self.fixed_task_changed
                && let Some(snapshot) = fixed_snapshot.as_ref()
                && let Err(error) =
                    restore_task_snapshot(snapshot, &self.context()?.definition_path)
            {
                self.preserve_scoped_task = true;
                failures.push(format!("restore the fixed-name task: {error}"));
            }
            if self.legacy_scm_stopped
                && legacy_snapshot.is_some_and(|snapshot| snapshot.was_running)
            {
                match start_legacy_scm_service() {
                    Ok(()) => self.legacy_scm_stopped = false,
                    Err(error) => {
                        self.preserve_scoped_task = true;
                        failures.push(format!("restart the legacy SCM service: {error}"));
                    }
                }
            }
            if self.scoped_task_changed && !self.preserve_scoped_task {
                let context = self.context()?;
                let result = match context.scoped_snapshot.as_ref() {
                    Some(snapshot) => restore_task_snapshot(snapshot, &context.definition_path),
                    None => {
                        delete_owned_scheduled_task(&context.scoped_task_name, &context.user_sid)
                    }
                };
                if let Err(error) = result {
                    failures.push(format!(
                        "restore the previous SID-scoped task state: {error}"
                    ));
                }
            }
            if let Err(error) = self.definition_transaction_mut()?.rollback() {
                failures.push(format!("restore the Windows task definition: {error}"));
            }
            if failures.is_empty() {
                Ok(())
            } else {
                Err(CliError::Other(failures.join("; ")))
            }
        }

        fn start_scoped_task(&mut self) -> CliResult<()> {
            let context = self.context()?;
            let record = query_scheduled_task(&context.scoped_task_name)?.ok_or_else(|| {
                CliError::Other(format!(
                    "the verified Windows task '{}' disappeared before start",
                    context.scoped_task_name
                ))
            })?;
            require_task_owner(&record, &context.user_sid, "SID-scoped")?;
            run_scheduled_task(&record.task_name)
        }
    }

    pub(super) fn install(options: ServiceInstallOptions) -> CliResult<()> {
        let start = options.start;
        run_windows_install_transaction(&mut NativeWindowsInstallBackend::new(options), start)
    }

    pub(super) fn uninstall(stop_first: bool) -> CliResult<()> {
        let (user_sid, scoped_task_name) = current_task_identity()?;
        if let Some(record) = query_scheduled_task(&scoped_task_name)? {
            require_task_owner(&record, &user_sid, "SID-scoped")?;
            if stop_first && scheduled_task_requires_end(&record)? {
                end_owned_scheduled_task(&record.task_name, &user_sid)?;
            }
            delete_owned_scheduled_task(&record.task_name, &user_sid)?;
        }
        if let Some(record) = query_scheduled_task(WINDOWS_TASK_BASENAME)?
            && windows_task_owner_matches(&record, &user_sid)
        {
            if stop_first && scheduled_task_requires_end(&record)? {
                end_owned_scheduled_task(&record.task_name, &user_sid)?;
            }
            delete_owned_scheduled_task(&record.task_name, &user_sid)?;
        }
        let definition = task_definition_path(&proxy_home_dir());
        match std::fs::remove_file(&definition) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(CliError::Other(format!(
                    "remove scheduled-task definition {}: {error}",
                    definition.display()
                )));
            }
        }
        remove_legacy_scm_service(stop_first)
    }

    pub(super) fn start() -> CliResult<()> {
        let (user_sid, scoped_task_name) = current_task_identity()?;
        if let Some(record) = current_user_task(&user_sid, &scoped_task_name)? {
            return run_scheduled_task(&record.task_name);
        }
        Err(CliError::Other(
            "the current user's Windows task is not installed; run `codex-helper service install` to migrate any legacy SCM service"
                .to_string(),
        ))
    }

    pub(super) fn stop() -> CliResult<()> {
        let (user_sid, scoped_task_name) = current_task_identity()?;
        if let Some(record) = current_user_task(&user_sid, &scoped_task_name)? {
            return if scheduled_task_requires_end(&record)? {
                end_owned_scheduled_task(&record.task_name, &user_sid)
            } else {
                Ok(())
            };
        }
        stop_legacy_scm_service()
    }

    pub(super) fn status() -> CliResult<ServiceStatus> {
        let (user_sid, scoped_task_name) = current_task_identity()?;
        if let Some(record) = current_user_task(&user_sid, &scoped_task_name)? {
            let state = match record.state {
                4 => ServiceRuntimeState::Running,
                2 => ServiceRuntimeState::Starting,
                1 | 3 => ServiceRuntimeState::Stopped,
                _ => ServiceRuntimeState::Unknown,
            };
            let fixed_name = record.task_name == WINDOWS_TASK_BASENAME;
            let mut status = base_status(state, true, record.enabled);
            status.service_name.clone_from(&record.task_name);
            status.service_definition = Some(task_definition_path(&proxy_home_dir()));
            status.detail = Some(if fixed_name {
                format!(
                    "legacy fixed-name per-user scheduled task owned by the current SID; run `codex-helper service install` to migrate; task_state_code={}",
                    record.state
                )
            } else {
                format!(
                    "SID-scoped per-user scheduled task (interactive token, least privilege); task_state_code={}",
                    record.state
                )
            });
            return Ok(status);
        }
        legacy_scm_status()
    }

    fn legacy_scm_status() -> CliResult<ServiceStatus> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(windows_error("open Windows Service Control Manager"))?;
        let service = match manager.open_service(
            WINDOWS_SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG,
        ) {
            Ok(service) => service,
            Err(error) if windows_service_missing(&error) => {
                return Ok(base_status(ServiceRuntimeState::NotInstalled, false, false));
            }
            Err(error) => return Err(windows_error("open codex-helper Windows service")(error)),
        };
        let raw = service
            .query_status()
            .map_err(windows_error("query Windows service status"))?;
        let config = service
            .query_config()
            .map_err(windows_error("query Windows service config"))?;
        let state = match raw.current_state {
            ServiceState::Running => ServiceRuntimeState::Running,
            ServiceState::Stopped => ServiceRuntimeState::Stopped,
            ServiceState::StartPending => ServiceRuntimeState::Starting,
            ServiceState::StopPending => ServiceRuntimeState::Stopping,
            _ => ServiceRuntimeState::Unknown,
        };
        let mut status = base_status(
            state,
            true,
            config.start_type == ServiceStartType::AutoStart,
        );
        status.detail = Some(match raw.process_id {
            Some(pid) => format!(
                "legacy LocalSystem SCM service, pid={pid}; run `codex-helper service install` to migrate"
            ),
            None => "legacy LocalSystem SCM service; run `codex-helper service install` to migrate"
                .to_string(),
        });
        Ok(status)
    }

    fn current_task_identity() -> CliResult<(String, String)> {
        let user_sid = codex_helper_core::local_operator::current_windows_user_sid_string()
            .map_err(|error| {
                CliError::Other(format!("resolve current Windows user SID: {error}"))
            })?;
        let task_name = windows_task_name_for_sid(&user_sid)?;
        Ok((user_sid, task_name))
    }

    fn current_user_task(
        user_sid: &str,
        scoped_task_name: &str,
    ) -> CliResult<Option<WindowsTaskRecord>> {
        if let Some(record) = query_scheduled_task(scoped_task_name)? {
            require_task_owner(&record, user_sid, "SID-scoped")?;
            return Ok(Some(record));
        }
        match query_scheduled_task(WINDOWS_TASK_BASENAME)? {
            Some(record) if windows_task_owner_matches(&record, user_sid) => Ok(Some(record)),
            Some(_) | None => Ok(None),
        }
    }

    fn scheduled_task_requires_end(record: &WindowsTaskRecord) -> CliResult<bool> {
        match record.state {
            2 | 4 => Ok(true),
            1 | 3 => Ok(false),
            state => Err(CliError::Other(format!(
                "refusing to mutate Windows task '{}' while Task Scheduler reports unknown state code {state}",
                record.task_name
            ))),
        }
    }

    fn task_definition_path(helper_home: &Path) -> PathBuf {
        helper_home
            .join("service")
            .join(WINDOWS_TASK_DEFINITION_FILE)
    }

    fn validate_windows_install_paths(
        executable: &Path,
        options: &ServiceInstallOptions,
    ) -> CliResult<()> {
        for (path, description) in [
            (executable, "the codex-helper executable path"),
            (&options.helper_home, "the codex-helper home path"),
            (&options.client_home, "the client home path"),
        ] {
            if !path.is_absolute() {
                return Err(CliError::Other(format!(
                    "{description} must be absolute before installing a Windows task: {}",
                    path.display()
                )));
            }
            let _ = windows_path_text(path, description)?;
        }
        let metadata = std::fs::metadata(executable).map_err(|error| {
            CliError::Other(format!(
                "inspect codex-helper executable {}: {error}",
                executable.display()
            ))
        })?;
        if !metadata.is_file() {
            return Err(CliError::Other(format!(
                "the Windows task executable is not a file: {}",
                executable.display()
            )));
        }
        Ok(())
    }

    fn preflight_windows_commands(executable: &Path) -> CliResult<()> {
        powershell_command(
            "$ErrorActionPreference = 'Stop'; Get-Command schtasks.exe -ErrorAction Stop | Out-Null; Get-Command Get-ScheduledTask -ErrorAction Stop | Out-Null; Get-Command Export-ScheduledTask -ErrorAction Stop | Out-Null; [Console]::Out.Write('ok')",
        )?;
        let output = Command::new(executable)
            .args(["service", "task-run", "--help"])
            .output()
            .map_err(|error| {
                CliError::Other(format!(
                    "run the scheduled-task command preflight with {}: {error}",
                    executable.display()
                ))
            })?;
        if !output.status.success() {
            return Err(CliError::Other(format!(
                "the current executable does not accept `service task-run --help`: {}",
                output.status
            )));
        }
        Ok(())
    }

    fn powershell_command(script: &str) -> CliResult<String> {
        let script = format!(
            "$OutputEncoding = [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false); {script}"
        );
        run_command(
            "powershell.exe",
            &[
                OsString::from("-NoLogo"),
                OsString::from("-NoProfile"),
                OsString::from("-NonInteractive"),
                OsString::from("-Command"),
                OsString::from(script),
            ],
        )
    }

    fn powershell_single_quote(value: &str) -> String {
        value.replace('\'', "''")
    }

    fn query_scheduled_task(task_name: &str) -> CliResult<Option<WindowsTaskRecord>> {
        let task_name = powershell_single_quote(task_name);
        let script = r#"
$ErrorActionPreference = 'Stop'
$target = '__TASK_NAME__'
$tasks = @(Get-ScheduledTask -TaskPath '\' -ErrorAction Stop | Where-Object { $_.TaskName -ceq $target })
if ($tasks.Count -eq 0) {
    [Console]::Out.Write(([ordered]@{ found = $false } | ConvertTo-Json -Compress))
    exit 0
}
if ($tasks.Count -ne 1) { throw "Scheduled Task lookup returned more than one exact root task" }
function Resolve-Sid([string] $identity) {
    try { return ([System.Security.Principal.SecurityIdentifier] $identity).Value } catch {}
    return ([System.Security.Principal.NTAccount] $identity).Translate([System.Security.Principal.SecurityIdentifier]).Value
}
$task = $tasks[0]
$actions = @($task.Actions)
$triggers = @($task.Triggers)
$ownerSid = Resolve-Sid ([string] $task.Principal.UserId)
$triggerSid = ''
$triggerType = ''
$triggerEnabled = $false
if ($triggers.Count -eq 1) {
    $triggerType = [string] $triggers[0].CimClass.CimClassName
    $triggerEnabled = [bool] $triggers[0].Enabled
    if (-not [string]::IsNullOrWhiteSpace([string] $triggers[0].UserId)) {
        $triggerSid = Resolve-Sid ([string] $triggers[0].UserId)
    }
}
$action = if ($actions.Count -eq 1) { $actions[0] } else { $null }
$execute = ''
$arguments = ''
$workingDirectory = ''
if ($null -ne $action) {
    $execute = [string] $action.Execute
    $arguments = [string] $action.Arguments
    $workingDirectory = [string] $action.WorkingDirectory
}
$record = [ordered]@{
    task_name = [string] $task.TaskName
    task_path = [string] $task.TaskPath
    owner_sid = [string] $ownerSid
    state = [int] $task.State
    enabled = [bool] $task.Settings.Enabled
    action_count = [int] $actions.Count
    execute = $execute
    arguments = $arguments
    working_directory = $workingDirectory
    logon_type = [string] $task.Principal.LogonType
    run_level = [string] $task.Principal.RunLevel
    trigger_count = [int] $triggers.Count
    trigger_enabled = [bool] $triggerEnabled
    trigger_type = [string] $triggerType
    trigger_user_sid = [string] $triggerSid
}
[Console]::Out.Write(([ordered]@{ found = $true; record = $record } | ConvertTo-Json -Depth 4 -Compress))
"#
        .replace("__TASK_NAME__", &task_name);
        parse_windows_task_probe(&powershell_command(&script)?)
    }

    fn require_task_owner(
        record: &WindowsTaskRecord,
        expected_sid: &str,
        description: &str,
    ) -> CliResult<()> {
        if windows_task_owner_matches(record, expected_sid) {
            Ok(())
        } else {
            Err(CliError::Other(format!(
                "refusing to alter {description} Windows task '{}': its Principal SID does not match the current process SID",
                record.task_name
            )))
        }
    }

    fn snapshot_owned_task(record: WindowsTaskRecord) -> CliResult<OwnedTaskSnapshot> {
        use base64::Engine;

        let task_name = powershell_single_quote(&record.task_name);
        let script = r#"
$ErrorActionPreference = 'Stop'
$source = Export-ScheduledTask -TaskName '__TASK_NAME__' -TaskPath '\' -ErrorAction Stop
$document = New-Object System.Xml.XmlDocument
$document.PreserveWhitespace = $true
$document.LoadXml($source)
$stream = New-Object System.IO.MemoryStream
$settings = New-Object System.Xml.XmlWriterSettings
$settings.Encoding = New-Object System.Text.UTF8Encoding($false)
$settings.Indent = $false
$writer = [System.Xml.XmlWriter]::Create($stream, $settings)
try {
    $document.Save($writer)
    $writer.Flush()
    [Console]::Out.Write([Convert]::ToBase64String($stream.ToArray()))
} finally {
    $writer.Dispose()
    $stream.Dispose()
}
"#
        .replace("__TASK_NAME__", &task_name);
        let encoded = powershell_command(&script)?;
        let definition = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .map_err(|error| {
                CliError::Other(format!(
                    "decode exported Windows task '{}' as UTF-8 XML bytes: {error}",
                    record.task_name
                ))
            })?;
        let definition_text = std::str::from_utf8(&definition).map_err(|error| {
            CliError::Other(format!(
                "exported Windows task '{}' was not normalized to UTF-8 XML: {error}",
                record.task_name
            ))
        })?;
        if definition_text.trim().is_empty() || !definition_text.contains("<Task") {
            return Err(CliError::Other(format!(
                "exported Windows task '{}' had an invalid normalized definition",
                record.task_name
            )));
        }
        Ok(OwnedTaskSnapshot { record, definition })
    }

    fn register_task_from_file(task_name: &str, definition: &Path) -> CliResult<()> {
        run_command(
            "schtasks.exe",
            &[
                OsString::from("/Create"),
                OsString::from("/TN"),
                OsString::from(task_name),
                OsString::from("/XML"),
                definition.as_os_str().to_os_string(),
                OsString::from("/F"),
            ],
        )
        .map(|_| ())
    }

    fn restore_task_snapshot(
        snapshot: &OwnedTaskSnapshot,
        installed_definition: &Path,
    ) -> CliResult<()> {
        if let Some(current) = query_scheduled_task(&snapshot.record.task_name)? {
            require_task_owner(&current, &snapshot.record.owner_sid, "rollback destination")?;
        }
        let rollback_path = installed_definition.with_file_name(format!(
            "windows-task-rollback-{}-{}.xml",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        write_service_definition(&rollback_path, &snapshot.definition)?;
        let restore = register_task_from_file(&snapshot.record.task_name, &rollback_path);
        let cleanup = match std::fs::remove_file(&rollback_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(CliError::Other(format!(
                "remove rollback task definition {}: {error}",
                rollback_path.display()
            ))),
        };
        restore?;
        let restored = query_scheduled_task(&snapshot.record.task_name)?.ok_or_else(|| {
            CliError::Other(format!(
                "restored Windows task '{}' could not be queried",
                snapshot.record.task_name
            ))
        })?;
        require_task_owner(&restored, &snapshot.record.owner_sid, "restored")?;
        if scheduled_task_requires_end(&snapshot.record)?
            && !scheduled_task_requires_end(&restored)?
        {
            run_scheduled_task(&restored.task_name)?;
        }
        cleanup
    }

    fn run_scheduled_task(task_name: &str) -> CliResult<()> {
        run_command(
            "schtasks.exe",
            &[
                OsString::from("/Run"),
                OsString::from("/TN"),
                OsString::from(task_name),
            ],
        )
        .map(|_| ())
    }

    fn end_owned_scheduled_task(task_name: &str, expected_sid: &str) -> CliResult<()> {
        let Some(record) = query_scheduled_task(task_name)? else {
            return Ok(());
        };
        require_task_owner(&record, expected_sid, "owned")?;
        run_command(
            "schtasks.exe",
            &[
                OsString::from("/End"),
                OsString::from("/TN"),
                OsString::from(task_name),
            ],
        )
        .map(|_| ())
    }

    fn delete_owned_scheduled_task(task_name: &str, expected_sid: &str) -> CliResult<()> {
        let Some(record) = query_scheduled_task(task_name)? else {
            return Ok(());
        };
        require_task_owner(&record, expected_sid, "owned")?;
        run_command(
            "schtasks.exe",
            &[
                OsString::from("/Delete"),
                OsString::from("/TN"),
                OsString::from(task_name),
                OsString::from("/F"),
            ],
        )
        .map(|_| ())
    }

    fn probe_legacy_scm_for_migration() -> CliResult<Option<LegacyScmSnapshot>> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(windows_error("open Windows Service Control Manager"))?;
        let access = ServiceAccess::QUERY_STATUS
            | ServiceAccess::START
            | ServiceAccess::STOP
            | ServiceAccess::DELETE;
        let service = match manager.open_service(WINDOWS_SERVICE_NAME, access) {
            Ok(service) => service,
            Err(error) if windows_service_missing(&error) => return Ok(None),
            Err(error) => {
                return Err(CliError::Other(format!(
                    "preflight legacy LocalSystem SCM service migration: {error}; rerun once from an elevated terminal"
                )));
            }
        };
        let state = service
            .query_status()
            .map_err(windows_error(
                "query legacy Windows service status during preflight",
            ))?
            .current_state;
        Ok(Some(LegacyScmSnapshot {
            was_running: state != ServiceState::Stopped,
        }))
    }

    fn stop_legacy_scm_service() -> CliResult<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(windows_error("open Windows Service Control Manager"))?;
        let service = match manager.open_service(
            WINDOWS_SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP,
        ) {
            Ok(service) => service,
            Err(error) if windows_service_missing(&error) => return Ok(()),
            Err(error) => return Err(windows_error("open legacy codex-helper SCM service")(error)),
        };
        if service
            .query_status()
            .map_err(windows_error("query legacy Windows service status"))?
            .current_state
            == ServiceState::Stopped
        {
            return Ok(());
        }
        service
            .stop()
            .map_err(windows_error("stop legacy codex-helper Windows service"))?;
        wait_for_legacy_service_stop(&service)
    }

    fn start_legacy_scm_service() -> CliResult<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(windows_error("open Windows Service Control Manager"))?;
        let service = match manager.open_service(
            WINDOWS_SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::START,
        ) {
            Ok(service) => service,
            Err(error) if windows_service_missing(&error) => {
                return Err(CliError::Other(
                    "the legacy SCM service disappeared before rollback".to_string(),
                ));
            }
            Err(error) => return Err(windows_error("open legacy SCM service for rollback")(error)),
        };
        if service
            .query_status()
            .map_err(windows_error("query legacy SCM service during rollback"))?
            .current_state
            == ServiceState::Running
        {
            return Ok(());
        }
        service
            .start::<&OsStr>(&[])
            .map_err(windows_error("restart legacy codex-helper Windows service"))
    }

    fn retire_legacy_scm_service(snapshot: LegacyScmSnapshot, stopped: &mut bool) -> CliResult<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(windows_error("open Windows Service Control Manager"))?;
        let service = match manager.open_service(
            WINDOWS_SERVICE_NAME,
            ServiceAccess::QUERY_STATUS
                | ServiceAccess::START
                | ServiceAccess::STOP
                | ServiceAccess::DELETE,
        ) {
            Ok(service) => service,
            Err(error) if windows_service_missing(&error) => return Ok(()),
            Err(error) => {
                return Err(windows_error("open legacy SCM service for migration")(
                    error,
                ));
            }
        };
        *stopped = snapshot.was_running;
        if service
            .query_status()
            .map_err(windows_error("query legacy Windows service status"))?
            .current_state
            != ServiceState::Stopped
        {
            service
                .stop()
                .map_err(windows_error("stop legacy codex-helper Windows service"))?;
            wait_for_legacy_service_stop(&service)?;
        }
        service
            .delete()
            .map_err(windows_error("delete legacy codex-helper Windows service"))
    }

    fn remove_legacy_scm_service(stop_first: bool) -> CliResult<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(windows_error("open Windows Service Control Manager"))?;
        let service = match manager.open_service(
            WINDOWS_SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
        ) {
            Ok(service) => service,
            Err(error) if windows_service_missing(&error) => return Ok(()),
            Err(error) => {
                return Err(CliError::Other(format!(
                    "remove legacy LocalSystem SCM service before installing the per-user task: {error}; rerun once from an elevated terminal"
                )));
            }
        };
        if stop_first
            && service
                .query_status()
                .map_err(windows_error("query legacy Windows service status"))?
                .current_state
                != ServiceState::Stopped
        {
            service
                .stop()
                .map_err(windows_error("stop legacy codex-helper Windows service"))?;
            wait_for_legacy_service_stop(&service)?;
        }
        service
            .delete()
            .map_err(windows_error("delete legacy codex-helper Windows service"))
    }

    fn wait_for_legacy_service_stop(service: &windows_service::service::Service) -> CliResult<()> {
        for _ in 0..40 {
            if service
                .query_status()
                .map_err(windows_error("query legacy Windows service status"))?
                .current_state
                == ServiceState::Stopped
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(CliError::Other(
            "legacy codex-helper Windows service did not stop within 10 seconds".to_string(),
        ))
    }

    fn windows_service_missing(error: &windows_service::Error) -> bool {
        let raw_os_error = match error {
            windows_service::Error::Winapi(error) => error.raw_os_error(),
            _ => None,
        };
        classify_windows_service_probe_error(raw_os_error)
            == WindowsServiceProbeClassification::Missing
    }

    pub(super) fn run_dispatcher(options: ServiceInstallOptions) -> CliResult<()> {
        SERVICE_OPTIONS.set(options).map_err(|_| {
            CliError::Other("Windows service options were initialized twice".to_string())
        })?;
        service_dispatcher::start(WINDOWS_SERVICE_NAME, service_entry)
            .map_err(windows_error("start Windows service dispatcher"))
    }

    fn service_main(arguments: Vec<OsString>) {
        if let Err(error) = run_service(arguments) {
            tracing::error!(error = %error, "Windows service stopped with an error");
        }
    }

    fn run_service(_arguments: Vec<OsString>) -> CliResult<()> {
        let options = SERVICE_OPTIONS.get().cloned().ok_or_else(|| {
            CliError::Other("Windows service options are unavailable".to_string())
        })?;
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let event_handler = move |control| match control {
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = shutdown_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        };
        let status_handle = service_control_handler::register(WINDOWS_SERVICE_NAME, event_handler)
            .map_err(windows_error("register Windows service control handler"))?;
        report_status(&status_handle, ServiceState::StartPending, false)?;
        let executable = current_executable()?;
        let service_flag = if options.service_name == "claude" {
            "--claude"
        } else {
            "--codex"
        };
        let child_args = vec![
            OsString::from("serve"),
            OsString::from(service_flag),
            OsString::from("--host"),
            OsString::from(options.host.to_string()),
            OsString::from("--port"),
            OsString::from(options.port.to_string()),
            OsString::from("--no-tui"),
            OsString::from("--service-managed"),
        ];
        let mut child = Command::new(executable)
            .args(child_args)
            .spawn()
            .map_err(|error| CliError::Other(format!("start service relay child: {error}")))?;
        report_status(&status_handle, ServiceState::Running, true)?;
        loop {
            if shutdown_rx.recv_timeout(Duration::from_millis(250)).is_ok() {
                let _ = child.kill();
                let _ = child.wait();
                report_status(&status_handle, ServiceState::Stopped, false)?;
                return Ok(());
            }
            if let Some(status) = child.try_wait().map_err(|error| {
                CliError::Other(format!("wait for service relay child: {error}"))
            })? {
                report_status(&status_handle, ServiceState::Stopped, false)?;
                return Err(CliError::Other(format!(
                    "service relay child exited unexpectedly: {status}"
                )));
            }
        }
    }

    fn report_status(
        handle: &ServiceStatusHandle,
        current_state: ServiceState,
        accepts_stop: bool,
    ) -> CliResult<()> {
        handle
            .set_service_status(WindowsStatus {
                service_type: SERVICE_TYPE,
                current_state,
                controls_accepted: if accepts_stop {
                    ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
                } else {
                    ServiceControlAccept::empty()
                },
                exit_code: ServiceExitCode::Win32(0),
                checkpoint: 0,
                wait_hint: Duration::from_secs(10),
                process_id: None,
            })
            .map_err(windows_error("report Windows service status"))
    }

    fn windows_error(action: &'static str) -> impl FnOnce(windows_service::Error) -> CliError {
        move |error| CliError::Other(format!("{action}: {error}"))
    }

    fn base_status(state: ServiceRuntimeState, installed: bool, autostart: bool) -> ServiceStatus {
        ServiceStatus {
            platform: ServicePlatform::current(),
            service_name: WINDOWS_SERVICE_NAME.to_string(),
            state,
            installed,
            autostart,
            service_definition: None,
            log_directory: service_log_dir(),
            detail: None,
            receipt_state: ServiceReceiptState::Absent,
            credential_context: ServiceCredentialContext::Unverified,
            runtime_identity_verified: false,
            install_generation: None,
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    pub(super) fn install(options: ServiceInstallOptions) -> CliResult<()> {
        let executable = current_executable()?;
        let log_dir = ensure_service_log_dir()?;
        let path = launch_agent_path()?;
        let document = render_launch_agent(&executable, &log_dir, &options);
        let domain = launchd_domain()?;
        let target = format!("{}/{MACOS_LABEL}", launchd_domain_string()?);
        let was_loaded = run_command(
            "launchctl",
            &[OsString::from("print"), OsString::from(target)],
        )
        .is_ok();
        let mut definition = codex_helper_core::ManagedFileTransaction::begin(
            path.clone(),
            MAX_SERVICE_DEFINITION_BYTES,
        )
        .map_err(|error| CliError::Other(format!("begin LaunchAgent transaction: {error}")))?;
        let original_definition_exists = definition.current().bytes().is_some();
        let (mut receipt_transaction, receipt) = begin_service_receipt_transaction(&options)?;
        let mutation = (|| {
            if was_loaded {
                run_command(
                    "launchctl",
                    &[
                        OsString::from("bootout"),
                        domain.clone(),
                        path.clone().into_os_string(),
                    ],
                )?;
            }
            definition.replace(document.as_bytes()).map_err(|error| {
                CliError::Other(format!("publish LaunchAgent definition: {error}"))
            })?;
            if definition.current().bytes() != Some(document.as_bytes()) {
                return Err(CliError::Other(
                    "LaunchAgent definition failed transaction read-back verification".to_string(),
                ));
            }
            run_command(
                "plutil",
                &[OsString::from("-lint"), path.clone().into_os_string()],
            )?;
            receipt_transaction.replace(&receipt).map_err(|error| {
                CliError::Other(format!("publish LaunchAgent service receipt: {error}"))
            })
        })();
        if let Err(primary) = mutation {
            let mut failures = Vec::new();
            if let Err(error) = receipt_transaction.rollback() {
                failures.push(format!("restore previous service receipt: {error}"));
            }
            if let Err(error) = definition.rollback() {
                failures.push(format!("restore previous LaunchAgent definition: {error}"));
            }
            if was_loaded
                && original_definition_exists
                && let Err(error) = run_command(
                    "launchctl",
                    &[OsString::from("bootstrap"), domain, path.into_os_string()],
                )
            {
                failures.push(format!("reload previous LaunchAgent: {error}"));
            }
            return Err(rollback_error(primary, failures));
        }
        if options.start {
            start()?;
        }
        Ok(())
    }

    pub(super) fn uninstall(stop_first: bool) -> CliResult<()> {
        let path = launch_agent_path()?;
        if stop_first && path.exists() {
            let _ = run_command(
                "launchctl",
                &[
                    OsString::from("bootout"),
                    launchd_domain()?,
                    path.clone().into_os_string(),
                ],
            );
        }
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(CliError::Other(format!(
                "remove launch agent {}: {error}",
                path.display()
            ))),
        }
    }

    pub(super) fn start() -> CliResult<()> {
        let target = format!("{}/{MACOS_LABEL}", launchd_domain_string()?);
        if run_command(
            "launchctl",
            &[OsString::from("print"), OsString::from(&target)],
        )
        .is_err()
        {
            return run_command(
                "launchctl",
                &[
                    OsString::from("bootstrap"),
                    launchd_domain()?,
                    launch_agent_path()?.into_os_string(),
                ],
            )
            .map(|_| ());
        }
        run_command(
            "launchctl",
            &[
                OsString::from("kickstart"),
                OsString::from("-k"),
                OsString::from(target),
            ],
        )
        .map(|_| ())
    }

    pub(super) fn stop() -> CliResult<()> {
        run_command(
            "launchctl",
            &[
                OsString::from("kill"),
                OsString::from("SIGTERM"),
                OsString::from(format!("{}/{MACOS_LABEL}", launchd_domain_string()?)),
            ],
        )
        .map(|_| ())
    }

    pub(super) fn status() -> CliResult<ServiceStatus> {
        let path = launch_agent_path()?;
        if !path.exists() {
            return Ok(base_status(
                ServiceRuntimeState::NotInstalled,
                false,
                false,
                Some(path),
            ));
        }
        let target = format!("{}/{MACOS_LABEL}", launchd_domain_string()?);
        let output = run_command(
            "launchctl",
            &[OsString::from("print"), OsString::from(target)],
        );
        let (state, detail) = match output {
            Ok(output) if output.contains("state = running") => {
                (ServiceRuntimeState::Running, Some(output))
            }
            Ok(output) => (ServiceRuntimeState::Stopped, Some(output)),
            Err(error) => (ServiceRuntimeState::Installed, Some(error.to_string())),
        };
        let mut status = base_status(state, true, true, Some(path));
        status.detail = detail;
        Ok(status)
    }

    fn launch_agent_path() -> CliResult<PathBuf> {
        let home = dirs::home_dir()
            .ok_or_else(|| CliError::Other("resolve home directory".to_string()))?;
        Ok(home
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{MACOS_LABEL}.plist")))
    }

    fn launchd_domain_string() -> CliResult<String> {
        let output = run_command("id", &[OsString::from("-u")])?;
        if output.is_empty() || !output.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(CliError::Other("resolve launchd GUI user id".to_string()));
        }
        Ok(format!("gui/{output}"))
    }

    fn launchd_domain() -> CliResult<OsString> {
        launchd_domain_string().map(OsString::from)
    }

    pub(super) fn render_launch_agent(
        executable: &Path,
        log_dir: &Path,
        options: &ServiceInstallOptions,
    ) -> String {
        let service_flag = if options.service_name == "claude" {
            "--claude"
        } else {
            "--codex"
        };
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{MACOS_LABEL}</string>\n<key>ProgramArguments</key><array><string>{}</string><string>serve</string><string>{service_flag}</string><string>--host</string><string>{}</string><string>--port</string><string>{}</string><string>--no-tui</string><string>--service-managed</string></array>\n<key>EnvironmentVariables</key><dict><key>CODEX_HELPER_HOME</key><string>{}</string><key>{}</key><string>{}</string><key>{}</key><string>{}</string></dict>\n<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>\n<key>ThrottleInterval</key><integer>10</integer>\n<key>StandardOutPath</key><string>{}</string>\n<key>StandardErrorPath</key><string>{}</string>\n</dict></plist>\n",
            xml_escape(executable.to_string_lossy().as_ref()),
            options.host,
            options.port,
            xml_escape(options.helper_home.to_string_lossy().as_ref()),
            service_client_home_env(options.service_name),
            xml_escape(options.client_home.to_string_lossy().as_ref()),
            codex_helper_core::service_target::SERVICE_INSTALL_GENERATION_ENV_VAR,
            options.install_generation,
            xml_escape(
                log_dir
                    .join("service.stdout.log")
                    .to_string_lossy()
                    .as_ref()
            ),
            xml_escape(
                log_dir
                    .join("service.stderr.log")
                    .to_string_lossy()
                    .as_ref()
            ),
        )
    }

    fn base_status(
        state: ServiceRuntimeState,
        installed: bool,
        autostart: bool,
        definition: Option<PathBuf>,
    ) -> ServiceStatus {
        ServiceStatus {
            platform: ServicePlatform::current(),
            service_name: MACOS_LABEL.to_string(),
            state,
            installed,
            autostart,
            service_definition: definition,
            log_directory: service_log_dir(),
            detail: None,
            receipt_state: ServiceReceiptState::Absent,
            credential_context: ServiceCredentialContext::Unverified,
            runtime_identity_verified: false,
            install_generation: None,
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    pub(super) fn install(options: ServiceInstallOptions) -> CliResult<()> {
        let executable = current_executable()?;
        ensure_service_log_dir()?;
        systemctl(&["show-environment"])?;
        let path = user_unit_path()?;
        let document = render_systemd_unit(&executable, &options);
        let was_active =
            systemctl_output(&["is-active", LINUX_UNIT_NAME]).is_ok_and(|value| value == "active");
        let was_enabled = systemctl_output(&["is-enabled", LINUX_UNIT_NAME])
            .is_ok_and(|value| value == "enabled");
        let mut definition =
            codex_helper_core::ManagedFileTransaction::begin(path, MAX_SERVICE_DEFINITION_BYTES)
                .map_err(|error| {
                    CliError::Other(format!("begin systemd unit transaction: {error}"))
                })?;
        let (mut receipt_transaction, receipt) = begin_service_receipt_transaction(&options)?;
        let mutation = (|| {
            if was_active {
                systemctl(&["stop", LINUX_UNIT_NAME])?;
            }
            definition
                .replace(document.as_bytes())
                .map_err(|error| CliError::Other(format!("publish systemd user unit: {error}")))?;
            if definition.current().bytes() != Some(document.as_bytes()) {
                return Err(CliError::Other(
                    "systemd user unit failed transaction read-back verification".to_string(),
                ));
            }
            systemctl(&["daemon-reload"])?;
            systemctl(&["enable", LINUX_UNIT_NAME])?;
            if !matches!(
                systemctl_output(&["is-enabled", LINUX_UNIT_NAME]).as_deref(),
                Ok("enabled")
            ) {
                return Err(CliError::Other(
                    "systemd user unit did not report enabled after installation".to_string(),
                ));
            }
            receipt_transaction.replace(&receipt).map_err(|error| {
                CliError::Other(format!("publish systemd service receipt: {error}"))
            })
        })();
        if let Err(primary) = mutation {
            let mut failures = Vec::new();
            if let Err(error) = receipt_transaction.rollback() {
                failures.push(format!("restore previous service receipt: {error}"));
            }
            if let Err(error) = definition.rollback() {
                failures.push(format!("restore previous systemd user unit: {error}"));
            }
            if let Err(error) = systemctl(&["daemon-reload"]) {
                failures.push(format!("reload restored systemd user units: {error}"));
            }
            let restore_enablement = if was_enabled {
                systemctl(&["enable", LINUX_UNIT_NAME])
            } else {
                systemctl(&["disable", LINUX_UNIT_NAME])
            };
            if let Err(error) = restore_enablement {
                failures.push(format!("restore systemd user unit enablement: {error}"));
            }
            if was_active && let Err(error) = systemctl(&["start", LINUX_UNIT_NAME]) {
                failures.push(format!("restart previous systemd user unit: {error}"));
            }
            return Err(rollback_error(primary, failures));
        }
        if options.start {
            systemctl(&["start", LINUX_UNIT_NAME])?;
        }
        Ok(())
    }

    pub(super) fn uninstall(stop_first: bool) -> CliResult<()> {
        if stop_first {
            let _ = systemctl(&["stop", LINUX_UNIT_NAME]);
        }
        let _ = systemctl(&["disable", LINUX_UNIT_NAME]);
        let path = user_unit_path()?;
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(CliError::Other(format!(
                    "remove systemd user unit {}: {error}",
                    path.display()
                )));
            }
        }
        systemctl(&["daemon-reload"])
    }

    pub(super) fn start() -> CliResult<()> {
        systemctl(&["start", LINUX_UNIT_NAME])
    }

    pub(super) fn stop() -> CliResult<()> {
        systemctl(&["stop", LINUX_UNIT_NAME])
    }

    pub(super) fn status() -> CliResult<ServiceStatus> {
        let path = user_unit_path()?;
        if !path.exists() {
            return Ok(base_status(
                ServiceRuntimeState::NotInstalled,
                false,
                false,
                Some(path),
            ));
        }
        let active = systemctl_output(&["is-active", LINUX_UNIT_NAME]);
        let enabled = systemctl_output(&["is-enabled", LINUX_UNIT_NAME]);
        let state = match active.as_deref() {
            Ok("active") => ServiceRuntimeState::Running,
            Ok("activating") => ServiceRuntimeState::Starting,
            Ok("deactivating") => ServiceRuntimeState::Stopping,
            Ok(_) => ServiceRuntimeState::Stopped,
            Err(_) => ServiceRuntimeState::Unknown,
        };
        let mut status = base_status(
            state,
            true,
            enabled.as_deref().is_ok_and(|value| value == "enabled"),
            Some(path),
        );
        status.detail = active.err().map(|error| error.to_string());
        Ok(status)
    }

    fn systemctl(arguments: &[&str]) -> CliResult<()> {
        systemctl_output(arguments).map(|_| ())
    }

    fn systemctl_output(arguments: &[&str]) -> CliResult<String> {
        let mut args = vec![OsString::from("--user")];
        args.extend(arguments.iter().map(OsString::from));
        run_command("systemctl", &args)
    }

    fn user_unit_path() -> CliResult<PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| CliError::Other("resolve user config directory".to_string()))?;
        Ok(config_dir
            .join("systemd")
            .join("user")
            .join(LINUX_UNIT_NAME))
    }

    fn base_status(
        state: ServiceRuntimeState,
        installed: bool,
        autostart: bool,
        definition: Option<PathBuf>,
    ) -> ServiceStatus {
        ServiceStatus {
            platform: ServicePlatform::current(),
            service_name: LINUX_UNIT_NAME.to_string(),
            state,
            installed,
            autostart,
            service_definition: definition,
            log_directory: service_log_dir(),
            detail: None,
            receipt_state: ServiceReceiptState::Absent,
            credential_context: ServiceCredentialContext::Unverified,
            runtime_identity_verified: false,
            install_generation: None,
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn render_systemd_unit(executable: &Path, options: &ServiceInstallOptions) -> String {
    let service_flag = if options.service_name == "claude" {
        "--claude"
    } else {
        "--codex"
    };
    let helper_home = systemd_environment_assignment("CODEX_HELPER_HOME", &options.helper_home);
    let client_home = systemd_environment_assignment(
        service_client_home_env(options.service_name),
        &options.client_home,
    );
    let install_generation = format!(
        "\"{}={}\"",
        codex_helper_core::service_target::SERVICE_INSTALL_GENERATION_ENV_VAR,
        options.install_generation
    );
    format!(
        "[Unit]\nDescription=codex-helper resident relay\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nEnvironment={helper_home}\nEnvironment={client_home}\nEnvironment={install_generation}\nExecStart={} serve {service_flag} --host {} --port {} --no-tui --service-managed\nRestart=on-failure\nRestartSec=10s\nStartLimitIntervalSec=300\nStartLimitBurst=10\n\n[Install]\nWantedBy=default.target\n",
        systemd_quote(executable),
        options.host,
        options.port,
    )
}

#[cfg(any(target_os = "linux", test))]
fn systemd_environment_assignment(name: &str, value: &Path) -> String {
    let assignment = format!("{name}={}", value.to_string_lossy());
    format!(
        "\"{}\"",
        assignment.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

#[cfg(any(windows, test))]
fn write_service_definition(path: &Path, contents: &[u8]) -> CliResult<()> {
    let parent = path.parent().ok_or_else(|| {
        CliError::Other(format!(
            "service definition {} has no parent",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        CliError::Other(format!(
            "create service directory {}: {error}",
            parent.display()
        ))
    })?;
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&temporary, contents).map_err(|error| {
        CliError::Other(format!(
            "write service definition {}: {error}",
            temporary.display()
        ))
    })?;
    if let Err(error) = replace_service_definition(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(CliError::Other(format!(
            "install service definition {}: {error}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(all(not(windows), test))]
fn replace_service_definition(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_service_definition(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    fn wide_path(path: &Path) -> std::io::Result<Vec<u16>> {
        let encoded = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        if encoded[..encoded.len().saturating_sub(1)].contains(&0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path contains an embedded null",
            ));
        }
        Ok(encoded)
    }

    let temporary = wide_path(temporary)?;
    let destination = wide_path(destination)?;
    // SAFETY: Both path buffers are NUL-terminated and remain alive for the API call.
    if unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(any(target_os = "macos", windows, test))]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(any(target_os = "linux", test))]
fn systemd_quote(path: &Path) -> String {
    let value = path.to_string_lossy();
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'.'))
    {
        return value.into_owned();
    }
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_detection_is_total() {
        assert_ne!(ServicePlatform::current(), ServicePlatform::Unsupported);
    }

    #[test]
    fn service_name_flags_are_unambiguous() {
        assert_eq!(service_name_from_flags(false, false).unwrap(), "codex");
        assert_eq!(service_name_from_flags(true, false).unwrap(), "codex");
        assert_eq!(service_name_from_flags(false, true).unwrap(), "claude");
        assert!(service_name_from_flags(true, true).is_err());
    }

    fn test_service_status(state: ServiceRuntimeState) -> ServiceStatus {
        ServiceStatus {
            platform: ServicePlatform::current(),
            service_name: "test-service".to_string(),
            state,
            installed: true,
            autostart: true,
            service_definition: None,
            log_directory: PathBuf::from("/tmp/test-service-logs"),
            detail: None,
            receipt_state: ServiceReceiptState::Absent,
            credential_context: ServiceCredentialContext::Unverified,
            runtime_identity_verified: false,
            install_generation: None,
        }
    }

    #[tokio::test]
    async fn status_tolerates_absent_legacy_unsupported_invalid_and_unreachable_receipts() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-service-status-test-{}",
            uuid::Uuid::new_v4()
        ));
        let absent_home = root.join("absent");
        std::fs::create_dir_all(&absent_home).expect("create absent helper home");
        let absent = enrich_service_status(
            test_service_status(ServiceRuntimeState::Stopped),
            &absent_home,
        )
        .await;
        assert_eq!(absent.receipt_state, ServiceReceiptState::Absent);

        let legacy_home = root.join("legacy");
        std::fs::create_dir_all(&legacy_home).expect("create legacy helper home");
        std::fs::write(
            crate::service_receipt::service_receipt_path(&legacy_home),
            br#"{"schema_version":0}"#,
        )
        .expect("write legacy receipt");
        let legacy = enrich_service_status(
            test_service_status(ServiceRuntimeState::Stopped),
            &legacy_home,
        )
        .await;
        assert_eq!(legacy.receipt_state, ServiceReceiptState::Legacy);

        let unsupported_home = root.join("unsupported");
        std::fs::create_dir_all(&unsupported_home).expect("create unsupported helper home");
        std::fs::write(
            crate::service_receipt::service_receipt_path(&unsupported_home),
            br#"{"schema_version":2}"#,
        )
        .expect("write unsupported receipt");
        let unsupported = enrich_service_status(
            test_service_status(ServiceRuntimeState::Stopped),
            &unsupported_home,
        )
        .await;
        assert_eq!(unsupported.receipt_state, ServiceReceiptState::Unsupported);

        let invalid_home = root.join("invalid");
        std::fs::create_dir_all(&invalid_home).expect("create invalid helper home");
        std::fs::write(
            crate::service_receipt::service_receipt_path(&invalid_home),
            b"not-json",
        )
        .expect("write invalid receipt");
        let invalid = enrich_service_status(
            test_service_status(ServiceRuntimeState::Stopped),
            &invalid_home,
        )
        .await;
        assert_eq!(invalid.receipt_state, ServiceReceiptState::Invalid);

        let current_home = root.join("current");
        let client_home = root.join("client");
        std::fs::create_dir_all(&current_home).expect("create current helper home");
        std::fs::create_dir_all(&client_home).expect("create client home");
        let generation = codex_helper_core::service_target::ServiceInstallGeneration::generate();
        let receipt = ServiceReceipt::new(
            codex_helper_core::config::ServiceKind::Codex,
            current_home.clone(),
            client_home,
            "http://127.0.0.1:4211",
            ServicePlatformBackend::current().expect("supported test platform"),
            generation.clone(),
        )
        .expect("build current receipt");
        {
            let mut transaction = ServiceReceiptTransaction::begin(&current_home)
                .expect("begin current receipt transaction");
            transaction
                .replace(&receipt)
                .expect("publish current receipt");
        }
        let stopped = enrich_service_status(
            test_service_status(ServiceRuntimeState::Stopped),
            &current_home,
        )
        .await;
        assert_eq!(stopped.receipt_state, ServiceReceiptState::Current);
        assert_eq!(
            stopped.credential_context,
            ServiceCredentialContext::Unverified
        );
        assert_eq!(
            stopped.install_generation.as_deref(),
            Some(generation.as_str())
        );

        let unreachable = enrich_service_status(
            test_service_status(ServiceRuntimeState::Running),
            &current_home,
        )
        .await;
        assert_eq!(unreachable.receipt_state, ServiceReceiptState::Current);
        assert_eq!(
            unreachable.credential_context,
            ServiceCredentialContext::RuntimeUnavailable
        );
        assert!(!unreachable.runtime_identity_verified);

        std::fs::remove_dir_all(root).expect("remove service status test root");
    }

    #[test]
    fn service_definition_publication_replaces_an_existing_file() {
        let directory = std::env::temp_dir().join(format!(
            "codex-helper-service-definition-test-{}",
            uuid::Uuid::new_v4()
        ));
        let path = directory.join("service.xml");

        write_service_definition(&path, b"first").unwrap();
        write_service_definition(&path, b"second").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn started_service_readiness_has_distinct_ready_degraded_and_blocked_outcomes() {
        use codex_helper_core::credentials::CredentialAggregateReadiness;

        assert!(
            ensure_started_service_credential_readiness(CredentialAggregateReadiness::Ready)
                .is_ok()
        );
        assert!(
            ensure_started_service_credential_readiness(CredentialAggregateReadiness::Degraded)
                .is_ok()
        );
        let blocked =
            ensure_started_service_credential_readiness(CredentialAggregateReadiness::Blocked)
                .expect_err("blocked runtime must return a nonzero CLI outcome");
        assert!(
            blocked
                .to_string()
                .contains("admin endpoint remains available")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launch_agent_escapes_paths_and_configures_external_restart() {
        let options = ServiceInstallOptions {
            service_name: "codex",
            host: IpAddr::from([127, 0, 0, 1]),
            port: 3211,
            start: true,
            helper_home: PathBuf::from("/tmp/helper-home"),
            client_home: PathBuf::from("/tmp/codex-home"),
            install_generation:
                codex_helper_core::service_target::ServiceInstallGeneration::generate(),
        };
        let document = macos::render_launch_agent(
            Path::new("/tmp/codex & helper"),
            Path::new("/tmp/logs"),
            &options,
        );

        assert!(document.contains("/tmp/codex &amp; helper"));
        assert!(document.contains("<key>KeepAlive</key>"));
        assert!(document.contains("<key>SuccessfulExit</key><false/>"));
        assert!(document.contains("--service-managed"));
        assert!(document.contains("<key>CODEX_HOME</key><string>/tmp/codex-home</string>"));
        assert!(
            document
                .contains(codex_helper_core::service_target::SERVICE_INSTALL_GENERATION_ENV_VAR)
        );
        assert!(document.contains(options.install_generation.as_str()));
    }

    #[test]
    fn systemd_unit_preserves_helper_home_and_runs_as_user() {
        let options = ServiceInstallOptions {
            service_name: "claude",
            host: IpAddr::from([127, 0, 0, 1]),
            port: 3210,
            start: true,
            helper_home: PathBuf::from("/tmp/helper home"),
            client_home: PathBuf::from("/tmp/claude home"),
            install_generation:
                codex_helper_core::service_target::ServiceInstallGeneration::generate(),
        };
        let document = render_systemd_unit(Path::new("/usr/bin/codex-helper"), &options);

        assert!(document.contains("Restart=on-failure"));
        assert!(document.contains("WantedBy=default.target"));
        assert!(document.contains("--claude"));
        assert!(document.contains("--service-managed"));
        assert!(document.contains("Environment=\"CODEX_HELPER_HOME=/tmp/helper home\""));
        assert!(document.contains("Environment=\"CLAUDE_HOME=/tmp/claude home\""));
        assert!(
            document
                .contains(codex_helper_core::service_target::SERVICE_INSTALL_GENERATION_ENV_VAR)
        );
        assert!(document.contains(options.install_generation.as_str()));
    }

    #[test]
    fn windows_task_arguments_include_only_non_secret_runtime_paths() {
        let options = ServiceInstallOptions {
            service_name: "codex",
            host: IpAddr::from([127, 0, 0, 1]),
            port: 3211,
            start: true,
            helper_home: PathBuf::from("C:/Users/test/.codex-helper"),
            client_home: PathBuf::from("C:/Users/test/.codex"),
            install_generation:
                codex_helper_core::service_target::ServiceInstallGeneration::generate(),
        };

        let arguments = service_task_arguments(&options)
            .into_iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(&arguments[..2], ["service", "task-run"]);
        assert!(arguments.windows(2).any(|pair| {
            pair == [
                "--client-home".to_string(),
                "C:/Users/test/.codex".to_string(),
            ]
        }));
        assert!(arguments.windows(2).any(|pair| {
            pair == [
                "--install-generation".to_string(),
                options.install_generation.to_string(),
            ]
        }));
        assert!(!arguments.iter().any(|argument| {
            let argument = argument.to_ascii_lowercase();
            argument.contains("authorization")
                || argument.contains("api-key")
                || argument.contains("auth.json")
        }));
    }

    #[test]
    fn windows_task_definition_runs_as_current_user_at_least_privilege() {
        let options = ServiceInstallOptions {
            service_name: "codex",
            host: IpAddr::from([127, 0, 0, 1]),
            port: 3211,
            start: true,
            helper_home: PathBuf::from(r"C:\Users\test user\.codex-helper"),
            client_home: PathBuf::from(r"C:\Users\test user\.codex"),
            install_generation:
                codex_helper_core::service_target::ServiceInstallGeneration::generate(),
        };

        let document = render_windows_task_definition(
            Path::new(r"C:\Users\test user\.cargo\bin\codex-helper.exe"),
            &options,
            "S-1-5-21-100-200-300-400",
        );

        assert!(document.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(document.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(document.contains("<RestartOnFailure>"));
        assert!(document.contains("service task-run"));
        assert!(document.contains("--helper-home"));
        assert!(document.contains("&quot;C:\\Users\\test user\\.codex-helper&quot;"));
        assert!(!document.contains("LocalSystem"));
        assert!(!document.to_ascii_lowercase().contains("api-key"));
    }

    #[test]
    fn windows_argument_quoting_preserves_trailing_backslashes_and_quotes() {
        assert_eq!(quote_windows_argument("plain"), "plain");
        assert_eq!(quote_windows_argument("two words"), r#""two words""#);
        assert_eq!(quote_windows_argument(r#"a\"b\"#), r#""a\\\"b\\""#);
    }

    #[test]
    fn windows_task_names_are_scoped_to_a_valid_principal_sid() {
        let first = windows_task_name_for_sid("S-1-5-21-100-200-300-400").unwrap();
        let second = windows_task_name_for_sid("S-1-5-21-100-200-300-401").unwrap();

        assert_eq!(first, "codex-helper-S-1-5-21-100-200-300-400");
        assert_ne!(first, second);
        for invalid in [
            "",
            "Administrator",
            "S-1",
            "S-1-5-owner",
            "S-1-5-21-100'bad",
        ] {
            assert!(windows_task_name_for_sid(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn scheduled_task_probe_only_maps_explicit_absence_to_none() {
        assert_eq!(
            parse_windows_task_probe(r#"{"found":false}"#).unwrap(),
            None
        );
        for invalid in [
            "",
            r#"{"found":true}"#,
            r#"{"found":false,"record":{"task_name":"unexpected"}}"#,
            "Access is denied.",
        ] {
            assert!(parse_windows_task_probe(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn scheduled_task_readback_rejects_foreign_owner_and_changed_action() {
        let sid = "S-1-5-21-100-200-300-400";
        let task_name = windows_task_name_for_sid(sid).unwrap();
        let executable = Path::new("C:/Users/test/.cargo/bin/codex-helper.exe");
        let options = ServiceInstallOptions {
            service_name: "codex",
            host: IpAddr::from([127, 0, 0, 1]),
            port: 3211,
            start: true,
            helper_home: PathBuf::from("C:/Users/test/.codex-helper"),
            client_home: PathBuf::from("C:/Users/test/.codex"),
            install_generation:
                codex_helper_core::service_target::ServiceInstallGeneration::generate(),
        };
        let arguments = service_task_arguments(&options)
            .iter()
            .map(|argument| quote_windows_argument(argument.to_string_lossy().as_ref()))
            .collect::<Vec<_>>()
            .join(" ");
        let record = WindowsTaskRecord {
            task_name: task_name.clone(),
            task_path: "\\".to_string(),
            owner_sid: sid.to_string(),
            state: 3,
            enabled: true,
            action_count: 1,
            execute: executable.display().to_string(),
            arguments,
            working_directory: "C:/Users/test/.cargo/bin".to_string(),
            logon_type: "Interactive".to_string(),
            run_level: "Limited".to_string(),
            trigger_count: 1,
            trigger_enabled: true,
            trigger_type: "MSFT_TaskLogonTrigger".to_string(),
            trigger_user_sid: sid.to_string(),
        };

        assert!(verify_windows_task_record(&record, &task_name, sid, executable, &options).is_ok());
        let mut foreign = record.clone();
        foreign.owner_sid = "S-1-5-21-100-200-300-999".to_string();
        assert!(!windows_task_owner_matches(&foreign, sid));
        assert!(
            verify_windows_task_record(&foreign, &task_name, sid, executable, &options).is_err()
        );
        let mut changed_action = record;
        changed_action.execute = "C:/Windows/System32/cmd.exe".to_string();
        assert!(
            verify_windows_task_record(&changed_action, &task_name, sid, executable, &options)
                .is_err()
        );
    }

    #[test]
    fn windows_service_probe_only_classifies_error_1060_as_missing() {
        assert_eq!(
            classify_windows_service_probe_error(Some(1060)),
            WindowsServiceProbeClassification::Missing
        );
        for raw_os_error in [Some(5), Some(1072), None] {
            assert_eq!(
                classify_windows_service_probe_error(raw_os_error),
                WindowsServiceProbeClassification::Error
            );
        }
    }

    #[derive(Default)]
    struct FakeWindowsInstallBackend {
        events: Vec<&'static str>,
        fail_at: Option<&'static str>,
        rollback_fails: bool,
    }

    impl FakeWindowsInstallBackend {
        fn step(&mut self, name: &'static str) -> CliResult<()> {
            self.events.push(name);
            if self.fail_at == Some(name) || (name == "rollback" && self.rollback_fails) {
                Err(CliError::Other(format!("injected {name} failure")))
            } else {
                Ok(())
            }
        }
    }

    impl WindowsInstallTransactionBackend for FakeWindowsInstallBackend {
        fn preflight(&mut self) -> CliResult<()> {
            self.step("preflight")
        }

        fn register_scoped_task(&mut self) -> CliResult<()> {
            self.step("register")
        }

        fn verify_scoped_task(&mut self) -> CliResult<()> {
            self.step("verify")
        }

        fn publish_receipt(&mut self) -> CliResult<()> {
            self.step("publish_receipt")
        }

        fn retire_owned_fixed_task(&mut self) -> CliResult<()> {
            self.step("retire_fixed")
        }

        fn retire_legacy_scm(&mut self) -> CliResult<()> {
            self.step("retire_scm")
        }

        fn rollback_receipt(&mut self) -> CliResult<()> {
            self.step("rollback_receipt")
        }

        fn rollback(&mut self) -> CliResult<()> {
            self.step("rollback")
        }

        fn start_scoped_task(&mut self) -> CliResult<()> {
            self.step("start")
        }
    }

    #[test]
    fn windows_migration_verifies_new_task_before_retiring_legacy_installations() {
        let mut backend = FakeWindowsInstallBackend::default();

        run_windows_install_transaction(&mut backend, true).unwrap();

        assert_eq!(
            backend.events,
            [
                "preflight",
                "register",
                "verify",
                "publish_receipt",
                "retire_fixed",
                "retire_scm",
                "start",
            ]
        );
    }

    #[test]
    fn windows_migration_rolls_back_every_failure_after_preflight() {
        for (failure, expected) in [
            (
                "register",
                vec!["preflight", "register", "rollback_receipt", "rollback"],
            ),
            (
                "verify",
                vec![
                    "preflight",
                    "register",
                    "verify",
                    "rollback_receipt",
                    "rollback",
                ],
            ),
            (
                "publish_receipt",
                vec![
                    "preflight",
                    "register",
                    "verify",
                    "publish_receipt",
                    "rollback_receipt",
                    "rollback",
                ],
            ),
            (
                "retire_fixed",
                vec![
                    "preflight",
                    "register",
                    "verify",
                    "publish_receipt",
                    "retire_fixed",
                    "rollback_receipt",
                    "rollback",
                ],
            ),
            (
                "retire_scm",
                vec![
                    "preflight",
                    "register",
                    "verify",
                    "publish_receipt",
                    "retire_fixed",
                    "retire_scm",
                    "rollback_receipt",
                    "rollback",
                ],
            ),
        ] {
            let mut backend = FakeWindowsInstallBackend {
                fail_at: Some(failure),
                ..FakeWindowsInstallBackend::default()
            };

            let error = run_windows_install_transaction(&mut backend, true).unwrap_err();

            assert_eq!(backend.events, expected, "{failure}");
            assert!(error.to_string().contains("restored"), "{error}");
        }
    }

    #[test]
    fn windows_migration_preflight_failure_never_registers_or_retires() {
        let mut backend = FakeWindowsInstallBackend {
            fail_at: Some("preflight"),
            ..FakeWindowsInstallBackend::default()
        };

        assert!(run_windows_install_transaction(&mut backend, true).is_err());
        assert_eq!(backend.events, ["preflight"]);
    }

    #[test]
    fn windows_migration_reports_rollback_failure_and_keeps_verified_fallback() {
        let mut backend = FakeWindowsInstallBackend {
            fail_at: Some("retire_scm"),
            rollback_fails: true,
            ..FakeWindowsInstallBackend::default()
        };

        let error = run_windows_install_transaction(&mut backend, true).unwrap_err();

        assert!(error.to_string().contains("rollback also failed"));
        assert!(error.to_string().contains("left installed"));
        assert_eq!(backend.events.last(), Some(&"rollback"));
    }

    #[test]
    fn windows_migration_start_failure_does_not_remove_verified_installed_task() {
        let mut backend = FakeWindowsInstallBackend {
            fail_at: Some("start"),
            ..FakeWindowsInstallBackend::default()
        };

        let error = run_windows_install_transaction(&mut backend, true).unwrap_err();

        assert!(error.to_string().contains("was installed"));
        assert!(!backend.events.contains(&"rollback"));
    }

    #[test]
    fn legacy_default_service_derives_client_home_from_helper_owner() {
        assert_eq!(
            legacy_service_client_home("codex", Path::new("C:/Users/test/.codex-helper")),
            PathBuf::from("C:/Users/test/.codex")
        );
        assert_eq!(
            legacy_service_client_home("claude", Path::new("C:/Users/test/.codex-helper")),
            PathBuf::from("C:/Users/test/.claude")
        );
    }
}

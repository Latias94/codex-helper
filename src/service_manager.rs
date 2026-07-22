#[cfg(windows)]
use std::ffi::OsStr;
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
#[cfg(any(target_os = "macos", target_os = "linux", test))]
const MACOS_LABEL: &str = "io.github.latias94.codex-helper";
#[cfg(any(target_os = "linux", test))]
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
    legacy_installation: bool,
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
            let candidate_receipt = service_receipt(&options)?;
            preflight_service_install_identity(
                read_service_receipt(&options.helper_home),
                status(),
                &candidate_receipt,
            )?;
            preflight_service_credentials(service_kind(service_name)?, &options.helper_home)
                .await?;
            ensure_service_operator_token()?;
            let started_readiness = install(options).await?;
            if no_start {
                println!(
                    "Credential readiness is valid in the installer context but remains unverified in the installed service context until start."
                );
            } else {
                let readiness = started_readiness.ok_or_else(|| {
                    CliError::Other(
                        "service installation started the runtime without returning its verified credential readiness"
                            .to_string(),
                    )
                })?;
                ensure_started_service_credential_readiness(readiness)?;
            }
            print_status(&service_status().await?, false)?;
        }
        ServiceCommand::Uninstall { keep_running } => {
            uninstall_with_receipt(!keep_running)?;
            if keep_running {
                println!(
                    "Service registration and receipt were removed; the existing runtime was intentionally left running. No Codex client switch was restored because its target remains alive. Run `codex-helper switch off` before stopping the detached runtime, and run `codex-helper service install` before managing a future runtime; use `codex-helper daemon status` for best-effort observation."
                );
            }
        }
        ServiceCommand::Start => {
            preflight_installed_service_credentials().await?;
            ensure_service_operator_token()?;
            start()?;
            verify_started_service_runtime().await?;
        }
        ServiceCommand::Stop => run_service_stop_with_switch_policy(
            ServiceStopSwitchPolicy::RestoreMatchingCodexSwitch,
            reconcile_installed_service_switch_before_stop,
            stop,
        )?,
        ServiceCommand::Restart => {
            preflight_installed_service_credentials().await?;
            ensure_service_operator_token()?;
            run_service_stop_with_switch_policy(
                ServiceStopSwitchPolicy::PreserveForRestart,
                reconcile_installed_service_switch_before_stop,
                stop,
            )?;
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
    version: String,
    description: String,
    owner_sid: String,
    principal_count: usize,
    principal_id: String,
    actions_context: String,
    state: u8,
    enabled: bool,
    multiple_instances: String,
    disallow_start_if_on_batteries: bool,
    stop_if_going_on_batteries: bool,
    allow_hard_terminate: bool,
    start_when_available: bool,
    run_only_if_network_available: bool,
    allow_start_on_demand: bool,
    hidden: bool,
    run_only_if_idle: bool,
    wake_to_run: bool,
    execution_time_limit: String,
    priority: u8,
    restart_interval: String,
    restart_count: u32,
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
    codex_helper_core::path_identity::windows_path_strings_equal(left, right)
}

#[cfg(any(windows, test))]
fn parse_canonical_windows_command_line(value: &str) -> CliResult<Vec<String>> {
    let value = value.trim();
    if value.is_empty() {
        return Err(CliError::Other(
            "the Windows command line is empty".to_string(),
        ));
    }

    let characters = value.chars().collect::<Vec<_>>();
    let mut arguments = Vec::new();
    let mut index = 0_usize;
    while index < characters.len() {
        while index < characters.len() && characters[index].is_whitespace() {
            index += 1;
        }
        if index == characters.len() {
            break;
        }

        let mut argument = String::new();
        let mut quoted = false;
        loop {
            let mut backslashes = 0_usize;
            while index < characters.len() && characters[index] == '\\' {
                backslashes += 1;
                index += 1;
            }
            if index < characters.len() && characters[index] == '"' {
                argument.extend(std::iter::repeat_n('\\', backslashes / 2));
                if backslashes.is_multiple_of(2) {
                    quoted = !quoted;
                } else {
                    argument.push('"');
                }
                index += 1;
                continue;
            }
            argument.extend(std::iter::repeat_n('\\', backslashes));
            if index == characters.len() || (!quoted && characters[index].is_whitespace()) {
                break;
            }
            argument.push(characters[index]);
            index += 1;
        }
        if quoted {
            return Err(CliError::Other(
                "the Windows command line contains an unterminated quoted argument".to_string(),
            ));
        }
        arguments.push(argument);
    }

    let canonical = arguments
        .iter()
        .map(|argument| quote_windows_argument(argument))
        .collect::<Vec<_>>()
        .join(" ");
    if canonical != value {
        return Err(CliError::Other(
            "the Windows command line is not in the canonical codex-helper argument form"
                .to_string(),
        ));
    }
    Ok(arguments)
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct LegacyWindowsServiceInvocation {
    service_name: String,
    host: IpAddr,
    port: u16,
    helper_home: String,
    client_home: Option<String>,
}

#[cfg(any(windows, test))]
impl LegacyWindowsServiceInvocation {
    fn matches_install(&self, options: &ServiceInstallOptions) -> bool {
        self.service_name == options.service_name
            && self.host == options.host
            && self.port == options.port
            && windows_paths_equal(
                &self.helper_home,
                options.helper_home.to_string_lossy().as_ref(),
            )
            && self.client_home.as_deref().is_none_or(|client_home| {
                windows_paths_equal(client_home, options.client_home.to_string_lossy().as_ref())
            })
    }

    fn conflicts_with_install(&self, options: &ServiceInstallOptions) -> bool {
        self.port == options.port
    }
}

#[cfg(any(windows, test))]
fn parse_legacy_service_arguments(
    arguments: &[String],
    entrypoint: &str,
) -> CliResult<LegacyWindowsServiceInvocation> {
    let expected_len = match entrypoint {
        "run" => 10,
        "task-run" if arguments.len() == 12 || arguments.len() == 14 => arguments.len(),
        _ => 0,
    };
    let service_name = arguments
        .get(3)
        .filter(|value| matches!(value.as_str(), "codex" | "claude"));
    let host = arguments
        .get(5)
        .and_then(|value| value.parse::<IpAddr>().ok());
    let port = arguments.get(7).and_then(|value| value.parse::<u16>().ok());
    let paths_valid = arguments.get(9).is_some_and(|value| !value.is_empty())
        && (entrypoint == "run" || arguments.get(11).is_some_and(|value| !value.is_empty()));
    let generation_valid = arguments.len() != 14
        || arguments.get(13).is_some_and(|value| {
            codex_helper_core::service_target::ServiceInstallGeneration::parse(value).is_ok()
        });
    let exact_shape = arguments.len() == expected_len
        && arguments.first().is_some_and(|value| value == "service")
        && arguments.get(1).is_some_and(|value| value == entrypoint)
        && arguments
            .get(2)
            .is_some_and(|value| value == "--service-name")
        && arguments.get(4).is_some_and(|value| value == "--host")
        && arguments.get(6).is_some_and(|value| value == "--port")
        && arguments
            .get(8)
            .is_some_and(|value| value == "--helper-home")
        && (entrypoint == "run"
            || (arguments
                .get(10)
                .is_some_and(|value| value == "--client-home")
                && (arguments.len() != 14
                    || arguments
                        .get(12)
                        .is_some_and(|value| value == "--install-generation"))));
    let invalid = || {
        CliError::Other(format!(
            "the legacy Windows service invocation did not match the supported `service {entrypoint}` argument contract"
        ))
    };
    if !exact_shape || !paths_valid || !generation_valid {
        return Err(invalid());
    }
    Ok(LegacyWindowsServiceInvocation {
        service_name: service_name.cloned().ok_or_else(&invalid)?,
        host: host.ok_or_else(&invalid)?,
        port: port.ok_or_else(invalid)?,
        helper_home: arguments[9].clone(),
        client_home: (entrypoint == "task-run").then(|| arguments[11].clone()),
    })
}

#[cfg(any(windows, test))]
fn verify_legacy_fixed_windows_task_record(
    record: &WindowsTaskRecord,
    user_sid: &str,
    executable: &Path,
) -> CliResult<LegacyWindowsServiceInvocation> {
    let executable = windows_path_text(executable, "the codex-helper executable path")?;
    let working_directory = windows_path_text(
        Path::new(executable)
            .parent()
            .unwrap_or_else(|| Path::new(".")),
        "the codex-helper working directory",
    )?;
    let arguments = parse_canonical_windows_command_line(&record.arguments)?;
    let invocation = parse_legacy_service_arguments(&arguments, "task-run")?;
    let valid = record.task_name == WINDOWS_TASK_BASENAME
        && record.task_path == "\\"
        && windows_task_owner_matches(record, user_sid)
        && matches!(record.state, 1..=4)
        && record.enabled
        && record.action_count == 1
        && windows_paths_equal(&record.execute, executable)
        && windows_paths_equal(&record.working_directory, working_directory)
        && (record.logon_type.eq_ignore_ascii_case("interactive")
            || record.logon_type.eq_ignore_ascii_case("interactivetoken"))
        && (record.run_level.eq_ignore_ascii_case("limited")
            || record.run_level.eq_ignore_ascii_case("leastprivilege"))
        && record.trigger_count == 1
        && record.trigger_enabled
        && record
            .trigger_type
            .eq_ignore_ascii_case("MSFT_TaskLogonTrigger")
        && record.trigger_user_sid.eq_ignore_ascii_case(user_sid);
    if valid {
        Ok(invocation)
    } else {
        Err(CliError::Other(format!(
            "refusing to migrate Windows task '{}': it does not match the verified legacy codex-helper action, trigger, SID, or least-privilege definition",
            record.task_name
        )))
    }
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct LegacyWindowsScmDefinition {
    own_process: bool,
    start_type: u32,
    error_control: u32,
    dependencies: Vec<String>,
    account_name: Option<String>,
    display_name: String,
    load_order_group: Option<String>,
    command_line: String,
}

#[cfg(any(windows, test))]
fn verify_legacy_windows_scm_definition(
    definition: &LegacyWindowsScmDefinition,
    executable: &Path,
) -> CliResult<LegacyWindowsServiceInvocation> {
    const SERVICE_AUTO_START: u32 = 2;
    const SERVICE_ERROR_NORMAL: u32 = 1;
    let account_is_local_system = definition.account_name.as_deref().is_none_or(|account| {
        let account = account.trim();
        account.is_empty()
            || account.eq_ignore_ascii_case("LocalSystem")
            || account.eq_ignore_ascii_case("NT AUTHORITY\\SYSTEM")
            || account.eq_ignore_ascii_case(".\\LocalSystem")
    });
    let arguments = parse_canonical_windows_command_line(&definition.command_line)?;
    let executable_matches = arguments
        .first()
        .is_some_and(|configured| windows_paths_equal(configured, &executable.to_string_lossy()));
    let invocation = parse_legacy_service_arguments(arguments.get(1..).unwrap_or_default(), "run")?;
    if definition.own_process
        && definition.start_type == SERVICE_AUTO_START
        && definition.error_control == SERVICE_ERROR_NORMAL
        && definition.dependencies.is_empty()
        && account_is_local_system
        && definition.display_name == "codex-helper relay"
        && definition.load_order_group.is_none()
        && executable_matches
    {
        Ok(invocation)
    } else {
        Err(CliError::Other(
            "refusing to migrate the same-name Windows SCM service because its startup, account, dependencies, display name, service type, or executable is not the verified legacy codex-helper definition"
                .to_string(),
        ))
    }
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
        && record.version == "1.2"
        && record.description == "Resident codex-helper relay for the current user."
        && windows_task_owner_matches(record, user_sid)
        && record.principal_count == 1
        && record.principal_id == "Author"
        && record.actions_context == "Author"
        && matches!(record.state, 2..=4)
        && record.enabled
        && (record.multiple_instances.eq_ignore_ascii_case("IgnoreNew")
            || record.multiple_instances == "2")
        && !record.disallow_start_if_on_batteries
        && !record.stop_if_going_on_batteries
        && record.allow_hard_terminate
        && record.start_when_available
        && !record.run_only_if_network_available
        && record.allow_start_on_demand
        && !record.hidden
        && !record.run_only_if_idle
        && !record.wake_to_run
        && record.execution_time_limit.eq_ignore_ascii_case("PT0S")
        && record.priority == 7
        && record.restart_interval.eq_ignore_ascii_case("PT10S")
        && record.restart_count == 10
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
            "the registered Windows task '{task_name}' did not match the complete canonical SID, registration, settings, action, trigger, or least-privilege definition"
        )))
    }
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsDaemonExecutableAuthority {
    Receipt,
    CurrentExecutableCompatibility,
}

#[cfg(any(windows, test))]
fn installed_windows_daemon_executable(
    receipt: &ServiceReceipt,
    current_executable: &Path,
) -> (PathBuf, WindowsDaemonExecutableAuthority) {
    match receipt.daemon_executable() {
        Some(executable) => (
            executable.to_path_buf(),
            WindowsDaemonExecutableAuthority::Receipt,
        ),
        None => (
            current_executable.to_path_buf(),
            WindowsDaemonExecutableAuthority::CurrentExecutableCompatibility,
        ),
    }
}

#[cfg(any(windows, test))]
fn verify_installed_windows_task_record(
    record: &WindowsTaskRecord,
    task_name: &str,
    user_sid: &str,
    receipt: &ServiceReceipt,
    current_executable: &Path,
    installed_options: &ServiceInstallOptions,
) -> CliResult<()> {
    let (executable, authority) = installed_windows_daemon_executable(receipt, current_executable);
    verify_windows_task_record(
        record,
        task_name,
        user_sid,
        &executable,
        installed_options,
    )
    .map_err(|error| match authority {
        WindowsDaemonExecutableAuthority::Receipt => error,
        WindowsDaemonExecutableAuthority::CurrentExecutableCompatibility => CliError::Other(
            format!(
                "{error}; this schema-1 compatibility receipt has no daemon_executable, so verification is intentionally limited to the current CLI executable {}. Retry with the binary path that installed the task, then run `codex-helper service install` to refresh the receipt",
                current_executable.display()
            ),
        ),
    })
}

#[cfg(any(windows, test))]
fn verify_existing_windows_task_for_replacement(
    record: &WindowsTaskRecord,
    task_name: &str,
    user_sid: &str,
    current_executable: &Path,
    installed_receipt: Option<&ServiceReceipt>,
) -> CliResult<()> {
    let installed_receipt = installed_receipt.ok_or_else(|| {
        CliError::Other(
            "refusing to replace an existing SID-scoped Windows task without a current receipt proving its complete canonical definition"
                .to_string(),
        )
    })?;
    let installed_options = service_install_options_from_receipt(installed_receipt, false)?;
    verify_installed_windows_task_record(
        record,
        task_name,
        user_sid,
        installed_receipt,
        current_executable,
        &installed_options,
    )
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsServiceProbeClassification {
    Missing,
    MarkedForDelete,
    Error,
}

#[cfg(any(windows, test))]
fn classify_windows_service_probe_error(
    raw_os_error: Option<i32>,
) -> WindowsServiceProbeClassification {
    const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;
    const ERROR_SERVICE_MARKED_FOR_DELETE: i32 = 1072;
    match raw_os_error {
        Some(ERROR_SERVICE_DOES_NOT_EXIST) => WindowsServiceProbeClassification::Missing,
        Some(ERROR_SERVICE_MARKED_FOR_DELETE) => WindowsServiceProbeClassification::MarkedForDelete,
        _ => WindowsServiceProbeClassification::Error,
    }
}

#[cfg(any(windows, test))]
trait WindowsInstallTransactionBackend {
    fn preflight(&mut self) -> CliResult<()>;
    fn stop_existing_scoped_task(&mut self) -> CliResult<()>;
    fn stop_legacy_runtimes(&mut self) -> CliResult<()>;
    fn register_scoped_task(&mut self) -> CliResult<()>;
    fn verify_scoped_task(&mut self) -> CliResult<()>;
    fn publish_receipt(&mut self) -> CliResult<()>;
    fn retire_owned_fixed_task(&mut self) -> CliResult<()>;
    fn retire_legacy_scm(&mut self) -> CliResult<()>;
    fn rollback_receipt(&mut self) -> CliResult<()>;
    fn rollback(&mut self) -> CliResult<()>;
    fn rollback_preserved_replacement(&self) -> bool;
    fn start_scoped_task(&mut self) -> CliResult<()>;
    async fn verify_started_runtime_identity(
        &mut self,
    ) -> CliResult<codex_helper_core::credentials::CredentialAggregateReadiness>;
}

#[cfg(any(windows, test))]
async fn run_windows_install_transaction(
    backend: &mut impl WindowsInstallTransactionBackend,
    start: bool,
) -> CliResult<Option<codex_helper_core::credentials::CredentialAggregateReadiness>> {
    backend.preflight()?;
    let mut new_task_verified = false;
    let mut started_readiness = None;
    let migration: CliResult<()> = async {
        backend.stop_existing_scoped_task()?;
        // A verified legacy runtime can listen on the same proxy address as the replacement.
        // Keep its registration for rollback, but stop it before the new task is started.
        backend.stop_legacy_runtimes()?;
        backend.register_scoped_task()?;
        backend.verify_scoped_task()?;
        new_task_verified = true;
        backend.publish_receipt()?;
        if start {
            backend.start_scoped_task()?;
            started_readiness = Some(backend.verify_started_runtime_identity().await?);
        }
        backend.retire_owned_fixed_task()?;
        backend.retire_legacy_scm()
    }
    .await;
    if let Err(primary) = migration {
        let platform_rollback = if backend.rollback_preserved_replacement() {
            Err(CliError::Other(
                "the verified replacement task was preserved because legacy retirement may have committed"
                    .to_string(),
            ))
        } else {
            backend.rollback()
        };
        let replacement_preserved = backend.rollback_preserved_replacement();
        let receipt_rollback = if replacement_preserved {
            Err(CliError::Other(
                "kept the current service receipt because the verified replacement task was preserved"
                    .to_string(),
            ))
        } else {
            backend.rollback_receipt()
        };
        return match (receipt_rollback, platform_rollback) {
            (Ok(()), Ok(())) => Err(CliError::Other(format!(
                "Windows service migration failed and the previous runnable installation was restored: {primary}"
            ))),
            (receipt_rollback, platform_rollback) => {
                let fallback = if replacement_preserved && new_task_verified {
                    "The verified SID-scoped task was left installed when required to avoid removing the last runnable installation"
                } else {
                    "The replacement task was not preserved as a verified fallback; inspect the reported platform recovery failures before starting or removing any task"
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
    Ok(started_readiness)
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
trait UnixInstallTransactionBackend {
    fn prepare_replacement(&mut self) -> CliResult<()>;
    fn start_replacement(&mut self) -> CliResult<()>;
    async fn verify_started_runtime_identity(
        &mut self,
    ) -> CliResult<codex_helper_core::credentials::CredentialAggregateReadiness>;
    fn rollback(&mut self) -> CliResult<()>;
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
async fn run_unix_install_transaction(
    backend: &mut impl UnixInstallTransactionBackend,
    start: bool,
) -> CliResult<Option<codex_helper_core::credentials::CredentialAggregateReadiness>> {
    let mutation: CliResult<_> = async {
        backend.prepare_replacement()?;
        if start {
            backend.start_replacement()?;
            backend.verify_started_runtime_identity().await.map(Some)
        } else {
            Ok(None)
        }
    }
    .await;
    match mutation {
        Ok(readiness) => Ok(readiness),
        Err(primary) => {
            let failures = backend
                .rollback()
                .err()
                .map(|error| vec![error.to_string()])
                .unwrap_or_default();
            Err(rollback_error(primary, failures))
        }
    }
}

#[cfg(any(windows, test))]
trait WindowsUninstallTransactionBackend {
    fn stop_and_verify(&mut self) -> CliResult<()>;
    fn remove_scoped_task(&mut self) -> CliResult<()>;
    fn remove_fixed_task(&mut self) -> CliResult<()>;
    fn remove_definition(&mut self) -> CliResult<()>;
    fn remove_receipt(&mut self) -> CliResult<()>;
    fn retire_legacy_scm(&mut self) -> CliResult<()>;
    // Ok means every mutated resource, including an attempted legacy SCM retirement, was
    // independently observed in its original registration and runtime state.
    fn rollback(&mut self) -> CliResult<()>;
}

#[cfg(any(windows, test))]
fn run_windows_uninstall_transaction(
    backend: &mut impl WindowsUninstallTransactionBackend,
    stop_first: bool,
) -> CliResult<()> {
    let mutation = (|| {
        if stop_first {
            backend.stop_and_verify()?;
        }
        backend.remove_scoped_task()?;
        backend.remove_fixed_task()?;
        backend.remove_definition()?;
        backend.remove_receipt()?;
        // SCM service definitions cannot be reconstructed from the public Windows API. Retire
        // the legacy service only after every reversible resource has committed successfully.
        backend.retire_legacy_scm()
    })();
    if let Err(primary) = mutation {
        return match backend.rollback() {
            Ok(()) => {
                let restored = if stop_first {
                    "the previous task registrations, definition, receipt, and runtime state were restored"
                } else {
                    "the previous task registrations, definition, and receipt were restored; detached runtime instances were intentionally never stopped"
                };
                Err(CliError::Other(format!(
                    "Windows service uninstall failed before completion and {restored}: {primary}"
                )))
            }
            Err(rollback) => Err(CliError::Other(format!(
                "Windows service uninstall failed: {primary}; restoring the previous task registrations, definition, receipt, or runtime state also failed: {rollback}. The Windows service installation is partial; inspect Task Scheduler and SCM, then run `codex-helper service install` to repair it"
            ))),
        };
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
trait ServiceUninstallTransactionBackend {
    fn stop_and_verify(&mut self) -> CliResult<()>;
    fn disable_and_verify(&mut self) -> CliResult<()>;
    fn remove_definition(&mut self) -> CliResult<()>;
    fn remove_receipt(&mut self) -> CliResult<()>;
    fn rollback(&mut self) -> CliResult<()>;
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn run_service_uninstall_transaction(
    backend: &mut impl ServiceUninstallTransactionBackend,
    stop_first: bool,
) -> CliResult<()> {
    let mutation = (|| {
        if stop_first {
            backend.stop_and_verify()?;
        }
        backend.disable_and_verify()?;
        backend.remove_definition()?;
        backend.remove_receipt()
    })();
    if let Err(primary) = mutation {
        return match backend.rollback() {
            Ok(()) => Err(CliError::Other(format!(
                "service uninstall failed before completion and the previous definition and receipt were restored: {primary}"
            ))),
            Err(rollback) => Err(CliError::Other(format!(
                "service uninstall failed: {primary}; restoring the previous definition, receipt, or runtime state also failed: {rollback}"
            ))),
        };
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ServiceSwitchUninstallPreparation {
    NotApplicable,
    Restored,
    Unchanged,
    Warning(String),
}

fn local_proxy_target_from_service_receipt(
    receipt: &ServiceReceipt,
) -> CliResult<codex_helper_core::codex_switch::ValidatedCodexBaseUrl> {
    let admin_url = reqwest::Url::parse(receipt.admin_base_url()).map_err(|error| {
        CliError::Other(format!(
            "cannot derive the installed proxy target from service receipt admin URL {:?}: {error}",
            receipt.admin_base_url()
        ))
    })?;
    if admin_url.scheme() != "http" || admin_url.host_str() != Some("127.0.0.1") {
        return Err(CliError::Other(format!(
            "service receipt admin URL {:?} is not the canonical local service authority",
            receipt.admin_base_url()
        )));
    }
    let admin_port = admin_url.port().ok_or_else(|| {
        CliError::Other(format!(
            "service receipt admin URL {:?} does not contain an explicit local admin port",
            receipt.admin_base_url()
        ))
    })?;
    let offset = codex_helper_core::proxy::ADMIN_PORT_OFFSET;
    let ambiguous_start = u16::MAX - (offset * 2) + 1;
    let ambiguous_end = u16::MAX - offset;
    if (ambiguous_start..=ambiguous_end).contains(&admin_port) {
        return Err(CliError::Other(format!(
            "service receipt admin port {admin_port} is ambiguous and cannot identify one proxy port"
        )));
    }
    let proxy_port = admin_port.checked_sub(offset).ok_or_else(|| {
        CliError::Other(format!(
            "service receipt admin port {admin_port} is outside the reversible local proxy range"
        ))
    })?;
    let round_trip = codex_helper_core::proxy::local_admin_base_url_for_proxy_port(proxy_port);
    if round_trip != receipt.admin_base_url() {
        return Err(CliError::Other(format!(
            "service receipt admin URL {:?} does not round-trip to one canonical local proxy target",
            receipt.admin_base_url()
        )));
    }
    Ok(codex_helper_core::codex_switch::ValidatedCodexBaseUrl::local(proxy_port))
}

fn switch_target_may_point_to_service(
    active_target: Option<&str>,
    expected_target: &codex_helper_core::codex_switch::ValidatedCodexBaseUrl,
) -> bool {
    let Some(active_target) = active_target else {
        return true;
    };
    if active_target == expected_target.as_str() {
        return true;
    }
    let (Ok(active), Ok(expected)) = (
        reqwest::Url::parse(active_target),
        reqwest::Url::parse(expected_target.as_str()),
    ) else {
        return true;
    };
    let Some(host) = active.host_str() else {
        return true;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    active.scheme() == "http"
        && active.port_or_known_default() == expected.port_or_known_default()
        && loopback
}

#[cfg(test)]
fn reconcile_service_switch_before_uninstall<F>(
    receipt: &ServiceReceipt,
    stop_first: bool,
    restore: F,
) -> CliResult<ServiceSwitchUninstallPreparation>
where
    F: FnOnce(
        &Path,
        &codex_helper_core::codex_switch::ValidatedCodexBaseUrl,
    ) -> Result<
        codex_helper_core::codex_switch::CodexSwitchTargetRestoreOutcome,
        codex_helper_core::codex_switch::CodexSwitchError,
    >,
{
    reconcile_service_switch_before_runtime_stop(
        receipt,
        stop_first,
        "stop and uninstall the Codex service",
        "Service uninstall",
        restore,
    )
}

fn reconcile_service_switch_before_runtime_stop<F>(
    receipt: &ServiceReceipt,
    runtime_will_stop: bool,
    operation: &str,
    continuation: &str,
    restore: F,
) -> CliResult<ServiceSwitchUninstallPreparation>
where
    F: FnOnce(
        &Path,
        &codex_helper_core::codex_switch::ValidatedCodexBaseUrl,
    ) -> Result<
        codex_helper_core::codex_switch::CodexSwitchTargetRestoreOutcome,
        codex_helper_core::codex_switch::CodexSwitchError,
    >,
{
    if !runtime_will_stop || receipt.service() != codex_helper_core::config::ServiceKind::Codex {
        return Ok(ServiceSwitchUninstallPreparation::NotApplicable);
    }

    let expected_target = local_proxy_target_from_service_receipt(receipt).map_err(|error| {
        CliError::Other(format!(
            "refusing to {operation} because its receipt cannot prove the matching client switch target: {error}. Run `codex-helper switch off` first, then retry"
        ))
    })?;
    let outcome = restore(receipt.client_home(), &expected_target).map_err(|error| {
        CliError::Other(format!(
            "refusing to {operation} because its matching client switch could not be restored safely: {error}. The service registration, receipt, and runtime were left in place; repair or run `codex-helper switch off`, then retry"
        ))
    })?;
    match outcome {
        codex_helper_core::codex_switch::CodexSwitchTargetRestoreOutcome::Restored(_) => {
            Ok(ServiceSwitchUninstallPreparation::Restored)
        }
        codex_helper_core::codex_switch::CodexSwitchTargetRestoreOutcome::Unchanged(status) => {
            let active_target = status.base_url.as_deref();
            if status.enabled && switch_target_may_point_to_service(active_target, &expected_target)
            {
                return Err(CliError::Other(format!(
                    "refusing to {operation} because Codex client config {} still selects the service target {} but is not a clean helper-managed Applied switch (phase={}, managed={}). No client or service files were changed; run `codex-helper switch off` or reconcile the reported switch state before retrying",
                    status.config_path.display(),
                    expected_target.as_str(),
                    status.phase.as_str(),
                    status.managed,
                )));
            }
            if status.phase == codex_helper_core::codex_switch::CodexSwitchPhase::Off
                && !status.enabled
            {
                return Ok(ServiceSwitchUninstallPreparation::Unchanged);
            }
            Ok(ServiceSwitchUninstallPreparation::Warning(format!(
                "Codex client switch at {} was not modified because it does not actively select the service target {} (phase={}, target={}). {continuation} will continue without changing client files",
                status.config_path.display(),
                expected_target.as_str(),
                status.phase.as_str(),
                status.base_url.as_deref().unwrap_or("<unknown>"),
            )))
        }
    }
}

fn reconcile_service_switch_before_no_start_install<F>(
    start_after_install: bool,
    platform_state: ServiceRuntimeState,
    installed_receipt: Option<&ServiceReceipt>,
    restore: F,
) -> CliResult<ServiceSwitchUninstallPreparation>
where
    F: FnOnce(
        &Path,
        &codex_helper_core::codex_switch::ValidatedCodexBaseUrl,
    ) -> Result<
        codex_helper_core::codex_switch::CodexSwitchTargetRestoreOutcome,
        codex_helper_core::codex_switch::CodexSwitchError,
    >,
{
    let runtime_will_stop = !start_after_install
        && matches!(
            platform_state,
            ServiceRuntimeState::Running
                | ServiceRuntimeState::Starting
                | ServiceRuntimeState::Stopping
        );
    let Some(receipt) = installed_receipt else {
        return Ok(ServiceSwitchUninstallPreparation::NotApplicable);
    };
    reconcile_service_switch_before_runtime_stop(
        receipt,
        runtime_will_stop,
        "replace the running Codex service with a stopped installation",
        "Service installation",
        restore,
    )
}

fn prepare_service_switch_for_install(
    options: &ServiceInstallOptions,
    preflight: &ServiceInstallPreflight,
) -> CliResult<()> {
    let preparation = reconcile_service_switch_before_no_start_install(
        options.start,
        preflight.platform_state,
        preflight.installed_receipt.as_ref(),
        codex_helper_core::codex_switch::restore_managed_applied_for_target,
    )?;
    report_service_switch_preparation(
        preparation,
        "Restored the helper-managed Codex client switch before replacing the running service with a stopped installation.",
    );
    Ok(())
}

fn report_service_switch_preparation(
    preparation: ServiceSwitchUninstallPreparation,
    restored_message: &str,
) {
    match preparation {
        ServiceSwitchUninstallPreparation::Restored => println!("{restored_message}"),
        ServiceSwitchUninstallPreparation::Warning(warning) => eprintln!("Warning: {warning}"),
        ServiceSwitchUninstallPreparation::NotApplicable
        | ServiceSwitchUninstallPreparation::Unchanged => {}
    }
}

fn reconcile_installed_service_switch_for_operation(
    operation: &'static str,
    continuation: &'static str,
    restored_message: &'static str,
) -> CliResult<()> {
    let receipt = read_service_receipt(proxy_home_dir()).map_err(|error| {
        CliError::Other(format!(
            "refusing to {operation} without a current install receipt: {error}. Run `codex-helper switch off` first if Codex points at this service, then repair the receipt with the codex-helper version that created the service"
        ))
    })?;
    verify_installed_service_definition_authority(&receipt)?;
    let preparation = reconcile_service_switch_before_runtime_stop(
        &receipt,
        true,
        operation,
        continuation,
        codex_helper_core::codex_switch::restore_managed_applied_for_target,
    )?;
    report_service_switch_preparation(preparation, restored_message);
    Ok(())
}

fn reconcile_installed_service_switch() -> CliResult<()> {
    reconcile_installed_service_switch_for_operation(
        "stop and uninstall the service",
        "Service uninstall",
        "Restored the helper-managed Codex client switch before stopping the local service.",
    )
}

fn reconcile_installed_service_switch_before_stop() -> CliResult<()> {
    reconcile_installed_service_switch_for_operation(
        "stop the service",
        "Service stop",
        "Restored the helper-managed Codex client switch before stopping the local service.",
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceStopSwitchPolicy {
    RestoreMatchingCodexSwitch,
    PreserveForRestart,
}

fn run_service_stop_with_switch_policy<R, S>(
    policy: ServiceStopSwitchPolicy,
    reconcile: R,
    stop_platform: S,
) -> CliResult<()>
where
    R: FnOnce() -> CliResult<()>,
    S: FnOnce() -> CliResult<()>,
{
    if policy == ServiceStopSwitchPolicy::RestoreMatchingCodexSwitch {
        reconcile()?;
    }
    stop_platform()
}

fn run_service_uninstall_with_switch_preflight<R, U>(
    stop_first: bool,
    reconcile: R,
    uninstall_platform: U,
) -> CliResult<()>
where
    R: FnOnce() -> CliResult<()>,
    U: FnOnce(bool) -> CliResult<()>,
{
    if stop_first {
        // A successful restore is intentionally one-way: platform rollback leaves the client Off,
        // which is safe, while reapplying could overwrite a concurrent client edit.
        reconcile()?;
    }
    uninstall_platform(stop_first)
}

fn validate_service_receipt_for_uninstall(
    receipt: Result<ServiceReceipt, ServiceReceiptError>,
    current_backend: Option<ServicePlatformBackend>,
) -> CliResult<ServiceReceipt> {
    let receipt = receipt.map_err(|error| {
        CliError::Other(format!(
            "refusing to uninstall without a current, valid service receipt: {error}. No service registration, receipt, runtime, or client files were changed; repair or migrate the installation before retrying"
        ))
    })?;
    let current_backend = current_backend.ok_or_else(|| {
        CliError::Other("system service management is unsupported on this platform".to_string())
    })?;
    if receipt.platform_backend() != current_backend {
        return Err(CliError::Other(format!(
            "refusing to uninstall a service receipt for {:?} with the current {:?} backend. No service registration, receipt, runtime, or client files were changed",
            receipt.platform_backend(),
            current_backend,
        )));
    }
    CanonicalServiceInstallIdentity::from_receipt(&receipt).map_err(|error| {
        CliError::Other(format!(
            "refusing to uninstall because the service receipt identity is invalid: {error}. No service registration, receipt, runtime, or client files were changed"
        ))
    })?;
    Ok(receipt)
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

fn service_receipt_with_daemon_executable(
    options: &ServiceInstallOptions,
    daemon_executable: &Path,
) -> CliResult<ServiceReceipt> {
    service_receipt(options)?
        .with_daemon_executable(daemon_executable.to_path_buf())
        .map_err(|error| {
            CliError::Other(format!(
                "record canonical daemon executable in service install receipt: {error}"
            ))
        })
}

#[derive(Debug, Clone)]
struct CanonicalServiceInstallIdentity {
    service: codex_helper_core::config::ServiceKind,
    proxy_target: codex_helper_core::codex_switch::ValidatedCodexBaseUrl,
    client_home: PathBuf,
    platform_backend: ServicePlatformBackend,
}

impl CanonicalServiceInstallIdentity {
    fn from_receipt(receipt: &ServiceReceipt) -> CliResult<Self> {
        Ok(Self {
            service: receipt.service(),
            proxy_target: local_proxy_target_from_service_receipt(receipt)?,
            client_home: receipt.client_home().to_path_buf(),
            platform_backend: receipt.platform_backend(),
        })
    }

    fn matches(&self, other: &Self) -> CliResult<bool> {
        // The platform transaction authorizes and migrates the daemon executable separately.
        if self.service != other.service
            || self.proxy_target != other.proxy_target
            || self.platform_backend != other.platform_backend
        {
            return Ok(false);
        }
        service_paths_identify_same_location(&self.client_home, &other.client_home)
    }

    fn describe(&self) -> String {
        format!(
            "service={}, proxy_target={}, client_home={}, platform={:?}",
            codex_helper_core::runtime_host::service_name_for_kind(self.service),
            self.proxy_target.as_str(),
            self.client_home.display(),
            self.platform_backend,
        )
    }
}

fn service_paths_identify_same_location(left: &Path, right: &Path) -> CliResult<bool> {
    let left = resolve_service_path_identity(left)?;
    let right = resolve_service_path_identity(right)?;
    Ok(codex_helper_core::path_identity::path_identities_equal(
        &left, &right,
    ))
}

fn resolve_service_path_identity(path: &Path) -> CliResult<PathBuf> {
    if !path.is_absolute() {
        return Err(CliError::Other(format!(
            "service path identity must be absolute: {}",
            path.display()
        )));
    }

    codex_helper_core::path_identity::resolve_path_identity(path).map_err(|error| {
        CliError::Other(format!(
            "resolve service path identity {}: {error}",
            path.display()
        ))
    })
}

#[cfg(test)]
fn service_path_identities_equal_with_windows_semantics(
    left: &Path,
    right: &Path,
    windows_semantics: bool,
) -> bool {
    codex_helper_core::path_identity::path_identities_equal_with_windows_semantics(
        left,
        right,
        windows_semantics,
    )
}

#[cfg(any(windows, target_os = "macos", target_os = "linux", test))]
fn service_install_options_from_receipt(
    receipt: &ServiceReceipt,
    start: bool,
) -> CliResult<ServiceInstallOptions> {
    let proxy_target = local_proxy_target_from_service_receipt(receipt)?;
    let target = reqwest::Url::parse(proxy_target.as_str()).map_err(|error| {
        CliError::Other(format!(
            "cannot parse the canonical proxy target from the service receipt: {error}"
        ))
    })?;
    let host = target
        .host_str()
        .and_then(|host| host.parse::<IpAddr>().ok())
        .ok_or_else(|| {
            CliError::Other(
                "the service receipt proxy target does not contain a numeric local address"
                    .to_string(),
            )
        })?;
    let port = target.port().ok_or_else(|| {
        CliError::Other(
            "the service receipt proxy target does not contain an explicit proxy port".to_string(),
        )
    })?;
    Ok(ServiceInstallOptions {
        service_name: codex_helper_core::runtime_host::service_name_for_kind(receipt.service()),
        host,
        port,
        start,
        helper_home: receipt.helper_home().to_path_buf(),
        client_home: receipt.client_home().to_path_buf(),
        install_generation: receipt.install_generation().clone(),
    })
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn unix_service_definition_name(backend: ServicePlatformBackend) -> CliResult<&'static str> {
    match backend {
        ServicePlatformBackend::MacosLaunchAgent => Ok("LaunchAgent plist"),
        ServicePlatformBackend::LinuxSystemdUser => Ok("systemd user unit"),
        ServicePlatformBackend::WindowsScheduledTask => Err(CliError::Other(
            "a Windows service receipt cannot authorize a Unix service definition".to_string(),
        )),
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn required_unix_daemon_executable(receipt: &ServiceReceipt) -> CliResult<&Path> {
    let definition_name = unix_service_definition_name(receipt.platform_backend())?;
    receipt.daemon_executable().ok_or_else(|| {
        CliError::Other(format!(
            "refusing to manage the installed {definition_name}: the current schema-1 service receipt has no daemon_executable authority. No service registration, definition, receipt, or runtime was changed. Use the codex-helper binary/version that installed this service to uninstall it, then run `codex-helper service install` with the current binary"
        ))
    })
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn expected_unix_service_definition(receipt: &ServiceReceipt) -> CliResult<Vec<u8>> {
    let executable = required_unix_daemon_executable(receipt)?;
    let options = service_install_options_from_receipt(receipt, false)?;
    let definition = match receipt.platform_backend() {
        ServicePlatformBackend::MacosLaunchAgent => render_launch_agent_definition(
            executable,
            &receipt.helper_home().join("logs"),
            &options,
        ),
        ServicePlatformBackend::LinuxSystemdUser => render_systemd_unit(executable, &options),
        ServicePlatformBackend::WindowsScheduledTask => {
            return Err(CliError::Other(
                "a Windows service receipt cannot authorize a Unix service definition".to_string(),
            ));
        }
    };
    Ok(definition.into_bytes())
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn verify_unix_service_definition_snapshot(
    receipt: &ServiceReceipt,
    actual: Option<&[u8]>,
) -> CliResult<()> {
    let definition_name = unix_service_definition_name(receipt.platform_backend())?;
    let expected = expected_unix_service_definition(receipt)?;
    match actual {
        Some(actual) if actual == expected.as_slice() => Ok(()),
        Some(_) => Err(CliError::Other(format!(
            "refusing to mutate the installed {definition_name}: its complete on-disk definition does not match the current service receipt (daemon executable, command, service, host, port, helper/client homes, install generation, logs, or restart policy). No service registration, definition, receipt, or runtime was changed. Restore the matching definition or use the codex-helper binary/version that installed it to uninstall it, then run `codex-helper service install`"
        ))),
        None => Err(CliError::Other(format!(
            "refusing to mutate the installed {definition_name}: its definition is missing while a current service receipt still claims it. No service registration, definition, receipt, or runtime was changed. Restore the matching definition or use the codex-helper binary/version that installed it to uninstall it, then run `codex-helper service install`"
        ))),
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn verify_unix_service_install_replacement(
    installed_receipt: Option<&ServiceReceipt>,
    actual: Option<&[u8]>,
    candidate_backend: ServicePlatformBackend,
) -> CliResult<()> {
    match installed_receipt {
        Some(receipt) if receipt.platform_backend() == candidate_backend => {
            verify_unix_service_definition_snapshot(receipt, actual)
        }
        Some(receipt) => Err(CliError::Other(format!(
            "refusing to replace a {:?} service using a {:?} receipt. No service registration, definition, receipt, or runtime was changed; use the codex-helper binary/version that created the service to uninstall it, then run `codex-helper service install`",
            candidate_backend,
            receipt.platform_backend(),
        ))),
        None if actual.is_none() => Ok(()),
        None => {
            let definition_name = unix_service_definition_name(candidate_backend)?;
            Err(CliError::Other(format!(
                "refusing to replace an existing {definition_name} without a current service receipt proving its complete definition. No service registration, definition, receipt, or runtime was changed. Use the codex-helper binary/version that created it to uninstall it, then run `codex-helper service install`"
            )))
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn verify_unix_service_definition_at(
    receipt: &ServiceReceipt,
    expected_backend: ServicePlatformBackend,
    path: &Path,
) -> CliResult<()> {
    if receipt.platform_backend() != expected_backend {
        return Err(CliError::Other(format!(
            "refusing to manage a {:?} service with a {:?} receipt. No service registration, definition, receipt, or runtime was changed; repair or reinstall the service before retrying",
            expected_backend,
            receipt.platform_backend(),
        )));
    }
    let snapshot = codex_helper_core::read_managed_file_snapshot(
        path,
        MAX_SERVICE_DEFINITION_BYTES,
    )
    .map_err(|error| {
        CliError::Other(format!(
            "inspect the installed service definition {} before mutation: {error}. No service registration, definition, receipt, or runtime was changed; repair or reinstall the service before retrying",
            path.display(),
        ))
    })?;
    verify_unix_service_definition_snapshot(receipt, snapshot.bytes())
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn verify_current_service_receipt_snapshot(
    helper_home: &Path,
    expected: Option<&ServiceReceipt>,
    operation: &str,
) -> CliResult<()> {
    let actual = match read_service_receipt(helper_home) {
        Ok(receipt) => Some(receipt),
        Err(ServiceReceiptError::Missing) => None,
        Err(error) => {
            return Err(CliError::Other(format!(
                "refusing to {operation}: reread the current service receipt at the mutation boundary: {error}. No service registration, definition, receipt, or runtime was changed; repair or reinstall the service before retrying"
            )));
        }
    };
    if actual.as_ref() == expected {
        return Ok(());
    }
    Err(CliError::Other(format!(
        "refusing to {operation}: the service receipt changed after the transaction began. No service registration, definition, receipt, or runtime was changed; retry from a fresh service state"
    )))
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn verify_current_unix_service_definition_bytes(
    path: &Path,
    expected: Option<&[u8]>,
    operation: &str,
) -> CliResult<()> {
    let snapshot = codex_helper_core::read_managed_file_snapshot(
        path,
        MAX_SERVICE_DEFINITION_BYTES,
    )
    .map_err(|error| {
        CliError::Other(format!(
            "refusing to {operation}: reread the current service definition {} at the mutation boundary: {error}. No service registration, definition, receipt, or runtime was changed; repair or reinstall the service before retrying",
            path.display(),
        ))
    })?;
    if snapshot.bytes() == expected {
        return Ok(());
    }
    Err(CliError::Other(format!(
        "refusing to {operation}: the service definition {} changed after the transaction began. No service registration, definition, receipt, or runtime was changed; retry from a fresh service state",
        path.display(),
    )))
}

#[cfg(any(target_os = "macos", test))]
fn verify_launchd_registration_snapshot(
    output: Option<&str>,
    expected_definition_path: &Path,
) -> CliResult<()> {
    let Some(output) = output else {
        // `service stop` unloads the job while preserving its authoritative plist.
        return Ok(());
    };
    let registered_path = output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("path = ")
            .map(|path| path.trim_matches('"'))
    });
    let Some(registered_path) = registered_path else {
        return Err(CliError::Other(
            "refusing to mutate the loaded LaunchAgent because launchd did not expose its source plist path. No service registration, definition, receipt, or runtime was changed; unload the unverified job and rerun `codex-helper service install`"
                .to_string(),
        ));
    };
    let path_matches = service_paths_identify_same_location(
        Path::new(registered_path),
        expected_definition_path,
    )
    .map_err(|error| {
        CliError::Other(format!(
            "refusing to mutate the loaded LaunchAgent because its registered plist path is not a valid comparable authority: {error}. No service registration, definition, receipt, or runtime was changed; unload the external registration and rerun `codex-helper service install`"
        ))
    })?;
    if path_matches {
        return Ok(());
    }
    Err(CliError::Other(format!(
        "refusing to mutate the loaded LaunchAgent because launchd registered it from {}, not the receipt-authorized plist {}. No service registration, definition, receipt, or runtime was changed; unload the external registration and rerun `codex-helper service install`",
        registered_path,
        expected_definition_path.display(),
    )))
}

#[cfg(any(target_os = "linux", test))]
fn verify_systemd_registration_snapshot(
    load_state: &str,
    unit_file_state: &str,
    fragment_path: &str,
    drop_in_paths: &str,
    need_daemon_reload: &str,
    expected_definition_path: &Path,
) -> CliResult<()> {
    let fragment_matches = if load_state != "loaded"
        || unit_file_state != "enabled"
        || fragment_path.is_empty()
    {
        false
    } else {
        service_paths_identify_same_location(
            Path::new(fragment_path),
            expected_definition_path,
        )
        .map_err(|error| {
            CliError::Other(format!(
                "refusing to mutate the systemd user service because FragmentPath={fragment_path:?} is not a valid comparable authority: {error}. No service registration, definition, receipt, or runtime was changed; restore the matching unit, run `systemctl --user daemon-reload` and `systemctl --user enable {LINUX_UNIT_NAME}`, then retry `codex-helper service install`"
            ))
        })?
    };
    let registration_matches = load_state == "loaded"
        && unit_file_state == "enabled"
        && fragment_matches
        && drop_in_paths.is_empty()
        && need_daemon_reload == "no";
    if registration_matches {
        return Ok(());
    }
    Err(CliError::Other(format!(
        "refusing to mutate the systemd user service because its registration does not match the receipt-authorized unit (LoadState={load_state:?}, UnitFileState={unit_file_state:?}, FragmentPath={fragment_path:?}, DropInPaths={drop_in_paths:?}, NeedDaemonReload={need_daemon_reload:?}, expected={}). No service registration, definition, receipt, or runtime was changed; remove untrusted drop-ins, run `systemctl --user daemon-reload` and `systemctl --user enable {LINUX_UNIT_NAME}` after restoring the matching unit, then retry `codex-helper service install`",
        expected_definition_path.display(),
    )))
}

#[derive(Debug)]
struct ServiceInstallPreflight {
    installed_receipt: Option<ServiceReceipt>,
    platform_state: ServiceRuntimeState,
}

fn preflight_service_install_identity(
    installed_receipt: Result<ServiceReceipt, ServiceReceiptError>,
    platform_status: CliResult<ServiceStatus>,
    candidate_receipt: &ServiceReceipt,
) -> CliResult<ServiceInstallPreflight> {
    let installed_receipt = match installed_receipt {
        Ok(receipt) => receipt,
        Err(ServiceReceiptError::Missing) => {
            let platform_status = platform_status.map_err(|error| {
                CliError::Other(format!(
                    "cannot verify that the platform service registration is absent while the install receipt is missing: {error}. No service files were changed; inspect the platform service manager and repair or remove the stale registration before retrying"
                ))
            })?;
            let migratable_windows_legacy = platform_status.platform == ServicePlatform::Windows
                && platform_status.legacy_installation
                && candidate_receipt.platform_backend()
                    == ServicePlatformBackend::WindowsScheduledTask;
            if migratable_windows_legacy {
                return Ok(ServiceInstallPreflight {
                    installed_receipt: None,
                    platform_state: platform_status.state,
                });
            }
            if platform_status.installed
                || platform_status.state != ServiceRuntimeState::NotInstalled
            {
                return Err(CliError::Other(format!(
                    "refusing to install without a current receipt because the platform service registration is not proven absent (installed={}, state={:?}). No service files were changed; use the codex-helper version that created the registration to uninstall it, or repair the matching receipt before retrying",
                    platform_status.installed, platform_status.state,
                )));
            }
            return Ok(ServiceInstallPreflight {
                installed_receipt: None,
                platform_state: platform_status.state,
            });
        }
        Err(ServiceReceiptError::LegacySchema { schema_version }) => {
            return Err(CliError::Other(format!(
                "refusing to replace a service receipt with an unsupported legacy schema version {schema_version:?}. No service files were changed; use a compatible codex-helper version to uninstall or migrate that service before retrying"
            )));
        }
        Err(error) => {
            return Err(CliError::Other(format!(
                "cannot verify the existing service identity before installation: {error}. No service files were changed; repair the receipt or use a compatible codex-helper version before retrying"
            )));
        }
    };
    let platform_status = platform_status.map_err(|error| {
        CliError::Other(format!(
            "cannot verify the existing platform service registration before installation: {error}. No service files were changed; inspect the platform service manager before retrying"
        ))
    })?;
    let installed = CanonicalServiceInstallIdentity::from_receipt(&installed_receipt)?;
    let candidate = CanonicalServiceInstallIdentity::from_receipt(candidate_receipt)?;
    if installed.matches(&candidate)? {
        return Ok(ServiceInstallPreflight {
            installed_receipt: Some(installed_receipt),
            platform_state: platform_status.state,
        });
    }

    Err(CliError::Other(format!(
        "refusing to change the installed service identity during `service install` (installed: {}; requested: {}). No service files were changed. Run `codex-helper service uninstall` first so any matching Codex client switch is restored safely, then rerun the install command",
        installed.describe(),
        candidate.describe(),
    )))
}

fn begin_service_receipt_transaction_with_daemon_executable(
    options: &ServiceInstallOptions,
    daemon_executable: &Path,
) -> CliResult<(
    ServiceReceiptTransaction,
    ServiceReceipt,
    ServiceInstallPreflight,
)> {
    begin_service_receipt_transaction_with_daemon_executable_and_status(
        options,
        daemon_executable,
        status,
    )
}

fn begin_service_receipt_transaction_with_daemon_executable_and_status<F>(
    options: &ServiceInstallOptions,
    daemon_executable: &Path,
    read_platform_status: F,
) -> CliResult<(
    ServiceReceiptTransaction,
    ServiceReceipt,
    ServiceInstallPreflight,
)>
where
    F: FnOnce() -> CliResult<ServiceStatus>,
{
    let receipt = service_receipt_with_daemon_executable(options, daemon_executable)?;
    begin_service_receipt_transaction_with_candidate_and_status(
        options,
        receipt,
        read_platform_status,
    )
}

fn begin_service_receipt_transaction_with_candidate_and_status<F>(
    options: &ServiceInstallOptions,
    receipt: ServiceReceipt,
    read_platform_status: F,
) -> CliResult<(
    ServiceReceiptTransaction,
    ServiceReceipt,
    ServiceInstallPreflight,
)>
where
    F: FnOnce() -> CliResult<ServiceStatus>,
{
    let transaction = ServiceReceiptTransaction::begin_install_replacement(
        options.helper_home.clone(),
    )
    .map_err(|error| {
        CliError::Other(format!(
            "begin service receipt install replacement: {error}"
        ))
    })?;
    let installed_receipt = transaction
        .current()
        .and_then(|receipt| receipt.ok_or(ServiceReceiptError::Missing));
    let preflight =
        preflight_service_install_identity(installed_receipt, read_platform_status(), &receipt)?;
    Ok((transaction, receipt, preflight))
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

async fn verify_started_service_runtime_identity()
-> CliResult<codex_helper_core::credentials::CredentialAggregateReadiness> {
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
                return Ok(runtime.credential_readiness);
            }
            Err(error) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(CliError::Other(format!(
                        "started service did not publish its matching runtime identity within {} seconds: {error}",
                        STARTUP_TIMEOUT.as_secs(),
                    )));
                }
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn verify_started_service_runtime() -> CliResult<()> {
    let readiness = verify_started_service_runtime_identity()
        .await
        .map_err(|error| {
            CliError::Other(format!(
                "{error}; the service remains installed for diagnosis"
            ))
        })?;
    ensure_started_service_credential_readiness(readiness)
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

#[cfg(any(target_os = "macos", target_os = "linux", test))]
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
    let receipt = validate_service_receipt_for_uninstall(
        read_service_receipt(proxy_home_dir()),
        ServicePlatformBackend::current(),
    )?;
    verify_installed_service_definition_authority(&receipt)?;
    run_service_uninstall_with_switch_preflight(
        stop_first,
        reconcile_installed_service_switch,
        uninstall_platform_with_receipt,
    )
}

#[cfg(target_os = "macos")]
fn verify_installed_service_definition_authority(receipt: &ServiceReceipt) -> CliResult<()> {
    macos::verify_receipt_definition(receipt)
}

#[cfg(target_os = "linux")]
fn verify_installed_service_definition_authority(receipt: &ServiceReceipt) -> CliResult<()> {
    linux::verify_receipt_definition(receipt)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn verify_installed_service_definition_authority(_receipt: &ServiceReceipt) -> CliResult<()> {
    Ok(())
}

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
fn uninstall_platform_with_receipt(stop_first: bool) -> CliResult<()> {
    uninstall(stop_first)
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn uninstall_platform_with_receipt(stop_first: bool) -> CliResult<()> {
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

fn canonical_daemon_executable(path: &Path) -> CliResult<PathBuf> {
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        CliError::Other(format!(
            "resolve canonical daemon executable {}: {error}",
            path.display()
        ))
    })?;
    #[cfg(windows)]
    let canonical = without_windows_verbatim_prefix(&canonical);
    if !canonical.is_absolute() {
        return Err(CliError::Other(format!(
            "the canonical daemon executable is not absolute: {}",
            canonical.display()
        )));
    }
    Ok(canonical)
}

#[cfg(windows)]
fn without_windows_verbatim_prefix(path: &Path) -> PathBuf {
    if !windows_path_is_legacy_safe(path) {
        return path.to_path_buf();
    }

    path.to_str()
        .and_then(|path| path.get(4..))
        .map(PathBuf::from)
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(windows)]
fn windows_path_is_legacy_safe(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Component, Prefix};

    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return false;
    };
    if !matches!(prefix.kind(), Prefix::VerbatimDisk(_)) {
        return false;
    }

    for component in components {
        match component {
            Component::RootDir => {}
            Component::Normal(file_name) => {
                if !windows_legacy_filename_is_valid(file_name)
                    || windows_filename_is_reserved(file_name)
                {
                    return false;
                }
            }
            _ => return false,
        }
    }

    path.as_os_str().encode_wide().count() < 260
}

#[cfg(windows)]
fn windows_legacy_filename_is_valid(file_name: &OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;

    if file_name.encode_wide().count() > 255 {
        return false;
    }
    let Some(file_name) = file_name.to_str() else {
        return false;
    };
    let bytes = file_name.as_bytes();
    !bytes.is_empty()
        && !bytes.iter().any(|&byte| {
            matches!(
                byte,
                0..=31 | b'<' | b'>' | b':' | b'"' | b'/' | b'\\' | b'|' | b'?' | b'*'
            )
        })
        && !matches!(bytes.last(), Some(b' ' | b'.'))
}

#[cfg(windows)]
fn windows_filename_is_reserved(file_name: &OsStr) -> bool {
    const RESERVED_NAMES: [&str; 28] = [
        "AUX", "NUL", "PRN", "CON", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "COM¹",
        "COM²", "COM³", "LPT¹", "LPT²", "LPT³",
    ];

    Path::new(file_name)
        .file_stem()
        .and_then(OsStr::to_str)
        .map(|stem| stem.trim_end_matches([' ', '.']))
        .is_some_and(|stem| {
            RESERVED_NAMES
                .iter()
                .any(|reserved| stem.eq_ignore_ascii_case(reserved))
        })
}

fn select_service_executable_candidate_with<F>(current: &Path, is_file: F) -> PathBuf
where
    F: FnOnce(&Path) -> bool,
{
    let sibling_name = current
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| {
            if name.eq_ignore_ascii_case("ch.exe") {
                Some("codex-helper.exe")
            } else if name.eq_ignore_ascii_case("ch") {
                Some("codex-helper")
            } else {
                None
            }
        });
    let Some(sibling_name) = sibling_name else {
        return current.to_path_buf();
    };
    let sibling = current.with_file_name(sibling_name);
    if is_file(&sibling) {
        sibling
    } else {
        current.to_path_buf()
    }
}

fn service_executable(current: &Path) -> CliResult<PathBuf> {
    let selected =
        select_service_executable_candidate_with(current, |candidate| candidate.is_file());
    canonical_daemon_executable(&selected)
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
async fn install(
    options: ServiceInstallOptions,
) -> CliResult<Option<codex_helper_core::credentials::CredentialAggregateReadiness>> {
    windows::install(options).await
}

#[cfg(target_os = "macos")]
async fn install(
    options: ServiceInstallOptions,
) -> CliResult<Option<codex_helper_core::credentials::CredentialAggregateReadiness>> {
    macos::install(options).await
}

#[cfg(target_os = "linux")]
async fn install(
    options: ServiceInstallOptions,
) -> CliResult<Option<codex_helper_core::credentials::CredentialAggregateReadiness>> {
    linux::install(options).await
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
async fn install(
    _options: ServiceInstallOptions,
) -> CliResult<Option<codex_helper_core::credentials::CredentialAggregateReadiness>> {
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

pub(crate) fn current_service_runtime_state() -> CliResult<ServiceRuntimeState> {
    status().map(|status| status.state)
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
    use std::time::Instant;

    use windows_service::define_windows_service;
    use windows_service::service::{
        Service, ServiceAccess, ServiceConfig, ServiceControl, ServiceControlAccept,
        ServiceExitCode, ServiceStartType, ServiceState, ServiceStatus as WindowsStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{
        self, ServiceControlHandlerResult, ServiceStatusHandle,
    };
    use windows_service::service_dispatcher;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    use super::*;

    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;
    const TASK_STOP_TIMEOUT: Duration = Duration::from_secs(15);
    const TASK_STOP_POLL_INTERVAL: Duration = Duration::from_millis(100);
    static SERVICE_OPTIONS: OnceLock<ServiceInstallOptions> = OnceLock::new();

    define_windows_service!(service_entry, service_main);

    #[derive(Debug, Clone)]
    struct OwnedTaskSnapshot {
        record: WindowsTaskRecord,
        definition: Vec<u8>,
    }

    #[derive(Debug, Clone)]
    struct LegacyScmSnapshot {
        was_running: bool,
        definition: LegacyWindowsScmDefinition,
        invocation: LegacyWindowsServiceInvocation,
    }

    struct LegacyScmRetirementFailure {
        error: CliError,
        preserve_replacement: bool,
    }

    impl LegacyScmRetirementFailure {
        fn rollback_safe(error: CliError) -> Self {
            Self {
                error,
                preserve_replacement: false,
            }
        }

        fn commit_unknown(error: CliError) -> Self {
            Self {
                error,
                preserve_replacement: true,
            }
        }
    }

    struct WindowsInstallContext {
        executable: PathBuf,
        user_sid: String,
        scoped_task_name: String,
        definition_path: PathBuf,
        definition_document: Vec<u8>,
        scoped_snapshot: Option<OwnedTaskSnapshot>,
        scoped_requires_end: bool,
        fixed_snapshot: Option<OwnedTaskSnapshot>,
        legacy_scm: Option<LegacyScmSnapshot>,
    }

    struct NativeWindowsInstallBackend {
        options: ServiceInstallOptions,
        context: Option<WindowsInstallContext>,
        scoped_task_changed: bool,
        scoped_task_stopped: bool,
        fixed_task_stopped: bool,
        fixed_task_changed: bool,
        legacy_scm_stopped: bool,
        preserve_scoped_task: bool,
        definition_transaction: Option<codex_helper_core::ManagedFileTransaction>,
        receipt_transaction: Option<ServiceReceiptTransaction>,
        receipt: Option<ServiceReceipt>,
        registered_scoped_snapshot: Option<OwnedTaskSnapshot>,
    }

    impl NativeWindowsInstallBackend {
        fn new(options: ServiceInstallOptions) -> Self {
            Self {
                options,
                context: None,
                scoped_task_changed: false,
                scoped_task_stopped: false,
                fixed_task_stopped: false,
                fixed_task_changed: false,
                legacy_scm_stopped: false,
                preserve_scoped_task: false,
                definition_transaction: None,
                receipt_transaction: None,
                receipt: None,
                registered_scoped_snapshot: None,
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
            let current_executable = current_executable()?;
            let executable = service_executable(&current_executable)?;
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
            let (receipt_transaction, receipt, install_preflight) =
                begin_service_receipt_transaction_with_daemon_executable(
                    &self.options,
                    &executable,
                )?;

            let (scoped_snapshot, scoped_requires_end) =
                match query_scheduled_task(&scoped_task_name)? {
                    Some(record) => {
                        verify_existing_windows_task_for_replacement(
                            &record,
                            &scoped_task_name,
                            &user_sid,
                            &current_executable,
                            install_preflight.installed_receipt.as_ref(),
                        )?;
                        let requires_end = scheduled_task_requires_end(&record)?;
                        (Some(snapshot_owned_task(record)?), requires_end)
                    }
                    None => (None, false),
                };
            let fixed_snapshot = match query_scheduled_task(WINDOWS_TASK_BASENAME)? {
                Some(record) if windows_task_owner_matches(&record, &user_sid) => {
                    let invocation = verify_legacy_fixed_windows_task_record(
                        &record,
                        &user_sid,
                        &current_executable,
                    )?;
                    if invocation.matches_install(&self.options) {
                        let _ = scheduled_task_requires_end(&record)?;
                        Some(snapshot_owned_task(record)?)
                    } else if invocation.conflicts_with_install(&self.options) {
                        return Err(CliError::Other(
                            "the verified legacy fixed-name task listens on the requested port but belongs to a different service or home; uninstall it with the codex-helper version that created it before retrying"
                                .to_string(),
                        ));
                    } else {
                        None
                    }
                }
                Some(_) | None => None,
            };
            let legacy_scm = probe_legacy_scm(
                "preflight the legacy LocalSystem SCM service for migration",
                &current_executable,
            )?
            .map(|snapshot| {
                if snapshot.invocation.matches_install(&self.options) {
                    Ok(Some(snapshot))
                } else if snapshot.invocation.conflicts_with_install(&self.options) {
                    Err(CliError::Other(
                        "the verified legacy SCM service listens on the requested port but belongs to a different service or home; uninstall it with the codex-helper version that created it before retrying"
                            .to_string(),
                    ))
                } else {
                    Ok(None)
                }
            })
            .transpose()?
            .flatten();
            let definition_transaction = codex_helper_core::ManagedFileTransaction::begin(
                definition_path.clone(),
                MAX_SERVICE_DEFINITION_BYTES,
            )
            .map_err(|error| {
                CliError::Other(format!("begin Windows definition transaction: {error}"))
            })?;
            prepare_service_switch_for_install(&self.options, &install_preflight)?;
            self.context = Some(WindowsInstallContext {
                executable,
                user_sid,
                scoped_task_name,
                definition_path,
                definition_document,
                scoped_snapshot,
                scoped_requires_end,
                fixed_snapshot,
                legacy_scm,
            });
            self.definition_transaction = Some(definition_transaction);
            self.receipt_transaction = Some(receipt_transaction);
            self.receipt = Some(receipt);
            Ok(())
        }

        fn stop_existing_scoped_task(&mut self) -> CliResult<()> {
            let (task_name, snapshot, preflight_requires_end) = {
                let context = self.context()?;
                (
                    context.scoped_task_name.clone(),
                    context.scoped_snapshot.clone(),
                    context.scoped_requires_end,
                )
            };
            let current = query_scheduled_task(&task_name)?;
            let (snapshot, current) = match (snapshot.as_ref(), current.as_ref()) {
                (None, None) => return Ok(()),
                (None, Some(_)) => {
                    return Err(CliError::Other(
                        "the SID-scoped Windows task appeared after installation preflight; retry after confirming no other service command is running"
                            .to_string(),
                    ));
                }
                (Some(_), None) => {
                    return Err(CliError::Other(
                        "the SID-scoped Windows task disappeared after installation preflight; retry after inspecting Task Scheduler"
                            .to_string(),
                    ));
                }
                (Some(snapshot), Some(current)) => (snapshot, current),
            };
            require_task_owner(current, &snapshot.record.owner_sid, "SID-scoped")?;
            let current_requires_end = scheduled_task_requires_end(current)?;
            self.scoped_task_stopped = preflight_requires_end || current_requires_end;
            if !current_requires_end {
                return Ok(());
            }
            end_unchanged_scheduled_task(snapshot, "existing SID-scoped").map_err(|error| {
                CliError::Other(format!(
                    "stop the existing SID-scoped Windows task before replacing it: {error}"
                ))
            })
        }

        fn stop_legacy_runtimes(&mut self) -> CliResult<()> {
            if let Some(snapshot) = self.context()?.fixed_snapshot.clone() {
                let current =
                    query_scheduled_task(&snapshot.record.task_name)?.ok_or_else(|| {
                        CliError::Other(
                        "the verified legacy fixed-name Windows task disappeared before migration"
                            .to_string(),
                    )
                    })?;
                require_task_snapshot_unchanged(&current, &snapshot, "legacy fixed-name")?;
                let preflight_running = scheduled_task_requires_end(&snapshot.record)?;
                let current_running = scheduled_task_requires_end(&current)?;
                self.fixed_task_stopped = preflight_running || current_running;
                if current_running {
                    end_unchanged_scheduled_task(&snapshot, "legacy fixed-name")
                        .map_err(|error| {
                            CliError::Other(format!(
                                "stop the legacy fixed-name Windows task before starting its replacement: {error}"
                            ))
                        })?;
                }
            }

            if let Some(snapshot) = self
                .context()?
                .legacy_scm
                .clone()
                .filter(|snapshot| snapshot.was_running)
            {
                // Mark the state before Stop because Windows can report an error after accepting
                // the control request. Rollback will re-query the signed legacy definition.
                self.legacy_scm_stopped = true;
                stop_legacy_scm_service(&snapshot).map_err(|error| {
                    CliError::Other(format!(
                        "stop the legacy SCM runtime before starting its replacement: {error}"
                    ))
                })?;
            }
            Ok(())
        }

        fn register_scoped_task(&mut self) -> CliResult<()> {
            let context = self.context()?;
            if let Some(record) = query_scheduled_task(&context.scoped_task_name)? {
                let Some(snapshot) = context.scoped_snapshot.as_ref() else {
                    return Err(CliError::Other(
                        "the SID-scoped Windows task appeared after installation preflight; its definition and receipt were not replaced"
                            .to_string(),
                    ));
                };
                require_task_snapshot_unchanged(&record, snapshot, "existing SID-scoped")?;
                if scheduled_task_requires_end(&record)? {
                    return Err(CliError::Other(format!(
                        "the SID-scoped Windows task '{}' restarted after it was stopped; its definition and receipt were not replaced",
                        record.task_name
                    )));
                }
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
            let (task_name, user_sid, executable) = {
                let context = self.context()?;
                (
                    context.scoped_task_name.clone(),
                    context.user_sid.clone(),
                    context.executable.clone(),
                )
            };
            let record = query_scheduled_task(&task_name)?.ok_or_else(|| {
                CliError::Other(format!(
                    "the newly registered Windows task '{}' was not found during verification",
                    task_name
                ))
            })?;
            verify_windows_task_record(&record, &task_name, &user_sid, &executable, &self.options)?;
            self.registered_scoped_snapshot = Some(snapshot_owned_task(record)?);
            Ok(())
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
            let current = query_scheduled_task(&snapshot.record.task_name)?.ok_or_else(|| {
                CliError::Other(
                    "the verified legacy fixed-name Windows task disappeared before migration"
                        .to_string(),
                )
            })?;
            require_task_snapshot_unchanged(&current, &snapshot, "legacy fixed-name")?;
            if scheduled_task_requires_end(&current)? {
                return Err(CliError::Other(format!(
                    "the legacy fixed-name Windows task '{}' restarted after handoff; its registration was left in place",
                    current.task_name
                )));
            }
            // The delete command can commit even when its process reports an error. Treat the
            // registration as changed until rollback proves or restores its exact snapshot.
            self.fixed_task_changed = true;
            delete_unchanged_scheduled_task(&snapshot, "legacy fixed-name")
        }

        fn retire_legacy_scm(&mut self) -> CliResult<()> {
            let Some(snapshot) = self.context()?.legacy_scm.clone() else {
                return Ok(());
            };
            match retire_legacy_scm_service(&snapshot) {
                Ok(()) => Ok(()),
                Err(failure) => {
                    self.preserve_scoped_task = failure.preserve_replacement;
                    Err(failure.error)
                }
            }
        }

        fn rollback_receipt(&mut self) -> CliResult<()> {
            self.receipt_transaction_mut()?.rollback().map_err(|error| {
                CliError::Other(format!("restore previous Windows service receipt: {error}"))
            })
        }

        fn rollback(&mut self) -> CliResult<()> {
            let mut failures = Vec::new();
            let (
                scoped_task_name,
                definition_path,
                scoped_snapshot,
                fixed_snapshot,
                legacy_snapshot,
            ) = {
                let context = self.context()?;
                (
                    context.scoped_task_name.clone(),
                    context.definition_path.clone(),
                    context.scoped_snapshot.clone(),
                    context.fixed_snapshot.clone(),
                    context.legacy_scm.clone(),
                )
            };

            // The replacement runtime must be stopped before any previous runtime is restarted;
            // both generations listen on the same proxy address.
            let mut scoped_registration_needs_restore = self.scoped_task_changed;
            if self.scoped_task_changed
                && let Some(current) = query_scheduled_task(&scoped_task_name)?
            {
                let current_is_previous = scoped_snapshot.as_ref().is_some_and(|snapshot| {
                    require_task_snapshot_unchanged(&current, snapshot, "previous SID-scoped")
                        .is_ok()
                });
                if current_is_previous {
                    scoped_registration_needs_restore = false;
                } else {
                    let Some(replacement) = self.registered_scoped_snapshot.as_ref() else {
                        self.preserve_scoped_task = true;
                        failures.push(
                            "the replacement SID-scoped task was not fully verified; rollback left it untouched"
                                .to_string(),
                        );
                        return Err(CliError::Other(failures.join("; ")));
                    };
                    if require_task_snapshot_unchanged(
                        &current,
                        replacement,
                        "replacement SID-scoped",
                    )
                    .is_err()
                    {
                        self.preserve_scoped_task = true;
                        failures.push(
                            "the SID-scoped task changed after migration verification; rollback left it untouched"
                                .to_string(),
                        );
                    } else {
                        if scheduled_task_requires_end(&current)?
                            && let Err(error) =
                                end_unchanged_scheduled_task(replacement, "replacement SID-scoped")
                        {
                            self.preserve_scoped_task = true;
                            failures.push(format!(
                                "stop the replacement SID-scoped task before rollback: {error}"
                            ));
                        }
                        if !self.preserve_scoped_task
                            && let Err(error) = delete_unchanged_scheduled_task(
                                replacement,
                                "replacement SID-scoped",
                            )
                        {
                            self.preserve_scoped_task = true;
                            failures.push(format!(
                                "remove the replacement SID-scoped task before rollback: {error}"
                            ));
                        }
                    }
                }
            }
            if self.preserve_scoped_task {
                return Err(CliError::Other(failures.join("; ")));
            }

            // Restore every registration before restarting any runtime.
            if scoped_registration_needs_restore {
                let result = match scoped_snapshot.as_ref() {
                    Some(snapshot) => restore_task_snapshot(snapshot, &definition_path, false),
                    None => Ok(()),
                };
                if let Err(error) = result {
                    failures.push(format!(
                        "restore the previous SID-scoped task registration: {error}"
                    ));
                }
            }
            if self.fixed_task_changed
                && let Some(snapshot) = fixed_snapshot.as_ref()
                && let Err(error) = restore_task_snapshot(snapshot, &definition_path, false)
            {
                failures.push(format!(
                    "restore the legacy fixed-name task registration: {error}"
                ));
            }
            if let Err(error) = self.definition_transaction_mut()?.rollback() {
                failures.push(format!("restore the Windows task definition: {error}"));
            }

            if self.scoped_task_stopped
                && let Some(snapshot) = scoped_snapshot.as_ref()
                && let Err(error) = restart_restored_task(snapshot)
            {
                failures.push(format!("restart the previous SID-scoped task: {error}"));
            }
            if self.fixed_task_stopped
                && let Some(snapshot) = fixed_snapshot.as_ref()
                && let Err(error) = restart_restored_task(snapshot)
            {
                failures.push(format!("restart the legacy fixed-name task: {error}"));
            }
            if self.legacy_scm_stopped
                && let Some(snapshot) = legacy_snapshot
                    .as_ref()
                    .filter(|snapshot| snapshot.was_running)
            {
                match start_legacy_scm_service(snapshot) {
                    Ok(()) => self.legacy_scm_stopped = false,
                    Err(error) => {
                        failures.push(format!("restart the legacy SCM service: {error}"));
                    }
                }
            }
            if failures.is_empty() {
                Ok(())
            } else {
                Err(CliError::Other(failures.join("; ")))
            }
        }

        fn rollback_preserved_replacement(&self) -> bool {
            self.preserve_scoped_task
        }

        fn start_scoped_task(&mut self) -> CliResult<()> {
            let (task_name, user_sid, executable) = {
                let context = self.context()?;
                (
                    context.scoped_task_name.clone(),
                    context.user_sid.clone(),
                    context.executable.clone(),
                )
            };
            let record = query_scheduled_task(&task_name)?.ok_or_else(|| {
                CliError::Other(format!(
                    "the verified Windows task '{}' disappeared before start",
                    task_name
                ))
            })?;
            verify_windows_task_record(&record, &task_name, &user_sid, &executable, &self.options)?;
            let registered = self.registered_scoped_snapshot.as_ref().ok_or_else(|| {
                CliError::Other(
                    "the replacement SID-scoped task has no verified registration snapshot"
                        .to_string(),
                )
            })?;
            require_task_snapshot_unchanged(&record, registered, "replacement SID-scoped")?;
            // `scoped_task_changed` is set before registration, so rollback always re-queries and
            // stops a possibly started replacement even if schtasks accepts /Run then errors.
            run_unchanged_scheduled_task(registered, "replacement SID-scoped")?;
            Ok(())
        }

        async fn verify_started_runtime_identity(
            &mut self,
        ) -> CliResult<codex_helper_core::credentials::CredentialAggregateReadiness> {
            verify_started_service_runtime_identity().await
        }
    }

    struct WindowsUninstallContext {
        scoped_task_name: String,
        scoped_snapshot: Option<OwnedTaskSnapshot>,
        fixed_snapshot: Option<OwnedTaskSnapshot>,
        ignore_foreign_fixed_task: bool,
        legacy_scm: Option<LegacyScmSnapshot>,
        definition_path: PathBuf,
    }

    struct NativeWindowsUninstallBackend {
        context: WindowsUninstallContext,
        stop_requested: bool,
        scoped_task_stopped: bool,
        scoped_task_removed: bool,
        fixed_task_stopped: bool,
        fixed_task_removed: bool,
        legacy_scm_stopped: bool,
        legacy_scm_retirement_attempted: bool,
        definition: codex_helper_core::ManagedFileTransaction,
        receipt: ServiceReceiptTransaction,
    }

    impl NativeWindowsUninstallBackend {
        fn new() -> CliResult<Self> {
            let helper_home = proxy_home_dir();
            let definition_path = task_definition_path(&helper_home);
            let definition = codex_helper_core::ManagedFileTransaction::begin(
                definition_path.clone(),
                MAX_SERVICE_DEFINITION_BYTES,
            )
            .map_err(|error| {
                CliError::Other(format!(
                    "begin Windows task definition removal transaction: {error}"
                ))
            })?;
            let receipt =
                ServiceReceiptTransaction::begin(helper_home.clone()).map_err(|error| {
                    CliError::Other(format!(
                        "begin Windows service receipt removal transaction: {error}"
                    ))
                })?;
            let installed_receipt = receipt
                .current()
                .map_err(|error| {
                    CliError::Other(format!(
                        "read the installed Windows service receipt before uninstall: {error}"
                    ))
                })?
                .ok_or_else(|| {
                    CliError::Other(
                    "the installed Windows service receipt disappeared before uninstall preflight"
                        .to_string(),
                )
                })?;
            let options = service_install_options_from_receipt(&installed_receipt, false)?;
            let executable = current_executable()?;
            let (user_sid, scoped_task_name) = current_task_identity()?;
            let scoped_snapshot = match query_scheduled_task(&scoped_task_name)? {
                Some(record) => {
                    verify_installed_windows_task_record(
                        &record,
                        &scoped_task_name,
                        &user_sid,
                        &installed_receipt,
                        &executable,
                        &options,
                    )?;
                    Some(snapshot_owned_task(record)?)
                }
                None => None,
            };
            let (fixed_snapshot, ignore_foreign_fixed_task) = match query_scheduled_task(
                WINDOWS_TASK_BASENAME,
            )? {
                Some(record) if windows_task_owner_matches(&record, &user_sid) => {
                    let invocation =
                        verify_legacy_fixed_windows_task_record(&record, &user_sid, &executable)?;
                    if invocation.matches_install(&options) {
                        (Some(snapshot_owned_task(record)?), false)
                    } else if invocation.conflicts_with_install(&options) {
                        return Err(CliError::Other(
                                "the legacy fixed-name task uses the installed service port but does not match the signed service receipt; no task was removed"
                                    .to_string(),
                            ));
                    } else {
                        (None, true)
                    }
                }
                Some(_) => (None, true),
                None => (None, false),
            };
            let legacy_scm = probe_legacy_scm(
                "preflight the legacy LocalSystem SCM service for uninstall",
                &executable,
            )?
            .map(|snapshot| {
                if snapshot.invocation.matches_install(&options) {
                    Ok(Some(snapshot))
                } else if snapshot.invocation.conflicts_with_install(&options) {
                    Err(CliError::Other(
                        "the legacy SCM service uses the installed service port but does not match the signed service receipt; no service was removed"
                            .to_string(),
                    ))
                } else {
                    Ok(None)
                }
            })
            .transpose()?
            .flatten();
            Ok(Self {
                context: WindowsUninstallContext {
                    scoped_task_name,
                    scoped_snapshot,
                    fixed_snapshot,
                    ignore_foreign_fixed_task,
                    legacy_scm,
                    definition_path,
                },
                stop_requested: false,
                scoped_task_stopped: false,
                scoped_task_removed: false,
                fixed_task_stopped: false,
                fixed_task_removed: false,
                legacy_scm_stopped: false,
                legacy_scm_retirement_attempted: false,
                definition,
                receipt,
            })
        }

        fn stop_task(
            snapshot: Option<&OwnedTaskSnapshot>,
            stopped: &mut bool,
            description: &str,
        ) -> CliResult<()> {
            let Some(snapshot) = snapshot else {
                return Ok(());
            };
            let current = query_scheduled_task(&snapshot.record.task_name)?.ok_or_else(|| {
                CliError::Other(format!(
                    "the {description} Windows task disappeared after uninstall preflight; no service files were removed"
                ))
            })?;
            require_task_snapshot_unchanged(&current, snapshot, description)?;
            if !scheduled_task_requires_end(&current)? {
                return Ok(());
            }
            // Mark the state before issuing /End because a command or verification error can be
            // returned after the runtime has already stopped.
            *stopped = true;
            end_unchanged_scheduled_task(snapshot, description).map_err(|error| {
                CliError::Other(format!(
                    "stop the {description} Windows task before uninstalling it: {error}"
                ))
            })
        }

        fn remove_task(
            task_name: &str,
            snapshot: Option<&OwnedTaskSnapshot>,
            stop_requested: bool,
            removed: &mut bool,
            description: &str,
        ) -> CliResult<()> {
            let current = query_scheduled_task(task_name)?;
            let Some(snapshot) = snapshot else {
                return if current.is_none() {
                    Ok(())
                } else {
                    Err(CliError::Other(format!(
                        "the {description} Windows task appeared after uninstall preflight; no unverified task was removed"
                    )))
                };
            };
            let current = current.as_ref().ok_or_else(|| {
                CliError::Other(format!(
                    "the {description} Windows task disappeared after uninstall preflight; no service files were removed"
                ))
            })?;
            require_task_snapshot_unchanged(current, snapshot, description)?;
            if stop_requested && scheduled_task_requires_end(current)? {
                return Err(CliError::Other(format!(
                    "the {description} Windows task restarted after it was stopped; its registration and service files were left in place"
                )));
            }
            // Treat the mutation as commit-state-unknown until the absence read-back succeeds.
            *removed = true;
            delete_unchanged_scheduled_task(snapshot, description).map_err(|error| {
                CliError::Other(format!(
                    "remove the {description} Windows task registration: {error}"
                ))
            })
        }
    }

    impl WindowsUninstallTransactionBackend for NativeWindowsUninstallBackend {
        fn stop_and_verify(&mut self) -> CliResult<()> {
            self.stop_requested = true;
            Self::stop_task(
                self.context.scoped_snapshot.as_ref(),
                &mut self.scoped_task_stopped,
                "SID-scoped",
            )?;
            if !self.context.ignore_foreign_fixed_task {
                Self::stop_task(
                    self.context.fixed_snapshot.as_ref(),
                    &mut self.fixed_task_stopped,
                    "legacy fixed-name",
                )?;
            }
            if let Some(snapshot) = self
                .context
                .legacy_scm
                .as_ref()
                .filter(|snapshot| snapshot.was_running)
            {
                // As with scheduled tasks, an error can arrive after the stop took effect.
                self.legacy_scm_stopped = true;
                stop_legacy_scm_service(snapshot).map_err(|error| {
                    CliError::Other(format!(
                        "stop the legacy SCM service before uninstalling it: {error}"
                    ))
                })?;
            }
            Ok(())
        }

        fn remove_scoped_task(&mut self) -> CliResult<()> {
            Self::remove_task(
                &self.context.scoped_task_name,
                self.context.scoped_snapshot.as_ref(),
                self.stop_requested,
                &mut self.scoped_task_removed,
                "SID-scoped",
            )
        }

        fn remove_fixed_task(&mut self) -> CliResult<()> {
            if self.context.ignore_foreign_fixed_task {
                return Ok(());
            }
            Self::remove_task(
                WINDOWS_TASK_BASENAME,
                self.context.fixed_snapshot.as_ref(),
                self.stop_requested,
                &mut self.fixed_task_removed,
                "legacy fixed-name",
            )
        }

        fn remove_definition(&mut self) -> CliResult<()> {
            self.definition.remove().map_err(|error| {
                CliError::Other(format!(
                    "remove Windows task definition {}: {error}",
                    self.context.definition_path.display()
                ))
            })?;
            if self.definition.current().bytes().is_some() {
                return Err(CliError::Other(format!(
                    "Windows task definition {} still exists after removal",
                    self.context.definition_path.display()
                )));
            }
            Ok(())
        }

        fn remove_receipt(&mut self) -> CliResult<()> {
            self.receipt.remove().map_err(|error| {
                CliError::Other(format!(
                    "remove service receipt after Windows task removal: {error}"
                ))
            })
        }

        fn retire_legacy_scm(&mut self) -> CliResult<()> {
            self.legacy_scm_retirement_attempted = true;
            let Some(snapshot) = self.context.legacy_scm.as_ref() else {
                return match probe_legacy_scm(
                    "verify that no legacy LocalSystem SCM service appeared during uninstall",
                    &current_executable()?,
                )? {
                    None => Ok(()),
                    Some(_) => Err(CliError::Other(
                        "a legacy SCM service appeared after uninstall preflight and was left untouched"
                            .to_string(),
                    )),
                };
            };
            // This is the final commit step. In keep-running mode the SCM definition is removed
            // without stopping its current process, preserving the explicit detached-runtime
            // contract.
            remove_legacy_scm_service(self.stop_requested, snapshot)
        }

        fn rollback(&mut self) -> CliResult<()> {
            let mut failures = Vec::new();
            if let Err(error) = self.receipt.rollback() {
                failures.push(format!("restore the Windows service receipt: {error}"));
            }
            if let Err(error) = self.definition.rollback() {
                failures.push(format!("restore the Windows task definition: {error}"));
            }
            // Restore both registrations before starting either previous runtime. This avoids a
            // failed first start preventing the second task definition from being recovered.
            let mut fixed_task_restored = false;
            if (self.fixed_task_removed || self.fixed_task_stopped)
                && let Some(snapshot) = self.context.fixed_snapshot.as_ref()
            {
                match restore_task_snapshot(snapshot, &self.context.definition_path, false) {
                    Ok(()) => fixed_task_restored = true,
                    Err(error) => failures.push(format!(
                        "restore the legacy fixed-name task registration: {error}"
                    )),
                }
            }
            let mut scoped_task_restored = false;
            if (self.scoped_task_removed || self.scoped_task_stopped)
                && let Some(snapshot) = self.context.scoped_snapshot.as_ref()
            {
                match restore_task_snapshot(snapshot, &self.context.definition_path, false) {
                    Ok(()) => scoped_task_restored = true,
                    Err(error) => {
                        failures.push(format!("restore the SID-scoped task registration: {error}"))
                    }
                }
            }
            if fixed_task_restored
                && self.fixed_task_stopped
                && let Some(snapshot) = self.context.fixed_snapshot.as_ref()
                && let Err(error) = restart_restored_task(snapshot)
            {
                failures.push(format!(
                    "restore the legacy fixed-name task runtime state: {error}"
                ));
            }
            if scoped_task_restored
                && self.scoped_task_stopped
                && let Some(snapshot) = self.context.scoped_snapshot.as_ref()
                && let Err(error) = restart_restored_task(snapshot)
            {
                failures.push(format!(
                    "restore the SID-scoped task runtime state: {error}"
                ));
            }
            if self.legacy_scm_stopped || self.legacy_scm_retirement_attempted {
                match self.context.legacy_scm.as_ref() {
                    Some(snapshot) => {
                        if let Err(error) = restore_legacy_scm_snapshot(snapshot) {
                            failures.push(format!(
                                "restore and verify the legacy SCM definition and runtime state: {error}"
                            ));
                        }
                    }
                    None if self.legacy_scm_retirement_attempted => {
                        match probe_legacy_scm(
                            "verify legacy LocalSystem SCM absence during uninstall rollback",
                            &current_executable()?,
                        ) {
                            Ok(None) => {}
                            Ok(Some(_)) => failures.push(
                                "a legacy SCM service appeared concurrently and was left untouched"
                                    .to_string(),
                            ),
                            Err(error) => failures.push(format!(
                                "verify that the legacy SCM definition remains absent: {error}"
                            )),
                        }
                    }
                    None => {}
                }
            }
            if failures.is_empty() {
                Ok(())
            } else {
                Err(CliError::Other(failures.join("; ")))
            }
        }
    }

    pub(super) async fn install(
        options: ServiceInstallOptions,
    ) -> CliResult<Option<codex_helper_core::credentials::CredentialAggregateReadiness>> {
        let start = options.start;
        run_windows_install_transaction(&mut NativeWindowsInstallBackend::new(options), start).await
    }

    pub(super) fn uninstall(stop_first: bool) -> CliResult<()> {
        run_windows_uninstall_transaction(&mut NativeWindowsUninstallBackend::new()?, stop_first)
    }

    fn installed_service_registration() -> CliResult<(ServiceReceipt, ServiceInstallOptions)> {
        let receipt = validate_service_receipt_for_uninstall(
            read_service_receipt(proxy_home_dir()),
            Some(ServicePlatformBackend::WindowsScheduledTask),
        )?;
        let options = service_install_options_from_receipt(&receipt, false)?;
        Ok((receipt, options))
    }

    fn matching_installed_task_snapshot(
        receipt: &ServiceReceipt,
        options: &ServiceInstallOptions,
    ) -> CliResult<Option<OwnedTaskSnapshot>> {
        let (user_sid, scoped_task_name) = current_task_identity()?;
        let executable = current_executable()?;
        if let Some(record) = query_scheduled_task(&scoped_task_name)? {
            verify_installed_windows_task_record(
                &record,
                &scoped_task_name,
                &user_sid,
                receipt,
                &executable,
                options,
            )?;
            return snapshot_owned_task(record).map(Some);
        }
        let Some(record) = query_scheduled_task(WINDOWS_TASK_BASENAME)? else {
            return Ok(None);
        };
        if !windows_task_owner_matches(&record, &user_sid) {
            return Ok(None);
        }
        let invocation = verify_legacy_fixed_windows_task_record(&record, &user_sid, &executable)?;
        if !invocation.matches_install(options) {
            return Err(CliError::Other(
                "the legacy fixed-name task does not match the signed service receipt and was left untouched"
                    .to_string(),
            ));
        }
        snapshot_owned_task(record).map(Some)
    }

    pub(super) fn start() -> CliResult<()> {
        let (receipt, options) = installed_service_registration()?;
        if let Some(snapshot) = matching_installed_task_snapshot(&receipt, &options)? {
            return run_unchanged_scheduled_task(&snapshot, "installed");
        }
        Err(CliError::Other(
            "the current user's Windows task is not installed; run `codex-helper service install` to migrate any legacy SCM service"
                .to_string(),
        ))
    }

    pub(super) fn stop() -> CliResult<()> {
        let (receipt, options) = installed_service_registration()?;
        if let Some(snapshot) = matching_installed_task_snapshot(&receipt, &options)? {
            return if scheduled_task_requires_end(&snapshot.record)? {
                end_unchanged_scheduled_task(&snapshot, "installed")
            } else {
                Ok(())
            };
        }
        let executable = current_executable()?;
        let Some(snapshot) = probe_legacy_scm(
            "verify the legacy LocalSystem SCM service before stopping it",
            &executable,
        )?
        else {
            return Ok(());
        };
        if !snapshot.invocation.matches_install(&options) {
            return Err(CliError::Other(
                "the legacy SCM service does not match the signed service receipt and was left untouched"
                    .to_string(),
            ));
        }
        stop_legacy_scm_service(&snapshot)
    }

    pub(super) fn status() -> CliResult<ServiceStatus> {
        let (user_sid, scoped_task_name) = current_task_identity()?;
        if let Some(record) = query_scheduled_task(&scoped_task_name)? {
            let state = match record.state {
                4 => ServiceRuntimeState::Running,
                2 => ServiceRuntimeState::Starting,
                1 | 3 => ServiceRuntimeState::Stopped,
                _ => ServiceRuntimeState::Unknown,
            };
            let validation = read_service_receipt(proxy_home_dir())
                .map_err(|error| error.to_string())
                .and_then(|receipt| {
                    let has_receipt_executable_authority = receipt.daemon_executable().is_some();
                    let options = service_install_options_from_receipt(&receipt, false)
                        .map_err(|error| error.to_string())?;
                    let current_executable =
                        current_executable().map_err(|error| error.to_string())?;
                    verify_installed_windows_task_record(
                        &record,
                        &scoped_task_name,
                        &user_sid,
                        &receipt,
                        &current_executable,
                        &options,
                    )
                    .map(|()| has_receipt_executable_authority)
                    .map_err(|error| error.to_string())
                });
            let verified = validation.is_ok();
            let mut status = base_status(
                if verified {
                    state
                } else {
                    ServiceRuntimeState::Unknown
                },
                true,
                record.enabled,
            );
            status.service_name.clone_from(&record.task_name);
            status.service_definition = Some(task_definition_path(&proxy_home_dir()));
            status.detail = Some(match validation {
                Err(error) => format!(
                    "the SID-scoped task is not proven to match the signed service receipt and will not be mutated; task_state_code={}; verification_error={error}",
                    record.state,
                ),
                Ok(false) => format!(
                    "SID-scoped per-user scheduled task (interactive token, least privilege); task_state_code={}; the schema-1 compatibility receipt has no daemon_executable, so verification is limited to the current CLI executable; run `codex-helper service install` from that executable to refresh the receipt",
                    record.state
                ),
                Ok(true) => format!(
                    "SID-scoped per-user scheduled task (interactive token, least privilege); task_state_code={}",
                    record.state
                ),
            });
            return Ok(status);
        }
        if let Some(record) = query_scheduled_task(WINDOWS_TASK_BASENAME)?
            && windows_task_owner_matches(&record, &user_sid)
        {
            verify_legacy_fixed_windows_task_record(&record, &user_sid, &current_executable()?)?;
            let state = match record.state {
                4 => ServiceRuntimeState::Running,
                2 => ServiceRuntimeState::Starting,
                1 | 3 => ServiceRuntimeState::Stopped,
                _ => ServiceRuntimeState::Unknown,
            };
            let mut status = base_status(state, true, record.enabled);
            status.service_name.clone_from(&record.task_name);
            status.service_definition = Some(task_definition_path(&proxy_home_dir()));
            status.legacy_installation = true;
            status.detail = Some(format!(
                "legacy fixed-name per-user scheduled task owned by the current SID; run `codex-helper service install` to migrate; task_state_code={}",
                record.state
            ));
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
        verify_legacy_windows_scm_definition(
            &legacy_scm_definition(&config),
            &current_executable()?,
        )?;
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
        status.legacy_installation = true;
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
$taskDocument = New-Object System.Xml.XmlDocument
$taskDocument.LoadXml((Export-ScheduledTask -TaskName $target -TaskPath '\' -ErrorAction Stop))
$namespaces = New-Object System.Xml.XmlNamespaceManager($taskDocument.NameTable)
$namespaces.AddNamespace('task', $taskDocument.DocumentElement.NamespaceURI)
$principalNodes = @($taskDocument.SelectNodes('/task:Task/task:Principals/task:Principal', $namespaces))
$actionsNode = $taskDocument.SelectSingleNode('/task:Task/task:Actions', $namespaces)
$descriptionNode = $taskDocument.SelectSingleNode('/task:Task/task:RegistrationInfo/task:Description', $namespaces)
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
    version = [string] $taskDocument.DocumentElement.GetAttribute('version')
    description = if ($null -eq $descriptionNode) { '' } else { [string] $descriptionNode.InnerText }
    owner_sid = [string] $ownerSid
    principal_count = [int] $principalNodes.Count
    principal_id = if ($principalNodes.Count -eq 1) { [string] $principalNodes[0].GetAttribute('id') } else { '' }
    actions_context = if ($null -eq $actionsNode) { '' } else { [string] $actionsNode.GetAttribute('Context') }
    state = [int] $task.State
    enabled = [bool] $task.Settings.Enabled
    multiple_instances = [string] $task.Settings.MultipleInstances
    disallow_start_if_on_batteries = [bool] $task.Settings.DisallowStartIfOnBatteries
    stop_if_going_on_batteries = [bool] $task.Settings.StopIfGoingOnBatteries
    allow_hard_terminate = [bool] $task.Settings.AllowHardTerminate
    start_when_available = [bool] $task.Settings.StartWhenAvailable
    run_only_if_network_available = [bool] $task.Settings.RunOnlyIfNetworkAvailable
    allow_start_on_demand = [bool] $task.Settings.AllowStartOnDemand
    hidden = [bool] $task.Settings.Hidden
    run_only_if_idle = [bool] $task.Settings.RunOnlyIfIdle
    wake_to_run = [bool] $task.Settings.WakeToRun
    execution_time_limit = [string] $task.Settings.ExecutionTimeLimit
    priority = [int] $task.Settings.Priority
    restart_interval = [string] $task.Settings.RestartInterval
    restart_count = [int] $task.Settings.RestartCount
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

    fn require_task_snapshot_unchanged(
        current: &WindowsTaskRecord,
        snapshot: &OwnedTaskSnapshot,
        description: &str,
    ) -> CliResult<()> {
        require_task_owner(current, &snapshot.record.owner_sid, description)?;
        let mut expected = snapshot.record.clone();
        // Running/ready transitions are expected while the transaction is being prepared. Every
        // registration field remains a CAS boundary so an external Task Scheduler edit is never
        // silently deleted.
        expected.state = current.state;
        let definition_unchanged =
            snapshot_owned_task(current.clone())?.definition == snapshot.definition;
        if &expected == current && definition_unchanged {
            Ok(())
        } else {
            Err(CliError::Other(format!(
                "refusing to alter the {description} Windows task '{}': its registration changed after uninstall preflight",
                current.task_name
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
        restart_runtime: bool,
    ) -> CliResult<()> {
        let destination_exists =
            if let Some(current) = query_scheduled_task(&snapshot.record.task_name)? {
                require_task_snapshot_unchanged(&current, snapshot, "rollback destination")?;
                true
            } else {
                false
            };
        let rollback_path = installed_definition.with_file_name(format!(
            "windows-task-rollback-{}-{}.xml",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        write_service_definition(&rollback_path, &snapshot.definition)?;
        let restore = if destination_exists {
            Ok(())
        } else {
            register_task_from_file(&snapshot.record.task_name, &rollback_path)
        };
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
        require_task_snapshot_unchanged(&restored, snapshot, "restored")?;
        if restart_runtime && scheduled_task_requires_end(&snapshot.record)? {
            restart_restored_task(snapshot)?;
        }
        cleanup
    }

    fn restart_restored_task(snapshot: &OwnedTaskSnapshot) -> CliResult<()> {
        let current = query_scheduled_task(&snapshot.record.task_name)?.ok_or_else(|| {
            CliError::Other(format!(
                "restored Windows task '{}' disappeared before its runtime state could be restored",
                snapshot.record.task_name
            ))
        })?;
        require_task_snapshot_unchanged(&current, snapshot, "restored")?;
        if scheduled_task_requires_end(&current)? {
            return Ok(());
        }
        run_scheduled_task(&current.task_name)?;
        let deadline = Instant::now() + TASK_STOP_TIMEOUT;
        loop {
            let current = query_scheduled_task(&snapshot.record.task_name)?.ok_or_else(|| {
                CliError::Other(format!(
                    "restored Windows task '{}' disappeared after schtasks /Run",
                    snapshot.record.task_name
                ))
            })?;
            require_task_snapshot_unchanged(&current, snapshot, "restored")?;
            if scheduled_task_requires_end(&current)? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(CliError::Other(format!(
                    "restored Windows task '{}' did not enter a queued or running state within {} seconds",
                    snapshot.record.task_name,
                    TASK_STOP_TIMEOUT.as_secs()
                )));
            }
            std::thread::sleep(TASK_STOP_POLL_INTERVAL);
        }
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

    fn run_unchanged_scheduled_task(
        snapshot: &OwnedTaskSnapshot,
        description: &str,
    ) -> CliResult<()> {
        let current = query_scheduled_task(&snapshot.record.task_name)?.ok_or_else(|| {
            CliError::Other(format!(
                "the {description} Windows task disappeared before it could be started"
            ))
        })?;
        require_task_snapshot_unchanged(&current, snapshot, description)?;
        run_scheduled_task(&current.task_name)
    }

    fn end_unchanged_scheduled_task(
        snapshot: &OwnedTaskSnapshot,
        description: &str,
    ) -> CliResult<()> {
        let current = query_scheduled_task(&snapshot.record.task_name)?.ok_or_else(|| {
            CliError::Other(format!(
                "the {description} Windows task disappeared before it could be stopped"
            ))
        })?;
        require_task_snapshot_unchanged(&current, snapshot, description)?;
        if !scheduled_task_requires_end(&current)? {
            return Ok(());
        }
        run_command(
            "schtasks.exe",
            &[
                OsString::from("/End"),
                OsString::from("/TN"),
                OsString::from(&current.task_name),
            ],
        )?;

        let deadline = Instant::now() + TASK_STOP_TIMEOUT;
        loop {
            let current = query_scheduled_task(&snapshot.record.task_name)?.ok_or_else(|| {
                CliError::Other(format!(
                    "the {description} Windows task disappeared while waiting for it to stop"
                ))
            })?;
            require_task_snapshot_unchanged(&current, snapshot, description)?;
            if !scheduled_task_requires_end(&current)? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(CliError::Other(format!(
                    "the {description} Windows task '{}' did not stop within {} seconds",
                    current.task_name,
                    TASK_STOP_TIMEOUT.as_secs()
                )));
            }
            std::thread::sleep(TASK_STOP_POLL_INTERVAL);
        }
    }

    fn delete_unchanged_scheduled_task(
        snapshot: &OwnedTaskSnapshot,
        description: &str,
    ) -> CliResult<()> {
        let current = query_scheduled_task(&snapshot.record.task_name)?.ok_or_else(|| {
            CliError::Other(format!(
                "the {description} Windows task disappeared before deletion"
            ))
        })?;
        require_task_snapshot_unchanged(&current, snapshot, description)?;
        run_command(
            "schtasks.exe",
            &[
                OsString::from("/Delete"),
                OsString::from("/TN"),
                OsString::from(&current.task_name),
                OsString::from("/F"),
            ],
        )?;
        if query_scheduled_task(&snapshot.record.task_name)?.is_some() {
            return Err(CliError::Other(format!(
                "the {description} Windows task still exists after deletion"
            )));
        }
        Ok(())
    }

    fn legacy_scm_definition(config: &ServiceConfig) -> LegacyWindowsScmDefinition {
        LegacyWindowsScmDefinition {
            own_process: config.service_type == SERVICE_TYPE,
            start_type: config.start_type.to_raw(),
            error_control: config.error_control.to_raw(),
            dependencies: config
                .dependencies
                .iter()
                .map(|dependency| {
                    dependency
                        .to_system_identifier()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect(),
            account_name: config
                .account_name
                .as_ref()
                .map(|account| account.to_string_lossy().into_owned()),
            display_name: config.display_name.to_string_lossy().into_owned(),
            load_order_group: config
                .load_order_group
                .as_ref()
                .map(|group| group.to_string_lossy().into_owned()),
            command_line: config.executable_path.to_string_lossy().into_owned(),
        }
    }

    fn require_legacy_scm_snapshot_unchanged(
        service: &Service,
        snapshot: &LegacyScmSnapshot,
        operation: &str,
    ) -> CliResult<()> {
        let current = service
            .query_config()
            .map_err(|error| CliError::Other(format!("{operation}: query config: {error}")))?;
        if legacy_scm_definition(&current) == snapshot.definition {
            Ok(())
        } else {
            Err(CliError::Other(format!(
                "{operation}: refusing to alter the same-name SCM service because its definition changed after preflight"
            )))
        }
    }

    fn probe_legacy_scm(
        operation: &str,
        executable: &Path,
    ) -> CliResult<Option<LegacyScmSnapshot>> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(windows_error("open Windows Service Control Manager"))?;
        let access = ServiceAccess::QUERY_STATUS
            | ServiceAccess::QUERY_CONFIG
            | ServiceAccess::START
            | ServiceAccess::STOP
            | ServiceAccess::DELETE;
        let service = match manager.open_service(WINDOWS_SERVICE_NAME, access) {
            Ok(service) => service,
            Err(error) if windows_service_missing(&error) => return Ok(None),
            Err(error) => {
                return Err(CliError::Other(format!(
                    "{operation}: {error}; rerun once from an elevated terminal"
                )));
            }
        };
        let definition = legacy_scm_definition(
            &service
                .query_config()
                .map_err(|error| CliError::Other(format!("{operation}: query config: {error}")))?,
        );
        let invocation = verify_legacy_windows_scm_definition(&definition, executable)
            .map_err(|error| CliError::Other(format!("{operation}: {error}")))?;
        let state = service
            .query_status()
            .map_err(|error| CliError::Other(format!("{operation}: query status: {error}")))?
            .current_state;
        let was_running = match state {
            ServiceState::Running => true,
            ServiceState::Stopped => false,
            ServiceState::StartPending | ServiceState::StopPending => {
                return Err(CliError::Other(format!(
                    "{operation}: the legacy SCM service is transitioning; wait for it to become Running or Stopped, then retry"
                )));
            }
            _ => {
                return Err(CliError::Other(format!(
                    "{operation}: the legacy SCM service is not in a supported Running or Stopped state"
                )));
            }
        };
        Ok(Some(LegacyScmSnapshot {
            was_running,
            definition,
            invocation,
        }))
    }

    fn restore_legacy_scm_snapshot(snapshot: &LegacyScmSnapshot) -> CliResult<()> {
        let current = probe_legacy_scm(
            "verify the legacy LocalSystem SCM service after failed uninstall",
            &current_executable()?,
        )?
        .ok_or_else(|| {
            CliError::Other(
                "the legacy SCM definition is absent after failed removal and cannot be reconstructed automatically"
                    .to_string(),
            )
        })?;
        if current.definition != snapshot.definition {
            return Err(CliError::Other(
                "the legacy SCM definition changed after uninstall preflight and was left untouched"
                    .to_string(),
            ));
        }
        if snapshot.was_running && !current.was_running {
            start_legacy_scm_service(snapshot)?;
        } else if !snapshot.was_running && current.was_running {
            stop_legacy_scm_service(snapshot)?;
        }
        let restored = probe_legacy_scm(
            "verify the restored legacy LocalSystem SCM service runtime state",
            &current_executable()?,
        )?
        .ok_or_else(|| {
            CliError::Other(
                "the legacy SCM definition disappeared during rollback and cannot be reconstructed automatically"
                    .to_string(),
            )
        })?;
        if restored.definition == snapshot.definition
            && restored.was_running == snapshot.was_running
        {
            Ok(())
        } else {
            Err(CliError::Other(format!(
                "the legacy SCM runtime state did not return to {}",
                if snapshot.was_running {
                    "running"
                } else {
                    "stopped"
                }
            )))
        }
    }

    fn stop_legacy_scm_service(snapshot: &LegacyScmSnapshot) -> CliResult<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(windows_error("open Windows Service Control Manager"))?;
        let service = match manager.open_service(
            WINDOWS_SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG | ServiceAccess::STOP,
        ) {
            Ok(service) => service,
            Err(error) if windows_service_missing(&error) => {
                return Err(CliError::Other(
                    "the verified legacy SCM service disappeared before it could be stopped"
                        .to_string(),
                ));
            }
            Err(error) => return Err(windows_error("open legacy codex-helper SCM service")(error)),
        };
        require_legacy_scm_snapshot_unchanged(&service, snapshot, "stop the legacy SCM service")?;
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

    fn start_legacy_scm_service(snapshot: &LegacyScmSnapshot) -> CliResult<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(windows_error("open Windows Service Control Manager"))?;
        let service = match manager.open_service(
            WINDOWS_SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG | ServiceAccess::START,
        ) {
            Ok(service) => service,
            Err(error) if windows_service_missing(&error) => {
                return Err(CliError::Other(
                    "the legacy SCM service disappeared before rollback".to_string(),
                ));
            }
            Err(error) => return Err(windows_error("open legacy SCM service for rollback")(error)),
        };
        require_legacy_scm_snapshot_unchanged(
            &service,
            snapshot,
            "restart the legacy SCM service",
        )?;
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

    fn retire_legacy_scm_service(
        snapshot: &LegacyScmSnapshot,
    ) -> Result<(), LegacyScmRetirementFailure> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(windows_error("open Windows Service Control Manager"))
            .map_err(LegacyScmRetirementFailure::rollback_safe)?;
        let service = match manager.open_service(
            WINDOWS_SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG | ServiceAccess::DELETE,
        ) {
            Ok(service) => service,
            Err(error) if windows_service_missing(&error) => {
                return Err(LegacyScmRetirementFailure::rollback_safe(CliError::Other(
                    "the verified legacy SCM service disappeared before migration committed"
                        .to_string(),
                )));
            }
            Err(error) => {
                return Err(LegacyScmRetirementFailure::rollback_safe(windows_error(
                    "open legacy SCM service for migration",
                )(
                    error
                )));
            }
        };
        require_legacy_scm_snapshot_unchanged(&service, snapshot, "retire the legacy SCM service")
            .map_err(LegacyScmRetirementFailure::rollback_safe)?;
        if service
            .query_status()
            .map_err(windows_error("query legacy Windows service status"))
            .map_err(LegacyScmRetirementFailure::rollback_safe)?
            .current_state
            != ServiceState::Stopped
        {
            return Err(LegacyScmRetirementFailure::rollback_safe(
                CliError::Other(
                "the legacy SCM service restarted after handoff; its registration was left in place"
                    .to_string(),
                ),
            ));
        }
        require_legacy_scm_snapshot_unchanged(
            &service,
            snapshot,
            "delete the legacy SCM service after handoff",
        )
        .map_err(LegacyScmRetirementFailure::rollback_safe)?;
        let Err(delete_error) = service.delete() else {
            return Ok(());
        };
        let delete_error =
            windows_error("delete legacy codex-helper Windows service")(delete_error);
        drop(service);

        match manager.open_service(
            WINDOWS_SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG,
        ) {
            Err(error)
                if matches!(
                    windows_service_probe_classification(&error),
                    WindowsServiceProbeClassification::Missing
                        | WindowsServiceProbeClassification::MarkedForDelete
                ) =>
            {
                Ok(())
            }
            Ok(service) => {
                require_legacy_scm_snapshot_unchanged(
                    &service,
                    snapshot,
                    "verify the legacy SCM service after DeleteService reported an error",
                )
                .map_err(|probe_error| {
                    LegacyScmRetirementFailure::commit_unknown(CliError::Other(format!(
                        "{delete_error}; the legacy SCM registration changed while confirming whether deletion committed: {probe_error}. The verified replacement and its receipt were preserved"
                    )))
                })?;
                Err(LegacyScmRetirementFailure::rollback_safe(delete_error))
            }
            Err(probe_error) => Err(LegacyScmRetirementFailure::commit_unknown(CliError::Other(
                format!(
                    "{delete_error}; could not determine whether the legacy SCM deletion committed: {probe_error}. The verified replacement and its receipt were preserved"
                ),
            ))),
        }
    }

    fn remove_legacy_scm_service(stop_first: bool, snapshot: &LegacyScmSnapshot) -> CliResult<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(windows_error("open Windows Service Control Manager"))?;
        let service = match manager.open_service(
            WINDOWS_SERVICE_NAME,
            ServiceAccess::QUERY_STATUS
                | ServiceAccess::QUERY_CONFIG
                | ServiceAccess::STOP
                | ServiceAccess::DELETE,
        ) {
            Ok(service) => service,
            Err(error) if windows_service_missing(&error) => {
                return Err(CliError::Other(
                    "the verified legacy SCM service disappeared before uninstall committed"
                        .to_string(),
                ));
            }
            Err(error) => {
                return Err(CliError::Other(format!(
                    "remove legacy LocalSystem SCM service before installing the per-user task: {error}; rerun once from an elevated terminal"
                )));
            }
        };
        require_legacy_scm_snapshot_unchanged(&service, snapshot, "remove the legacy SCM service")?;
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

    fn windows_service_probe_classification(
        error: &windows_service::Error,
    ) -> WindowsServiceProbeClassification {
        let raw_os_error = match error {
            windows_service::Error::Winapi(error) => error.raw_os_error(),
            _ => None,
        };
        classify_windows_service_probe_error(raw_os_error)
    }

    fn windows_service_missing(error: &windows_service::Error) -> bool {
        windows_service_probe_classification(error) == WindowsServiceProbeClassification::Missing
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
            legacy_installation: false,
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

    struct NativeMacosInstallBackend {
        path: PathBuf,
        domain: OsString,
        target: String,
        helper_home: PathBuf,
        installed_receipt: Option<ServiceReceipt>,
        was_loaded: bool,
        original_definition_exists: bool,
        definition: codex_helper_core::ManagedFileTransaction,
        receipt_transaction: ServiceReceiptTransaction,
        receipt: ServiceReceipt,
        document: String,
        replacement_start_attempted: bool,
    }

    impl NativeMacosInstallBackend {
        fn new(options: ServiceInstallOptions) -> CliResult<Self> {
            let executable = service_executable(&current_executable()?)?;
            let log_dir = service_log_dir();
            let path = launch_agent_path()?;
            let document = render_launch_agent(&executable, &log_dir, &options);
            let domain = launchd_domain()?;
            let target = format!("{}/{MACOS_LABEL}", launchd_domain_string()?);
            let definition = codex_helper_core::ManagedFileTransaction::begin(
                path.clone(),
                MAX_SERVICE_DEFINITION_BYTES,
            )
            .map_err(|error| CliError::Other(format!("begin LaunchAgent transaction: {error}")))?;
            let original_definition_exists = definition.current().bytes().is_some();
            let (receipt_transaction, receipt, install_preflight) =
                begin_service_receipt_transaction_with_daemon_executable(&options, &executable)?;
            verify_unix_service_install_replacement(
                install_preflight.installed_receipt.as_ref(),
                definition.current().bytes(),
                ServicePlatformBackend::MacosLaunchAgent,
            )?;
            let current_registration = NativeMacosUninstallBackend::query_registration(&target)?;
            match install_preflight.installed_receipt.as_ref() {
                Some(_) => {
                    verify_launchd_registration_snapshot(current_registration.as_deref(), &path)?
                }
                None if current_registration.is_some() => {
                    return Err(CliError::Other(
                        "refusing to replace a loaded LaunchAgent without a current service receipt proving its registration. No service registration, definition, receipt, or runtime was changed; unload the unverified job, then run `codex-helper service install`"
                            .to_string(),
                    ));
                }
                None => {}
            }
            ensure_service_log_dir()?;
            prepare_service_switch_for_install(&options, &install_preflight)?;
            Ok(Self {
                path,
                domain,
                target,
                helper_home: receipt.helper_home().to_path_buf(),
                installed_receipt: install_preflight.installed_receipt,
                was_loaded: false,
                original_definition_exists,
                definition,
                receipt_transaction,
                receipt,
                document,
                replacement_start_attempted: false,
            })
        }

        fn revalidate_original_authority(&self, require_unloaded: bool) -> CliResult<bool> {
            verify_current_service_receipt_snapshot(
                &self.helper_home,
                self.installed_receipt.as_ref(),
                "replace the LaunchAgent",
            )?;
            match self.installed_receipt.as_ref() {
                Some(receipt) => verify_unix_service_definition_at(
                    receipt,
                    ServicePlatformBackend::MacosLaunchAgent,
                    &self.path,
                )?,
                None => verify_current_unix_service_definition_bytes(
                    &self.path,
                    None,
                    "replace the LaunchAgent",
                )?,
            }
            let registration = NativeMacosUninstallBackend::query_registration(&self.target)?;
            match self.installed_receipt.as_ref() {
                Some(_) => {
                    verify_launchd_registration_snapshot(registration.as_deref(), &self.path)?
                }
                None if registration.is_some() => {
                    return Err(CliError::Other(
                        "refusing to replace a loaded LaunchAgent without a current service receipt proving its registration. No service registration, definition, receipt, or runtime was changed; unload the unverified job, then run `codex-helper service install`"
                            .to_string(),
                    ));
                }
                None => {}
            }
            if require_unloaded && registration.is_some() {
                return Err(CliError::Other(
                    "refusing to replace the LaunchAgent because it became loaded after the transaction began. No service registration, definition, receipt, or runtime was changed; retry from a fresh service state"
                        .to_string(),
                ));
            }
            Ok(registration.is_some())
        }

        fn revalidate_replacement_before_receipt_publish(&self) -> CliResult<()> {
            verify_current_service_receipt_snapshot(
                &self.helper_home,
                self.installed_receipt.as_ref(),
                "publish the LaunchAgent service receipt",
            )?;
            verify_current_unix_service_definition_bytes(
                &self.path,
                Some(self.document.as_bytes()),
                "publish the LaunchAgent service receipt",
            )?;
            if NativeMacosUninstallBackend::query_registration(&self.target)?.is_some() {
                return Err(CliError::Other(
                    "refusing to publish the LaunchAgent service receipt because launchd loaded a registration while its replacement definition was being prepared. No service registration, definition, receipt, or runtime was changed; retry from a fresh service state"
                        .to_string(),
                ));
            }
            Ok(())
        }
    }

    impl UnixInstallTransactionBackend for NativeMacosInstallBackend {
        fn prepare_replacement(&mut self) -> CliResult<()> {
            self.was_loaded = self.revalidate_original_authority(false)?;
            if self.was_loaded {
                run_command(
                    "launchctl",
                    &[
                        OsString::from("bootout"),
                        self.domain.clone(),
                        self.path.clone().into_os_string(),
                    ],
                )?;
            }
            self.revalidate_original_authority(true)?;
            self.definition
                .replace(self.document.as_bytes())
                .map_err(|error| {
                    CliError::Other(format!("publish LaunchAgent definition: {error}"))
                })?;
            if self.definition.current().bytes() != Some(self.document.as_bytes()) {
                return Err(CliError::Other(
                    "LaunchAgent definition failed transaction read-back verification".to_string(),
                ));
            }
            run_command(
                "plutil",
                &[OsString::from("-lint"), self.path.clone().into_os_string()],
            )?;
            self.revalidate_replacement_before_receipt_publish()?;
            self.receipt_transaction
                .replace(&self.receipt)
                .map_err(|error| {
                    CliError::Other(format!("publish LaunchAgent service receipt: {error}"))
                })
        }

        fn start_replacement(&mut self) -> CliResult<()> {
            self.replacement_start_attempted = true;
            start()
        }

        async fn verify_started_runtime_identity(
            &mut self,
        ) -> CliResult<codex_helper_core::credentials::CredentialAggregateReadiness> {
            verify_started_service_runtime_identity().await
        }

        fn rollback(&mut self) -> CliResult<()> {
            let mut failures = Vec::new();
            let replacement_stopped = if self.replacement_start_attempted {
                match stop() {
                    Ok(()) => true,
                    Err(stop_error) => {
                        match NativeMacosUninstallBackend::query_loaded(&self.target) {
                            Ok(false) => true,
                            Ok(true) => {
                                failures.push(format!(
                                "stop the replacement LaunchAgent before rollback: {stop_error}; launchd still reports it loaded"
                            ));
                                false
                            }
                            Err(probe_error) => {
                                failures.push(format!(
                                "stop the replacement LaunchAgent before rollback: {stop_error}; its state could not be verified: {probe_error}"
                            ));
                                false
                            }
                        }
                    }
                }
            } else {
                true
            };
            if !replacement_stopped {
                failures.push(
                    "the replacement LaunchAgent definition and service receipt were preserved because its runtime may still be active"
                        .to_string(),
                );
                return Err(CliError::Other(failures.join("; ")));
            }
            if let Err(error) = self.receipt_transaction.rollback() {
                failures.push(format!("restore previous service receipt: {error}"));
            }
            if let Err(error) = self.definition.rollback() {
                failures.push(format!("restore previous LaunchAgent definition: {error}"));
            }
            if self.was_loaded {
                if !self.original_definition_exists {
                    if !NativeMacosUninstallBackend::query_loaded(&self.target)? {
                        failures.push(
                            "the previous detached LaunchAgent had no definition and could not be reconstructed"
                                .to_string(),
                        );
                    }
                } else if !NativeMacosUninstallBackend::query_loaded(&self.target)?
                    && let Err(error) = start()
                {
                    failures.push(format!("reload previous LaunchAgent: {error}"));
                }
                match NativeMacosUninstallBackend::query_loaded(&self.target) {
                    Ok(true) => {}
                    Ok(false) => failures
                        .push("the previous LaunchAgent was not loaded after rollback".to_string()),
                    Err(error) => failures.push(format!(
                        "verify the previous LaunchAgent after rollback: {error}"
                    )),
                }
            }
            if failures.is_empty() {
                Ok(())
            } else {
                Err(CliError::Other(failures.join("; ")))
            }
        }
    }

    pub(super) async fn install(
        options: ServiceInstallOptions,
    ) -> CliResult<Option<codex_helper_core::credentials::CredentialAggregateReadiness>> {
        let start = options.start;
        run_unix_install_transaction(&mut NativeMacosInstallBackend::new(options)?, start).await
    }

    struct NativeMacosUninstallBackend {
        path: PathBuf,
        domain: OsString,
        target: String,
        helper_home: PathBuf,
        installed_receipt: ServiceReceipt,
        was_loaded: bool,
        stopped: bool,
        stop_requested: bool,
        original_definition_exists: bool,
        definition: codex_helper_core::ManagedFileTransaction,
        receipt: ServiceReceiptTransaction,
    }

    impl NativeMacosUninstallBackend {
        fn new() -> CliResult<Self> {
            let path = launch_agent_path()?;
            let domain = launchd_domain()?;
            let target = format!("{}/{MACOS_LABEL}", launchd_domain_string()?);
            let definition = codex_helper_core::ManagedFileTransaction::begin(
                path.clone(),
                MAX_SERVICE_DEFINITION_BYTES,
            )
            .map_err(|error| {
                CliError::Other(format!("begin LaunchAgent removal transaction: {error}"))
            })?;
            let original_definition_exists = definition.current().bytes().is_some();
            let receipt = ServiceReceiptTransaction::begin(proxy_home_dir()).map_err(|error| {
                CliError::Other(format!("begin service receipt removal: {error}"))
            })?;
            let installed_receipt = receipt
                .current()
                .map_err(|error| {
                    CliError::Other(format!(
                        "read the locked service receipt before LaunchAgent removal: {error}"
                    ))
                })?
                .ok_or_else(|| {
                    CliError::Other(
                        "refusing to remove the LaunchAgent without a current service receipt. No service registration, definition, receipt, or runtime was changed; repair or reinstall the service before retrying"
                            .to_string(),
                    )
                })?;
            if installed_receipt.platform_backend() != ServicePlatformBackend::MacosLaunchAgent {
                return Err(CliError::Other(
                    "refusing to remove the LaunchAgent with a receipt for another platform backend. No service registration, definition, receipt, or runtime was changed; repair or reinstall the service before retrying"
                        .to_string(),
                ));
            }
            verify_unix_service_definition_snapshot(
                &installed_receipt,
                definition.current().bytes(),
            )?;
            let current_registration = Self::query_registration(&target)?;
            verify_launchd_registration_snapshot(current_registration.as_deref(), &path)?;
            Ok(Self {
                path,
                domain,
                target,
                helper_home: installed_receipt.helper_home().to_path_buf(),
                installed_receipt,
                was_loaded: false,
                stopped: false,
                stop_requested: false,
                original_definition_exists,
                definition,
                receipt,
            })
        }

        fn query_registration(target: &str) -> CliResult<Option<String>> {
            match run_command(
                "launchctl",
                &[OsString::from("print"), OsString::from(target)],
            ) {
                Ok(output) => Ok(Some(output)),
                Err(error) if launchctl_reports_missing(&error) => Ok(None),
                Err(error) => Err(CliError::Other(format!(
                    "query the LaunchAgent registration before managing it: {error}"
                ))),
            }
        }

        fn query_loaded(target: &str) -> CliResult<bool> {
            Self::query_registration(target).map(|registration| registration.is_some())
        }

        fn is_loaded(&self) -> CliResult<bool> {
            Self::query_loaded(&self.target)
        }

        fn revalidate_installed_authority(&self, require_unloaded: bool) -> CliResult<bool> {
            verify_current_service_receipt_snapshot(
                &self.helper_home,
                Some(&self.installed_receipt),
                "remove the LaunchAgent",
            )?;
            verify_unix_service_definition_at(
                &self.installed_receipt,
                ServicePlatformBackend::MacosLaunchAgent,
                &self.path,
            )?;
            let registration = Self::query_registration(&self.target)?;
            verify_launchd_registration_snapshot(registration.as_deref(), &self.path)?;
            if require_unloaded && registration.is_some() {
                return Err(CliError::Other(
                    "refusing to remove the LaunchAgent because it became loaded after the transaction began. No service registration, definition, receipt, or runtime was changed; retry from a fresh service state"
                        .to_string(),
                ));
            }
            Ok(registration.is_some())
        }

        fn revalidate_before_receipt_removal(&self) -> CliResult<()> {
            verify_current_service_receipt_snapshot(
                &self.helper_home,
                Some(&self.installed_receipt),
                "remove the LaunchAgent service receipt",
            )?;
            verify_current_unix_service_definition_bytes(
                &self.path,
                None,
                "remove the LaunchAgent service receipt",
            )?;
            let registration = Self::query_registration(&self.target)?;
            if self.stop_requested && registration.is_some() {
                return Err(CliError::Other(
                    "refusing to remove the LaunchAgent service receipt because launchd loaded the job after it was stopped. No service registration, definition, receipt, or runtime was changed; retry from a fresh service state"
                        .to_string(),
                ));
            }
            if let Some(registration) = registration.as_deref() {
                verify_launchd_registration_snapshot(Some(registration), &self.path)?;
            }
            Ok(())
        }
    }

    fn launchctl_reports_missing(error: &CliError) -> bool {
        let text = error.to_string().to_ascii_lowercase();
        text.contains("could not find service")
            || text.contains("no such process")
            || text.contains("not found")
    }

    impl ServiceUninstallTransactionBackend for NativeMacosUninstallBackend {
        fn stop_and_verify(&mut self) -> CliResult<()> {
            self.stop_requested = true;
            self.was_loaded = self.revalidate_installed_authority(false)?;
            if !self.was_loaded {
                return Ok(());
            }
            self.stopped = true;
            run_command(
                "launchctl",
                &[
                    OsString::from("bootout"),
                    self.domain.clone(),
                    self.path.clone().into_os_string(),
                ],
            )
            .map_err(|error| {
                CliError::Other(format!(
                    "boot out the LaunchAgent before uninstalling it: {error}"
                ))
            })?;
            if self.is_loaded()? {
                return Err(CliError::Other(format!(
                    "LaunchAgent {MACOS_LABEL} is still loaded after launchctl bootout; its definition and receipt were not removed"
                )));
            }
            Ok(())
        }

        fn disable_and_verify(&mut self) -> CliResult<()> {
            Ok(())
        }

        fn remove_definition(&mut self) -> CliResult<()> {
            self.revalidate_installed_authority(self.stop_requested)?;
            self.definition.remove().map_err(|error| {
                CliError::Other(format!(
                    "remove LaunchAgent definition {}: {error}",
                    self.path.display()
                ))
            })?;
            if self.definition.current().bytes().is_some() {
                return Err(CliError::Other(format!(
                    "LaunchAgent definition {} still exists after removal",
                    self.path.display()
                )));
            }
            Ok(())
        }

        fn remove_receipt(&mut self) -> CliResult<()> {
            self.revalidate_before_receipt_removal()?;
            self.receipt.remove().map_err(|error| {
                CliError::Other(format!(
                    "remove service receipt after LaunchAgent removal: {error}"
                ))
            })
        }

        fn rollback(&mut self) -> CliResult<()> {
            let mut failures = Vec::new();
            let definition_restored = match self.definition.rollback() {
                Ok(()) => true,
                Err(error) => {
                    failures.push(format!("restore the LaunchAgent definition: {error}"));
                    false
                }
            };
            if let Err(error) = self.receipt.rollback() {
                failures.push(format!("restore the service receipt: {error}"));
            }
            if definition_restored && self.stopped && self.was_loaded {
                if !self.original_definition_exists {
                    failures.push(
                        "restart the previous LaunchAgent: its original definition was absent"
                            .to_string(),
                    );
                } else {
                    match self.is_loaded() {
                        Ok(true) => {}
                        Ok(false) => {
                            if let Err(error) = start() {
                                failures.push(format!("restart the previous LaunchAgent: {error}"));
                            } else {
                                match self.is_loaded() {
                                    Ok(true) => {}
                                    Ok(false) => failures.push(
                                        "restart the previous LaunchAgent: launchctl did not report it loaded"
                                            .to_string(),
                                    ),
                                    Err(error) => failures.push(format!(
                                        "verify the restarted LaunchAgent state: {error}"
                                    )),
                                }
                            }
                        }
                        Err(error) => failures
                            .push(format!("verify the restarted LaunchAgent state: {error}")),
                    }
                }
            }
            if failures.is_empty() {
                Ok(())
            } else {
                Err(CliError::Other(failures.join("; ")))
            }
        }
    }

    pub(super) fn uninstall(stop_first: bool) -> CliResult<()> {
        run_service_uninstall_transaction(&mut NativeMacosUninstallBackend::new()?, stop_first)
    }

    pub(super) fn verify_receipt_definition(receipt: &ServiceReceipt) -> CliResult<()> {
        let path = launch_agent_path()?;
        verify_unix_service_definition_at(
            receipt,
            ServicePlatformBackend::MacosLaunchAgent,
            &path,
        )?;
        let target = format!("{}/{MACOS_LABEL}", launchd_domain_string()?);
        let registered_definition = NativeMacosUninstallBackend::query_registration(&target)?;
        verify_launchd_registration_snapshot(registered_definition.as_deref(), &path)
    }

    fn read_verified_installed_receipt() -> CliResult<ServiceReceipt> {
        let receipt = read_service_receipt(proxy_home_dir()).map_err(|error| {
            CliError::Other(format!(
                "refusing to manage the installed LaunchAgent without a current service receipt: {error}. No service registration, definition, receipt, or runtime was changed; repair or reinstall the service before retrying"
            ))
        })?;
        verify_receipt_definition(&receipt)?;
        Ok(receipt)
    }

    pub(super) fn start() -> CliResult<()> {
        read_verified_installed_receipt()?;
        let target = format!("{}/{MACOS_LABEL}", launchd_domain_string()?);
        if NativeMacosUninstallBackend::query_registration(&target)?
            .as_deref()
            .is_some_and(|output| output.contains("state = running"))
        {
            return Ok(());
        }

        // The state read above only selects the action. Revalidate receipt, definition, and
        // registration immediately before the launchd mutation.
        read_verified_installed_receipt()?;
        match NativeMacosUninstallBackend::query_registration(&target)? {
            Some(output) if output.contains("state = running") => Ok(()),
            Some(_) => run_command(
                "launchctl",
                &[OsString::from("kickstart"), OsString::from(target)],
            )
            .map(|_| ()),
            None => run_command(
                "launchctl",
                &[
                    OsString::from("bootstrap"),
                    launchd_domain()?,
                    launch_agent_path()?.into_os_string(),
                ],
            )
            .map(|_| ()),
        }
    }

    pub(super) fn stop() -> CliResult<()> {
        read_verified_installed_receipt()?;
        let target = format!("{}/{MACOS_LABEL}", launchd_domain_string()?);
        let domain = launchd_domain()?;
        let path = launch_agent_path()?;
        stop_launch_agent_with(
            || {
                run_command(
                    "launchctl",
                    &[OsString::from("print"), OsString::from(&target)],
                )
            },
            || {
                read_verified_installed_receipt()?;
                run_command(
                    "launchctl",
                    &[OsString::from("bootout"), domain, path.into_os_string()],
                )
            },
            || {
                run_command(
                    "launchctl",
                    &[OsString::from("print"), OsString::from(&target)],
                )
            },
        )
    }

    pub(super) fn stop_launch_agent_with<Q, B, V>(
        query: Q,
        bootout: B,
        verify_unloaded: V,
    ) -> CliResult<()>
    where
        Q: FnOnce() -> CliResult<String>,
        B: FnOnce() -> CliResult<String>,
        V: FnOnce() -> CliResult<String>,
    {
        match query() {
            Ok(_) => {}
            Err(error) if launchctl_reports_missing(&error) => return Ok(()),
            Err(error) => {
                return Err(CliError::Other(format!(
                    "query LaunchAgent state before stopping it: {error}"
                )));
            }
        }
        match bootout() {
            Ok(_) => {}
            Err(error) if launchctl_reports_missing(&error) => {}
            Err(error) => Err(CliError::Other(format!(
                "unload the LaunchAgent with launchctl bootout: {error}"
            )))?,
        }
        match verify_unloaded() {
            Err(error) if launchctl_reports_missing(&error) => Ok(()),
            Err(error) => Err(CliError::Other(format!(
                "verify the LaunchAgent was unloaded after launchctl bootout: {error}"
            ))),
            Ok(_) => Err(CliError::Other(format!(
                "LaunchAgent {MACOS_LABEL} is still loaded after launchctl bootout"
            ))),
        }
    }

    pub(super) fn status() -> CliResult<ServiceStatus> {
        let path = launch_agent_path()?;
        let target = format!("{}/{MACOS_LABEL}", launchd_domain_string()?);
        let output = run_command(
            "launchctl",
            &[OsString::from("print"), OsString::from(target)],
        );
        if !path.exists() {
            return match output.as_ref() {
                Ok(output) => {
                    let state = if output.contains("state = running") {
                        ServiceRuntimeState::Running
                    } else {
                        ServiceRuntimeState::Stopped
                    };
                    let mut status = base_status(state, false, false, Some(path));
                    status.detail = Some(
                        "LaunchAgent definition is absent, but launchd still has the detached runtime loaded (for example after uninstall --keep-running)"
                            .to_string(),
                    );
                    Ok(status)
                }
                Err(error) if launchctl_reports_missing(error) => Ok(base_status(
                    ServiceRuntimeState::NotInstalled,
                    false,
                    false,
                    Some(path),
                )),
                Err(error) => {
                    let mut status =
                        base_status(ServiceRuntimeState::Unknown, false, false, Some(path));
                    status.detail = Some(format!(
                        "LaunchAgent definition is absent, but launchd state could not be verified: {error}"
                    ));
                    Ok(status)
                }
            };
        }
        let (state, detail) = match output {
            Ok(output) if output.contains("state = running") => {
                (ServiceRuntimeState::Running, Some(output))
            }
            Ok(output) => (ServiceRuntimeState::Stopped, Some(output)),
            Err(error) if launchctl_reports_missing(&error) => {
                (ServiceRuntimeState::Installed, Some(error.to_string()))
            }
            Err(error) => (ServiceRuntimeState::Unknown, Some(error.to_string())),
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
        render_launch_agent_definition(executable, log_dir, options)
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
            legacy_installation: false,
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

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn render_launch_agent_definition(
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

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    struct NativeLinuxInstallBackend {
        path: PathBuf,
        helper_home: PathBuf,
        installed_receipt: Option<ServiceReceipt>,
        was_active: bool,
        was_enabled: bool,
        definition: codex_helper_core::ManagedFileTransaction,
        receipt_transaction: ServiceReceiptTransaction,
        receipt: ServiceReceipt,
        document: String,
        replacement_enabled: bool,
        replacement_receipt_published: bool,
        replacement_start_attempted: bool,
    }

    impl NativeLinuxInstallBackend {
        fn new(options: ServiceInstallOptions) -> CliResult<Self> {
            let executable = service_executable(&current_executable()?)?;
            systemctl(&["show-environment"])?;
            let path = user_unit_path()?;
            let document = render_systemd_unit(&executable, &options);
            let definition = codex_helper_core::ManagedFileTransaction::begin(
                path.clone(),
                MAX_SERVICE_DEFINITION_BYTES,
            )
            .map_err(|error| CliError::Other(format!("begin systemd unit transaction: {error}")))?;
            let (receipt_transaction, receipt, install_preflight) =
                begin_service_receipt_transaction_with_daemon_executable(&options, &executable)?;
            verify_unix_service_install_replacement(
                install_preflight.installed_receipt.as_ref(),
                definition.current().bytes(),
                ServicePlatformBackend::LinuxSystemdUser,
            )?;
            match install_preflight.installed_receipt.as_ref() {
                Some(_) => verify_registration(&path)?,
                None => verify_registration_absent()?,
            }
            ensure_service_log_dir()?;
            prepare_service_switch_for_install(&options, &install_preflight)?;
            Ok(Self {
                path,
                helper_home: receipt.helper_home().to_path_buf(),
                installed_receipt: install_preflight.installed_receipt,
                was_active: false,
                was_enabled: false,
                definition,
                receipt_transaction,
                receipt,
                document,
                replacement_enabled: false,
                replacement_receipt_published: false,
                replacement_start_attempted: false,
            })
        }

        fn revalidate_original_authority(&self) -> CliResult<(bool, bool)> {
            verify_current_service_receipt_snapshot(
                &self.helper_home,
                self.installed_receipt.as_ref(),
                "replace the systemd user unit",
            )?;
            match self.installed_receipt.as_ref() {
                Some(receipt) => {
                    verify_unix_service_definition_at(
                        receipt,
                        ServicePlatformBackend::LinuxSystemdUser,
                        &self.path,
                    )?;
                    verify_registration(&self.path)?;
                    Ok((
                        systemd_active_state_requires_stop(&systemd_property("ActiveState")?)?,
                        systemd_unit_file_state_requires_disable(&systemd_property(
                            "UnitFileState",
                        )?)?,
                    ))
                }
                None => {
                    verify_current_unix_service_definition_bytes(
                        &self.path,
                        None,
                        "replace the systemd user unit",
                    )?;
                    verify_registration_absent()?;
                    Ok((false, false))
                }
            }
        }

        fn revalidate_replacement_before_manager_reload(&self) -> CliResult<()> {
            verify_current_service_receipt_snapshot(
                &self.helper_home,
                self.installed_receipt.as_ref(),
                "reload the replacement systemd user unit",
            )?;
            verify_current_unix_service_definition_bytes(
                &self.path,
                Some(self.document.as_bytes()),
                "reload the replacement systemd user unit",
            )?;
            verify_replacement_registration_before_reload(
                &self.path,
                self.installed_receipt.is_none(),
            )
        }

        fn revalidate_replacement_before_enable(&self) -> CliResult<()> {
            verify_current_service_receipt_snapshot(
                &self.helper_home,
                self.installed_receipt.as_ref(),
                "enable the replacement systemd user unit",
            )?;
            verify_current_unix_service_definition_bytes(
                &self.path,
                Some(self.document.as_bytes()),
                "enable the replacement systemd user unit",
            )?;
            verify_replacement_registration_after_reload(&self.path)
        }

        fn revalidate_replacement_before_receipt_publish(&self) -> CliResult<()> {
            verify_current_service_receipt_snapshot(
                &self.helper_home,
                self.installed_receipt.as_ref(),
                "publish the systemd service receipt",
            )?;
            verify_current_unix_service_definition_bytes(
                &self.path,
                Some(self.document.as_bytes()),
                "publish the systemd service receipt",
            )?;
            verify_registration(&self.path)
        }

        fn revalidate_replacement_before_rollback_disable(&self) -> CliResult<()> {
            let expected_receipt = if self.replacement_receipt_published {
                Some(&self.receipt)
            } else {
                self.installed_receipt.as_ref()
            };
            verify_current_service_receipt_snapshot(
                &self.helper_home,
                expected_receipt,
                "disable the replacement systemd user unit during rollback",
            )?;
            verify_current_unix_service_definition_bytes(
                &self.path,
                Some(self.document.as_bytes()),
                "disable the replacement systemd user unit during rollback",
            )?;
            verify_registration(&self.path)
        }

        fn revalidate_restored_definition_before_manager_reload(&self) -> CliResult<()> {
            verify_current_service_receipt_snapshot(
                &self.helper_home,
                self.installed_receipt.as_ref(),
                "reload restored systemd user units",
            )?;
            match self.installed_receipt.as_ref() {
                Some(receipt) => {
                    verify_unix_service_definition_at(
                        receipt,
                        ServicePlatformBackend::LinuxSystemdUser,
                        &self.path,
                    )?;
                    verify_replacement_registration_before_reload(&self.path, false)
                }
                None => {
                    verify_current_unix_service_definition_bytes(
                        &self.path,
                        None,
                        "reload restored systemd user units",
                    )?;
                    verify_registration_after_definition_removal(&self.path)
                }
            }
        }

        fn revalidate_restored_definition_before_enablement(&self) -> CliResult<()> {
            let Some(receipt) = self.installed_receipt.as_ref() else {
                return verify_registration_absent();
            };
            verify_current_service_receipt_snapshot(
                &self.helper_home,
                Some(receipt),
                "restore systemd user unit enablement",
            )?;
            verify_unix_service_definition_at(
                receipt,
                ServicePlatformBackend::LinuxSystemdUser,
                &self.path,
            )?;
            verify_replacement_registration_after_reload(&self.path)
        }
    }

    impl UnixInstallTransactionBackend for NativeLinuxInstallBackend {
        fn prepare_replacement(&mut self) -> CliResult<()> {
            (self.was_active, self.was_enabled) = self.revalidate_original_authority()?;
            if self.was_active {
                systemctl(&["stop", LINUX_UNIT_NAME])?;
            }
            self.revalidate_original_authority()?;
            self.definition
                .replace(self.document.as_bytes())
                .map_err(|error| CliError::Other(format!("publish systemd user unit: {error}")))?;
            if self.definition.current().bytes() != Some(self.document.as_bytes()) {
                return Err(CliError::Other(
                    "systemd user unit failed transaction read-back verification".to_string(),
                ));
            }
            self.revalidate_replacement_before_manager_reload()?;
            systemctl(&["daemon-reload"])?;
            self.revalidate_replacement_before_enable()?;
            systemctl(&["enable", LINUX_UNIT_NAME])?;
            self.replacement_enabled = true;
            if !matches!(
                systemctl_output(&["is-enabled", LINUX_UNIT_NAME]).as_deref(),
                Ok("enabled")
            ) {
                return Err(CliError::Other(
                    "systemd user unit did not report enabled after installation".to_string(),
                ));
            }
            self.revalidate_replacement_before_receipt_publish()?;
            self.receipt_transaction
                .replace(&self.receipt)
                .map_err(|error| {
                    CliError::Other(format!("publish systemd service receipt: {error}"))
                })?;
            self.replacement_receipt_published = true;
            Ok(())
        }

        fn start_replacement(&mut self) -> CliResult<()> {
            self.replacement_start_attempted = true;
            start()
        }

        async fn verify_started_runtime_identity(
            &mut self,
        ) -> CliResult<codex_helper_core::credentials::CredentialAggregateReadiness> {
            verify_started_service_runtime_identity().await
        }

        fn rollback(&mut self) -> CliResult<()> {
            let mut failures = Vec::new();
            let replacement_stopped = if self.replacement_start_attempted {
                match stop() {
                    Ok(()) => true,
                    Err(stop_error) => match systemd_property("ActiveState") {
                        Ok(state) if matches!(state.as_str(), "inactive" | "failed") => true,
                        Ok(state) => {
                            failures.push(format!(
                                "stop the replacement systemd user unit before rollback: {stop_error}; systemd still reports ActiveState={state:?}"
                            ));
                            false
                        }
                        Err(probe_error) => {
                            failures.push(format!(
                                "stop the replacement systemd user unit before rollback: {stop_error}; its state could not be verified: {probe_error}"
                            ));
                            false
                        }
                    },
                }
            } else {
                true
            };
            if !replacement_stopped {
                failures.push(
                    "the replacement systemd unit and service receipt were preserved because its runtime may still be active"
                        .to_string(),
                );
                return Err(CliError::Other(failures.join("; ")));
            }
            if self.installed_receipt.is_none() && self.replacement_enabled {
                if let Err(error) = self
                    .revalidate_replacement_before_rollback_disable()
                    .and_then(|()| systemctl(&["disable", LINUX_UNIT_NAME]))
                {
                    failures.push(format!(
                        "disable the replacement systemd user unit before removing its definition: {error}"
                    ));
                    failures.push(
                        "the replacement systemd unit and service receipt were preserved because its registration could not be disabled safely"
                            .to_string(),
                    );
                    return Err(CliError::Other(failures.join("; ")));
                }
            }
            if let Err(error) = self.receipt_transaction.rollback() {
                failures.push(format!("restore previous service receipt: {error}"));
            }
            if let Err(error) = self.definition.rollback() {
                failures.push(format!("restore previous systemd user unit: {error}"));
            }
            let reloaded = match self.revalidate_restored_definition_before_manager_reload() {
                Ok(()) => match systemctl(&["daemon-reload"]) {
                    Ok(()) => true,
                    Err(error) => {
                        failures.push(format!("reload restored systemd user units: {error}"));
                        false
                    }
                },
                Err(error) => {
                    failures.push(format!(
                        "revalidate restored systemd user unit before manager reload: {error}"
                    ));
                    false
                }
            };
            if reloaded && self.installed_receipt.is_some() {
                let restore_enablement = self
                    .revalidate_restored_definition_before_enablement()
                    .and_then(|()| {
                        if self.was_enabled {
                            systemctl(&["enable", LINUX_UNIT_NAME])
                        } else {
                            systemctl(&["disable", LINUX_UNIT_NAME])
                        }
                    });
                if let Err(error) = restore_enablement {
                    failures.push(format!("restore systemd user unit enablement: {error}"));
                }
            }
            if self.was_active {
                match start() {
                    Ok(()) => match systemd_property("ActiveState") {
                        Ok(state) if state == "active" => {}
                        Ok(state) => failures.push(format!(
                            "the previous systemd user unit reported ActiveState={state:?} after rollback"
                        )),
                        Err(error) => failures.push(format!(
                            "verify the previous systemd user unit after rollback: {error}"
                        )),
                    },
                    Err(error) => {
                        failures.push(format!("restart previous systemd user unit: {error}"));
                    }
                }
            }
            if failures.is_empty() {
                Ok(())
            } else {
                Err(CliError::Other(failures.join("; ")))
            }
        }
    }

    pub(super) async fn install(
        options: ServiceInstallOptions,
    ) -> CliResult<Option<codex_helper_core::credentials::CredentialAggregateReadiness>> {
        let start = options.start;
        run_unix_install_transaction(&mut NativeLinuxInstallBackend::new(options)?, start).await
    }

    struct NativeLinuxUninstallBackend {
        path: PathBuf,
        helper_home: PathBuf,
        installed_receipt: ServiceReceipt,
        restore_active: bool,
        restore_enabled: bool,
        stopped: bool,
        disabled: bool,
        definition: codex_helper_core::ManagedFileTransaction,
        receipt: ServiceReceiptTransaction,
    }

    impl NativeLinuxUninstallBackend {
        fn new() -> CliResult<Self> {
            let path = user_unit_path()?;
            let definition = codex_helper_core::ManagedFileTransaction::begin(
                path.clone(),
                MAX_SERVICE_DEFINITION_BYTES,
            )
            .map_err(|error| {
                CliError::Other(format!("begin systemd unit removal transaction: {error}"))
            })?;
            let receipt = ServiceReceiptTransaction::begin(proxy_home_dir()).map_err(|error| {
                CliError::Other(format!("begin service receipt removal: {error}"))
            })?;
            let installed_receipt = receipt
                .current()
                .map_err(|error| {
                    CliError::Other(format!(
                        "read the locked service receipt before systemd unit removal: {error}"
                    ))
                })?
                .ok_or_else(|| {
                    CliError::Other(
                        "refusing to remove the systemd user unit without a current service receipt. No service registration, definition, receipt, or runtime was changed; repair or reinstall the service before retrying"
                            .to_string(),
                    )
                })?;
            if installed_receipt.platform_backend() != ServicePlatformBackend::LinuxSystemdUser {
                return Err(CliError::Other(
                    "refusing to remove the systemd user unit with a receipt for another platform backend. No service registration, definition, receipt, or runtime was changed; repair or reinstall the service before retrying"
                        .to_string(),
                ));
            }
            verify_unix_service_definition_snapshot(
                &installed_receipt,
                definition.current().bytes(),
            )?;
            verify_registration(&path)?;
            Ok(Self {
                path,
                helper_home: installed_receipt.helper_home().to_path_buf(),
                installed_receipt,
                restore_active: false,
                restore_enabled: false,
                stopped: false,
                disabled: false,
                definition,
                receipt,
            })
        }

        fn revalidate_installed_authority(&self, operation: &str) -> CliResult<(bool, bool)> {
            verify_current_service_receipt_snapshot(
                &self.helper_home,
                Some(&self.installed_receipt),
                operation,
            )?;
            verify_unix_service_definition_at(
                &self.installed_receipt,
                ServicePlatformBackend::LinuxSystemdUser,
                &self.path,
            )?;
            verify_registration(&self.path)?;
            Ok((
                systemd_active_state_requires_stop(&systemd_property("ActiveState")?)?,
                systemd_unit_file_state_requires_disable(&systemd_property("UnitFileState")?)?,
            ))
        }

        fn revalidate_disabled_definition_authority(&self) -> CliResult<()> {
            verify_current_service_receipt_snapshot(
                &self.helper_home,
                Some(&self.installed_receipt),
                "remove the systemd user unit",
            )?;
            verify_unix_service_definition_at(
                &self.installed_receipt,
                ServicePlatformBackend::LinuxSystemdUser,
                &self.path,
            )?;
            verify_registration_with_unit_file_states(&self.path, &["disabled"], Some("no"))
        }

        fn revalidate_removed_definition_before_manager_reload(&self) -> CliResult<()> {
            verify_current_service_receipt_snapshot(
                &self.helper_home,
                Some(&self.installed_receipt),
                "reload systemd after removing the user unit",
            )?;
            verify_current_unix_service_definition_bytes(
                &self.path,
                None,
                "reload systemd after removing the user unit",
            )?;
            verify_registration_after_definition_removal(&self.path)
        }

        fn revalidate_before_receipt_removal(&self) -> CliResult<()> {
            verify_current_service_receipt_snapshot(
                &self.helper_home,
                Some(&self.installed_receipt),
                "remove the systemd service receipt",
            )?;
            verify_current_unix_service_definition_bytes(
                &self.path,
                None,
                "remove the systemd service receipt",
            )?;
            verify_registration_absent()
        }

        fn revalidate_restored_definition_before_manager_reload(&self) -> CliResult<()> {
            verify_current_service_receipt_snapshot(
                &self.helper_home,
                Some(&self.installed_receipt),
                "reload the restored systemd user unit",
            )?;
            verify_unix_service_definition_at(
                &self.installed_receipt,
                ServicePlatformBackend::LinuxSystemdUser,
                &self.path,
            )?;
            verify_registration_after_definition_removal(&self.path)
        }

        fn revalidate_restored_definition_before_enablement(&self) -> CliResult<()> {
            verify_current_service_receipt_snapshot(
                &self.helper_home,
                Some(&self.installed_receipt),
                "restore systemd user unit enablement",
            )?;
            verify_unix_service_definition_at(
                &self.installed_receipt,
                ServicePlatformBackend::LinuxSystemdUser,
                &self.path,
            )?;
            verify_replacement_registration_after_reload(&self.path)
        }
    }

    impl ServiceUninstallTransactionBackend for NativeLinuxUninstallBackend {
        fn stop_and_verify(&mut self) -> CliResult<()> {
            let (was_active, _) = self.revalidate_installed_authority(
                "stop the systemd user unit before uninstalling it",
            )?;
            self.restore_active = was_active;
            if was_active {
                self.stopped = true;
                systemctl(&["stop", LINUX_UNIT_NAME]).map_err(|error| {
                    CliError::Other(format!(
                        "stop {LINUX_UNIT_NAME} before uninstalling it: {error}"
                    ))
                })?;
            }
            let active_state = systemd_property("ActiveState")?;
            if systemd_active_state_requires_stop(&active_state)? {
                return Err(CliError::Other(format!(
                    "{LINUX_UNIT_NAME} still reports ActiveState={active_state} after systemctl stop; its definition and receipt were not removed"
                )));
            }
            Ok(())
        }

        fn disable_and_verify(&mut self) -> CliResult<()> {
            let (_, was_enabled) = self.revalidate_installed_authority(
                "disable the systemd user unit before removing it",
            )?;
            self.restore_enabled = was_enabled;
            if was_enabled {
                self.disabled = true;
                systemctl(&["disable", LINUX_UNIT_NAME]).map_err(|error| {
                    CliError::Other(format!(
                        "disable {LINUX_UNIT_NAME} before removing its definition: {error}"
                    ))
                })?;
            }
            let unit_file_state = systemd_property("UnitFileState")?;
            if systemd_unit_file_state_requires_disable(&unit_file_state)? {
                return Err(CliError::Other(format!(
                    "{LINUX_UNIT_NAME} still reports UnitFileState={unit_file_state} after systemctl disable; its definition and receipt were not removed"
                )));
            }
            Ok(())
        }

        fn remove_definition(&mut self) -> CliResult<()> {
            self.revalidate_disabled_definition_authority()?;
            self.definition.remove().map_err(|error| {
                CliError::Other(format!(
                    "remove systemd user unit {}: {error}",
                    self.path.display()
                ))
            })?;
            if self.definition.current().bytes().is_some() {
                return Err(CliError::Other(format!(
                    "systemd user unit {} still exists after removal",
                    self.path.display()
                )));
            }
            self.revalidate_removed_definition_before_manager_reload()?;
            systemctl(&["daemon-reload"]).map_err(|error| {
                CliError::Other(format!(
                    "reload systemd after removing {LINUX_UNIT_NAME}: {error}"
                ))
            })
        }

        fn remove_receipt(&mut self) -> CliResult<()> {
            self.revalidate_before_receipt_removal()?;
            self.receipt.remove().map_err(|error| {
                CliError::Other(format!(
                    "remove service receipt after systemd unit removal: {error}"
                ))
            })
        }

        fn rollback(&mut self) -> CliResult<()> {
            let mut failures = Vec::new();
            let definition_restored = match self.definition.rollback() {
                Ok(()) => true,
                Err(error) => {
                    failures.push(format!("restore the systemd user unit: {error}"));
                    false
                }
            };
            if let Err(error) = self.receipt.rollback() {
                failures.push(format!("restore the service receipt: {error}"));
            }
            if definition_restored {
                let reloaded = match self.revalidate_restored_definition_before_manager_reload() {
                    Ok(()) => match systemctl(&["daemon-reload"]) {
                        Ok(()) => true,
                        Err(error) => {
                            failures
                                .push(format!("reload the restored systemd user unit: {error}"));
                            false
                        }
                    },
                    Err(error) => {
                        failures.push(format!(
                            "revalidate the restored systemd user unit before manager reload: {error}"
                        ));
                        false
                    }
                };
                if reloaded
                    && self.disabled
                    && self.restore_enabled
                    && let Err(error) = self
                        .revalidate_restored_definition_before_enablement()
                        .and_then(|()| systemctl(&["enable", LINUX_UNIT_NAME]))
                {
                    failures.push(format!("re-enable the previous systemd user unit: {error}"));
                }
                if reloaded
                    && self.stopped
                    && self.restore_active
                    && let Err(error) = start()
                {
                    failures.push(format!("restart the previous systemd user unit: {error}"));
                }
            }
            if failures.is_empty() {
                Ok(())
            } else {
                Err(CliError::Other(failures.join("; ")))
            }
        }
    }

    pub(super) fn uninstall(stop_first: bool) -> CliResult<()> {
        run_service_uninstall_transaction(&mut NativeLinuxUninstallBackend::new()?, stop_first)
    }

    pub(super) fn verify_receipt_definition(receipt: &ServiceReceipt) -> CliResult<()> {
        let path = user_unit_path()?;
        verify_unix_service_definition_at(
            receipt,
            ServicePlatformBackend::LinuxSystemdUser,
            &path,
        )?;
        verify_registration(&path)
    }

    fn read_verified_installed_receipt() -> CliResult<ServiceReceipt> {
        let receipt = read_service_receipt(proxy_home_dir()).map_err(|error| {
            CliError::Other(format!(
                "refusing to manage the installed systemd user unit without a current service receipt: {error}. No service registration, definition, receipt, or runtime was changed; repair or reinstall the service before retrying"
            ))
        })?;
        verify_receipt_definition(&receipt)?;
        Ok(receipt)
    }

    pub(super) fn start() -> CliResult<()> {
        read_verified_installed_receipt()?;
        systemctl(&["start", LINUX_UNIT_NAME])
    }

    pub(super) fn stop() -> CliResult<()> {
        read_verified_installed_receipt()?;
        systemctl(&["stop", LINUX_UNIT_NAME])
    }

    pub(super) fn status() -> CliResult<ServiceStatus> {
        let path = user_unit_path()?;
        let load_state = systemd_property("LoadState")?;
        let loaded = systemd_load_state_is_present(&load_state)?;
        let active_state = systemd_property("ActiveState")?;
        let runtime_state = systemd_runtime_state(&active_state);
        if !path.exists() {
            if loaded
                && matches!(
                    runtime_state,
                    ServiceRuntimeState::Running
                        | ServiceRuntimeState::Starting
                        | ServiceRuntimeState::Stopping
                )
            {
                let mut status = base_status(runtime_state, false, false, Some(path));
                status.detail = Some(format!(
                    "systemd unit definition is absent, but the detached runtime remains {active_state} (for example after uninstall --keep-running)"
                ));
                return Ok(status);
            }
            return Ok(base_status(
                ServiceRuntimeState::NotInstalled,
                false,
                false,
                Some(path),
            ));
        }
        let unit_file_state = if loaded {
            Some(systemd_property("UnitFileState")?)
        } else {
            None
        };
        let mut status = base_status(
            if loaded {
                runtime_state
            } else {
                ServiceRuntimeState::Installed
            },
            true,
            unit_file_state.as_deref() == Some("enabled"),
            Some(path),
        );
        if runtime_state == ServiceRuntimeState::Unknown {
            status.detail = Some(format!(
                "systemd reports unknown ActiveState={active_state:?}"
            ));
        } else if let Some(unit_file_state) = unit_file_state.as_deref()
            && !matches!(unit_file_state, "enabled" | "disabled")
        {
            status.detail = Some(format!(
                "systemd reports nonstandard UnitFileState={unit_file_state:?}"
            ));
        }
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

    fn systemd_property(property: &str) -> CliResult<String> {
        let property = format!("--property={property}");
        systemctl_output(&["show", LINUX_UNIT_NAME, property.as_str(), "--value"])
    }

    fn verify_registration_with_unit_file_states(
        expected_definition_path: &Path,
        allowed_unit_file_states: &[&str],
        expected_need_daemon_reload: Option<&str>,
    ) -> CliResult<()> {
        let load_state = systemd_property("LoadState")?;
        let unit_file_state = systemd_property("UnitFileState")?;
        let fragment_path = systemd_property("FragmentPath")?;
        let drop_in_paths = systemd_property("DropInPaths")?;
        let need_daemon_reload = systemd_property("NeedDaemonReload")?;
        let fragment_matches = if load_state == "loaded" && !fragment_path.is_empty() {
            service_paths_identify_same_location(
                Path::new(&fragment_path),
                expected_definition_path,
            )
            .map_err(|error| {
                CliError::Other(format!(
                    "refusing to mutate the systemd user service because FragmentPath={fragment_path:?} is not a valid comparable authority: {error}. No service registration, definition, receipt, or runtime was changed; repair the matching unit before retrying"
                ))
            })?
        } else {
            false
        };
        let reload_matches = match expected_need_daemon_reload {
            Some(expected) => need_daemon_reload == expected,
            None => true,
        };
        if load_state == "loaded"
            && allowed_unit_file_states.contains(&unit_file_state.as_str())
            && fragment_matches
            && drop_in_paths.is_empty()
            && reload_matches
        {
            return Ok(());
        }
        Err(CliError::Other(format!(
            "refusing to mutate the systemd user service because its current registration no longer matches the transaction authority (LoadState={load_state:?}, UnitFileState={unit_file_state:?}, FragmentPath={fragment_path:?}, DropInPaths={drop_in_paths:?}, NeedDaemonReload={need_daemon_reload:?}, expected={}). No service registration, definition, receipt, or runtime was changed; retry from a fresh service state",
            expected_definition_path.display(),
        )))
    }

    fn verify_replacement_registration_before_reload(
        expected_definition_path: &Path,
        allow_absent: bool,
    ) -> CliResult<()> {
        let load_state = systemd_property("LoadState")?;
        if allow_absent && load_state == "not-found" {
            return Ok(());
        }
        verify_registration_with_unit_file_states(
            expected_definition_path,
            &["enabled", "disabled"],
            None,
        )
    }

    fn verify_replacement_registration_after_reload(
        expected_definition_path: &Path,
    ) -> CliResult<()> {
        verify_registration_with_unit_file_states(
            expected_definition_path,
            &["enabled", "disabled"],
            Some("no"),
        )
    }

    fn verify_registration_after_definition_removal(
        expected_definition_path: &Path,
    ) -> CliResult<()> {
        let load_state = systemd_property("LoadState")?;
        if load_state == "not-found" {
            return Ok(());
        }
        verify_registration_with_unit_file_states(expected_definition_path, &["disabled"], None)
    }

    fn verify_registration(expected_definition_path: &Path) -> CliResult<()> {
        verify_systemd_registration_snapshot(
            &systemd_property("LoadState")?,
            &systemd_property("UnitFileState")?,
            &systemd_property("FragmentPath")?,
            &systemd_property("DropInPaths")?,
            &systemd_property("NeedDaemonReload")?,
            expected_definition_path,
        )
    }

    fn verify_registration_absent() -> CliResult<()> {
        let load_state = systemd_property("LoadState")?;
        if load_state == "not-found" {
            return Ok(());
        }
        Err(CliError::Other(format!(
            "refusing to replace a systemd user registration with LoadState={load_state:?} without a current service receipt proving it. No service registration, definition, receipt, or runtime was changed; remove the unverified registration, then run `codex-helper service install`"
        )))
    }

    fn systemd_active_state_requires_stop(state: &str) -> CliResult<bool> {
        match state {
            "active" | "activating" | "deactivating" | "reloading" | "refreshing" => Ok(true),
            "inactive" | "failed" => Ok(false),
            state => Err(CliError::Other(format!(
                "refusing to uninstall {LINUX_UNIT_NAME} while systemd reports unknown ActiveState={state:?}"
            ))),
        }
    }

    fn systemd_runtime_state(state: &str) -> ServiceRuntimeState {
        match state {
            "active" | "reloading" | "refreshing" => ServiceRuntimeState::Running,
            "activating" => ServiceRuntimeState::Starting,
            "deactivating" => ServiceRuntimeState::Stopping,
            "inactive" | "failed" => ServiceRuntimeState::Stopped,
            _ => ServiceRuntimeState::Unknown,
        }
    }

    fn systemd_load_state_is_present(state: &str) -> CliResult<bool> {
        match state {
            "loaded" => Ok(true),
            "not-found" => Ok(false),
            state => Err(CliError::Other(format!(
                "refusing to uninstall {LINUX_UNIT_NAME} while systemd reports non-restorable LoadState={state:?}"
            ))),
        }
    }

    fn systemd_unit_file_state_requires_disable(state: &str) -> CliResult<bool> {
        match state {
            "enabled" => Ok(true),
            "disabled" => Ok(false),
            state => Err(CliError::Other(format!(
                "refusing to uninstall {LINUX_UNIT_NAME} while systemd reports non-restorable UnitFileState={state:?}; restore it to enabled or disabled first"
            ))),
        }
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
            legacy_installation: false,
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

#[cfg(any(target_os = "macos", target_os = "linux", test))]
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
        "[Unit]\nDescription=codex-helper resident relay\nAfter=network-online.target\nWants=network-online.target\nStartLimitIntervalSec=300\nStartLimitBurst=10\n\n[Service]\nType=simple\nEnvironment={helper_home}\nEnvironment={client_home}\nEnvironment={install_generation}\nExecStart={} serve {service_flag} --host {} --port {} --no-tui --service-managed\nRestart=on-failure\nRestartSec=10s\n\n[Install]\nWantedBy=default.target\n",
        systemd_quote(executable),
        options.host,
        options.port,
    )
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
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

#[cfg(any(target_os = "macos", target_os = "linux", windows, test))]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
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

    fn test_service_receipt(
        service: codex_helper_core::config::ServiceKind,
        admin_base_url: &str,
    ) -> ServiceReceipt {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-service-switch-test-{}",
            uuid::Uuid::new_v4()
        ));
        ServiceReceipt::new(
            service,
            root.join("helper"),
            root.join("client"),
            admin_base_url,
            ServicePlatformBackend::current().expect("supported test platform"),
            codex_helper_core::service_target::ServiceInstallGeneration::generate(),
        )
        .expect("build service receipt")
    }

    fn test_service_identity_receipt(
        service: codex_helper_core::config::ServiceKind,
        proxy_port: u16,
        helper_home: &Path,
        client_home: &Path,
    ) -> ServiceReceipt {
        ServiceReceipt::new(
            service,
            helper_home.to_path_buf(),
            client_home.to_path_buf(),
            codex_helper_core::proxy::local_admin_base_url_for_proxy_port(proxy_port),
            ServicePlatformBackend::current().expect("supported test platform"),
            codex_helper_core::service_target::ServiceInstallGeneration::generate(),
        )
        .expect("build service identity receipt")
    }

    fn test_unix_definition_receipt(
        backend: ServicePlatformBackend,
        service: codex_helper_core::config::ServiceKind,
        proxy_port: u16,
        helper_home: &Path,
        client_home: &Path,
        daemon_executable: &Path,
        install_generation: codex_helper_core::service_target::ServiceInstallGeneration,
    ) -> ServiceReceipt {
        ServiceReceipt::new(
            service,
            helper_home.to_path_buf(),
            client_home.to_path_buf(),
            codex_helper_core::proxy::local_admin_base_url_for_proxy_port(proxy_port),
            backend,
            install_generation,
        )
        .expect("build Unix service receipt")
        .with_daemon_executable(daemon_executable.to_path_buf())
        .expect("record Unix daemon executable authority")
    }

    #[test]
    fn unix_service_receipts_reconstruct_complete_platform_definitions() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-unix-definition-authority-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join("helper home");
        let client_home = root.join("client home");
        let executable = root.join("bin").join("codex-helper");

        for backend in [
            ServicePlatformBackend::MacosLaunchAgent,
            ServicePlatformBackend::LinuxSystemdUser,
        ] {
            let receipt = test_unix_definition_receipt(
                backend,
                codex_helper_core::config::ServiceKind::Codex,
                3211,
                &helper_home,
                &client_home,
                &executable,
                codex_helper_core::service_target::ServiceInstallGeneration::generate(),
            );
            let definition =
                expected_unix_service_definition(&receipt).expect("reconstruct definition");

            verify_unix_service_definition_snapshot(&receipt, Some(&definition))
                .expect("the exact reconstructed definition must be authoritative");
            let definition = String::from_utf8(definition).expect("definition is UTF-8");
            assert!(definition.contains("codex-helper"));
            assert!(definition.contains("3211"));
            assert!(definition.contains(receipt.install_generation().as_str()));
            assert!(definition.contains("CODEX_HELPER_HOME"));
            assert!(definition.contains("CODEX_HOME"));
            assert!(definition.contains("service-managed"));
        }
    }

    #[test]
    fn unix_service_definition_verifier_rejects_every_authoritative_field_tamper() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-unix-definition-tamper-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join("helper");
        let client_home = root.join("client");
        let executable = root.join("bin").join("codex-helper");
        let receipt = test_unix_definition_receipt(
            ServicePlatformBackend::LinuxSystemdUser,
            codex_helper_core::config::ServiceKind::Codex,
            3211,
            &helper_home,
            &client_home,
            &executable,
            codex_helper_core::service_target::ServiceInstallGeneration::generate(),
        );
        let options = service_install_options_from_receipt(&receipt, false)
            .expect("derive installed options");
        let mut tampered_definitions = Vec::new();

        tampered_definitions.push(render_systemd_unit(
            &root.join("other-bin").join("codex-helper"),
            &options,
        ));
        let mut tampered_command = render_systemd_unit(&executable, &options);
        tampered_command = tampered_command.replacen(" serve ", " proxy ", 1);
        tampered_definitions.push(tampered_command);
        let mut tampered_host = options.clone();
        tampered_host.host = IpAddr::from([127, 0, 0, 2]);
        tampered_definitions.push(render_systemd_unit(&executable, &tampered_host));
        let mut tampered_port = options.clone();
        tampered_port.port = 4321;
        tampered_definitions.push(render_systemd_unit(&executable, &tampered_port));
        let mut tampered_helper_home = options.clone();
        tampered_helper_home.helper_home = root.join("other-helper");
        tampered_definitions.push(render_systemd_unit(&executable, &tampered_helper_home));
        let mut tampered_client_home = options.clone();
        tampered_client_home.client_home = root.join("other-client");
        tampered_definitions.push(render_systemd_unit(&executable, &tampered_client_home));
        let mut tampered_generation = options.clone();
        tampered_generation.install_generation =
            codex_helper_core::service_target::ServiceInstallGeneration::generate();
        tampered_definitions.push(render_systemd_unit(&executable, &tampered_generation));
        let mut tampered_service = options;
        tampered_service.service_name = "claude";
        tampered_definitions.push(render_systemd_unit(&executable, &tampered_service));

        for definition in tampered_definitions {
            let error =
                verify_unix_service_definition_snapshot(&receipt, Some(definition.as_bytes()))
                    .expect_err("every authoritative field change must fail closed");
            assert!(error.to_string().contains("does not match"));
            assert!(error.to_string().contains("No service registration"));
        }
    }

    #[test]
    fn unix_service_definition_verifier_rejects_missing_definition_and_authority() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-unix-definition-missing-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join("helper");
        let client_home = root.join("client");
        let executable = root.join("bin").join("codex-helper");
        let generation = codex_helper_core::service_target::ServiceInstallGeneration::generate();
        let current = test_unix_definition_receipt(
            ServicePlatformBackend::LinuxSystemdUser,
            codex_helper_core::config::ServiceKind::Codex,
            3211,
            &helper_home,
            &client_home,
            &executable,
            generation.clone(),
        );
        let missing_definition = verify_unix_service_definition_snapshot(&current, None)
            .expect_err("a missing definition must fail closed");
        assert!(
            missing_definition
                .to_string()
                .contains("definition is missing")
        );

        let compatibility = ServiceReceipt::new(
            codex_helper_core::config::ServiceKind::Codex,
            helper_home,
            client_home,
            codex_helper_core::proxy::local_admin_base_url_for_proxy_port(3211),
            ServicePlatformBackend::LinuxSystemdUser,
            generation,
        )
        .expect("build schema-1 compatibility receipt");
        let missing_authority =
            verify_unix_service_definition_snapshot(&compatibility, Some(b"untrusted definition"))
                .expect_err("a receipt without daemon authority must fail closed");
        assert!(missing_authority.to_string().contains("daemon_executable"));
        assert!(missing_authority.to_string().contains("service install"));
    }

    #[test]
    fn unix_mutation_boundary_rereads_definition_and_receipt_after_replacement() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-unix-mutation-revalidation-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join("helper");
        let client_home = root.join("client");
        let executable = root.join("bin").join("codex-helper");
        let definition_path = root.join("codex-helper.service");
        std::fs::create_dir_all(&root).expect("create mutation revalidation test root");

        let installed = test_unix_definition_receipt(
            ServicePlatformBackend::LinuxSystemdUser,
            codex_helper_core::config::ServiceKind::Codex,
            3211,
            &helper_home,
            &client_home,
            &executable,
            codex_helper_core::service_target::ServiceInstallGeneration::generate(),
        );
        let expected_definition =
            expected_unix_service_definition(&installed).expect("render installed definition");
        std::fs::write(&definition_path, &expected_definition)
            .expect("write initial unit definition");

        {
            let definition_transaction = codex_helper_core::ManagedFileTransaction::begin(
                definition_path.clone(),
                MAX_SERVICE_DEFINITION_BYTES,
            )
            .expect("begin definition transaction");
            verify_unix_service_definition_at(
                &installed,
                ServicePlatformBackend::LinuxSystemdUser,
                &definition_path,
            )
            .expect("initial definition authority");

            // This is the deterministic replacement hook between transaction setup and mutation.
            std::fs::write(&definition_path, b"foreign unit definition")
                .expect("replace definition after initial snapshot");
            assert_eq!(
                definition_transaction.current().bytes(),
                Some(expected_definition.as_slice()),
                "the transaction cache intentionally retains its original snapshot"
            );
            let error = verify_unix_service_definition_at(
                &installed,
                ServicePlatformBackend::LinuxSystemdUser,
                &definition_path,
            )
            .expect_err("the mutation-boundary revalidation must reread the replacement");
            assert!(error.to_string().contains("does not match"), "{error}");
        }

        {
            let mut publish = ServiceReceiptTransaction::begin(helper_home.clone())
                .expect("begin initial receipt publication");
            publish
                .replace(&installed)
                .expect("publish initial receipt");
        }
        let replacement = test_unix_definition_receipt(
            ServicePlatformBackend::LinuxSystemdUser,
            codex_helper_core::config::ServiceKind::Codex,
            3211,
            &helper_home,
            &client_home,
            &executable,
            codex_helper_core::service_target::ServiceInstallGeneration::generate(),
        );
        {
            let receipt_transaction = ServiceReceiptTransaction::begin(helper_home.clone())
                .expect("begin receipt transaction");
            assert_eq!(
                receipt_transaction.current().expect("read cached receipt"),
                Some(installed.clone())
            );
            verify_current_service_receipt_snapshot(
                &helper_home,
                Some(&installed),
                "exercise receipt mutation boundary",
            )
            .expect("initial receipt authority");

            // The service receipt lock is advisory, so a non-cooperating writer is still a
            // deterministic way to prove that the boundary check is not using this cache.
            let mut replacement_bytes =
                serde_json::to_vec_pretty(&replacement).expect("serialize replacement receipt");
            replacement_bytes.push(b'\n');
            std::fs::write(
                crate::service_receipt::service_receipt_path(&helper_home),
                replacement_bytes,
            )
            .expect("replace receipt after initial snapshot");
            assert_eq!(
                receipt_transaction.current().expect("read cached receipt"),
                Some(installed.clone()),
                "the transaction cache intentionally retains its original snapshot"
            );
            let error = verify_current_service_receipt_snapshot(
                &helper_home,
                Some(&installed),
                "exercise receipt mutation boundary",
            )
            .expect_err("the mutation-boundary revalidation must reread the replacement");
            assert!(
                error
                    .to_string()
                    .contains("service receipt changed after the transaction began"),
                "{error}"
            );
        }

        std::fs::remove_dir_all(root).expect("remove mutation revalidation test root");
    }

    #[test]
    fn unix_install_replacement_verifies_the_old_executable_before_relocation() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-unix-definition-relocation-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join("helper");
        let client_home = root.join("client");
        let old_executable = root.join("old-bin").join("codex-helper");
        let new_executable = root.join("new-bin").join("codex-helper");
        let installed = test_unix_definition_receipt(
            ServicePlatformBackend::LinuxSystemdUser,
            codex_helper_core::config::ServiceKind::Codex,
            3211,
            &helper_home,
            &client_home,
            &old_executable,
            codex_helper_core::service_target::ServiceInstallGeneration::generate(),
        );
        let candidate = test_unix_definition_receipt(
            ServicePlatformBackend::LinuxSystemdUser,
            codex_helper_core::config::ServiceKind::Codex,
            3211,
            &helper_home,
            &client_home,
            &new_executable,
            codex_helper_core::service_target::ServiceInstallGeneration::generate(),
        );
        let installed_definition =
            expected_unix_service_definition(&installed).expect("render installed definition");
        let candidate_definition =
            expected_unix_service_definition(&candidate).expect("render candidate definition");
        assert_ne!(installed_definition, candidate_definition);

        verify_unix_service_install_replacement(
            Some(&installed),
            Some(&installed_definition),
            ServicePlatformBackend::LinuxSystemdUser,
        )
        .expect("a cargo-install relocation must verify against the old receipt authority");
        verify_unix_service_install_replacement(
            Some(&installed),
            Some(&candidate_definition),
            ServicePlatformBackend::LinuxSystemdUser,
        )
        .expect_err("the new binary cannot authorize replacement of a mismatched old definition");
    }

    #[test]
    fn unix_install_replacement_rejects_an_existing_definition_without_a_receipt() {
        let error = verify_unix_service_install_replacement(
            None,
            Some(b"externally installed definition"),
            ServicePlatformBackend::LinuxSystemdUser,
        )
        .expect_err("an unowned definition must not be replaced");
        assert!(
            error
                .to_string()
                .contains("without a current service receipt")
        );
        assert!(error.to_string().contains("No service registration"));
    }

    #[test]
    fn launchd_registration_accepts_unloaded_or_matching_plist_and_rejects_replacement() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-launchd-registration-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create registration test root");
        let expected = root.join("expected.plist");
        let replacement = root.join("replacement.plist");
        std::fs::write(&expected, b"expected").expect("write expected plist");
        std::fs::write(&replacement, b"replacement").expect("write replacement plist");

        verify_launchd_registration_snapshot(None, &expected)
            .expect("an explicitly unloaded LaunchAgent remains manageable");
        let matching_output = format!("path = {}\nstate = running", expected.display());
        verify_launchd_registration_snapshot(Some(&matching_output), &expected)
            .expect("the matching loaded plist is authoritative");
        let replacement_output = format!("path = {}\nstate = running", replacement.display());
        let replacement_error =
            verify_launchd_registration_snapshot(Some(&replacement_output), &expected)
                .expect_err("a loaded replacement plist must fail closed");
        assert!(
            replacement_error
                .to_string()
                .contains("not the receipt-authorized plist")
        );
        assert!(
            verify_launchd_registration_snapshot(Some("state = running"), &expected)
                .expect_err("a registration without a source plist is not authoritative")
                .to_string()
                .contains("did not expose")
        );

        std::fs::remove_dir_all(root).expect("remove registration test root");
    }

    #[test]
    fn systemd_registration_requires_loaded_enabled_expected_fragment() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-systemd-registration-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create registration test root");
        let expected = root.join("expected.service");
        let replacement = root.join("replacement.service");
        std::fs::write(&expected, b"expected").expect("write expected unit");
        std::fs::write(&replacement, b"replacement").expect("write replacement unit");

        verify_systemd_registration_snapshot(
            "loaded",
            "enabled",
            expected.to_string_lossy().as_ref(),
            "",
            "no",
            &expected,
        )
        .expect("the matching enabled unit is authoritative");
        for (load_state, unit_file_state, fragment_path, drop_in_paths, need_daemon_reload) in [
            (
                "not-found",
                "enabled",
                expected.to_string_lossy().into_owned(),
                "",
                "no",
            ),
            (
                "loaded",
                "disabled",
                expected.to_string_lossy().into_owned(),
                "",
                "no",
            ),
            (
                "loaded",
                "enabled",
                replacement.to_string_lossy().into_owned(),
                "",
                "no",
            ),
            (
                "loaded",
                "enabled",
                expected.to_string_lossy().into_owned(),
                "/tmp/codex-helper.service.d/override.conf",
                "no",
            ),
            (
                "loaded",
                "enabled",
                expected.to_string_lossy().into_owned(),
                "",
                "yes",
            ),
        ] {
            let error = verify_systemd_registration_snapshot(
                load_state,
                unit_file_state,
                &fragment_path,
                drop_in_paths,
                need_daemon_reload,
                &expected,
            )
            .expect_err("a replaced or disabled registration must fail closed");
            assert!(error.to_string().contains("registration does not match"));
            assert!(error.to_string().contains("systemctl --user"));
        }

        std::fs::remove_dir_all(root).expect("remove registration test root");
    }

    #[test]
    fn service_install_identity_allows_same_target_and_new_generation() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-service-install-identity-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join("helper");
        let client_home = root.join("client");
        let installed = test_service_identity_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            3211,
            &helper_home,
            &client_home,
        );
        let candidate = test_service_identity_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            3211,
            &helper_home,
            &client_home,
        );

        preflight_service_install_identity(
            Ok(installed),
            Ok(test_install_platform_status(
                ServiceRuntimeState::Running,
                true,
            )),
            &candidate,
        )
        .expect("same canonical identity must allow a binary update");
    }

    #[cfg(unix)]
    #[test]
    fn service_install_identity_resolves_client_home_alias_with_missing_tail() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-service-install-client-alias-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join("helper");
        let real_client_parent = root.join("real-client-parent");
        let client_alias = root.join("client-alias");
        std::fs::create_dir_all(&helper_home).expect("create helper home");
        std::fs::create_dir_all(&real_client_parent).expect("create real client parent");
        std::os::unix::fs::symlink(&real_client_parent, &client_alias)
            .expect("create client home alias");
        let installed_client_home = real_client_parent.join("missing").join("nested");
        let requested_client_home = client_alias.join("missing").join("nested");
        let installed = test_service_identity_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            3211,
            &helper_home,
            &installed_client_home,
        );
        let candidate = test_service_identity_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            3211,
            &helper_home,
            &requested_client_home,
        );

        preflight_service_install_identity(
            Ok(installed),
            Ok(test_install_platform_status(
                ServiceRuntimeState::Running,
                true,
            )),
            &candidate,
        )
        .expect("the same client home through an alias must preserve service identity");

        std::fs::remove_dir_all(root).expect("remove client alias test root");
    }

    #[cfg(windows)]
    #[test]
    fn service_install_identity_resolves_windows_case_alias_with_missing_tail() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-service-install-client-case-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join("helper");
        let client_parent = root.join("ClientParent");
        std::fs::create_dir_all(&helper_home).expect("create helper home");
        std::fs::create_dir_all(&client_parent).expect("create client parent");
        let installed_client_home = root.join("CLIENTPARENT").join("Missing").join("Nested");
        let requested_client_home = root.join("clientparent").join("missing").join("nested");
        let installed = test_service_identity_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            3211,
            &helper_home,
            &installed_client_home,
        );
        let candidate = test_service_identity_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            3211,
            &helper_home,
            &requested_client_home,
        );

        preflight_service_install_identity(
            Ok(installed),
            Ok(test_install_platform_status(
                ServiceRuntimeState::Running,
                true,
            )),
            &candidate,
        )
        .expect("Windows client-home case aliases must preserve service identity");

        std::fs::remove_dir_all(root).expect("remove client case test root");
    }

    #[test]
    fn service_client_home_windows_identity_is_case_and_separator_insensitive() {
        assert!(service_path_identities_equal_with_windows_semantics(
            Path::new(r"\\?\C:\Users\Operator\.Codex\missing"),
            Path::new("c:/users/operator/.codex/MISSING"),
            true,
        ));
        assert!(service_path_identities_equal_with_windows_semantics(
            Path::new(r"\\?\UNC\Server\Share\Codex"),
            Path::new(r"\\server\share\codex"),
            true,
        ));
        assert!(!service_path_identities_equal_with_windows_semantics(
            Path::new(r"C:\Users\Operator\.codex"),
            Path::new(r"C:\Users\Operator\.claude"),
            true,
        ));
    }

    #[test]
    fn service_install_identity_rejects_proxy_port_change() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-service-install-port-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join("helper");
        let client_home = root.join("client");
        let installed = test_service_identity_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            3211,
            &helper_home,
            &client_home,
        );
        let candidate = test_service_identity_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            4321,
            &helper_home,
            &client_home,
        );

        let error = preflight_service_install_identity(
            Ok(installed),
            Ok(test_install_platform_status(
                ServiceRuntimeState::Running,
                true,
            )),
            &candidate,
        )
        .expect_err("changing the installed proxy port must fail closed");
        let message = error.to_string();
        assert!(message.contains("proxy_target=http://127.0.0.1:3211"));
        assert!(message.contains("proxy_target=http://127.0.0.1:4321"));
        assert!(message.contains("codex-helper service uninstall"));
        assert!(message.contains("No service files were changed"));
    }

    #[test]
    fn service_install_identity_rejects_service_change() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-service-install-kind-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join("helper");
        let client_home = root.join("client");
        let installed = test_service_identity_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            3211,
            &helper_home,
            &client_home,
        );
        let candidate = test_service_identity_receipt(
            codex_helper_core::config::ServiceKind::Claude,
            3211,
            &helper_home,
            &client_home,
        );

        let error = preflight_service_install_identity(
            Ok(installed),
            Ok(test_install_platform_status(
                ServiceRuntimeState::Running,
                true,
            )),
            &candidate,
        )
        .expect_err("changing the installed service kind must fail closed");
        let message = error.to_string();
        assert!(message.contains("service=codex"));
        assert!(message.contains("service=claude"));
        assert!(message.contains("codex-helper service uninstall"));
    }

    #[test]
    fn service_install_identity_rejects_client_home_change() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-service-install-client-home-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join("helper");
        let installed_client_home = root.join("client-old");
        let requested_client_home = root.join("client-new");
        let installed = test_service_identity_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            3211,
            &helper_home,
            &installed_client_home,
        );
        let candidate = test_service_identity_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            3211,
            &helper_home,
            &requested_client_home,
        );

        let error = preflight_service_install_identity(
            Ok(installed),
            Ok(test_install_platform_status(
                ServiceRuntimeState::Running,
                true,
            )),
            &candidate,
        )
        .expect_err("changing the installed client home must fail closed");
        let message = error.to_string();
        assert!(message.contains(installed_client_home.to_string_lossy().as_ref()));
        assert!(message.contains(requested_client_home.to_string_lossy().as_ref()));
        assert!(message.contains("codex-helper service uninstall"));
    }

    #[test]
    fn service_install_identity_allows_only_proven_first_install_without_a_receipt() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-service-install-repair-test-{}",
            uuid::Uuid::new_v4()
        ));
        let candidate = test_service_identity_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            3211,
            &root.join("helper"),
            &root.join("client"),
        );

        preflight_service_install_identity(
            Err(ServiceReceiptError::Missing),
            Ok(test_install_platform_status(
                ServiceRuntimeState::NotInstalled,
                false,
            )),
            &candidate,
        )
        .expect("a missing receipt with a proven absent registration is a first install");

        let registered = preflight_service_install_identity(
            Err(ServiceReceiptError::Missing),
            Ok(test_install_platform_status(
                ServiceRuntimeState::Running,
                true,
            )),
            &candidate,
        )
        .expect_err("a platform registration without a receipt must fail closed");
        assert!(registered.to_string().contains("registration"));
        assert!(registered.to_string().contains("version that created"));

        let unknown = preflight_service_install_identity(
            Err(ServiceReceiptError::Missing),
            Err(CliError::Other(
                "injected platform registration query failure".to_string(),
            )),
            &candidate,
        )
        .expect_err("an unverified platform registration state must fail closed");
        assert!(unknown.to_string().contains("query failure"));

        let legacy = preflight_service_install_identity(
            Err(ServiceReceiptError::LegacySchema {
                schema_version: Some(0),
            }),
            Ok(test_install_platform_status(
                ServiceRuntimeState::NotInstalled,
                false,
            )),
            &candidate,
        )
        .expect_err("legacy receipts require an explicit uninstall or migration");
        assert!(legacy.to_string().contains("legacy schema"));
    }

    #[test]
    fn service_install_identity_allows_only_explicit_windows_legacy_migration() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-windows-legacy-migration-test-{}",
            uuid::Uuid::new_v4()
        ));
        let candidate = ServiceReceipt::new(
            codex_helper_core::config::ServiceKind::Codex,
            root.join("helper"),
            root.join("client"),
            codex_helper_core::proxy::local_admin_base_url_for_proxy_port(3211),
            ServicePlatformBackend::WindowsScheduledTask,
            codex_helper_core::service_target::ServiceInstallGeneration::generate(),
        )
        .expect("build Windows migration receipt");
        let mut legacy = test_install_platform_status(ServiceRuntimeState::Running, true);
        legacy.platform = ServicePlatform::Windows;
        legacy.legacy_installation = true;

        let preflight = preflight_service_install_identity(
            Err(ServiceReceiptError::Missing),
            Ok(legacy.clone()),
            &candidate,
        )
        .expect("an explicitly identified Windows legacy installation must reach migration");
        assert!(preflight.installed_receipt.is_none());
        assert_eq!(preflight.platform_state, ServiceRuntimeState::Running);

        legacy.legacy_installation = false;
        let error = preflight_service_install_identity(
            Err(ServiceReceiptError::Missing),
            Ok(legacy),
            &candidate,
        )
        .expect_err("a current Windows registration without a receipt must still fail closed");
        assert!(error.to_string().contains("registration"));
    }

    #[test]
    fn service_uninstall_requires_a_current_receipt_even_when_runtime_is_kept() {
        let future = validate_service_receipt_for_uninstall(
            Err(ServiceReceiptError::UnsupportedSchema { schema_version: 99 }),
            ServicePlatformBackend::current(),
        )
        .expect_err("a future receipt must not be deleted by uninstall");
        assert!(future.to_string().contains("newer unsupported schema"));
        assert!(future.to_string().contains("No service registration"));

        let receipt = test_service_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            "http://127.0.0.1:4211",
        );
        let foreign_backend = match receipt.platform_backend() {
            ServicePlatformBackend::WindowsScheduledTask => {
                ServicePlatformBackend::MacosLaunchAgent
            }
            ServicePlatformBackend::MacosLaunchAgent | ServicePlatformBackend::LinuxSystemdUser => {
                ServicePlatformBackend::WindowsScheduledTask
            }
        };
        let mismatch = validate_service_receipt_for_uninstall(Ok(receipt), Some(foreign_backend))
            .expect_err("a receipt for another platform backend must remain untouched");
        assert!(mismatch.to_string().contains("refusing to uninstall"));
    }

    #[test]
    fn service_install_transaction_rechecks_platform_registration_with_missing_receipt() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-service-install-transaction-preflight-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join("helper");
        let client_home = root.join("client");
        std::fs::create_dir_all(&helper_home).expect("create helper home");
        std::fs::create_dir_all(&client_home).expect("create client home");
        let options = ServiceInstallOptions {
            service_name: "codex",
            host: IpAddr::from([127, 0, 0, 1]),
            port: 3211,
            start: true,
            helper_home,
            client_home,
            install_generation:
                codex_helper_core::service_target::ServiceInstallGeneration::generate(),
        };

        let daemon_executable = root.join("bin").join("codex-helper");
        let error = begin_service_receipt_transaction_with_daemon_executable_and_status(
            &options,
            &daemon_executable,
            || {
                Ok(test_install_platform_status(
                    ServiceRuntimeState::Stopped,
                    true,
                ))
            },
        )
        .expect_err("the transaction-local registration check must reject a missing receipt");
        assert!(error.to_string().contains("registration"));

        std::fs::remove_dir_all(root).expect("remove test root");
    }

    #[test]
    fn service_install_transaction_allows_verified_daemon_relocation() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-service-install-relocation-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join("helper");
        let client_home = root.join("client");
        std::fs::create_dir_all(&helper_home).expect("create helper home");
        std::fs::create_dir_all(&client_home).expect("create client home");
        let mut options = ServiceInstallOptions {
            service_name: "codex",
            host: IpAddr::from([127, 0, 0, 1]),
            port: 3211,
            start: true,
            helper_home: helper_home.clone(),
            client_home,
            install_generation:
                codex_helper_core::service_target::ServiceInstallGeneration::generate(),
        };
        let old_executable = root.join("old-bin").join("codex-helper");
        let new_executable = root.join("new-bin").join("codex-helper");
        let installed = service_receipt_with_daemon_executable(&options, &old_executable)
            .expect("build installed receipt");
        {
            let mut transaction = ServiceReceiptTransaction::begin(&helper_home)
                .expect("begin installed receipt transaction");
            transaction
                .replace(&installed)
                .expect("publish installed receipt");
        }
        options.install_generation =
            codex_helper_core::service_target::ServiceInstallGeneration::generate();

        let (transaction, candidate, preflight) =
            begin_service_receipt_transaction_with_daemon_executable_and_status(
                &options,
                &new_executable,
                || {
                    Ok(test_install_platform_status(
                        ServiceRuntimeState::Running,
                        true,
                    ))
                },
            )
            .expect("verified daemon relocation must preserve service identity");
        assert_eq!(
            candidate.daemon_executable(),
            Some(new_executable.as_path())
        );
        assert_eq!(
            preflight
                .installed_receipt
                .as_ref()
                .and_then(ServiceReceipt::daemon_executable),
            Some(old_executable.as_path())
        );

        drop(transaction);
        std::fs::remove_dir_all(root).expect("remove relocation test root");
    }

    #[test]
    fn explicit_service_stop_restores_before_stop_while_restart_preserves_switch() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());
        run_service_stop_with_switch_policy(
            ServiceStopSwitchPolicy::RestoreMatchingCodexSwitch,
            || {
                events.borrow_mut().push("restore");
                Ok(())
            },
            || {
                events.borrow_mut().push("stop");
                Ok(())
            },
        )
        .expect("explicit stop");
        assert_eq!(&*events.borrow(), &["restore", "stop"]);

        events.borrow_mut().clear();
        run_service_stop_with_switch_policy(
            ServiceStopSwitchPolicy::PreserveForRestart,
            || -> CliResult<()> { unreachable!("restart must not restore the client switch") },
            || {
                events.borrow_mut().push("stop");
                Ok(())
            },
        )
        .expect("restart stop phase");
        assert_eq!(&*events.borrow(), &["stop"]);
    }

    #[test]
    fn no_start_replacement_restores_only_when_it_will_stop_a_running_runtime() {
        let receipt = test_service_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            "http://127.0.0.1:4211",
        );
        let calls = std::cell::Cell::new(0);
        let preparation = reconcile_service_switch_before_no_start_install(
            false,
            ServiceRuntimeState::Running,
            Some(&receipt),
            |_, _| {
                calls.set(calls.get() + 1);
                Ok(
                    codex_helper_core::codex_switch::CodexSwitchTargetRestoreOutcome::Unchanged(
                        test_switch_status(
                            codex_helper_core::codex_switch::CodexSwitchPhase::Off,
                            false,
                            false,
                            None,
                        ),
                    ),
                )
            },
        )
        .expect("prepare a no-start replacement");
        assert_eq!(preparation, ServiceSwitchUninstallPreparation::Unchanged);
        assert_eq!(calls.get(), 1);

        for (start, state) in [
            (true, ServiceRuntimeState::Running),
            (false, ServiceRuntimeState::Stopped),
        ] {
            let preparation = reconcile_service_switch_before_no_start_install(
                start,
                state,
                Some(&receipt),
                |_, _| -> Result<_, codex_helper_core::codex_switch::CodexSwitchError> {
                    unreachable!("this install does not make a running runtime unavailable")
                },
            )
            .expect("skip switch reconciliation");
            assert_eq!(
                preparation,
                ServiceSwitchUninstallPreparation::NotApplicable
            );
        }
    }

    fn test_install_platform_status(state: ServiceRuntimeState, installed: bool) -> ServiceStatus {
        ServiceStatus {
            platform: ServicePlatform::current(),
            service_name: "test-service".to_string(),
            state,
            installed,
            legacy_installation: false,
            autostart: installed,
            service_definition: None,
            log_directory: PathBuf::from("/tmp/test-service-logs"),
            detail: None,
            receipt_state: ServiceReceiptState::Absent,
            credential_context: ServiceCredentialContext::Unverified,
            runtime_identity_verified: false,
            install_generation: None,
        }
    }

    fn test_switch_status(
        phase: codex_helper_core::codex_switch::CodexSwitchPhase,
        enabled: bool,
        managed: bool,
        base_url: Option<&str>,
    ) -> codex_helper_core::codex_switch::CodexSwitchStatus {
        codex_helper_core::codex_switch::CodexSwitchStatus {
            phase,
            enabled,
            model_provider: None,
            managed,
            base_url: base_url.map(str::to_string),
            client_patch: None,
            recovery_reason: None,
            config_path: PathBuf::from("/tmp/codex/config.toml"),
            state_path: PathBuf::from("/tmp/helper/state/codex-switch.json"),
        }
    }

    #[test]
    fn service_receipt_admin_target_has_one_verified_inverse() {
        let receipt = test_service_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            "http://127.0.0.1:4211",
        );
        assert_eq!(
            local_proxy_target_from_service_receipt(&receipt)
                .expect("derive proxy target")
                .as_str(),
            "http://127.0.0.1:3211"
        );

        let ambiguous = test_service_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            "http://127.0.0.1:63536",
        );
        assert!(
            local_proxy_target_from_service_receipt(&ambiguous)
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
    }

    #[test]
    fn matching_service_switch_is_restored_before_uninstall() {
        let receipt = test_service_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            "http://127.0.0.1:4211",
        );
        let preparation =
            reconcile_service_switch_before_uninstall(&receipt, true, |client_home, target| {
                assert_eq!(client_home, receipt.client_home());
                assert_eq!(target.as_str(), "http://127.0.0.1:3211");
                Ok(
                    codex_helper_core::codex_switch::CodexSwitchTargetRestoreOutcome::Restored(
                        codex_helper_core::codex_switch::CodexSwitchOutcome {
                            change: codex_helper_core::codex_switch::CodexSwitchChange::Removed,
                            status: test_switch_status(
                                codex_helper_core::codex_switch::CodexSwitchPhase::Off,
                                false,
                                false,
                                None,
                            ),
                            restore_lease: None,
                        },
                    ),
                )
            })
            .expect("prepare uninstall");

        assert_eq!(preparation, ServiceSwitchUninstallPreparation::Restored);
    }

    #[test]
    fn different_local_or_remote_switch_target_is_not_modified() {
        let receipt = test_service_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            "http://127.0.0.1:4211",
        );
        for target in ["http://127.0.0.1:4321", "https://relay.example/v1"] {
            let preparation = reconcile_service_switch_before_uninstall(&receipt, true, |_, _| {
                Ok(
                    codex_helper_core::codex_switch::CodexSwitchTargetRestoreOutcome::Unchanged(
                        test_switch_status(
                            codex_helper_core::codex_switch::CodexSwitchPhase::Applied,
                            true,
                            true,
                            Some(target),
                        ),
                    ),
                )
            })
            .expect("different target does not block uninstall");

            let ServiceSwitchUninstallPreparation::Warning(warning) = preparation else {
                panic!("different target must produce an actionable warning")
            };
            assert!(warning.contains(target));
            assert!(warning.contains("was not modified"));
        }
    }

    #[test]
    fn active_matching_foreign_or_edited_switch_fails_closed() {
        let receipt = test_service_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            "http://127.0.0.1:4211",
        );
        for active_target in [
            "http://127.0.0.1:3211",
            "http://localhost:3211",
            "http://[::1]:3211",
        ] {
            for managed in [false, true] {
                let error = reconcile_service_switch_before_uninstall(&receipt, true, |_, _| {
                    Ok(
                        codex_helper_core::codex_switch::CodexSwitchTargetRestoreOutcome::Unchanged(
                            test_switch_status(
                                codex_helper_core::codex_switch::CodexSwitchPhase::RecoveryRequired,
                                true,
                                managed,
                                Some(active_target),
                            ),
                        ),
                    )
                })
                .expect_err("active matching unsafe switch must block uninstall");
                assert!(
                    error
                        .to_string()
                        .contains("still selects the service target")
                );
                assert!(
                    error
                        .to_string()
                        .contains("No client or service files were changed")
                );
            }
        }
    }

    #[test]
    fn matching_switch_restore_failure_prevents_platform_mutation() {
        let receipt = test_service_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            "http://127.0.0.1:4211",
        );
        let platform_called = std::cell::Cell::new(false);
        let error = run_service_uninstall_with_switch_preflight(
            true,
            || {
                reconcile_service_switch_before_uninstall(&receipt, true, |_, _| {
                    Err(
                        codex_helper_core::codex_switch::CodexSwitchError::RecoveryRequired {
                            reason: "injected matching switch CAS failure".to_string(),
                        },
                    )
                })
                .map(|_| ())
            },
            |_| {
                platform_called.set(true);
                Ok(())
            },
        )
        .expect_err("switch failure must fail closed");

        assert!(!platform_called.get());
        assert!(error.to_string().contains("switch CAS failure"));
        assert!(
            error
                .to_string()
                .contains("registration, receipt, and runtime were left in place")
        );
    }

    #[test]
    fn platform_rollback_keeps_a_successfully_restored_switch_off() {
        let switch_enabled = std::cell::Cell::new(true);
        let mut backend = FakeServiceUninstallBackend {
            fail_at: Some("remove_definition"),
            ..FakeServiceUninstallBackend::default()
        };

        let error = run_service_uninstall_with_switch_preflight(
            true,
            || {
                switch_enabled.set(false);
                Ok(())
            },
            |_| run_service_uninstall_transaction(&mut backend, true),
        )
        .expect_err("platform uninstall failure must be reported");

        assert!(!switch_enabled.get());
        assert!(backend.runtime_running);
        assert!(backend.enabled);
        assert_eq!(backend.definition, Some(b"original-definition".as_slice()));
        assert_eq!(backend.receipt, Some(b"original-receipt".as_slice()));
        assert!(error.to_string().contains("were restored"));
    }

    #[test]
    fn keep_running_and_claude_uninstall_never_touch_codex_switch() {
        let codex = test_service_receipt(
            codex_helper_core::config::ServiceKind::Codex,
            "http://127.0.0.1:4211",
        );
        let claude = test_service_receipt(
            codex_helper_core::config::ServiceKind::Claude,
            "http://127.0.0.1:4210",
        );
        for (receipt, stop_first) in [(&codex, false), (&claude, true)] {
            let called = std::cell::Cell::new(false);
            let preparation =
                reconcile_service_switch_before_uninstall(receipt, stop_first, |_, _| {
                    called.set(true);
                    unreachable!("Codex switch must not be called")
                })
                .expect("skip Codex switch reconciliation");
            assert_eq!(
                preparation,
                ServiceSwitchUninstallPreparation::NotApplicable
            );
            assert!(!called.get());
        }
    }

    fn test_service_status(state: ServiceRuntimeState) -> ServiceStatus {
        ServiceStatus {
            platform: ServicePlatform::current(),
            service_name: "test-service".to_string(),
            state,
            installed: true,
            legacy_installation: false,
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

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_stop_is_idempotent_for_an_unloaded_launch_agent() {
        let bootout_called = std::cell::Cell::new(false);
        macos::stop_launch_agent_with(
            || {
                Err(CliError::Other(
                    "launchctl failed: Could not find service".to_string(),
                ))
            },
            || {
                bootout_called.set(true);
                Ok(String::new())
            },
            || -> CliResult<String> { unreachable!("an unloaded job needs no verification") },
        )
        .expect("an unloaded LaunchAgent is already stopped");
        assert!(!bootout_called.get());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_stop_surfaces_a_real_launchctl_query_failure() {
        let error = macos::stop_launch_agent_with(
            || {
                Err(CliError::Other(
                    "launchctl failed: Input/output error".to_string(),
                ))
            },
            || -> CliResult<String> {
                unreachable!("query failure must prevent launchctl bootout")
            },
            || -> CliResult<String> {
                unreachable!("query failure must prevent unload verification")
            },
        )
        .expect_err("a real launchctl query failure must not look like an absent job");
        assert!(error.to_string().contains("query LaunchAgent state"));
        assert!(error.to_string().contains("Input/output error"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_stop_accepts_a_job_that_disappears_after_the_query() {
        macos::stop_launch_agent_with(
            || Ok("state = running".to_string()),
            || {
                Err(CliError::Other(
                    "launchctl failed: No such process".to_string(),
                ))
            },
            || {
                Err(CliError::Other(
                    "launchctl failed: Could not find service".to_string(),
                ))
            },
        )
        .expect("a concurrently unloaded LaunchAgent is already stopped");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_stop_requires_launchd_to_confirm_the_job_is_unloaded() {
        let error = macos::stop_launch_agent_with(
            || Ok("state = running".to_string()),
            || Ok(String::new()),
            || Ok("state = running".to_string()),
        )
        .expect_err("a still-loaded job must not be reported as stopped");
        assert!(error.to_string().contains("still loaded"));
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
        let (unit, remaining) = document
            .split_once("\n\n[Service]\n")
            .expect("separate Unit and Service sections");
        let (service, _) = remaining
            .split_once("\n\n[Install]\n")
            .expect("separate Service and Install sections");
        assert!(unit.contains("StartLimitIntervalSec=300"));
        assert!(unit.contains("StartLimitBurst=10"));
        assert!(!service.contains("StartLimit"));
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
    fn service_executable_prefers_only_an_existing_ch_sibling() {
        let ch = Path::new("C:/tools/ch.exe");
        let expected = PathBuf::from("C:/tools/codex-helper.exe");
        let selected =
            select_service_executable_candidate_with(ch, |candidate| candidate == expected);
        assert_eq!(selected, expected);

        let missing = select_service_executable_candidate_with(ch, |_| false);
        assert_eq!(missing, ch);

        let unix_ch = Path::new("/tools/ch");
        let unix_expected = PathBuf::from("/tools/codex-helper");
        let unix_selected = select_service_executable_candidate_with(unix_ch, |candidate| {
            candidate == unix_expected
        });
        assert_eq!(unix_selected, unix_expected);

        let canonical = Path::new("C:/tools/codex-helper.exe");
        let unchanged = select_service_executable_candidate_with(canonical, |_| {
            panic!("a canonical entrypoint must not probe a basename-derived sibling")
        });
        assert_eq!(unchanged, canonical);
    }

    #[cfg(windows)]
    #[test]
    fn canonical_windows_service_executable_only_simplifies_legacy_safe_disk_paths() {
        use std::os::windows::ffi::OsStrExt;

        assert_eq!(
            without_windows_verbatim_prefix(Path::new(
                r"\\?\C:\Users\Operator\bin\codex-helper.exe"
            )),
            PathBuf::from(r"C:\Users\Operator\bin\codex-helper.exe")
        );

        let verbatim_unc = Path::new(r"\\?\UNC\Server\Share\bin\codex-helper.exe");
        assert_eq!(without_windows_verbatim_prefix(verbatim_unc), verbatim_unc);

        let reserved = Path::new(r"\\?\C:\Users\CON\bin\codex-helper.exe");
        assert_eq!(without_windows_verbatim_prefix(reserved), reserved);
        let superscript_reserved = Path::new(r"\\?\C:\Users\COM¹.txt\codex-helper.exe");
        assert_eq!(
            without_windows_verbatim_prefix(superscript_reserved),
            superscript_reserved
        );

        let long_component = "a".repeat(240);
        let long_path = PathBuf::from(format!(
            r"\\?\C:\Users\{long_component}\bin\codex-helper.exe"
        ));
        assert!(long_path.as_os_str().encode_wide().count() > 260);
        assert_eq!(without_windows_verbatim_prefix(&long_path), long_path);

        let prefix = r"\\?\C:\";
        let suffix = r"\codex-helper.exe";
        let exact_limit_component = "a".repeat(260 - prefix.len() - suffix.len());
        let exact_limit = PathBuf::from(format!(r"{prefix}{exact_limit_component}{suffix}"));
        assert_eq!(exact_limit.as_os_str().encode_wide().count(), 260);
        assert_eq!(without_windows_verbatim_prefix(&exact_limit), exact_limit);
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
    fn windows_path_comparison_preserves_drive_root_semantics() {
        assert!(windows_paths_equal(r"C:\", "c:/"));
        assert!(windows_paths_equal(r"\\?\C:\", "c:/"));
        assert!(!windows_paths_equal(r"C:\", "C:"));
        assert!(windows_paths_equal(r"\\server\share\", r"\\SERVER\SHARE"));
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
        let root = std::env::temp_dir().join("codex-helper-windows-task-record-test");
        let executable = root.join("bin").join("codex-helper.exe");
        let options = ServiceInstallOptions {
            service_name: "codex",
            host: IpAddr::from([127, 0, 0, 1]),
            port: 3211,
            start: true,
            helper_home: root.join("helper"),
            client_home: root.join("client"),
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
            version: "1.2".to_string(),
            description: "Resident codex-helper relay for the current user.".to_string(),
            owner_sid: sid.to_string(),
            principal_count: 1,
            principal_id: "Author".to_string(),
            actions_context: "Author".to_string(),
            state: 3,
            enabled: true,
            multiple_instances: "IgnoreNew".to_string(),
            disallow_start_if_on_batteries: false,
            stop_if_going_on_batteries: false,
            allow_hard_terminate: true,
            start_when_available: true,
            run_only_if_network_available: false,
            allow_start_on_demand: true,
            hidden: false,
            run_only_if_idle: false,
            wake_to_run: false,
            execution_time_limit: "PT0S".to_string(),
            priority: 7,
            restart_interval: "PT10S".to_string(),
            restart_count: 10,
            action_count: 1,
            execute: executable.display().to_string(),
            arguments,
            working_directory: executable
                .parent()
                .expect("daemon executable parent")
                .display()
                .to_string(),
            logon_type: "Interactive".to_string(),
            run_level: "Limited".to_string(),
            trigger_count: 1,
            trigger_enabled: true,
            trigger_type: "MSFT_TaskLogonTrigger".to_string(),
            trigger_user_sid: sid.to_string(),
        };
        let receipt_for_options = |options: &ServiceInstallOptions| {
            ServiceReceipt::new(
                codex_helper_core::config::ServiceKind::Codex,
                options.helper_home.clone(),
                options.client_home.clone(),
                codex_helper_core::proxy::local_admin_base_url_for_proxy_port(options.port),
                ServicePlatformBackend::WindowsScheduledTask,
                options.install_generation.clone(),
            )
            .expect("build Windows task receipt")
        };
        let compatibility_receipt = receipt_for_options(&options);
        let installed_receipt = receipt_for_options(&options)
            .with_daemon_executable(executable.clone())
            .expect("record installed daemon executable");
        let relocated_cli = root.join("new-release").join("ch.exe");

        assert!(
            verify_windows_task_record(&record, &task_name, sid, &executable, &options).is_ok()
        );
        assert!(
            verify_existing_windows_task_for_replacement(
                &record,
                &task_name,
                sid,
                &executable,
                None,
            )
            .is_err()
        );
        assert!(
            verify_existing_windows_task_for_replacement(
                &record,
                &task_name,
                sid,
                &relocated_cli,
                Some(&installed_receipt),
            )
            .is_ok()
        );
        assert!(
            verify_existing_windows_task_for_replacement(
                &record,
                &task_name,
                sid,
                &executable,
                Some(&compatibility_receipt),
            )
            .is_ok()
        );
        let compatibility_error = verify_existing_windows_task_for_replacement(
            &record,
            &task_name,
            sid,
            &relocated_cli,
            Some(&compatibility_receipt),
        )
        .expect_err("a receipt without executable authority keeps the current-path boundary");
        assert!(
            compatibility_error
                .to_string()
                .contains("no daemon_executable")
        );
        assert!(
            compatibility_error
                .to_string()
                .contains("current CLI executable")
        );
        let mut foreign = record.clone();
        foreign.owner_sid = "S-1-5-21-100-200-300-999".to_string();
        assert!(!windows_task_owner_matches(&foreign, sid));
        assert!(
            verify_windows_task_record(&foreign, &task_name, sid, &executable, &options).is_err()
        );
        let mut changed_action = record.clone();
        changed_action.execute = "C:/Windows/System32/cmd.exe".to_string();
        assert!(
            verify_windows_task_record(&changed_action, &task_name, sid, &executable, &options)
                .is_err()
        );

        let mut changed_settings = record.clone();
        changed_settings.hidden = true;
        assert!(
            verify_windows_task_record(&changed_settings, &task_name, sid, &executable, &options,)
                .is_err()
        );

        let mut changed_context = record.clone();
        changed_context.actions_context = "OtherPrincipal".to_string();
        assert!(
            verify_windows_task_record(&changed_context, &task_name, sid, &executable, &options,)
                .is_err()
        );

        let mut replacement_options = options.clone();
        replacement_options.install_generation =
            codex_helper_core::service_target::ServiceInstallGeneration::generate();
        let replacement_receipt = receipt_for_options(&replacement_options)
            .with_daemon_executable(executable.clone())
            .expect("record replacement receipt daemon executable");
        assert!(
            verify_existing_windows_task_for_replacement(
                &record,
                &task_name,
                sid,
                &relocated_cli,
                Some(&replacement_receipt),
            )
            .is_err()
        );
    }

    #[test]
    fn legacy_fixed_task_requires_the_known_codex_helper_definition() {
        let sid = "S-1-5-21-100-200-300-400";
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
        let mut legacy_arguments = service_task_arguments(&options);
        legacy_arguments.truncate(12);
        let record = WindowsTaskRecord {
            task_name: WINDOWS_TASK_BASENAME.to_string(),
            task_path: "\\".to_string(),
            version: "1.2".to_string(),
            description: "Resident codex-helper relay for the current user.".to_string(),
            owner_sid: sid.to_string(),
            principal_count: 1,
            principal_id: "Author".to_string(),
            actions_context: "Author".to_string(),
            state: 3,
            enabled: true,
            multiple_instances: "IgnoreNew".to_string(),
            disallow_start_if_on_batteries: false,
            stop_if_going_on_batteries: false,
            allow_hard_terminate: true,
            start_when_available: true,
            run_only_if_network_available: false,
            allow_start_on_demand: true,
            hidden: false,
            run_only_if_idle: false,
            wake_to_run: false,
            execution_time_limit: "PT0S".to_string(),
            priority: 7,
            restart_interval: "PT10S".to_string(),
            restart_count: 10,
            action_count: 1,
            execute: executable.display().to_string(),
            arguments: legacy_arguments
                .iter()
                .map(|argument| quote_windows_argument(argument.to_string_lossy().as_ref()))
                .collect::<Vec<_>>()
                .join(" "),
            working_directory: "C:/Users/test/.cargo/bin".to_string(),
            logon_type: "InteractiveToken".to_string(),
            run_level: "LeastPrivilege".to_string(),
            trigger_count: 1,
            trigger_enabled: true,
            trigger_type: "MSFT_TaskLogonTrigger".to_string(),
            trigger_user_sid: sid.to_string(),
        };

        let invocation = verify_legacy_fixed_windows_task_record(&record, sid, executable).unwrap();
        assert!(invocation.matches_install(&options));
        let mut different_service = options.clone();
        different_service.service_name = "claude";
        assert!(!invocation.matches_install(&different_service));
        assert!(invocation.conflicts_with_install(&different_service));

        let mut arbitrary_action = record.clone();
        arbitrary_action.arguments = "service task-run --service-name codex --host 127.0.0.1 --port 3211 --helper-home C:/Users/test/.codex-helper --client-home C:/Users/test/.codex --exec calc.exe".to_string();
        assert!(
            verify_legacy_fixed_windows_task_record(&arbitrary_action, sid, executable).is_err()
        );

        let mut elevated = record;
        elevated.run_level = "Highest".to_string();
        assert!(verify_legacy_fixed_windows_task_record(&elevated, sid, executable).is_err());
    }

    #[test]
    fn legacy_scm_requires_local_system_and_the_known_dispatcher_command() {
        let executable = Path::new("C:/Program Files/codex-helper/codex-helper.exe");
        let arguments = [
            executable.to_string_lossy().into_owned(),
            "service".to_string(),
            "run".to_string(),
            "--service-name".to_string(),
            "codex".to_string(),
            "--host".to_string(),
            "127.0.0.1".to_string(),
            "--port".to_string(),
            "3211".to_string(),
            "--helper-home".to_string(),
            "C:/Users/test/.codex-helper".to_string(),
        ];
        let definition = LegacyWindowsScmDefinition {
            own_process: true,
            start_type: 2,
            error_control: 1,
            dependencies: Vec::new(),
            account_name: Some("LocalSystem".to_string()),
            display_name: "codex-helper relay".to_string(),
            load_order_group: None,
            command_line: arguments
                .iter()
                .map(|argument| quote_windows_argument(argument))
                .collect::<Vec<_>>()
                .join(" "),
        };

        let invocation = verify_legacy_windows_scm_definition(&definition, executable).unwrap();
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
        assert!(invocation.matches_install(&options));

        let mut foreign_account = definition.clone();
        foreign_account.account_name = Some("DOMAIN\\operator".to_string());
        assert!(verify_legacy_windows_scm_definition(&foreign_account, executable).is_err());

        let mut changed_startup = definition.clone();
        changed_startup.start_type = 3;
        assert!(verify_legacy_windows_scm_definition(&changed_startup, executable).is_err());

        let mut changed_dependencies = definition.clone();
        changed_dependencies
            .dependencies
            .push("foreign".to_string());
        assert!(verify_legacy_windows_scm_definition(&changed_dependencies, executable).is_err());

        let mut foreign_command = definition;
        foreign_command.command_line = quote_windows_argument("C:/Windows/System32/cmd.exe");
        assert!(verify_legacy_windows_scm_definition(&foreign_command, executable).is_err());
    }

    #[test]
    fn windows_command_line_parser_rejects_noncanonical_or_unterminated_input() {
        assert_eq!(
            parse_canonical_windows_command_line(
                r#""C:/Program Files/codex-helper.exe" service run"#
            )
            .unwrap(),
            ["C:/Program Files/codex-helper.exe", "service", "run"]
        );
        assert!(parse_canonical_windows_command_line("service  run").is_err());
        assert!(parse_canonical_windows_command_line(r#""unterminated"#).is_err());
    }

    #[test]
    fn windows_service_probe_distinguishes_missing_and_pending_deletion() {
        assert_eq!(
            classify_windows_service_probe_error(Some(1060)),
            WindowsServiceProbeClassification::Missing
        );
        assert_eq!(
            classify_windows_service_probe_error(Some(1072)),
            WindowsServiceProbeClassification::MarkedForDelete
        );
        for raw_os_error in [Some(5), None] {
            assert_eq!(
                classify_windows_service_probe_error(raw_os_error),
                WindowsServiceProbeClassification::Error
            );
        }
    }

    struct FakeServiceUninstallBackend {
        events: Vec<&'static str>,
        fail_at: Option<&'static str>,
        rollback_fails: bool,
        runtime_running: bool,
        enabled: bool,
        definition: Option<&'static [u8]>,
        receipt: Option<&'static [u8]>,
    }

    impl Default for FakeServiceUninstallBackend {
        fn default() -> Self {
            Self {
                events: Vec::new(),
                fail_at: None,
                rollback_fails: false,
                runtime_running: true,
                enabled: true,
                definition: Some(b"original-definition"),
                receipt: Some(b"original-receipt"),
            }
        }
    }

    impl FakeServiceUninstallBackend {
        fn step(&mut self, name: &'static str) -> CliResult<()> {
            self.events.push(name);
            if self.fail_at == Some(name) || (name == "rollback" && self.rollback_fails) {
                Err(CliError::Other(format!("injected {name} failure")))
            } else {
                Ok(())
            }
        }
    }

    impl ServiceUninstallTransactionBackend for FakeServiceUninstallBackend {
        fn stop_and_verify(&mut self) -> CliResult<()> {
            self.step("stop")?;
            self.runtime_running = false;
            Ok(())
        }

        fn disable_and_verify(&mut self) -> CliResult<()> {
            self.step("disable")?;
            self.enabled = false;
            Ok(())
        }

        fn remove_definition(&mut self) -> CliResult<()> {
            self.step("remove_definition")?;
            self.definition = None;
            Ok(())
        }

        fn remove_receipt(&mut self) -> CliResult<()> {
            self.step("remove_receipt")?;
            self.receipt = None;
            Ok(())
        }

        fn rollback(&mut self) -> CliResult<()> {
            self.step("rollback")?;
            self.runtime_running = true;
            self.enabled = true;
            self.definition = Some(b"original-definition");
            self.receipt = Some(b"original-receipt");
            Ok(())
        }
    }

    #[test]
    fn service_uninstall_stops_and_disables_before_removing_owned_files() {
        let mut backend = FakeServiceUninstallBackend::default();

        run_service_uninstall_transaction(&mut backend, true).unwrap();

        assert_eq!(
            backend.events,
            ["stop", "disable", "remove_definition", "remove_receipt"]
        );
        assert!(!backend.runtime_running);
        assert!(!backend.enabled);
        assert!(backend.definition.is_none());
        assert!(backend.receipt.is_none());
    }

    #[test]
    fn service_uninstall_rolls_back_every_failure_before_completion() {
        for (failure, expected) in [
            ("stop", vec!["stop", "rollback"]),
            ("disable", vec!["stop", "disable", "rollback"]),
            (
                "remove_definition",
                vec!["stop", "disable", "remove_definition", "rollback"],
            ),
            (
                "remove_receipt",
                vec![
                    "stop",
                    "disable",
                    "remove_definition",
                    "remove_receipt",
                    "rollback",
                ],
            ),
        ] {
            let mut backend = FakeServiceUninstallBackend {
                fail_at: Some(failure),
                ..FakeServiceUninstallBackend::default()
            };

            let error = run_service_uninstall_transaction(&mut backend, true).unwrap_err();

            assert_eq!(backend.events, expected, "{failure}");
            assert!(backend.runtime_running, "{failure}");
            assert!(backend.enabled, "{failure}");
            assert_eq!(
                backend.definition,
                Some(b"original-definition".as_slice()),
                "{failure}"
            );
            assert_eq!(
                backend.receipt,
                Some(b"original-receipt".as_slice()),
                "{failure}"
            );
            assert!(error.to_string().contains("were restored"), "{error}");
        }
    }

    #[test]
    fn service_uninstall_keep_running_skips_stop_but_removes_registration() {
        let mut backend = FakeServiceUninstallBackend::default();

        run_service_uninstall_transaction(&mut backend, false).unwrap();

        assert_eq!(
            backend.events,
            ["disable", "remove_definition", "remove_receipt"]
        );
        assert!(backend.runtime_running);
        assert!(!backend.enabled);
        assert!(backend.definition.is_none());
        assert!(backend.receipt.is_none());
    }

    #[test]
    fn service_uninstall_reports_rollback_failure_without_claiming_restoration() {
        let mut backend = FakeServiceUninstallBackend {
            fail_at: Some("remove_receipt"),
            rollback_fails: true,
            ..FakeServiceUninstallBackend::default()
        };

        let error = run_service_uninstall_transaction(&mut backend, true).unwrap_err();

        assert!(error.to_string().contains("restoring"));
        assert!(error.to_string().contains("also failed"));
    }

    struct FakeWindowsUninstallBackend {
        events: Vec<&'static str>,
        fail_at: Option<&'static str>,
        rollback_fails: bool,
        legacy_commit_unknown: bool,
        scoped_registered: bool,
        fixed_registered: bool,
        legacy_registered: bool,
        scoped_running: bool,
        fixed_running: bool,
        legacy_running: bool,
        definition_exists: bool,
        receipt_exists: bool,
    }

    impl Default for FakeWindowsUninstallBackend {
        fn default() -> Self {
            Self {
                events: Vec::new(),
                fail_at: None,
                rollback_fails: false,
                legacy_commit_unknown: false,
                scoped_registered: true,
                fixed_registered: true,
                legacy_registered: true,
                scoped_running: true,
                fixed_running: false,
                legacy_running: true,
                definition_exists: true,
                receipt_exists: true,
            }
        }
    }

    impl FakeWindowsUninstallBackend {
        fn finish_step(&self, name: &'static str) -> CliResult<()> {
            if self.fail_at == Some(name) {
                Err(CliError::Other(format!("injected {name} failure")))
            } else {
                Ok(())
            }
        }

        fn assert_original_state(&self, failure: &str) {
            assert!(self.scoped_registered, "{failure}");
            assert!(self.fixed_registered, "{failure}");
            assert!(self.legacy_registered, "{failure}");
            assert!(self.scoped_running, "{failure}");
            assert!(!self.fixed_running, "{failure}");
            assert!(self.legacy_running, "{failure}");
            assert!(self.definition_exists, "{failure}");
            assert!(self.receipt_exists, "{failure}");
        }
    }

    impl WindowsUninstallTransactionBackend for FakeWindowsUninstallBackend {
        fn stop_and_verify(&mut self) -> CliResult<()> {
            self.events.push("stop");
            self.scoped_running = false;
            self.fixed_running = false;
            self.legacy_running = false;
            self.finish_step("stop")
        }

        fn remove_scoped_task(&mut self) -> CliResult<()> {
            self.events.push("remove_scoped");
            self.scoped_registered = false;
            self.finish_step("remove_scoped")
        }

        fn remove_fixed_task(&mut self) -> CliResult<()> {
            self.events.push("remove_fixed");
            self.fixed_registered = false;
            self.finish_step("remove_fixed")
        }

        fn remove_definition(&mut self) -> CliResult<()> {
            self.events.push("remove_definition");
            self.definition_exists = false;
            self.finish_step("remove_definition")
        }

        fn remove_receipt(&mut self) -> CliResult<()> {
            self.events.push("remove_receipt");
            self.receipt_exists = false;
            self.finish_step("remove_receipt")
        }

        fn retire_legacy_scm(&mut self) -> CliResult<()> {
            self.events.push("retire_legacy");
            if self.fail_at == Some("retire_legacy") {
                if self.legacy_commit_unknown {
                    self.legacy_registered = false;
                }
                return self.finish_step("retire_legacy");
            }
            self.legacy_registered = false;
            Ok(())
        }

        fn rollback(&mut self) -> CliResult<()> {
            self.events.push("rollback");
            if self.rollback_fails {
                return Err(CliError::Other(
                    "restore SID-scoped task failed; restore service receipt failed".to_string(),
                ));
            }
            let legacy_commit_unknown = self.legacy_commit_unknown && !self.legacy_registered;
            self.scoped_registered = true;
            self.fixed_registered = true;
            self.scoped_running = true;
            self.fixed_running = false;
            self.definition_exists = true;
            self.receipt_exists = true;
            if legacy_commit_unknown {
                return Err(CliError::Other(
                    "the legacy SCM definition cannot be reconstructed automatically".to_string(),
                ));
            }
            self.legacy_registered = true;
            self.legacy_running = true;
            Ok(())
        }
    }

    #[test]
    fn windows_uninstall_commits_legacy_scm_only_after_reversible_resources() {
        let mut backend = FakeWindowsUninstallBackend::default();

        run_windows_uninstall_transaction(&mut backend, true).unwrap();

        assert_eq!(
            backend.events,
            [
                "stop",
                "remove_scoped",
                "remove_fixed",
                "remove_definition",
                "remove_receipt",
                "retire_legacy",
            ]
        );
        assert!(!backend.scoped_registered);
        assert!(!backend.fixed_registered);
        assert!(!backend.legacy_registered);
        assert!(!backend.scoped_running);
        assert!(!backend.legacy_running);
        assert!(!backend.definition_exists);
        assert!(!backend.receipt_exists);
    }

    #[test]
    fn windows_uninstall_rolls_back_every_partial_failure() {
        for (failure, expected) in [
            ("stop", vec!["stop", "rollback"]),
            ("remove_scoped", vec!["stop", "remove_scoped", "rollback"]),
            (
                "remove_fixed",
                vec!["stop", "remove_scoped", "remove_fixed", "rollback"],
            ),
            (
                "remove_definition",
                vec![
                    "stop",
                    "remove_scoped",
                    "remove_fixed",
                    "remove_definition",
                    "rollback",
                ],
            ),
            (
                "remove_receipt",
                vec![
                    "stop",
                    "remove_scoped",
                    "remove_fixed",
                    "remove_definition",
                    "remove_receipt",
                    "rollback",
                ],
            ),
            (
                "retire_legacy",
                vec![
                    "stop",
                    "remove_scoped",
                    "remove_fixed",
                    "remove_definition",
                    "remove_receipt",
                    "retire_legacy",
                    "rollback",
                ],
            ),
        ] {
            let mut backend = FakeWindowsUninstallBackend {
                fail_at: Some(failure),
                ..FakeWindowsUninstallBackend::default()
            };

            let error = run_windows_uninstall_transaction(&mut backend, true).unwrap_err();

            assert_eq!(backend.events, expected, "{failure}");
            backend.assert_original_state(failure);
            assert!(error.to_string().contains("were restored"), "{error}");
            assert!(error.to_string().contains(failure), "{error}");
        }
    }

    #[test]
    fn windows_uninstall_keep_running_detaches_each_runtime_without_stopping_it() {
        let mut backend = FakeWindowsUninstallBackend::default();

        run_windows_uninstall_transaction(&mut backend, false).unwrap();

        assert_eq!(
            backend.events,
            [
                "remove_scoped",
                "remove_fixed",
                "remove_definition",
                "remove_receipt",
                "retire_legacy",
            ]
        );
        assert!(backend.scoped_running);
        assert!(!backend.fixed_running);
        assert!(backend.legacy_running);
        assert!(!backend.scoped_registered);
        assert!(!backend.fixed_registered);
        assert!(!backend.legacy_registered);
        assert!(!backend.definition_exists);
        assert!(!backend.receipt_exists);
    }

    #[test]
    fn windows_uninstall_keep_running_failure_restores_registration_without_stopping_runtime() {
        let mut backend = FakeWindowsUninstallBackend {
            fail_at: Some("remove_receipt"),
            ..FakeWindowsUninstallBackend::default()
        };

        let error = run_windows_uninstall_transaction(&mut backend, false).unwrap_err();

        assert_eq!(
            backend.events,
            [
                "remove_scoped",
                "remove_fixed",
                "remove_definition",
                "remove_receipt",
                "rollback",
            ]
        );
        backend.assert_original_state("keep_running rollback");
        assert!(error.to_string().contains("were restored"), "{error}");
    }

    #[test]
    fn windows_uninstall_rollback_failure_reports_each_recovery_diagnostic() {
        let mut backend = FakeWindowsUninstallBackend {
            fail_at: Some("remove_receipt"),
            rollback_fails: true,
            ..FakeWindowsUninstallBackend::default()
        };

        let error = run_windows_uninstall_transaction(&mut backend, true).unwrap_err();
        let message = error.to_string();

        assert!(
            message.contains("injected remove_receipt failure"),
            "{message}"
        );
        assert!(message.contains("also failed"), "{message}");
        assert!(
            message.contains("restore SID-scoped task failed"),
            "{message}"
        );
        assert!(
            message.contains("restore service receipt failed"),
            "{message}"
        );
    }

    #[test]
    fn windows_uninstall_unknown_legacy_delete_is_reported_as_partial_installation() {
        let mut backend = FakeWindowsUninstallBackend {
            fail_at: Some("retire_legacy"),
            legacy_commit_unknown: true,
            ..FakeWindowsUninstallBackend::default()
        };

        let error = run_windows_uninstall_transaction(&mut backend, true).unwrap_err();
        let message = error.to_string();

        assert!(backend.scoped_registered);
        assert!(backend.fixed_registered);
        assert!(backend.definition_exists);
        assert!(backend.receipt_exists);
        assert!(!backend.legacy_registered);
        assert!(message.contains("cannot be reconstructed"), "{message}");
        assert!(message.contains("installation is partial"), "{message}");
        assert!(message.contains("service install"), "{message}");
        assert!(!message.contains("were restored"), "{message}");
    }

    #[derive(Default)]
    struct FakeUnixInstallBackend {
        events: Vec<&'static str>,
        fail_at: Option<&'static str>,
        rollback_fails: bool,
        readiness: Option<codex_helper_core::credentials::CredentialAggregateReadiness>,
    }

    impl FakeUnixInstallBackend {
        fn step(&mut self, name: &'static str) -> CliResult<()> {
            self.events.push(name);
            if self.fail_at == Some(name) || (name == "rollback" && self.rollback_fails) {
                Err(CliError::Other(format!("injected {name} failure")))
            } else {
                Ok(())
            }
        }
    }

    impl UnixInstallTransactionBackend for FakeUnixInstallBackend {
        fn prepare_replacement(&mut self) -> CliResult<()> {
            self.step("prepare")
        }

        fn start_replacement(&mut self) -> CliResult<()> {
            self.step("start")
        }

        async fn verify_started_runtime_identity(
            &mut self,
        ) -> CliResult<codex_helper_core::credentials::CredentialAggregateReadiness> {
            self.step("verify_identity")?;
            Ok(self
                .readiness
                .unwrap_or(codex_helper_core::credentials::CredentialAggregateReadiness::Ready))
        }

        fn rollback(&mut self) -> CliResult<()> {
            self.step("rollback")
        }
    }

    #[tokio::test]
    async fn unix_install_rolls_back_start_and_runtime_identity_failures() {
        for failure in ["start", "verify_identity"] {
            let mut backend = FakeUnixInstallBackend {
                fail_at: Some(failure),
                ..FakeUnixInstallBackend::default()
            };

            let error = run_unix_install_transaction(&mut backend, true)
                .await
                .unwrap_err();

            assert_eq!(backend.events.last(), Some(&"rollback"), "{failure}");
            assert!(error.to_string().contains("restored"), "{error}");
        }
    }

    #[tokio::test]
    async fn unix_install_commits_every_signed_runtime_readiness_state() {
        use codex_helper_core::credentials::CredentialAggregateReadiness;

        for readiness in [
            CredentialAggregateReadiness::Ready,
            CredentialAggregateReadiness::Degraded,
            CredentialAggregateReadiness::Blocked,
        ] {
            let mut backend = FakeUnixInstallBackend {
                readiness: Some(readiness),
                ..FakeUnixInstallBackend::default()
            };

            let actual = run_unix_install_transaction(&mut backend, true)
                .await
                .unwrap();

            assert_eq!(actual, Some(readiness));
            assert_eq!(backend.events, ["prepare", "start", "verify_identity"]);
        }
    }

    #[tokio::test]
    async fn unix_install_without_start_commits_without_runtime_probe() {
        let mut backend = FakeUnixInstallBackend::default();

        let readiness = run_unix_install_transaction(&mut backend, false)
            .await
            .unwrap();

        assert_eq!(readiness, None);
        assert_eq!(backend.events, ["prepare"]);
    }

    #[derive(Default)]
    struct FakeWindowsInstallBackend {
        events: Vec<&'static str>,
        fail_at: Option<&'static str>,
        rollback_fails: bool,
        rollback_attempted: bool,
        preserve_before_rollback: bool,
        readiness: Option<codex_helper_core::credentials::CredentialAggregateReadiness>,
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

        fn stop_existing_scoped_task(&mut self) -> CliResult<()> {
            self.step("stop")
        }

        fn stop_legacy_runtimes(&mut self) -> CliResult<()> {
            self.step("stop_legacy")
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
            self.rollback_attempted = true;
            self.step("rollback")
        }

        fn rollback_preserved_replacement(&self) -> bool {
            self.preserve_before_rollback || (self.rollback_attempted && self.rollback_fails)
        }

        fn start_scoped_task(&mut self) -> CliResult<()> {
            self.step("start")
        }

        async fn verify_started_runtime_identity(
            &mut self,
        ) -> CliResult<codex_helper_core::credentials::CredentialAggregateReadiness> {
            self.step("verify_runtime")?;
            Ok(self
                .readiness
                .unwrap_or(codex_helper_core::credentials::CredentialAggregateReadiness::Ready))
        }
    }

    #[tokio::test]
    async fn windows_migration_verifies_runtime_before_retiring_legacy_installations() {
        let mut backend = FakeWindowsInstallBackend::default();

        run_windows_install_transaction(&mut backend, true)
            .await
            .unwrap();

        assert_eq!(
            backend.events,
            [
                "preflight",
                "stop",
                "stop_legacy",
                "register",
                "verify",
                "publish_receipt",
                "start",
                "verify_runtime",
                "retire_fixed",
                "retire_scm",
            ]
        );
    }

    #[tokio::test]
    async fn windows_migration_commits_a_runtime_with_blocked_credentials() {
        use codex_helper_core::credentials::CredentialAggregateReadiness;

        let mut backend = FakeWindowsInstallBackend {
            readiness: Some(CredentialAggregateReadiness::Blocked),
            ..FakeWindowsInstallBackend::default()
        };

        let readiness = run_windows_install_transaction(&mut backend, true)
            .await
            .unwrap();

        assert_eq!(readiness, Some(CredentialAggregateReadiness::Blocked));
        assert_eq!(backend.events.last(), Some(&"retire_scm"));
        assert!(!backend.events.contains(&"rollback"));
        assert!(!backend.events.contains(&"rollback_receipt"));
    }

    #[tokio::test]
    async fn windows_migration_preserves_replacement_when_scm_delete_commit_is_unknown() {
        let mut backend = FakeWindowsInstallBackend {
            fail_at: Some("retire_scm"),
            preserve_before_rollback: true,
            ..FakeWindowsInstallBackend::default()
        };

        let error = run_windows_install_transaction(&mut backend, true)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("preserved"), "{error}");
        assert_eq!(backend.events.last(), Some(&"retire_scm"));
        assert!(!backend.events.contains(&"rollback"));
        assert!(!backend.events.contains(&"rollback_receipt"));
    }

    #[tokio::test]
    async fn windows_migration_rolls_back_every_failure_after_preflight() {
        for (failure, expected) in [
            (
                "stop",
                vec!["preflight", "stop", "rollback", "rollback_receipt"],
            ),
            (
                "stop_legacy",
                vec![
                    "preflight",
                    "stop",
                    "stop_legacy",
                    "rollback",
                    "rollback_receipt",
                ],
            ),
            (
                "register",
                vec![
                    "preflight",
                    "stop",
                    "stop_legacy",
                    "register",
                    "rollback",
                    "rollback_receipt",
                ],
            ),
            (
                "verify",
                vec![
                    "preflight",
                    "stop",
                    "stop_legacy",
                    "register",
                    "verify",
                    "rollback",
                    "rollback_receipt",
                ],
            ),
            (
                "publish_receipt",
                vec![
                    "preflight",
                    "stop",
                    "stop_legacy",
                    "register",
                    "verify",
                    "publish_receipt",
                    "rollback",
                    "rollback_receipt",
                ],
            ),
            (
                "start",
                vec![
                    "preflight",
                    "stop",
                    "stop_legacy",
                    "register",
                    "verify",
                    "publish_receipt",
                    "start",
                    "rollback",
                    "rollback_receipt",
                ],
            ),
            (
                "verify_runtime",
                vec![
                    "preflight",
                    "stop",
                    "stop_legacy",
                    "register",
                    "verify",
                    "publish_receipt",
                    "start",
                    "verify_runtime",
                    "rollback",
                    "rollback_receipt",
                ],
            ),
            (
                "retire_fixed",
                vec![
                    "preflight",
                    "stop",
                    "stop_legacy",
                    "register",
                    "verify",
                    "publish_receipt",
                    "start",
                    "verify_runtime",
                    "retire_fixed",
                    "rollback",
                    "rollback_receipt",
                ],
            ),
            (
                "retire_scm",
                vec![
                    "preflight",
                    "stop",
                    "stop_legacy",
                    "register",
                    "verify",
                    "publish_receipt",
                    "start",
                    "verify_runtime",
                    "retire_fixed",
                    "retire_scm",
                    "rollback",
                    "rollback_receipt",
                ],
            ),
        ] {
            let mut backend = FakeWindowsInstallBackend {
                fail_at: Some(failure),
                ..FakeWindowsInstallBackend::default()
            };

            let error = run_windows_install_transaction(&mut backend, true)
                .await
                .unwrap_err();

            assert_eq!(backend.events, expected, "{failure}");
            assert!(error.to_string().contains("restored"), "{error}");
        }
    }

    #[tokio::test]
    async fn windows_migration_preflight_failure_never_registers_or_retires() {
        let mut backend = FakeWindowsInstallBackend {
            fail_at: Some("preflight"),
            ..FakeWindowsInstallBackend::default()
        };

        assert!(
            run_windows_install_transaction(&mut backend, true)
                .await
                .is_err()
        );
        assert_eq!(backend.events, ["preflight"]);
    }

    #[tokio::test]
    async fn windows_migration_reports_rollback_failure_and_keeps_verified_fallback() {
        let mut backend = FakeWindowsInstallBackend {
            fail_at: Some("retire_scm"),
            rollback_fails: true,
            ..FakeWindowsInstallBackend::default()
        };

        let error = run_windows_install_transaction(&mut backend, true)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("rollback also failed"));
        assert!(error.to_string().contains("left installed"));
        assert_eq!(backend.events.last(), Some(&"rollback"));
    }

    #[tokio::test]
    async fn windows_migration_stops_before_replacing_and_no_start_leaves_task_stopped() {
        let mut backend = FakeWindowsInstallBackend::default();

        run_windows_install_transaction(&mut backend, false)
            .await
            .unwrap();

        assert_eq!(
            backend.events,
            [
                "preflight",
                "stop",
                "stop_legacy",
                "register",
                "verify",
                "publish_receipt",
                "retire_fixed",
                "retire_scm",
            ]
        );
        assert!(!backend.events.contains(&"start"));
    }

    #[tokio::test]
    async fn windows_migration_start_failure_restores_the_previous_installation() {
        let mut backend = FakeWindowsInstallBackend {
            fail_at: Some("start"),
            ..FakeWindowsInstallBackend::default()
        };

        let error = run_windows_install_transaction(&mut backend, true)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("restored"));
        assert_eq!(
            backend.events,
            [
                "preflight",
                "stop",
                "stop_legacy",
                "register",
                "verify",
                "publish_receipt",
                "start",
                "rollback",
                "rollback_receipt",
            ]
        );
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

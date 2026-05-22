param(
    [string]$InstallerPath = "target/release/bundle/nsis/codex-helper_0.16.0_x64-setup.exe",
    [string]$InstallDir = (Join-Path $env:TEMP "codex-helper-tdrp-080-install"),
    [string]$AdminUrl = "http://127.0.0.1:6211",
    [int]$DevToolsPort = 0,
    [switch]$SkipDevToolsSmoke,
    [switch]$KeepInstall
)

$ErrorActionPreference = "Stop"

function New-SmokeResult {
    param(
        [string]$Name,
        [bool]$Passed,
        [string]$Detail
    )
    [pscustomobject]@{
        name = $Name
        passed = $Passed
        detail = $Detail
    }
}

function Get-FreeTcpPort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    try {
        $listener.Start()
        return $listener.LocalEndpoint.Port
    } finally {
        $listener.Stop()
    }
}

function Wait-DevToolsWebSocketUrl {
    param(
        [int]$Port,
        [int]$TimeoutSeconds = 12
    )
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        try {
            $pages = Invoke-RestMethod -UseBasicParsing -Uri "http://127.0.0.1:$Port/json" -TimeoutSec 2
            $page = @($pages) |
                Where-Object { $_.type -eq "page" -and $_.webSocketDebuggerUrl } |
                Select-Object -First 1
            if ($page) {
                return [string]$page.webSocketDebuggerUrl
            }
        } catch {
            Start-Sleep -Milliseconds 250
        }
    } while ((Get-Date) -lt $deadline)
    throw "WebView2 DevTools endpoint did not expose a page websocket on port $Port."
}

function Invoke-CdpExpression {
    param(
        [string]$WebSocketUrl,
        [string]$Expression,
        [int]$TimeoutSeconds = 15
    )

    $socket = [System.Net.WebSockets.ClientWebSocket]::new()
    $cts = [System.Threading.CancellationTokenSource]::new([TimeSpan]::FromSeconds($TimeoutSeconds))
    try {
        [void]$socket.ConnectAsync([Uri]$WebSocketUrl, $cts.Token).GetAwaiter().GetResult()
        $request = @{
            id = 1
            method = "Runtime.evaluate"
            params = @{
                expression = $Expression
                awaitPromise = $true
                returnByValue = $true
                userGesture = $true
            }
        } | ConvertTo-Json -Depth 10 -Compress
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($request)
        [void]$socket.SendAsync(
            [ArraySegment[byte]]::new($bytes),
            [System.Net.WebSockets.WebSocketMessageType]::Text,
            $true,
            $cts.Token
        ).GetAwaiter().GetResult()

        $buffer = New-Object byte[] 65536
        $message = [System.Text.StringBuilder]::new()
        while ($true) {
            $segment = [ArraySegment[byte]]::new($buffer)
            $received = $socket.ReceiveAsync($segment, $cts.Token).GetAwaiter().GetResult()
            if ($received.MessageType -eq [System.Net.WebSockets.WebSocketMessageType]::Close) {
                throw "DevTools websocket closed before Runtime.evaluate returned."
            }
            if ($received.Count -gt 0) {
                [void]$message.Append([System.Text.Encoding]::UTF8.GetString($buffer, 0, $received.Count))
            }
            if (-not $received.EndOfMessage) {
                continue
            }

            $payload = $message.ToString()
            [void]$message.Clear()
            if ([string]::IsNullOrWhiteSpace($payload)) {
                continue
            }
            $response = $payload | ConvertFrom-Json
            if ($response.id -ne 1) {
                continue
            }
            if ($response.exceptionDetails) {
                $detail = $response.exceptionDetails.exception.description
                if (-not $detail) {
                    $detail = $response.exceptionDetails.text
                }
                throw "Runtime.evaluate exception: $detail"
            }
            if ($response.result.result.subtype -eq "error") {
                throw "Runtime.evaluate error: $($response.result.result.description)"
            }
            return $response.result.result.value
        }
    } finally {
        try {
            if ($socket.State -eq [System.Net.WebSockets.WebSocketState]::Open) {
                [void]$socket.CloseAsync(
                    [System.Net.WebSockets.WebSocketCloseStatus]::NormalClosure,
                    "done",
                    [System.Threading.CancellationToken]::None
                ).GetAwaiter().GetResult()
            }
        } catch {}
        $socket.Dispose()
        $cts.Dispose()
    }
}

function Invoke-TauriCommandViaCdp {
    param(
        [string]$WebSocketUrl,
        [string]$Command,
        [object]$CommandArgs = @{}
    )
    $commandJson = $Command | ConvertTo-Json -Compress
    if ($null -eq $CommandArgs) {
        $CommandArgs = @{}
    }
    if ($CommandArgs -is [System.Collections.IDictionary]) {
        $CommandArgs = [pscustomobject]$CommandArgs
    }
    $argsJson = $CommandArgs | ConvertTo-Json -Depth 10 -Compress
    $expression = "window.__codexSmokeInvoke($commandJson, $argsJson)"
    $raw = Invoke-CdpExpression -WebSocketUrl $WebSocketUrl -Expression $expression
    $envelope = $raw | ConvertFrom-Json
    if (-not $envelope.ok) {
        throw "Tauri command $Command failed: $($envelope.error); expression=$expression"
    }
    return $envelope.value
}

function Initialize-CdpSmokeBridge {
    param([string]$WebSocketUrl)
    $bridge = @"
(() => {
  window.__codexSmokeInvoke = async (command, args) => {
    const invoke = window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke;
    if (!invoke) {
      return JSON.stringify({ ok: false, error: "Tauri internals unavailable in packaged WebView" });
    }
    try {
      const value = await invoke(command, args || {});
      return JSON.stringify({ ok: true, value });
    } catch (error) {
      const message = error && (error.message || ((error.toString) ? error.toString() : String(error)));
      return JSON.stringify({ ok: false, error: message });
    }
  };
  return "ready";
})()
"@
    Invoke-CdpExpression -WebSocketUrl $WebSocketUrl -Expression $bridge | Out-Null
}

if (-not ("CodexHelperSmoke.NativeWindow" -as [type])) {
    Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;

namespace CodexHelperSmoke {
    public static class NativeWindow {
        public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

        [DllImport("user32.dll")]
        public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);

        [DllImport("user32.dll")]
        public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);

        [DllImport("user32.dll")]
        public static extern bool IsWindowVisible(IntPtr hWnd);

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        public static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int count);

        [DllImport("user32.dll")]
        public static extern bool PostMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);

        [DllImport("user32.dll")]
        public static extern bool SetForegroundWindow(IntPtr hWnd);

        [DllImport("user32.dll")]
        public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    }
}
'@
}

function Get-MainWindowHandle {
    param([int]$ProcessId)
    $found = [IntPtr]::Zero
    [CodexHelperSmoke.NativeWindow]::EnumWindows({
        param([IntPtr]$hWnd, [IntPtr]$lParam)
        [uint32]$windowProcessId = 0
        [void][CodexHelperSmoke.NativeWindow]::GetWindowThreadProcessId($hWnd, [ref]$windowProcessId)
        if ($windowProcessId -ne [uint32]$ProcessId) {
            return $true
        }
        $buffer = New-Object System.Text.StringBuilder 256
        [void][CodexHelperSmoke.NativeWindow]::GetWindowText($hWnd, $buffer, $buffer.Capacity)
        if ($buffer.ToString() -eq "codex-helper") {
            $script:__smoke_found_hwnd = $hWnd
            return $false
        }
        return $true
    }, [IntPtr]::Zero) | Out-Null
    if ($script:__smoke_found_hwnd) {
        $found = $script:__smoke_found_hwnd
        Remove-Variable -Scope Script -Name __smoke_found_hwnd -ErrorAction SilentlyContinue
    }
    return $found
}

function Test-WindowVisible {
    param([IntPtr]$Handle)
    if ($Handle -eq [IntPtr]::Zero) {
        return $false
    }
    return [CodexHelperSmoke.NativeWindow]::IsWindowVisible($Handle)
}

function Test-AdminReachable {
    param([string]$BaseUrl)
    try {
        $response = Invoke-WebRequest `
            -UseBasicParsing `
            -Uri ($BaseUrl.TrimEnd("/") + "/__codex_helper/api/v1/operator/summary") `
            -TimeoutSec 1
        return $response.StatusCode -eq 200
    } catch {
        return $false
    }
}

function Invoke-HttpShutdown {
    param([string]$BaseUrl)
    try {
        Invoke-WebRequest `
            -UseBasicParsing `
            -Method Post `
            -Uri ($BaseUrl.TrimEnd("/") + "/__codex_helper/api/v1/runtime/shutdown") `
            -TimeoutSec 2 | Out-Null
    } catch {
        # The runtime may already be gone; cleanup best-effort only.
    }
}

function Write-SmokeConfig {
    param([string]$Path)
    $config = @'
version = 5

[codex.routing]
entry = "relay"

[codex.routing.routes.relay]
strategy = "ordered-failover"
children = ["relay"]

[codex.providers.relay]
alias = "Relay Smoke"
base_url = "https://relay.example/v1"
auth_token_env = "RELAY_API_KEY"
enabled = true
'@
    [System.IO.File]::WriteAllText($Path, $config, [System.Text.UTF8Encoding]::new($false))
}

$installer = Resolve-Path -LiteralPath $InstallerPath
$installPath = [System.IO.Path]::GetFullPath($InstallDir)
$smokeHome = Join-Path $env:TEMP ("codex-helper-tdrp-080-home-" + [guid]::NewGuid().ToString("N"))
$oldWebViewArgs = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
if (-not $SkipDevToolsSmoke) {
    if ($DevToolsPort -le 0) {
        $DevToolsPort = Get-FreeTcpPort
    }
    $remoteDebugArg = "--remote-debugging-port=$DevToolsPort"
    if ([string]::IsNullOrWhiteSpace($oldWebViewArgs)) {
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $remoteDebugArg
    } elseif ($oldWebViewArgs -notmatch "--remote-debugging-port=") {
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "$oldWebViewArgs $remoteDebugArg"
    }
}
$env:CODEX_HELPER_HOME = $smokeHome
$env:CODEX_HELPER_DESKTOP_ADMIN_URL = $AdminUrl
$env:CODEX_HELPER_CLI_PATH = ""
$env:CODEX_HELPER_CLI = ""

$results = [System.Collections.Generic.List[object]]::new()
$desktop = $null
$runtimeStarted = $false

try {
    if (Test-Path -LiteralPath $installPath) {
        Remove-Item -LiteralPath $installPath -Recurse -Force
    }
    New-Item -ItemType Directory -Path $installPath | Out-Null
    New-Item -ItemType Directory -Path $smokeHome | Out-Null
    Write-SmokeConfig -Path (Join-Path $smokeHome "config.toml")

    $installerProcess = Start-Process `
        -FilePath $installer.Path `
        -ArgumentList @("/S", "/D=$installPath") `
        -Wait `
        -PassThru
    $results.Add((New-SmokeResult `
        -Name "nsis-install" `
        -Passed ($installerProcess.ExitCode -eq 0) `
        -Detail "exit=$($installerProcess.ExitCode); install=$installPath"))

    $desktopExe = Join-Path $installPath "codex-helper-desktop.exe"
    $sidecarExe = Join-Path $installPath "codex-helper.exe"
    $results.Add((New-SmokeResult `
        -Name "packaged-files" `
        -Passed ((Test-Path -LiteralPath $desktopExe) -and (Test-Path -LiteralPath $sidecarExe)) `
        -Detail "desktop=$(Test-Path -LiteralPath $desktopExe); sidecar=$(Test-Path -LiteralPath $sidecarExe)"))

    $desktop = Start-Process -FilePath $desktopExe -PassThru
    Start-Sleep -Seconds 6
    $windowHandle = Get-MainWindowHandle -ProcessId $desktop.Id
    $results.Add((New-SmokeResult `
        -Name "packaged-window-start" `
        -Passed ($windowHandle -ne [IntPtr]::Zero -and (Test-WindowVisible -Handle $windowHandle)) `
        -Detail "pid=$($desktop.Id); hwnd=$windowHandle; visible=$(Test-WindowVisible -Handle $windowHandle)"))

    if ($windowHandle -ne [IntPtr]::Zero) {
        [void][CodexHelperSmoke.NativeWindow]::PostMessage($windowHandle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
        Start-Sleep -Seconds 2
        $aliveAfterClose = $desktop.HasExited -eq $false
        $windowAfterClose = Get-MainWindowHandle -ProcessId $desktop.Id
        $visibleAfterClose = Test-WindowVisible -Handle $windowAfterClose
        $results.Add((New-SmokeResult `
            -Name "close-hides-to-tray" `
            -Passed ($aliveAfterClose -and -not $visibleAfterClose) `
            -Detail "alive=$aliveAfterClose; hwnd_after_close=$windowAfterClose; visible_after_close=$visibleAfterClose"))
    }

    $secondLaunch = Start-Process -FilePath $desktopExe -PassThru
    Start-Sleep -Seconds 3
    $secondExited = $secondLaunch.HasExited
    $desktopAliveAfterSecondLaunch = $desktop.HasExited -eq $false
    $windowAfterSecondLaunch = Get-MainWindowHandle -ProcessId $desktop.Id
    $visibleAfterSecondLaunch = Test-WindowVisible -Handle $windowAfterSecondLaunch
    $results.Add((New-SmokeResult `
        -Name "second-launch-focuses-existing-window" `
        -Passed ($desktopAliveAfterSecondLaunch -and $visibleAfterSecondLaunch) `
        -Detail "second_pid=$($secondLaunch.Id); second_exited=$secondExited; first_alive=$desktopAliveAfterSecondLaunch; hwnd=$windowAfterSecondLaunch; visible=$visibleAfterSecondLaunch"))

    if (-not $SkipDevToolsSmoke) {
        try {
            $webSocketUrl = Wait-DevToolsWebSocketUrl -Port $DevToolsPort
            Initialize-CdpSmokeBridge -WebSocketUrl $webSocketUrl
            $metadata = Invoke-TauriCommandViaCdp -WebSocketUrl $webSocketUrl -Command "get_app_metadata"
            $results.Add((New-SmokeResult `
                -Name "devtools-tauri-command-bridge" `
                -Passed ($metadata.name -eq "codex-helper" -and $metadata.tauri -eq "2") `
                -Detail "port=$DevToolsPort; app=$($metadata.name); version=$($metadata.version); tauri=$($metadata.tauri)"))

            $knownPaths = Invoke-TauriCommandViaCdp -WebSocketUrl $webSocketUrl -Command "get_known_paths"
            $results.Add((New-SmokeResult `
                -Name "packaged-known-paths-command" `
                -Passed ([string]$knownPaths.home -eq $smokeHome -and [string]$knownPaths.config -like "*config.toml") `
                -Detail "home=$($knownPaths.home); config=$($knownPaths.config); logs=$($knownPaths.logs); cache=$($knownPaths.cache)"))

            $exportPath = Join-Path $smokeHome "codex-helper-export.toml"
            $exportResult = Invoke-TauriCommandViaCdp `
                -WebSocketUrl $webSocketUrl `
                -Command "export_config" `
                -CommandArgs @{ payload = @{ destination = $exportPath } }
            $results.Add((New-SmokeResult `
                -Name "packaged-export-config-command" `
                -Passed ($exportResult.ok -and (Test-Path -LiteralPath $exportPath) -and $exportResult.secret_warning) `
                -Detail "destination=$($exportResult.destination); secret_warning=$($exportResult.secret_warning)"))

            $importPath = Join-Path $smokeHome "codex-helper-import.toml"
            Write-SmokeConfig -Path $importPath
            $importResult = Invoke-TauriCommandViaCdp `
                -WebSocketUrl $webSocketUrl `
                -Command "import_config" `
                -CommandArgs @{ payload = @{ source = $importPath } }
            $backupExists = $false
            if ($importResult.backup) {
                $backupExists = Test-Path -LiteralPath ([string]$importResult.backup)
            }
            $results.Add((New-SmokeResult `
                -Name "packaged-import-config-command" `
                -Passed ($importResult.ok -and $backupExists -and $importResult.secret_warning) `
                -Detail "source=$($importResult.source); destination=$($importResult.destination); backup=$($importResult.backup); backup_exists=$backupExists"))

            $startResult = Invoke-TauriCommandViaCdp -WebSocketUrl $webSocketUrl -Command "start_desktop_proxy"
            $runtimeStarted = $true
            $startState = $startResult.state
            $adminReachable = Test-AdminReachable -BaseUrl $AdminUrl
            $results.Add((New-SmokeResult `
                -Name "packaged-starts-managed-sidecar" `
                -Passed ($startResult.ok -and $adminReachable -and $startState.connection_mode -eq "desktop-owned") `
                -Detail "action=$($startResult.action); reachable=$adminReachable; mode=$($startState.connection_mode); admin=$($startState.admin_base_url)"))

            $hideResult = Invoke-TauriCommandViaCdp -WebSocketUrl $webSocketUrl -Command "hide_main_window"
            Start-Sleep -Seconds 1
            $desktopAliveAfterDetach = $desktop.HasExited -eq $false
            $adminAliveAfterDetach = Test-AdminReachable -BaseUrl $AdminUrl
            $hiddenAfterDetach = -not (Test-WindowVisible -Handle (Get-MainWindowHandle -ProcessId $desktop.Id))
            $results.Add((New-SmokeResult `
                -Name "packaged-detach-keeps-sidecar-running" `
                -Passed ($desktopAliveAfterDetach -and $adminAliveAfterDetach -and $hiddenAfterDetach) `
                -Detail "desktop_alive=$desktopAliveAfterDetach; admin_alive=$adminAliveAfterDetach; hidden=$hiddenAfterDetach; command_result=$hideResult"))

            $thirdLaunch = Start-Process -FilePath $desktopExe -PassThru
            Start-Sleep -Seconds 3
            $thirdExited = $thirdLaunch.HasExited
            $visibleAfterThirdLaunch = Test-WindowVisible -Handle (Get-MainWindowHandle -ProcessId $desktop.Id)
            $adminAliveAfterThirdLaunch = Test-AdminReachable -BaseUrl $AdminUrl
            $results.Add((New-SmokeResult `
                -Name "second-launch-restores-detached-window-with-sidecar" `
                -Passed ($thirdExited -and $visibleAfterThirdLaunch -and $adminAliveAfterThirdLaunch) `
                -Detail "third_pid=$($thirdLaunch.Id); third_exited=$thirdExited; visible=$visibleAfterThirdLaunch; admin_alive=$adminAliveAfterThirdLaunch"))

            $stopResult = Invoke-TauriCommandViaCdp `
                -WebSocketUrl $webSocketUrl `
                -Command "stop_proxy" `
                -CommandArgs @{ payload = @{ scope = "owned"; confirmation = "STOP OWNED PROXY" } }
            Start-Sleep -Seconds 2
            $adminStopped = -not (Test-AdminReachable -BaseUrl $AdminUrl)
            if ($adminStopped) {
                $runtimeStarted = $false
            }
            $results.Add((New-SmokeResult `
                -Name "packaged-owned-stop-proxy-command" `
                -Passed ($stopResult.ok -and $adminStopped) `
                -Detail "action=$($stopResult.action); admin_stopped=$adminStopped; mode=$($stopResult.state.connection_mode)"))

            $restartResult = Invoke-TauriCommandViaCdp -WebSocketUrl $webSocketUrl -Command "start_desktop_proxy"
            $runtimeStarted = $true
            Start-Sleep -Seconds 1
            try {
                Invoke-TauriCommandViaCdp -WebSocketUrl $webSocketUrl -Command "quit_app" | Out-Null
            } catch {
                # The command exits the desktop process, so the DevTools websocket can close before
                # the invoke response is delivered. The post-condition below is authoritative.
            }
            Start-Sleep -Seconds 3
            $desktop.Refresh()
            $desktopExitedAfterQuit = $desktop.HasExited
            $adminAliveAfterQuit = Test-AdminReachable -BaseUrl $AdminUrl
            $results.Add((New-SmokeResult `
                -Name "packaged-quit-app-leaves-sidecar-running" `
                -Passed ($restartResult.ok -and $desktopExitedAfterQuit -and $adminAliveAfterQuit) `
                -Detail "desktop_exited=$desktopExitedAfterQuit; admin_alive=$adminAliveAfterQuit; restart_action=$($restartResult.action)"))
        } catch {
            $results.Add((New-SmokeResult `
                -Name "packaged-devtools-command-smoke" `
                -Passed $false `
                -Detail $_.Exception.Message))
        }
    }

    $summary = [pscustomobject]@{
        timestamp = (Get-Date).ToString("o")
        installer = $installer.Path
        install_dir = $installPath
        smoke_home = $smokeHome
        admin_url = $AdminUrl
        devtools_port = if ($SkipDevToolsSmoke) { $null } else { $DevToolsPort }
        results = $results
        passed = -not ($results | Where-Object { -not $_.passed })
    }
    $summary | ConvertTo-Json -Depth 6
    if (-not $summary.passed) {
        exit 1
    }
} finally {
    if ($runtimeStarted -or (Test-AdminReachable -BaseUrl $AdminUrl)) {
        Invoke-HttpShutdown -BaseUrl $AdminUrl
        Start-Sleep -Seconds 1
    }
    if ($null -ne $desktop -and -not $desktop.HasExited) {
        Stop-Process -Id $desktop.Id -Force
        try {
            $desktop.WaitForExit(5000)
        } catch {}
    }
    if (-not $KeepInstall -and (Test-Path -LiteralPath $installPath)) {
        try {
            Remove-Item -LiteralPath $installPath -Recurse -Force
        } catch {
            Write-Warning "could not remove temporary install directory ${installPath}: $($_.Exception.Message)"
        }
    }
    if ($null -eq $oldWebViewArgs) {
        Remove-Item Env:\WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -ErrorAction SilentlyContinue
    } else {
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $oldWebViewArgs
    }
}

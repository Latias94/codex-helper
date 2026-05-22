param(
    [string]$InstallerPath = "target/release/bundle/nsis/codex-helper_0.16.0_x64-setup.exe",
    [string]$InstallDir = (Join-Path $env:TEMP "codex-helper-tdrp-080-install"),
    [string]$AdminUrl = "http://127.0.0.1:6211",
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

$installer = Resolve-Path -LiteralPath $InstallerPath
$installPath = [System.IO.Path]::GetFullPath($InstallDir)
$smokeHome = Join-Path $env:TEMP ("codex-helper-tdrp-080-home-" + [guid]::NewGuid().ToString("N"))
$env:CODEX_HELPER_HOME = $smokeHome
$env:CODEX_HELPER_DESKTOP_ADMIN_URL = $AdminUrl
$env:CODEX_HELPER_CLI_PATH = ""
$env:CODEX_HELPER_CLI = ""

$results = [System.Collections.Generic.List[object]]::new()
$desktop = $null

try {
    if (Test-Path -LiteralPath $installPath) {
        Remove-Item -LiteralPath $installPath -Recurse -Force
    }
    New-Item -ItemType Directory -Path $installPath | Out-Null
    New-Item -ItemType Directory -Path $smokeHome | Out-Null

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

    $summary = [pscustomobject]@{
        timestamp = (Get-Date).ToString("o")
        installer = $installer.Path
        install_dir = $installPath
        smoke_home = $smokeHome
        admin_url = $AdminUrl
        results = $results
        passed = -not ($results | Where-Object { -not $_.passed })
    }
    $summary | ConvertTo-Json -Depth 6
    if (-not $summary.passed) {
        exit 1
    }
} finally {
    if ($adminReady) {
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
}

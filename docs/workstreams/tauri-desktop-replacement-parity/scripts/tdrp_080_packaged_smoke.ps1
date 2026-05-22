param(
    [string]$InstallerPath = "target/release/bundle/nsis/codex-helper_0.16.0_x64-setup.exe",
    [string]$InstallDir = (Join-Path $env:TEMP "codex-helper-tdrp-080-install"),
    [string]$AdminUrl = "",
    [int]$DevToolsPort = 0,
    [switch]$SkipDevToolsSmoke,
    [switch]$RunAutostartSmoke,
    [switch]$KeepInstall
)

$ErrorActionPreference = "Stop"

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes

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

function Test-TcpPortAvailable {
    param([int]$Port)
    try {
        $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, $Port)
        try {
            $listener.Start()
            return $true
        } finally {
            $listener.Stop()
        }
    } catch {
        return $false
    }
}

function Get-FreeProxyAdminPortPair {
    while ($true) {
        $proxyPort = Get-FreeTcpPort
        $adminPort = $proxyPort + 1000
        if ($adminPort -gt 65535) {
            continue
        }
        if (Test-TcpPortAvailable -Port $adminPort) {
            return [pscustomobject]@{
                proxy = $proxyPort
                admin = $adminPort
            }
        }
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

function Invoke-CdpSmokeJson {
    param(
        [string]$WebSocketUrl,
        [string]$Expression,
        [int]$TimeoutSeconds = 20
    )
    $raw = Invoke-CdpExpression -WebSocketUrl $WebSocketUrl -Expression $Expression -TimeoutSeconds $TimeoutSeconds
    if ($null -eq $raw) {
        return $null
    }
    return $raw | ConvertFrom-Json
}

function Invoke-CdpSmokeDomAction {
    param(
        [string]$WebSocketUrl,
        [string]$Script,
        [int]$TimeoutSeconds = 75
    )
    $expression = @"
(async () => {
  try {
    const value = await (async () => {
$Script
    })();
    return JSON.stringify({ ok: true, value });
  } catch (error) {
    const message = error && (error.message || ((error.toString) ? error.toString() : String(error)));
    return JSON.stringify({ ok: false, error: message });
  }
})()
"@
    $result = Invoke-CdpSmokeJson -WebSocketUrl $WebSocketUrl -Expression $expression -TimeoutSeconds $TimeoutSeconds
    if (-not $result.ok) {
        throw "CDP DOM smoke action failed: $($result.error)"
    }
    return $result.value
}

function Wait-CdpSmokeJsonReady {
    param(
        [string]$WebSocketUrl,
        [string]$Expression,
        [string]$Label,
        [int]$TimeoutSeconds = 30
    )
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $last = ""
    do {
        try {
            $result = Invoke-CdpSmokeJson -WebSocketUrl $WebSocketUrl -Expression $Expression -TimeoutSeconds 6
            if ($result.ready) {
                return $result
            }
            $last = $result | ConvertTo-Json -Depth 6 -Compress
        } catch {
            $last = $_.Exception.Message
        }
        Start-Sleep -Milliseconds 350
    } while ((Get-Date) -lt $deadline)
    throw "timed out waiting for $Label; last=$last"
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
using System.Collections.Generic;
using System.Text;
using System.Threading;
using System.Runtime.InteropServices;

namespace CodexHelperSmoke {
    public sealed class TrayButtonInfo {
        public IntPtr RootHandle { get; set; }
        public IntPtr ToolbarHandle { get; set; }
        public int Index { get; set; }
        public string Text { get; set; }
        public bool IsOverflow { get; set; }
        public bool RootVisible { get; set; }
        public int ClientCenterX { get; set; }
        public int ClientCenterY { get; set; }
        public int ScreenCenterX { get; set; }
        public int ScreenCenterY { get; set; }
        public string Source { get; set; }
    }

    public static class NativeWindow {
        public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

        private const uint PROCESS_VM_OPERATION = 0x0008;
        private const uint PROCESS_VM_READ = 0x0010;
        private const uint PROCESS_VM_WRITE = 0x0020;
        private const uint MEM_COMMIT = 0x1000;
        private const uint MEM_RESERVE = 0x2000;
        private const uint MEM_RELEASE = 0x8000;
        private const uint PAGE_READWRITE = 0x04;
        private const uint TB_GETBUTTON = 0x0417;
        private const uint TB_BUTTONCOUNT = 0x0418;
        private const uint TB_GETITEMRECT = 0x041D;
        private const uint TB_GETBUTTONTEXTW = 0x044B;
        private const uint WM_MOUSEMOVE = 0x0200;
        private const uint WM_RBUTTONDOWN = 0x0204;
        private const uint WM_RBUTTONUP = 0x0205;
        private const uint WM_USER_TRAYICON = 6002;
        private const uint MK_RBUTTON = 0x0002;
        private const int SW_SHOWNOACTIVATE = 4;
        private const uint MOUSEEVENTF_LEFTDOWN = 0x0002;
        private const uint MOUSEEVENTF_LEFTUP = 0x0004;
        private const uint MOUSEEVENTF_RIGHTDOWN = 0x0008;
        private const uint MOUSEEVENTF_RIGHTUP = 0x0010;
        private const int S_OK = 0;
        private const byte VK_DOWN = 0x28;
        private const byte VK_RETURN = 0x0D;
        private const uint KEYEVENTF_KEYUP = 0x0002;

        [StructLayout(LayoutKind.Sequential)]
        public struct POINT {
            public int X;
            public int Y;
        }

        [StructLayout(LayoutKind.Sequential)]
        public struct RECT {
            public int Left;
            public int Top;
            public int Right;
            public int Bottom;
        }

        [StructLayout(LayoutKind.Sequential)]
        public struct NOTIFYICONIDENTIFIER {
            public uint cbSize;
            public IntPtr hWnd;
            public uint uID;
            public Guid guidItem;
        }

        [DllImport("user32.dll")]
        public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);

        [DllImport("user32.dll")]
        public static extern bool EnumChildWindows(IntPtr hWndParent, EnumWindowsProc lpEnumFunc, IntPtr lParam);

        [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        public static extern IntPtr FindWindow(string lpClassName, string lpWindowName);

        [DllImport("user32.dll")]
        public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);

        [DllImport("user32.dll")]
        public static extern bool IsWindowVisible(IntPtr hWnd);

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        public static extern int GetClassName(IntPtr hWnd, StringBuilder className, int count);

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        public static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int count);

        [DllImport("user32.dll")]
        public static extern bool PostMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);

        [DllImport("user32.dll")]
        public static extern IntPtr SendMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);

        [DllImport("user32.dll")]
        public static extern bool SetForegroundWindow(IntPtr hWnd);

        [DllImport("user32.dll")]
        public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

        [DllImport("user32.dll")]
        public static extern bool ClientToScreen(IntPtr hWnd, ref POINT point);

        [DllImport("user32.dll")]
        public static extern bool SetCursorPos(int x, int y);

        [DllImport("user32.dll")]
        public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extraInfo);

        [DllImport("user32.dll")]
        public static extern void keybd_event(byte virtualKey, byte scanCode, uint flags, UIntPtr extraInfo);

        [DllImport("shell32.dll", SetLastError = false)]
        private static extern int Shell_NotifyIconGetRect(ref NOTIFYICONIDENTIFIER identifier, out RECT iconLocation);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern IntPtr OpenProcess(uint desiredAccess, bool inheritHandle, uint processId);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool CloseHandle(IntPtr handle);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern IntPtr VirtualAllocEx(IntPtr process, IntPtr address, UIntPtr size, uint allocationType, uint protect);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool VirtualFreeEx(IntPtr process, IntPtr address, UIntPtr size, uint freeType);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool ReadProcessMemory(IntPtr process, IntPtr baseAddress, byte[] buffer, int size, out IntPtr bytesRead);

        public static TrayButtonInfo FindTrayButton(string textContains) {
            foreach (var root in TrayRoots()) {
                var toolbars = ChildToolbars(root.Handle);
                foreach (var toolbar in toolbars) {
                    var button = FindTrayButtonInToolbar(root.Handle, toolbar, root.IsOverflow, textContains);
                    if (button != null) {
                        return button;
                    }
                }
            }
            return null;
        }

        public static bool OpenTrayContextMenu(TrayButtonInfo button) {
            if (button == null || button.ToolbarHandle == IntPtr.Zero) {
                return false;
            }
            if (button.RootHandle != IntPtr.Zero && !IsWindowVisible(button.RootHandle)) {
                ShowWindow(button.RootHandle, SW_SHOWNOACTIVATE);
                Thread.Sleep(250);
            }
            var point = new POINT { X = button.ClientCenterX, Y = button.ClientCenterY };
            ClientToScreen(button.ToolbarHandle, ref point);
            button.ScreenCenterX = point.X;
            button.ScreenCenterY = point.Y;
            SetCursorPos(point.X, point.Y);
            SetForegroundWindow(button.ToolbarHandle);
            var lParam = MakeLParam(button.ClientCenterX, button.ClientCenterY);
            SendMessage(button.ToolbarHandle, WM_MOUSEMOVE, IntPtr.Zero, lParam);
            SendMessage(button.ToolbarHandle, WM_RBUTTONDOWN, (IntPtr)MK_RBUTTON, lParam);
            Thread.Sleep(80);
            SendMessage(button.ToolbarHandle, WM_RBUTTONUP, IntPtr.Zero, lParam);
            return true;
        }

        public static IntPtr[] FindVisiblePopupMenuWindows() {
            var windows = new List<IntPtr>();
            EnumWindows((hWnd, lParam) => {
                if (IsWindowVisible(hWnd) && WindowClass(hWnd) == "#32768") {
                    windows.Add(hWnd);
                }
                return true;
            }, IntPtr.Zero);
            return windows.ToArray();
        }

        public static TrayButtonInfo FindNotifyIconByProcess(uint processId) {
            var hwnd = FindTopLevelWindowByClassAndProcess("tray_icon_app", processId);
            if (hwnd == IntPtr.Zero) {
                return null;
            }
            for (uint id = 1; id <= 256; id++) {
                var identifier = new NOTIFYICONIDENTIFIER {
                    cbSize = (uint)Marshal.SizeOf(typeof(NOTIFYICONIDENTIFIER)),
                    hWnd = hwnd,
                    uID = id,
                    guidItem = Guid.Empty
                };
                RECT rect;
                if (Shell_NotifyIconGetRect(ref identifier, out rect) != S_OK) {
                    continue;
                }
                var width = Math.Max(1, rect.Right - rect.Left);
                var height = Math.Max(1, rect.Bottom - rect.Top);
                return new TrayButtonInfo {
                    RootHandle = hwnd,
                    ToolbarHandle = hwnd,
                    Index = (int)id,
                    Text = "Shell_NotifyIcon",
                    IsOverflow = false,
                    RootVisible = IsWindowVisible(hwnd),
                    ClientCenterX = 0,
                    ClientCenterY = 0,
                    ScreenCenterX = rect.Left + width / 2,
                    ScreenCenterY = rect.Top + height / 2,
                    Source = "notifyicon"
                };
            }
            return null;
        }

        public static bool OpenNotifyIconContextMenu(TrayButtonInfo button) {
            if (button == null || button.RootHandle == IntPtr.Zero) {
                return false;
            }
            SetCursorPos(button.ScreenCenterX, button.ScreenCenterY);
            SetForegroundWindow(button.RootHandle);
            var downPosted = PostMessage(button.RootHandle, WM_USER_TRAYICON, (IntPtr)button.Index, (IntPtr)WM_RBUTTONDOWN);
            Thread.Sleep(80);
            var upPosted = PostMessage(button.RootHandle, WM_USER_TRAYICON, (IntPtr)button.Index, (IntPtr)WM_RBUTTONUP);
            return downPosted && upPosted;
        }

        public static string DescribeVisibleTopLevelWindows() {
            var windows = new List<string>();
            EnumWindows((hWnd, lParam) => {
                if (!IsWindowVisible(hWnd)) {
                    return true;
                }
                var cls = WindowClass(hWnd);
                var title = WindowTitle(hWnd);
                var probe = (cls + " " + title).ToLowerInvariant();
                if (probe.Contains("tray") ||
                    probe.Contains("notify") ||
                    probe.Contains("overflow") ||
                    probe.Contains("xaml") ||
                    probe.Contains("shell") ||
                    probe.Contains("task") ||
                    probe.Contains("codex")) {
                    windows.Add(cls + ":" + title);
                }
                return windows.Count < 80;
            }, IntPtr.Zero);
            return String.Join(" | ", windows.ToArray());
        }

        public static void LeftClickScreenPoint(int x, int y) {
            SetCursorPos(x, y);
            mouse_event(MOUSEEVENTF_LEFTDOWN, (uint)x, (uint)y, 0, UIntPtr.Zero);
            Thread.Sleep(60);
            mouse_event(MOUSEEVENTF_LEFTUP, (uint)x, (uint)y, 0, UIntPtr.Zero);
        }

        public static void RightClickScreenPoint(int x, int y) {
            SetCursorPos(x, y);
            mouse_event(MOUSEEVENTF_RIGHTDOWN, (uint)x, (uint)y, 0, UIntPtr.Zero);
            Thread.Sleep(80);
            mouse_event(MOUSEEVENTF_RIGHTUP, (uint)x, (uint)y, 0, UIntPtr.Zero);
        }

        public static void SendMenuNavigationKeys(int downCount) {
            for (var i = 0; i < downCount; i++) {
                PressVirtualKey(VK_DOWN);
                Thread.Sleep(80);
            }
            PressVirtualKey(VK_RETURN);
        }

        private static void PressVirtualKey(byte virtualKey) {
            keybd_event(virtualKey, 0, 0, UIntPtr.Zero);
            Thread.Sleep(35);
            keybd_event(virtualKey, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        }

        private sealed class TrayRoot {
            public IntPtr Handle;
            public bool IsOverflow;
        }

        private static IEnumerable<TrayRoot> TrayRoots() {
            var shell = FindWindow("Shell_TrayWnd", null);
            if (shell != IntPtr.Zero) {
                yield return new TrayRoot { Handle = shell, IsOverflow = false };
            }
            var overflow = FindWindow("NotifyIconOverflowWindow", null);
            if (overflow != IntPtr.Zero) {
                yield return new TrayRoot { Handle = overflow, IsOverflow = true };
            }
        }

        private static List<IntPtr> ChildToolbars(IntPtr root) {
            var toolbars = new List<IntPtr>();
            EnumChildWindows(root, (hWnd, lParam) => {
                if (WindowClass(hWnd) == "ToolbarWindow32") {
                    toolbars.Add(hWnd);
                }
                return true;
            }, IntPtr.Zero);
            return toolbars;
        }

        private static IntPtr FindTopLevelWindowByClassAndProcess(string className, uint processId) {
            var found = IntPtr.Zero;
            EnumWindows((hWnd, lParam) => {
                if (WindowClass(hWnd) != className) {
                    return true;
                }
                uint windowProcessId;
                GetWindowThreadProcessId(hWnd, out windowProcessId);
                if (windowProcessId == processId) {
                    found = hWnd;
                    return false;
                }
                return true;
            }, IntPtr.Zero);
            return found;
        }

        private static TrayButtonInfo FindTrayButtonInToolbar(IntPtr root, IntPtr toolbar, bool isOverflow, string textContains) {
            var count = SendMessage(toolbar, TB_BUTTONCOUNT, IntPtr.Zero, IntPtr.Zero).ToInt32();
            if (count <= 0) {
                return null;
            }
            uint processId;
            GetWindowThreadProcessId(toolbar, out processId);
            var process = OpenProcess(PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE, false, processId);
            if (process == IntPtr.Zero) {
                return null;
            }
            var buttonSize = IntPtr.Size == 8 ? 32 : 20;
            var textBytes = 2048;
            var buttonMem = IntPtr.Zero;
            var textMem = IntPtr.Zero;
            var rectMem = IntPtr.Zero;
            try {
                buttonMem = VirtualAllocEx(process, IntPtr.Zero, (UIntPtr)buttonSize, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
                textMem = VirtualAllocEx(process, IntPtr.Zero, (UIntPtr)textBytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
                rectMem = VirtualAllocEx(process, IntPtr.Zero, (UIntPtr)16, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
                if (buttonMem == IntPtr.Zero || textMem == IntPtr.Zero || rectMem == IntPtr.Zero) {
                    return null;
                }
                for (var index = 0; index < count; index++) {
                    if (SendMessage(toolbar, TB_GETBUTTON, (IntPtr)index, buttonMem) == IntPtr.Zero) {
                        continue;
                    }
                    var buttonBuffer = ReadRemoteBytes(process, buttonMem, buttonSize);
                    var idCommand = BitConverter.ToInt32(buttonBuffer, 4);
                    SendMessage(toolbar, TB_GETBUTTONTEXTW, (IntPtr)idCommand, textMem);
                    var text = ReadRemoteUtf16(process, textMem, textBytes);
                    if (String.IsNullOrWhiteSpace(text) ||
                        text.IndexOf(textContains, StringComparison.OrdinalIgnoreCase) < 0) {
                        continue;
                    }
                    if (SendMessage(toolbar, TB_GETITEMRECT, (IntPtr)index, rectMem) == IntPtr.Zero) {
                        continue;
                    }
                    var rectBuffer = ReadRemoteBytes(process, rectMem, 16);
                    var left = BitConverter.ToInt32(rectBuffer, 0);
                    var top = BitConverter.ToInt32(rectBuffer, 4);
                    var right = BitConverter.ToInt32(rectBuffer, 8);
                    var bottom = BitConverter.ToInt32(rectBuffer, 12);
                    var clientX = left + Math.Max(1, right - left) / 2;
                    var clientY = top + Math.Max(1, bottom - top) / 2;
                    var screen = new POINT { X = clientX, Y = clientY };
                    ClientToScreen(toolbar, ref screen);
                    return new TrayButtonInfo {
                        RootHandle = root,
                        ToolbarHandle = toolbar,
                        Index = index,
                        Text = text,
                        IsOverflow = isOverflow,
                        RootVisible = IsWindowVisible(root),
                        ClientCenterX = clientX,
                        ClientCenterY = clientY,
                        ScreenCenterX = screen.X,
                        ScreenCenterY = screen.Y,
                        Source = isOverflow ? "overflow" : "taskbar"
                    };
                }
                return null;
            } finally {
                if (buttonMem != IntPtr.Zero) {
                    VirtualFreeEx(process, buttonMem, UIntPtr.Zero, MEM_RELEASE);
                }
                if (textMem != IntPtr.Zero) {
                    VirtualFreeEx(process, textMem, UIntPtr.Zero, MEM_RELEASE);
                }
                if (rectMem != IntPtr.Zero) {
                    VirtualFreeEx(process, rectMem, UIntPtr.Zero, MEM_RELEASE);
                }
                CloseHandle(process);
            }
        }

        private static byte[] ReadRemoteBytes(IntPtr process, IntPtr address, int size) {
            var buffer = new byte[size];
            IntPtr bytesRead;
            if (!ReadProcessMemory(process, address, buffer, size, out bytesRead)) {
                return new byte[size];
            }
            return buffer;
        }

        private static string ReadRemoteUtf16(IntPtr process, IntPtr address, int size) {
            var bytes = ReadRemoteBytes(process, address, size);
            var text = Encoding.Unicode.GetString(bytes);
            var nullIndex = text.IndexOf('\0');
            if (nullIndex >= 0) {
                text = text.Substring(0, nullIndex);
            }
            return text;
        }

        private static string WindowClass(IntPtr hWnd) {
            var buffer = new StringBuilder(256);
            GetClassName(hWnd, buffer, buffer.Capacity);
            return buffer.ToString();
        }

        private static string WindowTitle(IntPtr hWnd) {
            var buffer = new StringBuilder(256);
            GetWindowText(hWnd, buffer, buffer.Capacity);
            return buffer.ToString();
        }

        private static IntPtr MakeLParam(int low, int high) {
            var value = (high << 16) | (low & 0xFFFF);
            return (IntPtr)value;
        }
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

function Find-CodexTrayButton {
    foreach ($needle in @("codex-helper local proxy control center", "codex-helper")) {
        $button = [CodexHelperSmoke.NativeWindow]::FindTrayButton($needle)
        if ($null -ne $button) {
            return $button
        }
    }
    throw "codex-helper tray icon was not found in the taskbar or notification overflow toolbar."
}

function Get-AutomationRootByWindowClass {
    param([string]$ClassName)
    $handle = [CodexHelperSmoke.NativeWindow]::FindWindow($ClassName, $null)
    if ($handle -eq [IntPtr]::Zero) {
        return $null
    }
    try {
        return [System.Windows.Automation.AutomationElement]::FromHandle($handle)
    } catch {
        return $null
    }
}

function Get-TrayAutomationRoots {
    $roots = [System.Collections.Generic.List[object]]::new()
    foreach ($className in @("Shell_TrayWnd", "Shell_SecondaryTrayWnd", "NotifyIconOverflowWindow", "TopLevelWindowForOverflowXamlIsland", "Xaml_WindowedPopupClass")) {
        $root = Get-AutomationRootByWindowClass -ClassName $className
        if ($null -ne $root) {
            $roots.Add($root)
        }
    }
    return @($roots)
}

function Find-AutomationDescendantByName {
    param(
        [object[]]$Roots,
        [string[]]$Names,
        [switch]$Contains
    )
    foreach ($root in $Roots) {
        foreach ($name in $Names) {
            try {
                if (-not $Contains) {
                    $condition = [System.Windows.Automation.PropertyCondition]::new(
                        [System.Windows.Automation.AutomationElement]::NameProperty,
                        $name
                    )
                    $match = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition)
                    if ($null -ne $match) {
                        return $match
                    }
                }
            } catch {
                # Continue with the next narrow automation root.
            }
        }
        try {
            $buttonCondition = [System.Windows.Automation.PropertyCondition]::new(
                [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
                [System.Windows.Automation.ControlType]::Button
            )
            $buttons = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $buttonCondition)
            foreach ($button in $buttons) {
                $buttonName = [string]$button.Current.Name
                foreach ($name in $Names) {
                    $matched = if ($Contains) {
                        $buttonName.IndexOf($name, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
                    } else {
                        $buttonName -eq $name
                    }
                    if ($matched) {
                        return $button
                    }
                }
            }
        } catch {
            # Continue with the next narrow automation root.
        }
    }
    return $null
}

function Get-AutomationElementCenter {
    param([System.Windows.Automation.AutomationElement]$Element)
    try {
        $point = $Element.GetClickablePoint()
        return [pscustomobject]@{ X = [int]$point.X; Y = [int]$point.Y }
    } catch {
        $rect = $Element.Current.BoundingRectangle
        if ($rect.IsEmpty) {
            throw
        }
        return [pscustomobject]@{
            X = [int]($rect.Left + ($rect.Width / 2))
            Y = [int]($rect.Top + ($rect.Height / 2))
        }
    }
}

function Open-HiddenTrayIconsFlyout {
    $roots = @(Get-AutomationRootByWindowClass -ClassName "Shell_TrayWnd") | Where-Object { $null -ne $_ }
    $showHiddenIconsZh = -join ([char[]](0x663E, 0x793A, 0x9690, 0x85CF, 0x7684, 0x56FE, 0x6807))
    $hiddenIconsZh = -join ([char[]](0x9690, 0x85CF, 0x56FE, 0x6807))
    $showHiddenIconZh = -join ([char[]](0x663E, 0x793A, 0x9690, 0x85CF, 0x56FE, 0x6807))
    $button = Find-AutomationDescendantByName `
        -Roots $roots `
        -Names @(
            "Show hidden icons",
            "Show Hidden Icons",
            "Hidden icons",
            "Notification overflow",
            $showHiddenIconsZh,
            $hiddenIconsZh,
            $showHiddenIconZh
        ) `
        -Contains
    if ($null -eq $button) {
        return $false
    }
    try {
        $point = Get-AutomationElementCenter -Element $button
        [CodexHelperSmoke.NativeWindow]::LeftClickScreenPoint($point.X, $point.Y)
        Start-Sleep -Milliseconds 500
        return $true
    } catch {
        return $false
    }
}

function Find-CodexTrayAutomationElement {
    $names = @("codex-helper local proxy control center", "codex-helper")
    $roots = Get-TrayAutomationRoots
    $element = Find-AutomationDescendantByName -Roots $roots -Names $names -Contains
    if ($null -ne $element) {
        return $element
    }
    [void](Open-HiddenTrayIconsFlyout)
    Start-Sleep -Milliseconds 500
    $roots = Get-TrayAutomationRoots
    return Find-AutomationDescendantByName -Roots $roots -Names $names -Contains
}

function Get-TrayAutomationButtonSummary {
    $names = [System.Collections.Generic.List[string]]::new()
    foreach ($root in Get-TrayAutomationRoots) {
        try {
            $buttonCondition = [System.Windows.Automation.PropertyCondition]::new(
                [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
                [System.Windows.Automation.ControlType]::Button
            )
            $buttons = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $buttonCondition)
            foreach ($button in $buttons) {
                $name = [string]$button.Current.Name
                if (-not [string]::IsNullOrWhiteSpace($name) -and -not $names.Contains($name)) {
                    $names.Add($name)
                }
            }
        } catch {
            # Keep diagnostics best-effort only.
        }
    }
    return (@($names) | Select-Object -First 80) -join " | "
}

function Invoke-CodexTrayMenuItemViaAutomation {
    param([string]$Text)
    $element = Find-CodexTrayAutomationElement
    if ($null -eq $element) {
        $summary = Get-TrayAutomationButtonSummary
        $windows = [CodexHelperSmoke.NativeWindow]::DescribeVisibleTopLevelWindows()
        throw "codex-helper tray icon was not found through targeted UI Automation roots. buttons=[$summary]; windows=[$windows]"
    }
    $name = [string]$element.Current.Name
    $point = Get-AutomationElementCenter -Element $element
    [CodexHelperSmoke.NativeWindow]::RightClickScreenPoint($point.X, $point.Y)
    $menuDetail = Invoke-VisibleContextMenuItem -Text $Text
    if (-not $menuDetail) {
        throw "tray context menu item '$Text' was not found after UIA right-click on '$name'."
    }
    return "uia_tray_name=$name; screen=$($point.X),$($point.Y); $menuDetail"
}

function Invoke-CodexTrayMenuItemViaNotifyIcon {
    param([string]$Text)
    if (-not $script:CodexTrayProcessId) {
        throw "desktop process id is not available for Shell_NotifyIcon lookup."
    }
    [void](Open-HiddenTrayIconsFlyout)
    Start-Sleep -Milliseconds 350
    $button = [CodexHelperSmoke.NativeWindow]::FindNotifyIconByProcess([uint32]$script:CodexTrayProcessId)
    if ($null -eq $button) {
        throw "Shell_NotifyIconGetRect did not find a tray icon for desktop pid $script:CodexTrayProcessId."
    }
    if (-not [CodexHelperSmoke.NativeWindow]::OpenNotifyIconContextMenu($button)) {
        throw "failed to open Shell_NotifyIcon context menu for desktop pid $script:CodexTrayProcessId."
    }
    $menuDetail = Invoke-VisibleContextMenuItem -Text $Text
    if (-not $menuDetail) {
        $keyboardDetail = Invoke-VisibleContextMenuItemByKeyboard -Text $Text
        return "notifyicon_id=$($button.Index); screen=$($button.ScreenCenterX),$($button.ScreenCenterY); $keyboardDetail; uia_menu_fallback=missing"
    }
    return "notifyicon_id=$($button.Index); screen=$($button.ScreenCenterX),$($button.ScreenCenterY); $menuDetail"
}

function Invoke-VisibleContextMenuItem {
    param(
        [string]$Text,
        [int]$TimeoutSeconds = 8
    )
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        foreach ($menuHandle in [CodexHelperSmoke.NativeWindow]::FindVisiblePopupMenuWindows()) {
            try {
                $root = [System.Windows.Automation.AutomationElement]::FromHandle($menuHandle)
                $items = $root.FindAll(
                    [System.Windows.Automation.TreeScope]::Descendants,
                    [System.Windows.Automation.Condition]::TrueCondition
                )
                foreach ($item in $items) {
                    $name = [string]$item.Current.Name
                    if ($name -ne $Text) {
                        continue
                    }
                    try {
                        $pattern = $item.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
                        $pattern.Invoke()
                    } catch {
                        $point = $item.GetClickablePoint()
                        [CodexHelperSmoke.NativeWindow]::LeftClickScreenPoint([int]$point.X, [int]$point.Y)
                    }
                    return "menu_hwnd=$menuHandle; item=$name"
                }
            } catch {
                # Ignore unrelated or disappearing popup menus while waiting for the tray menu.
            }
        }
        Start-Sleep -Milliseconds 120
    } while ((Get-Date) -lt $deadline)
    return $null
}

function Get-VisibleContextMenuSummary {
    $names = [System.Collections.Generic.List[string]]::new()
    $handleCount = 0
    foreach ($menuHandle in [CodexHelperSmoke.NativeWindow]::FindVisiblePopupMenuWindows()) {
        $handleCount++
        try {
            $root = [System.Windows.Automation.AutomationElement]::FromHandle($menuHandle)
            $items = $root.FindAll(
                [System.Windows.Automation.TreeScope]::Descendants,
                [System.Windows.Automation.Condition]::TrueCondition
            )
            foreach ($item in $items) {
                $name = [string]$item.Current.Name
                if (-not [string]::IsNullOrWhiteSpace($name) -and -not $names.Contains($name)) {
                    $names.Add($name)
                }
            }
        } catch {
            # Diagnostics only.
        }
    }
    $nameSummary = (@($names) | Select-Object -First 80) -join " | "
    return "handles=$handleCount; names=[$nameSummary]"
}

function Invoke-VisibleContextMenuItemByKeyboard {
    param([string]$Text)
    $downCount = switch ($Text) {
        "Show Window" { 1; break }
        "Hide to Tray" { 2; break }
        "Quit App (Proxy Keeps Running)" { 3; break }
        default { throw "no keyboard fallback is defined for tray menu item '$Text'." }
    }
    [CodexHelperSmoke.NativeWindow]::SendMenuNavigationKeys($downCount)
    return "keyboard_downs=$downCount; item=$Text"
}

function Invoke-CodexTrayMenuItem {
    param([string]$Text)
    $notifyError = $null
    try {
        return Invoke-CodexTrayMenuItemViaNotifyIcon -Text $Text
    } catch {
        $popupSummary = Get-VisibleContextMenuSummary
        $notifyError = "$($_.Exception.Message); popup_items=[$popupSummary]"
    }
    $nativeError = $null
    try {
        $button = Find-CodexTrayButton
        if (-not [CodexHelperSmoke.NativeWindow]::OpenTrayContextMenu($button)) {
            throw "failed to open codex-helper tray context menu from $($button.Source) toolbar."
        }
        $menuDetail = Invoke-VisibleContextMenuItem -Text $Text
        if (-not $menuDetail) {
            throw "tray context menu item '$Text' was not found after opening $($button.Source) tray icon '$($button.Text)'."
        }
        return "button_source=$($button.Source); button_text=$($button.Text); screen=$($button.ScreenCenterX),$($button.ScreenCenterY); $menuDetail"
    } catch {
        $nativeError = $_.Exception.Message
    }
    try {
        $automationDetail = Invoke-CodexTrayMenuItemViaAutomation -Text $Text
        return "$automationDetail; notify_fallback_reason=$notifyError; native_fallback_reason=$nativeError"
    } catch {
        throw "$notifyError; $nativeError; UIA fallback failed: $($_.Exception.Message)"
    }
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

function Get-RunKeySnapshot {
    $path = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
    $snapshot = @{}
    try {
        $item = Get-ItemProperty -Path $path -ErrorAction Stop
        foreach ($property in $item.PSObject.Properties) {
            if ($property.Name -like "PS*") {
                continue
            }
            $snapshot[$property.Name] = [string]$property.Value
        }
    } catch {
        # Missing Run key is unusual but not fatal for a before/after smoke snapshot.
    }
    return $snapshot
}

function Restore-SmokeRunKeyChanges {
    param(
        [hashtable]$Before,
        [string]$InstallPath
    )
    $path = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
    $after = Get-RunKeySnapshot
    foreach ($name in $after.Keys) {
        $value = [string]$after[$name]
        $looksLikeSmokeEntry =
            $value.IndexOf($InstallPath, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
            $name -match "codex|latias"
        if (-not $looksLikeSmokeEntry) {
            continue
        }
        if ($Before.ContainsKey($name)) {
            Set-ItemProperty -Path $path -Name $name -Value $Before[$name] -ErrorAction SilentlyContinue
        } else {
            Remove-ItemProperty -Path $path -Name $name -ErrorAction SilentlyContinue
        }
    }
}

function Get-RunKeyChanges {
    param(
        [hashtable]$Before,
        [hashtable]$After,
        [string]$InstallPath
    )
    $changes = [System.Collections.Generic.List[string]]::new()
    foreach ($name in $After.Keys) {
        $value = [string]$After[$name]
        $changed = (-not $Before.ContainsKey($name)) -or ([string]$Before[$name] -ne $value)
        $looksLikeSmokeEntry =
            $value.IndexOf($InstallPath, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
            $name -match "codex|latias"
        if ($changed -and $looksLikeSmokeEntry) {
            $changes.Add("${name}=${value}")
        }
    }
    return @($changes)
}

function Write-SmokeConfig {
    param([string]$Path)
$config = @'
version = 5

[codex.providers.relay]
alias = "Relay Smoke"
base_url = "https://relay.example/v1"
auth_token_env = "RELAY_API_KEY"
enabled = true

[codex.routing]
entry = "main"
affinity_policy = "fallback-sticky"

[codex.routing.routes.main]
strategy = "manual-sticky"
target = "relay"
'@
    [System.IO.File]::WriteAllText($Path, $config, [System.Text.UTF8Encoding]::new($false))
}

$installer = Resolve-Path -LiteralPath $InstallerPath
$installPath = [System.IO.Path]::GetFullPath($InstallDir)
$smokeHome = Join-Path $env:TEMP ("codex-helper-tdrp-080-home-" + [guid]::NewGuid().ToString("N"))
$smokeCodexHome = Join-Path $env:TEMP ("codex-helper-tdrp-080-codex-home-" + [guid]::NewGuid().ToString("N"))
$smokeProxyPort = $null
$smokeAdminUrl = $AdminUrl
if ([string]::IsNullOrWhiteSpace($smokeAdminUrl)) {
    $portPair = Get-FreeProxyAdminPortPair
    $smokeProxyPort = [int]$portPair.proxy
    $smokeAdminUrl = "http://127.0.0.1:$($portPair.admin)"
}
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
$env:CODEX_HOME = $smokeCodexHome
$env:CODEX_HELPER_DESKTOP_ADMIN_URL = $smokeAdminUrl
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
    New-Item -ItemType Directory -Path $smokeCodexHome | Out-Null
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
    $script:CodexTrayProcessId = $desktop.Id
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

    try {
        $windowBeforeTrayShow = Get-MainWindowHandle -ProcessId $desktop.Id
        if ($windowBeforeTrayShow -ne [IntPtr]::Zero -and (Test-WindowVisible -Handle $windowBeforeTrayShow)) {
            [void][CodexHelperSmoke.NativeWindow]::PostMessage($windowBeforeTrayShow, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
            Start-Sleep -Seconds 2
        }
        $hiddenBeforeTrayShow = -not (Test-WindowVisible -Handle (Get-MainWindowHandle -ProcessId $desktop.Id))
        $trayShowDetail = Invoke-CodexTrayMenuItem -Text "Show Window"
        Start-Sleep -Seconds 2
        $visibleAfterTrayShow = Test-WindowVisible -Handle (Get-MainWindowHandle -ProcessId $desktop.Id)
        $results.Add((New-SmokeResult `
            -Name "tray-menu-show-window" `
            -Passed ($hiddenBeforeTrayShow -and $visibleAfterTrayShow -and -not $desktop.HasExited) `
            -Detail "hidden_before=$hiddenBeforeTrayShow; visible_after=$visibleAfterTrayShow; $trayShowDetail"))

        $trayHideDetail = Invoke-CodexTrayMenuItem -Text "Hide to Tray"
        Start-Sleep -Seconds 2
        $hiddenAfterTrayHide = -not (Test-WindowVisible -Handle (Get-MainWindowHandle -ProcessId $desktop.Id))
        $trayRestoreDetail = Invoke-CodexTrayMenuItem -Text "Show Window"
        Start-Sleep -Seconds 2
        $visibleAfterTrayRestore = Test-WindowVisible -Handle (Get-MainWindowHandle -ProcessId $desktop.Id)
        $results.Add((New-SmokeResult `
            -Name "tray-menu-hide-to-tray" `
            -Passed ($hiddenAfterTrayHide -and $visibleAfterTrayRestore -and -not $desktop.HasExited) `
            -Detail "hidden_after_hide=$hiddenAfterTrayHide; visible_after_restore=$visibleAfterTrayRestore; hide=[$trayHideDetail]; restore=[$trayRestoreDetail]"))
    } catch {
        $results.Add((New-SmokeResult `
            -Name "tray-menu-show-hide-smoke" `
            -Passed $false `
            -Detail $_.Exception.Message))
    }

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

            if ($RunAutostartSmoke) {
                $runKeyBefore = Get-RunKeySnapshot
                $wasAutostartEnabled = $false
                $enabledAfterEnable = $false
                $disabledAfterDisable = $false
                $runKeyChanges = @()
                try {
                    $wasAutostartEnabled = [bool](Invoke-TauriCommandViaCdp `
                        -WebSocketUrl $webSocketUrl `
                        -Command "plugin:autostart|is_enabled")
                    if ($wasAutostartEnabled) {
                        Invoke-TauriCommandViaCdp `
                            -WebSocketUrl $webSocketUrl `
                            -Command "plugin:autostart|disable" | Out-Null
                        Start-Sleep -Milliseconds 500
                        $disabledAfterDisable = -not [bool](Invoke-TauriCommandViaCdp `
                            -WebSocketUrl $webSocketUrl `
                            -Command "plugin:autostart|is_enabled")
                    }
                    Invoke-TauriCommandViaCdp `
                        -WebSocketUrl $webSocketUrl `
                        -Command "plugin:autostart|enable" | Out-Null
                    Start-Sleep -Milliseconds 800
                    $enabledAfterEnable = [bool](Invoke-TauriCommandViaCdp `
                        -WebSocketUrl $webSocketUrl `
                        -Command "plugin:autostart|is_enabled")
                    $runKeyAfterEnable = Get-RunKeySnapshot
                    $runKeyChanges = Get-RunKeyChanges `
                        -Before $runKeyBefore `
                        -After $runKeyAfterEnable `
                        -InstallPath $installPath
                    Invoke-TauriCommandViaCdp `
                        -WebSocketUrl $webSocketUrl `
                        -Command "plugin:autostart|disable" | Out-Null
                    Start-Sleep -Milliseconds 800
                    $disabledAfterDisable = -not [bool](Invoke-TauriCommandViaCdp `
                        -WebSocketUrl $webSocketUrl `
                        -Command "plugin:autostart|is_enabled")
                } finally {
                    Restore-SmokeRunKeyChanges -Before $runKeyBefore -InstallPath $installPath
                }
                $results.Add((New-SmokeResult `
                    -Name "packaged-autostart-os-registration" `
                    -Passed ($enabledAfterEnable -and $disabledAfterDisable -and @($runKeyChanges).Count -gt 0) `
                    -Detail "was_enabled=$wasAutostartEnabled; enabled_after_enable=$enabledAfterEnable; disabled_after_disable=$disabledAfterDisable; run_key_changes=$(@($runKeyChanges) -join '; ')"))
            }

            $exportPath = Join-Path $smokeHome "codex-helper-export.toml"
            $exportResult = Invoke-TauriCommandViaCdp `
                -WebSocketUrl $webSocketUrl `
                -Command "export_config" `
                -CommandArgs @{ payload = @{ destination = $exportPath } }
            $results.Add((New-SmokeResult `
                -Name "packaged-export-config-command" `
                -Passed ($exportResult.ok -and (Test-Path -LiteralPath $exportPath) -and $exportResult.secretWarning) `
                -Detail "destination=$($exportResult.destination); secret_warning=$($exportResult.secretWarning)"))

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
                -Passed ($importResult.ok -and $backupExists -and $importResult.secretWarning) `
                -Detail "source=$($importResult.source); destination=$($importResult.destination); backup=$($importResult.backup); backup_exists=$backupExists; secret_warning=$($importResult.secretWarning)"))

            $startResult = Invoke-TauriCommandViaCdp -WebSocketUrl $webSocketUrl -Command "start_desktop_proxy"
            $runtimeStarted = $true
            $startState = $startResult.state
            $adminReachable = Test-AdminReachable -BaseUrl $smokeAdminUrl
            $results.Add((New-SmokeResult `
                -Name "packaged-starts-managed-sidecar" `
                -Passed ($startResult.ok -and $adminReachable -and $startState.connectionMode -eq "desktop-owned") `
                -Detail "action=$($startResult.action); reachable=$adminReachable; mode=$($startState.connectionMode); admin=$($startState.adminBaseUrl)"))

            Invoke-CdpSmokeJson -WebSocketUrl $webSocketUrl -Expression @'
(() => {
  window.location.hash = "#/providers";
  window.dispatchEvent(new HashChangeEvent("hashchange"));
  return JSON.stringify({ ready: true, hash: window.location.hash });
})()
'@ | Out-Null
            $providerButton = Wait-CdpSmokeJsonReady -WebSocketUrl $webSocketUrl -Label "provider edit button" -Expression @'
(() => {
  const buttons = Array.from(document.querySelectorAll("button"))
    .map((button) => ({
      text: (button.textContent || "").trim(),
      aria: button.getAttribute("aria-label") || "",
      disabled: button.disabled,
    }));
  const edit = buttons.find((button) => button.aria.includes("Relay Smoke") && !button.disabled);
  return JSON.stringify({
    ready: Boolean(edit),
    hash: window.location.hash,
    editLabel: edit ? edit.aria : "",
    buttons: buttons.slice(0, 20),
    body: (document.body.textContent || "").replace(/\s+/g, " ").slice(0, 500),
  });
})()
'@ -TimeoutSeconds 45
            $providerClick = Invoke-CdpSmokeJson -WebSocketUrl $webSocketUrl -Expression @'
(() => {
  const editButton = Array.from(document.querySelectorAll("button"))
    .find((button) => (button.getAttribute("aria-label") || "").includes("Relay Smoke") && !button.disabled);
  if (!editButton) {
    return JSON.stringify({ ready: false, error: "missing edit button" });
  }
  const editLabel = editButton.getAttribute("aria-label") || "";
  editButton.click();
  return JSON.stringify({
    ready: true,
    hash: window.location.hash,
    editLabel,
    providerId: editLabel,
  });
})()
'@
            if (-not $providerClick.ready) {
                throw "provider edit click failed: $($providerClick.error)"
            }
            $providerForm = Wait-CdpSmokeJsonReady -WebSocketUrl $webSocketUrl -Label "provider edit form" -Expression @'
(() => {
  const aliasInput = document.querySelector('input[id^="provider-alias-"]');
  return JSON.stringify({
    ready: Boolean(aliasInput),
    hash: window.location.hash,
    suffix: aliasInput ? aliasInput.id.replace("provider-alias-", "") : "",
    body: (document.body.textContent || "").replace(/\s+/g, " ").slice(0, 500),
  });
})()
'@ -TimeoutSeconds 30
            $providerSave = Invoke-CdpSmokeJson -WebSocketUrl $webSocketUrl -Expression @'
(() => {
  const setInput = (id, value) => {
    const input = document.getElementById(id);
    if (!input) {
      throw new Error(`missing input ${id}`);
    }
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
    setter.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new Event("change", { bubbles: true }));
  };
  const aliasInput = document.querySelector('input[id^="provider-alias-"]');
  if (!aliasInput) {
    return JSON.stringify({ ready: false, error: "missing provider form" });
  }
  const suffix = aliasInput.id.replace("provider-alias-", "");
  setInput(`provider-alias-${suffix}`, "Relay Smoke Edited");
  setInput(`provider-base-url-${suffix}`, "https://relay-edited.example/v1");
  setInput(`provider-auth-token-env-${suffix}`, "RELAY_EDITED_API_KEY");
  const saveButton = document.querySelector('form button[type="submit"]:not(:disabled)');
  if (!saveButton) {
    return JSON.stringify({ ready: false, error: "missing save button", suffix });
  }
  saveButton.click();
  return JSON.stringify({ ready: true, hash: window.location.hash, suffix });
})()
'@
            if (-not $providerSave.ready) {
                throw "provider save click failed: $($providerSave.error)"
            }
            $providerUiResult = Wait-CdpSmokeJsonReady -WebSocketUrl $webSocketUrl -Label "provider save success banner" -Expression @'
(() => {
  const banner = Array.from(document.querySelectorAll('[role="status"]'))
    .find((element) => {
      const text = element.textContent || "";
      return text.includes("provider") && text.includes("relay");
    });
  return JSON.stringify({
    ready: Boolean(banner),
    hash: window.location.hash,
    banner: banner ? (banner.textContent || "") : "",
    body: (document.body.textContent || "").replace(/\s+/g, " ").slice(0, 500),
  });
})()
'@ -TimeoutSeconds 45
            $configAfterProviderEdit = Get-Content -Raw -LiteralPath (Join-Path $smokeHome "config.toml")
            $providerConfigUpdated =
                $configAfterProviderEdit.Contains('alias = "Relay Smoke Edited"') -and
                $configAfterProviderEdit.Contains('base_url = "https://relay-edited.example/v1"') -and
                $configAfterProviderEdit.Contains('auth_token_env = "RELAY_EDITED_API_KEY"')
            $results.Add((New-SmokeResult `
                -Name "packaged-provider-edit-ui" `
                -Passed ($providerConfigUpdated -and [string]$providerUiResult.hash -eq "#/providers" -and [string]$providerUiResult.banner -like "*provider*" -and [string]$providerUiResult.banner -like "*relay*") `
                -Detail "hash=$($providerUiResult.hash); edit_label=$($providerButton.editLabel); suffix=$($providerForm.suffix); config_updated=$providerConfigUpdated; banner=$($providerUiResult.banner)"))

            $hideResult = Invoke-TauriCommandViaCdp -WebSocketUrl $webSocketUrl -Command "hide_main_window"
            Start-Sleep -Seconds 1
            $desktopAliveAfterDetach = $desktop.HasExited -eq $false
            $adminAliveAfterDetach = Test-AdminReachable -BaseUrl $smokeAdminUrl
            $hiddenAfterDetach = -not (Test-WindowVisible -Handle (Get-MainWindowHandle -ProcessId $desktop.Id))
            $results.Add((New-SmokeResult `
                -Name "packaged-detach-keeps-sidecar-running" `
                -Passed ($desktopAliveAfterDetach -and $adminAliveAfterDetach -and $hiddenAfterDetach) `
                -Detail "desktop_alive=$desktopAliveAfterDetach; admin_alive=$adminAliveAfterDetach; hidden=$hiddenAfterDetach; command_result=$hideResult"))

            $thirdLaunch = Start-Process -FilePath $desktopExe -PassThru
            Start-Sleep -Seconds 3
            $thirdExited = $thirdLaunch.HasExited
            $visibleAfterThirdLaunch = Test-WindowVisible -Handle (Get-MainWindowHandle -ProcessId $desktop.Id)
            $adminAliveAfterThirdLaunch = Test-AdminReachable -BaseUrl $smokeAdminUrl
            $results.Add((New-SmokeResult `
                -Name "second-launch-restores-detached-window-with-sidecar" `
                -Passed ($thirdExited -and $visibleAfterThirdLaunch -and $adminAliveAfterThirdLaunch) `
                -Detail "third_pid=$($thirdLaunch.Id); third_exited=$thirdExited; visible=$visibleAfterThirdLaunch; admin_alive=$adminAliveAfterThirdLaunch"))

            $stopResult = Invoke-TauriCommandViaCdp `
                -WebSocketUrl $webSocketUrl `
                -Command "stop_proxy" `
                -CommandArgs @{ payload = @{ scope = "owned"; confirmation = "STOP OWNED PROXY" } }
            Start-Sleep -Seconds 2
            $adminStopped = -not (Test-AdminReachable -BaseUrl $smokeAdminUrl)
            if ($adminStopped) {
                $runtimeStarted = $false
            }
            $results.Add((New-SmokeResult `
                -Name "packaged-owned-stop-proxy-command" `
                -Passed ($stopResult.ok -and $adminStopped) `
                -Detail "action=$($stopResult.action); admin_stopped=$adminStopped; mode=$($stopResult.state.connectionMode)"))

            $restartResult = Invoke-TauriCommandViaCdp -WebSocketUrl $webSocketUrl -Command "start_desktop_proxy"
            $runtimeStarted = $true
            Start-Sleep -Seconds 1
            $trayQuitDetail = Invoke-CodexTrayMenuItem -Text "Quit App (Proxy Keeps Running)"
            Start-Sleep -Seconds 3
            $desktop.Refresh()
            $desktopExitedAfterQuit = $desktop.HasExited
            $adminAliveAfterQuit = Test-AdminReachable -BaseUrl $smokeAdminUrl
            $results.Add((New-SmokeResult `
                -Name "tray-menu-quit-app-leaves-sidecar-running" `
                -Passed ($restartResult.ok -and $desktopExitedAfterQuit -and $adminAliveAfterQuit) `
                -Detail "desktop_exited=$desktopExitedAfterQuit; admin_alive=$adminAliveAfterQuit; restart_action=$($restartResult.action); $trayQuitDetail"))
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
        codex_home = $smokeCodexHome
        admin_url = $smokeAdminUrl
        proxy_port = $smokeProxyPort
        devtools_port = if ($SkipDevToolsSmoke) { $null } else { $DevToolsPort }
        results = $results
        passed = -not ($results | Where-Object { -not $_.passed })
    }
    $summary | ConvertTo-Json -Depth 6
    if (-not $summary.passed) {
        exit 1
    }
} finally {
    if ($runtimeStarted -or (Test-AdminReachable -BaseUrl $smokeAdminUrl)) {
        Invoke-HttpShutdown -BaseUrl $smokeAdminUrl
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
    if (-not $KeepInstall -and (Test-Path -LiteralPath $smokeCodexHome)) {
        try {
            Remove-Item -LiteralPath $smokeCodexHome -Recurse -Force
        } catch {
            Write-Warning "could not remove temporary Codex home ${smokeCodexHome}: $($_.Exception.Message)"
        }
    }
    if ($null -eq $oldWebViewArgs) {
        Remove-Item Env:\WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -ErrorAction SilentlyContinue
    } else {
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $oldWebViewArgs
    }
}

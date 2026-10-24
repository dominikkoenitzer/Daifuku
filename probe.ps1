Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class P {
    [StructLayout(LayoutKind.Sequential)]
    public struct SI {
        public int cb; public IntPtr lpReserved, lpDesktop, lpTitle;
        public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
        public short wShowWindow, cbReserved2; public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct PI { public IntPtr hProcess, hThread; public int dwProcessId, dwThreadId; }
    [StructLayout(LayoutKind.Sequential)]
    public struct WP { public int length, flags, showCmd, minX, minY, maxX, maxY, left, top, right, bottom; }
    public delegate bool EnumProc(IntPtr h, IntPtr l);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CreateProcessW(string app, StringBuilder cmd, IntPtr pa, IntPtr ta, bool inherit, uint flags, IntPtr env, string cwd, ref SI si, out PI pi);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool SystemParametersInfoW(uint a, uint b, ref uint v, uint c);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool GetWindowPlacement(IntPtr h, ref WP p);
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr h);
    [DllImport("user32.dll")] static extern bool AttachThreadInput(uint a, uint b, bool f);

    public static string Desc(IntPtr h) {
        if (h == IntPtr.Zero) return "none";
        uint pid; uint tid = GetWindowThreadProcessId(h, out pid);
        var sb = new StringBuilder(256); GetClassNameW(h, sb, 256);
        string n = "?"; try { n = Process.GetProcessById((int)pid).ProcessName; } catch {}
        return string.Format("0x{0:x} {1} {2} pid {3} tid {4}", h.ToInt64(), sb, n, pid, tid);
    }

    public static uint LockTimeout() { uint v = 0; SystemParametersInfoW(0x2000, 0, ref v, 0); return v; }

    public static IntPtr[] Windows(string cls) {
        var list = new List<IntPtr>();
        EnumWindows((h, l) => {
            var sb = new StringBuilder(256); GetClassNameW(h, sb, 256);
            if (sb.ToString() == cls && IsWindowVisible(h)) list.Add(h);
            return true;
        }, IntPtr.Zero);
        return list.ToArray();
    }

    public static string Placement(IntPtr h) {
        var p = new WP(); p.length = Marshal.SizeOf(p); GetWindowPlacement(h, ref p);
        return Desc(h) + " showCmd " + p.showCmd;
    }

    public static bool Raise(IntPtr h) {
        for (int i = 0; i < 5; i++) {
            IntPtr front = GetForegroundWindow();
            uint pid; uint ft = GetWindowThreadProcessId(front, out pid);
            uint me = GetCurrentThreadId();
            bool att = ft != 0 && ft != me && AttachThreadInput(me, ft, true);
            BringWindowToTop(h); SetForegroundWindow(h);
            if (att) AttachThreadInput(me, ft, false);
            Thread.Sleep(300);
            if (GetForegroundWindow() == h) return true;
        }
        return false;
    }

    public static string LaunchAndWatch(string app, string cmd, bool useShow, short show, int ms) {
        var si = new SI(); si.cb = Marshal.SizeOf(si);
        if (useShow) { si.dwFlags = 1; si.wShowWindow = show; }
        PI pi;
        IntPtr last = GetForegroundWindow();
        var log = new StringBuilder("front " + Desc(last));
        var sw = Stopwatch.StartNew();
        if (!CreateProcessW(app, new StringBuilder(cmd), IntPtr.Zero, IntPtr.Zero, false, 0x08000000, IntPtr.Zero, null, ref si, out pi))
            return log + " | CreateProcess failed " + Marshal.GetLastWin32Error();
        CloseHandle(pi.hThread); CloseHandle(pi.hProcess);
        log.Append(" | started pid " + pi.dwProcessId);
        while (sw.ElapsedMilliseconds < ms) {
            IntPtr f = GetForegroundWindow();
            if (f != last) { log.Append(string.Format(" | {0} ms -> {1}", sw.ElapsedMilliseconds, Desc(f))); last = f; }
            Thread.Sleep(10);
        }
        return log.ToString();
    }
}
'@

$cls = 'CASCADIA_HOSTING_WINDOW_CLASS'
foreach ($p in @(Get-AppxPackage -Name 'Microsoft.WindowsTerminal*')) {
    "package $($p.Name) $($p.Version) $($p.InstallLocation)"
}
$pkg = @(Get-AppxPackage -Name 'Microsoft.WindowsTerminal')[0]
$wt = Join-Path $pkg.InstallLocation 'wt.exe'
$wtapp = Join-Path $pkg.InstallLocation 'WindowsTerminal.exe'
"wt $wt exists $(Test-Path $wt)"
"foreground lock timeout $([P]::LockTimeout()) ms"
"front at start $([P]::Desc([P]::GetForegroundWindow()))"
"settings file $(Test-Path "$env:LOCALAPPDATA\Packages\Microsoft.WindowsTerminal_8wekyb3d8bbwe\LocalState\settings.json")"
Get-Process WindowsTerminal -ErrorAction SilentlyContinue | ForEach-Object { "WindowsTerminal already running, pid $($_.Id)" }

Start-Process notepad
Start-Sleep -Seconds 3
$np = @([P]::Windows('Notepad'))
if ($np.Count -eq 0) { throw 'no notepad window' }
$note = $np[0]
"notepad $([P]::Desc($note))"

function Scenario([string]$name, [IntPtr]$front, [bool]$useShow, [int]$show, [string]$app = $wt) {
    $before = @([P]::Windows($cls))
    if ($front -ne [IntPtr]::Zero -and -not [P]::Raise($front)) {
        "$name could not raise $([P]::Desc($front)), front is $([P]::Desc([P]::GetForegroundWindow()))"
    }
    Start-Sleep -Milliseconds 700
    $line = [P]::LaunchAndWatch($app, "`"$app`" -w new new-tab cmd /k echo $name", $useShow, [int16]$show, 5000)
    $new = @([P]::Windows($cls) | Where-Object { $before -notcontains $_ })
    $placed = ($new | ForEach-Object { [P]::Placement($_) }) -join '; '
    "$name useShow=$useShow show=$show"
    "  $line"
    "  new windows: $placed"
    "  all terminal windows: $((@([P]::Windows($cls)) | ForEach-Object { [P]::Desc($_) }) -join '; ')"
}

Scenario 'fresh-notepad-noactivate' $note $true 4
Scenario 'running-notepad-noactivate' $note $true 4
Scenario 'running-notepad-noflag' $note $false 0
Scenario 'running-notepad-normal' $note $true 1
$term = @([P]::Windows($cls))[0]
Scenario 'terminal-front-noactivate' $term $true 4
Scenario 'terminal-front-showna' $term $true 8
Scenario 'terminal-front-minnoactive' $term $true 7
Scenario 'terminal-front-direct-noactivate' $term $true 4 $wtapp
Scenario 'notepad-again-noactivate' $note $true 4

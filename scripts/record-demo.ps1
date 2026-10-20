<#
.SYNOPSIS
Records `daifuku demo` as the GIF the README shows.

.DESCRIPTION
Runs on a machine nobody is sitting at, such as a CI runner: it switches the
screen to a fixed size, paints the desktop one dark colour, starts a daemon of
its own, opens the six demo agents, approves two of them the way a person
would (`daifuku next`, then Enter), and grabs the work area the whole time.
The frames become a lossless video, and from that the GIFs, a review video
and a few stills.

Never run it on a desktop in use. It refuses to start beside a daemon that
already runs, as the end-to-end tests do, so an installed Daifuku is safe
from it; the resolution and the wallpaper it does change, and it puts the
resolution back only at the end.

.PARAMETER Width
.PARAMETER Height
The screen size to record at. Six terminals need room: at 1600 by 900 the
grid is three by two with cells Windows Terminal accepts, at the 1024 pixels
a runner starts with it is not.

.PARAMETER Seconds
How long to record. Forty seconds show every state: the first round of tasks
ends at about eight seconds, the second at about thirty.

.PARAMETER Fps
Frames per second, of the recording and of the GIFs.

.PARAMETER Target
The folder holding daifuku.exe and daifukud.exe.

.PARAMETER Out
Where the results go.
#>
[CmdletBinding()]
param(
    [int]$Width = 1600,
    [int]$Height = 900,
    [int]$Seconds = 40,
    [int]$Fps = 12,
    [string]$Target = 'target/release',
    [string]$Out = 'dist/demo'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class Desk
{
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct DEVMODE
    {
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmDeviceName;
        public ushort dmSpecVersion, dmDriverVersion, dmSize, dmDriverExtra;
        public uint dmFields;
        public int dmPositionX, dmPositionY;
        public uint dmDisplayOrientation, dmDisplayFixedOutput;
        public short dmColor, dmDuplex, dmYResolution, dmTTOption, dmCollate;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmFormName;
        public ushort dmLogPixels;
        public uint dmBitsPerPel, dmPelsWidth, dmPelsHeight, dmDisplayFlags, dmDisplayFrequency;
        public uint dmICMMethod, dmICMIntent, dmMediaType, dmDitherType, dmReserved1, dmReserved2;
        public uint dmPanningWidth, dmPanningHeight;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern bool EnumDisplaySettingsW(string device, int mode, ref DEVMODE devMode);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int ChangeDisplaySettingsExW(string device, ref DEVMODE devMode, IntPtr hwnd, uint flags, IntPtr param);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, EntryPoint = "SystemParametersInfoW")]
    public static extern bool SetWallpaper(uint action, uint param, string value, uint winIni);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, EntryPoint = "SystemParametersInfoW")]
    public static extern bool GetRect(uint action, uint param, ref RECT rect, uint winIni);

    [DllImport("user32.dll")]
    public static extern bool SetSysColors(int count, int[] elements, int[] colours);

    [DllImport("user32.dll")]
    public static extern int GetSystemMetrics(int index);

    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();

    // SendInput takes the size of INPUT and fails silently on the wrong one:
    // on x64 it is 40 bytes, which the union only reaches with MOUSEINPUT in it.
    [StructLayout(LayoutKind.Sequential)]
    public struct MOUSEINPUT { public int dx, dy; public uint mouseData, dwFlags, time; public IntPtr dwExtraInfo; }

    [StructLayout(LayoutKind.Sequential)]
    public struct KEYBDINPUT { public ushort wVk, wScan; public uint dwFlags, time; public IntPtr dwExtraInfo; }

    [StructLayout(LayoutKind.Explicit)]
    public struct INPUTUNION
    {
        [FieldOffset(0)] public MOUSEINPUT mi;
        [FieldOffset(0)] public KEYBDINPUT ki;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct INPUT { public uint type; public INPUTUNION u; }

    [DllImport("user32.dll", SetLastError = true)]
    public static extern uint SendInput(uint count, INPUT[] inputs, int size);

    // A key with its scan code: Windows Terminal drops a key that has none.
    public static uint Key(ushort vk, ushort scan, bool down)
    {
        var input = new INPUT();
        input.type = 1;
        input.u.ki.wVk = vk;
        input.u.ki.wScan = scan;
        input.u.ki.dwFlags = down ? 0u : 2u;
        return SendInput(1, new[] { input }, Marshal.SizeOf(typeof(INPUT)));
    }

    public static uint Enter()
    {
        return Key(0x0D, 0x1C, true) + Key(0x0D, 0x1C, false);
    }

    public static DEVMODE Current()
    {
        var mode = new DEVMODE();
        mode.dmSize = (ushort)Marshal.SizeOf(typeof(DEVMODE));
        EnumDisplaySettingsW(null, -1, ref mode);
        return mode;
    }

    // The mode of `width` by `height` the display offers, at 32 bits and its
    // highest refresh rate, or null.
    public static DEVMODE? Find(uint width, uint height)
    {
        DEVMODE? best = null;
        var mode = new DEVMODE();
        mode.dmSize = (ushort)Marshal.SizeOf(typeof(DEVMODE));
        for (int i = 0; EnumDisplaySettingsW(null, i, ref mode); i++)
        {
            if (mode.dmPelsWidth != width || mode.dmPelsHeight != height || mode.dmBitsPerPel != 32) continue;
            if (best == null || mode.dmDisplayFrequency > best.Value.dmDisplayFrequency) best = mode;
        }
        return best;
    }

    public static string Modes()
    {
        var seen = new System.Collections.Generic.SortedSet<string>();
        var mode = new DEVMODE();
        mode.dmSize = (ushort)Marshal.SizeOf(typeof(DEVMODE));
        for (int i = 0; EnumDisplaySettingsW(null, i, ref mode); i++)
        {
            if (mode.dmBitsPerPel == 32) seen.Add(mode.dmPelsWidth + "x" + mode.dmPelsHeight);
        }
        return string.Join(" ", seen);
    }

    public static int Apply(DEVMODE mode)
    {
        // Width, height, depth and frequency, and the registry too, so the
        // change outlives this process.
        mode.dmFields = 0x40000 | 0x80000 | 0x100000 | 0x400000;
        return ChangeDisplaySettingsExW(null, ref mode, IntPtr.Zero, 1, IntPtr.Zero);
    }

    public static RECT WorkArea()
    {
        var rect = new RECT();
        GetRect(48, 0, ref rect, 0);
        return rect;
    }
}
'@

function Say([string]$text) {
    Write-Output ("{0:HH:mm:ss.fff} {1}" -f (Get-Date), $text)
}

$cli = Join-Path $Target 'daifuku.exe'
$daemonExe = Join-Path $Target 'daifukud.exe'
foreach ($exe in $cli, $daemonExe) {
    if (-not (Test-Path $exe)) { throw "$exe is missing; build the workspace first" }
}
$cli = (Resolve-Path $cli).Path
$daemonExe = (Resolve-Path $daemonExe).Path
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Out = (Resolve-Path $Out).Path
$stills = Join-Path $Out 'stills'
New-Item -ItemType Directory -Force -Path $stills | Out-Null

# The status of the daemon that answers, or null.
function Status {
    $text = & $cli status --json 2>$null | Out-String
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($text)) { return $null }
    try { return $text | ConvertFrom-Json } catch { return $null }
}

if (Status) {
    throw 'a daifukud already runs in this session; this script records on a machine of its own'
}

# ffmpeg grabs the screen and encodes the results. Hosted runners carry it;
# anywhere else it is fetched.
if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) {
    Say 'ffmpeg is not installed, installing it'
    choco install ffmpeg -y --no-progress | Out-Null
    $env:PATH = "$env:PATH;$env:ProgramData\chocolatey\bin"
    if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) { throw 'ffmpeg is not available' }
}
Say ((ffmpeg -version | Select-Object -First 1) -join '')

# The screen: the asked size when the display offers it, else as it is.
$original = [Desk]::Current()
Say "screen is $([Desk]::GetSystemMetrics(0))x$([Desk]::GetSystemMetrics(1)); modes: $([Desk]::Modes())"
$changed = $false
$wanted = [Desk]::Find($Width, $Height)
if ($null -eq $wanted) {
    Say "the display offers no ${Width}x${Height} mode, recording at the current size"
} elseif ($original.dmPelsWidth -eq $Width -and $original.dmPelsHeight -eq $Height) {
    Say 'the screen already has the asked size'
} else {
    $result = [Desk]::Apply($wanted)
    if ($result -ne 0) {
        Say "changing the screen to ${Width}x${Height} failed with $result, recording at the current size"
    } else {
        $changed = $true
        Start-Sleep -Seconds 3
        Say "screen is now $([Desk]::GetSystemMetrics(0))x$([Desk]::GetSystemMetrics(1))"
    }
}

# One dark colour behind the terminals: the gaps between them compress to
# nothing, and nothing in the picture is anyone's wallpaper.
[Desk]::SetSysColors(1, @(1), @(0x1B1111)) | Out-Null
[Desk]::SetWallpaper(20, 0, '', 3) | Out-Null

$work = [Desk]::WorkArea()
# Even sizes: the video encoders want them.
$grabW = ($work.Right - $work.Left) -band -bnot 1
$grabH = ($work.Bottom - $work.Top) -band -bnot 1
Say "work area $($work.Left),$($work.Top) ${grabW}x${grabH}"

# The daemon's own config: no fleets and no hotkeys, so the demo goes on the
# primary monitor and nothing a runner holds can refuse a key.
$scratch = Join-Path ([IO.Path]::GetTempPath()) "daifuku-demo-$PID"
New-Item -ItemType Directory -Force -Path $scratch | Out-Null
$config = Join-Path $scratch 'daifuku.json'
'{"fleets":[],"hotkeys":{"next_waiting":null,"snap":null,"close":null}}' | Set-Content -Encoding utf8 $config

$raw = Join-Path $Out 'raw.mkv'
$statusLog = Join-Path $Out 'status.log'
$ffmpegLog = Join-Path $Out 'ffmpeg.log'
$daemon = $null
$grab = $null
try {
    $daemon = Start-Process -FilePath $daemonExe -ArgumentList '--config', "`"$config`"" -PassThru
    $deadline = (Get-Date).AddSeconds(15)
    while (-not (Status)) {
        if ($daemon.HasExited) { throw 'the daemon exited at once; another one holds the pipes' }
        if ((Get-Date) -gt $deadline) { throw 'the daemon did not answer within 15 s' }
        Start-Sleep -Milliseconds 100
    }
    $status = Status
    Say "daemon $($status.version) answers, elevated: $($status.elevated)"

    # The screen grab, lossless, for as long as asked. It runs on its own
    # clock from here, so the demo is opened into a recording already running.
    $grabArgs = @(
        '-y', '-hide_banner', '-loglevel', 'warning',
        '-f', 'gdigrab', '-framerate', "$Fps", '-draw_mouse', '0',
        '-offset_x', "$($work.Left)", '-offset_y', "$($work.Top)", '-video_size', "${grabW}x${grabH}",
        '-i', 'desktop', '-t', "$Seconds", '-c:v', 'ffv1', "`"$raw`""
    )
    $grab = Start-Process -FilePath ffmpeg -ArgumentList $grabArgs -NoNewWindow -PassThru -RedirectStandardError $ffmpegLog
    $started = Get-Date
    Start-Sleep -Milliseconds 1500
    if ($grab.HasExited) { throw "ffmpeg stopped at once: $(Get-Content -Raw $ffmpegLog)" }

    $opened = & $cli demo | Out-String
    Say $opened.Trim()
    if ($opened -notmatch 'opened demo') { throw 'the demo did not open' }

    # The script of the recording: at these seconds, bring the agent that has
    # waited longest to the front and approve it, as a person would.
    $approvals = @(12, 15, 30, 33)
    $approved = 0
    $nextStatus = Get-Date
    while (-not $grab.HasExited) {
        $elapsed = ((Get-Date) - $started).TotalSeconds
        if ((Get-Date) -ge $nextStatus) {
            $s = Status
            if ($s) {
                $states = ($s.agents | Group-Object state | ForEach-Object { "$($_.Count) $($_.Name.ToLower())" }) -join ', '
                ('{0,5:N1}s  {1}' -f $elapsed, $states) | Add-Content $statusLog
            }
            $nextStatus = (Get-Date).AddSeconds(1)
        }
        if ($approved -lt $approvals.Count -and $elapsed -ge $approvals[$approved]) {
            $approved++
            $before = [long][Desk]::GetForegroundWindow()
            $reply = (& $cli next | Out-String).Trim()
            Start-Sleep -Milliseconds 400
            $front = [long][Desk]::GetForegroundWindow()
            $sent = [Desk]::Enter()
            Say ('approval {0}: next said "{1}"; front {2:x} -> {3:x}; {4} of 2 key events sent' -f $approved, $reply, $before, $front, $sent)
        }
        Start-Sleep -Milliseconds 100
    }
    Say "recording ended after $(((Get-Date) - $started).TotalSeconds.ToString('N1')) s"
    if ($grab.ExitCode -ne 0) { throw "ffmpeg exited with $($grab.ExitCode): $(Get-Content -Raw $ffmpegLog)" }
    $grab = $null
    Get-Content $statusLog | Write-Output
}
finally {
    if ($grab -and -not $grab.HasExited) { $grab.Kill() }
    if ($daemon) {
        if (Status) {
            & $cli close demo | Out-Null
            & $cli stop | Out-Null
        }
        if (-not $daemon.WaitForExit(10000)) { $daemon.Kill() }
    }
    if ($changed) { [Desk]::Apply($original) | Out-Null }
    Remove-Item -Recurse -Force $scratch -ErrorAction SilentlyContinue
}

# The GIFs, at two widths, from one palette per run built on what changes
# between frames. A review video and a still every five seconds go with them.
foreach ($gifWidth in 1200, 960) {
    $gif = Join-Path $Out "demo-$gifWidth.gif"
    $filters = "fps=$Fps,scale=${gifWidth}:-2:flags=lanczos,split[a][b];[a]palettegen=max_colors=255:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle"
    ffmpeg -y -hide_banner -loglevel error -i $raw -vf $filters -loop 0 $gif
    if ($LASTEXITCODE -ne 0) { throw "the GIF at $gifWidth failed" }
    $frames = ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 $gif
    Say ("{0}: {1:N2} MB, {2} frames at {3} fps" -f (Split-Path -Leaf $gif), ((Get-Item $gif).Length / 1MB), $frames, $Fps)
}
$review = Join-Path $Out 'demo.mp4'
ffmpeg -y -hide_banner -loglevel error -i $raw -c:v libx264 -pix_fmt yuv420p -preset veryfast -crf 22 $review
if ($LASTEXITCODE -ne 0) { throw 'the review video failed' }
ffmpeg -y -hide_banner -loglevel error -i $raw -vf fps=1/5 (Join-Path $stills '%02d.png')
if ($LASTEXITCODE -ne 0) { throw 'the stills failed' }
Remove-Item $raw
Say "done: $Out"

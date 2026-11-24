# Captures a screenshot of the running control panel for design review.
# Usage: powershell -File scripts/screenshot.ps1 [-Out shot.png] [-Exe path] [-Page 0..5]
param(
    [string]$Out = "shot.png",
    [string]$Exe = "",
    [int]$Page = -1,
    [int]$WaitMs = 2500,
    [switch]$KeepOpen,
    # Control id to hover (moves the pointer to its centre before capturing).
    [int]$HoverId = 0,
    # Combobox control id to open before capturing.
    [int]$DropId = 0,
    # Number of forced full repaints to time (performance check, no screenshot).
    [int]$BenchPaints = 0,
    # "id:text" to write into a control before capturing (e.g. 119:hello).
    [string]$SetText = ""
)

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public class Win {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    public delegate bool EnumProc(IntPtr h, IntPtr p);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, IntPtr title);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] public static extern bool InvalidateRect(IntPtr h, IntPtr r, bool erase);
    [DllImport("user32.dll")] public static extern bool UpdateWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
"@

[Win]::SetProcessDPIAware() | Out-Null

function Find-Panel {
    $class = New-Object System.Text.StringBuilder 256
    $found = [IntPtr]::Zero
    $cb = [Win+EnumProc]{
        param($h, $p)
        [void][Win]::GetClassNameW($h, $class, 256)
        if ($class.ToString() -eq "SCRP.Desktop") { $script:found = $h; return $false }
        return $true
    }
    [void][Win]::EnumWindows($cb, [IntPtr]::Zero)
    return $script:found
}

$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
if (-not $Exe) { $Exe = Join-Path $root "target\release\star-citizen-rich-presence.exe" }
if (-not (Test-Path $Exe)) { throw "Missing $Exe" }

$script:found = [IntPtr]::Zero
$hwnd = Find-Panel
$proc = $null
if ($hwnd -eq [IntPtr]::Zero) {
    $proc = Start-Process -FilePath $Exe -PassThru
    $deadline = (Get-Date).AddMilliseconds($WaitMs)
    while ($hwnd -eq [IntPtr]::Zero -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 150
        $hwnd = Find-Panel
    }
}
if ($hwnd -eq [IntPtr]::Zero) { throw "Control panel window not found" }

[Win]::ShowWindow($hwnd, 5) | Out-Null
[Win]::SetForegroundWindow($hwnd) | Out-Null
Start-Sleep -Milliseconds 500

if ($Page -ge 0) {
    [void][Win]::SendMessageW($hwnd, 0x0111, [IntPtr](300 + $Page), [IntPtr]::Zero)
    Start-Sleep -Milliseconds 500
}

if ($HoverId -gt 0) {
    # Bring the panel above the terminal so the pointer really lands on the control.
    [void][Win]::SetWindowPos($hwnd, [IntPtr](-1), 0, 0, 0, 0, 0x0001 -bor 0x0002 -bor 0x0040)
    Start-Sleep -Milliseconds 300
    $target = [Win]::GetDlgItem($hwnd, $HoverId)
    if ($target -ne [IntPtr]::Zero) {
        $tr = New-Object Win+RECT
        [Win]::GetWindowRect($target, [ref]$tr) | Out-Null
        [void][Win]::SetCursorPos([int](($tr.Left + $tr.Right) / 2), [int](($tr.Top + $tr.Bottom) / 2))
        # Synthetic WM_MOUSEMOVE so hover styling is testable without focus games.
        $w = $tr.Right - $tr.Left
        $h = $tr.Bottom - $tr.Top
        $lp = [IntPtr](($h / 2) * 65536 + ($w / 2))
        [void][Win]::SendMessageW($target, 0x0200, [IntPtr]::Zero, $lp)
        Start-Sleep -Milliseconds 400
    }
}

if ($DropId -gt 0) {
    $target = [Win]::GetDlgItem($hwnd, $DropId)
    if ($target -ne [IntPtr]::Zero) {
        [void][Win]::SendMessageW($target, 0x014F, [IntPtr]1, [IntPtr]::Zero)
        Start-Sleep -Milliseconds 400
    }
}

if ($BenchPaints -gt 0) {
    # Warm up, then time full synchronous repaints through UpdateWindow.
    for ($i = 0; $i -lt 10; $i++) {
        [void][Win]::InvalidateRect($hwnd, [IntPtr]::Zero, $false)
        [void][Win]::UpdateWindow($hwnd)
    }
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    for ($i = 0; $i -lt $BenchPaints; $i++) {
        [void][Win]::InvalidateRect($hwnd, [IntPtr]::Zero, $false)
        [void][Win]::UpdateWindow($hwnd)
    }
    $sw.Stop()
    $per = [math]::Round($sw.Elapsed.TotalMilliseconds / $BenchPaints, 2)
    Write-Output ("{0}: {1} repaints in {2} ms = {3} ms per paint" -f $hwnd, $BenchPaints, [math]::Round($sw.Elapsed.TotalMilliseconds, 1), $per)
    if (-not $KeepOpen -and $proc) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
    exit 0
}

if ($SetText) {
    $parts = $SetText.Split(":", 2)
    $target = [Win]::GetDlgItem($hwnd, [int]$parts[0])
    if ($target -ne [IntPtr]::Zero -and $parts.Count -eq 2) {
        [void][Win]::SendMessageW($target, 0x000C, [IntPtr]::Zero, [System.Runtime.InteropServices.Marshal]::StringToHGlobalUni($parts[1]))
        Start-Sleep -Milliseconds 300
    }
}

$r = New-Object Win+RECT
[Win]::GetWindowRect($hwnd, [ref]$r) | Out-Null
$w = $r.Right - $r.Left
$h = $r.Bottom - $r.Top
$bmp = New-Object System.Drawing.Bitmap($w, $h)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
[void][Win]::PrintWindow($hwnd, $hdc, 2)
$g.ReleaseHdc($hdc)
$g.Dispose()
$path = if ([System.IO.Path]::IsPathRooted($Out)) { $Out } else { Join-Path $root $Out }
$bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()

if (-not $KeepOpen -and $proc) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
Write-Output "$path ($w x $h)"

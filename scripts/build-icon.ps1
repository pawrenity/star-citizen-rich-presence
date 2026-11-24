# Builds assets/scrp.ico from assets/scrp-logo.svg.
#
# Renders the SVG with headless Chrome at every icon size (crisp per size, no
# resampling mush), then packs the PNGs into a single .ico (PNG-compressed
# entries, supported by Windows Vista and later).
#
# Usage: powershell -ExecutionPolicy Bypass -File scripts/build-icon.ps1
param(
    [string]$Svg = "",
    [string]$Ico = "",
    [string]$Preview = "",
    [int[]]$Sizes = @(16, 20, 24, 32, 40, 48, 64, 128, 256)
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
if (-not $Svg) { $Svg = Join-Path $root "assets\scrp-logo.svg" }
if (-not $Ico) { $Ico = Join-Path $root "assets\scrp.ico" }

$chrome = @(
    "$env:ProgramFiles\Google\Chrome\Application\chrome.exe",
    "${env:ProgramFiles(x86)}\Google\Chrome\Application\chrome.exe",
    "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe",
    "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $chrome) { throw "Chrome or Edge is required to render the SVG" }
if (-not (Test-Path $Svg)) { throw "Missing $Svg" }

$svgText = [System.IO.File]::ReadAllText($Svg, [System.Text.Encoding]::UTF8)
# Drop the XML prolog and any DOCTYPE so the markup can be inlined in HTML.
$svgText = $svgText -replace '^\s*<\?xml[^>]*\?>', ''
$svgText = $svgText -replace '<!DOCTYPE[^>]*>', ''

$work = Join-Path $env:TEMP ("scrp-icon-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $work | Out-Null

function Render-Png([int]$size, [string]$path) {
    # The SVG fills a square page with no margins and a transparent backdrop.
    $html = @"
<!doctype html><html><head><meta charset="utf-8"><style>
html,body{margin:0;padding:0;background:transparent;overflow:hidden}
svg{display:block;width:${size}px;height:${size}px}
</style></head><body>$svgText</body></html>
"@
    $page = Join-Path $work "page-$size.html"
    [System.IO.File]::WriteAllText($page, $html, (New-Object System.Text.UTF8Encoding($false)))
    $uri = ([System.Uri]$page).AbsoluteUri
    $log = Join-Path $work "chrome-$size.log"
    $err = Join-Path $work "chrome-$size.err"
    $arguments = @(
        "--headless=new", "--disable-gpu", "--hide-scrollbars",
        "--force-device-scale-factor=1", "--default-background-color=00000000",
        "--window-size=$size,$size", "--screenshot=$path", $uri
    )
    $process = Start-Process -FilePath $chrome -ArgumentList $arguments -Wait -PassThru `
        -WindowStyle Hidden -RedirectStandardError $err -RedirectStandardOutput $log
    if (-not (Test-Path $path)) {
        $detail = if (Test-Path $err) { Get-Content $err -Raw } else { "" }
        throw "Chrome did not render $size px (exit $($process.ExitCode)): $detail"
    }
}

$rendered = @()
foreach ($size in $Sizes) {
    $path = Join-Path $work "icon-$size.png"
    Render-Png -size $size -path $path
    $bytes = [System.IO.File]::ReadAllBytes($path)
    if ($bytes.Length -lt 100) { throw "Rendered $size px is empty" }
    $rendered += [pscustomobject]@{ Size = $size; Bytes = $bytes; Path = $path }
    Write-Output ("rendered {0}x{0} ({1} bytes)" -f $size, $bytes.Length)
}

Add-Type -AssemblyName System.Drawing

# Classic 32-bit BMP entry: header, bottom-up BGRA rows, then the AND mask.
# Windows, Explorer and Inno Setup all read these without PNG support.
function ConvertTo-BmpEntry([string]$path, [int]$size) {
    $bmp = [System.Drawing.Bitmap]::new($path)
    if ($bmp.Width -ne $size -or $bmp.Height -ne $size) {
        $bmp.Dispose()
        throw "Rendered $path is $($bmp.Width)x$($bmp.Height), expected $size"
    }
    $rect = [System.Drawing.Rectangle]::new(0, 0, $size, $size)
    $data = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly,
        [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $stride = $data.Stride
    $pixels = New-Object byte[] ($stride * $size)
    [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $pixels, 0, $pixels.Length)
    $bmp.UnlockBits($data)
    $bmp.Dispose()

    $maskStride = [int]([math]::Floor(($size + 31) / 32) * 4)
    $imageSize = $size * $size * 4 + $maskStride * $size

    $entry = New-Object System.IO.MemoryStream
    $w = New-Object System.IO.BinaryWriter($entry)
    $w.Write([uint32]40)                    # biSize
    $w.Write([int32]$size)                  # biWidth
    $w.Write([int32]($size * 2))            # biHeight (XOR + AND)
    $w.Write([uint16]1)                     # biPlanes
    $w.Write([uint16]32)                    # biBitCount
    $w.Write([uint32]0)                     # biCompression: BI_RGB
    $w.Write([uint32]$imageSize)
    $w.Write([int32]0); $w.Write([int32]0)  # pixels per metre
    $w.Write([uint32]0); $w.Write([uint32]0)# palette
    # BGRA rows, bottom-up.
    for ($y = $size - 1; $y -ge 0; $y--) {
        $w.Write($pixels, $y * $stride, $size * 4)
    }
    # AND mask: all zero (the alpha channel carries transparency).
    $maskRow = New-Object byte[] $maskStride
    for ($y = 0; $y -lt $size; $y++) { $w.Write($maskRow) }
    $w.Flush()
    $out = $entry.ToArray()
    $w.Dispose(); $entry.Dispose()
    return $out
}

# Every entry is a 32-bit BMP: Explorer, the taskbar, Inno Setup and .NET all
# read these, whereas PNG entries are rejected by older loaders.
$entries = @()
foreach ($item in $rendered) {
    $entries += [pscustomobject]@{ Size = $item.Size; Bytes = (ConvertTo-BmpEntry $item.Path $item.Size) }
}

# ICONDIR + ICONDIRENTRY + payloads.
$stream = New-Object System.IO.MemoryStream
$writer = New-Object System.IO.BinaryWriter($stream)
$writer.Write([uint16]0)                  # reserved
$writer.Write([uint16]1)                  # type: icon
$writer.Write([uint16]$entries.Count)     # image count
$offset = 6 + 16 * $entries.Count
foreach ($entry in $entries) {
    $dimension = $entry.Size
    if ($dimension -ge 256) { $dimension = 0 }
    $writer.Write([byte]$dimension)
    $writer.Write([byte]$dimension)
    $writer.Write([byte]0)                # palette count
    $writer.Write([byte]0)                # reserved
    $writer.Write([uint16]1)              # colour planes
    $writer.Write([uint16]32)             # bits per pixel
    $writer.Write([uint32]$entry.Bytes.Length)
    $writer.Write([uint32]$offset)
    $offset += $entry.Bytes.Length
}
foreach ($entry in $entries) { $writer.Write([byte[]]$entry.Bytes) }
$writer.Flush()
[System.IO.File]::WriteAllBytes($Ico, $stream.ToArray())
$writer.Dispose()
$stream.Dispose()
Write-Output ("wrote {0} ({1} bytes, {2} sizes)" -f $Ico, (Get-Item $Ico).Length, $entries.Count)

if ($Preview) {
    # Contact sheet: every size at 4x for eyeballing small-size legibility.
    Add-Type -AssemblyName System.Drawing
    $pad = 24
    $sheetW = (($Sizes | ForEach-Object { $_ * 4 + $pad }) | Measure-Object -Sum).Sum
    $sheetH = ($Sizes | ForEach-Object { $_ * 4 } | Measure-Object -Maximum).Maximum
    $sheet = [System.Drawing.Bitmap]::new([int]$sheetW, [int]($sheetH + 2 * $pad))
    $g = [System.Drawing.Graphics]::FromImage($sheet)
    $g.Clear([System.Drawing.Color]::FromArgb(255, 32, 32, 32))
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
    $x = 0
    foreach ($size in $Sizes) {
        $img = [System.Drawing.Image]::FromFile((Join-Path $work "icon-$size.png"))
        $g.DrawImage($img, $x + $pad / 2, $pad, $size * 4, $size * 4)
        $img.Dispose()
        $x += $size * 4 + $pad
    }
    $g.Dispose()
    $sheet.Save($Preview, [System.Drawing.Imaging.ImageFormat]::Png)
    $sheet.Dispose()
    Write-Output "preview: $Preview"
}

Remove-Item -Recurse -Force $work

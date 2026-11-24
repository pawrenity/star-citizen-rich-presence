# Verifies an .ico by loading each requested size back through System.Drawing
# and writing a 4x contact sheet. Catches malformed BMP entries.
param(
    [string]$Ico,
    [string]$Out,
    [int[]]$Sizes = @(16, 20, 24, 32, 40, 48, 64, 128, 256)
)

Add-Type -AssemblyName System.Drawing
if (-not $Ico -or -not $Out) { throw "Usage: verify-icon.ps1 -Ico x.ico -Out sheet.png" }
if (-not (Test-Path $Ico)) { throw "Missing $Ico" }

$pad = 24
$sheetW = (($Sizes | ForEach-Object { $_ * 4 + $pad }) | Measure-Object -Sum).Sum
$sheetH = ($Sizes | ForEach-Object { $_ * 4 } | Measure-Object -Maximum).Maximum
$sheet = [System.Drawing.Bitmap]::new([int]$sheetW, [int]($sheetH + 2 * $pad))
$g = [System.Drawing.Graphics]::FromImage($sheet)
$g.Clear([System.Drawing.Color]::FromArgb(255, 32, 32, 32))
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor

$x = 0
foreach ($size in $Sizes) {
    $icon = [System.Drawing.Icon]::new((Resolve-Path $Ico).Path, $size, $size)
    $bmp = $icon.ToBitmap()
    if ($bmp.Width -ne $size -or $bmp.Height -ne $size) {
        Write-Output ("WARNING {0}px request returned {1}x{2}" -f $size, $bmp.Width, $bmp.Height)
    }
    $g.DrawImage($bmp, $x + $pad / 2, $pad, $size * 4, $size * 4)
    Write-Output ("{0}px ok ({1}x{2})" -f $size, $bmp.Width, $bmp.Height)
    $bmp.Dispose()
    $icon.Dispose()
    $x += $size * 4 + $pad
}

$g.Dispose()
$sheet.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$sheet.Dispose()
Write-Output "sheet: $Out"

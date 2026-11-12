<#
.SYNOPSIS
  Regenerate packaging\windows\systui.ico (multi-size, PNG-compressed) with System.Drawing.
.DESCRIPTION
  Draws a dark rounded tile with three usage bars and a baseline - a nod to a system monitor.
  Run only when changing the artwork; the generated .ico is committed.
#>
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$out = Join-Path (Split-Path -Parent $PSScriptRoot) 'packaging\windows\systui.ico'

function New-Frame([int]$s) {
    $bmp = New-Object System.Drawing.Bitmap $s, $s, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'; $g.Clear([System.Drawing.Color]::Transparent)
    $r = [single]($s * 0.22); $d = $r * 2; $w = $s - 1
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.AddArc(0, 0, $d, $d, 180, 90); $path.AddArc($w - $d, 0, $d, $d, 270, 90)
    $path.AddArc($w - $d, $w - $d, $d, $d, 0, 90); $path.AddArc(0, $w - $d, $d, $d, 90, 90); $path.CloseFigure()
    $g.FillPath((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 22, 27, 34))), $path)
    $cols = @([System.Drawing.Color]::FromArgb(255, 63, 185, 80), [System.Drawing.Color]::FromArgb(255, 210, 153, 34), [System.Drawing.Color]::FromArgb(255, 88, 166, 255))
    $heights = @(0.38, 0.62, 0.48)
    $bw = $s * 0.16; $gap = $s * 0.07; $x0 = ($s - (3 * $bw + 2 * $gap)) / 2; $base = $s * 0.76
    for ($i = 0; $i -lt 3; $i++) {
        $h = $s * $heights[$i]
        $g.FillRectangle((New-Object System.Drawing.SolidBrush $cols[$i]), [single]($x0 + $i * ($bw + $gap)), [single]($base - $h), [single]$bw, [single]$h)
    }
    $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 139, 148, 158)), ([single][Math]::Max(1, $s * 0.04))
    $g.DrawLine($pen, [single]($s * 0.18), [single]($base + $s * 0.06), [single]($s * 0.82), [single]($base + $s * 0.06))
    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream; $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
    , $ms.ToArray()
}

$sizes = 16, 24, 32, 48, 64, 128, 256
$frames = foreach ($s in $sizes) { , (New-Frame $s) }
$fs = [System.IO.File]::Create($out); $bw = New-Object System.IO.BinaryWriter $fs
$bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $s = $sizes[$i]; $b = if ($s -ge 256) { 0 } else { $s }
    $bw.Write([byte]$b); $bw.Write([byte]$b); $bw.Write([byte]0); $bw.Write([byte]0)
    $bw.Write([uint16]1); $bw.Write([uint16]32); $bw.Write([uint32]$frames[$i].Length); $bw.Write([uint32]$offset)
    $offset += $frames[$i].Length
}
foreach ($f in $frames) { $bw.Write($f) }
$bw.Close(); $fs.Close()
Write-Host "Wrote $out"

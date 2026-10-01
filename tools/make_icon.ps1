# Genera los iconos del navegador a partir del original (PNG cuadrado con
# fondo transparente):
#   assets/icon/michi.ico  - icono del .exe y de la ventana (todos los
#                                tamanos que pide Windows, cada uno en PNG)
#   assets/icon/michi-256.png - logo de la interfaz (Ajustes > Acerca de)
#
# Uso: powershell -File tools/make_icon.ps1 [original.png]
param([string]$Origen = "$PSScriptRoot\..\assets\icon\michi.png")
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$dir = Split-Path -Parent (Resolve-Path $Origen)
$src = [System.Drawing.Bitmap]::FromFile((Resolve-Path $Origen))

function Escalar([int]$lado) {
    $bmp = New-Object System.Drawing.Bitmap $lado, $lado, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)
    $g.DrawImage($src, 0, 0, $lado, $lado)
    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    return ,$ms.ToArray()
}

# .ico: encabezado, una entrada por tamano y despues los PNG.
$lados = 16, 20, 24, 32, 40, 48, 64, 96, 128, 256
$pngs = $lados | ForEach-Object { ,(Escalar $_) }
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $out
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$lados.Count)
$offset = 6 + 16 * $lados.Count
for ($i = 0; $i -lt $lados.Count; $i++) {
    $l = $lados[$i]; $data = $pngs[$i]
    $b = if ($l -ge 256) { 0 } else { $l }
    $w.Write([byte]$b); $w.Write([byte]$b); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$data.Length); $w.Write([uint32]$offset)
    $offset += $data.Length
}
foreach ($data in $pngs) { $w.Write($data) }
$w.Flush()
[IO.File]::WriteAllBytes((Join-Path $dir 'michi.ico'), $out.ToArray())
[IO.File]::WriteAllBytes((Join-Path $dir 'michi-256.png'), $pngs[$lados.IndexOf(256)])
$src.Dispose()
"Listo: $dir\michi.ico ($($lados -join ', ') px) y michi-256.png"

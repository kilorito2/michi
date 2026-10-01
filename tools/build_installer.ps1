# Genera dist\Michi-Setup-<version>.exe: el instalador (installer/) con
# los archivos del navegador pegados al final (ver installer/src/payload.rs):
#
#   [instalador][archivo 1]...[archivo N][indice JSON][largo del indice: u64 LE]["MICHISET"]
#
# Uso: powershell -ExecutionPolicy Bypass -File tools\build_installer.ps1
$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\.."
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
"== Michi $version"

"== Compilando el navegador y el instalador (release)"
cargo build --release -p michi -p michi-setup
if ($LASTEXITCODE -ne 0) { throw "fallo la compilacion" }

# Lo que se instala: el .exe y los fondos (solo las versiones optimizadas).
$files = @(@{ Path = 'michi.exe'; Source = Join-Path $root 'target\release\michi.exe' })
Get-ChildItem (Join-Path $root 'assets\wallpapers\optimized') -File | Sort-Object Name | ForEach-Object {
    $files += @{ Path = "wallpapers/$($_.Name)"; Source = $_.FullName }
}

$stub = Join-Path $root 'target\release\michi-setup.exe'
New-Item -ItemType Directory -Force (Join-Path $root 'dist') | Out-Null
$out = Join-Path $root "dist\Michi-Setup-$version.exe"
$stubLen = (Get-Item $stub).Length

$dest = [IO.File]::Create($out)
try {
    $src = [IO.File]::OpenRead($stub); $src.CopyTo($dest); $src.Close()
    $index = @()
    foreach ($f in $files) {
        $offset = $dest.Position
        $src = [IO.File]::OpenRead($f.Source); $src.CopyTo($dest); $src.Close()
        $hash = (Get-FileHash -Algorithm SHA256 $f.Source).Hash.ToLower()
        $len = (Get-Item $f.Source).Length
        $index += [ordered]@{ path = $f.Path; offset = $offset; len = $len; sha256 = $hash }
        "   + {0,-42} {1,8:N0} KB" -f $f.Path, ($len / 1KB)
    }
    $json = [ordered]@{ version = $version; stub_len = $stubLen; files = $index } | ConvertTo-Json -Depth 4 -Compress
    $bytes = [Text.Encoding]::UTF8.GetBytes($json)
    $dest.Write($bytes, 0, $bytes.Length)
    $dest.Write([BitConverter]::GetBytes([uint64]$bytes.Length), 0, 8)
    $magic = [Text.Encoding]::ASCII.GetBytes('MICHISET')
    $dest.Write($magic, 0, 8)
} finally {
    $dest.Close()
}
"== Listo: $out ({0:N1} MB)" -f ((Get-Item $out).Length / 1MB)

# SPDX-License-Identifier: AGPL-3.0-or-later
# Packages the Bamburino service: bamburino-<version>-win64.zip + .sha256 (sha256sum format).
# The zip root holds exactly: bamburino.exe, install.ps1 (= ci/service-install.ps1), LICENSE, NOTICE.md, README.md.
# Usage: pwsh ci/package-service.ps1 -Exe service/target/release/bamburino.exe -Out dist [-Version 0.1.0]
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Exe,
    [Parameter(Mandatory)][string]$Out,
    [string]$Version
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) { throw "exe not found: $Exe" }
if (-not $Version) {
    $toml = Join-Path $repo 'service/Cargo.toml'
    if (-not (Test-Path -LiteralPath $toml)) { throw 'no -Version given and service/Cargo.toml not found' }
    $m = Select-String -LiteralPath $toml -Pattern '^\s*version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if (-not $m) { throw 'no -Version given and no version = "..." in service/Cargo.toml' }
    $Version = $m.Matches[0].Groups[1].Value
}
if ($Version -notmatch '^[0-9A-Za-z][0-9A-Za-z.+-]*$') { throw "invalid version: $Version" }

$sources = [ordered]@{
    'bamburino.exe' = (Resolve-Path -LiteralPath $Exe).Path
    'install.ps1'   = Join-Path $repo 'ci/service-install.ps1'
    'LICENSE'       = Join-Path $repo 'LICENSE'
    'NOTICE.md'     = Join-Path $repo 'NOTICE.md'
    'README.md'     = Join-Path $repo 'README.md'
}
foreach ($k in $sources.Keys) { if (-not (Test-Path -LiteralPath $sources[$k])) { throw "missing package input: $($sources[$k])" } }

New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Out = (Resolve-Path -LiteralPath $Out).Path
$name = "bamburino-$Version-win64.zip"
$zip = Join-Path $Out $name
$stage = Join-Path ([IO.Path]::GetTempPath()) ("bamburino-pkg-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    foreach ($k in $sources.Keys) { Copy-Item -LiteralPath $sources[$k] -Destination (Join-Path $stage $k) }
    if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}
$hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLower()
Set-Content -LiteralPath "$zip.sha256" -Value "$hash  $name" -Encoding ascii -NoNewline
Write-Host "packaged $zip"
Write-Host "sha256   $hash"

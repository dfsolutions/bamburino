<#
.SYNOPSIS
    Prints the driver patch_id; with -Write also stores it in driver/upstream.toml (driver.patch_id).
.DESCRIPTION
    patch_id = first 12 lowercase hex chars of SHA-256 over the raw bytes of every driver/patches/*.diff,
    concatenated in ordinal file-name order. Shared contract with plan B (build.rs) and plan C: do not change.
#>
[CmdletBinding()]
param([switch]$Write)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot '..\tools\Upstream.psm1') -Force

$dir = Join-Path $PSScriptRoot 'patches'
$files = @(Get-ChildItem -LiteralPath $dir -Filter '*.diff' -File)
# Ordinal (byte-wise) name order, independent of the current culture.
$names = [string[]]@($files | ForEach-Object Name)
[Array]::Sort($names, [StringComparer]::Ordinal)
if ($names.Count -eq 0) { throw "no *.diff files in $dir" }

$sha = [System.Security.Cryptography.SHA256]::Create()
try {
    foreach ($n in $names) {
        $bytes = [System.IO.File]::ReadAllBytes((Join-Path $dir $n))
        [void]$sha.TransformBlock($bytes, 0, $bytes.Length, $null, 0)
    }
    [void]$sha.TransformFinalBlock([byte[]]::new(0), 0, 0)
    $id = (-join ($sha.Hash | ForEach-Object { $_.ToString('x2') })).Substring(0, 12)
} finally { $sha.Dispose() }

if ($Write) { Set-UpstreamValue -Key 'driver.patch_id' -Value $id }
Write-Output $id

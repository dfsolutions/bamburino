# Integration test for driver/build.ps1 -Step deps (Task 6): the dependency prefix exists under
# <BuildRoot>\deps-out\usr\local and the cache sentinel holds the git tree SHA-1 of deps/ at studio.tag_commit.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module "$PSScriptRoot/../Upstream.psm1" -Force
$u = Get-Upstream
$tagCommit = $u['studio.tag_commit']
if ($tagCommit -notmatch '^[0-9a-f]{40}$') { throw "studio.tag_commit is not 40 hex: '$tagCommit'" }
$root = Get-BuildRoot
$clone = Join-Path $root 'BambuStudio'
if (-not (Test-Path -LiteralPath (Join-Path $clone '.git'))) { throw "clone missing at ${clone}: run driver/build.ps1 -Step fetch first" }
$out = Join-Path $root 'deps-out'
$prefix = Join-Path $out 'usr\local'

# Boost's own CMake install uses the versioned layout on Windows (include\boost-1_84\boost), so accept both forms.
$inc = Join-Path $prefix 'include'
$boost = @(Join-Path $inc 'boost\version.hpp')
if (Test-Path -LiteralPath $inc) {
    $boost += @(Get-ChildItem -LiteralPath $inc -Directory -Filter 'boost-*' | ForEach-Object { Join-Path $_.FullName 'boost\version.hpp' })
}
if (-not @($boost | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf })) { throw "missing $inc\boost\version.hpp (or $inc\boost-*\boost\version.hpp)" }
$ssl = Join-Path $inc 'openssl\ssl.h'
if (-not (Test-Path -LiteralPath $ssl -PathType Leaf)) { throw "missing $ssl" }
$libDir = Join-Path $prefix 'lib'
$wx = @()
if (Test-Path -LiteralPath $libDir) {
    $wx = @(Get-ChildItem -LiteralPath $libDir -Recurse -File -Filter '*.lib' |
        Where-Object { $_.Name -like 'wxmsw*u_core.lib' -or $_.Name -like 'wx*core*.lib' })
}
if ($wx.Count -lt 1) { throw "no wxWidgets core library (wxmsw*u_core.lib or wx*core*.lib) under $libDir" }

$want = ((& git -C $clone rev-parse "${tagCommit}:deps") | Select-Object -First 1)
if ($LASTEXITCODE -ne 0 -or -not $want) { throw "git rev-parse ${tagCommit}:deps failed" }
$want = $want.Trim()
$keyFile = Join-Path $out '.bamburino-deps-key'
if (-not (Test-Path -LiteralPath $keyFile -PathType Leaf)) { throw "sentinel missing: $keyFile" }
$key = ([System.IO.File]::ReadAllText($keyFile)).Trim()
if ($key -notmatch '^[0-9a-f]{40}$') { throw "sentinel is not 40 hex: '$key'" }
if ($key -ne $want) { throw "sentinel $key does not match deps tree $want" }

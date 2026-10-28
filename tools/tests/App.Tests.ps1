# Integration test for driver/build.ps1 -Step app, -Step test and -Step package (Task 8): the installed app exists with
# the upstream version and carries the build identity, the last MCP standalone test run passed, and the portable zip
# exists with a matching .sha256.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module "$PSScriptRoot/../Upstream.psm1" -Force
$u = Get-Upstream
$patchId = $u['driver.patch_id']
$prefix = $u['driver.build_suffix_prefix']
if ($patchId -notmatch '^[0-9a-f]{12}$') { throw "driver.patch_id is not 12 hex: '$patchId'" }
$root = Get-BuildRoot

# App: installed executable with the upstream version (SLIC3R_VERSION 02.08.04.57).
# The product version is read from the fixed VERSIONINFO block (PRODUCTVERSION 02,08,04,57 -> 2.8.4.57). The
# ProductVersion *string* of upstream's BambuStudio.rc.in is "@SLIC3R_BUILD_ID@", which no public upstream CMake
# file defines, so it is empty in any source build; it is accepted when empty and checked when set.
$install = Join-Path $root 'install-dir'
$exe = Join-Path $install 'bambu-studio.exe'
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw "missing $exe (run driver/build.ps1 -Step app)" }
$vi = (Get-Item -LiteralPath $exe).VersionInfo
$fixed = "$($vi.ProductMajorPart).$($vi.ProductMinorPart).$($vi.ProductBuildPart).$($vi.ProductPrivatePart)"
$wantFixed = (($u['studio.version'] -split '\.') | ForEach-Object { [int]$_ }) -join '.'
if ($fixed -ne $wantFixed -or -not $fixed.StartsWith('2.8.4')) { throw "$exe product version is $fixed, expected $wantFixed (2.8.4*)" }
if (-not $vi.FileVersion -or -not $vi.FileVersion.StartsWith('02.08.04')) { throw "$exe FileVersion is '$($vi.FileVersion)', expected 02.08.04*" }
$pv = $vi.ProductVersion
if ($pv -and -not ($pv.StartsWith('2.8.4') -or $pv.StartsWith('02.08.04'))) { throw "$exe ProductVersion is '$pv', expected 2.8.4* or 02.08.04*" }

# Identity: the installed binaries embed the build suffix of the current patch_id (not "unknown", not a stale one).
$suffix = "$prefix.$patchId"
$latin1 = [System.Text.Encoding]::Latin1
$carrier = @(Get-ChildItem -LiteralPath $install -File | Where-Object { $_.Extension -in '.exe', '.dll' } |
    Where-Object { $latin1.GetString([System.IO.File]::ReadAllBytes($_.FullName)).Contains($suffix) })
if ($carrier.Count -lt 1) { throw "no .exe/.dll in $install embeds '$suffix'" }

# MCP standalone tests: the last -Step test log shows ctest at 100% and test_legacy_contract.py exit 0.
$logDir = Join-Path $root 'logs'
$testLog = if (Test-Path -LiteralPath $logDir) { Get-ChildItem -LiteralPath $logDir -Filter 'test-*.log' -File | Sort-Object Name | Select-Object -Last 1 } else { $null }
if (-not $testLog) { throw "no test-*.log in $logDir (run driver/build.ps1 -Step test)" }
if (-not (Select-String -LiteralPath $testLog.FullName -SimpleMatch -Quiet -Pattern '100% tests passed')) { throw "$($testLog.Name) does not report '100% tests passed'" }
if (-not (Select-String -LiteralPath $testLog.FullName -SimpleMatch -Quiet -Pattern '== python unittest exit 0')) { throw "$($testLog.Name) does not report '== python unittest exit 0' (test_legacy_contract.py)" }

# Package: zip named after the upstream version and patch_id, with a sha256sum-style sidecar that matches.
$zip = Join-Path $root "artifacts\BambuStudio-bamburino-$($u['studio.version'])-$patchId-win64.zip"
if (-not (Test-Path -LiteralPath $zip -PathType Leaf)) { throw "missing $zip (run driver/build.ps1 -Step package)" }
$side = "$zip.sha256"
if (-not (Test-Path -LiteralPath $side -PathType Leaf)) { throw "missing $side" }
$recorded = (([System.IO.File]::ReadAllText($side).Trim() -split '\s+')[0]).ToLowerInvariant()
$actual = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
if ($recorded -ne $actual) { throw "$side says $recorded, Get-FileHash says $actual" }

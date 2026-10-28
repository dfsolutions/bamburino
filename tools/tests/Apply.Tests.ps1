# Integration test for driver/build.ps1 -Step patch on the real clone at <BuildRoot>\BambuStudio (Task 3 fetch).
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module "$PSScriptRoot/../Upstream.psm1" -Force
$build = (Resolve-Path "$PSScriptRoot/../../driver/build.ps1").Path
$u = Get-Upstream
$tagCommit = $u['studio.tag_commit']
$clone = Join-Path (Get-BuildRoot) 'BambuStudio'
if (-not (Test-Path -LiteralPath (Join-Path $clone '.git'))) { throw "clone missing at ${clone}: run driver/build.ps1 -Step fetch first" }

function Invoke-PatchStep {
    $out = & pwsh -NoProfile -NonInteractive -File $build -Step patch 2>&1
    return [pscustomobject]@{ Code = $LASTEXITCODE; Out = ($out -join "`n") }
}
function Get-Porcelain { return ((& git -C $clone status --porcelain --untracked-files=all) -join "`n") }

$patchCount = @(Get-ChildItem "$PSScriptRoot/../../driver/patches" -Filter '*.diff' -File).Count
# Distinct paths across all patches: a later patch may touch a file that an earlier one already touched.
$touched = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::Ordinal)
foreach ($d in Get-ChildItem "$PSScriptRoot/../../driver/patches" -Filter '*.diff' -File) {
    foreach ($m in (Select-String -LiteralPath $d.FullName -Pattern '^diff --git a/(\S+) b/').Matches) { [void]$touched.Add($m.Groups[1].Value) }
}
$expectedFiles = $touched.Count
if ($patchCount -eq 1 -and $expectedFiles -ne 41) { throw "patch 0001 alone should touch 41 files, counted $expectedFiles" }
if ((Test-Path "$PSScriptRoot/../../driver/patches/0100-build-identity.diff") -and $patchCount -eq 2 -and $expectedFiles -ne 44) {
    throw "patches 0001 + 0100 should touch 44 distinct files (41 + CMakeLists.txt, AboutDialog.cpp, bamburino_identity.h.in), counted $expectedFiles"
}
if ((Test-Path "$PSScriptRoot/../../driver/patches/0120-expose-undo-snapshot.diff") -and $patchCount -eq 3 -and $expectedFiles -ne 44) {
    throw "patches 0001 + 0100 + 0120 should touch 44 distinct files (0120 only touches McpTools.cpp, already in 0001), counted $expectedFiles"
}

# Two consecutive runs: both succeed and leave the same tree.
$r1 = Invoke-PatchStep
if ($r1.Code -ne 0) { throw "first -Step patch exit $($r1.Code): $($r1.Out)" }
$s1 = Get-Porcelain
$r2 = Invoke-PatchStep
if ($r2.Code -ne 0) { throw "second -Step patch exit $($r2.Code): $($r2.Out)" }
$s2 = Get-Porcelain
if ($s1 -ne $s2) { throw "git status --porcelain differs between runs:`n--- run 1`n$s1`n--- run 2`n$s2" }
if (-not $s1) { throw 'clone shows no changes after -Step patch' }

if (-not (Test-Path -LiteralPath (Join-Path $clone 'src/slic3r/Utils/McpServer.cpp'))) { throw 'McpServer.cpp not present after patch' }
$head = (& git -C $clone rev-parse HEAD).Trim()
if ($head -ne $tagCommit) { throw "HEAD $head, expected $tagCommit" }
$short = (& git -C $clone diff --shortstat $tagCommit) -join ''
if ($short -notmatch '^\s*(\d+) files? changed') { throw "unexpected shortstat: $short" }
if ([int]$Matches[1] -ne $expectedFiles) { throw "diff --shortstat reports $($Matches[1]) files, expected $expectedFiles ($short)" }

# A patch_id mismatch exits 21 without touching the clone.
$tmpToml = Join-Path ([System.IO.Path]::GetTempPath()) ("bamburino-upstream-" + [guid]::NewGuid().ToString('N') + '.toml')
Copy-Item -LiteralPath (Resolve-Path "$PSScriptRoot/../../driver/upstream.toml").Path -Destination $tmpToml
$prev = $env:BAMBURINO_UPSTREAM_TOML
try {
    Set-UpstreamValue -Path $tmpToml -Key 'driver.patch_id' -Value '000000000000'
    $env:BAMBURINO_UPSTREAM_TOML = $tmpToml
    $r3 = Invoke-PatchStep
} finally {
    $env:BAMBURINO_UPSTREAM_TOML = $prev
    Remove-Item -LiteralPath $tmpToml -Force -ErrorAction SilentlyContinue
}
if ($r3.Code -ne 21) { throw "patch_id mismatch: expected exit 21, got $($r3.Code): $($r3.Out)" }
$head3 = (& git -C $clone rev-parse HEAD).Trim()
if ($head3 -ne $head) { throw "patch_id mismatch moved HEAD to $head3" }
if ((Get-Porcelain) -ne $s2) { throw 'patch_id mismatch changed the clone working tree' }

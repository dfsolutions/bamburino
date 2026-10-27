# SPDX-License-Identifier: AGPL-3.0-or-later
# Tests for ci/check-workflows.ps1. Run: pwsh ci/tests/CheckWorkflows.Tests.ps1
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$script = Join-Path $here '../check-workflows.ps1'
$fixtures = Join-Path $here 'fixtures'
$failures = New-Object System.Collections.Generic.List[string]

function Assert-True($cond, $msg) {
    if ($cond) { Write-Host "  ok   $msg" } else { Write-Host "  FAIL $msg"; $failures.Add($msg) }
}
function Invoke-Check([string]$dir) {
    $out = & pwsh -NoProfile -File $script -Path $dir 2>&1 | Out-String
    [pscustomobject]@{ Code = $LASTEXITCODE; Out = $out }
}
function New-TempDir {
    $d = Join-Path ([IO.Path]::GetTempPath()) ("cw-" + [guid]::NewGuid())
    New-Item -ItemType Directory $d | Out-Null
    $d
}
$SHA = '0123456789abcdef0123456789abcdef01234567'
$guard = @"
  guard:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@$SHA # v7.0.1
        with: { sparse-checkout: ci/maintainers.txt }
      - run: grep -qx "`${{ github.actor }}" ci/maintainers.txt || { echo "::error::actor not allowed"; exit 1; }
"@
# Check one synthetic workflow written to a temp dir; returns the result object.
function Test-Case([string]$name, [string]$yaml) {
    $d = New-TempDir
    try { Set-Content -LiteralPath (Join-Path $d $name) -Value $yaml -Encoding utf8; Invoke-Check $d }
    finally { Remove-Item -Recurse -Force $d }
}

if (-not (Test-Path -LiteralPath $script)) {
    Write-Host "FAIL: $script not found"; exit 1
}

Write-Host 'fixtures'
$r = Invoke-Check $fixtures
Assert-True ($r.Code -eq 1) 'fixtures dir exits 1'
Assert-True ($r.Out -match 'bad-pr-trigger\.yml.*\[a\]') 'bad-pr-trigger.yml reported with rule a'
Assert-True ($r.Out -match 'bad-no-guard\.yml.*\[c\]') 'bad-no-guard.yml reported with rule c'
Assert-True ($r.Out -match 'bad-selfhosted-write\.yml.*\[d\]') 'bad-selfhosted-write.yml reported with rule d'
Assert-True ($r.Out -notmatch 'good-self-hosted\.yml') 'good-self-hosted.yml not reported'

Write-Host 'clean and empty'
$d = New-TempDir
Copy-Item (Join-Path $fixtures 'good-self-hosted.yml') $d
$r = Invoke-Check $d
Assert-True ($r.Code -eq 0) 'good fixture alone exits 0'
Remove-Item -Recurse -Force $d
$r = Invoke-Check (Join-Path ([IO.Path]::GetTempPath()) 'cw-does-not-exist')
Assert-True ($r.Code -eq 0) 'nonexistent path exits 0'
$d = New-TempDir
$r = Invoke-Check $d
Assert-True ($r.Code -eq 0) 'empty dir exits 0'
Remove-Item -Recurse -Force $d

Write-Host 'rule a/b (triggers, push filters)'
$sh = @"
jobs:
$guard
  build:
    needs: guard
    runs-on: self-hosted
    steps:
      - run: echo hi
"@
$r = Test-Case 'b1.yml' "on:`n  push:`n    branches: [dev]`n$sh"
Assert-True ($r.Code -eq 1 -and $r.Out -match '\[b\]') 'push to branch other than main -> b'
$r = Test-Case 'b2.yml' "on: push`n$sh"
Assert-True ($r.Code -eq 1 -and $r.Out -match '\[b\]') 'bare push without filters -> b'
$r = Test-Case 'b3.yml' "on:`n  push:`n    tags:`n      - 'v*'`n$sh"
Assert-True ($r.Code -eq 0) 'push with block-list tags v* is clean'
$r = Test-Case 'a2.yml' "on: [push, workflow_dispatch, schedule]`n$sh"
Assert-True ($r.Code -eq 1 -and $r.Out -match '\[a\].*schedule') 'extra trigger (schedule) -> a'

Write-Host 'rule c (guard)'
$noGuardJob = "on:`n  push:`n    branches: [main]`njobs:`n  build:`n    runs-on: self-hosted`n    steps:`n      - run: echo hi`n"
$r = Test-Case 'c1.yml' $noGuardJob
Assert-True ($r.Code -eq 1 -and $r.Out -match '\[c\].*guard') 'no guard job -> c'
$weak = $guard -replace 'grep -qx.*', 'echo ok'
$r = Test-Case 'c2.yml' ("on:`n  push:`n    branches: [main]`njobs:`n$weak`n  build:`n    needs: guard`n    runs-on: self-hosted`n    steps:`n      - run: echo hi`n")
Assert-True ($r.Code -eq 1 -and $r.Out -match '\[c\]') 'guard that does not check actor -> c'
$r = Test-Case 'c3.yml' ("on:`n  push:`n    branches: [main]`njobs:`n$guard`n  build:`n    needs:`n      - other`n      - guard`n    runs-on: self-hosted`n    steps:`n      - run: echo hi`n")
Assert-True ($r.Code -eq 0) 'block-list needs containing guard is clean'
$r = Test-Case 'c4.yml' ("on:`n  push:`n    branches: [main]`njobs:`n$($guard -replace 'ubuntu-latest','self-hosted')`n  build:`n    needs: guard`n    runs-on: self-hosted`n    steps:`n      - run: echo hi`n")
Assert-True ($r.Code -eq 1 -and $r.Out -match '\[c\]') 'guard on self-hosted runner -> c'

Write-Host 'rule d (write permissions on self-hosted)'
$r = Test-Case 'd1.yml' ("on:`n  push:`n    branches: [main]`npermissions: write-all`n$sh")
Assert-True ($r.Code -eq 1 -and $r.Out -match '\[d\]') 'write-all in self-hosted workflow -> d'
$r = Test-Case 'd2.yml' ("on:`n  push:`n    branches: [main]`npermissions:`n  packages: write`n$sh")
Assert-True ($r.Code -eq 1 -and $r.Out -match '\[d\]') 'workflow-level packages: write in self-hosted workflow -> d'

Write-Host 'global rules'
$r = Test-Case 'g1.yml' "on:`n  pull_request_target:`njobs:`n  x:`n    runs-on: ubuntu-latest`n    steps:`n      - run: echo`n"
Assert-True ($r.Code -eq 1 -and $r.Out -match '\[g\].*pull_request_target') 'pull_request_target -> g'
$r = Test-Case 'g2.yml' "on: workflow_dispatch`njobs:`n  x:`n    runs-on: ubuntu-latest`n    steps:`n      - uses: someone/action@v1`n"
Assert-True ($r.Code -eq 1 -and $r.Out -match '\[g\].*someone/action@v1') 'unpinned third-party uses -> g'
$r = Test-Case 'g3.yml' "on: workflow_dispatch`njobs:`n  x:`n    runs-on: ubuntu-latest`n    steps:`n      - uses: ./.github/actions/local`n      - uses: someone/action@$SHA # v1`n"
Assert-True ($r.Code -eq 0) 'local action and SHA-pinned action are clean'
$r = Test-Case 'g4.yml' "on: workflow_dispatch`njobs:`n  publish:`n    runs-on: ubuntu-latest`n    permissions:`n      contents: write`n    steps:`n      - run: echo`n"
Assert-True ($r.Code -eq 1 -and $r.Out -match '\[g\].*contents: write') 'contents: write outside release.yml -> g'
$r = Test-Case 'release.yml' "on: workflow_dispatch`njobs:`n  release:`n    runs-on: ubuntu-latest`n    permissions:`n      contents: write`n    steps:`n      - run: echo`n"
Assert-True ($r.Code -eq 0) 'contents: write in release job of release.yml, hosted -> clean'
$r = Test-Case 'release.yml' "on: workflow_dispatch`njobs:`n  other:`n    runs-on: ubuntu-latest`n    permissions:`n      contents: write`n    steps:`n      - run: echo`n"
Assert-True ($r.Code -eq 1 -and $r.Out -match '\[g\]') 'contents: write in non-release job of release.yml -> g'
$r = Test-Case 'release.yml' "on: workflow_dispatch`njobs:`n  release:`n    runs-on: self-hosted`n    permissions:`n      contents: write`n    steps:`n      - run: echo`n"
Assert-True ($r.Code -eq 1 -and $r.Out -match '\[g\]') 'release job on self-hosted runner -> g'
$r = Test-Case 'g5.yml' "on: workflow_dispatch`npermissions:`n  contents: write`njobs:`n  x:`n    runs-on: ubuntu-latest`n    steps:`n      - run: echo`n"
Assert-True ($r.Code -eq 1 -and $r.Out -match '\[g\]') 'workflow-level contents: write -> g'

if ($failures.Count -gt 0) { Write-Host "`n$($failures.Count) assertion(s) failed"; exit 1 }
Write-Host "`nAll assertions passed"

# SPDX-License-Identifier: AGPL-3.0-or-later
# Static assertions on the real .github/workflows/*.yml. Run: pwsh ci/tests/Workflows.Tests.ps1
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$wf = Join-Path $root '.github/workflows'
$failures = New-Object System.Collections.Generic.List[string]
function Assert-True($cond, $msg) {
    if ($cond) { Write-Host "  ok   $msg" } else { Write-Host "  FAIL $msg"; $failures.Add($msg) }
}
function Get-Wf([string]$n) { $p = Join-Path $wf $n; if (Test-Path $p) { Get-Content -LiteralPath $p -Raw } else { '' } }

$checker = & pwsh -NoProfile -File (Join-Path $root 'ci/check-workflows.ps1') -Path $wf 2>&1 | Out-String
Assert-True ($LASTEXITCODE -eq 0) "check-workflows.ps1 passes on the real workflows ($($checker.Trim()))"

$svc = Get-Wf 'build-service.yml'; $std = Get-Wf 'build-studio.yml'; $rel = Get-Wf 'release.yml'
Assert-True ($svc -and $std -and $rel) 'the three workflow files exist'

foreach ($pair in @(@('build-service.yml', $svc), @('build-studio.yml', $std), @('release.yml', $rel))) {
    $n, $t = $pair
    Assert-True ($t -match '(?m)^# SPDX-License-Identifier: AGPL-3\.0-or-later') "$n has the SPDX header"
    Assert-True ($t -notmatch '\$\{\{\s*secrets\.') "$n uses no secrets.*"
    Assert-True ($t -notmatch '(?m)^\s*pull_request_target') "$n has no pull_request_target"
    $uses = [regex]::Matches($t, '(?m)uses:\s*(\S+)') | ForEach-Object { $_.Groups[1].Value }
    Assert-True (@($uses | Where-Object { $_ -notmatch '@[0-9a-f]{40}$' }).Count -eq 0) "$n pins every uses: to a 40-hex SHA"
}

# build-service: hosted only
Assert-True ($svc -match '(?m)^\s+pull_request:' -and $svc -match "tags:\s*\['v\*'\]" -and $svc -match '(?m)^\s+workflow_dispatch:') 'build-service: push (tags v*), pull_request, workflow_dispatch'
Assert-True ($svc -notmatch 'self-hosted' -and $svc -match 'runs-on: windows-latest') 'build-service: hosted windows-latest only'
Assert-True ($svc -notmatch 'contents:\s*write' -and $svc -notmatch 'gh release') 'build-service: no release, no write'
foreach ($c in 'cargo fmt --all --check',
    'cargo clippy --workspace --all-targets -- -D warnings',
    'cargo clippy --workspace --all-targets --features bamburino-core/mock -- -D warnings',
    'cargo test --workspace --features bamburino-core/mock',
    'cargo build --release --workspace',
    'python -I catalog/fetch_sources.py',
    'python -I -m unittest catalog/test_build_catalog.py',
    'ci/package-service.ps1') {
    Assert-True ($svc.Contains($c)) "build-service runs: $c"
}
Assert-True ($svc -match 'name: bamburino-win64' -and $svc -match 'actions/cache@' -and $svc -match 'rustup show') 'build-service: artifact bamburino-win64, cargo cache, rustup show'

# build-studio: self-hosted behind guard
Assert-True ($std -match '(?m)^\s+branches:\s*\[main\]' -and $std -match 'driver/\*\*' -and $std -match '\.github/workflows/build-studio\.yml' -and $std -match "tags:\s*\['v\*'\]" -and $std -match '(?m)^\s+workflow_dispatch:') 'build-studio: push main (paths) + tags v*, workflow_dispatch'
Assert-True ($std -notmatch '(?m)^\s+pull_request') 'build-studio: no pull_request'
Assert-True ($std -match 'runs-on: \[self-hosted, bamburino-win\]' -and $std -match 'needs: \[guard\]') 'build-studio: runs on [self-hosted, bamburino-win], needs guard'
Assert-True ($std -match '(?m)^concurrency:' -and $std -match 'cancel-in-progress: false') 'build-studio: concurrency without cancel'
Assert-True ($std -match 'timeout-minutes: 720') 'build-studio: timeout-minutes 720'
Assert-True ($std -match 'BAMBURINO_BUILD_ROOT: D:\\bb' -and $std -match 'pwsh driver/build\.ps1 -Step all') 'build-studio: build root D:\bb, build.ps1 -Step all'
foreach ($m in '10 prerequisites missing', '20 patch does not apply', '21 patch_id differs from driver/upstream.toml', '30 dependency build failed', '40 app build failed', '45 MCP standalone tests failed', '50 packaging failed') {
    Assert-True ($std.Contains($m)) "build-studio maps exit code: $m"
}
Assert-True ($std -match 'name: studio-win64' -and $std -match 'retention-days: 14') 'build-studio: artifact studio-win64, retention 14 days'
Assert-True ($std -notmatch 'contents:\s*write' -and $std -notmatch 'gh release') 'build-studio: no write, no release'

# release: one hosted job
Assert-True ($rel -match '(?m)^on:\s*\r?\n\s+workflow_dispatch:' -and $rel -notmatch '(?m)^\s+(push|pull_request|schedule):') 'release: workflow_dispatch only'
foreach ($i in 'tag:', 'service_run_id:', 'studio_run_id:') { Assert-True ($rel.Contains($i)) "release: input $i" }
Assert-True ($rel -match '(?m)^  release:\s*$') 'release: job id release'
Assert-True (([regex]::Matches($rel, 'runs-on:')).Count -eq 1 -and $rel -match 'runs-on: ubuntu-latest') 'release: exactly one job, on ubuntu-latest'
Assert-True (([regex]::Matches($rel, 'contents: write')).Count -eq 1) 'release: contents: write exactly once'
Assert-True ($rel -match 'gh release create' -and $rel -match 'sha256' -and $rel -match 'gh run view' -and $rel -match 'run-id:' -and $rel -match 'github-token:') 'release: verifies runs, sha256, creates the release'

if ($failures.Count -gt 0) { Write-Host "`n$($failures.Count) assertion(s) failed"; exit 1 }
Write-Host "`nAll assertions passed"

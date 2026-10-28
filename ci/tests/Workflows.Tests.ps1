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
    'python -I catalog/test_build_catalog.py',
    'ci/package-service.ps1') {
    Assert-True ($svc.Contains($c)) "build-service runs: $c"
}
Assert-True ($svc -match 'name: bamburino-win64' -and $svc -match 'actions/cache@' -and $svc -match 'rustup toolchain install') 'build-service: artifact bamburino-win64, cargo cache, rustup toolchain install'

# build-studio: self-hosted behind guard
Assert-True ($std -match '(?m)^\s+branches:\s*\[main\]' -and $std -match 'driver/\*\*' -and $std -match '\.github/workflows/build-studio\.yml' -and $std -match "tags:\s*\['v\*'\]" -and $std -match '(?m)^\s+workflow_dispatch:') 'build-studio: push main (paths) + tags v*, workflow_dispatch'
Assert-True ($std -notmatch '(?m)^\s+pull_request') 'build-studio: no pull_request'
Assert-True ($std -match 'runs-on: \[self-hosted, bamburino-win\]' -and $std -match 'needs: \[guard\]') 'build-studio: runs on [self-hosted, bamburino-win], needs guard'
Assert-True ($std -match '(?m)^concurrency:' -and $std -match 'cancel-in-progress: false') 'build-studio: concurrency without cancel'
Assert-True ($std -match 'timeout-minutes: 720') 'build-studio: timeout-minutes 720'
Assert-True ($std -match 'BAMBURINO_BUILD_ROOT: D:\\BambuBuild' -and $std -match 'pwsh driver/build\.ps1 -Step all') 'build-studio: build root D:\BambuBuild, build.ps1 -Step all'
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

# ---- pin age, checkout hygiene, no workflow_run ----
$denyTags = @('v7.1.0')   # releases younger than 14 days when pinned: setup-python v7.1.0 (2026-10-22)
$all = $svc + $std + $rel
foreach ($m in [regex]::Matches($all, 'uses:\s*\S+@[0-9a-f]{40}\s*#\s*(v\S+)')) {
    Assert-True ($m.Groups[1].Value -notin $denyTags) "pin comment $($m.Groups[1].Value) is not in the too-recent deny-list"
}
Assert-True ($svc -match 'actions/setup-python@5fda3b95a4ea91299a34e894583c3862153e4b97 # v7\.0\.0') 'setup-python is pinned to v7.0.0 (>= 14 days old)'
Assert-True ($all -notmatch '(?m)^\s+workflow_run:') 'no workflow_run trigger anywhere'
$studioJob = $std.Substring($std.IndexOf('  studio:'))
Assert-True ($studioJob -match 'actions/checkout@[0-9a-f]{40} # v\S+\s+with:\s+persist-credentials: false') 'self-hosted checkout sets persist-credentials: false'
Assert-True ($svc -match '(?m)^\s+run: \|[\s\S]*?pwsh ci/check-repo\.ps1' -and $svc.Contains('pwsh ci/check-docs.ps1') -and $svc.Contains('pwsh ci/check-workflows.ps1 -Path .github/workflows')) 'build-service runs check-repo, check-docs and check-workflows'
Assert-True ($svc -notmatch 'python\s+-I\s+-m\s+unittest') 'build-service: no `python -I -m unittest` (-I drops the cwd from sys.path; run the test file directly)'
Assert-True ($svc -match 'rustup component add rustfmt clippy') 'build-service: rustfmt and clippy components are installed'
Assert-True ($svc -match 'Script tests' -and $svc -match 'ci/tests/CheckDocs\.Tests\.ps1' -and $svc -match 'ci/runner/tests/run\.ps1' -and $svc -match 'tools/tests/run\.ps1' -and $svc -match 'ci/tests/Package\.Tests\.ps1') 'build-service: script tests step'
Assert-True ($svc -match 'Script tests' -and $svc -match 'ci/tests/Package\.Tests\.ps1') 'build-service: script tests step runs Package.Tests'
Assert-True ($svc.IndexOf('pwsh ci/check-repo.ps1') -lt $svc.IndexOf('rustup toolchain install')) 'build-service: repository checks come before the cargo steps'
Assert-True ($svc -match 'rustup toolchain install' -and $svc -notmatch 'rustup show') 'build-service installs the pinned toolchain (rustup toolchain install)'
Assert-True ($std -notmatch 'D:\\BambuBuild\\artifacts' -and $std -match 'env\.BAMBURINO_BUILD_ROOT') 'build-studio upload path uses BAMBURINO_BUILD_ROOT'

# ---- release.yml: event check (YAML + the real shell function against a fake gh) ----
Assert-True ($rel -match '--json event' -and $rel -match 'push\|workflow_dispatch\) ;;') 'release: check_run requires event push or workflow_dispatch'
$lines = $rel -split "`r?`n"
$a = ($lines | Select-String -SimpleMatch 'check_run() {').LineNumber
if ($a) {
    $fn = New-Object System.Collections.Generic.List[string]
    for ($i = $a - 1; $i -lt $lines.Count; $i++) { $fn.Add($lines[$i].Substring(10)); if ($lines[$i] -match '^          }$') { break } }
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("rel-" + [guid]::NewGuid() + ".sh")
    $stub = @'
gh() { case "$*" in
  *"api repos"*) echo "${FAKE_PATH:-.github/workflows/build-service.yml@refs/heads/main}";;
  *"--json event"*) echo "$FAKE_EVENT";;
  *"--json conclusion"*) echo success;;
  *"--json headSha"*) echo abc;;
  *"--json workflowName"*) echo build-service;;
esac; }
ref_sha=abc
'@
    [IO.File]::WriteAllText($tmp, ($stub + "`n" + ($fn -join "`n") + "`ncheck_run 1 build-service`n"))
    # `bash` on PATH can be the WSL launcher (System32\bash.exe) with no distro: it exits 1 for every script,
    # which would make the reject tests vacuous and the accept tests fail. Probe candidates, use the first that works.
    $bash = $null
    $cands = @(Get-Command bash -All -ErrorAction SilentlyContinue | ForEach-Object { $_.Source })
    $gitCmd = Get-Command git -ErrorAction SilentlyContinue
    if ($gitCmd) { $gr = Split-Path (Split-Path $gitCmd.Source); $cands = @((Join-Path $gr 'bin\bash.exe'), (Join-Path $gr 'usr\bin\bash.exe')) + $cands }
    foreach ($c in $cands) {
        if ($c -and (Test-Path $c)) { $probe = (& $c -c 'echo probe-ok' 2>$null); if ($LASTEXITCODE -eq 0 -and "$probe".Trim() -eq 'probe-ok') { $bash = $c; break } }
    }
    if ($bash) {
        foreach ($ev in 'pull_request', 'pull_request_target', 'schedule') {
            $env:FAKE_EVENT = $ev; & $bash $tmp 2>&1 | Out-Null
            Assert-True ($LASTEXITCODE -ne 0) "check_run rejects a '$ev' run"
        }
        foreach ($ev in 'push', 'workflow_dispatch') {
            $env:FAKE_EVENT = $ev; & $bash $tmp 2>&1 | Out-Null
            Assert-True ($LASTEXITCODE -eq 0) "check_run accepts a '$ev' run"
        }
        $env:FAKE_EVENT = 'push'; $env:FAKE_PATH = '.github/workflows/evil.yml@refs/heads/main'; & $bash $tmp 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -ne 0) 'check_run rejects a run of another workflow file'
        # tag-triggered runs report the bare workflow path (no @ref): accepted; a longer name with the same prefix is not
        $env:FAKE_EVENT = 'push'; $env:FAKE_PATH = '.github/workflows/build-service.yml'; & $bash $tmp 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -eq 0) 'check_run accepts the bare workflow path (no @ref)'
        $env:FAKE_PATH = '.github/workflows/build-service.yml.evil'; & $bash $tmp 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -ne 0) 'check_run rejects a bare path with a suffix'
        $env:FAKE_PATH = '.github/workflows/evil.yml'; & $bash $tmp 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -ne 0) 'check_run rejects a bare path of another workflow file'
        Remove-Item Env:FAKE_PATH
        Remove-Item Env:FAKE_EVENT
    } else { Write-Host '  skip bash not found: shell snippet test' }
    Remove-Item $tmp -Force -ErrorAction SilentlyContinue
} else { Assert-True $false 'release: check_run function found' }

if ($failures.Count -gt 0) { Write-Host "`n$($failures.Count) assertion(s) failed"; exit 1 }
Write-Host "`nAll assertions passed"

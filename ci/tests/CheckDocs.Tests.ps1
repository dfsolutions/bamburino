# SPDX-License-Identifier: AGPL-3.0-or-later
# Tests for ci/check-docs.ps1: one bad file per rule, plus a clean tree and the real repo.
# Run: pwsh ci/tests/CheckDocs.Tests.ps1
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$script = Join-Path $root 'ci/check-docs.ps1'
$failures = New-Object System.Collections.Generic.List[string]
function Assert-True($cond, $msg) {
    if ($cond) { Write-Host "  ok   $msg" } else { Write-Host "  FAIL $msg"; $failures.Add($msg) }
}
$pwsh = (Get-Process -Id $PID).Path

# Builds a fixture repo: README.md + docs/other.md; returns its path.
function New-Fixture([string]$readme, [string]$other = "# Other`n") {
    $d = Join-Path ([IO.Path]::GetTempPath()) ("cd-" + [guid]::NewGuid())
    New-Item -ItemType Directory (Join-Path $d 'docs') | Out-Null
    [IO.File]::WriteAllText((Join-Path $d 'README.md'), $readme)
    [IO.File]::WriteAllText((Join-Path $d 'docs/other.md'), $other)
    $d
}
function Invoke-Check([string]$dir) {
    $out = & $pwsh -NoProfile -File $script -Root $dir 2>&1 | Out-String
    [pscustomobject]@{ Code = $LASTEXITCODE; Out = $out }
}

$tmp = @()
try {
    $clean = New-Fixture "# T`nSee [other](docs/other.md) and [web](https://example.com).`n"
    $tmp += $clean
    $r = Invoke-Check $clean
    Assert-True ($r.Code -eq 0) 'clean tree passes'

    $cases = [ordered]@{
        'italian marker'      = @("# T`nQuesto verificato è finito.`n", 'italian marker')
        'italian in docs'     = @("# T`n", 'italian marker', "Attenzione al file.`n")
        'status table'        = @("# T`n| Item | Outcome |`n|---|---|`n| a | passed |`n", 'outside verified')
        'status table (ok)'   = @("# T`n| Item | Outcome |`n|---|---|`n| a | verified |`n| b | not run |`n", $null)
        'stale version'       = @("# T`nBuilt on 02.08.02.61.`n", 'stale Studio version')
        'broken link'         = @("# T`n[x](docs/missing.md)`n", 'broken link')
        'hex token'           = @(("# T`n" + ('a1' * 32) + "`n"), 'token-like')
        'base64url token'     = @("# T`nAbCdEfGh1234567890_-AbCdEfGh1234567890`n", 'token-like')
    }
    foreach ($name in $cases.Keys) {
        $c = $cases[$name]
        $other = if ($c.Count -gt 2) { $c[2] } else { "# Other`n" }
        $d = New-Fixture $c[0] $other
        $tmp += $d
        $r = Invoke-Check $d
        if ($null -eq $c[1]) {
            Assert-True ($r.Code -eq 0) "$name -> exit 0"
        } else {
            Assert-True ($r.Code -eq 1 -and $r.Out -match [regex]::Escape($c[1])) "$name -> exit 1 with '$($c[1])'"
        }
    }

    # docs/superpowers is not scanned
    $d = New-Fixture "# T`n"
    $tmp += $d
    New-Item -ItemType Directory (Join-Path $d 'docs/superpowers') | Out-Null
    [IO.File]::WriteAllText((Join-Path $d 'docs/superpowers/x.md'), "Attenzione, verificato.`n")
    Assert-True ((Invoke-Check $d).Code -eq 0) 'docs/superpowers is excluded'

    # the real repository
    $r = Invoke-Check $root
    Assert-True ($r.Code -eq 0) 'the repository docs pass'
    if ($r.Code -ne 0) { Write-Host $r.Out }
} finally {
    foreach ($d in $tmp) { Remove-Item -LiteralPath $d -Recurse -Force -ErrorAction SilentlyContinue }
}

if ($failures.Count -gt 0) { Write-Host "$($failures.Count) failure(s)"; exit 1 }
Write-Host 'all CheckDocs tests passed'
exit 0

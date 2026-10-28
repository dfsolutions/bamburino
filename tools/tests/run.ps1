<#
.SYNOPSIS
    Runs tools/tests/*.Tests.ps1.
.DESCRIPTION
    Unit tests (Upstream, Patch, Audit, Package) always run: they need no build root and never touch the clone.
    Integration tests (Apply, Identity, Snapshot, App, Deps, Prereqs) run -Step patch on <BuildRoot>\BambuStudio
    (re-applying the patch set) or read the build root. They run when -Integration is passed, or by default when
    <BuildRoot>\BambuStudio exists. In both cases they are refused while a build is running: a cmake.exe process, or
    an MSBuild.exe that is not an idle worker node ('/nodemode:'), because -Step patch resets the tree under it.
    Refused with -Integration: exit 1. Skipped by default (no clone, or a build running): reported as SKIP, exit 0.
    Every test file not in either list is treated as integration.
#>
[CmdletBinding()]
param([switch]$Integration)
Import-Module "$PSScriptRoot/../Upstream.psm1" -Force
$unit = @('Upstream', 'Patch', 'Audit', 'Package')

function Get-RunningBuild {
    $procs = @(Get-CimInstance Win32_Process -Filter "Name='cmake.exe' OR Name='MSBuild.exe'" -ErrorAction SilentlyContinue)
    return @($procs | Where-Object { $_.Name -ieq 'cmake.exe' -or -not ($_.CommandLine -match '/nodemode:') })
}

$tests = @(Get-ChildItem -Path $PSScriptRoot -Filter '*.Tests.ps1' | Sort-Object Name)
$runIntegration = $true
$clone = Join-Path (Get-BuildRoot) 'BambuStudio'
$busy = @(Get-RunningBuild)
if ($busy.Count -gt 0) {
    $what = ($busy | ForEach-Object { "$($_.Name) pid $($_.ProcessId)" }) -join ', '
    if ($Integration) { Write-Host "REFUSED integration tests: a build is running ($what); they run -Step patch on $clone. Wait for it to finish."; exit 1 }
    Write-Host "SKIP integration tests: a build is running ($what)"
    $runIntegration = $false
} elseif (-not $Integration -and -not (Test-Path -LiteralPath $clone)) {
    Write-Host "SKIP integration tests: $clone does not exist (pass -Integration to force)"
    $runIntegration = $false
}

$failed = $false
foreach ($t in $tests) {
    $name = $t.Name -replace '\.Tests\.ps1$', ''
    if ($unit -notcontains $name -and -not $runIntegration) { Write-Host "SKIP $($t.Name)"; continue }
    try {
        $global:LASTEXITCODE = 0
        & $t.FullName | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "exit code $LASTEXITCODE" }
        Write-Host "PASS $($t.Name)"
    } catch {
        Write-Host "FAIL $($t.Name): $($_.Exception.Message)"
        $failed = $true
    }
}
if ($failed) { exit 1 }
exit 0

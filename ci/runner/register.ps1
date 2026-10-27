# SPDX-License-Identifier: AGPL-3.0-or-later
# Registers THIS PC as an on-demand self-hosted runner for dfsolutions/bamburino.
# Not a Windows service: the runner lives in the interactive session (the smoke test opens a GUI)
# and starts only when ci/runner/start.ps1 is run. Never prints the registration token.
[CmdletBinding()]
param(
    [string]$Repo = 'dfsolutions/bamburino',
    [string]$Version = '2.338.0',
    [string]$RunnerDir = 'D:\bb\runner',
    [string]$WorkDir = 'D:\bb\runner\_work',
    [string]$Name = 'federico-pc',
    [string]$Labels = 'bamburino-win'
)
$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot/../.."
Import-Module "$root/tools/GitHub.psm1" -Force
Import-Module "$PSScriptRoot/Runner.psm1" -Force
Set-GhToken

$zipName = "actions-runner-win-x64-$Version.zip"
$zip = Join-Path $RunnerDir $zipName
New-Item -ItemType Directory -Force -Path $RunnerDir | Out-Null

if (-not (Test-Path (Join-Path $RunnerDir 'config.cmd'))) {
    $body = gh api "repos/actions/runner/releases/tags/v$Version" --jq .body
    if ($LASTEXITCODE -ne 0) { throw "cannot read release notes for v$Version" }
    $expected = Get-RunnerSha256 -ReleaseBody ($body -join "`n")
    Invoke-WebRequest -Uri "https://github.com/actions/runner/releases/download/v$Version/$zipName" -OutFile $zip
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLowerInvariant()
    if ($actual -ne $expected) { Remove-Item -LiteralPath $zip -Force; throw "SHA-256 mismatch for $zipName" }
    Write-Host "SHA-256 verified: $actual"
    Expand-Archive -LiteralPath $zip -DestinationPath $RunnerDir -Force
}

# The token lives only in this variable and in the config.cmd command line; it is never printed.
$token = gh api -X POST "repos/$Repo/actions/runners/registration-token" --jq .token
if ($LASTEXITCODE -ne 0 -or -not $token) { throw 'registration-token request failed (needs repo scope and admin on the repository)' }
$cfgArgs = New-RunnerConfigArgs -Url "https://github.com/$Repo" -Token $token -Name $Name -Labels $Labels -Work $WorkDir
Push-Location $RunnerDir
try {
    $out = & .\config.cmd @cfgArgs 2>&1 | Out-String
    $code = $LASTEXITCODE
} finally { Pop-Location }
Write-Host ($out -replace [regex]::Escape($token), '***')
$token = $null; $cfgArgs = $null
if ($code -ne 0) { throw "config.cmd failed with exit code $code" }

& "$PSScriptRoot/secure-repo.ps1" -Repo $Repo

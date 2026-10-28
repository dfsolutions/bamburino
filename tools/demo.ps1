# SPDX-License-Identifier: AGPL-3.0-or-later
<#
.SYNOPSIS
  Runs the acceptance steps 2, 3, 5, 6, 7 and 8 of the design (section 9)
  through the running Bamburino service, over plain JSON-RPC.

.DESCRIPTION
  Needs Bambu Studio (patched build, "Local MCP server" on) with a project
  that holds at least one object, and the Bamburino service running.
  The script changes the open project: it sets infill/pattern/walls/layer,
  rotates the first object by 90 degrees about Z, enables tree supports and
  slices. Use a scratch project (the test cube) and do not save it.

  Each step prints "verified" or "failed" with the values read back. With
  -Record the matching rows of docs/acceptance-mvp-service.md are updated.
  Steps 1, 4, 4b, 4c, 9 and 10 are not scripted (manual: Ctrl+Z, closed
  Studio, modal dialog).

.PARAMETER BaseUrl
  MCP endpoint. Default http://127.0.0.1:27600/mcp.

.PARAMETER Token
  Client token. If omitted it is read from `bamburino config client-snippet
  --client generic` (needs the same Windows account that ran `config init`).

.PARAMETER DryRun
  Prints the JSON-RPC calls without sending anything.

.PARAMETER Record
  Path of the acceptance file to update (default docs/acceptance-mvp-service.md
  next to this repository's tools folder); pass an empty string to skip.

.EXAMPLE
  pwsh tools/demo.ps1
  pwsh tools/demo.ps1 -DryRun
#>
[CmdletBinding()]
param(
    [string]$BaseUrl = 'http://127.0.0.1:27600/mcp',
    [string]$Token,
    [switch]$DryRun,
    [string]$Record = (Join-Path $PSScriptRoot '..\docs\acceptance-mvp-service.md'),
    [string]$Bamburino = 'bamburino'
)
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'Demo.psm1') -Force

if (-not $Token -and -not $DryRun) {
    $text = & $Bamburino config client-snippet --client generic 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) { throw "bamburino config client-snippet failed: $text" }
    $snippet = Read-ClientSnippet $text
    $Token = $snippet.Token
    if (-not $PSBoundParameters.ContainsKey('BaseUrl')) { $BaseUrl = $snippet.Url }
}
if ($DryRun -and -not $Token) { $Token = '<token>' }

$session = New-McpSession -BaseUrl $BaseUrl -Token $Token -DryRun:$DryRun
$prompt = { param($msg) Write-Host ''; Write-Host ">>> $msg" -ForegroundColor Cyan; [void](Read-Host 'Press Enter when done') }
$record = if ($Record -and -not $DryRun -and (Test-Path -LiteralPath $Record)) { $Record } else { $null }

Write-Host "Bamburino demo against $BaseUrl$(if ($DryRun) { ' (dry run: nothing is sent)' })"
Initialize-McpSession $session

$results = @()
$steps = @(
    @{ N = '2'; Run = { Invoke-DemoStep2 $session } },
    @{ N = '3'; Run = { Invoke-DemoStep3 $session } },
    @{ N = '5'; Run = { Invoke-DemoStep5 $session $prompt } },
    @{ N = '6'; Run = { Invoke-DemoStep6 $session } },
    @{ N = '7'; Run = { Invoke-DemoStep7 $session } },
    @{ N = '8'; Run = { Invoke-DemoStep8 $session } }
)
foreach ($s in $steps) {
    Write-Host ''
    Write-Host "== step $($s.N)"
    try {
        $r = & $s.Run
    } catch {
        $r = New-StepResult $s.N 'failed' "exception: $($_.Exception.Message)"
    }
    $results += $r
    $color = switch ($r.Result) { 'verified' { 'Green' } 'failed' { 'Red' } default { 'Yellow' } }
    Write-Host ("step {0} | {1} | {2}" -f $r.Step, $r.Result, $r.Observed) -ForegroundColor $color
    if ($record -and $r.Result -ne 'not run') {
        Update-AcceptanceRow -Path $record -Step $r.Step -Result $r.Result -Observed $r.Observed
    }
}

Write-Host ''
Write-Host 'Manual steps still to do and record by hand: 1, 4, 4b, 4c, 9, 10.'
if ($record) { Write-Host "Results written to $record" }
if (@($results | Where-Object { $_.Result -eq 'failed' }).Count -gt 0) { exit 1 }
exit 0

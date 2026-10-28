# SPDX-License-Identifier: AGPL-3.0-or-later
# Stops the on-demand runner: graceful stop of the process started by start.ps1, forced stop of
# Runner.Listener / Runner.Worker after a timeout, then waits until GitHub reports it offline.
# Exit 0 only when the processes are gone AND the runner is offline (or absent) on GitHub.
[CmdletBinding()]
param([string]$RunnerDir = 'D:\GoogleAI\BAMBU-MCP\build\runner', [string]$Name = 'federico-pc')
$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot/../.."
Import-Module "$root/tools/GitHub.psm1" -Force
Import-Module "$PSScriptRoot/Runner.psm1" -Force
Set-GhToken
$ok = Stop-Runner -PidFile (Join-Path $RunnerDir '.bamburino-runner.pid') -Name $Name
if (-not $ok) { exit 1 }
exit 0

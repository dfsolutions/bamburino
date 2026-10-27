# SPDX-License-Identifier: AGPL-3.0-or-later
# Starts the runner in its own console window (interactive session). Stop it with ci/runner/stop.ps1
# (closing the window with X leaves the runner 'online' on GitHub for about a minute). The runner never starts with Windows.
[CmdletBinding()]
param([string]$RunnerDir = 'D:\bb\runner')
$ErrorActionPreference = 'Stop'
$run = Join-Path $RunnerDir 'run.cmd'
if (-not (Test-Path -LiteralPath $run)) { throw "run.cmd not found in $RunnerDir; run ci/runner/register.ps1 first" }
$proc = Start-Process -FilePath $run -WorkingDirectory $RunnerDir -PassThru
Set-Content -LiteralPath (Join-Path $RunnerDir '.bamburino-runner.pid') -Value $proc.Id -Encoding ascii
Write-Host "Runner started in a new window ($RunnerDir). It is listening when the window shows 'Listening for Jobs'."

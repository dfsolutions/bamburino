# SPDX-License-Identifier: AGPL-3.0-or-later
# Starts the runner in its own console window (interactive session). Close the window or press
# Ctrl+C there to stop it. The runner never starts with Windows.
[CmdletBinding()]
param([string]$RunnerDir = 'D:\bb\runner')
$ErrorActionPreference = 'Stop'
$run = Join-Path $RunnerDir 'run.cmd'
if (-not (Test-Path -LiteralPath $run)) { throw "run.cmd not found in $RunnerDir; run ci/runner/register.ps1 first" }
Start-Process -FilePath $run -WorkingDirectory $RunnerDir
Write-Host "Runner started in a new window ($RunnerDir). It is listening when the window shows 'Listening for Jobs'."

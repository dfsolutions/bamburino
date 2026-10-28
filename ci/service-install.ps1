# SPDX-License-Identifier: AGPL-3.0-or-later
# Shipped in the service zip as install.ps1. Run from an elevated PowerShell in the extracted folder.
$ErrorActionPreference = 'Stop'
$id = [Security.Principal.WindowsIdentity]::GetCurrent()
if (-not ([Security.Principal.WindowsPrincipal]$id).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host 'This installer registers a Windows service and must run as Administrator.'
    Write-Host 'Open PowerShell with "Run as administrator", cd to this folder and run .\install.ps1 again.'
    exit 5
}
$exe = Join-Path $PSScriptRoot 'bamburino.exe'
if (-not (Test-Path -LiteralPath $exe)) { Write-Host "bamburino.exe not found next to install.ps1 ($PSScriptRoot)"; exit 1 }
& $exe --install
if ($LASTEXITCODE -ne 0) { Write-Host "bamburino.exe --install failed (exit $LASTEXITCODE)"; exit $LASTEXITCODE }
Write-Host ''
Write-Host 'Next steps (as your normal user, not elevated):'
Write-Host '  1. Give the service the driver token from Bambu Studio (Preferences, MCP server):'
Write-Host '       & "$env:ProgramFiles\Bamburino\bamburino.exe" config set-driver-token'
Write-Host '     then, from an elevated shell:  Restart-Service Bamburino'
Write-Host '  2. Print the line that registers Bamburino in your MCP client:'
Write-Host '       & "$env:ProgramFiles\Bamburino\bamburino.exe" config client-snippet'
exit 0

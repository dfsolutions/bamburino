# SPDX-License-Identifier: AGPL-3.0-or-later
# Per-user installer for the Bamburino build of Bambu Studio (no administrator rights).
#   pwsh install-studio.ps1 [-Zip <BambuStudio-bamburino-...-win64.zip>]   install or upgrade
#   pwsh install-studio.ps1 -Uninstall                                     remove folder and shortcut
# Without -Zip, installs from the folder this script sits in (the script is shipped inside the zip).
# Installs to %LOCALAPPDATA%\Programs\BambuStudio-Bamburino. %APPDATA%\BambuStudio (your settings) is never touched.
[CmdletBinding()]
param([string]$Zip, [switch]$Uninstall)
$ErrorActionPreference = 'Stop'
$installDir = Join-Path $env:LOCALAPPDATA 'Programs\BambuStudio-Bamburino'
$lnkDir = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$lnk = Join-Path $lnkDir 'Bambu Studio (Bamburino).lnk'

function Test-RunningFromInstall {
    $prefix = $installDir.TrimEnd('\') + '\'
    @(Get-Process -ErrorAction SilentlyContinue | Where-Object {
        try { $_.Path -and $_.Path.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) } catch { $false }
    }).Count -gt 0
}

if ($Uninstall) {
    if (Test-RunningFromInstall) { throw 'Bambu Studio (Bamburino) is running; close it and retry.' }
    if (Test-Path -LiteralPath $installDir) { Remove-Item -LiteralPath $installDir -Recurse -Force }
    if (Test-Path -LiteralPath $lnk) { Remove-Item -LiteralPath $lnk -Force }
    Write-Host "Removed $installDir and the Start Menu shortcut. Your settings in $env:APPDATA\BambuStudio were left untouched."
    exit 0
}

$stage = Join-Path ([IO.Path]::GetTempPath()) ("bamburino-studio-" + [guid]::NewGuid())
$src = $null
try {
    if ($Zip) {
        if (-not (Test-Path -LiteralPath $Zip -PathType Leaf)) { throw "zip not found: $Zip" }
        New-Item -ItemType Directory -Path $stage | Out-Null
        Expand-Archive -LiteralPath $Zip -DestinationPath $stage -Force
        # tolerate a single top-level folder inside the zip
        if (-not (Test-Path -LiteralPath (Join-Path $stage 'bambu-studio.exe'))) {
            $subs = @(Get-ChildItem -LiteralPath $stage -Directory)
            if ($subs.Count -eq 1 -and (Test-Path -LiteralPath (Join-Path $subs[0].FullName 'bambu-studio.exe'))) { $src = $subs[0].FullName }
        }
        if (-not $src) { $src = $stage }
    } else {
        $src = $PSScriptRoot
    }
    if (-not (Test-Path -LiteralPath (Join-Path $src 'bambu-studio.exe'))) { throw "bambu-studio.exe not found in $src" }
    if (Test-RunningFromInstall) { throw 'Bambu Studio (Bamburino) is running; close it and retry.' }

    if (Test-Path -LiteralPath $installDir) { Remove-Item -LiteralPath $installDir -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
    Copy-Item -Path (Join-Path $src '*') -Destination $installDir -Recurse -Force
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}

New-Item -ItemType Directory -Force -Path $lnkDir | Out-Null
$sh = New-Object -ComObject WScript.Shell
$sc = $sh.CreateShortcut($lnk)
$sc.TargetPath = Join-Path $installDir 'bambu-studio.exe'
$sc.WorkingDirectory = $installDir
$sc.Description = 'Bambu Studio with the Bamburino MCP driver'
$sc.Save()

Write-Host "Installed to $installDir"
Write-Host 'Start Menu shortcut: "Bambu Studio (Bamburino)"'
Write-Host ''
Write-Host 'Notes:'
Write-Host '  - Do not run this build together with the official Bambu Studio: they share the same settings folder'
Write-Host '    (%APPDATA%\BambuStudio). Close one before starting the other.'
Write-Host '  - This installer and the build are not code-signed: Windows SmartScreen will warn on first launch'
Write-Host '    ("More info" > "Run anyway").'
Write-Host '  - Uninstall: install-studio.ps1 -Uninstall'
exit 0

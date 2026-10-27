<#
.SYNOPSIS
    Checks (and optionally installs) the Windows prerequisites for building Bambu Studio.
.DESCRIPTION
    -Check   prints one line 'OK|MISSING <name> <detail>' per prerequisite; exit 0 if all OK, 10 otherwise.
    -Install installs only what is missing (Strawberry Perl, pkg-config-lite, git core.longpaths), then checks.
             Visual Studio and the Windows SDK are never installed by this script.
    -Export  (with -Check or -Install) sets session variables BAMBURINO_CMAKE and BAMBURINO_WIN10SDK_PATH
             and rebuilds the session PATH for a deterministic toolchain: the VS-bundled CMake bin directory first,
             then Strawberry perl\bin and pkg-config; C:\Strawberry\c\bin (MinGW gcc, headers, libs, its own CMake)
             is removed so MinGW files cannot leak into find_package during the upstream build.
             Invoke in-process (& driver/prereqs.ps1 -Check -Export) for the variables to reach the caller.
#>
[CmdletBinding()]
param(
    [switch]$Check,
    [switch]$Install,
    [switch]$Export
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot '..\tools\Upstream.psm1') -Force

$MinCmake = [version]'3.24.0'
$MinFreeGB = 25
$SdkIncludeRoot = 'C:\Program Files (x86)\Windows Kits\10\Include'
$StrawberryRoot = 'C:\Strawberry\'
$StrawberryMingwBin = 'C:\Strawberry\c\bin'

# Looks up an executable in the process PATH, then in the persisted Machine/User PATH
# (a fresh install updates the registry but not the PATH of already running processes).
function Find-Exe([string]$Name) {
    $dirs = @($env:Path -split ';') +
        @([Environment]::GetEnvironmentVariable('Path', 'Machine') -split ';') +
        @([Environment]::GetEnvironmentVariable('Path', 'User') -split ';')
    foreach ($d in $dirs) {
        if (-not $d) { continue }
        $p = Join-Path ([Environment]::ExpandEnvironmentVariables($d)) $Name
        if (Test-Path -LiteralPath $p -PathType Leaf) { $p }
    }
}

function Test-VsBuildTools {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere)) { return @{ Ok = $false; Detail = 'vswhere.exe not found' } }
    $installs = @(& $vswhere -products * -version '[17.0,18.0)' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -format json | ConvertFrom-Json)
    foreach ($i in $installs) {
        $msvc = Join-Path $i.installationPath 'VC\Tools\MSVC'
        $toolset = Get-ChildItem -LiteralPath $msvc -Directory -Filter '14.4*' -ErrorAction SilentlyContinue |
            Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'bin\Hostx64\x64\cl.exe') } |
            Sort-Object { [version]$_.Name } | Select-Object -Last 1
        if ($toolset) {
            return @{
                Ok          = $true
                InstallPath = $i.installationPath
                Detail      = "$($i.displayName) $($i.installationVersion), MSVC $($toolset.Name) at $($i.installationPath)"
            }
        }
    }
    if ($installs.Count -eq 0) { return @{ Ok = $false; Detail = 'no VS 2022 instance with the C++ x64 toolset' } }
    return @{ Ok = $false; InstallPath = $installs[0].installationPath; Detail = 'no MSVC 14.4x toolset with cl.exe' }
}

# VS-bundled CMake first (deterministic toolchain), PATH only as fallback; Strawberry's MinGW CMake is never accepted.
function Test-Cmake([string]$VsInstall) {
    $candidates = @()
    if ($VsInstall) { $candidates += Join-Path $VsInstall 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe' }
    $candidates += @(Find-Exe 'cmake.exe')
    $seen = @()
    foreach ($c in $candidates) {
        if (-not (Test-Path -LiteralPath $c -PathType Leaf)) { continue }
        if ($c.StartsWith($StrawberryRoot, [StringComparison]::OrdinalIgnoreCase)) { continue }
        $out = (& $c --version 2>$null) -join "`n"
        if ($out -match 'cmake version (\d+\.\d+\.\d+)') {
            if ([version]$Matches[1] -ge $MinCmake) { return @{ Ok = $true; Path = $c; Detail = "$($Matches[1]) at $c" } }
            $seen += "$($Matches[1]) at $c"
        }
    }
    $why = if ($seen) { 'only ' + ($seen -join ', ') } else { 'cmake.exe not found' }
    return @{ Ok = $false; Detail = "need >= $MinCmake; $why" }
}

function Test-StrawberryPerl {
    $candidates = @(Find-Exe 'perl.exe') + 'C:\Strawberry\perl\bin\perl.exe'
    foreach ($c in $candidates) {
        if (-not (Test-Path -LiteralPath $c -PathType Leaf)) { continue }
        # Strawberry identifies itself only in perl -V (uname='Win32 strawberry-perl ...'), not in perl -v.
        $out = (& $c -V 2>$null) -join "`n"
        if ($out -match 'strawberry-perl') {
            $ver = if ($out -match 'strawberry-perl (\d+(\.\d+)+)') { $Matches[1] } else { '?' }
            return @{ Ok = $true; Path = $c; Detail = "$ver at $c" }
        }
    }
    return @{ Ok = $false; Detail = 'no Strawberry perl.exe (other perls do not count)' }
}

function Test-PkgConfig {
    $c = @(Find-Exe 'pkg-config.exe') | Select-Object -First 1
    if (-not $c) { return @{ Ok = $false; Detail = 'pkg-config.exe not found' } }
    $ver = (& $c --version 2>$null) -join ''
    return @{ Ok = $true; Path = $c; Detail = "$ver at $c" }
}

function Test-WindowsSdk {
    $best = Get-ChildItem -LiteralPath $SdkIncludeRoot -Directory -Filter '10.0.*' -ErrorAction SilentlyContinue |
        Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'um\windows.h') } |
        Sort-Object { [version]$_.Name } | Select-Object -Last 1
    if (-not $best) { return @{ Ok = $false; Detail = "no 10.0.* with um\windows.h under $SdkIncludeRoot" } }
    return @{ Ok = $true; Path = ($best.FullName -replace '\\', '/'); Detail = "$($best.Name) at $($best.FullName)" }
}

function Test-GitLongPaths {
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) { return @{ Ok = $false; Detail = 'git not found' } }
    $v = & git config --get core.longpaths
    $global:LASTEXITCODE = 0
    if ($v -eq 'true') { return @{ Ok = $true; Detail = 'core.longpaths=true' } }
    $shown = if ($v) { $v } else { '<unset>' }
    return @{ Ok = $false; Detail = "core.longpaths=$shown" }
}

function Test-DiskSpace {
    $root = Get-BuildRoot
    $drive = [System.IO.DriveInfo]::new([System.IO.Path]::GetPathRoot([System.IO.Path]::GetFullPath($root)))
    $gb = [math]::Round($drive.AvailableFreeSpace / 1GB, 1)
    return @{ Ok = ($gb -ge $MinFreeGB); Detail = "$gb GB free on $($drive.Name) for $root (need >= $MinFreeGB GB)" }
}

# Runs one check; an exception becomes a MISSING result instead of aborting before the list is printed.
function Invoke-Check([scriptblock]$Test) {
    try { & $Test } catch { @{ Ok = $false; Detail = "error: $($_.Exception.Message)" } }
}

function Invoke-Checks {
    $vs = Invoke-Check { Test-VsBuildTools }
    [ordered]@{
        'vs-buildtools'   = $vs
        'cmake'           = Invoke-Check { Test-Cmake $vs['InstallPath'] }
        'strawberry-perl' = Invoke-Check { Test-StrawberryPerl }
        'pkg-config'      = Invoke-Check { Test-PkgConfig }
        'windows-sdk'     = Invoke-Check { Test-WindowsSdk }
        'git-longpaths'   = Invoke-Check { Test-GitLongPaths }
        'disk-space'      = Invoke-Check { Test-DiskSpace }
    }
}

function Invoke-Native([string]$What, [scriptblock]$Command) {
    Write-Host "INSTALL $What"
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
}

if (-not ($Check -or $Install)) { throw 'Specify -Check and/or -Install' }

$results = Invoke-Checks
if ($Install) {
    if (-not $results['strawberry-perl'].Ok) {
        Invoke-Native 'Strawberry Perl (winget)' { winget install --id StrawberryPerl.StrawberryPerl -e --accept-package-agreements --accept-source-agreements }
    }
    if (-not $results['pkg-config'].Ok) {
        Invoke-Native 'pkg-config-lite (winget)' { winget install --id bloodrock.pkg-config-lite -e --accept-package-agreements --accept-source-agreements }
    }
    if (-not $results['git-longpaths'].Ok -and (Get-Command git -ErrorAction SilentlyContinue)) {
        Invoke-Native 'git core.longpaths' { git config --global core.longpaths true }
    }
    $results = Invoke-Checks
}

foreach ($name in $results.Keys) {
    $r = $results[$name]
    $status = if ($r.Ok) { 'OK' } else { 'MISSING' }
    Write-Host "$status $name $($r.Detail)"
}

if ($Export) {
    if ($results['cmake'].Ok) { $env:BAMBURINO_CMAKE = $results['cmake'].Path }
    if ($results['windows-sdk'].Ok) { $env:BAMBURINO_WIN10SDK_PATH = $results['windows-sdk'].Path }
    # Drop Strawberry's MinGW bin, then prepend in reverse priority order so the CMake bin directory ends up first.
    $paths = @($env:Path -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ne $StrawberryMingwBin })
    foreach ($tool in 'pkg-config', 'strawberry-perl', 'cmake') {
        if (-not $results[$tool].Ok) { continue }
        $dir = Split-Path -Parent $results[$tool].Path
        $paths = @($dir) + @($paths | Where-Object { $_.TrimEnd('\') -ne $dir.TrimEnd('\') })
    }
    $env:Path = $paths -join ';'
}

$missing = @($results.Values | Where-Object { -not $_.Ok }).Count
if ($missing -eq 0) { exit 0 }
exit 10

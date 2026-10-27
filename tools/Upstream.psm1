Set-StrictMode -Version Latest

function Resolve-UpstreamPath {
    param([string]$Path)
    if ($Path) { return [string]$Path }
    if ($env:BAMBURINO_UPSTREAM_TOML) { return $env:BAMBURINO_UPSTREAM_TOML }
    return (Join-Path $PSScriptRoot '..\driver\upstream.toml')
}

function Get-Upstream {
    [CmdletBinding()]
    [OutputType([hashtable])]
    param([string]$Path)
    $file = Resolve-UpstreamPath $Path
    $result = @{}
    $section = ''
    foreach ($line in Get-Content -LiteralPath $file) {
        $t = $line.Trim()
        if ($t -eq '' -or $t.StartsWith('#')) { continue }
        if ($t -match '^\[([A-Za-z0-9_.-]+)\]$') { $section = $Matches[1]; continue }
        if ($t -match '^([A-Za-z0-9_-]+)\s*=\s*(?:"([^"]*)"|(-?\d+))\s*(?:#.*)?$') {
            $value = if ($null -ne $Matches[2] -and $Matches[2] -ne '' -or $t -match '=\s*""') { $Matches[2] } else { $Matches[3] }
            $result["$section.$($Matches[1])"] = $value
        }
    }
    return $result
}

function Set-UpstreamValue {
    [CmdletBinding()]
    param(
        [string]$Path,
        [Parameter(Mandatory)][string]$Key,
        [Parameter(Mandatory)]$Value
    )
    $file = Resolve-UpstreamPath $Path
    $dot = $Key.LastIndexOf('.')
    if ($dot -lt 1) { throw "Key must be 'section.name': $Key" }
    $wantSection = $Key.Substring(0, $dot)
    $wantName = $Key.Substring($dot + 1)
    $rendered = if ($Value -is [int] -or $Value -is [long]) { "$Value" } else { '"' + ([string]$Value) + '"' }
    $section = ''
    $found = $false
    $lines = foreach ($line in Get-Content -LiteralPath $file) {
        $t = $line.Trim()
        if ($t -match '^\[([A-Za-z0-9_.-]+)\]$') { $section = $Matches[1] }
        elseif (-not $found -and $section -eq $wantSection -and $t -match '^([A-Za-z0-9_-]+)\s*=' -and $Matches[1] -eq $wantName) {
            $found = $true
            "$wantName = $rendered"
            continue
        }
        $line
    }
    if (-not $found) { throw "Key not found: $Key" }
    [System.IO.File]::WriteAllText((Resolve-Path -LiteralPath $file).Path, (($lines -join "`n") + "`n"), (New-Object System.Text.UTF8Encoding($false)))
}

function Get-BuildRoot {
    [OutputType([string])]
    param()
    if ($env:BAMBURINO_BUILD_ROOT) { return $env:BAMBURINO_BUILD_ROOT }
    return 'D:\bb'
}

function Enter-VsEnv {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) { throw 'vswhere.exe not found: Visual Studio 2022 Build Tools are not installed' }
    $install = & $vswhere -latest -products * -version '[17.0,18.0)' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if (-not $install) { throw 'Visual Studio 2022 Build Tools with the C++ x64 toolset not found' }
    $dll = Join-Path $install 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll'
    Import-Module $dll
    Enter-VsDevShell -VsInstallPath $install -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64' | Out-Null
}

function Write-StepLog {
    [OutputType([string])]
    param([Parameter(Mandatory)][string]$Step)
    $dir = Join-Path (Get-BuildRoot) 'logs'
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    return (Join-Path $dir ("$Step-" + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.log'))
}

Export-ModuleMember -Function Get-Upstream, Set-UpstreamValue, Get-BuildRoot, Enter-VsEnv, Write-StepLog

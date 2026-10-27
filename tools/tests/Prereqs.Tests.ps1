# Runs prereqs.ps1 in-process: -Export sets session environment variables, which a child pwsh could not hand back.
# The disk-space check is tested with explicit thresholds (-MinFreeGB), not with the machine's current free space.
$script = Join-Path $PSScriptRoot '..\..\driver\prereqs.ps1'
$pathBefore = $env:Path

# A threshold no drive meets: exit 10, and disk-space is the only MISSING line.
$global:LASTEXITCODE = 0
$out = & $script -Check -MinFreeGB 10000 6>&1 | ForEach-Object { "$_" }
if ($LASTEXITCODE -ne 10) { throw "prereqs.ps1 -Check -MinFreeGB 10000: expected exit 10, got $LASTEXITCODE" }
$missing = @($out | Where-Object { $_ -like 'MISSING *' })
if ($missing.Count -ne 1 -or $missing[0] -notmatch '^MISSING disk-space .*need >= 10000 GB') { throw "expected only 'MISSING disk-space ... need >= 10000 GB', got: $($missing -join '; ')" }

# A threshold that holds on the build machine (the post-deps minimum, see build.ps1 Get-StepMinFreeGB).
$global:LASTEXITCODE = 0
$out = & $script -Check -Export -MinFreeGB 12 6>&1 | ForEach-Object { "$_" }
if ($LASTEXITCODE -ne 0) {
    $missing = @($out | Where-Object { $_ -like 'MISSING *' }) -join '; '
    throw "prereqs.ps1 -Check -MinFreeGB 12 exit $LASTEXITCODE`: $missing"
}
if (-not @($out | Where-Object { $_ -match '^OK disk-space .*need >= 12 GB' })) { throw 'no "OK disk-space ... need >= 12 GB" line' }

# Per-step thresholds in build.ps1: 25 before the first deps build, 12 once deps\build exists and for app.
$build = (Resolve-Path (Join-Path $PSScriptRoot '..\..\driver\build.ps1')).Path
$ast = [System.Management.Automation.Language.Parser]::ParseFile($build, [ref]$null, [ref]$null)
$fn = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Get-StepMinFreeGB' }, $true)
if (-not $fn) { throw 'driver/build.ps1 has no Get-StepMinFreeGB function' }
. ([scriptblock]::Create($fn.Extent.Text))
$tmpSrc = Join-Path ([System.IO.Path]::GetTempPath()) ('bamburino-src-' + [guid]::NewGuid().ToString('N'))
try {
    New-Item -ItemType Directory -Path $tmpSrc | Out-Null
    if ((Get-StepMinFreeGB -Step deps -Src $tmpSrc) -ne 25) { throw 'Get-StepMinFreeGB deps without deps\build should be 25' }
    if ((Get-StepMinFreeGB -Step app -Src $tmpSrc) -ne 12) { throw 'Get-StepMinFreeGB app should be 12' }
    New-Item -ItemType Directory -Path (Join-Path $tmpSrc 'deps\build') | Out-Null
    if ((Get-StepMinFreeGB -Step deps -Src $tmpSrc) -ne 12) { throw 'Get-StepMinFreeGB deps with deps\build should be 12' }
} finally {
    Remove-Item -LiteralPath $tmpSrc -Recurse -Force -ErrorAction SilentlyContinue
}

if (-not $env:BAMBURINO_CMAKE -or -not (Test-Path -LiteralPath $env:BAMBURINO_CMAKE -PathType Leaf)) {
    throw "BAMBURINO_CMAKE is not an existing file: '$env:BAMBURINO_CMAKE'"
}
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vsPaths = @(& $vswhere -products * -version '[17.0,18.0)' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
if (-not @($vsPaths | Where-Object { $env:BAMBURINO_CMAKE.StartsWith($_.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase) })) {
    throw "BAMBURINO_CMAKE is not under a VS 2022 installation ($($vsPaths -join ', ')): $env:BAMBURINO_CMAKE"
}
$sessionPath = @($env:Path -split ';' | Where-Object { $_ })
if ($sessionPath[0].TrimEnd('\') -ne (Split-Path -Parent $env:BAMBURINO_CMAKE)) { throw "PATH does not start with the CMake bin dir: $($sessionPath[0])" }
if (@($sessionPath | Where-Object { $_.TrimEnd('\') -eq 'C:\Strawberry\c\bin' })) { throw 'C:\Strawberry\c\bin is still on the session PATH' }
$gitRoot = Join-Path $env:ProgramFiles 'Git'
foreach ($deny in 'mingw64\bin', 'usr\bin', 'usr\local\bin') {
    $d = Join-Path $gitRoot $deny
    if (@($sessionPath | Where-Object { $_.TrimEnd('\') -eq $d })) { throw "$d is still on the session PATH" }
}
$gitCmd = Join-Path $gitRoot 'cmd'
if (@($pathBefore -split ';' | Where-Object { $_.TrimEnd('\') -eq $gitCmd }) -and -not @($sessionPath | Where-Object { $_.TrimEnd('\') -eq $gitCmd })) {
    throw "$gitCmd was removed from the session PATH (git must keep working)"
}
$cmakeOut = (& $env:BAMBURINO_CMAKE --version) -join "`n"
if ($cmakeOut -notmatch 'cmake version (\d+\.\d+\.\d+)') { throw "cannot parse cmake --version: $cmakeOut" }
if ([version]$Matches[1] -lt [version]'3.24.0') { throw "cmake $($Matches[1]) < 3.24" }

if (-not $env:BAMBURINO_WIN10SDK_PATH -or -not (Test-Path -LiteralPath $env:BAMBURINO_WIN10SDK_PATH -PathType Container)) {
    throw "BAMBURINO_WIN10SDK_PATH does not exist: '$env:BAMBURINO_WIN10SDK_PATH'"
}
if (-not (Test-Path -LiteralPath (Join-Path $env:BAMBURINO_WIN10SDK_PATH 'um\windows.h'))) {
    throw "no um\windows.h under $env:BAMBURINO_WIN10SDK_PATH"
}

# Strawberry Perl 5.42 names itself only in perl -V (uname='Win32 strawberry-perl ...'), not in perl -v.
$perlOut = (& perl -V) -join "`n"
if ($perlOut -notmatch 'Strawberry') { throw "perl on PATH is not Strawberry Perl: $((Get-Command perl).Source)" }
exit 0

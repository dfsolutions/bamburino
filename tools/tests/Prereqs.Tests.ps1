# Runs prereqs.ps1 in-process: -Export sets session environment variables, which a child pwsh could not hand back.
$script = Join-Path $PSScriptRoot '..\..\driver\prereqs.ps1'
$global:LASTEXITCODE = 0
$out = & $script -Check -Export 6>&1 | ForEach-Object { "$_" }
if ($LASTEXITCODE -ne 0) {
    $missing = @($out | Where-Object { $_ -like 'MISSING *' }) -join '; '
    throw "prereqs.ps1 -Check exit $LASTEXITCODE`: $missing"
}

if (-not $env:BAMBURINO_CMAKE -or -not (Test-Path -LiteralPath $env:BAMBURINO_CMAKE -PathType Leaf)) {
    throw "BAMBURINO_CMAKE is not an existing file: '$env:BAMBURINO_CMAKE'"
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

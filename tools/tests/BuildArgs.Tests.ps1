# Unit test for Get-MsBuildJobArgs in driver/build.ps1 (BAMBURINO_MSBUILD_JOBS): extracted via the AST, no build is run.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$build = (Resolve-Path (Join-Path $PSScriptRoot '..\..\driver\build.ps1')).Path
$ast = [System.Management.Automation.Language.Parser]::ParseFile($build, [ref]$null, [ref]$null)
$fn = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Get-MsBuildJobArgs' }, $true)
if (-not $fn) { throw 'Get-MsBuildJobArgs not found in driver/build.ps1' }
. ([scriptblock]::Create($fn.Extent.Text))

$saved = $env:BAMBURINO_MSBUILD_JOBS
try {
    $env:BAMBURINO_MSBUILD_JOBS = $null
    $a = @(Get-MsBuildJobArgs)
    if ($a[0] -ne '-m:8' -or $a[1] -ne '/p:CL_MPCount=8') { throw "default: expected -m:8 /p:CL_MPCount=8, got $($a -join ' ')" }
    $env:BAMBURINO_MSBUILD_JOBS = '4'
    $a = @(Get-MsBuildJobArgs)
    if ($a[0] -ne '-m:4' -or $a[1] -ne '/p:CL_MPCount=4') { throw "override 4: got $($a -join ' ')" }
    foreach ($bad in @('0', '65', 'abc', '-3', '4.5', '8 9')) {
        $env:BAMBURINO_MSBUILD_JOBS = $bad
        $threw = $false
        try { $null = Get-MsBuildJobArgs } catch { $threw = $true }
        if (-not $threw) { throw "invalid value '$bad' did not throw" }
    }
} finally { $env:BAMBURINO_MSBUILD_JOBS = $saved }

# Static: the deps, app and test msbuild command lines all go through the function; no bare '-m' / '/m' left.
$text = Get-Content -LiteralPath $build -Raw
if (([regex]::Matches($text, '\(Get-MsBuildJobArgs\)')).Count -ne 3) { throw 'expected 3 msbuild command lines using Get-MsBuildJobArgs (deps, app, test)' }
if ($text -match "'-m'|'/m'") { throw "build.ps1 still passes a bare -m or /m to msbuild" }
if ($text -notmatch "'--'\) \+ @\(Get-MsBuildJobArgs\) \+ @\('/v:minimal'") { throw 'app/test cmake --build lines do not pass the job args after --' }
Write-Host 'BuildArgs OK'

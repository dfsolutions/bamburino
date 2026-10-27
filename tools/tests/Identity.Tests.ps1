# Build identity (Task 5): after driver/build.ps1 -Step patch the clone exposes appVersion, buildSuffix and patchId
# in app.get_capabilities, the values come from CMake definitions passed by build.ps1, and version.inc is untouched.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module "$PSScriptRoot/../Upstream.psm1" -Force
$repo = (Resolve-Path "$PSScriptRoot/../..").Path
$build = Join-Path $repo 'driver/build.ps1'
$u = Get-Upstream
$tagCommit = $u['studio.tag_commit']
$patchId = $u['driver.patch_id']
$prefix = $u['driver.build_suffix_prefix']
if ($patchId -notmatch '^[0-9a-f]{12}$') { throw "driver.patch_id is not 12 hex: '$patchId'" }
if ($prefix -ne 'bamburino') { throw "driver.build_suffix_prefix is '$prefix', expected 'bamburino'" }
$clone = Join-Path (Get-BuildRoot) 'BambuStudio'
if (-not (Test-Path -LiteralPath (Join-Path $clone '.git'))) { throw "clone missing at ${clone}: run driver/build.ps1 -Step fetch first" }

# The divergence register names the patch.
$div = Join-Path $repo 'docs/driver-divergences.md'
if (-not (Test-Path -LiteralPath $div)) { throw 'docs/driver-divergences.md missing' }
if (-not (Select-String -LiteralPath $div -SimpleMatch -Quiet -Pattern '0100-build-identity.diff')) { throw 'docs/driver-divergences.md does not cite 0100-build-identity.diff' }

# build.ps1 builds both CMake definitions from upstream.toml (function used by -Step app).
$ast = [System.Management.Automation.Language.Parser]::ParseFile($build, [ref]$null, [ref]$null)
$fn = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Get-IdentityCMakeDefines' }, $true)
if (-not $fn) { throw 'driver/build.ps1 has no Get-IdentityCMakeDefines function' }
. ([scriptblock]::Create($fn.Extent.Text))
$defs = @(Get-IdentityCMakeDefines -Upstream $u)
$want = @("-DBAMBURINO_PATCH_ID=$patchId", "-DBAMBURINO_BUILD_SUFFIX=bamburino.$patchId")
if (($defs -join '|') -ne ($want -join '|')) { throw "Get-IdentityCMakeDefines returned '$($defs -join ' ')', expected '$($want -join ' ')'" }
$appCall = $ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Get-IdentityCMakeDefines' }, $true)
if (@($appCall).Count -lt 1) { throw 'driver/build.ps1 never calls Get-IdentityCMakeDefines' }

# The last -Step app log, when one exists, shows both definitions on the CMake command line.
$logDir = Join-Path (Get-BuildRoot) 'logs'
$appLog = if (Test-Path -LiteralPath $logDir) { Get-ChildItem -LiteralPath $logDir -Filter 'app-*.log' -File | Sort-Object Name | Select-Object -Last 1 } else { $null }
if ($appLog) {
    foreach ($d in $want) {
        if (-not (Select-String -LiteralPath $appLog.FullName -SimpleMatch -Quiet -Pattern $d)) { throw "$($appLog.Name) lacks $d" }
    }
}

# Patch the clone, then inspect it.
$out = & pwsh -NoProfile -NonInteractive -File $build -Step patch 2>&1
if ($LASTEXITCODE -ne 0) { throw "-Step patch exit ${LASTEXITCODE}: $($out -join "`n")" }

& git -C $clone diff --quiet $tagCommit -- version.inc
if ($LASTEXITCODE -ne 0) { throw 'version.inc differs from the tag' }

$tools = Get-Content -LiteralPath (Join-Path $clone 'src/slic3r/GUI/McpTools.cpp') -Raw
$m = [regex]::Match($tools, 'if \(method == "app\.get_capabilities"\) \{(?<body>.*?)\n    \}', 'Singleline')
if (-not $m.Success) { throw 'app.get_capabilities branch not found in McpTools.cpp' }
$body = $m.Groups['body'].Value
foreach ($pair in @('\{"appVersion", SLIC3R_VERSION\}', '\{"buildSuffix", BAMBURINO_BUILD_SUFFIX\}', '\{"patchId", BAMBURINO_PATCH_ID\}')) {
    if ($body -notmatch $pair) { throw "app.get_capabilities response lacks $pair" }
}

$cmake = Get-Content -LiteralPath (Join-Path $clone 'CMakeLists.txt') -Raw
foreach ($v in 'BAMBURINO_PATCH_ID', 'BAMBURINO_BUILD_SUFFIX') {
    if ($cmake -notmatch "add_compile_definitions\(\s*$v=`"\`$\{$v\}`"\s*\)") { throw "CMakeLists.txt lacks add_compile_definitions($v=`"`${$v}`")" }
    if ($cmake -notmatch "set\(\s*$v\s+`"unknown`"\s+CACHE\s+STRING") { throw "CMakeLists.txt lacks the 'unknown' default for $v" }
}

$about = Get-Content -LiteralPath (Join-Path $clone 'src/slic3r/GUI/AboutDialog.cpp') -Raw
if ($about -notmatch 'BAMBURINO_BUILD_SUFFIX') { throw 'AboutDialog.cpp does not show BAMBURINO_BUILD_SUFFIX' }

$root = Resolve-Path "$PSScriptRoot/.."
Set-Location $root
$problems = New-Object System.Collections.Generic.List[string]

$required = @('.gitignore', '.gitattributes', 'LICENSE', 'NOTICE.md', 'README.md',
    'driver/upstream.toml', 'tools/Upstream.psm1', 'tools/GitHub.psm1', 'tools/Package.psm1',
    'tools/tests/run.ps1', 'tools/tests/Upstream.Tests.ps1', 'ci/check-repo.ps1',
    'service/.gitkeep', 'catalog/.gitkeep', 'docs/.gitkeep')
foreach ($f in $required) {
    if (-not (Test-Path -LiteralPath $f)) { $problems.Add("missing file: $f") }
}

if (Test-Path LICENSE) {
    $lic = Get-Content LICENSE -Raw
    foreach ($s in 'GNU AFFERO GENERAL PUBLIC LICENSE', 'Version 3, 19 November 2007') {
        if ($lic -notmatch [regex]::Escape($s)) { $problems.Add("LICENSE lacks: $s") }
    }
}
if (Test-Path NOTICE.md) {
    $n = Get-Content NOTICE.md -Raw
    foreach ($s in 'Bambu Lab', 'PrusaSlicer', 'otarkhan', 'subhashdasyam', 'Ding-Ding-Projects') {
        if ($n -notmatch [regex]::Escape($s)) { $problems.Add("NOTICE.md lacks: $s") }
    }
}

$tracked = @(git ls-files)
foreach ($f in $tracked) {
    if ((Test-Path -LiteralPath $f) -and (Get-Item -LiteralPath $f).Length -gt 5MB) { $problems.Add("tracked file > 5 MB: $f") }
    if ($f -in 'ANALISI_AVVERSARIALE_Brief_v2.md', 'BambuStudio_MCP_Codex_Brief_v2.md' -or $f -like 'docs/superpowers/*') {
        $problems.Add("private file tracked: $f")
    }
}

if (-not (Test-Path .gitattributes) -or -not (Select-String -Path .gitattributes -SimpleMatch -Quiet -Pattern 'driver/patches/*.diff -text')) {
    $problems.Add('.gitattributes lacks line: driver/patches/*.diff -text')
}

if (Test-Path driver) {
    $allowed = 'upstream.toml', 'build.ps1', 'prereqs.ps1', 'patch-id.ps1'
    foreach ($item in Get-ChildItem driver -Force) {
        if ($item.PSIsContainer) {
            if ($item.Name -ne 'patches') { $problems.Add("unexpected in driver/: $($item.Name)") }
            else {
                foreach ($p in Get-ChildItem $item.FullName -Force) {
                    if ($p.PSIsContainer -or $p.Extension -ne '.diff') { $problems.Add("unexpected in driver/patches/: $($p.Name)") }
                }
            }
        } elseif ($item.Name -notin $allowed) { $problems.Add("unexpected in driver/: $($item.Name)") }
    }
}

if ($problems.Count -gt 0) {
    $problems | ForEach-Object { Write-Host "FAIL $_" }
    exit 1
}
Write-Host 'OK check-repo'
exit 0

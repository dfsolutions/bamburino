# Unit test for driver/build.ps1 -Step package (final review fixes): the identity guard on install-dir\BambuStudio.dll
# and the AGPL-3.0 section 6 files at the zip root (SOURCE.txt, LICENSE, NOTICE.md, install-studio.ps1 when present).
# Runs on temporary folders only: a fake install-dir of two small files, a copy of upstream.toml
# (BAMBURINO_UPSTREAM_TOML) and a temporary build root (BAMBURINO_BUILD_ROOT). It never builds and never touches D:\GoogleAI\BAMBU-MCP\build.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module "$PSScriptRoot/../Upstream.psm1" -Force
Import-Module "$PSScriptRoot/../Package.psm1" -Force
$repo = (Resolve-Path "$PSScriptRoot/../..").Path
$build = Join-Path $repo 'driver/build.ps1'
$u = Get-Upstream -Path (Join-Path $repo 'driver/upstream.toml')
$utf8 = New-Object System.Text.UTF8Encoding($false)

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ('bamburino-package-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
$savedToml = $env:BAMBURINO_UPSTREAM_TOML; $savedRoot = $env:BAMBURINO_BUILD_ROOT
try {
    # --- Marker ---
    $marker = Get-PackageMarker -Upstream $u
    if ($marker -ne "$($u['driver.build_suffix_prefix']).$($u['driver.patch_id'])") { throw "Get-PackageMarker returned '$marker'" }
    $bin = Join-Path $tmp 'marker.bin'
    [System.IO.File]::WriteAllBytes($bin, [byte[]](0, 1, 2) + [System.Text.Encoding]::ASCII.GetBytes('xx bamburino.0123456789ab yy') + [byte[]](0))
    if (-not (Test-BinaryMarker -Path $bin -Marker 'bamburino.0123456789ab')) { throw 'Test-BinaryMarker missed a present marker' }
    if (Test-BinaryMarker -Path $bin -Marker 'bamburino.ba9876543210') { throw 'Test-BinaryMarker matched an absent marker' }
    if (Test-BinaryMarker -Path (Join-Path $tmp 'absent.dll') -Marker 'x') { throw 'Test-BinaryMarker true on a missing file' }

    # --- Commit ---
    $nogit = Join-Path $tmp 'nogit'; New-Item -ItemType Directory -Force -Path $nogit | Out-Null
    $c = Get-BamburinoCommit -RepoRoot $nogit
    # %TEMP% may sit inside a git work tree on some machines; then any 40-hex value is acceptable.
    if ($c -ne 'unknown' -and $c -notmatch '^[0-9a-f]{40}$') { throw "Get-BamburinoCommit outside the repo returned '$c'" }
    $head = (& git -C $repo rev-parse HEAD).Trim()
    if ((Get-BamburinoCommit -RepoRoot $repo) -ne $head) { throw 'Get-BamburinoCommit does not return git rev-parse HEAD' }

    # --- SOURCE.txt and the copy list, real repo root (this branch has no ci/install-studio.ps1) ---
    $patches = [string[]]@(Get-ChildItem -LiteralPath (Join-Path $repo 'driver/patches') -Filter '*.diff' -File | ForEach-Object { $_.Name })
    [Array]::Sort($patches, [StringComparer]::Ordinal)
    if ((Get-PatchNames -RepoRoot $repo) -join '|' -ne ($patches -join '|')) { throw 'Get-PatchNames order/content mismatch' }
    $out1 = Join-Path $tmp 'extras1'
    $written = @(Write-PackageExtras -RepoRoot $repo -Destination $out1 -Upstream $u -Commit 'unknown')
    $hasInstaller = Test-Path -LiteralPath (Join-Path $repo 'ci/install-studio.ps1') -PathType Leaf
    $want = @('SOURCE.txt', 'LICENSE', 'NOTICE.md') + @(if ($hasInstaller) { 'install-studio.ps1' })
    if (($written -join '|') -ne ($want -join '|')) { throw "Write-PackageExtras wrote '$($written -join ', ')', expected '$($want -join ', ')'" }
    foreach ($n in $want) { if (-not (Test-Path -LiteralPath (Join-Path $out1 $n) -PathType Leaf)) { throw "missing $n in $out1" } }
    foreach ($n in 'LICENSE', 'NOTICE.md') {
        if ((Get-FileHash (Join-Path $out1 $n)).Hash -ne (Get-FileHash (Join-Path $repo $n)).Hash) { throw "$n is not a byte copy of the repo root file" }
    }
    $src = [System.IO.File]::ReadAllText((Join-Path $out1 'SOURCE.txt'))
    $must = @(
        'https://github.com/dfsolutions/bamburino',
        'Bamburino commit:     unknown',
        "studio.tag        = $($u['studio.tag'])",
        "studio.tag_commit = $($u['studio.tag_commit'])",
        "driver.patch_id   = $($u['driver.patch_id'])",
        "https://github.com/$($u['studio.repo']) (tag $($u['studio.tag']))"
    ) + @($patches | ForEach-Object { "  $_" })
    foreach ($m in $must) { if (-not $src.Contains($m)) { throw "SOURCE.txt lacks '$m'" } }

    # --- Copy list with an installer present (fake repo root) ---
    $fake = Join-Path $tmp 'fakerepo'
    New-Item -ItemType Directory -Force -Path (Join-Path $fake 'driver/patches'), (Join-Path $fake 'ci') | Out-Null
    foreach ($n in 'LICENSE', 'NOTICE.md') { [System.IO.File]::WriteAllText((Join-Path $fake $n), "fake $n`n", $utf8) }
    [System.IO.File]::WriteAllText((Join-Path $fake 'driver/patches/0001-a.diff'), "x`n", $utf8)
    [System.IO.File]::WriteAllText((Join-Path $fake 'ci/install-studio.ps1'), "# fake installer`n", $utf8)
    $written2 = @(Write-PackageExtras -RepoRoot $fake -Destination (Join-Path $tmp 'extras2') -Upstream $u)
    if (($written2 -join '|') -ne 'SOURCE.txt|LICENSE|NOTICE.md|install-studio.ps1') { throw "with installer: wrote '$($written2 -join ', ')'" }
    if (-not [System.IO.File]::ReadAllText((Join-Path $tmp 'extras2/SOURCE.txt')).Contains('Bamburino commit:     unknown')) {
        # The fake repo is not a git work tree (unless %TEMP% is inside one).
        if ((Get-BamburinoCommit -RepoRoot $fake) -eq 'unknown') { throw 'SOURCE.txt of a non-git root does not say unknown' }
    }
    Remove-Item -LiteralPath (Join-Path $fake 'NOTICE.md')
    $threw = $false
    try { Write-PackageExtras -RepoRoot $fake -Destination (Join-Path $tmp 'extras3') -Upstream $u | Out-Null } catch { $threw = $true }
    if (-not $threw) { throw 'Write-PackageExtras did not fail on a missing NOTICE.md' }

    # --- -Step package on a fake build root ---
    $root = Join-Path $tmp 'root'
    $install = Join-Path $root 'install-dir'
    New-Item -ItemType Directory -Force -Path $install | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $install 'bambu-studio.exe'), "fake exe`n", $utf8)
    $builtId = 'aaaaaaaaaaaa'
    [System.IO.File]::WriteAllText((Join-Path $install 'BambuStudio.dll'), "fake dll $($u['driver.build_suffix_prefix']).$builtId end`n", $utf8)
    $toml = Join-Path $tmp 'upstream.toml'
    Copy-Item -LiteralPath (Join-Path $repo 'driver/upstream.toml') -Destination $toml
    $env:BAMBURINO_BUILD_ROOT = $root
    $env:BAMBURINO_UPSTREAM_TOML = $toml

    # Mismatch: upstream.toml says another patch_id -> exit 50, nothing in artifacts.
    Set-UpstreamValue -Path $toml -Key 'driver.patch_id' -Value 'bbbbbbbbbbbb'
    $o = & pwsh -NoProfile -NonInteractive -File $build -Step package 2>&1
    $code = $LASTEXITCODE
    if ($code -ne 50) { throw "mismatched patch_id: -Step package exit $code, expected 50: $($o -join "`n")" }
    if (-not (($o -join "`n") -match "does not contain '[a-z-]+\.bbbbbbbbbbbb'")) { throw "mismatched patch_id: unclear message: $($o -join "`n")" }
    $art = Join-Path $root 'artifacts'
    if ((Test-Path -LiteralPath $art) -and @(Get-ChildItem -LiteralPath $art -File).Count -gt 0) { throw 'mismatched patch_id: artifacts were written' }

    # Match: the zip holds the install-dir files plus SOURCE.txt, LICENSE, NOTICE.md at its root.
    Set-UpstreamValue -Path $toml -Key 'driver.patch_id' -Value $builtId
    $o = & pwsh -NoProfile -NonInteractive -File $build -Step package 2>&1
    $code = $LASTEXITCODE
    if ($code -ne 0) { throw "matching patch_id: -Step package exit ${code}: $($o -join "`n")" }
    $zip = Join-Path $art "BambuStudio-bamburino-$($u['studio.version'])-$builtId-win64.zip"
    if (-not (Test-Path -LiteralPath $zip -PathType Leaf)) { throw "missing $zip" }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $z = [System.IO.Compression.ZipFile]::OpenRead($zip)
    try {
        $entries = @($z.Entries | ForEach-Object { $_.FullName })
        $source = $z.GetEntry('SOURCE.txt')
        $text = if ($source) { (New-Object System.IO.StreamReader($source.Open())).ReadToEnd() } else { '' }
    } finally { $z.Dispose() }
    foreach ($n in @('bambu-studio.exe', 'BambuStudio.dll') + $want) { if ($entries -notcontains $n) { throw "zip lacks $n (has: $($entries -join ', '))" } }
    if (-not $text.Contains("driver.patch_id   = $builtId")) { throw 'zip SOURCE.txt lacks the patch_id' }
    if (-not $text.Contains("Bamburino commit:     $head")) { throw 'zip SOURCE.txt lacks the Bamburino commit' }
    $recorded = (([System.IO.File]::ReadAllText("$zip.sha256").Trim() -split '\s+')[0])
    if ($recorded -ne (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()) { throw '.sha256 does not match the zip' }
    if (@(Get-ChildItem -LiteralPath $root -Directory -Filter 'tmp-package-extras-*').Count -ne 0) { throw 'staging folder left behind' }
} finally {
    $env:BAMBURINO_UPSTREAM_TOML = $savedToml; $env:BAMBURINO_BUILD_ROOT = $savedRoot
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

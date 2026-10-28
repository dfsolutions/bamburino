Set-StrictMode -Version Latest

# Helpers for driver/build.ps1 -Step package: the build identity guard on install-dir and the AGPL-3.0 section 6
# corresponding-source files added at the zip root. Kept in a module so tools/tests/Package.Tests.ps1 can test them on
# a temporary folder without building or zipping anything.

function Get-PackageMarker {
    # The build suffix the app step compiles into BambuStudio.dll: '<build_suffix_prefix>.<patch_id>'.
    [OutputType([string])]
    param([Parameter(Mandatory)][hashtable]$Upstream)
    $id = $Upstream['driver.patch_id']; $prefix = $Upstream['driver.build_suffix_prefix']
    if ($id -notmatch '^[0-9a-f]{12}$') { throw "upstream.toml: driver.patch_id is not 12 hex: '$id'" }
    if ($prefix -notmatch '^[a-z][a-z0-9-]*$') { throw "upstream.toml: driver.build_suffix_prefix invalid: '$prefix'" }
    return "$prefix.$id"
}

function Test-BinaryMarker {
    # True when the file's bytes contain $Marker (ASCII), false otherwise (also when the file is missing).
    [OutputType([bool])]
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Marker)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $false }
    $latin1 = [System.Text.Encoding]::Latin1
    return $latin1.GetString([System.IO.File]::ReadAllBytes($Path)).Contains($Marker)
}

function Get-BamburinoCommit {
    # 'git rev-parse HEAD' of the Bamburino checkout, or 'unknown' outside a git work tree.
    [OutputType([string])]
    param([Parameter(Mandatory)][string]$RepoRoot)
    $out = & git -C $RepoRoot rev-parse HEAD 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $out) { $global:LASTEXITCODE = 0; return 'unknown' }
    $sha = ([string]($out | Select-Object -First 1)).Trim()
    if ($sha -notmatch '^[0-9a-f]{40}$') { return 'unknown' }
    return $sha
}

function New-SourceInfoText {
    # Content of SOURCE.txt (AGPL-3.0 section 6: where the corresponding source of this binary is).
    [OutputType([string])]
    param(
        [Parameter(Mandatory)][hashtable]$Upstream,
        [Parameter(Mandatory)][string]$Commit,
        [Parameter(Mandatory)][AllowEmptyCollection()][string[]]$PatchNames
    )
    $repo = $Upstream['studio.repo']; $tag = $Upstream['studio.tag']
    $lines = @(
        'Bamburino driver build of Bambu Studio: corresponding source (AGPL-3.0, section 6)'
        ''
        'Bamburino repository: https://github.com/dfsolutions/bamburino'
        "Bamburino commit:     $Commit"
        ''
        'driver/upstream.toml:'
        "  studio.tag        = $tag"
        "  studio.tag_commit = $($Upstream['studio.tag_commit'])"
        "  driver.patch_id   = $($Upstream['driver.patch_id'])"
        ''
        "Upstream Bambu Studio: https://github.com/$repo (tag $tag)"
        "  https://github.com/$repo/tree/$tag"
        ''
        'Patches applied on that tag, in this order (driver/patches/ in the Bamburino repository):'
    )
    $lines += @($PatchNames | ForEach-Object { "  $_" })
    $lines += @(
        ''
        'To rebuild: check out the Bamburino commit above and run driver/build.ps1 -Step all (Windows, VS 2022).'
    )
    return (($lines -join "`n") + "`n")
}

function Get-PatchNames {
    # driver/patches/*.diff names in ordinal order (the order -Step patch applies them in).
    [OutputType([string[]])]
    param([Parameter(Mandatory)][string]$RepoRoot)
    $dir = Join-Path $RepoRoot 'driver\patches'
    $names = [string[]]@(Get-ChildItem -LiteralPath $dir -Filter '*.diff' -File | ForEach-Object { $_.Name })
    [Array]::Sort($names, [StringComparer]::Ordinal)
    return $names
}

function Write-PackageExtras {
    # Writes the files that go at the zip root next to the install-dir contents into $Destination:
    # SOURCE.txt, LICENSE and NOTICE.md (from the repo root), and ci/install-studio.ps1 when this checkout has it.
    # Returns the written file names. Throws when LICENSE or NOTICE.md is missing.
    [OutputType([string[]])]
    param(
        [Parameter(Mandatory)][string]$RepoRoot,
        [Parameter(Mandatory)][string]$Destination,
        [Parameter(Mandatory)][hashtable]$Upstream,
        [string]$Commit
    )
    if (-not $Commit) { $Commit = Get-BamburinoCommit -RepoRoot $RepoRoot }
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    $text = New-SourceInfoText -Upstream $Upstream -Commit $Commit -PatchNames (Get-PatchNames -RepoRoot $RepoRoot)
    [System.IO.File]::WriteAllText((Join-Path $Destination 'SOURCE.txt'), $text, (New-Object System.Text.UTF8Encoding($false)))
    $written = @('SOURCE.txt')
    foreach ($name in 'LICENSE', 'NOTICE.md') {
        $src = Join-Path $RepoRoot $name
        if (-not (Test-Path -LiteralPath $src -PathType Leaf)) { throw "missing $src (required in the package for AGPL-3.0)" }
        Copy-Item -LiteralPath $src -Destination (Join-Path $Destination $name) -Force
        $written += $name
    }
    $installer = Join-Path $RepoRoot 'ci\install-studio.ps1'
    if (Test-Path -LiteralPath $installer -PathType Leaf) {
        Copy-Item -LiteralPath $installer -Destination (Join-Path $Destination 'install-studio.ps1') -Force
        $written += 'install-studio.ps1'
    } else {
        Write-Host 'installer not present on this branch, added at integration (ci/install-studio.ps1)'
    }
    return [string[]]$written
}

Export-ModuleMember -Function Get-PackageMarker, Test-BinaryMarker, Get-BamburinoCommit, New-SourceInfoText, Get-PatchNames, Write-PackageExtras

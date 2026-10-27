<#
.SYNOPSIS
    Builds the Bambu Studio driver (upstream + Bamburino patch set) step by step.
.DESCRIPTION
    -Step fetch  Shallow-clones studio.repo at studio.tag into <BuildRoot>\BambuStudio and checks that
                 HEAD == studio.tag_commit. If the folder already exists, is clean and sits on that commit,
                 nothing is done. If it exists but is dirty (tracked or untracked changes), exits 20 without
                 touching it. If it exists clean but on another commit or is not a git work tree, exits 21.
    Exit codes: 0 ok, 20 clone dirty, 21 clone at an unexpected state, 1 any other error.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('fetch')][string]$Step
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot '..\tools\Upstream.psm1') -Force

function Invoke-Git {
    param([Parameter(ValueFromRemainingArguments)][string[]]$GitArgs)
    $out = & git @GitArgs 2>&1
    if ($LASTEXITCODE -ne 0) { throw "git $($GitArgs -join ' ') failed (exit $LASTEXITCODE): $($out -join "`n")" }
    return $out
}

function Invoke-Fetch {
    $u = Get-Upstream
    $repo = $u['studio.repo']; $tag = $u['studio.tag']; $want = $u['studio.tag_commit']
    if (-not $repo -or -not $tag -or $want -notmatch '^[0-9a-f]{40}$') { throw 'upstream.toml: studio.repo/tag/tag_commit not set' }
    $src = Join-Path (Get-BuildRoot) 'BambuStudio'
    $url = "https://github.com/$repo.git"

    if (Test-Path -LiteralPath $src) {
        # The folder must be the top of its own work tree, not a plain folder inside another repository.
        $top = & git -C $src rev-parse --show-toplevel 2>$null
        $srcFull = [System.IO.Path]::GetFullPath($src).TrimEnd([char]92, [char]47)
        $topFull = if ($LASTEXITCODE -eq 0 -and $top) { [System.IO.Path]::GetFullPath(($top | Select-Object -First 1).Trim()).TrimEnd([char]92, [char]47) } else { '' }
        if ($topFull -ine $srcFull) {
            Write-Host "ERROR $src exists but is not the root of a git work tree (partial or failed clone?). Delete the folder and rerun -Step fetch."
            exit 21
        }
        $dirty = Invoke-Git -C $src status --porcelain --untracked-files=normal
        if ($dirty) { Write-Host "DIRTY $src has local changes; not touching it"; exit 20 }
        $head = (Invoke-Git -C $src rev-parse HEAD | Select-Object -First 1).Trim()
        if ($head -ne $want) { Write-Host "ERROR $src is clean but at $head, expected $want"; exit 21 }
        Write-Host "OK fetch: $src already at $want (clean), not recloning"
        return
    }

    New-Item -ItemType Directory -Force -Path (Get-BuildRoot) | Out-Null
    $log = Write-StepLog -Step 'fetch'
    Write-Host "Cloning $url ($tag) into $src; log: $log"
    & git -c core.longpaths=true clone --depth 1 --branch $tag $url $src *> $log
    if ($LASTEXITCODE -ne 0) { throw "git clone failed (exit $LASTEXITCODE), see $log. If $src was left behind, delete it before rerunning -Step fetch." }
    Invoke-Git -C $src config core.longpaths true | Out-Null
    $head = (Invoke-Git -C $src rev-parse HEAD | Select-Object -First 1).Trim()
    if ($head -ne $want) { throw "cloned $tag but HEAD is $head, expected $want (tag moved?). Delete $src before rerunning." }
    Write-Host "OK fetch: $src at $want"
}

switch ($Step) {
    'fetch' { Invoke-Fetch }
}
exit 0

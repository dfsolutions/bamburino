<#
.SYNOPSIS
    Builds the Bambu Studio driver (upstream + Bamburino patch set) step by step.
.DESCRIPTION
    -Step fetch  Shallow-clones studio.repo at studio.tag into <BuildRoot>\BambuStudio and checks that
                 HEAD == studio.tag_commit. If the folder already exists, is clean and sits on that commit,
                 nothing is done. If it exists but is dirty (tracked or untracked changes), exits 20 without
                 touching it. If it exists clean but on another commit or is not a git work tree, exits 21.
    -Step patch  Recomputes patch_id (driver/patch-id.ps1); if it differs from driver.patch_id in upstream.toml,
                 exits 21 without touching the clone. Otherwise resets the clone to studio.tag_commit
                 (git checkout -f + git clean -fdx, keeping /deps/build and /build), then applies every
                 driver/patches/*.diff in ordinal name order (git apply --check, then git apply; both with
                 --index so new files are tracked and 'git diff <tag_commit>' sees the whole patch set).
                 If a --check fails, exits 20 and writes the failing file and its rejected hunks
                 (git apply --reject on a temporary copy of the touched files) to the step log.
                 Idempotent: every run starts again from the reset tree. Before the reset, the clone's
                 'git status --porcelain' is written to the step log, so discarded local edits stay visible.
                 Also prints the build identity CMake definitions (Get-IdentityCMakeDefines) for that patch_id.
    -Step deps   Builds the upstream dependencies (deps/CMakeLists.txt) into <BuildRoot>\deps-out (DESTDIR; the prefix
                 is deps-out\usr\local). Cache contract (plan C): deps-out\.bamburino-deps-key holds the git tree SHA-1
                 of deps/ at studio.tag_commit ('git rev-parse <tag_commit>:deps'); when it matches, the step is a no-op
                 (checked before any toolchain setup). Otherwise the sentinel is removed, the toolchain is set up
                 in-process (Enter-VsEnv, then prereqs.ps1 -Check -Export, so the VS CMake comes first and Strawberry's
                 MinGW bin is off PATH), and in <clone>\deps\build it runs
                 cmake .. -G "Visual Studio 17 2022" -A x64 -DDESTDIR=<BuildRoot>\deps-out -DCMAKE_BUILD_TYPE=Release
                 -DDEP_DEBUG=OFF, then msbuild /m ALL_BUILD.vcxproj /p:Configuration=Release /v:minimal, with the whole
                 output in the step log. The sentinel is written only after both succeed. deps-out is not wiped on a
                 mismatch: deps\build keeps the ExternalProject stamps, so a re-run after a failure resumes there.
                 Touches only <clone>\deps\build (kept by -Step patch) and deps-out, never the rest of the clone.
    Free disk: every step that calls prereqs.ps1 passes -MinFreeGB (Get-StepMinFreeGB): 25 GB only before the
    first deps build (deps without <clone>\deps\build: the peak), 12 GB for deps once deps\build exists (resume)
    and for the app step (Task 8 must call Get-StepMinFreeGB -Step app).
    Step order: -Step fetch is only for an absent or clean clone (a patched clone is dirty, so fetch exits 20
    on it by design). -Step patch does its own reset and never needs fetch again. The deps and app steps never
    call fetch; an 'all' step runs fetch only when the clone is missing.
    Build identity: the app step must pass Get-IdentityCMakeDefines to CMake (-DBAMBURINO_PATCH_ID=<patch_id>
    -DBAMBURINO_BUILD_SUFFIX=<build_suffix_prefix>.<patch_id>) and write its CMake command line to the step log.
    Patch 0100-build-identity.diff writes them into a generated header (bamburino_identity.h, "unknown" when not
    passed) read only by app.get_capabilities (buildSuffix, patchId) and the About dialog; SLIC3R_VERSION and
    version.inc are not changed.
    Exit codes: 0 ok; 20 clone dirty (fetch) or patch does not apply (patch);
                21 clone at an unexpected state (fetch, patch, deps) or patch_id mismatch (patch);
                10 prerequisites missing (deps); 30 cmake configure or msbuild failed (deps); 1 any other error.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('fetch', 'patch', 'deps')][string]$Step
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

function Test-CloneRoot {
    # True when $Path is the top of its own git work tree, not a plain folder inside another repository.
    param([Parameter(Mandatory)][string]$Path)
    $top = & git -C $Path rev-parse --show-toplevel 2>$null
    $full = [System.IO.Path]::GetFullPath($Path).TrimEnd([char]92, [char]47)
    $topFull = if ($LASTEXITCODE -eq 0 -and $top) { [System.IO.Path]::GetFullPath(($top | Select-Object -First 1).Trim()).TrimEnd([char]92, [char]47) } else { '' }
    return ($topFull -ieq $full)
}

function Invoke-Fetch {
    $u = Get-Upstream
    $repo = $u['studio.repo']; $tag = $u['studio.tag']; $want = $u['studio.tag_commit']
    if (-not $repo -or -not $tag -or $want -notmatch '^[0-9a-f]{40}$') { throw 'upstream.toml: studio.repo/tag/tag_commit not set' }
    $src = Join-Path (Get-BuildRoot) 'BambuStudio'
    $url = "https://github.com/$repo.git"

    if (Test-Path -LiteralPath $src) {
        if (-not (Test-CloneRoot $src)) {
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

function Get-IdentityCMakeDefines {
    # CMake definitions carrying the build identity (contract with plan B DriverCapabilities: buildSuffix, patchId).
    param([Parameter(Mandatory)][hashtable]$Upstream)
    $id = $Upstream['driver.patch_id']; $prefix = $Upstream['driver.build_suffix_prefix']
    if ($id -notmatch '^[0-9a-f]{12}$') { throw "upstream.toml: driver.patch_id is not 12 hex: '$id'" }
    if ($prefix -notmatch '^[a-z][a-z0-9-]*$') { throw "upstream.toml: driver.build_suffix_prefix invalid: '$prefix'" }
    return @("-DBAMBURINO_PATCH_ID=$id", "-DBAMBURINO_BUILD_SUFFIX=$prefix.$id")
}

function Get-PatchFiles {
    # driver/patches/*.diff in ordinal (byte-wise) name order: the same order patch-id.ps1 hashes them in.
    $dir = Join-Path $PSScriptRoot 'patches'
    $byName = @{}
    foreach ($f in @(Get-ChildItem -LiteralPath $dir -Filter '*.diff' -File)) { $byName[$f.Name] = $f.FullName }
    $names = [string[]]@($byName.Keys)
    [Array]::Sort($names, [StringComparer]::Ordinal)
    return @($names | ForEach-Object { $byName[$_] })
}

function Add-Log {
    param([Parameter(Mandatory)][string]$Log, $Lines)
    $text = @($Lines | Where-Object { $null -ne $_ } | ForEach-Object { "$_" })
    if ($text.Count -gt 0) { Add-Content -LiteralPath $Log -Value $text }
}

function Write-RejectReport {
    # Applies $Diff with --reject to a temporary copy of the files it touches and appends the rejected hunks to $Log.
    param([Parameter(Mandatory)][string]$Src, [Parameter(Mandatory)][string]$Diff, [Parameter(Mandatory)][string]$Log)
    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ('bamburino-reject-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        foreach ($line in [System.IO.File]::ReadLines($Diff)) {
            if ($line -match '^diff --git a/(\S+) b/') {
                $from = Join-Path $Src $Matches[1]
                if (Test-Path -LiteralPath $from -PathType Leaf) {
                    $to = Join-Path $tmp $Matches[1]
                    New-Item -ItemType Directory -Force -Path (Split-Path $to -Parent) | Out-Null
                    Copy-Item -LiteralPath $from -Destination $to
                }
            }
        }
        # Make the copy its own repository root, so git apply does not resolve paths against an enclosing repo.
        & git -C $tmp init -q 2>&1 | Out-Null
        $out = & git -C $tmp apply --reject --whitespace=nowarn $Diff 2>&1
        Add-Log $Log "== git apply --reject $([System.IO.Path]::GetFileName($Diff)) (temporary copy) exit $LASTEXITCODE"
        Add-Log $Log $out
        $rejects = @(Get-ChildItem -LiteralPath $tmp -Recurse -File -Filter '*.rej' | Sort-Object FullName)
        foreach ($r in $rejects) {
            $rel = ($r.FullName.Substring($tmp.Length + 1) -replace '\\', '/') -replace '\.rej$', ''
            $hunks = @(Select-String -LiteralPath $r.FullName -Pattern '^@@ ').Count
            Add-Log $Log "== REJECTED $rel ($hunks hunk(s))"
            Add-Log $Log ([System.IO.File]::ReadAllLines($r.FullName))
            Write-Host "  rejected: $rel ($hunks hunk(s))"
        }
        if ($rejects.Count -eq 0) { Write-Host '  no .rej produced (failure is not hunk-level, e.g. a missing or already existing file); see log' }
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Invoke-Patch {
    $u = Get-Upstream
    $want = $u['studio.tag_commit']; $pinned = $u['driver.patch_id']
    if ($want -notmatch '^[0-9a-f]{40}$') { throw 'upstream.toml: studio.tag_commit not set' }
    $actual = ([string](& (Join-Path $PSScriptRoot 'patch-id.ps1') | Select-Object -Last 1)).Trim()
    if ($actual -ne $pinned) {
        Write-Host "ERROR patch_id mismatch: driver/patches hash to $actual, upstream.toml driver.patch_id is '$pinned'. Clone not touched. Run driver/patch-id.ps1 -Write only if the patch change is intended."
        exit 21
    }

    $src = Join-Path (Get-BuildRoot) 'BambuStudio'
    if (-not (Test-Path -LiteralPath $src) -or -not (Test-CloneRoot $src)) {
        Write-Host "ERROR $src is missing or not the root of a git work tree. Run driver/build.ps1 -Step fetch first."
        exit 21
    }
    & git -C $src cat-file -e "$want^{commit}" 2>$null
    if ($LASTEXITCODE -ne 0) { Write-Host "ERROR $src does not contain commit $want. Delete it and rerun -Step fetch."; exit 21 }

    $log = Write-StepLog -Step 'patch'
    $before = & git -C $src status --porcelain --untracked-files=normal 2>&1
    Add-Log $log "== git status --porcelain before reset (these changes are discarded)"
    Add-Log $log $before
    Write-Host "Resetting $src to $want ($(@($before).Count) status line(s) discarded); log: $log"
    Invoke-Git -C $src checkout --quiet --force --detach $want | Out-Null
    Invoke-Git -C $src clean --quiet -fdx --exclude=/deps/build --exclude=/build | Out-Null
    $left = Invoke-Git -C $src status --porcelain --untracked-files=all
    if ($left) { throw "reset of $src left changes behind: $($left -join '; ')" }
    $head = (Invoke-Git -C $src rev-parse HEAD | Select-Object -First 1).Trim()
    if ($head -ne $want) { throw "after checkout HEAD is $head, expected $want" }

    $patches = @(Get-PatchFiles)
    if ($patches.Count -eq 0) { throw 'no driver/patches/*.diff to apply' }
    foreach ($p in $patches) {
        $name = [System.IO.Path]::GetFileName($p)
        $out = & git -C $src apply --check --index --whitespace=nowarn $p 2>&1
        $code = $LASTEXITCODE
        Add-Log $log "== git apply --check $name exit $code"
        Add-Log $log $out
        if ($code -ne 0) {
            Write-Host "PATCH FAILED $name does not apply on $want; rejected hunks in $log"
            Write-RejectReport -Src $src -Diff $p -Log $log
            exit 20
        }
        $out = & git -C $src apply --index --whitespace=nowarn $p 2>&1
        $code = $LASTEXITCODE
        Add-Log $log "== git apply $name exit $code"
        Add-Log $log $out
        if ($code -ne 0) { throw "git apply $name failed after a passing --check (exit $code), see $log" }
        Write-Host "applied $name"
    }
    $identity = Get-IdentityCMakeDefines -Upstream $u
    Add-Log $log "== build identity for -Step app: $($identity -join ' ')"
    Write-Host "OK patch: $($patches.Count) patch(es) applied on $want (patch_id $actual); identity: $($identity -join ' ')"
}

function Get-DepsKey {
    # Git tree SHA-1 of deps/ at studio.tag_commit: the cache key of the dependency build.
    param([Parameter(Mandatory)][string]$Src, [Parameter(Mandatory)][string]$Commit)
    $key = ([string](Invoke-Git -C $Src rev-parse "${Commit}:deps" | Select-Object -First 1)).Trim()
    if ($key -notmatch '^[0-9a-f]{40}$') { throw "git rev-parse ${Commit}:deps returned '$key'" }
    return $key
}

function Get-StepMinFreeGB {
    # Free space (GB) prereqs.ps1 -MinFreeGB must require on the build root drive for a step.
    # 25 only for the first deps build (peak); 12 for a deps resume/no-op and for the app step.
    param([Parameter(Mandatory)][ValidateSet('deps', 'app')][string]$Step, [string]$Src = (Join-Path (Get-BuildRoot) 'BambuStudio'))
    if ($Step -eq 'deps' -and -not (Test-Path -LiteralPath (Join-Path $Src 'deps\build') -PathType Container)) { return 25 }
    return 12
}

function Invoke-Deps {
    $u = Get-Upstream
    $want = $u['studio.tag_commit']
    if ($want -notmatch '^[0-9a-f]{40}$') { throw 'upstream.toml: studio.tag_commit not set' }
    $root = Get-BuildRoot
    $src = Join-Path $root 'BambuStudio'
    if (-not (Test-Path -LiteralPath $src) -or -not (Test-CloneRoot $src)) {
        Write-Host "ERROR $src is missing or not the root of a git work tree. Run driver/build.ps1 -Step fetch and -Step patch first."
        exit 21
    }
    $key = Get-DepsKey -Src $src -Commit $want
    $out = Join-Path $root 'deps-out'
    $keyFile = Join-Path $out '.bamburino-deps-key'
    if ((Test-Path -LiteralPath $keyFile -PathType Leaf) -and ([System.IO.File]::ReadAllText($keyFile).Trim() -eq $key)) {
        Write-Host "OK deps: $out already built for deps tree $key, nothing to do"
        return
    }
    if (Test-Path -LiteralPath $keyFile) { Remove-Item -LiteralPath $keyFile -Force }

    # Toolchain, in-process: VS developer environment first, then prereqs -Export, which puts the VS CMake first and
    # strips Strawberry's MinGW bin from the PATH that Enter-VsDevShell inherited from the Machine PATH.
    $minFree = Get-StepMinFreeGB -Step deps -Src $src
    Enter-VsEnv
    & (Join-Path $PSScriptRoot 'prereqs.ps1') -Check -Export -MinFreeGB $minFree
    if ($LASTEXITCODE -ne 0) { Write-Host "ERROR prerequisites missing (prereqs.ps1 exit $LASTEXITCODE)"; exit 10 }
    $cmake = $env:BAMBURINO_CMAKE
    if (-not $cmake -or -not (Test-Path -LiteralPath $cmake -PathType Leaf)) { throw "BAMBURINO_CMAKE not set by prereqs.ps1 -Export: '$cmake'" }
    $msbuild = (Get-Command msbuild.exe -ErrorAction Stop).Source

    $buildDir = Join-Path $src 'deps\build'
    New-Item -ItemType Directory -Force -Path $buildDir, $out | Out-Null
    $log = Write-StepLog -Step 'deps'
    $cmakeArgs = @('..', '-G', 'Visual Studio 17 2022', '-A', 'x64', "-DDESTDIR=$out", '-DCMAKE_BUILD_TYPE=Release', '-DDEP_DEBUG=OFF')
    $msbuildArgs = @('/m', 'ALL_BUILD.vcxproj', '/p:Configuration=Release', '/v:minimal')
    Add-Log $log "== deps tree $key (studio.tag_commit $want)"
    Add-Log $log "== prereqs.ps1 -MinFreeGB $minFree"
    Add-Log $log "== PATH=$env:Path"
    Add-Log $log "== cwd $buildDir"
    Add-Log $log "== $cmake $($cmakeArgs -join ' ')"
    Write-Host "Building deps for tree $key into $out; log: $log"
    $t0 = Get-Date
    Push-Location -LiteralPath $buildDir
    try {
        & $cmake @cmakeArgs *>> $log
        $code = $LASTEXITCODE
        Add-Log $log "== cmake configure exit $code"
        if ($code -ne 0) { Write-Host "ERROR cmake configure failed (exit $code), see $log"; exit 30 }
        Add-Log $log "== $msbuild $($msbuildArgs -join ' ')"
        & $msbuild @msbuildArgs *>> $log
        $code = $LASTEXITCODE
        $mins = [math]::Round(((Get-Date) - $t0).TotalMinutes, 1)
        Add-Log $log "== msbuild exit $code after $mins min"
        if ($code -ne 0) { Write-Host "ERROR msbuild failed (exit $code) after $mins min, see $log"; exit 30 }
    } finally {
        Pop-Location
    }
    [System.IO.File]::WriteAllText($keyFile, "$key`n", (New-Object System.Text.UTF8Encoding($false)))
    Write-Host "OK deps: built into $out in $mins min; sentinel $keyFile = $key"
}

switch ($Step) {
    'fetch' { Invoke-Fetch }
    'patch' { Invoke-Patch }
    'deps' { Invoke-Deps }
}
exit 0

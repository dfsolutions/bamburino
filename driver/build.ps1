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
                 -DDEP_DEBUG=OFF, then msbuild -m:<jobs> ALL_BUILD.vcxproj /p:Configuration=Release /v:minimal /nodeReuse:false, with the whole
                 output in the step log. The sentinel is written only after both succeed. deps-out is not wiped on a
                 mismatch: deps\build keeps the ExternalProject stamps, so a re-run after a failure resumes there.
                 Touches only <clone>\deps\build (kept by -Step patch) and deps-out, never the rest of the clone.
    -Step app    Builds and installs the patched app. Requires a patched clone on studio.tag_commit (else exit 21) and the
                 deps sentinel (else exit 21); sets up the toolchain in-process like deps (exit 10 when prerequisites
                 are missing). In <clone>\build it runs, in the same step every time,
                 cmake .. -G "Visual Studio 17 2022" -A x64 -DBBL_RELEASE_TO_PUBLIC=1 -DBBL_INTERNAL_TESTING=0
                 -DCMAKE_PREFIX_PATH=<BuildRoot>\deps-out\usr\local -DCMAKE_INSTALL_PREFIX=<BuildRoot>\install-dir
                 -DCMAKE_BUILD_TYPE=Release -DWIN10SDK_PATH=<BAMBURINO_WIN10SDK_PATH> <Get-IdentityCMakeDefines>
                 -DPKG_CONFIG_EXECUTABLE=<PKG_CONFIG_EXECUTABLE from prereqs.ps1 -Export>
                 then cmake --build . --target install --config Release -- -m:<jobs> /p:CL_MPCount=<jobs> /v:minimal /nodeReuse:false
                 (/nodeReuse:false is an msbuild switch, not a cmake flag, so it goes after '--': no idle MSBuild
                 worker nodes stay behind once the step ends). The configure command line
                 is written to the step log as '== cmake configure: ...'. Exit 40 when configure or build fails or
                 install-dir\bambu-studio.exe is missing afterwards.
    -Step test   PR #12485 standalone MCP tests: cmake -S tests/mcp -B build-mcp-tests (in the clone) against the deps
                 prefix, build Release, ctest -C Release --output-on-failure, then python
                 tests/mcp/test_legacy_contract.py -v, run on an LF export of the clone's index (git checkout-index
                 into a temporary folder under the build root; the test pins the oracle's SHA-256 over LF bytes and the
                 work tree may be CRLF). Python is needed on the build machine only. Exit 45 on any failure.
    -Step package  Zips the contents of <BuildRoot>\install-dir into
                 <BuildRoot>\artifacts\BambuStudio-bamburino-<studio.version>-<patch_id>-win64.zip and writes
                 <zip>.sha256 (sha256sum format). Before zipping it checks that install-dir\BambuStudio.dll contains
                 '<build_suffix_prefix>.<patch_id>' for the patch_id in upstream.toml (exit 50 otherwise: a stale or
                 foreign build would get a misleading name). At the zip root it adds (tools/Package.psm1) SOURCE.txt
                 (AGPL-3.0 section 6: Bamburino repo URL and commit, upstream.toml tag/tag_commit/patch_id, the
                 driver/patches/*.diff list, the upstream Bambu Studio URL and tag), LICENSE and NOTICE.md from the
                 repo root, and ci/install-studio.ps1 as install-studio.ps1 when this checkout has it.
                 Exit 50 on any failure (including a missing bambu-studio.exe).
    -Step all    prereqs -> fetch -> patch -> deps -> app -> test -> package, each in its own pwsh process; fetch
                 runs only when the clone folder is absent. The prereqs check uses the app threshold (12 GB) when the
                 deps sentinel exists, else the deps one (Get-StepMinFreeGB). Stops at the first non-zero exit code and returns it.
    Free disk: every step that calls prereqs.ps1 passes -MinFreeGB (Get-StepMinFreeGB): 25 GB only before the
    first deps build (deps without <clone>\deps\build: the peak), 12 GB for deps once deps\build exists (resume)
    and for the app step; 2 GB for the test step.
    Step order: -Step fetch is only for an absent or clean clone (a patched clone is dirty, so fetch exits 20
    on it by design). -Step patch does its own reset and never needs fetch again. The deps and app steps never
    call fetch; an 'all' step runs fetch only when the clone is missing.
    Build identity: the app step must pass Get-IdentityCMakeDefines to CMake (-DBAMBURINO_PATCH_ID=<patch_id>
    -DBAMBURINO_BUILD_SUFFIX=<build_suffix_prefix>.<patch_id>) and write its CMake command line to the step log.
    Patch 0100-build-identity.diff writes them into a generated header (bamburino_identity.h, "unknown" when not
    passed) read only by app.get_capabilities (buildSuffix, patchId) and the About dialog; SLIC3R_VERSION and
    version.inc are not changed.
    Parallelism: every msbuild invocation (deps, app, test) gets -m:<jobs> /p:CL_MPCount=<jobs>, where <jobs> is the
                 environment variable BAMBURINO_MSBUILD_JOBS (integer 1..64, default 8; anything else is an error).
                 Without a cap msbuild -m starts one node per logical CPU (20 on the i7-12700K), each running cl.exe
                 children of 1-2 GB on large translation units, and Windows kills the build when the PC is in use
                 (cl D8040). CL_MPCount caps /MP inside a project (harmless when a project does not use /MP).
    Exit codes: 0 ok; 20 clone dirty (fetch) or patch does not apply (patch);
                21 clone at an unexpected state (fetch, patch, deps) or patch_id mismatch (patch);
                10 prerequisites missing (deps, app, test); 30 cmake configure or msbuild failed (deps);
                40 app configure/build failed; 45 MCP standalone tests failed; 50 packaging failed; 1 any other error.
                21 also covers a missing, unpatched or wrong-commit clone, or missing deps, for app and test.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('fetch', 'patch', 'deps', 'app', 'test', 'package', 'all')][string]$Step
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot '..\tools\Upstream.psm1') -Force
Import-Module (Join-Path $PSScriptRoot '..\tools\Package.psm1') -Force

function Get-MsBuildJobArgs {
    # msbuild parallelism cap: BAMBURINO_MSBUILD_JOBS (integer 1..64, default 8). Returns '-m:<n>' and the
    # CL_MPCount property that caps /MP inside projects. Throws on an invalid value.
    $raw = $env:BAMBURINO_MSBUILD_JOBS
    $n = 8
    if (-not [string]::IsNullOrWhiteSpace($raw)) {
        $t = $raw.Trim()
        if ($t -notmatch '^[0-9]{1,3}$' -or [int]$t -lt 1 -or [int]$t -gt 64) {
            throw "BAMBURINO_MSBUILD_JOBS must be an integer 1..64, got '$raw'"
        }
        $n = [int]$t
    }
    return @("-m:$n", "/p:CL_MPCount=$n")
}

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
    # 25 only for the first deps build (peak); 12 for a deps resume/no-op and for the app step; 2 for the MCP
    # standalone tests (two source files against the prebuilt deps).
    param([Parameter(Mandatory)][ValidateSet('deps', 'app', 'test')][string]$Step, [string]$Src = (Join-Path (Get-BuildRoot) 'BambuStudio'))
    if ($Step -eq 'test') { return 2 }
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
    $msbuildArgs = @(Get-MsBuildJobArgs) + @('ALL_BUILD.vcxproj', '/p:Configuration=Release', '/v:minimal', '/nodeReuse:false')
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

function Format-CommandLine {
    # One log line for a native command: arguments containing spaces are double-quoted.
    param([Parameter(Mandatory)][string]$Exe, [string[]]$Arguments = @())
    $parts = @($Exe) + @($Arguments) | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } }
    return ($parts -join ' ')
}

function Assert-PatchedClone {
    # The app and test steps build the patched clone: it must exist, sit on studio.tag_commit and carry the patch set.
    param([Parameter(Mandatory)][string]$Src, [Parameter(Mandatory)][string]$Commit)
    if (-not (Test-Path -LiteralPath $Src) -or -not (Test-CloneRoot $Src)) {
        Write-Host "ERROR $Src is missing or not the root of a git work tree. Run driver/build.ps1 -Step fetch and -Step patch first."
        exit 21
    }
    $head = (Invoke-Git -C $Src rev-parse HEAD | Select-Object -First 1).Trim()
    if ($head -ne $Commit) { Write-Host "ERROR $Src is at $head, expected $Commit. Run driver/build.ps1 -Step patch."; exit 21 }
    if (-not (Test-Path -LiteralPath (Join-Path $Src 'src\slic3r\GUI\bamburino_identity.h.in') -PathType Leaf)) {
        Write-Host "ERROR $Src is not patched (no src\slic3r\GUI\bamburino_identity.h.in). Run driver/build.ps1 -Step patch."
        exit 21
    }
}

function Enter-BuildToolchain {
    # VS developer environment, then prereqs.ps1 -Check -Export in-process (VS CMake first, MinGW bins off PATH).
    param([Parameter(Mandatory)][int]$MinFreeGB)
    Enter-VsEnv
    & (Join-Path $PSScriptRoot 'prereqs.ps1') -Check -Export -MinFreeGB $MinFreeGB | Out-Host
    if ($LASTEXITCODE -ne 0) { Write-Host "ERROR prerequisites missing (prereqs.ps1 exit $LASTEXITCODE)"; exit 10 }
    $cmake = $env:BAMBURINO_CMAKE
    if (-not $cmake -or -not (Test-Path -LiteralPath $cmake -PathType Leaf)) { throw "BAMBURINO_CMAKE not set by prereqs.ps1 -Export: '$cmake'" }
    return $cmake
}

function Invoke-App {
    $u = Get-Upstream
    $want = $u['studio.tag_commit']
    if ($want -notmatch '^[0-9a-f]{40}$') { throw 'upstream.toml: studio.tag_commit not set' }
    $root = Get-BuildRoot
    $src = Join-Path $root 'BambuStudio'
    Assert-PatchedClone -Src $src -Commit $want
    $prefix = Join-Path $root 'deps-out\usr\local'
    if (-not (Test-Path -LiteralPath (Join-Path $root 'deps-out\.bamburino-deps-key') -PathType Leaf)) {
        Write-Host "ERROR dependencies not built ($root\deps-out\.bamburino-deps-key missing). Run driver/build.ps1 -Step deps."
        exit 21
    }
    # The identity is never cached (patch 0100), so it is passed on every configure, right before the build.
    $identity = @(Get-IdentityCMakeDefines -Upstream $u)

    $minFree = Get-StepMinFreeGB -Step app -Src $src
    $cmake = Enter-BuildToolchain -MinFreeGB $minFree
    $install = Join-Path $root 'install-dir'
    $buildDir = Join-Path $src 'build'
    New-Item -ItemType Directory -Force -Path $buildDir | Out-Null
    $log = Write-StepLog -Step 'app'
    $configureArgs = @('..', '-G', 'Visual Studio 17 2022', '-A', 'x64', '-DBBL_RELEASE_TO_PUBLIC=1', '-DBBL_INTERNAL_TESTING=0',
        "-DCMAKE_PREFIX_PATH=$prefix", "-DCMAKE_INSTALL_PREFIX=$install", '-DCMAKE_BUILD_TYPE=Release',
        "-DWIN10SDK_PATH=$env:BAMBURINO_WIN10SDK_PATH") + $identity
    # Explicit, so a cache from an earlier configure cannot keep Strawberry's Perl 'pkg-config' script.
    if (-not $env:PKG_CONFIG_EXECUTABLE) { throw 'PKG_CONFIG_EXECUTABLE not set by prereqs.ps1 -Export' }
    $configureArgs += "-DPKG_CONFIG_EXECUTABLE=$env:PKG_CONFIG_EXECUTABLE"
    # /v:minimal keeps warnings and errors but not the per-file progress, so the log stays small on a full build.
    $buildArgs = @('--build', '.', '--target', 'install', '--config', 'Release', '--') + @(Get-MsBuildJobArgs) + @('/v:minimal', '/nodeReuse:false')
    Add-Log $log "== patch_id $($u['driver.patch_id']) on studio.tag_commit $want"
    Add-Log $log "== prereqs.ps1 -MinFreeGB $minFree"
    Add-Log $log "== PATH=$env:Path"
    Add-Log $log "== cwd $buildDir"
    Add-Log $log "== cmake configure: $(Format-CommandLine $cmake $configureArgs)"
    Write-Host "Building the app into $install; log: $log"
    $t0 = Get-Date
    Push-Location -LiteralPath $buildDir
    try {
        & $cmake @configureArgs *>> $log
        $code = $LASTEXITCODE
        Add-Log $log "== cmake configure exit $code"
        if ($code -ne 0) { Write-Host "ERROR cmake configure failed (exit $code), see $log"; exit 40 }
        Add-Log $log "== cmake build: $(Format-CommandLine $cmake $buildArgs)"
        & $cmake @buildArgs *>> $log
        $code = $LASTEXITCODE
        $mins = [math]::Round(((Get-Date) - $t0).TotalMinutes, 1)
        Add-Log $log "== cmake build exit $code after $mins min"
        if ($code -ne 0) { Write-Host "ERROR app build failed (exit $code) after $mins min, see $log"; exit 40 }
    } finally {
        Pop-Location
    }
    $exe = Join-Path $install 'bambu-studio.exe'
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { Write-Host "ERROR build succeeded but $exe is missing, see $log"; exit 40 }
    $vi = (Get-Item -LiteralPath $exe).VersionInfo
    Write-Host "OK app: $exe (version $($vi.ProductMajorPart).$($vi.ProductMinorPart).$($vi.ProductBuildPart).$($vi.ProductPrivatePart), FileVersion $($vi.FileVersion)) built in $mins min"
}

function Invoke-Test {
    $u = Get-Upstream
    $want = $u['studio.tag_commit']
    if ($want -notmatch '^[0-9a-f]{40}$') { throw 'upstream.toml: studio.tag_commit not set' }
    $root = Get-BuildRoot
    $src = Join-Path $root 'BambuStudio'
    Assert-PatchedClone -Src $src -Commit $want
    $prefix = Join-Path $root 'deps-out\usr\local'
    $cmake = Enter-BuildToolchain -MinFreeGB (Get-StepMinFreeGB -Step test -Src $src)
    $ctest = Join-Path (Split-Path $cmake -Parent) 'ctest.exe'
    if (-not (Test-Path -LiteralPath $ctest -PathType Leaf)) { throw "ctest.exe not found next to $cmake" }
    # The PR's legacy contract test is Python: it runs here, on the build machine only, never on the user's PC.
    $python = Get-Command python.exe, py.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $python) { Write-Host 'ERROR Python not found (python.exe or py.exe): needed by tests/mcp/test_legacy_contract.py'; exit 45 }

    $buildDir = Join-Path $src 'build-mcp-tests'
    $log = Write-StepLog -Step 'test'
    $configureArgs = @('-S', 'tests/mcp', '-B', 'build-mcp-tests', '-G', 'Visual Studio 17 2022', '-A', 'x64', "-DCMAKE_PREFIX_PATH=$prefix")
    $buildArgs = @('--build', 'build-mcp-tests', '--config', 'Release', '--') + @(Get-MsBuildJobArgs) + @('/v:minimal', '/nodeReuse:false')
    $ctestArgs = @('-C', 'Release', '--output-on-failure')
    # Run as a script (it calls unittest.main()); 'python -m unittest <path>' would import it as package 'tests.mcp'.
    $pyArgs = @('tests/mcp/test_legacy_contract.py', '-v')
    Add-Log $log "== PATH=$env:Path"
    Add-Log $log "== cwd $src"
    Write-Host "Building and running the MCP standalone tests in $buildDir; log: $log"
    Push-Location -LiteralPath $src
    try {
        Add-Log $log "== cmake configure: $(Format-CommandLine $cmake $configureArgs)"
        & $cmake @configureArgs *>> $log
        $code = $LASTEXITCODE; Add-Log $log "== cmake configure exit $code"
        if ($code -ne 0) { Write-Host "ERROR MCP tests configure failed (exit $code), see $log"; exit 45 }
        Add-Log $log "== cmake build: $(Format-CommandLine $cmake $buildArgs)"
        & $cmake @buildArgs *>> $log
        $code = $LASTEXITCODE; Add-Log $log "== cmake build exit $code"
        if ($code -ne 0) { Write-Host "ERROR MCP tests build failed (exit $code), see $log"; exit 45 }
        Push-Location -LiteralPath $buildDir
        try {
            Add-Log $log "== ctest: $(Format-CommandLine $ctest $ctestArgs)"
            & $ctest @ctestArgs *>> $log
            $code = $LASTEXITCODE
        } finally { Pop-Location }
        Add-Log $log "== ctest exit $code"
        if ($code -ne 0) { Write-Host "ERROR ctest failed (exit $code), see $log"; exit 45 }
        # The test pins the oracle's SHA-256 over its committed (LF) bytes, while the clone's work tree may have CRLF
        # (core.autocrlf=true). Run it on an LF export of the index: the patched content exactly as applied.
        $pyRoot = Join-Path $root ('tmp-legacy-contract-' + [guid]::NewGuid().ToString('N'))
        $pyFiles = @('tests/mcp/test_legacy_contract.py', 'tests/mcp/legacy_tools_oracle.json', 'src/slic3r/GUI/McpToolsManifest.json')
        try {
            $out = & git -c core.autocrlf=false -C $src checkout-index "--prefix=$($pyRoot -replace '\\', '/')/" -- @pyFiles 2>&1
            if ($LASTEXITCODE -ne 0) { Add-Log $log $out; Write-Host "ERROR git checkout-index of the legacy contract files failed, see $log"; exit 45 }
            Push-Location -LiteralPath $pyRoot
            try {
                Add-Log $log "== cwd $pyRoot (LF export of the index: $($pyFiles -join ', '))"
                Add-Log $log "== python: $(Format-CommandLine $python.Source $pyArgs)"
                & $python.Source @pyArgs *>> $log
                $code = $LASTEXITCODE
            } finally { Pop-Location }
        } finally {
            Remove-Item -LiteralPath $pyRoot -Recurse -Force -ErrorAction SilentlyContinue
        }
        Add-Log $log "== python unittest exit $code"
        if ($code -ne 0) { Write-Host "ERROR test_legacy_contract.py failed (exit $code), see $log"; exit 45 }
    } finally {
        Pop-Location
    }
    Write-Host "OK test: ctest and test_legacy_contract.py passed; log: $log"
}

function Invoke-Package {
    $u = Get-Upstream
    $version = $u['studio.version']; $patchId = $u['driver.patch_id']
    if ($version -notmatch '^\d+(\.\d+){3}$') { throw "upstream.toml: studio.version invalid: '$version'" }
    if ($patchId -notmatch '^[0-9a-f]{12}$') { throw "upstream.toml: driver.patch_id is not 12 hex: '$patchId'" }
    $root = Get-BuildRoot
    $install = Join-Path $root 'install-dir'
    $exe = Join-Path $install 'bambu-studio.exe'
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { Write-Host "ERROR $exe missing. Run driver/build.ps1 -Step app."; exit 50 }
    # Identity guard: the zip is named after patch_id, so install-dir must hold the build of that patch_id.
    $marker = Get-PackageMarker -Upstream $u
    $dll = Join-Path $install 'BambuStudio.dll'
    if (-not (Test-BinaryMarker -Path $dll -Marker $marker)) {
        Write-Host "ERROR $dll does not contain '$marker' (driver.patch_id in upstream.toml): install-dir is missing, stale or from another patch set. Run driver/build.ps1 -Step app."
        exit 50
    }
    $dir = Join-Path $root 'artifacts'
    $extras = Join-Path $root ('tmp-package-extras-' + [guid]::NewGuid().ToString('N'))
    $name = "BambuStudio-bamburino-$version-$patchId-win64.zip"
    $zip = Join-Path $dir $name
    $tmp = "$zip.partial"
    try {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        foreach ($f in @($zip, "$zip.sha256", $tmp)) { if (Test-Path -LiteralPath $f) { Remove-Item -LiteralPath $f -Force } }
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        # The contents of install-dir at the zip root (no enclosing folder).
        [System.IO.Compression.ZipFile]::CreateFromDirectory($install, $tmp, [System.IO.Compression.CompressionLevel]::Optimal, $false)
        # AGPL-3.0 section 6: SOURCE.txt, LICENSE, NOTICE.md (and install-studio.ps1 when present) at the zip root.
        $added = Write-PackageExtras -RepoRoot (Join-Path $PSScriptRoot '..') -Destination $extras -Upstream $u
        foreach ($n in $added) {
            if (Test-Path -LiteralPath (Join-Path $install $n)) { throw "install-dir already has $n; refusing to overwrite it in the zip" }
        }
        $archive = [System.IO.Compression.ZipFile]::Open($tmp, [System.IO.Compression.ZipArchiveMode]::Update)
        try {
            foreach ($n in $added) {
                [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, (Join-Path $extras $n), $n, [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
            }
        } finally { $archive.Dispose() }
        Write-Host "Added at the zip root: $($added -join ', ')"
        Move-Item -LiteralPath $tmp -Destination $zip
        $hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
        # sha256sum format, so 'sha256sum -c' works next to the zip.
        [System.IO.File]::WriteAllText("$zip.sha256", "$hash  $name`n", (New-Object System.Text.UTF8Encoding($false)))
    } catch {
        if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue }
        Write-Host "ERROR packaging failed: $($_.Exception.Message)"
        exit 50
    } finally {
        Remove-Item -LiteralPath $extras -Recurse -Force -ErrorAction SilentlyContinue
    }
    $mb = [math]::Round((Get-Item -LiteralPath $zip).Length / 1MB, 1)
    Write-Host "OK package: $zip ($mb MB) sha256 $hash"
}

function Invoke-All {
    # prereqs -> fetch (only when the clone is absent) -> patch -> deps -> app -> test -> package; each step runs in
    # its own pwsh process (fresh environment), and the first non-zero exit code stops the chain and is propagated.
    $root = Get-BuildRoot
    $src = Join-Path $root 'BambuStudio'
    $pwsh = (Get-Process -Id $PID).Path
    # Free-disk threshold of the heaviest step that will do work: a first deps build (25) unless the deps sentinel is
    # already there (deps is then a no-op, or a resume that the deps step checks itself), else the app build (12).
    $sentinel = Join-Path $root 'deps-out\.bamburino-deps-key'
    $minFree = if (Test-Path -LiteralPath $sentinel -PathType Leaf) { Get-StepMinFreeGB -Step app -Src $src } else { Get-StepMinFreeGB -Step deps -Src $src }
    Write-Host "== all: prereqs (-MinFreeGB $minFree)"
    & $pwsh -NoProfile -NonInteractive -File (Join-Path $PSScriptRoot 'prereqs.ps1') -Check -MinFreeGB $minFree
    if ($LASTEXITCODE -ne 0) { Write-Host "STOP all: prereqs exit $LASTEXITCODE"; exit $LASTEXITCODE }
    $steps = @('patch', 'deps', 'app', 'test', 'package')
    if (-not (Test-Path -LiteralPath $src)) { $steps = @('fetch') + $steps } else { Write-Host "== all: fetch skipped, $src exists" }
    $t0 = Get-Date
    foreach ($s in $steps) {
        Write-Host "== all: $s"
        & $pwsh -NoProfile -NonInteractive -File $PSCommandPath -Step $s
        if ($LASTEXITCODE -ne 0) { Write-Host "STOP all: step $s exit $LASTEXITCODE"; exit $LASTEXITCODE }
    }
    Write-Host "OK all: $($steps -join ', ') in $([math]::Round(((Get-Date) - $t0).TotalMinutes, 1)) min"
}

switch ($Step) {
    'fetch' { Invoke-Fetch }
    'patch' { Invoke-Patch }
    'deps' { Invoke-Deps }
    'app' { Invoke-App }
    'test' { Invoke-Test }
    'package' { Invoke-Package }
    'all' { Invoke-All }
}
exit 0

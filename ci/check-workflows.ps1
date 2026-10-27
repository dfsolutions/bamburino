# SPDX-License-Identifier: AGPL-3.0-or-later
# Static guard for workflows that may run on the self-hosted runner.
# Pure PowerShell, line/regex based (no YAML module). FAIL-CLOSED: anything this
# checker cannot positively verify (flow mappings, expressions, unknown forms) is a violation.
#
# A job is HOSTED only if its runs-on is exactly one unquoted scalar:
# ubuntu-latest or windows-latest. Everything else (lists, mappings, ${{ }},
# default labels such as [Windows, X64], missing runs-on) is treated as self-hosted.
#
# Rules (the rule letter is printed with every violation):
#   a  workflow with a non-hosted job: `on:` has only push and/or workflow_dispatch
#      (an inline `on: {...}` mapping is always a violation)
#   b  same workflows: push has branches [main] and/or tags ['v*'], nothing else
#   c  same workflows: job `guard` on `runs-on: ubuntu-latest` (pwsh on Windows would
#      not run the `{ ...; }` block) containing exactly this step line
#        - run: grep -qxF "${{ github.actor }}" ci/maintainers.txt || { echo "::error::actor not allowed on self-hosted runner"; exit 1; }
#      with no `if:`, `continue-on-error` or `shell:` inside guard; every non-hosted job
#      has `needs: guard` and no `if:` using always()/failure()/cancelled()
#   d  same workflows: no `write` permission at all (incl. write-all)
#   g  all workflows: no pull_request_target; top-level `permissions:` block with
#      `contents: read`; no inline `permissions: {...}`; a recognizable `jobs:` section;
#      every non-local `uses:` (also flow style) pinned to a 40-hex SHA; any `write`
#      permission only as `contents: write` in job `release` of release.yml (hosted)
#
# Limits that cannot be solved statically:
#   - an "imposter commit" SHA (a SHA that exists only in a fork) passes the pin check;
#   - the real barrier against fork PRs is the GitHub repository setting (approval required
#     for all external contributors + self-hosted runner restricted to selected workflows),
#     configured in plan C Task 2. This script is the second line of defence.
#
# Usage: pwsh ci/check-workflows.ps1 [-Path .github/workflows]   (exit 1 on violations)
param([string]$Path = '.github/workflows')

$ErrorActionPreference = 'Stop'
$violations = New-Object System.Collections.Generic.List[string]
$hostedRe = '^(ubuntu-latest|windows-latest)$'
$guardStep = 'run: grep -qxF "${{ github.actor }}" ci/maintainers.txt || { echo "::error::actor not allowed on self-hosted runner"; exit 1; }'

function Add-V([string]$file, [string]$rule, [string]$msg) { $script:violations.Add("${file}: [$rule] $msg") }
function Get-Indent([string]$l) { $l.Length - $l.TrimStart().Length }

# Drop a ` #...` comment, ignoring '#' inside quoted strings.
function Remove-Comment([string]$l) {
    $q = $null
    for ($i = 0; $i -lt $l.Length; $i++) {
        $c = $l[$i]
        if ($null -ne $q) {
            if ($c -eq '\' -and $q -eq '"') { $i++; continue }   # escaped char inside "..."
            if ($c -eq $q) { $q = $null }
            continue
        }
        if ($c -eq '"' -or $c -eq "'") { $q = $c; continue }
        if ($c -eq '#' -and ($i -eq 0 -or $l[$i - 1] -match '\s')) { return $l.Substring(0, $i).TrimEnd() }
    }
    $l.TrimEnd()
}
# Comment strip + unquote keys ("jobs": -> jobs:) + unquote whole scalar values
# (contents: 'write' -> contents: write), except runs-on (a quoted runs-on is never hosted).
function Normalize-Line([string]$l) {
    $l = Remove-Comment $l
    $l = [regex]::Replace($l, '^(\s*(?:-\s*)?)(["''])([^"'']+)\2(\s*:)', '$1$3$4')
    # `key : value` -> `key: value` (first colon after a plain key only)
    $l = [regex]::Replace($l, '^(\s*(?:-\s*)?[\w-]+)\s+:(?=\s|$)\s*', '$1: ')
    $l = $l -replace ':\s+$', ':'
    if ($l -notmatch '^\s*(?:-\s*)?runs-on:') {
        $l = [regex]::Replace($l, '^(\s*(?:-\s*)?[\w-]+:\s*)(["''])([^"'']*)\2\s*$', '$1$3')
    }
    $l
}
# Split a scalar/inline-list value ("a", "[a, b]", "'x'") into trimmed items.
function Split-Inline([string]$v) {
    $v = $v.Trim().TrimStart('[').TrimEnd(']')
    @($v -split ',' | ForEach-Object { $_.Trim().Trim("'").Trim('"') } | Where-Object { $_ -ne '' })
}
# Items of `key:` on line $i: inline value, or following `- item` block lines.
function Get-KeyItems([string[]]$lines, [int]$i) {
    $m = [regex]::Match($lines[$i], '^\s*[\w-]+:\s*(.*)$')
    $val = $m.Groups[1].Value.Trim()
    if ($val -ne '') { return @(Split-Inline $val) }
    $items = @()
    $ind = Get-Indent $lines[$i]
    for ($j = $i + 1; $j -lt $lines.Count; $j++) {
        if ($lines[$j].Trim() -eq '') { continue }
        if ($lines[$j] -match '^\s*-\s*(.+)$' -and (Get-Indent $lines[$j]) -ge $ind) {
            $items += $Matches[1].Trim().Trim("'").Trim('"')
        } else { break }
    }
    $items
}

function Test-Workflow([IO.FileInfo]$f) {
    $name = $f.Name
    $lines = @(Get-Content -LiteralPath $f.FullName | ForEach-Object { Normalize-Line $_ })

    function Get-SectionEnd([int]$start) {
        for ($j = $start + 1; $j -lt $lines.Count; $j++) {
            if ($lines[$j].Trim() -ne '' -and (Get-Indent $lines[$j]) -eq 0) { return $j }
        }
        $lines.Count
    }

    $onIdx = -1; $jobsIdx = -1; $permIdx = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^on:') { $onIdx = $i }
        if ($lines[$i] -match '^jobs:\s*$') { $jobsIdx = $i }
        if ($lines[$i] -match '^permissions:') { $permIdx = $i }
    }

    # ---- unsupported structure: multi-document, duplicate top-level keys ----
    $seen = $false; $topKeys = @{}
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $l = $lines[$i]
        if ($l.Trim() -eq '') { continue }
        if ($l -match '^(---|\.\.\.)(\s|$)') {
            if ($seen) { Add-V $name 'g' "multi-document YAML is not supported (line $($i + 1))" }
            continue
        }
        $seen = $true
        if ($l -match '^([\w-]+):') {
            $k = $Matches[1]
            if ($topKeys.ContainsKey($k)) { Add-V $name 'g' "duplicate top-level key '$k' (line $($i + 1)): unsupported form" }
            $topKeys[$k] = $true
        }
    }

    # ---- on: triggers ----
    $triggers = @{}
    if ($onIdx -ge 0) {
        $onEnd = Get-SectionEnd $onIdx
        $inline = ($lines[$onIdx] -replace '^on:\s*', '').Trim()
        if ($inline -match '[\{]') { Add-V $name 'a' 'unsupported inline form for on: (use block form)' }
        elseif ($inline -ne '') { foreach ($t in Split-Inline $inline) { $triggers[$t] = @() } }
        else {
            $childInd = -1
            for ($j = $onIdx + 1; $j -lt $onEnd; $j++) {
                $l = $lines[$j]; if ($l.Trim() -eq '') { continue }
                if ($childInd -lt 0) { $childInd = Get-Indent $l }
                if ((Get-Indent $l) -ne $childInd) { continue }
                if ($l -match '^\s*(?:-\s*)?([\w]+):?\s*(.*)$') {
                    $t = $Matches[1]; $rest = $Matches[2]
                    $block = New-Object System.Collections.Generic.List[string]
                    if ($rest -ne '') { $block.Add($rest) }
                    for ($k = $j + 1; $k -lt $onEnd; $k++) {
                        if ($lines[$k].Trim() -ne '' -and (Get-Indent $lines[$k]) -le $childInd) { break }
                        $block.Add($lines[$k])
                    }
                    $triggers[$t] = @($block)
                }
            }
        }
    } else { Add-V $name 'a' 'no on: section found' }

    # ---- jobs ----
    $jobs = [ordered]@{}
    $jobAt = @{}      # line index -> job id
    if ($jobsIdx -lt 0) {
        Add-V $name 'g' 'no recognizable jobs: section (inline or missing jobs is not supported)'
    } else {
        $jobsEnd = Get-SectionEnd $jobsIdx
        $jobInd = -1; $cur = $null
        for ($j = $jobsIdx + 1; $j -lt $jobsEnd; $j++) {
            $l = $lines[$j]; if ($l.Trim() -eq '') { continue }
            if ($jobInd -lt 0) { $jobInd = Get-Indent $l }
            if ((Get-Indent $l) -eq $jobInd) {
                if ($l -match '^\s*([\w-]+):\s*$') {
                    $cur = $Matches[1]
                    if ($jobs.Contains($cur)) {
                        Add-V $name 'g' "duplicate job id '$cur' (line $($j + 1)): unsupported form"
                    } else {
                    $jobs[$cur] = @{ Lines = (New-Object System.Collections.Generic.List[string]); Idx = (New-Object System.Collections.Generic.List[int]) }
                    }
                    continue
                } else {
                    $cur = $null
                    Add-V $name 'g' "unsupported job form at line $($j + 1): '$($l.Trim())' (use 'id:' block mapping)"
                }
            }
            if ($null -ne $cur) { $jobs[$cur].Lines.Add($l); $jobs[$cur].Idx.Add($j); $jobAt[$j] = $cur }
        }
        if ($jobs.Count -eq 0) { Add-V $name 'g' 'jobs: section has no jobs' }
        foreach ($id in @($jobs.Keys)) {
            $jl = @($jobs[$id].Lines)
            $runsOn = $null; $needs = @(); $jInd = -1
            for ($k = 0; $k -lt $jl.Count; $k++) {
                if ($jInd -lt 0) { $jInd = Get-Indent $jl[$k] }
                if ((Get-Indent $jl[$k]) -ne $jInd) { continue }
                if ($jl[$k] -match '^\s*runs-on:\s*(\S+)\s*$') { $runsOn = $Matches[1] }
                if ($jl[$k] -match '^\s*needs:') { $needs = @(Get-KeyItems $jl $k) }
            }
            $jobs[$id].RunsOn = $runsOn
            $jobs[$id].Needs = $needs
            $jobs[$id].Hosted = ($null -ne $runsOn -and $runsOn -cmatch $hostedRe)
        }
    }
    $selfJobs = @($jobs.Keys | Where-Object { -not $jobs[$_].Hosted })
    $isSelf = $selfJobs.Count -gt 0

    # ---- global rules (all files) ----
    if ($triggers.ContainsKey('pull_request_target')) { Add-V $name 'g' 'pull_request_target is forbidden' }
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $l = $lines[$i]
        foreach ($m in [regex]::Matches($l, '(?:^\s*(?:-\s*)?|[{,]\s*)uses\s*:\s*([^\s,}]+)')) {
            $u = $m.Groups[1].Value.Trim("'").Trim('"')
            if ($u -notmatch '^\./' -and $u -notmatch '@[0-9a-f]{40}$') {
                Add-V $name 'g' "uses: $u is not pinned to a 40-hex SHA (line $($i + 1))"
            }
        }
        if ($l -match '^\s*permissions:\s*[\{\[]') {
            Add-V $name 'g' "unsupported inline form for permissions (line $($i + 1)); use a block mapping"
        }
        if ($l -match '^\s*permissions:\s*write-all') {
            Add-V $name 'g' "permissions: write-all is forbidden (line $($i + 1))"
        }
        if ($l -match '^\s*([\w-]+):\s*write\b') {
            $key = $Matches[1]; $jid = $jobAt[$i]
            if ($null -eq $jid) { Add-V $name 'g' "${key}: write at workflow level (line $($i + 1))" }
            elseif ($key -ne 'contents' -or $name -ne 'release.yml' -or $jid -ne 'release') {
                Add-V $name 'g' "${key}: write in job '$jid' (line $($i + 1)); allowed only as 'contents: write' in job 'release' of release.yml"
            } elseif (-not $jobs[$jid].Hosted) {
                Add-V $name 'g' "contents: write in job 'release' requires runs-on ubuntu-latest or windows-latest (line $($i + 1))"
            }
        }
    }
    # top-level permissions with contents: read
    if ($permIdx -lt 0) {
        Add-V $name 'g' 'missing top-level permissions: block (needs contents: read)'
    } else {
        $inl = ($lines[$permIdx] -replace '^permissions:\s*', '').Trim()
        if ($inl -ne '') {
            if ($inl -notmatch '^[\{\[]') { Add-V $name 'g' "top-level permissions must be a block mapping with 'contents: read' (found scalar '$inl')" }
        } else {
            $pe = Get-SectionEnd $permIdx; $ok = $false
            for ($j = $permIdx + 1; $j -lt $pe; $j++) { if ($lines[$j] -match '^\s+contents:\s*read\s*$') { $ok = $true } }
            if (-not $ok) { Add-V $name 'g' "top-level permissions lacks 'contents: read'" }
        }
    }

    if (-not $isSelf) { return }

    # ---- rule a ----
    foreach ($t in $triggers.Keys) {
        if ($t -notin 'push', 'workflow_dispatch') { Add-V $name 'a' "trigger '$t' not allowed on self-hosted workflows (only push, workflow_dispatch)" }
    }
    if ($triggers.Count -eq 0 -and $onIdx -ge 0 -and $lines[$onIdx] -notmatch '\{') { Add-V $name 'a' 'no on: triggers found' }

    # ---- rule b ----
    if ($triggers.ContainsKey('push')) {
        $pb = @($triggers['push'] | ForEach-Object { $_ -replace '^\s*\{', '' -replace '\}\s*$', '' })
        $branches = $null; $tags = $null
        for ($i = 0; $i -lt $pb.Count; $i++) {
            if ($pb[$i] -match '^\s*branches:') { $branches = @(Get-KeyItems $pb $i) }
            if ($pb[$i] -match '^\s*tags:') { $tags = @(Get-KeyItems $pb $i) }
        }
        if ($null -eq $branches -and $null -eq $tags) {
            Add-V $name 'b' "push needs branches: [main] and/or tags: ['v*']"
        }
        if ($null -ne $branches -and ($branches.Count -eq 0 -or @($branches | Where-Object { $_ -ne 'main' }).Count -gt 0)) {
            Add-V $name 'b' "push branches must be exactly [main], found: $($branches -join ', ')"
        }
        if ($null -ne $tags -and ($tags.Count -eq 0 -or @($tags | Where-Object { $_ -ne 'v*' }).Count -gt 0)) {
            Add-V $name 'b' "push tags must be exactly ['v*'], found: $($tags -join ', ')"
        }
    }

    # ---- rule c ----
    if (-not $jobs.Contains('guard')) {
        Add-V $name 'c' "no 'guard' job found"
    } else {
        $g = $jobs['guard']
        if ($g.RunsOn -cne 'ubuntu-latest') {
            Add-V $name 'c' "'guard' job must have runs-on: ubuntu-latest (found: $($g.RunsOn))"
        }
        # Structural check: the guard job must be exactly the template, nothing else.
        $gl = @($g.Lines)
        foreach ($l in $gl) {
            if ($l -match '^\s*(?:-\s*)?(if|continue-on-error|shell|env):') {
                Add-V $name 'c' "'guard' job must not use '$($Matches[1]):'"
            }
        }
        $ok = $false
        if ($gl.Count -eq 5) {
            $j0 = Get-Indent $gl[0]; $s0 = Get-Indent $gl[2]
            $ok = ($gl[0].Trim() -ceq 'runs-on: ubuntu-latest') -and
                  ($gl[1].Trim() -ceq 'steps:') -and ((Get-Indent $gl[1]) -eq $j0) -and
                  ($s0 -gt $j0) -and
                  ($gl[2].Trim() -cmatch '^- uses: actions/checkout@[0-9a-f]{40}$') -and
                  ((Get-Indent $gl[3]) -eq ($s0 + 2)) -and
                  ($gl[3].Trim() -ceq 'with: { sparse-checkout: ci/maintainers.txt }') -and
                  ((Get-Indent $gl[4]) -eq $s0) -and
                  ($gl[4].Trim() -ceq "- $guardStep")
        }
        if (-not $ok) {
            Add-V $name 'c' "'guard' job must be exactly: runs-on: ubuntu-latest; steps: [ - uses: actions/checkout@<40-hex sha> with { sparse-checkout: ci/maintainers.txt }, - $guardStep ] (no block scalars, extra steps or keys)"
        }
    }
    foreach ($id in $selfJobs) {
        if ($id -eq 'guard') { continue }
        if ($jobs[$id].Needs -notcontains 'guard') { Add-V $name 'c' "self-hosted job '$id' lacks needs: guard" }
        foreach ($l in $jobs[$id].Lines) {
            if ($l -match '^\s*(?:-\s*)?if:.*\b(always|failure|cancelled)\s*\(') {
                Add-V $name 'c' "self-hosted job '$id' uses $($Matches[1])() in an if: condition, which can bypass the guard"
            }
        }
    }
    # ---- rule d ----
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^\s*[\w-]+:\s*write\b') {
            Add-V $name 'd' "write permission on self-hosted workflow: '$($lines[$i].Trim())' (line $($i + 1))"
        }
        if ($lines[$i] -match '^\s*permissions:\s*write-all') {
            Add-V $name 'd' "permissions: write-all on self-hosted workflow (line $($i + 1))"
        }
    }
}

if (-not (Test-Path -LiteralPath $Path)) {
    Write-Host "check-workflows: '$Path' not found, nothing to check"
    exit 0
}
$files = @(Get-ChildItem -LiteralPath $Path -File | Where-Object { $_.Extension -in '.yml', '.yaml' } | Sort-Object Name)
if ($files.Count -eq 0) {
    Write-Host "check-workflows: no workflow files in '$Path'"
    exit 0
}
foreach ($f in $files) { Test-Workflow $f }

if ($violations.Count -gt 0) {
    Write-Host "check-workflows: $($violations.Count) violation(s)"
    $violations | ForEach-Object { Write-Host $_ }
    exit 1
}
Write-Host "check-workflows: $($files.Count) file(s) OK"
exit 0

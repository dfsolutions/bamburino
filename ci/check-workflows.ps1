# SPDX-License-Identifier: AGPL-3.0-or-later
# Static guard for workflows that run on the self-hosted runner.
# Pure PowerShell, line/regex based (no YAML module): it covers the workflow
# style used in this repo (block mappings, inline or block lists), not all YAML.
#
# Rules (rule letter is printed with every violation):
#   a  self-hosted workflow: `on:` has only push and/or workflow_dispatch
#   b  self-hosted workflow: push has branches [main] and/or tags ['v*'] (nothing else)
#   c  self-hosted workflow: a `guard` job on ubuntu-latest/windows-latest checks
#      github.actor against ci/maintainers.txt; every self-hosted job has needs: guard
#   d  self-hosted workflow: no `write` permission anywhere (incl. write-all)
#   g  all workflows: no pull_request_target; every non-local `uses:` pinned to a
#      40-hex SHA; `contents: write` only in job `release` of release.yml, which
#      must run on ubuntu-latest/windows-latest
# Usage: pwsh ci/check-workflows.ps1 [-Path .github/workflows]   (exit 1 on violations)
param([string]$Path = '.github/workflows')

$ErrorActionPreference = 'Stop'
$violations = New-Object System.Collections.Generic.List[string]
$hosted = @('ubuntu-latest', 'windows-latest')

function Add-V([string]$file, [string]$rule, [string]$msg) { $script:violations.Add("${file}: [$rule] $msg") }
function Get-Indent([string]$l) { $l.Length - $l.TrimStart().Length }
function Remove-Comment([string]$l) {
    if ($l -match '^\s*#') { return '' }
    ($l -replace '\s+#.*$', '').TrimEnd()
}
# Split a scalar/inline-list value ("a", "[a, b]", "'x'") into trimmed items.
function Split-Inline([string]$v) {
    $v = $v.Trim().TrimStart('[').TrimEnd(']')
    @($v -split ',' | ForEach-Object { $_.Trim().Trim("'").Trim('"') } | Where-Object { $_ -ne '' })
}
# Value of `key:` at the start of a line: inline items, or following `- item` block lines.
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
    $lines = @(Get-Content -LiteralPath $f.FullName | ForEach-Object { Remove-Comment $_ })

    # ---- top-level sections ----
    $onIdx = -1; $jobsIdx = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^["'']?on["'']?:') { $onIdx = $i }
        if ($lines[$i] -match '^jobs:\s*$') { $jobsIdx = $i }
    }
    function Get-SectionEnd([int]$start) {
        for ($j = $start + 1; $j -lt $lines.Count; $j++) {
            if ($lines[$j].Trim() -ne '' -and (Get-Indent $lines[$j]) -eq 0) { return $j }
        }
        $lines.Count
    }

    # ---- on: triggers ----
    $triggers = @{}      # trigger -> block lines (string[])
    $onUnsupported = $false
    if ($onIdx -ge 0) {
        $onEnd = Get-SectionEnd $onIdx
        $inline = ($lines[$onIdx] -replace '^["'']?on["'']?:\s*', '').Trim()
        if ($inline -match '\{') { $onUnsupported = $true }
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
    }

    # ---- jobs ----
    $jobs = [ordered]@{}   # id -> @{ Lines; RunsOn; Needs }
    if ($jobsIdx -ge 0) {
        $jobsEnd = Get-SectionEnd $jobsIdx
        $jobInd = -1; $cur = $null; $curLines = $null
        $flush = {
            if ($null -ne $cur) { $jobs[$cur] = @{ Lines = @($curLines) } }
        }
        for ($j = $jobsIdx + 1; $j -lt $jobsEnd; $j++) {
            $l = $lines[$j]; if ($l.Trim() -eq '') { continue }
            if ($jobInd -lt 0) { $jobInd = Get-Indent $l }
            if ((Get-Indent $l) -eq $jobInd -and $l -match '^\s*([\w-]+):\s*$') {
                & $flush
                $cur = $Matches[1]; $curLines = New-Object System.Collections.Generic.List[string]
            } elseif ($null -ne $cur) { $curLines.Add($l) }
        }
        & $flush
        foreach ($id in @($jobs.Keys)) {
            $jl = $jobs[$id].Lines
            $ro = @(); $needs = @()
            $jInd = -1
            for ($k = 0; $k -lt $jl.Count; $k++) {
                if ($jInd -lt 0) { $jInd = Get-Indent $jl[$k] }
                if ((Get-Indent $jl[$k]) -ne $jInd) { continue }
                if ($jl[$k] -match '^\s*runs-on:') { $ro = @(Get-KeyItems $jl $k) }
                if ($jl[$k] -match '^\s*needs:') { $needs = @(Get-KeyItems $jl $k) }
            }
            $jobs[$id].RunsOn = $ro
            $jobs[$id].Needs = $needs
            $jobs[$id].SelfHosted = [bool]($ro -contains 'self-hosted')
        }
    }
    $selfJobs = @($jobs.Keys | Where-Object { $jobs[$_].SelfHosted })
    $isSelf = $selfJobs.Count -gt 0

    # ---- global rules (all files) ----
    if ($triggers.ContainsKey('pull_request_target')) { Add-V $name 'g' 'pull_request_target is forbidden' }
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^\s*(?:-\s*)?uses:\s*(\S+)') {
            $u = $Matches[1].Trim("'").Trim('"')
            if ($u -notmatch '^\./' -and $u -notmatch '@[0-9a-f]{40}$') {
                Add-V $name 'g' "uses: $u is not pinned to a 40-hex SHA (line $($i + 1))"
            }
        }
        if ($lines[$i] -match '^\s*permissions:\s*write-all') {
            Add-V $name 'g' "permissions: write-all is forbidden (line $($i + 1))"
        }
    }
    # contents: write only in job `release` of release.yml (hosted runner)
    $inJobsRange = @{}   # line index -> job id
    if ($jobsIdx -ge 0) {
        $jobsEnd = Get-SectionEnd $jobsIdx
        $curId = $null; $jobInd = -1
        for ($j = $jobsIdx + 1; $j -lt $jobsEnd; $j++) {
            $l = $lines[$j]; if ($l.Trim() -eq '') { continue }
            if ($jobInd -lt 0) { $jobInd = Get-Indent $l }
            if ((Get-Indent $l) -eq $jobInd -and $l -match '^\s*([\w-]+):\s*$') { $curId = $Matches[1] }
            $inJobsRange[$j] = $curId
        }
    }
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^\s*contents:\s*write\b') {
            $jid = $inJobsRange[$i]
            if ($null -eq $jid) { Add-V $name 'g' "contents: write at workflow level (line $($i + 1))" }
            elseif ($name -ne 'release.yml' -or $jid -ne 'release') {
                Add-V $name 'g' "contents: write in job '$jid' (line $($i + 1)); allowed only in job 'release' of release.yml"
            } elseif (-not ($jobs[$jid].RunsOn.Count -eq 1 -and $hosted -contains $jobs[$jid].RunsOn[0])) {
                Add-V $name 'g' "contents: write in job 'release' requires runs-on ubuntu-latest or windows-latest (line $($i + 1))"
            }
        }
    }

    if (-not $isSelf) { return }

    # ---- rule a ----
    if ($onUnsupported) { Add-V $name 'a' 'inline mapping for on: is not supported by this checker; use block form' }
    foreach ($t in $triggers.Keys) {
        if ($t -notin 'push', 'workflow_dispatch') { Add-V $name 'a' "trigger '$t' not allowed on self-hosted workflows (only push, workflow_dispatch)" }
    }
    if ($triggers.Count -eq 0 -and -not $onUnsupported) { Add-V $name 'a' 'no on: triggers found' }

    # ---- rule b ----
    if ($triggers.ContainsKey('push')) {
        $pb = @($triggers['push'])
        $branches = $null; $tags = $null
        for ($i = 0; $i -lt $pb.Count; $i++) {
            if ($pb[$i] -match '^\s*\{?\s*branches:') {
                $branches = @(Get-KeyItems ($pb -replace '^\s*\{', '' -replace '\}\s*$', '') $i)
            }
            if ($pb[$i] -match '^\s*\{?\s*tags:') {
                $tags = @(Get-KeyItems ($pb -replace '^\s*\{', '' -replace '\}\s*$', '') $i)
            }
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
        if (-not ($g.RunsOn.Count -eq 1 -and $hosted -contains $g.RunsOn[0])) {
            Add-V $name 'c' "'guard' job must run on ubuntu-latest or windows-latest (found: $($g.RunsOn -join ', '))"
        }
        $gt = $g.Lines -join "`n"
        if ($gt -notmatch 'github\.actor' -or $gt -notmatch 'maintainers\.txt' -or $gt -notmatch '\bexit 1\b') {
            Add-V $name 'c' "'guard' job must check github.actor against ci/maintainers.txt and 'exit 1' when not listed"
        }
    }
    foreach ($id in $selfJobs) {
        if ($jobs[$id].Needs -notcontains 'guard') { Add-V $name 'c' "self-hosted job '$id' lacks needs: guard" }
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

# SPDX-License-Identifier: AGPL-3.0-or-later
# Helpers for the on-demand self-hosted runner (registration and repository safety settings).
# Nothing in here prints or logs a token.

function Get-RunnerSha256 {
    # Extract the published SHA-256 of a runner package from the release notes body.
    param([Parameter(Mandatory)][string]$ReleaseBody, [string]$Platform = 'win-x64')
    $p = [regex]::Escape($Platform)
    $m = [regex]::Match($ReleaseBody, "<!--\s*BEGIN SHA $p\s*-->([0-9a-fA-F]{64})<!--\s*END SHA $p\s*-->")
    if (-not $m.Success) { throw "SHA-256 for $Platform not found in release notes" }
    $m.Groups[1].Value.ToLowerInvariant()
}

function New-RunnerConfigArgs {
    # Arguments for config.cmd. Never includes --runasservice: the runner must live in the
    # interactive session (the smoke test opens a GUI) and start only on demand.
    param(
        [Parameter(Mandatory)][string]$Url,
        [Parameter(Mandatory)][string]$Token,
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$Labels,
        [Parameter(Mandatory)][string]$Work
    )
    @('--url', $Url, '--token', $Token, '--name', $Name, '--labels', $Labels,
      '--work', $Work, '--unattended', '--replace')
}

function Invoke-GhApi {
    param([string]$Method, [string]$Endpoint, $Fields)
    $a = @('api', '-X', $Method, $Endpoint)
    if ($Fields) { foreach ($k in $Fields.Keys) { $a += @('-F', "$k=$($Fields[$k])") } }
    $out = & gh @a 2>&1
    $ok = ($LASTEXITCODE -eq 0)
    $text = ($out | Out-String).Trim()
    $status = 0
    if (-not $ok -and $text -match 'HTTP (\d{3})') { $status = [int]$Matches[1] }
    $json = $null
    if ($ok -and $text) { try { $json = $text | ConvertFrom-Json } catch { $json = $null } }
    @{ Ok = $ok; Status = $status; Json = $json }
}

function Set-RepoActionsPolicy {
    # Apply and verify the two fork-PR safety settings. $Invoker(method, endpoint, fields) runs
    # gh api and returns @{ Ok; Status; Json }; injectable so unit tests need no network.
    param(
        [Parameter(Mandatory)][string]$Repo,
        [scriptblock]$Invoker = { param($Method, $Endpoint, $Fields) Invoke-GhApi $Method $Endpoint $Fields }
    )
    $settings = @(
        @{ Name = 'fork-pr-approval'; Endpoint = "repos/$Repo/actions/permissions/fork-pr-contributor-approval"
           Fields = [ordered]@{ approval_policy = 'all_external_contributors' }
           Check = { param($j) $j.approval_policy -eq 'all_external_contributors' }
           Manual = 'Settings > Actions > General > Fork pull request workflows from outside collaborators > Require approval for all external contributors' },
        @{ Name = 'workflow-permissions'; Endpoint = "repos/$Repo/actions/permissions/workflow"
           Fields = [ordered]@{ default_workflow_permissions = 'read'; can_approve_pull_request_reviews = 'false' }
           Check = { param($j) $j.default_workflow_permissions -eq 'read' -and $j.can_approve_pull_request_reviews -eq $false }
           Manual = 'Settings > Actions > General > Workflow permissions > Read repository contents and packages permissions; untick "Allow GitHub Actions to create and approve pull requests"' }
    )
    foreach ($s in $settings) {
        $before = & $Invoker 'GET' $s.Endpoint $null
        $beforeOk = $before.Ok -and (& $s.Check $before.Json)
        $result = 'applied'
        if ($beforeOk) { $result = 'already set' }
        else {
            $put = & $Invoker 'PUT' $s.Endpoint $s.Fields
            if (-not $put.Ok) {
                $result = if ($put.Status -in 401, 403, 404) { 'forbidden-by-scope' } else { "failed ($($put.Status))" }
            }
        }
        $after = & $Invoker 'GET' $s.Endpoint $null
        $afterOk = $after.Ok -and (& $s.Check $after.Json)
        if ($result -eq 'applied' -and -not $afterOk) { $result = 'failed (verification)' }
        [pscustomobject]@{
            Setting = $s.Name; Endpoint = $s.Endpoint; Result = $result
            Before = if ($before.Ok) { $before.Json | ConvertTo-Json -Compress } else { "unreadable ($($before.Status))" }
            After  = if ($after.Ok) { $after.Json | ConvertTo-Json -Compress } else { "unreadable ($($after.Status))" }
            Verified = [bool]$afterOk; Manual = if ($afterOk) { $null } else { $s.Manual }
        }
    }
}

function Stop-Runner {
    # Stop the on-demand runner and wait until GitHub reports it offline. Returns $true only then.
    # All side effects are injectable so unit tests need no processes and no network.
    param(
        [string]$PidFile = 'D:\BambuBuild\runner\.bamburino-runner.pid',
        [string]$Name = 'federico-pc',
        [int]$GracefulSec = 20,
        [int]$OfflineSec = 90,
        [scriptblock]$GetProcs = { @(Get-Process -Name 'Runner.Listener', 'Runner.Worker' -ErrorAction SilentlyContinue) },
        [scriptblock]$GetProcInfo = {
            param($ProcId)
            $p = Get-Process -Id $ProcId -ErrorAction SilentlyContinue
            if (-not $p) { return $null }
            $c = Get-CimInstance Win32_Process -Filter "ProcessId=$ProcId" -ErrorAction SilentlyContinue
            @{ Name = $p.ProcessName; StartTime = $p.StartTime; CommandLine = $c.CommandLine }
        },
        [scriptblock]$Graceful = { param($ProcId) & taskkill.exe /PID $ProcId /T 2>&1 | Out-Null },
        [scriptblock]$Force = { param($Procs) $Procs | Stop-Process -Force -ErrorAction SilentlyContinue },
        [scriptblock]$GetStatus = {
            param($RunnerName)
            $r = Invoke-GhApi 'GET' 'repos/dfsolutions/bamburino/actions/runners' $null
            if (-not $r.Ok) { return 'unknown' }
            $m = @($r.Json.runners | Where-Object name -eq $RunnerName)
            if ($m.Count) { $m[0].status } else { 'absent' }
        },
        [scriptblock]$Sleep = { param($Sec) Start-Sleep -Seconds $Sec },
        [scriptblock]$Log = { param($Msg) Write-Host $Msg }
    )
    # Pid file: line 1 = pid, line 2 = StartTime (round-trip ISO), line 3 = full path of run.cmd.
    # The pid is acted on only if that process is still the launcher we started (a pid can be
    # reassigned after a reboot or after the window was closed): cmd.exe, same start time (+-1 s),
    # command line mentioning the recorded run.cmd.
    $launcher = $null
    if (Test-Path -LiteralPath $PidFile) {
        $valid = $false
        $l = @(Get-Content -LiteralPath $PidFile -ErrorAction SilentlyContinue)
        if ($l.Count -ge 3 -and "$($l[0])".Trim() -match '^\d+$') {
            $id = [int]"$($l[0])".Trim()
            try {
                $rec = [datetime]::Parse("$($l[1])".Trim(), [cultureinfo]::InvariantCulture, [Globalization.DateTimeStyles]::RoundtripKind)
                $info = & $GetProcInfo $id
                if ($info -and $info.Name -ieq 'cmd' -and
                    [math]::Abs(($info.StartTime.ToUniversalTime() - $rec.ToUniversalTime()).TotalSeconds) -le 1 -and
                    "$($info.CommandLine)".IndexOf("$($l[2])".Trim(), [StringComparison]::OrdinalIgnoreCase) -ge 0) {
                    $valid = $true
                }
            } catch { $valid = $false }
        }
        if ($valid) { $launcher = $id }
        else { & $Log 'stale pid file, ignored' }
        if (-not $valid) { Remove-Item -LiteralPath $PidFile -Force -ErrorAction SilentlyContinue }
    }
    if ($launcher) { & $Graceful $launcher }
    $waited = 0
    while (@(& $GetProcs).Count -and $waited -lt $GracefulSec) { & $Sleep 1; $waited++ }
    $left = @(& $GetProcs)
    if ($left.Count) {
        & $Log "Graceful stop timed out after $GracefulSec s: forcing $($left.Count) process(es)."
        & $Force $left
        $waited = 0
        while (@(& $GetProcs).Count -and $waited -lt 10) { & $Sleep 1; $waited++ }
        if (@(& $GetProcs).Count) { & $Log 'Runner processes are still alive.'; return $false }
    }
    if (Test-Path -LiteralPath $PidFile) { Remove-Item -LiteralPath $PidFile -Force -ErrorAction SilentlyContinue }
    $waited = 0
    do {
        $st = & $GetStatus $Name
        if ($st -in 'offline', 'absent') { & $Log "Runner '$Name' is $st on GitHub."; return $true }
        if ($waited -ge $OfflineSec) { break }
        & $Sleep 5; $waited += 5
    } while ($true)
    & $Log "Runner '$Name' still '$st' on GitHub after $OfflineSec s."
    return $false
}

Export-ModuleMember -Function Get-RunnerSha256, New-RunnerConfigArgs, Set-RepoActionsPolicy, Invoke-GhApi, Stop-Runner

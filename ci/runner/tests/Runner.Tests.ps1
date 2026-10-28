# SPDX-License-Identifier: AGPL-3.0-or-later
$ErrorActionPreference = 'Stop'
Import-Module "$PSScriptRoot/../Runner.psm1" -Force

# --- Get-RunnerSha256
$sha = 'f48e0750a21812bca5f82de5f7f5aeae71abee647fab5a582f1742d07eba455f'
$other = 'a' * 64
$body = "- linux <!-- BEGIN SHA linux-x64 -->$other<!-- END SHA linux-x64 -->`n" +
        "- win <!-- BEGIN SHA win-x64 -->$($sha.ToUpper())<!-- END SHA win-x64 -->"
if ((Get-RunnerSha256 -ReleaseBody $body) -ne $sha) { throw 'sha: wrong value or case' }
$threw = $false; try { Get-RunnerSha256 -ReleaseBody 'nothing' } catch { $threw = $true }
if (-not $threw) { throw 'sha: missing must throw' }
$threw = $false; try { Get-RunnerSha256 -ReleaseBody 'x <!-- BEGIN SHA win-x64 -->abc<!-- END SHA win-x64 -->' } catch { $threw = $true }
if (-not $threw) { throw 'sha: short hash must throw' }

# --- New-RunnerConfigArgs
$a = New-RunnerConfigArgs -Url 'https://github.com/o/r' -Token 'TKN' -Name 'n' -Labels 'bamburino-win' -Work 'D:\w'
if ($a -contains '--runasservice') { throw 'must not install as a service' }
foreach ($f in '--unattended', '--replace') { if ($a -notcontains $f) { throw "missing $f" } }
if ($a[($a.IndexOf('--labels') + 1)] -ne 'bamburino-win') { throw 'labels' }
if ($a[($a.IndexOf('--token') + 1)] -ne 'TKN') { throw 'token' }

# --- Set-RepoActionsPolicy with a fake API (no network)
$script:log = New-Object System.Collections.Generic.List[string]
function New-Fake($state, $forbidPut) {
    $log = $script:log; $log.Clear()
    {
        param($Method, $Endpoint, $Fields)
        $log.Add("$Method $Endpoint")
        $key = if ($Endpoint -like '*fork-pr*') { 'fork' } else { 'wf' }
        if ($Method -eq 'PUT') {
            if ($forbidPut) { return @{ Ok = $false; Status = 403; Json = $null } }
            foreach ($k in $Fields.Keys) { $state[$key][$k] = if ($Fields[$k] -eq 'false') { $false } else { $Fields[$k] } }
            return @{ Ok = $true; Status = 204; Json = $null }
        }
        @{ Ok = $true; Status = 200; Json = [pscustomobject]$state[$key] }
    }.GetNewClosure()
}
$st = @{ fork = @{ approval_policy = 'first_time_contributors' }; wf = @{ default_workflow_permissions = 'write'; can_approve_pull_request_reviews = $true } }
$r = @(Set-RepoActionsPolicy -Repo 'o/r' -Invoker (New-Fake $st $false))
if (@($r | Where-Object Result -ne 'applied').Count) { throw "expected both applied: $($r.Result -join ',')" }
if (@($r | Where-Object { -not $_.Verified }).Count) { throw 'not verified' }
$r2 = @(Set-RepoActionsPolicy -Repo 'o/r' -Invoker (New-Fake $st $false))
if (@($r2 | Where-Object Result -ne 'already set').Count) { throw 'second run must be idempotent' }
if ($script:log -contains 'PUT repos/o/r/actions/permissions/workflow') { throw 'PUT when already set' }
$st2 = @{ fork = @{ approval_policy = 'first_time_contributors' }; wf = @{ default_workflow_permissions = 'read'; can_approve_pull_request_reviews = $false } }
$r3 = @(Set-RepoActionsPolicy -Repo 'o/r' -Invoker (New-Fake $st2 $true))
$f = $r3 | Where-Object Setting -eq 'fork-pr-approval'
if ($f.Result -ne 'forbidden-by-scope' -or $f.Verified -or -not $f.Manual) { throw 'forbidden must report manual step' }
if (($r3 | Where-Object Setting -eq 'workflow-permissions').Result -ne 'already set') { throw 'wf already set' }

# --- Stop-Runner with fakes (no processes, no network)
function New-StopFake($procsAfterGraceful, $statuses) {
    $s = @{ procs = @('L', 'W'); log = New-Object System.Collections.Generic.List[string]; st = [System.Collections.Generic.Queue[string]]::new([string[]]$statuses); sleeps = 0 }
    $s.GetProcs = { @($s.procs) }.GetNewClosure()
    $s.Graceful = { param($ProcId) $s.log.Add("graceful $ProcId"); if ($procsAfterGraceful -eq 'exit') { $s.procs = @() } }.GetNewClosure()
    $s.Force = { param($Procs) $s.log.Add('force'); $s.procs = @() }.GetNewClosure()
    $s.GetStatus = { param($n) if ($s.st.Count -gt 1) { $s.st.Dequeue() } else { $s.st.Peek() } }.GetNewClosure()
    $s.Sleep = { param($x) $s.sleeps++ }.GetNewClosure()
    $s
}
$pidf = Join-Path ([IO.Path]::GetTempPath()) "bb-test-$([guid]::NewGuid()).pid"
$script:logged = New-Object System.Collections.Generic.List[string]
$runCmd = 'D:' + [char]92 + 'bb' + [char]92 + 'runner' + [char]92 + 'run.cmd'
$when = [datetime]::UtcNow.AddMinutes(-5)
$script:procInfo = { param($i) @{ Name = 'cmd'; StartTime = $when; CommandLine = ('cmd.exe /c ' + [char]34 + $runCmd + [char]34) } }.GetNewClosure()
function Write-Pid($id, $t = $when) { Set-Content $pidf @($id, $t.ToString('o'), $runCmd) }
function Invoke-StopFake($s, $off = 90) {
    Stop-Runner -PidFile $pidf -OfflineSec $off -GracefulSec 3 -GetProcs $s.GetProcs -Graceful $s.Graceful -Force $s.Force `
        -GetProcInfo $script:procInfo -GetStatus $s.GetStatus -Sleep $s.Sleep -Log { param($m) $script:logged.Add($m) }
}
# graceful path: processes exit, runner goes online -> offline
Write-Pid 4242
$s = New-StopFake 'exit' @('online', 'online', 'offline')
if (-not (Invoke-StopFake $s)) { throw 'stop: graceful path must succeed' }
if ($s.log -notcontains 'graceful 4242') { throw 'stop: pid file not used' }
if ($s.log -contains 'force') { throw 'stop: forced although graceful worked' }
if (Test-Path $pidf) { throw 'stop: pid file not removed' }
# fallback: processes survive graceful stop -> forced
Write-Pid 4242
$s = New-StopFake 'stay' @('offline')
if (-not (Invoke-StopFake $s)) { throw 'stop: fallback path must succeed' }
if ($s.log -notcontains 'force') { throw 'stop: must force after timeout' }
# no pid file: still stops stragglers
$s = New-StopFake 'stay' @('absent')
if (-not (Invoke-StopFake $s)) { throw 'stop: no pid file must still succeed' }
if (@($s.log | Where-Object { $_ -like 'graceful*' }).Count) { throw 'stop: graceful without pid' }
# GitHub never reports offline -> failure
Write-Pid 4242
$s = New-StopFake 'exit' @('online')
if (Invoke-StopFake $s 10) { throw 'stop: must fail while GitHub says online' }
# processes that cannot be killed -> failure
$s = New-StopFake 'stay' @('offline'); $s.Force = { param($p) }
if (Invoke-StopFake $s) { throw 'stop: must fail when processes survive' }

# stale pid: process gone, wrong name, different start time, old one-line format -> ignored, no graceful, file deleted
foreach ($case in 'gone', 'name', 'time', 'oldformat') {
    switch ($case) {
        'gone'      { Write-Pid 4242; $script:procInfo = { param($i) $null } }
        'name'      { Write-Pid 4242; $script:procInfo = { param($i) @{ Name = 'explorer'; StartTime = $when; CommandLine = $runCmd } }.GetNewClosure() }
        'time'      { Write-Pid 4242; $script:procInfo = { param($i) @{ Name = 'cmd'; StartTime = $when.AddHours(1); CommandLine = $runCmd } }.GetNewClosure() }
        'oldformat' { Set-Content $pidf '4242'; $script:procInfo = { param($i) @{ Name = 'cmd'; StartTime = $when; CommandLine = $runCmd } }.GetNewClosure() }
    }
    $script:logged.Clear()
    $s = New-StopFake 'stay' @('offline')
    if (-not (Invoke-StopFake $s)) { throw "stop stale($case): name-based fallback must still succeed" }
    if (@($s.log | Where-Object { $_ -like 'graceful*' }).Count) { throw "stop stale($case): acted on a stale pid" }
    if ($script:logged -notcontains 'stale pid file, ignored') { throw "stop stale($case): not logged" }
    if (Test-Path $pidf) { throw "stop stale($case): pid file not deleted" }
    if ($s.log -notcontains 'force') { throw "stop stale($case): fallback must stop by name" }
}
# matching pid -> graceful path (covered above by the first cases with the default fake); assert explicitly
Write-Pid 4242
$script:procInfo = { param($i) @{ Name = 'cmd'; StartTime = $when.AddMilliseconds(400); CommandLine = ('cmd.exe /c ' + [char]34 + $runCmd + [char]34) } }.GetNewClosure()
$script:logged.Clear()
$s = New-StopFake 'exit' @('offline')
if (-not (Invoke-StopFake $s)) { throw 'stop match: must succeed' }
if ($s.log -notcontains 'graceful 4242') { throw 'stop match: graceful not used' }
if ($script:logged -contains 'stale pid file, ignored') { throw 'stop match: wrongly stale' }
Remove-Item $pidf -ErrorAction SilentlyContinue

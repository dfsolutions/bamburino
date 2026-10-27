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

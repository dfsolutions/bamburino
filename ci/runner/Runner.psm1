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

Export-ModuleMember -Function Get-RunnerSha256, New-RunnerConfigArgs, Set-RepoActionsPolicy, Invoke-GhApi

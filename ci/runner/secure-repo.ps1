# SPDX-License-Identifier: AGPL-3.0-or-later
# Applies and verifies the repository-side Actions safety settings for fork pull requests:
#   - require approval for all external contributors
#   - default GITHUB_TOKEN permissions read-only, Actions cannot approve PRs
# Exit code 1 if any setting is not in the desired state afterwards (manual step printed).
[CmdletBinding()]
param([string]$Repo = 'dfsolutions/bamburino')
$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot/../.."
Import-Module "$root/tools/GitHub.psm1" -Force
Import-Module "$PSScriptRoot/Runner.psm1" -Force
Set-GhToken
$res = @(Set-RepoActionsPolicy -Repo $Repo)
$bad = $false
foreach ($r in $res) {
    Write-Host ("{0}: {1}" -f $r.Setting, $r.Result)
    Write-Host "  endpoint: $($r.Endpoint)"
    Write-Host "  before:   $($r.Before)"
    Write-Host "  after:    $($r.After)"
    if (-not $r.Verified) { $bad = $true; Write-Warning "Set by hand: $($r.Manual)" }
}
if ($bad) { exit 1 }

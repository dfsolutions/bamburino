Import-Module "$PSScriptRoot/../Upstream.psm1" -Force
$u = Get-Upstream
if ($u['studio.tag'] -ne 'v02.08.04.57') { throw "tag: $($u['studio.tag'])" }
if ($u['studio.tag_commit'] -ne 'f977235e6d736c4c0b650520ac5a5b72cbfe9244') { throw 'tag_commit' }
if ($u['driver.pr_head'] -ne 'c31e97108e4d1ea137f90a075502e4d4536535fe') { throw 'pr_head' }
if ([int]$u['driver.port'] -ne 27183) { throw 'port' }
if (-not $u.ContainsKey('driver.patch_id') -or $u['driver.patch_id'] -notmatch '^([0-9a-f]{12})?$') { throw 'patch_id must be present and empty or 12 lowercase hex' }
if (@($u.Keys | Where-Object { $_ -like '*.' }).Count -ne 0) { throw 'spurious key ending with dot' }
$tmp = New-TemporaryFile; Copy-Item "$PSScriptRoot/../../driver/upstream.toml" $tmp
Set-UpstreamValue -Path $tmp -Key 'driver.patch_id' -Value 'abcdef012345'
if ((Get-Upstream -Path $tmp)['driver.patch_id'] -ne 'abcdef012345') { throw 'Set-UpstreamValue' }
if ((Get-Upstream -Path $tmp)['studio.tag'] -ne 'v02.08.04.57') { throw 'Set-UpstreamValue touched other keys' }

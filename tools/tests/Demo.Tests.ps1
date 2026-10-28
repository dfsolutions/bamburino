# SPDX-License-Identifier: AGPL-3.0-or-later
# Tests of tools/Demo.psm1 and the demo steps against a fake MCP endpoint (no network).
Import-Module "$PSScriptRoot/../Demo.psm1" -Force

function Assert-Eq($Actual, $Expected, $What) {
    if ("$Actual" -ne "$Expected") { throw "$What : expected '$Expected', got '$Actual'" }
}

# ---- wire helpers
$j = ConvertFrom-McpBody -Content '{"jsonrpc":"2.0","id":3,"result":{"x":1}}' -ContentType 'application/json'
Assert-Eq $j.result.x 1 'json body'
$sse = "id: 0`nretry: 3000`ndata: `n`nid: 1`nevent: message`ndata: {`"jsonrpc`":`"2.0`",`"id`":4,`"result`":{`"x`":2}}`n`n"
$s = ConvertFrom-McpBody -Content $sse -ContentType 'text/event-stream'
Assert-Eq $s.result.x 2 'sse body'
$t = Read-ClientSnippet "URL:    http://127.0.0.1:27600/mcp`r`nHeader: Authorization: Bearer abc123`r`n"
Assert-Eq $t.Url 'http://127.0.0.1:27600/mcp' 'snippet url'
Assert-Eq $t.Token 'abc123' 'snippet token'
$threw = $false; try { Read-ClientSnippet 'nope' | Out-Null } catch { $threw = $true }
if (-not $threw) { throw 'Read-ClientSnippet accepted garbage' }
$body = ConvertTo-RpcBody (New-RpcRequest -Id 7 -Method 'tools/call' -Params (New-ToolCallParams 'studio_get_state' @{}))
Assert-Eq $body '{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"studio_get_state","arguments":{}}}' 'request json'
Assert-Eq (ConvertTo-NormalizedAngle 270) -90 'angle 270'
if (-not (Test-AngleClose 270 -90)) { throw 'angle compare' }
if (Test-AngleClose 0 90) { throw 'angle compare false positive' }

# ---- fake endpoint
$script:Fake = $null
function New-Fake {
    @{ Revision = 100; Infill = '20%'; Pattern = 'cubic'; Walls = '2'; Layer = '0.2'; RotZ = 0.0
       Support = '0'; SupportType = 'normal(auto)'; PollsLeft = 2; Seen = (New-Object System.Collections.ArrayList); Tokens = @() }
}
function New-ToolReply($Id, $Payload, [bool]$IsError = $false) {
    $r = [ordered]@{ jsonrpc = '2.0'; id = $Id; result = [ordered]@{ content = @(@{ type = 'text'; text = 'x' }); structuredContent = $Payload; isError = $IsError } }
    ConvertTo-Json -InputObject $r -Depth 20 -Compress
}
function Get-Applied($f, $keys) {
    $map = @{ sparse_infill_density = $f.Infill; sparse_infill_pattern = $f.Pattern; wall_loops = $f.Walls; layer_height = $f.Layer; enable_support = $f.Support; support_type = $f.SupportType }
    @($keys | ForEach-Object { [ordered]@{ key = $_; applied = $map[$_] } })
}
$transport = {
    param($Url, $Headers, $Body)
    $f = $script:Fake
    $f.Tokens += $Headers['Authorization']
    $m = ConvertFrom-Json $Body
    [void]$f.Seen.Add($m.method)
    if ($m.method -eq 'initialize') {
        return [pscustomobject]@{ Status = 200; ContentType = 'application/json'; SessionId = 'sess-1'
            Content = '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"fake","version":"0"}}}' }
    }
    if ($m.method -eq 'notifications/initialized') {
        if ($Headers['Mcp-Session-Id'] -ne 'sess-1') { throw 'session id not echoed' }
        return [pscustomobject]@{ Status = 202; ContentType = ''; Content = ''; SessionId = $null }
    }
    $a = $m.params.arguments
    $out = switch ($m.params.name) {
        'studio_get_state' {
            New-ToolReply $m.id ([ordered]@{ project = @{ name = 'cube' }; plates = @(@{ plate_id = 0 }); models = @(@{ object_id = 'obj-1' })
                    presets = @{ process = '0.20mm Standard' }; revision = $f.Revision })
        }
        'studio_set_settings' {
            if ([long]$a.expected_revision -ne $f.Revision) {
                New-ToolReply $m.id ([ordered]@{ error = [ordered]@{ code = 'REVISION_CONFLICT'; message = 'changed'; http_status = 409
                            details = [ordered]@{ reason = 'revision_mismatch'; expected = $a.expected_revision; current = $f.Revision } } }) $true
            } else {
                $from = $f.Infill
                foreach ($c in $a.changes) {
                    switch ($c.key) {
                        'sparse_infill_density' { $f.Infill = "$($c.value)%" }
                        'sparse_infill_pattern' { $f.Pattern = $c.value }
                        'wall_loops' { $f.Walls = "$($c.value)" }
                        'layer_height' { $f.Layer = "$($c.value)" }
                    }
                }
                $f.Revision++
                New-ToolReply $m.id ([ordered]@{ applied = (Get-Applied $f @($a.changes | ForEach-Object { $_.key }))
                        diff = @([ordered]@{ key = 'sparse_infill_density'; from = $from }); revision = $f.Revision; dry_run = $false })
            }
        }
        'studio_transform_model' {
            $before = $f.RotZ
            $f.RotZ = ConvertTo-NormalizedAngle ($f.RotZ + [double]@($a.transform.rotate_deg)[2])
            $f.Revision++
            New-ToolReply $m.id ([ordered]@{ object_id = $a.object_id; before = @{ rotation_deg = @(0, 0, $before) }
                    after = @{ rotation_deg = @(0, 0, $f.RotZ) }; reread = @{ rotation_deg = @(0, 0, $f.RotZ) }; revision = $f.Revision })
        }
        'studio_set_supports' {
            $f.Support = '1'; $f.SupportType = 'tree(auto)'; $f.Revision++
            New-ToolReply $m.id ([ordered]@{ applied = (Get-Applied $f @('enable_support', 'support_type')); revision = $f.Revision })
        }
        'studio_slice' { New-ToolReply $m.id ([ordered]@{ job_id = 'job-1'; plate_id = 0; status = 'running' }) }
        'studio_slice_status' {
            $st = if ($f.PollsLeft -gt 0) { $f.PollsLeft--; 'running' } else { 'succeeded' }
            New-ToolReply $m.id ([ordered]@{ job_id = 'job-1'; status = $st; error = $null })
        }
        'studio_get_slice_summary' {
            New-ToolReply $m.id ([ordered]@{ plate_id = 0; print_time_s = 1830; filaments = @(@{ index = 0; grams = 12.5 }); revision = $f.Revision })
        }
        default { throw "unexpected tool $($m.params.name)" }
    }
    [pscustomobject]@{ Status = 200; ContentType = 'application/json'; Content = $out; SessionId = $null }
}

# ---- full flow
$script:Fake = New-Fake
$sess = New-McpSession -BaseUrl 'http://fake/mcp' -Token 'tok' -Transport $transport
Initialize-McpSession $sess
Assert-Eq $script:Fake.Tokens[0] 'Bearer tok' 'bearer header'
$prompted = 0
# The operator edits the infill by hand: the fake bumps its revision.
$prompt = { param($msg) $script:Fake.Infill = '20%'; $script:Fake.Revision += 5; $script:prompted++ }
$r2 = Invoke-DemoStep2 $sess; Assert-Eq $r2.Result 'verified' "step 2: $($r2.Observed)"
$r3 = Invoke-DemoStep3 $sess; Assert-Eq $r3.Result 'verified' "step 3: $($r3.Observed)"
Assert-Eq $script:Fake.Infill '35%' 'infill after step 3'
$r5 = Invoke-DemoStep5 $sess $prompt; Assert-Eq $r5.Result 'verified' "step 5: $($r5.Observed)"
Assert-Eq $prompted 1 'operator prompted once'
$r6 = Invoke-DemoStep6 $sess; Assert-Eq $r6.Result 'verified' "step 6: $($r6.Observed)"
$r7 = Invoke-DemoStep7 $sess; Assert-Eq $r7.Result 'verified' "step 7: $($r7.Observed)"
$r8 = Invoke-DemoStep8 $sess -Sleep { param($s) }; Assert-Eq $r8.Result 'verified' "step 8: $($r8.Observed)"
if ($r8.Observed -notmatch 'print time 30\.5 min' -or $r8.Observed -notmatch 'grams 12.5') { throw "step 8 observed: $($r8.Observed)" }

# ---- a failing step is reported as failed
$script:Fake = New-Fake
$sess = New-McpSession -BaseUrl 'http://fake/mcp' -Token 'tok' -Transport $transport
Initialize-McpSession $sess
Invoke-DemoStep2 $sess | Out-Null
$sess.Revision = 1   # stale on purpose
$bad = Invoke-DemoStep3 $sess
Assert-Eq $bad.Result 'failed' 'stale revision fails step 3'
if ($bad.Observed -notmatch 'REVISION_CONFLICT') { throw "observed: $($bad.Observed)" }

# step 5 fails when the old revision is still accepted (no manual edit happened)
$script:Fake = New-Fake
$sess = New-McpSession -BaseUrl 'http://fake/mcp' -Token 'tok' -Transport $transport
Initialize-McpSession $sess
Invoke-DemoStep2 $sess | Out-Null
$no = Invoke-DemoStep5 $sess { param($m) }
Assert-Eq $no.Result 'failed' 'step 5 without a manual edit'

# ---- dry run sends nothing and prints the calls
$script:Fake = New-Fake
$dry = New-McpSession -BaseUrl 'http://fake/mcp' -Token 'x' -Transport { throw 'network used in dry run' } -DryRun
Initialize-McpSession $dry
foreach ($step in 2, 3, 6, 7, 8) { & "Invoke-DemoStep$step" $dry | Out-Null }
Invoke-DemoStep5 $dry { throw 'prompted in dry run' } | Out-Null
if ($dry.Log.Count -lt 10) { throw "dry run recorded $($dry.Log.Count) calls" }

# ---- acceptance file rows
$tmp = New-TemporaryFile
Copy-Item "$PSScriptRoot/../../docs/acceptance-mvp-service.md" $tmp -Force
$rows = (Get-Content $tmp | Where-Object { $_ -match '^\| (\d|4b|4c)' })
Assert-Eq $rows.Count 12 'skeleton row count'
foreach ($row in $rows) {
    $cells = @($row.Trim().Trim('|').Split('|') | ForEach-Object { $_.Trim() })
    if ($cells[3] -ne 'not run') { throw "skeleton observed not 'not run': $row" }
    if ($cells[4] -notin @('not run', 'manual')) { throw "skeleton result: $row" }
}
Update-AcceptanceRow -Path $tmp -Step '3' -Result 'verified' -Observed 'infill 35%|gyroid' -Date '2026-10-28'
$row3 = Get-Content $tmp | Where-Object { $_ -match '^\| 3 \|' }
if ($row3 -notmatch '\| infill 35%/gyroid \| verified \| 2026-10-28 \|$') { throw "row 3: $row3" }
Assert-Eq (@(Get-Content $tmp | Where-Object { $_ -match '^\| 4b \|' }).Count) 1 'row 4b untouched'
Remove-Item $tmp

# SPDX-License-Identifier: AGPL-3.0-or-later
# Helpers of tools/demo.ps1: plain JSON-RPC (MCP Streamable HTTP) against the
# Bamburino service and the acceptance steps of design section 9. Everything
# that touches the network goes through a transport scriptblock, so the tests
# run against a fake endpoint.

Set-StrictMode -Version Latest

# ---------------------------------------------------------------- wire format

function New-RpcRequest {
    param([int]$Id, [string]$Method, $Params = $null)
    $o = [ordered]@{ jsonrpc = '2.0'; id = $Id; method = $Method }
    if ($null -ne $Params) { $o.params = $Params }
    $o
}

function New-RpcNotification {
    param([string]$Method)
    [ordered]@{ jsonrpc = '2.0'; method = $Method }
}

function New-ToolCallParams {
    param([string]$Name, $Arguments = @{})
    [ordered]@{ name = $Name; arguments = $Arguments }
}

function ConvertTo-RpcBody {
    param($Object)
    ConvertTo-Json -InputObject $Object -Depth 20 -Compress
}

# A response body is either a JSON object or an SSE stream whose `data:` lines
# carry JSON-RPC messages. Returns the message with an `id` (or the last one).
function ConvertFrom-McpBody {
    param([string]$Content, [string]$ContentType = '')
    if ([string]::IsNullOrWhiteSpace($Content)) { return $null }
    $messages = @()
    $head = $Content.TrimStart()
    if ($ContentType -match 'text/event-stream' -or $head.StartsWith('data:') -or $head.StartsWith('id:') -or $head.StartsWith('event:')) {
        $data = New-Object System.Text.StringBuilder
        foreach ($line in ($Content -split "`r?`n")) {
            if ($line -match '^data:\s?(.*)$') { [void]$data.Append($Matches[1]) }
            elseif ($line -eq '' -and $data.Length -gt 0) {
                $messages += ConvertFrom-Json $data.ToString()
                [void]$data.Clear()
            }
        }
        if ($data.Length -gt 0) { $messages += ConvertFrom-Json $data.ToString() }
    } else {
        $messages += ConvertFrom-Json $Content
    }
    $withId = @($messages | Where-Object { $_.PSObject.Properties['id'] -and $null -ne $_.id })
    if ($withId.Count -gt 0) { return $withId[-1] }
    if ($messages.Count -gt 0) { return $messages[-1] }
    $null
}

# Turns a tools/call JSON-RPC response into { IsError; Payload; Code; Message; Details }.
# Payload is `structuredContent`, else the JSON in the first text block.
function Get-ToolResult {
    param($Rpc)
    if ($null -eq $Rpc) { throw 'empty response' }
    if ($Rpc.PSObject.Properties['error'] -and $Rpc.error) {
        return [pscustomobject]@{ IsError = $true; Payload = $null; Code = "RPC$($Rpc.error.code)"; Message = [string]$Rpc.error.message; Details = $null }
    }
    $r = $Rpc.result
    $payload = $null
    if ($r.PSObject.Properties['structuredContent'] -and $null -ne $r.structuredContent) {
        $payload = $r.structuredContent
    } elseif ($r.PSObject.Properties['content'] -and @($r.content).Count -gt 0) {
        $text = [string]@($r.content)[0].text
        try { $payload = ConvertFrom-Json $text } catch { $payload = [pscustomobject]@{ text = $text } }
    }
    $isErr = ($r.PSObject.Properties['isError'] -and [bool]$r.isError)
    $code = $null; $msg = $null; $details = $null
    if ($isErr -and $payload -and $payload.PSObject.Properties['error']) {
        $code = [string]$payload.error.code
        $msg = [string]$payload.error.message
        $details = $payload.error.details
    }
    [pscustomobject]@{ IsError = $isErr; Payload = $payload; Code = $code; Message = $msg; Details = $details }
}

# Parses the output of `bamburino config client-snippet --client generic`:
#   URL:    http://127.0.0.1:27600/mcp
#   Header: Authorization: Bearer <token>
function Read-ClientSnippet {
    param([string]$Text)
    $url = $null; $token = $null
    foreach ($line in ($Text -split "`r?`n")) {
        if ($line -match '^\s*URL:\s*(\S+)') { $url = $Matches[1] }
        elseif ($line -match 'Authorization:\s*Bearer\s+(\S+)') { $token = $Matches[1] }
    }
    if (-not $url -or -not $token) { throw 'client snippet not recognized (expected the output of: bamburino config client-snippet --client generic)' }
    [pscustomobject]@{ Url = $url; Token = $token }
}

# ---------------------------------------------------------------- session

function Invoke-HttpTransport {
    param([string]$Url, [hashtable]$Headers, [string]$Body)
    $r = Invoke-WebRequest -Uri $Url -Method Post -Headers $Headers -Body $Body `
        -ContentType 'application/json' -SkipHttpErrorCheck -UseBasicParsing
    $sid = $r.Headers['Mcp-Session-Id']
    [pscustomobject]@{
        Status      = [int]$r.StatusCode
        ContentType = [string]($r.Headers['Content-Type'] | Select-Object -First 1)
        Content     = [string]$r.Content
        SessionId   = if ($sid) { [string]($sid | Select-Object -First 1) } else { $null }
    }
}

function New-McpSession {
    param(
        [string]$BaseUrl,
        [string]$Token,
        [scriptblock]$Transport = { param($Url, $Headers, $Body) Invoke-HttpTransport -Url $Url -Headers $Headers -Body $Body },
        [switch]$DryRun
    )
    @{ BaseUrl = $BaseUrl; Token = $Token; Transport = $Transport; DryRun = [bool]$DryRun
       SessionId = $null; NextId = 1; Revision = $(if ($DryRun) { '<revision>' } else { $null }); Log = (New-Object System.Collections.ArrayList) }
}

# Sends one message. Returns the parsed JSON-RPC response (or $null for a
# notification). In dry-run mode only records and prints the call.
function Invoke-McpRpc {
    param($Session, $Message, [switch]$Notification)
    $body = ConvertTo-RpcBody $Message
    [void]$Session.Log.Add($body)
    if ($Session.DryRun) {
        Write-Host "  > $body"
        return $null
    }
    $headers = @{ Accept = 'application/json, text/event-stream'; Authorization = "Bearer $($Session.Token)" }
    if ($Session.SessionId) { $headers['Mcp-Session-Id'] = $Session.SessionId }
    $resp = & $Session.Transport $Session.BaseUrl $headers $body
    if ($resp.Status -eq 401) { throw '401 unauthorized: the client token is wrong (bamburino config client-snippet)' }
    if ($resp.Status -eq 403) { throw '403 forbidden: Host/Origin check failed' }
    if ($resp.Status -ge 400) { throw "HTTP $($resp.Status): $($resp.Content)" }
    if ($resp.SessionId) { $Session.SessionId = $resp.SessionId }
    if ($Notification) { return $null }
    ConvertFrom-McpBody -Content $resp.Content -ContentType $resp.ContentType
}

function Initialize-McpSession {
    param($Session)
    $id = $Session.NextId++
    $init = New-RpcRequest -Id $id -Method 'initialize' -Params ([ordered]@{
            protocolVersion = '2025-06-18'
            capabilities    = @{}
            clientInfo      = [ordered]@{ name = 'bamburino-demo'; version = '1' }
        })
    $r = Invoke-McpRpc -Session $Session -Message $init
    if (-not $Session.DryRun -and ($null -eq $r -or ($r.PSObject.Properties['error'] -and $r.error))) {
        throw "initialize failed: $($r.error.message)"
    }
    Invoke-McpRpc -Session $Session -Message (New-RpcNotification 'notifications/initialized') -Notification | Out-Null
}

function Invoke-McpTool {
    param($Session, [string]$Name, $Arguments = @{})
    $id = $Session.NextId++
    $rpc = Invoke-McpRpc -Session $Session -Message (New-RpcRequest -Id $id -Method 'tools/call' -Params (New-ToolCallParams $Name $Arguments))
    if ($Session.DryRun) {
        return [pscustomobject]@{ IsError = $false; Payload = $null; Code = $null; Message = $null; Details = $null }
    }
    $res = Get-ToolResult $rpc
    if ($res.Payload -and $res.Payload.PSObject.Properties['revision'] -and $null -ne $res.Payload.revision) {
        $Session.Revision = $res.Payload.revision
    }
    $res
}

# ---------------------------------------------------------------- checks

function Test-NumberClose {
    param($A, $B, [double]$Tolerance = 1e-6)
    try { [math]::Abs([double]$A - [double]$B) -le $Tolerance } catch { $false }
}

# Angle in (-180, 180].
function ConvertTo-NormalizedAngle {
    param([double]$Deg)
    $x = $Deg % 360
    if ($x -gt 180) { $x -= 360 }
    if ($x -le -180) { $x += 360 }
    $x
}

function Test-AngleClose {
    param($A, $B, [double]$Tolerance = 0.01)
    try {
        $d = ConvertTo-NormalizedAngle ([double]$A - [double]$B)
        [math]::Abs($d) -le $Tolerance
    } catch { $false }
}

function Get-AppliedValue {
    param($Applied, [string]$Key)
    foreach ($a in @($Applied)) { if ($a.key -eq $Key) { return [string]$a.applied } }
    $null
}

function New-StepResult {
    param([string]$Step, [string]$Result, [string]$Observed)
    [pscustomobject]@{ Step = $Step; Result = $Result; Observed = $Observed }
}

# ---------------------------------------------------------------- steps

function Get-SettingsChanges {
    @(
        [ordered]@{ key = 'sparse_infill_density'; value = 35 },
        [ordered]@{ key = 'sparse_infill_pattern'; value = 'gyroid' },
        [ordered]@{ key = 'wall_loops'; value = 4 },
        [ordered]@{ key = 'layer_height'; value = 0.2 }
    )
}

function Invoke-DemoStep2 {
    param($S)
    $r = Invoke-McpTool $S 'studio_get_state'
    if ($S.DryRun) { return New-StepResult '2' 'not run' 'dry run' }
    if ($r.IsError) { return New-StepResult '2' 'failed' "$($r.Code): $($r.Message)" }
    $p = $r.Payload
    $plates = @($p.plates).Count; $models = @($p.models).Count
    $ok = ($null -ne $p.revision) -and $plates -ge 1 -and $models -ge 1
    $obs = "project '$($p.project.name)', $plates plate(s), $models object(s), process '$($p.presets.process)', revision $($p.revision)"
    New-StepResult '2' ($(if ($ok) { 'verified' } else { 'failed' })) $obs
}

function Invoke-DemoStep3 {
    param($S)
    $r = Invoke-McpTool $S 'studio_set_settings' ([ordered]@{ changes = (Get-SettingsChanges); expected_revision = $S.Revision })
    if ($S.DryRun) { return New-StepResult '3' 'not run' 'dry run' }
    if ($r.IsError) { return New-StepResult '3' 'failed' "$($r.Code): $($r.Message)" }
    $ap = $r.Payload.applied
    $inf = Get-AppliedValue $ap 'sparse_infill_density'
    $pat = Get-AppliedValue $ap 'sparse_infill_pattern'
    $wl = Get-AppliedValue $ap 'wall_loops'
    $lh = Get-AppliedValue $ap 'layer_height'
    $ok = ($inf -eq '35%') -and ($pat -eq 'gyroid') -and ($wl -eq '4') -and (Test-NumberClose $lh 0.2)
    $obs = "infill $inf, pattern $pat, walls $wl, layer $lh, new revision $($r.Payload.revision)"
    New-StepResult '3' ($(if ($ok) { 'verified' } else { 'failed' })) $obs
}

# $Prompt is called when the operator must act in Studio (it waits for Enter).
function Invoke-DemoStep5 {
    param($S, [scriptblock]$Prompt)
    $old = $S.Revision
    if (-not $S.DryRun) {
        & $Prompt 'In Bambu Studio, change the infill density to 20% by hand (process panel), then come back here.'
    } else { Write-Host '  (operator action: change infill to 20% in Studio)' }
    $changes = @([ordered]@{ key = 'sparse_infill_density'; value = 35 })
    $r1 = Invoke-McpTool $S 'studio_set_settings' ([ordered]@{ changes = $changes; expected_revision = $old })
    if ($S.DryRun) {
        Invoke-McpTool $S 'studio_set_settings' ([ordered]@{ changes = $changes; expected_revision = '<details.current>' }) | Out-Null
        return New-StepResult '5' 'not run' 'dry run'
    }
    if (-not $r1.IsError) {
        return New-StepResult '5' 'failed' "the write with the old revision $old was accepted (expected 409 REVISION_CONFLICT); was the infill changed by hand?"
    }
    if ($r1.Code -ne 'REVISION_CONFLICT') { return New-StepResult '5' 'failed' "expected REVISION_CONFLICT, got $($r1.Code): $($r1.Message)" }
    $cur = if ($r1.Details) { $r1.Details.current } else { $null }
    if ($null -eq $cur) { return New-StepResult '5' 'failed' '409 without details.current' }
    $r2 = Invoke-McpTool $S 'studio_set_settings' ([ordered]@{ changes = $changes; expected_revision = $cur })
    if ($r2.IsError) { return New-StepResult '5' 'failed' "409 ok (current $cur) but the retry failed: $($r2.Code): $($r2.Message)" }
    $from = $null
    foreach ($d in @($r2.Payload.diff)) { if ($d.key -eq 'sparse_infill_density') { $from = $d.from } }
    $applied = Get-AppliedValue $r2.Payload.applied 'sparse_infill_density'
    $ok = ($applied -eq '35%') -and ($from -eq '20%')
    $obs = "old revision $old -> 409 REVISION_CONFLICT (current $cur); retry ok, infill before the write $from, applied $applied, revision $($r2.Payload.revision)"
    New-StepResult '5' ($(if ($ok) { 'verified' } else { 'failed' })) $obs
}

function Invoke-DemoStep6 {
    param($S)
    $st = Invoke-McpTool $S 'studio_get_state'
    if ($S.DryRun) {
        Invoke-McpTool $S 'studio_transform_model' ([ordered]@{ object_id = '<first object>'; transform = @{ rotate_deg = @(0, 0, 90) }; expected_revision = '<revision>' }) | Out-Null
        return New-StepResult '6' 'not run' 'dry run'
    }
    if ($st.IsError) { return New-StepResult '6' 'failed' "$($st.Code): $($st.Message)" }
    $obj = @($st.Payload.models)[0]
    if (-not $obj) { return New-StepResult '6' 'failed' 'no object in the project' }
    $r = Invoke-McpTool $S 'studio_transform_model' ([ordered]@{
            object_id = $obj.object_id; transform = [ordered]@{ rotate_deg = @(0, 0, 90) }; expected_revision = $S.Revision })
    if ($r.IsError) { return New-StepResult '6' 'failed' "$($r.Code): $($r.Message)" }
    $bz = @($r.Payload.before.rotation_deg)[2]; $az = @($r.Payload.after.rotation_deg)[2]; $rz = @($r.Payload.reread.rotation_deg)[2]
    $ok = (Test-AngleClose $az ([double]$bz + 90)) -and (Test-AngleClose $rz ([double]$bz + 90))
    $obs = "object $($obj.object_id): Z $bz -> $az (re-read $rz; angles are normalized to -180..180), revision $($r.Payload.revision)"
    New-StepResult '6' ($(if ($ok) { 'verified' } else { 'failed' })) $obs
}

function Invoke-DemoStep7 {
    param($S)
    $a = [ordered]@{
        target            = [ordered]@{ scope = 'project' }
        settings          = [ordered]@{ enabled = $true; type = 'tree' }
        expected_revision = $S.Revision
    }
    $r = Invoke-McpTool $S 'studio_set_supports' $a
    if ($S.DryRun) { return New-StepResult '7' 'not run' 'dry run' }
    if ($r.IsError) { return New-StepResult '7' 'failed' "$($r.Code): $($r.Message)" }
    $en = Get-AppliedValue $r.Payload.applied 'enable_support'
    $ty = Get-AppliedValue $r.Payload.applied 'support_type'
    $ok = ($en -in @('1', 'true', 'True')) -and ($ty -like 'tree*')
    New-StepResult '7' ($(if ($ok) { 'verified' } else { 'failed' })) "enable_support $en, support_type $ty, revision $($r.Payload.revision)"
}

function Invoke-DemoStep8 {
    param($S, [int]$TimeoutSec = 900, [int]$PollSec = 2, [scriptblock]$Sleep = { param($s) Start-Sleep -Seconds $s })
    $r = Invoke-McpTool $S 'studio_slice' @{}
    if ($S.DryRun) {
        Invoke-McpTool $S 'studio_slice_status' @{ job_id = '<job_id>' } | Out-Null
        Invoke-McpTool $S 'studio_get_slice_summary' @{} | Out-Null
        return New-StepResult '8' 'not run' 'dry run'
    }
    if ($r.IsError) { return New-StepResult '8' 'failed' "$($r.Code): $($r.Message)" }
    $job = $r.Payload.job_id
    $deadline = [datetime]::UtcNow.AddSeconds($TimeoutSec)
    $status = $null; $polls = 0
    while ($true) {
        $js = Invoke-McpTool $S 'studio_slice_status' @{ job_id = $job }
        $polls++
        if ($js.IsError) { return New-StepResult '8' 'failed' "status: $($js.Code): $($js.Message)" }
        $status = $js.Payload.status
        if ($status -ne 'running') { break }
        if ([datetime]::UtcNow -gt $deadline) { return New-StepResult '8' 'failed' "job $job still running after $TimeoutSec s" }
        & $Sleep $PollSec
    }
    if ($status -ne 'succeeded') {
        $em = if ($js.Payload.error) { "$($js.Payload.error.code) $($js.Payload.error.message)" } else { $js.Payload.message }
        return New-StepResult '8' 'failed' "job $job ended $status after $polls poll(s): $em"
    }
    $sm = Invoke-McpTool $S 'studio_get_slice_summary' @{}
    if ($sm.IsError) { return New-StepResult '8' 'failed' "job succeeded but the summary failed: $($sm.Code): $($sm.Message)" }
    $t = $sm.Payload.print_time_s
    $inv = [cultureinfo]::InvariantCulture
    $grams = @(@($sm.Payload.filaments) | Where-Object { $null -ne $_.grams } | ForEach-Object { ([double]$_.grams).ToString($inv) })
    $ok = ($null -ne $t) -and $grams.Count -ge 1
    $tt = if ($null -ne $t) { (([double]$t / 60).ToString('0.#', $inv)) + ' min' } else { 'n/a' }
    $obs = "job $job succeeded after $polls poll(s); print time $tt ($t s), grams $($grams -join '/')"
    New-StepResult '8' ($(if ($ok) { 'verified' } else { 'failed' })) $obs
}

# ---------------------------------------------------------------- acceptance file

# Rewrites the row of $Step in the markdown table: observed, result, date.
function Update-AcceptanceRow {
    param([string]$Path, [string]$Step, [string]$Result, [string]$Observed, [string]$Date = (Get-Date -Format 'yyyy-MM-dd'))
    $lines = Get-Content -LiteralPath $Path
    $found = $false
    $out = foreach ($l in $lines) {
        if ($l -match ('^\|\s*' + [regex]::Escape($Step) + '\s*\|')) {
            $cells = @($l.Trim().Trim('|').Split('|') | ForEach-Object { $_.Trim() })
            if ($cells.Count -ne 6) { throw "row $Step has $($cells.Count) cells, expected 6" }
            $clean = ($Observed -replace '\|', '/' -replace '\r?\n', ' ')
            $found = $true
            "| $($cells[0]) | $($cells[1]) | $($cells[2]) | $clean | $Result | $Date |"
        } else { $l }
    }
    if (-not $found) { throw "step $Step not found in $Path" }
    Set-Content -LiteralPath $Path -Value $out -Encoding utf8
}

Export-ModuleMember -Function *-*

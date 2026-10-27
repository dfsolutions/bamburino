# Checks docs/driver-audit.md against the tool manifest frozen inside the driver patch.
Set-StrictMode -Version Latest
$patch = "$PSScriptRoot/../../driver/patches/0001-pr12485-native-mcp-server.diff"
$audit = "$PSScriptRoot/../../docs/driver-audit.md"
if (-not (Test-Path $patch)) { throw 'patch missing' }
if (-not (Test-Path $audit)) { throw 'docs/driver-audit.md missing' }

# Tool names: '"name": "..."' lines of McpToolsManifest.json inside the diff (added lines only).
$names = New-Object System.Collections.Generic.List[string]
$inManifest = $false
foreach ($line in [System.IO.File]::ReadAllLines((Resolve-Path $patch).Path)) {
    if ($line -like 'diff --git *') { $inManifest = $line -match 'McpToolsManifest\.json$'; continue }
    if ($inManifest -and $line -match '^\+\s*"name"\s*:\s*"([^"]+)"') { $names.Add($Matches[1]) }
}
if ($names.Count -ne 90) { throw "expected 90 tools in manifest, got $($names.Count)" }

$header = 'driver tool | method | file/function | live GUI | take_snapshot | revision | Windows | Bamburino tool'
$lines = [System.IO.File]::ReadAllLines((Resolve-Path $audit).Path)
$hi = -1
for ($i = 0; $i -lt $lines.Count; $i++) {
    $cells = ($lines[$i].Trim().Trim('|').Split('|') | ForEach-Object { $_.Trim() }) -join ' | '
    if ($cells -eq $header) { $hi = $i; break }
}
if ($hi -lt 0) { throw "audit table header not found: $header" }

$rows = @()
for ($i = $hi + 2; $i -lt $lines.Count -and $lines[$i].Trim().StartsWith('|'); $i++) {
    $rows += , @($lines[$i].Trim().Trim('|').Split('|') | ForEach-Object { $_.Trim().Trim('`') })
}
if ($rows.Count -ne 90) { throw "expected 90 data rows, got $($rows.Count)" }

$first = @($rows | ForEach-Object { $_[0] })
foreach ($n in $names) { if ($first -notcontains $n) { throw "tool missing from audit: $n" } }
foreach ($r in $rows) {
    if ($r.Count -ne 8) { throw "row '$($r[0])' has $($r.Count) cells, expected 8" }
    if ($r[6] -notmatch '^(not run|verified \(\d{4}-\d{2}-\d{2}\)|failed \(\d{4}-\d{2}-\d{2}\))$') {
        throw "row '$($r[0])' bad Windows cell: $($r[6])"
    }
}

$fi = [array]::IndexOf($lines, '## Findings')
if ($fi -lt 0) { throw '## Findings section missing' }
$findings = @()
for ($i = $fi + 1; $i -lt $lines.Count -and -not $lines[$i].StartsWith('## '); $i++) { $findings += $lines[$i] }
foreach ($path in 'settings.update', 'object.transform', 'settings.override') {
    $hit = @($findings | Where-Object { $_.StartsWith("take_snapshot ${path}:") -and $_ -match '\b(present|missing)\b' })
    if ($hit.Count -ne 1) { throw "Findings: expected one 'take_snapshot ${path}:' line with present|missing, got $($hit.Count)" }
}

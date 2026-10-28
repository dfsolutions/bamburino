# Undo snapshots (Task 7): after driver/build.ps1 -Step patch, the three mutating MCP paths take an undo snapshot
# before they mutate, and, when patch 0120 exists, app.get_state exposes "undoSnapshot" (contract with plan B
# AppState.undo_snapshot) read from Plater::get_active_snapshot_time().
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module "$PSScriptRoot/../Upstream.psm1" -Force
$repo = (Resolve-Path "$PSScriptRoot/../..").Path
$build = Join-Path $repo 'driver/build.ps1'
$clone = Join-Path (Get-BuildRoot) 'BambuStudio'
if (-not (Test-Path -LiteralPath (Join-Path $clone '.git'))) { throw "clone missing at ${clone}: run driver/build.ps1 -Step fetch first" }

$out = & pwsh -NoProfile -NonInteractive -File $build -Step patch 2>&1
if ($LASTEXITCODE -ne 0) { throw "-Step patch exit ${LASTEXITCODE}: $($out -join "`n")" }

$lines = [string[]](Get-Content -LiteralPath (Join-Path $clone 'src/slic3r/GUI/McpTools.cpp'))

# Body of a handler: from the line matching $Start to the first following line matching $End (inclusive).
function Get-Body([string]$Name, [string]$Start, [string]$End) {
    $s = -1
    for ($i = 0; $i -lt $lines.Count; $i++) { if ($lines[$i] -match $Start) { $s = $i; break } }
    if ($s -lt 0) { throw "${Name}: start of handler not found ($Start)" }
    for ($e = $s + 1; $e -lt $lines.Count; $e++) { if ($lines[$e] -match $End) { return , [string[]]$lines[$s..$e] } }
    throw "${Name}: end of handler not found ($End)"
}
function Get-FirstLine([string[]]$Body, [string]$Pattern) {
    for ($i = 0; $i -lt $Body.Count; $i++) { if ($Body[$i] -match $Pattern) { return $i } }
    return -1
}
# take_snapshot( must appear inside the handler, on a line before the first mutation.
function Assert-SnapshotFirst([string]$Name, [string[]]$Body, [string]$Mutation) {
    $snap = Get-FirstLine $Body 'take_snapshot\('
    $mut = Get-FirstLine $Body $Mutation
    if ($mut -lt 0) { throw "${Name}: mutation line not found ($Mutation)" }
    if ($snap -lt 0) { throw "${Name}: no take_snapshot( in the handler" }
    if ($snap -ge $mut) { throw "${Name}: take_snapshot( (handler line $snap) is not before the mutation (handler line $mut)" }
}

$update = Get-Body 'settings.update' '^\s*\{"settings\.update", \[\]\(Plater& p, const Json& a\) \{' '^\s*\}\}'
Assert-SnapshotFirst 'settings.update' $update 'tab->load_config\(candidate\)'

$transform = Get-Body 'object.transform' '^\s*\{"object\.transform", \[\]\(Plater& p, const Json& a\) \{' '^\s*\}\},?$'
Assert-SnapshotFirst 'object.transform' $transform 'instance->set_(offset|rotation|scaling_factor)\('

# settings.override dispatches to update_overrides.
if (-not ($lines -match '\{"settings\.override", \[\]\(Plater& p, const Json& a\) \{ return update_overrides\(p, a\); \}\}')) {
    throw 'settings.override no longer dispatches to update_overrides'
}
$override = Get-Body 'settings.override' '^Json update_overrides\(Plater& plater, const Json& params\) \{' '^\}'
Assert-SnapshotFirst 'settings.override' $override '(\*plate_config = std::move\(candidate\)|assign_config\(std::move\(candidate\)\))'

# Patch 0120: app_state carries "undoSnapshot" as a decimal string of the active undo snapshot time.
if (Test-Path -LiteralPath (Join-Path $repo 'driver/patches/0120-expose-undo-snapshot.diff')) {
    $state = Get-Body 'app_state' '^Json app_state\(Plater& plater\) \{' '^\}'
    $joined = $state -join "`n"
    if ($joined -notmatch '\{"undoSnapshot", std::to_string\(plater\.get_active_snapshot_time\(\)\)\}') {
        throw 'app_state lacks {"undoSnapshot", std::to_string(plater.get_active_snapshot_time())}'
    }
}

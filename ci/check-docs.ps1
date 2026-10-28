# SPDX-License-Identifier: AGPL-3.0-or-later
# Checks README.md and docs/*.md (not docs/superpowers/): English only, status vocabulary,
# no stale Studio version, working relative links, no token-like strings.
# Usage: pwsh ci/check-docs.ps1 [-Root <repo root>]      exit 0 clean, 1 with a list of problems.
# Also runs under Windows PowerShell 5.1.
[CmdletBinding()]
param([string]$Root)
$ErrorActionPreference = 'Stop'
if (-not $Root) { $Root = Join-Path $PSScriptRoot '..' }
$Root = (Resolve-Path -LiteralPath $Root).Path
$problems = New-Object System.Collections.Generic.List[string]

$files = New-Object System.Collections.Generic.List[string]
if (Test-Path -LiteralPath (Join-Path $Root 'README.md')) { $files.Add((Join-Path $Root 'README.md')) }
$docs = Join-Path $Root 'docs'
if (Test-Path -LiteralPath $docs) {
    Get-ChildItem -LiteralPath $docs -Filter '*.md' -File | Sort-Object Name | ForEach-Object { $files.Add($_.FullName) }
}

$italian = @('verificato', 'verificata', 'verificati', 'non eseguito', 'non eseguita', 'manuale', 'fallito', 'fallita',
    'attenzione', 'nota:', 'della', 'per il', 'che', 'sono', 'questo', 'questa', 'dopo che', 'non è')
$italianRe = '(?i)(?<![\p{L}\p{N}_])(' + (($italian | ForEach-Object { [regex]::Escape($_) }) -join '|') + ')(?![\p{L}\p{N}_])'
$vocab = @('verified', 'failed', 'not run', 'manual')
$statusColumns = @('outcome', 'result', 'verification', 'esito')

foreach ($f in $files) {
    $rel = $f.Substring($Root.Length).TrimStart('\', '/').Replace('\', '/')
    $dir = Split-Path -Parent $f
    $lines = [IO.File]::ReadAllLines($f)
    $inFence = $false
    $statusCol = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        $n = $i + 1
        if ($line -match '^\s*```') { $inFence = -not $inFence; continue }

        if ($line -match $italianRe) { $problems.Add("${rel}:${n} italian marker '$($Matches[1])'") }
        if ($line -match '02\.08\.02\.61') { $problems.Add("${rel}:${n} stale Studio version 02.08.02.61") }
        foreach ($m in [regex]::Matches($line, '[0-9a-fA-F]{64}')) { $problems.Add("${rel}:${n} token-like string (64 hex)") }
        foreach ($m in [regex]::Matches($line, '[A-Za-z0-9_-]{32,}')) {
            if ($m.Value -match '[0-9]' -and $m.Value -match '[A-Za-z]' -and $m.Value -notmatch '^[0-9a-fA-F]{64}$') {
                $problems.Add("${rel}:${n} token-like string (base64url, 32+ chars)")
            }
        }

        if (-not $inFence) {
            foreach ($m in [regex]::Matches($line, '\]\(([^)\s]+)\)')) {
                $t = $m.Groups[1].Value
                if ($t -match '^(https?:|mailto:|#)') { continue }
                $t = ($t -split '#')[0]
                if (-not $t) { continue }
                $p = Join-Path $dir ($t.Replace('/', [IO.Path]::DirectorySeparatorChar))
                if (-not (Test-Path -LiteralPath $p)) { $problems.Add("${rel}:${n} broken link: $($m.Groups[1].Value)") }
            }

            # status tables: the column named Outcome / Result / Verification holds only the vocabulary
            if ($line -match '^\s*\|') {
                $cells = @(($line.Trim().Trim('|')) -split '\|' | ForEach-Object { $_.Trim() })
                $prevIsTable = ($i -gt 0 -and $lines[$i - 1] -match '^\s*\|')
                if (-not $prevIsTable) {
                    $statusCol = -1
                    for ($c = 0; $c -lt $cells.Count; $c++) {
                        if ($statusColumns -contains $cells[$c].ToLower()) { $statusCol = $c }
                    }
                } elseif ($statusCol -ge 0 -and $line -notmatch '^\s*\|[\s:|-]+\|?\s*$' -and $statusCol -lt $cells.Count) {
                    $v = $cells[$statusCol].Trim('`').Trim()
                    if ($v -ne '-' -and $vocab -notcontains $v) { $problems.Add("${rel}:${n} status '$v' outside verified | failed | not run | manual") }
                }
            } else { $statusCol = -1 }

            if ($line -match '^\s*(Outcome|Result)\s*:\s*`?([^`\s][^`]*?)`?\s*$') {
                $v = $Matches[2].Trim().TrimEnd('.')
                if ($vocab -notcontains $v) { $problems.Add("${rel}:${n} status '$v' outside verified | failed | not run | manual") }
            }
        }
    }
}

if ($problems.Count -gt 0) {
    $problems | ForEach-Object { Write-Host "FAIL $_" }
    exit 1
}
Write-Host "OK check-docs ($($files.Count) files)"
exit 0

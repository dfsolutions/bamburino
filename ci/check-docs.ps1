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
    'attenzione', 'nota:', 'della', 'per il', 'che', 'sono', 'questo', 'questa', 'dopo che', ('non ' + [char]0x00E8))
$italianRe = '(?i)(?<![\p{L}\p{N}_])(' + (($italian | ForEach-Object { [regex]::Escape($_) }) -join '|') + ')(?![\p{L}\p{N}_])'
$vocab = @('verified', 'failed', 'not run', 'manual')
$statusColumns = @('outcome', 'result', 'verification', 'esito')
# the status vocabulary applies only here (the build log's Outcome column is free text by design)
$vocabFileRe = '^(README\.md|docs/verification\.md|docs/acceptance-[^/]*\.md)$'

foreach ($f in $files) {
    $rel = $f.Substring($Root.Length).TrimStart('\', '/').Replace('\', '/')
    $dir = Split-Path -Parent $f
    $checkVocab = $rel -match $vocabFileRe
    $lines = [IO.File]::ReadAllLines($f)
    $inFence = $false
    $statusCol = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        $n = $i + 1
        if ($line -match '^\s*```') { $inFence = -not $inFence; continue }

        if ($line -match $italianRe) { $problems.Add("${rel}:${n} italian marker '$($Matches[1])'") }
        if ($line -match '02\.08\.02\.61') { $problems.Add("${rel}:${n} stale Studio version 02.08.02.61") }
        # 64 hex is a checksum when the line (or the one before) says sha256 / SHA-256; otherwise it looks like a secret
        if ($line -notmatch '(?i)sha-?256' -and -not ($i -gt 0 -and $lines[$i - 1] -match '(?i)sha-?256')) {
            foreach ($m in [regex]::Matches($line, '(?<![0-9A-Za-z])[0-9a-fA-F]{64}(?![0-9A-Za-z])')) { $problems.Add("${rel}:${n} token-like string (64 hex)") }
        }
        foreach ($m in [regex]::Matches($line, '[A-Za-z0-9_-]{32,}')) {
            $v = $m.Value
            if ($v -notmatch '[0-9]' -or $v -notmatch '[A-Za-z]') { continue }
            if ($v -match '^[0-9a-f]{40}$' -or $v -match '^[0-9a-fA-F]{64}$') { continue }   # git SHA / checksum (64 hex handled above)
            # file-name-like (patch names): every -/_ separated segment is lowercase letters (optionally followed by digits) or digits only
            if ($v -match '[-_]' -and (@($v -split '[-_]') | Where-Object { $_ -notmatch '^([a-z]+[0-9]*|[0-9]+)$' }).Count -eq 0) { continue }
            $problems.Add("${rel}:${n} token-like string (base64url, 32+ chars)")
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
            if ($checkVocab -and $line -match '^\s*\|') {
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

            if ($checkVocab -and $line -match '^\s*(Outcome|Result)\s*:\s*`?([^`\s][^`]*?)`?\s*$') {
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

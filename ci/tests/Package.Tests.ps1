# SPDX-License-Identifier: AGPL-3.0-or-later
# Tests for ci/package-service.ps1, ci/service-install.ps1 and ci/install-studio.ps1.
# Run: pwsh ci/tests/Package.Tests.ps1
# The service exe is a stub unless BAMBURINO_TEST_BUILD=1 (then `cargo build --release -p bamburino` in service/);
# the Studio zip is synthesised (bambu-studio.exe stub), the real one comes from driver/build.ps1.
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$failures = New-Object System.Collections.Generic.List[string]
function Assert-True($cond, $msg) {
    if ($cond) { Write-Host "  ok   $msg" } else { Write-Host "  FAIL $msg"; $failures.Add($msg) }
}
function New-TempDir {
    $d = Join-Path ([IO.Path]::GetTempPath()) ("pk-" + [guid]::NewGuid())
    New-Item -ItemType Directory $d | Out-Null
    $d
}
Add-Type -AssemblyName System.IO.Compression.FileSystem
function Get-ZipNames([string]$zip) {
    $z = [IO.Compression.ZipFile]::OpenRead($zip)
    try { @($z.Entries | ForEach-Object { $_.FullName } | Sort-Object) } finally { $z.Dispose() }
}

$tmp = New-TempDir
try {
    # ---- service package ----
    $exe = Join-Path $tmp 'bamburino.exe'
    if ($env:BAMBURINO_TEST_BUILD -eq '1') {
        Push-Location (Join-Path $root 'service')
        try { cargo build --release -p bamburino; if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' } } finally { Pop-Location }
        Copy-Item (Join-Path $root 'service/target/release/bamburino.exe') $exe
    } else {
        Set-Content -LiteralPath $exe -Value 'stub exe' -Encoding ascii
    }
    $out = Join-Path $tmp 'out'
    & pwsh -NoProfile -File (Join-Path $root 'ci/package-service.ps1') -Exe $exe -Out $out -Version '9.9.9' | Out-Null
    $code = $LASTEXITCODE
    Assert-True ($code -eq 0) 'package-service.ps1 exits 0'
    $zip = Join-Path $out 'bamburino-9.9.9-win64.zip'
    Assert-True (Test-Path $zip) 'zip bamburino-<ver>-win64.zip exists'
    if (Test-Path $zip) {
        $names = Get-ZipNames $zip
        $want = @('LICENSE', 'NOTICE.md', 'README.md', 'bamburino.exe', 'install.ps1') | Sort-Object
        Assert-True (($names -join '|') -ceq ($want -join '|')) "zip holds exactly: $($want -join ', ') (got: $($names -join ', '))"
        $sha = "$zip.sha256"
        Assert-True (Test-Path $sha) '.sha256 exists'
        if (Test-Path $sha) {
            $line = (Get-Content $sha -Raw).Trim()
            $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
            Assert-True ($line -ceq "$actual  bamburino-9.9.9-win64.zip") '.sha256 matches the zip (sha256sum format)'
        }
        # install.ps1 inside the zip is ci/service-install.ps1
        $x = Join-Path $tmp 'x'; [IO.Compression.ZipFile]::ExtractToDirectory($zip, $x)
        Assert-True ((Get-FileHash (Join-Path $x 'install.ps1')).Hash -eq (Get-FileHash (Join-Path $root 'ci/service-install.ps1')).Hash) 'install.ps1 == ci/service-install.ps1'
    }
    $r = & pwsh -NoProfile -File (Join-Path $root 'ci/package-service.ps1') -Exe (Join-Path $tmp 'nope.exe') -Out $out -Version '9.9.9' 2>&1
    Assert-True ($LASTEXITCODE -ne 0) 'missing exe -> non-zero exit'

    # ---- install-studio.ps1 ----
    $studio = Join-Path $tmp 'studio'; New-Item -ItemType Directory $studio | Out-Null
    Set-Content (Join-Path $studio 'bambu-studio.exe') 'stub' -Encoding ascii
    Set-Content (Join-Path $studio 'install-studio.ps1') 'stub' -Encoding ascii
    New-Item -ItemType Directory (Join-Path $studio 'resources') | Out-Null
    Set-Content (Join-Path $studio 'resources/x.txt') 'x' -Encoding ascii
    $szip = Join-Path $tmp 'BambuStudio-test.zip'
    [IO.Compression.ZipFile]::CreateFromDirectory($studio, $szip)
    $la = Join-Path $tmp 'localappdata'; $ad = Join-Path $tmp 'appdata'
    New-Item -ItemType Directory $la, $ad | Out-Null
    # a pre-existing user config that must survive install and uninstall
    $cfg = Join-Path $ad 'BambuStudio'; New-Item -ItemType Directory $cfg | Out-Null
    Set-Content (Join-Path $cfg 'keep.json') '{}' -Encoding ascii
    $script = Join-Path $root 'ci/install-studio.ps1'
    $inst = Join-Path $la 'Programs/BambuStudio-Bamburino'
    $lnk = Join-Path $ad 'Microsoft/Windows/Start Menu/Programs/Bambu Studio (Bamburino).lnk'
    $env:LOCALAPPDATA_BAK = $env:LOCALAPPDATA; $env:APPDATA_BAK = $env:APPDATA
    try {
        $env:LOCALAPPDATA = $la; $env:APPDATA = $ad
        $o = & pwsh -NoProfile -File $script -Zip $szip 2>&1 | Out-String
        Assert-True ($LASTEXITCODE -eq 0) 'install-studio.ps1 -Zip exits 0'
        Assert-True (Test-Path (Join-Path $inst 'bambu-studio.exe')) 'bambu-studio.exe installed under LOCALAPPDATA\Programs\BambuStudio-Bamburino'
        Assert-True (Test-Path (Join-Path $inst 'resources/x.txt')) 'subfolders extracted'
        Assert-True (Test-Path $lnk) 'Start Menu shortcut created'
        Assert-True ($o -match 'SmartScreen') 'warns about SmartScreen (unsigned)'
        Assert-True ($o -match '(?i)official') 'warns not to run the official instance together'
        # re-install over an existing install works (upgrade)
        & pwsh -NoProfile -File $script -Zip $szip 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -eq 0 -and (Test-Path (Join-Path $inst 'bambu-studio.exe'))) 'second install (upgrade) works'
        # sha256 next to the zip is verified when present
        Set-Content "$szip.sha256" ("0" * 64 + "  x.zip") -Encoding ascii
        & pwsh -NoProfile -File $script -Zip $szip 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -ne 0) 'wrong .sha256 next to -Zip -> refuses'
        $good = (Get-FileHash $szip -Algorithm SHA256).Hash.ToLower()
        Set-Content "$szip.sha256" "$good  BambuStudio-test.zip" -Encoding ascii
        & pwsh -NoProfile -File $script -Zip $szip 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -eq 0) 'correct .sha256 next to -Zip -> installs'
        # running the installer from the install folder itself is refused
        Copy-Item $script $inst
        & pwsh -NoProfile -File (Join-Path $inst 'install-studio.ps1') 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -ne 0) 'source folder == install folder -> refuses'
        & pwsh -NoProfile -File $script -Uninstall 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -eq 0) '-Uninstall exits 0'
        Assert-True (-not (Test-Path $inst)) 'install folder removed'
        Assert-True (-not (Test-Path $lnk)) 'shortcut removed'
        Assert-True (Test-Path (Join-Path $cfg 'keep.json')) '%APPDATA%\BambuStudio untouched'
        & pwsh -NoProfile -File $script -Uninstall 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -eq 0) '-Uninstall twice is idempotent'
        & pwsh -NoProfile -File $script -Zip (Join-Path $tmp 'missing.zip') 2>&1 | Out-Null
        Assert-True ($LASTEXITCODE -ne 0) 'missing zip -> non-zero exit'
    } finally {
        $env:LOCALAPPDATA = $env:LOCALAPPDATA_BAK; $env:APPDATA = $env:APPDATA_BAK
        Remove-Item Env:LOCALAPPDATA_BAK, Env:APPDATA_BAK -ErrorAction SilentlyContinue
    }
} finally {
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
if ($failures.Count -gt 0) { Write-Host "`n$($failures.Count) assertion(s) failed"; exit 1 }
Write-Host "`nAll assertions passed"

$failed = $false
foreach ($t in Get-ChildItem -Path $PSScriptRoot -Filter '*.Tests.ps1' | Sort-Object Name) {
    try {
        $global:LASTEXITCODE = 0
        & $t.FullName | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "exit code $LASTEXITCODE" }
        Write-Host "PASS $($t.Name)"
    } catch {
        Write-Host "FAIL $($t.Name): $($_.Exception.Message)"
        $failed = $true
    }
}
if ($failed) { exit 1 }
exit 0

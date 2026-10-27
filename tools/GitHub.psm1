function Set-GhToken {
    $env:GH_TOKEN = (("protocol=https`nhost=github.com`n`n" | git credential fill | Select-String '^password=').Line -replace '^password=','')
    if (-not $env:GH_TOKEN) { throw 'GitHub credential not available' }
}
Export-ModuleMember -Function Set-GhToken

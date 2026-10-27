$p = "$PSScriptRoot/../../driver/patches/0001-pr12485-native-mcp-server.diff"
if (-not (Test-Path $p)) { throw 'patch missing' }
$files = (Select-String -Path $p -Pattern '^diff --git a/(\S+)').Matches | ForEach-Object { $_.Groups[1].Value }
if ($files.Count -ne 41) { throw "expected 41 files, got $($files.Count)" }
if ($files -notcontains 'src/slic3r/Utils/McpServer.cpp') { throw 'McpServer.cpp missing' }
$id = & "$PSScriptRoot/../../driver/patch-id.ps1"
if ($id -notmatch '^[0-9a-f]{12}$') { throw "bad patch id $id" }
Import-Module "$PSScriptRoot/../Upstream.psm1" -Force
if ((Get-Upstream)['driver.patch_id'] -ne $id) { throw 'upstream.toml patch_id not updated' }

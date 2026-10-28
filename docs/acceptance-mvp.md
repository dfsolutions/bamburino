# MVP acceptance

Outcome vocabulary: `verified | failed | not run | manual`. An item is `verified` only when the command was executed
and its result checked; the evidence column says where.

## Driver (plan A)

### Task 8 - Windows app build, MCP standalone tests, portable package

| Item | Outcome | Date | Evidence |
|---|---|---|---|
| App build (`driver/build.ps1 -Step app`), patch set up to 0210 (patch_id `8bd194447b99`) | verified | 2026-10-28 | Superseded by the final build below. Round 3, `D:\bb\logs\app-20261028-081137.log`, cmake build exit 0. `install-dir\bambu-studio.exe`: fixed version 2.8.4.57, `FileVersion` `02.08.04.57`. `BambuStudio.dll` embeds `bamburino.8bd194447b99` |
| App build with the final patch set (patch_id `2703638ef313`) | verified | 2026-10-28 | Step 5 `-Step all`, `D:\bb\logs\app-20261028-085238.log`, 22.3 min, exit 0. `bambu-studio.exe` 2.8.4.57, `BambuStudio.dll` embeds `bamburino.2703638ef313` (`App.Tests.ps1`) |
| MCP standalone tests (`-Step test`: ctest `mcp_server` + `test_legacy_contract.py`) | verified | 2026-10-28 | `D:\bb\logs\test-20261028-091459.log` (final patch set): `100% tests passed, 0 tests failed out of 1`; legacy contract 3 tests OK |
| Portable package (`-Step package`) | verified | 2026-10-28 | `D:\bb\artifacts\BambuStudio-bamburino-02.08.04.57-2703638ef313-win64.zip`, 260.6 MB, sha256 `1b101b80c5acc7f7fd0379ca07d9a04e1993bfea7dce785a16b174cd28414438`; `.sha256` matches `Get-FileHash` (`App.Tests.ps1`) |
| Identity on the CMake configure line (`Identity.Tests.ps1` app-log check) | verified | 2026-10-28 | `app-20261028-085238.log` (final build) carries both `-DBAMBURINO_*` defines |
| Reproducibility (`-Step all` on a clean `build`, deps reused) | verified | 2026-10-28 | 23.2 min, exit 0 (second attempt; the first stopped after 3.6 min with `cl D8040`, a process-spawn failure, no code change in between). See `docs/driver-build-log.md` |
| `ProductVersion` string of `bambu-studio.exe` starts with 2.8.4 / 02.08.04 | not run | - | Accepted deviation: upstream's rc template uses `@SLIC3R_BUILD_ID@`, which no public upstream CMake file defines, so the string is empty in any source build. `App.Tests.ps1` checks the fixed version and `FileVersion` instead |

Notes for Task 9:

- On the build PC, the TCP ranges 23374-27520 are reserved by Hyper-V/WinNAT (`netsh int ipv4 show excludedportrange
  protocol=tcp`), so the driver's default port 27183 cannot bind there. The driver port is configurable
  (`mcp_server_port`, Preferences > Local MCP server): use 27601 for the driver on this machine.

Deferred items: none for Task 8.

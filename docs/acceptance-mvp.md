# MVP acceptance

Outcome vocabulary: `verified | failed | not run | manual`. An item is `verified` only when the command was executed
and its result checked; the evidence column says where.

## Driver (plan A)

### Task 8 - Windows app build, MCP standalone tests, portable package

| Item | Outcome | Date | Evidence |
|---|---|---|---|
| App build (`driver/build.ps1 -Step app`), patch set up to 0210 (patch_id `8bd194447b99`) | verified | 2026-10-28 | Superseded by the final build below. Round 3, `D:\BambuBuild\logs\app-20261028-081137.log`, cmake build exit 0. `install-dir\bambu-studio.exe`: fixed version 2.8.4.57, `FileVersion` `02.08.04.57`. `BambuStudio.dll` embeds `bamburino.8bd194447b99` |
| App build with the final patch set (patch_id `2703638ef313`) | verified | 2026-10-28 | Step 5 `-Step all`, `D:\BambuBuild\logs\app-20261028-085238.log`, 22.3 min, exit 0. `bambu-studio.exe` 2.8.4.57, `BambuStudio.dll` embeds `bamburino.2703638ef313` (`App.Tests.ps1`) |
| MCP standalone tests (`-Step test`: ctest `mcp_server` + `test_legacy_contract.py`) | verified | 2026-10-28 | `D:\BambuBuild\logs\test-20261028-091459.log` (final patch set): `100% tests passed, 0 tests failed out of 1`; legacy contract 3 tests OK |
| Portable package (`-Step package`) | verified | 2026-10-28 | `D:\BambuBuild\artifacts\BambuStudio-bamburino-02.08.04.57-2703638ef313-win64.zip`, 260.6 MB, sha256 `1b101b80c5acc7f7fd0379ca07d9a04e1993bfea7dce785a16b174cd28414438`; `.sha256` matches `Get-FileHash` (`App.Tests.ps1`) |
| Identity on the CMake configure line (`Identity.Tests.ps1` app-log check) | verified | 2026-10-28 | `app-20261028-085238.log` (final build) carries both `-DBAMBURINO_*` defines |
| Reproducibility (`-Step all` on a clean `build`, deps reused) | verified | 2026-10-28 | 23.2 min, exit 0 (second attempt; the first stopped after 3.6 min with `cl D8040`, a process-spawn failure, no code change in between). See `docs/driver-build-log.md` |
| App version identity (controller ruling's criterion: `FileVersion` `02.08.04.*` of `bambu-studio.exe` and `BambuStudio.dll` marker `bamburino.<patch_id>`) | verified | 2026-10-28 | `App.Tests.ps1` on the final build: `FileVersion` `02.08.04.57`, fixed version 2.8.4.57, `BambuStudio.dll` embeds `bamburino.2703638ef313`. The `ProductVersion` *string* is not part of the criterion: upstream's rc template fills it from `@SLIC3R_BUILD_ID@`, which no public upstream CMake file defines, so it is empty in any source build |
| `-Step package` identity guard (exit 50 when `BambuStudio.dll` lacks `bamburino.<patch_id>` of `upstream.toml`) | verified | 2026-10-28 | `tools/tests/Package.Tests.ps1` on a fake build root (`BAMBURINO_BUILD_ROOT`, `BAMBURINO_UPSTREAM_TOML` copy): mismatched `patch_id` exit 50 and no artifact; matching `patch_id` exit 0. Not run on the real `D:\BambuBuild\install-dir` (no re-package) |
| `SOURCE.txt`, `LICENSE`, `NOTICE.md` at the zip root (AGPL-3.0 section 6) | verified | 2026-10-28 | Unit scope only, `tools/tests/Package.Tests.ps1`: content of `SOURCE.txt` (repo URL, commit, tag, tag_commit, patch_id, patch list, upstream URL + tag) and the copy list, on a temp dir and in a zip built by `-Step package` from a fake install-dir |
| The same files inside `D:\BambuBuild\artifacts\BambuStudio-bamburino-02.08.04.57-2703638ef313-win64.zip` | not run | - | That zip predates the change (built 2026-10-28 09:15) and does not contain them; the next real `-Step package` must be run and checked |
| `install-studio.ps1` at the zip root | not run | - | `ci/install-studio.ps1` is not on the `driver` branch; it is added at integration with the `ci` branch |

Notes for Task 9:

- On the build PC, the TCP ranges 23374-27520 are reserved by Hyper-V/WinNAT (`netsh int ipv4 show excludedportrange
  protocol=tcp`), so the driver's default port 27183 cannot bind there. The driver port is configurable
  (`mcp_server_port`, Preferences > Local MCP server): use 27601 for the driver on this machine.

Deferred items: none for Task 8.

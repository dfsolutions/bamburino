# Driver build log

## 2026-10-27 - Prerequisites (Task 2)

Detected by `pwsh driver/prereqs.ps1 -Check` on the build machine (exit 0):

| Prerequisite | Detected |
|---|---|
| Visual Studio | Build Tools 2022 17.13.35818.85 at `C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools` |
| MSVC toolset / cl | 14.43.34808 / cl 19.43.34808 (x64) |
| CMake | 3.30.5, VS-bundled, at `<VS>\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe` (`BAMBURINO_CMAKE`) |
| Windows SDK | 10.0.26100.0 (`BAMBURINO_WIN10SDK_PATH=C:/Program Files (x86)/Windows Kits/10/Include/10.0.26100.0`) |
| Strawberry Perl | 5.42.3.1 at `C:\Strawberry\perl\bin\perl.exe` |
| pkg-config | pkg-config-lite 0.28 at `%LOCALAPPDATA%\Microsoft\WinGet\Links\pkg-config.exe` |
| git | `core.longpaths=true` (global) |
| Free space | 32.9 GB on `D:\` for build root `D:\bb` (minimum 25 GB) |

Installed by `pwsh driver/prereqs.ps1 -Install`: Strawberry Perl 5.42.3.1 (winget `StrawberryPerl.StrawberryPerl`),
pkg-config-lite 0.28-1 (winget `bloodrock.pkg-config-lite`), `git config --global core.longpaths true`.

Toolchain determinism: CMake is resolved from the VS 2022 instance first, PATH only as fallback, and any cmake
under `C:\Strawberry\` is rejected (Strawberry Perl ships a MinGW-built CMake 3.29.2 in `C:\Strawberry\c\bin`,
which it adds to the Machine PATH). `prereqs.ps1 -Export` puts the VS CMake bin directory first on the session
PATH, followed by Strawberry `perl\bin` and pkg-config, and removes `C:\Strawberry\c\bin` from the session PATH so
MinGW headers, libraries and tools cannot leak into `find_package` during the upstream build. The Machine PATH is
not modified.

## 2026-10-27 - Dependencies (Task 6)

`pwsh driver/build.ps1 -Step deps` on the patched clone (`D:\bb\BambuStudio`, `v02.08.04.57`), deps tree
`8e648ef0f91f8d5ef2b0844e1e252be91ce81ac9` (`git rev-parse <tag_commit>:deps`).

| Round | Start | End | Duration | Outcome | First error | Fix |
|---|---|---|---|---|---|---|
| 1 | 23:11:06 | 23:33:58 | 22.9 min (cmake configure + msbuild 22.8 min) | OK, exit 0 | none (0 `error XXnnnn:` lines in the log) | none |

Log: `D:\bb\logs\deps-20261027-231109.log` (~51k lines). Rounds used: 1 of 3.

Result: `D:\bb\deps-out\usr\local` with Boost 1.84, OpenSSL, wxWidgets 3.1 (`lib\vc_x64_lib\wxmsw31u_core.lib`),
CGAL, OpenVDB, OCCT, TBB, FFmpeg, OpenCV, assimp, curl, GMP/MPFR and the rest of `deps/CMakeLists.txt`.
Sentinel `D:\bb\deps-out\.bamburino-deps-key` = the deps tree SHA-1 above; a second `-Step deps` is a no-op in 0.5 s.

Notes:
- Boost's own CMake install uses the versioned layout on Windows: headers are in
  `include\boost-1_84\boost`, not `include\boost`. The app step finds Boost through its CMake package config
  (`lib\cmake\Boost-1.84.0`, via `CMAKE_PREFIX_PATH=<deps-out>\usr\local`), not through `include\boost`.
  `tools/tests/Deps.Tests.ps1` accepts both header layouts and asserts that `lib\cmake\Boost-*` exists.
- Round 1 ran with Git for Windows `mingw64\bin`, `usr\bin` and `usr\local\bin` on the session PATH (inherited from
  the launching shell, after the VS, CMake and Strawberry perl entries); it did not affect the deps build. Since fix
  round 1, `prereqs.ps1 -Export` also removes those three directories (keeping `Git\cmd`), so MinGW zlib/curl and
  MSYS `link.exe` cannot shadow MSVC during `find_package` in the app build.
- Disk: free space on D: went from 29.6 GB before the build to 19.3 GB after. Measured: `deps-out` 0.9 GB,
  `deps\build` 4.8 GB (sources, downloads, intermediates); the remaining ~4.5 GB of the drop is not accounted for
  under these two folders (other activity on D: or files skipped by the measurement).
- Free-disk thresholds (ruling, fix round 1): 25 GB is required only before the FIRST deps build (the peak).
  `prereqs.ps1 -MinFreeGB` (default 25) sets the threshold; `build.ps1` picks it with `Get-StepMinFreeGB`:
  `deps` 25 when `<clone>\deps\build` does not exist, 12 when it does (resume or no-op), `app` 12.
  `tools/tests/Prereqs.Tests.ps1` tests the check with explicit thresholds (12 holds, 10000 fails) instead of the
  machine's current free space.

## 2026-10-28 - App build, MCP standalone tests, package (Task 8)

`pwsh driver/build.ps1 -Step app` on the patched clone (`D:\bb\BambuStudio`, `v02.08.04.57`), VS 2022 Build Tools
17.13 / MSVC 14.43, CMake 3.30.5 (VS), deps from Task 6 (no-op). Rounds count from the first `-Step app`.

| Round | Start | End | Duration | Outcome | First error | Fix |
|---|---|---|---|---|---|---|
| 1 | 00:11:52 | 00:11:59 | 0.1 min | failed (cmake configure, exit 40) | `Could NOT find PkgConfig`: CMake's `find_program` picked Strawberry's Perl script `C:/Strawberry/perl/bin/pkg-config`, which is not an executable | build scripts only, no clone patch: `prereqs.ps1 -Export` sets `PKG_CONFIG_EXECUTABLE` (pkg-config-lite) and puts its directory before Strawberry `perl\bin`; `-Step app` passes `-DPKG_CONFIG_EXECUTABLE` explicitly (overrides a stale cache) |
| 2 | 00:13:06 | 00:29 | 15.8 min | failed (msbuild, exit 40), 3 errors, all in `libslic3r_gui` (PR #12485 code, never built on Windows) | `src/slic3r/GUI/McpAdvanced.inc(686,23): error C2664` `TriangleMesh::write_binary(const char*)` given `std::filesystem::path::c_str()` (`const wchar_t*` on Windows); same at 687 (`WriteOBJFile`); `McpToolsManifest.inc(1,1): error C2026` string literal too large | `0200-windows-msvc-string-literal-limit.diff`, `0210-windows-utf8-mesh-export-path.diff` |
| 3 | 08:11:37 | 08:31:26 | 19.8 min (configure + incremental build + install) | OK, exit 0; 0 `error` lines | none | none |

Round 2 overlapped a session interruption (rate limit, about 00:13-00:30) but was not cut by it: the build ended on its
own with exit 1 at 15.8 min, so it is a genuine failed round. During round 2's configure, an `Identity.Tests.ps1`
run executed `-Step patch` on the clone (00:13:48): file contents were restored identically, only timestamps changed;
the three errors are deterministic code errors unrelated to it.

Result of round 3: `D:\bb\install-dir\bambu-studio.exe`, fixed product version 2.8.4.57, `FileVersion` `02.08.04.57`;
the `ProductVersion` string is empty because upstream's `BambuStudio.rc.in` uses `@SLIC3R_BUILD_ID@`, which no
public upstream CMake file defines. `BambuStudio.dll` embeds `bamburino.8bd194447b99` (patch_id after 0200/0210).

`-Step test` (08:35:23, log `D:\bb\logs\test-20261028-083523.log`): configure OK, build failed (exit 45):
`boost-1_84\boost\asio\detail\socket_types.hpp(24,1): error C1189: WinSock.h has already been included`, compiling
`tests/mcp/test_mcp_server.cpp`, which includes `slic3r/GUI/McpGcodeExport.hpp` (`#include <windows.h>` at line 9,
without `WIN32_LEAN_AND_MEAN`) before `<boost/asio.hpp>`. Not fixed: the round budget is spent (see Task 8 report).

`-Step package`: `D:\bb\artifacts\BambuStudio-bamburino-02.08.04.57-8bd194447b99-win64.zip` (259.5 MB) and `.sha256`.

Disk: D: had 17.3 GB free before round 1. `D:\bb\BambuStudio\deps\build` (4.8 GB) was deleted before round 3
(deps stay a no-op: the sentinel lives in `deps-out`). The app `build` folder is 8.5 GB (obj 2.3, lib 2.3, pdb 2.2,
pch 1.3), install-dir 0.55 GB. Free space after round 3 and packaging: 2.2 GB, also because another build on D:
(`D:\GoogleAI\bamburino-service\service`, about 9.3 GB written in the same hour) consumed space. The reproducibility
run (Step 5) was not run: it needs at least 12 GB free.

### Test-program round (extra, controller ruling; not an app round)

Scope: `tests/mcp` only (the PR's standalone `mcp_server_tests` and `test_legacy_contract.py`). Reason: the
`-Step test` compile error above (C1189). Before it, `D:\bb\BambuStudio\build` (8.5 GB) was deleted: D: free went from
1.3 GB to 10.1 GB. The app stays installed in `D:\bb\install-dir`, which `-Step patch` does not touch because it is
outside the clone. The app was not rebuilt.

One round, several `-Step patch` + `-Step test` passes (by ruling, the link-setup fixes of the test program belong to
the same round):

| Pass | Time | Outcome | First error | Fix |
|---|---|---|---|---|
| a | 08:41:51-08:42:15 | failed (exit 45), compile OK, link failed | `LNK1104 libboost_locale-vc143-mt-x64-1_84.lib`: Boost auto-linking asks for `vc143` names, deps-out holds `libboost_*-vc144-*.lib`. The app defines `BOOST_ALL_NO_LIB` globally (top-level `CMakeLists.txt`); `tests/mcp` did not (pass a ran with 0220 = `WIN32_LEAN_AND_MEAN`, which fixed the C1189) | 0220 amended: `BOOST_ALL_NO_LIB` |
| b | 08:44:28-08:45:22 | failed (exit 45), link | `libcrypto.lib(e_capi.obj): LNK2019 __imp_CertOpenStore` and 6 more `Cert*` symbols: the static OpenSSL needs `crypt32` | 0220 amended: `crypt32` |
| c | 08:45:35-08:46:08 | ctest 1/1 passed; `test_legacy_contract.py` failed | `ModuleNotFoundError: No module named 'tests.mcp'`: `python -m unittest <path>` imports the file as a package | `build.ps1` runs the file as a script |
| d | 08:46:19 | ctest passed; `test_legacy_contract.py` failed | `setUpClass` `assert sha256(oracle) == ORACLE_SHA256`: the clone work tree is CRLF (`core.autocrlf=true`), and the test pins the hash of the committed LF bytes | `build.ps1` runs the Python test on an LF export of the clone's index (`git -c core.autocrlf=false checkout-index --prefix=<tmp>/`), no clone change |
| e | 08:47:00 | **OK** (exit 0) | none | none |

Final pass log: `D:\bb\logs\test-20261028-084700.log`. Results:
- ctest `100% tests passed, 0 tests failed out of 1` (`mcp_server`, 2.22 s);
- `test_legacy_contract.py`: 3 tests, OK.

`0220-windows-mcp-tests-lean-and-mean.diff` (`tests/mcp/CMakeLists.txt`, +4 lines): `WIN32_LEAN_AND_MEAN` and
`BOOST_ALL_NO_LIB` definitions and the `crypt32` link library for `mcp_server_tests`, on Windows only.

Final patch_id: `2703638ef313`. The installed app and the zip from round 3 carry `8bd194447b99`: 0220 changes only
`tests/mcp/CMakeLists.txt`. By ruling, the app is rebuilt with the final patch set in Step 5.

### Reproducibility and final app build (Step 5)

Disk: about 30 GB free on D: once the other plan's cargo target was moved to C:. `D:\bb\BambuStudio\build` was
absent (deleted earlier), so this started from a clean `build` folder.

| Attempt | Start | End | Duration | Outcome | First error |
|---|---|---|---|---|---|
| 1 | 08:48:00 | 08:51:36 | 3.6 min | failed (app, exit 40) | in `libslic3r`: `cl : command line error D8040: error creating or communicating with child process`, then `MSB6006 CL.exe exited with code 2`. This is a process-level failure (the `/MP` children could not be spawned), not a compile error. RAM: 8.1 GB free of 31.8 GB, 18.7 GB commit free while idle; the `-m` x `/MP` fan-out of a clean build may exhaust it |
| 2 | 08:52:25 | 09:15:37 | **23.2 min** | **OK, exit 0** | none |

No code change between the two attempts. Before attempt 2 I removed the partial `build` folder and stopped the
leftover MSBuild worker processes (15 idle `MSBuild.exe` nodes from earlier builds), so attempt 2 is again a clean
build.

Attempt 2 ran `prereqs (12 GB) -> fetch skipped (clone exists) -> patch (6 patches) -> deps no-op -> app (22.3 min)
-> test -> package`. Results:
- `bambu-studio.exe` version 2.8.4.57, `FileVersion` `02.08.04.57`; the final patch_id `2703638ef313`
  (0001+0100+0120+0200+0210+0220) is embedded as `bamburino.2703638ef313`;
- `D:\bb\logs\test-20261028-091459.log`: ctest `100% tests passed, 0 tests failed out of 1`, legacy contract 3
  tests OK;
- `D:\bb\artifacts\BambuStudio-bamburino-02.08.04.57-2703638ef313-win64.zip` (260.6 MB), sha256
  `1b101b80c5acc7f7fd0379ca07d9a04e1993bfea7dce785a16b174cd28414438`.

The superseded `8bd194447b99` zip was removed. `pwsh tools/tests/run.ps1`: all 9 PASS. Free on D: afterwards:
21 GB.

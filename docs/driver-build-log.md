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
  `include\boost-1_84\boost`, not `include\boost`. `tools/tests/Deps.Tests.ps1` accepts both. The app step must rely
  on the Boost CMake config under `lib\cmake` (via `CMAKE_PREFIX_PATH=<deps-out>\usr\local`), not on `include\boost`.
- The session PATH during the build still contains Git for Windows `usr\bin` and `mingw64\bin` (inherited from the
  launching shell), after the VS, CMake and Strawberry perl entries; it did not affect the build (Strawberry perl
  resolves first for OpenSSL).
- Disk: free space on D: went from 29.6 GB before the build to 19.3 GB after. Measured: `deps-out` 0.9 GB,
  `deps\build` 4.8 GB (sources, downloads, intermediates); the remaining ~4.5 GB of the drop is not under `D:\bb`
  deps output as measured (other activity on D: or files skipped by the measurement). `prereqs.ps1 -Check`
  (minimum 25 GB) now reports `MISSING disk-space`, so `tools/tests/Prereqs.Tests.ps1` fails until space is freed.

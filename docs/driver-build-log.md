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

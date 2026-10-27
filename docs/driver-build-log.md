# Driver build log

## 2026-10-27 - Prerequisites (Task 2)

Detected by `pwsh driver/prereqs.ps1 -Check` on the build machine (exit 0):

| Prerequisite | Detected |
|---|---|
| Visual Studio | Build Tools 2022 17.13.35818.85 at `C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools` |
| MSVC toolset / cl | 14.43.34808 / cl 19.43.34808 (x64) |
| CMake | 3.29.2 at `C:\Strawberry\c\bin\cmake.exe` (first on PATH, comes with Strawberry Perl); VS-bundled 3.30.5 also present |
| Windows SDK | 10.0.26100.0 (`BAMBURINO_WIN10SDK_PATH=C:/Program Files (x86)/Windows Kits/10/Include/10.0.26100.0`) |
| Strawberry Perl | 5.42.3.1 at `C:\Strawberry\perl\bin\perl.exe` |
| pkg-config | pkg-config-lite 0.28 at `%LOCALAPPDATA%\Microsoft\WinGet\Links\pkg-config.exe` |
| git | `core.longpaths=true` (global) |
| Free space | 32.9 GB on `D:\` for build root `D:\bb` (minimum 25 GB) |

Installed by `pwsh driver/prereqs.ps1 -Install`: Strawberry Perl 5.42.3.1 (winget `StrawberryPerl.StrawberryPerl`),
pkg-config-lite 0.28-1 (winget `bloodrock.pkg-config-lite`), `git config --global core.longpaths true`.

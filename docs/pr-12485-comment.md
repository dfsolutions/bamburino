DRAFT — not yet posted; owner approval required

Windows/MSVC build report for this PR (c31e971 on v02.08.04.57), thanks @otarkhan for the work.

**Setup.** PR head `c31e97108e4d1ea137f90a075502e4d4536535fe` applied on tag `v02.08.04.57` (`f977235e`), Windows 11 x64, VS 2022 Build Tools 17.13 (MSVC 14.43), CMake 3.30.5, Boost 1.84 and OpenSSL from the upstream deps, Strawberry Perl, i7-12700K / 32 GB. The patch applied cleanly; deps took 22.9 min, a clean app build about 22 min.

**Three small changes were needed to build on MSVC** (PR code had not been built on Windows):

1. `src/slic3r/GUI/McpToolsManifest.inc`: the tool manifest is one 56655-byte raw string literal; MSVC rejects literals over 16380 bytes (`C2026`). We split the same JSON into four adjacent raw literals of at most 16000 bytes, cut after a comma. `McpTools.cpp` and the manifest bytes are unchanged.
2. `src/slic3r/GUI/McpAdvanced.inc` (lines 686-687, mesh export): `std::filesystem::path::c_str()` is `const wchar_t*` on Windows, so `TriangleMesh::write_binary(const char*)` and `WriteOBJFile(const char*)` do not compile (`C2664`). We use `path::u8string().c_str()`; both writers open the file with `boost::nowide::fopen`, which expects UTF-8. Caveat: this only fixes the build. The rest of that handler still uses `source.string()` (ANSI code page on Windows), so non-ASCII paths remain unsupported there; and under C++20 `u8string()` returns `std::u8string` (`char8_t`), so the line would stop compiling. A portable form for both is `boost::nowide::narrow(source.wstring())` (UTF-8 `std::string` on Windows), used for every path handed to the nowide writers.
3. `tests/mcp/CMakeLists.txt` (`mcp_server_tests`, Windows only, +4 lines): define `WIN32_LEAN_AND_MEAN` (`McpGcodeExport.hpp` includes `<windows.h>` before `<boost/asio.hpp>`, giving `C1189` WinSock.h already included), define `BOOST_ALL_NO_LIB` (as the app build already does; otherwise auto-linking asks for `vc143` library names), and link `crypt32` (static OpenSSL needs it, `LNK2019 CertOpenStore`). The app build is not affected.

@otarkhan, if useful I can open a PR against your branch with these three changes; the diffs are in `driver/patches/` (`0200-*`, `0210-*`, `0220-*`) of the repo linked below.

**Suggestion, not a request.** For our own needs we added one line to `app.get_state`: `"undoSnapshot"`, the value of the existing `Plater::get_active_snapshot_time()` as a decimal string. It changes on every `take_snapshot`, undo and redo, so a client can tell the project changed (including a user Ctrl+Z) without hashing the whole state. Happy to send it if you think it fits.

**Results.**
- standalone `ctest` (`mcp_server`): 1/1 passed; `tests/mcp/test_legacy_contract.py`: 3/3 passed.
- app: builds and installs, `bambu-studio.exe` 2.8.4.57.
- Live verification against the running GUI (driving the MCP server end to end, including undo behaviour): not run yet on our side; I will report back once done. Nothing in this comment claims it.

We are building a desktop client on top of this server: https://github.com/dfsolutions/bamburino (public, AGPL-3.0).

Thanks again for building this.

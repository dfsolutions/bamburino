# Driver divergences

Every change to Bambu Studio `v02.08.04.57` beyond PR #12485 (`driver/patches/0001-pr12485-native-mcp-server.diff`)
is a separate `driver/patches/NNNN-*.diff`, applied in ordinal name order by `driver/build.ps1 -Step patch`,
with one line here stating why it exists.

| Patch | Files | Reason | Proposed upstream |
|---|---|---|---|
| `0100-build-identity.diff` | `CMakeLists.txt`, `src/slic3r/GUI/bamburino_identity.h.in` (new), `src/slic3r/GUI/McpTools.cpp`, `src/slic3r/GUI/AboutDialog.cpp` | Lets Bamburino tell its own driver build from any other: `app.get_capabilities` gains `buildSuffix` (`bamburino.<patch_id>`) and `patchId` next to the unchanged `appVersion` (`SLIC3R_VERSION`), and the About dialog shows the suffix. Values come from `-DBAMBURINO_PATCH_ID` / `-DBAMBURINO_BUILD_SUFFIX` passed by `driver/build.ps1` (`"unknown"` when not passed, never a stale cached value) into a generated header included only by those two files, so a new `patch_id` does not rebuild the tree or the precompiled header; `SLIC3R_VERSION` and `version.inc` are untouched so preset version parsing is unaffected. | no |
| `0120-expose-undo-snapshot.diff` | `src/slic3r/GUI/McpTools.cpp` | `app.get_state` gains `"undoSnapshot"`: the active undo snapshot time from the existing public `Plater::get_active_snapshot_time()`, as a decimal string. It changes on every `take_snapshot`, undo and redo, so a client can tell that the project changed (including a user Ctrl+Z) without hashing the whole state. One line, no new counter. | yes |
| `0200-windows-msvc-string-literal-limit.diff` | `src/slic3r/GUI/McpToolsManifest.inc` | MSVC rejects a single string literal longer than 16380 bytes (error C2026); PR #12485 embeds the 56655-byte tool manifest as one raw literal (it was only built on macOS). The same JSON is split into four adjacent raw literals of at most 16000 bytes each, cut after a comma; the compiler concatenates them, so `McpTools.cpp` and the manifest bytes are unchanged. | yes |
| `0210-windows-utf8-mesh-export-path.diff` | `src/slic3r/GUI/McpAdvanced.inc` | On Windows `std::filesystem::path::c_str()` is `const wchar_t*`, so passing it to `TriangleMesh::write_binary` / `WriteOBJFile` (`const char*`) does not compile (error C2664). Uses `path::u8string().c_str()`: both writers open the file with `boost::nowide::fopen`, which expects UTF-8. | yes |
| `0220-windows-mcp-tests-lean-and-mean.diff` | `tests/mcp/CMakeLists.txt` | Makes the PR's standalone test program build on Windows, where it was never built:

- `WIN32_LEAN_AND_MEAN`: the test includes `McpGcodeExport.hpp` (`<windows.h>`) before `<boost/asio.hpp>`, and windows.h would pull in winsock.h (error C1189).
- `BOOST_ALL_NO_LIB`: no Boost auto-linking by toolset-tagged name, as in the app build (LNK1104).
- `crypt32`: needed by the static OpenSSL libcrypto (LNK2019 `Cert*`).

Only `mcp_server_tests` on Windows; the app build is unaffected. | yes |

Not created: `0110-snapshot-settings-update.diff`, `0111-snapshot-object-transform.diff` and
`0112-snapshot-settings-override.diff`. The audit found `take_snapshot` already called before the mutation in
`settings.update`, `object.transform` and `settings.override` (`docs/driver-audit.md`, Findings 1 and
"Revision contract"), so no patch is needed to add the snapshots: they are present.
Whether undoing that snapshot actually restores a `settings.update` edit is OPEN (`docs/driver-audit.md`, Open
questions 1): the snapshot stores Model, Selection, Gizmos and PartPlateList, which may not include the edited
process preset config, so Ctrl+Z may not revert it. It is to be checked manually in Task 9; until then a client
must not rely on undo to roll back `settings.update`.

# Driver divergences

Every change to Bambu Studio `v02.08.04.57` beyond PR #12485 (`driver/patches/0001-pr12485-native-mcp-server.diff`)
is a separate `driver/patches/NNNN-*.diff`, applied in ordinal name order by `driver/build.ps1 -Step patch`,
with one line here stating why it exists.

| Patch | Files | Reason | Proposed upstream |
|---|---|---|---|
| `0100-build-identity.diff` | `CMakeLists.txt`, `src/slic3r/GUI/McpTools.cpp`, `src/slic3r/GUI/AboutDialog.cpp` | Lets Bamburino tell its own driver build from any other: `app.get_capabilities` gains `buildSuffix` (`bamburino.<patch_id>`) and `patchId` next to the unchanged `appVersion` (`SLIC3R_VERSION`), and the About dialog shows the suffix. Values come from `-DBAMBURINO_PATCH_ID` / `-DBAMBURINO_BUILD_SUFFIX` passed by `driver/build.ps1` (default `"unknown"`); `SLIC3R_VERSION` and `version.inc` are untouched so preset version parsing is unaffected. | no |

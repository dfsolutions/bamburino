# Driver divergences

Every change to Bambu Studio `v02.08.04.57` beyond PR #12485 (`driver/patches/0001-pr12485-native-mcp-server.diff`)
is a separate `driver/patches/NNNN-*.diff`, applied in ordinal name order by `driver/build.ps1 -Step patch`,
with one line here stating why it exists.

| Patch | Files | Reason | Proposed upstream |
|---|---|---|---|
| `0100-build-identity.diff` | `CMakeLists.txt`, `src/slic3r/GUI/bamburino_identity.h.in` (new), `src/slic3r/GUI/McpTools.cpp`, `src/slic3r/GUI/AboutDialog.cpp` | Lets Bamburino tell its own driver build from any other: `app.get_capabilities` gains `buildSuffix` (`bamburino.<patch_id>`) and `patchId` next to the unchanged `appVersion` (`SLIC3R_VERSION`), and the About dialog shows the suffix. Values come from `-DBAMBURINO_PATCH_ID` / `-DBAMBURINO_BUILD_SUFFIX` passed by `driver/build.ps1` (`"unknown"` when not passed, never a stale cached value) into a generated header included only by those two files, so a new `patch_id` does not rebuild the tree or the precompiled header; `SLIC3R_VERSION` and `version.inc` are untouched so preset version parsing is unaffected. | no |
| `0120-expose-undo-snapshot.diff` | `src/slic3r/GUI/McpTools.cpp` | `app.get_state` gains `"undoSnapshot"`: the active undo snapshot time from the existing public `Plater::get_active_snapshot_time()`, as a decimal string. It changes on every `take_snapshot`, undo and redo, so a client can tell that the project changed (including a user Ctrl+Z) without hashing the whole state. One line, no new counter. | yes |

Not created: `0110-snapshot-settings-update.diff`, `0111-snapshot-object-transform.diff` and
`0112-snapshot-settings-override.diff`. The audit found `take_snapshot` already called before the mutation in
`settings.update`, `object.transform` and `settings.override` (`docs/driver-audit.md`, Findings 1 and
"Revision contract"), so PR #12485 needs no change there.

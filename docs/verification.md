<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Verification status

Vocabulary: `verified | failed | not run | manual`. A row is `verified` only when the command or check was executed and its result recorded; the Source column says where. Written by hand from the first live CI runs of 2026-10-28 and from [acceptance-mvp.md](acceptance-mvp.md) and [acceptance-mvp-service.md](acceptance-mvp-service.md). Nobody has run the live demo against a real Bambu Studio yet, so no tool is `verified`.

## Functions

| Function | Status | Source | Date |
|---|---|---|---|
| `bamburino_capabilities` | not run | none | - |
| `studio_get_state` | not run | none | - |
| `studio_get_settings` | not run | none | - |
| `studio_set_settings` | not run | none | - |
| `studio_list_models` | not run | none | - |
| `studio_transform_model` | not run | none | - |
| `studio_set_supports` | not run | none | - |
| `studio_slice` | not run | none | - |
| `studio_slice_status` | not run | none | - |
| `studio_cancel_slice` | not run | none | - |
| `studio_get_slice_summary` | not run | none | - |
| `catalog_search` | not run | none | - |
| Install as Windows service | not run | none | - |
| Ctrl+Z reverts agent changes | manual | acceptance-mvp-service.md steps 4, 4b, 4c | - |
| Busy while modal dialog is open | manual | acceptance-mvp-service.md step 10 | - |
| Coexistence with official Bambu Studio | manual | none | - |

## Build and CI

| Check | Status | Source | Date |
|---|---|---|---|
| Patched Studio builds, MCP standalone tests pass, portable zip packaged on the self-hosted runner (`build-studio`, run 40659835599, attempt 2) | verified | automated, [run](https://github.com/dfsolutions/bamburino/actions/runs/40659835599) | 2026-10-28 |
| Studio zip: SHA-256 matches, root holds `bambu-studio.exe`, `SOURCE.txt`, `LICENSE`, `NOTICE.md`, `install-studio.ps1`; patch_id `2703638ef313` matches `driver/upstream.toml` | verified | automated, downloaded artifact `studio-win64` | 2026-10-28 |
| Service build, tests and package on the hosted runner (`build-service`, run 40659835498) | failed | automated, [run](https://github.com/dfsolutions/bamburino/actions/runs/40659835498): 4 tests in `crates/bamburino/tests/config.rs` fail on the hosted runner (the runner account is the built-in Administrator, whose SID prints as `LA` in SDDL) | 2026-10-28 |
| Release workflow dry run (`release.yml`) | not run | none; it needs a green `build-service` run on the tag commit | - |
| Live demo against Studio ([acceptance-mvp-service.md](acceptance-mvp-service.md)) | not run | none | - |

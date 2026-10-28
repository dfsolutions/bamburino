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
| `studio_list_filaments` | not run | none | - |
| `studio_assign_filament` | not run | none | - |
| `studio_plate_create` | not run | none | - |
| `studio_plate_rename` | not run | none | - |
| `studio_plate_delete` | not run | none | - |
| `studio_move_to_plate` | not run | none | - |
| `studio_arrange` | not run | none | - |
| `studio_save_project` | not run | none | - |
| `studio_export_plate` | not run | none | - |
| Install as Windows service | not run | none | - |
| Ctrl+Z reverts agent changes | manual | acceptance-mvp-service.md steps 4, 4b, 4c | - |
| Busy while modal dialog is open | manual | acceptance-mvp-service.md step 10 | - |
| Coexistence with official Bambu Studio | manual | none | - |

## Build and CI

| Check | Status | Source | Date |
|---|---|---|---|
| Patched Studio builds, MCP standalone tests pass, portable zip packaged on the self-hosted runner (`build-studio`, run 40659835599, attempt 2) | verified | automated, [run](https://github.com/dfsolutions/bamburino/actions/runs/40659835599) | 2026-10-28 |
| Studio zip: SHA-256 matches, root holds `bambu-studio.exe`, `SOURCE.txt`, `LICENSE`, `NOTICE.md`, `install-studio.ps1`; patch_id `2703638ef313` matches `driver/upstream.toml` | verified | automated, downloaded artifact `studio-win64` | 2026-10-28 |
| Service build, tests and package on the hosted runner (`build-service`, run 40659835498) | failed | automated, [run](https://github.com/dfsolutions/bamburino/actions/runs/40659835498): 4 ACL tests failed on the hosted runner; fixed in 69b2288, later runs (40668601313, 40673230961) are green |
| Pre-release `v0.1.0-rc3` (commit 101b305, merge of PR #1), `build-service` [40776212935](https://github.com/dfsolutions/bamburino/actions/runs/40776212935), `build-studio` [40776212923](https://github.com/dfsolutions/bamburino/actions/runs/40776212923) (attempt 3; attempts 1-2 failed on stale `D:b` paths left in the CMake cache and the deps-out `.cmake`/`.pc` files after the build root move, fixed in the build tree only), `release.yml` [40782329333](https://github.com/dfsolutions/bamburino/actions/runs/40782329333): [pre-release](https://github.com/dfsolutions/bamburino/releases/tag/v0.1.0-rc3) with service zip, Studio zip and both `.sha256` files; service zip sha256 re-checked, `bamburino.exe --version` reports 0.1.0, `config --help` lists `rotate-client-token` | verified | automated | 2026-10-28 |
| Release dry run: tag `v0.1.0-rc2` (commit 16e0ae3), `build-service` [40673230961](https://github.com/dfsolutions/bamburino/actions/runs/40673230961), `build-studio` [40673231043](https://github.com/dfsolutions/bamburino/actions/runs/40673231043), `release.yml` [40676412379](https://github.com/dfsolutions/bamburino/actions/runs/40676412379): [pre-release](https://github.com/dfsolutions/bamburino/releases/tag/v0.1.0-rc2) with service zip, Studio zip and both `.sha256` files; checksums and Studio zip root re-checked | verified | automated | 2026-10-28 |
| `release.yml` first attempt (run 40676323955) | failed | automated: its run-path check rejected tag-triggered runs; fixed in adadb2b, the second dispatch passed | 2026-10-28 |
| `build-studio` on tag `v0.1.0-rc1`, attempt 1 (run 40669441266) | failed | automated: the runner process tree and the build died at about 11:14 with no log message or event-log entry (cause not established; the PC was in use); the tag was deleted and replaced by rc2 | 2026-10-28 |
| `build-studio` on `main` at fafc0ed, attempt 1 (run 40659835599) | failed | automated: build killed mid-compilation (exit -1), no compile error; attempt 2 verified | 2026-10-28 |
| Live demo against Studio ([acceptance-mvp-service.md](acceptance-mvp-service.md)) | not run | none | - |

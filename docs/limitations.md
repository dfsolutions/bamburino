# Limitations

What Bamburino does not do, or does only within stated bounds. Status vocabulary used across the docs: `verified | failed | not run | manual`.

## Scope of the MVP

- **Windows x64 only.** One Bambu Studio version at a time: `v02.08.04.57`, with the Bamburino driver patch.
- **No printer actions.** No start of a print, no printer control, no sending to a printer. Not in this version, by design.
- **Bamburino does not start Studio.** Studio must be open (the Bamburino build, with Preferences > Local MCP server on). With Studio closed, only `bamburino_capabilities` and `catalog_search` work.
- **Not in scope:** support painting, lay-on-face, comparison of variants, filament preset editing, opening or importing files. Plates, filament assignment per object, arrange, saving the project and exporting a sliced plate exist now (`studio_plate_*`, `studio_assign_filament`, `studio_arrange`, `studio_save_project`, `studio_export_plate`); they are `not run` against a live Studio, see [acceptance-plates.md](acceptance-plates.md).

## Plates, filaments and files

- **File tools work only inside `allowed_dirs`.** `studio_save_project` and `studio_export_plate` take an absolute path in an existing folder under one of the roots written in `config.toml` (listed by `bamburino_capabilities`). They never create folders and never overwrite unless `overwrite: true`. `config init` writes the roots for the account that runs it; `--install` run by a different admin writes that admin's Documents and Downloads folders.
- **The service runs as LocalService and may not read the user's folder.** Studio writes the file as the user; the service then reads it back for `bytes` and `sha256`. When it cannot, the call still succeeds with `sha256: null` and `hash_unavailable_reason`. Whether the Downloads folder is readable in service mode is acceptance row P10.
- **Arrange is project-wide.** `studio_arrange` selects the plate you name, but the driver arranges the whole project, so instances can move to other plates (`moved_between_plates`). The plate stays selected afterwards.
- **One slice result at a time.** Studio holds the result of the selected plate only, so slice and export plate by plate (`NOT_SLICED` otherwise).
- **Project colours rely on `filament_colour`.** The format Studio returns is recorded in acceptance row P2; a colour that cannot be parsed is `color_hex: null`.
- **The AMS is optional.** Without a selected, online printer with an AMS, `studio_list_filaments` returns `ams: null` and a reason; the agent must ask which colours are loaded.
- **Printing is out of scope.** The exported files are opened with Bambu Connect or official Bambu Studio; the patched Studio cannot print.

## Writes

- **Only 20 keys are writable live**, although the catalog describes 603 settings. The driver accepts: `layer_height`, `initial_layer_print_height`, `wall_loops`, `top_shell_layers`, `bottom_shell_layers`, `sparse_infill_density`, `sparse_infill_pattern`, `enable_support`, `support_type`, `support_threshold_angle`, `support_on_build_plate_only`, `brim_type`, `brim_width`, `skirt_loops`, `outer_wall_speed`, `inner_wall_speed`, `sparse_infill_speed`, `travel_speed`, `print_sequence`, `seam_position`. For plate and object overrides the list has 19 keys (all but `print_sequence`). Any other key is refused with `NOT_WRITABLE_BY_DRIVER`.
- **Supports** are the project/plate/object settings of the list above. A plate-scope support request is implemented as per-object overrides.
- **Rotation read-back is normalized.** After you set Z to 270 degrees, Studio reports -90. The value is equivalent; the representation changes.
- **Whether Ctrl+Z reverts a process-setting edit** is not established: the driver takes an undo snapshot before each change, but the snapshot stores the model, selection and plates, not the process preset. Status: `manual` (acceptance step 4, not recorded yet).

## Conflict detection

See [architecture.md](architecture.md) for the model. The gaps:

- A single manual edit during an agent's write window is indistinguishable from an undo jump and is not detected.
- A manual edit between the last revision refresh and the mutation can be overwritten; polling narrows the window but cannot close it.
- Without the `undoSnapshot` signal, edits that leave all hashed values identical are invisible.

## Read-backs

- **Print time** comes from Studio's own estimate string, parsed.
- **Filament grams, meters and volume** come from parsing the header of a temporary G-code that the service exports and deletes. The slice preview in Studio is not affected.
- Slice results exist only for the selected plate.

## Environment

- The patched Studio build is **unsigned**: SmartScreen warns on first run. The service installer is unsigned as well.
- It is an **unofficial build** of Bambu Studio. Do not run it at the same time as the official one; they share `%APPDATA%\BambuStudio`.
- **Reserved TCP ranges.** Hyper-V / WinNAT can reserve whole port ranges (check with `netsh int ipv4 show excludedportrange protocol=tcp`). Binding a port inside one fails with error 10013. Both the service port (`listen_port`, default 27600) and the driver port (Studio Preferences, default 27183) are configurable for this reason.
- The service runs as `NT AUTHORITY\LocalService`. `config set-driver-token` takes effect without a restart. `--install` run elevated under another account makes that account the owner of the configuration.
- The self-hosted CI runner that builds Studio and runs the GUI smoke test is on demand and takes maintainer pushes only ([ci-runner.md](ci-runner.md)).

## Verification

Per-function status is in [verification.md](verification.md). A function that does not appear there as `verified` is `not run` or `manual`: as of 2026-10-28 none of the 21 tools has been run against a live Studio.

<!-- verification:start -->
**Functions**

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

**Build and CI**

| Check | Status | Source | Date |
|---|---|---|---|
| Patched Studio builds, MCP standalone tests pass, portable zip packaged on the self-hosted runner (`build-studio`, run 40659835599, attempt 2) | verified | automated, [run](https://github.com/dfsolutions/bamburino/actions/runs/40659835599) | 2026-10-28 |
| Studio zip: SHA-256 matches, root holds `bambu-studio.exe`, `SOURCE.txt`, `LICENSE`, `NOTICE.md`, `install-studio.ps1`; patch_id `2703638ef313` matches `driver/upstream.toml` | verified | automated, downloaded artifact `studio-win64` | 2026-10-28 |
| Service build, tests and package on the hosted runner (`build-service`, run 40659835498) | failed | automated, [run](https://github.com/dfsolutions/bamburino/actions/runs/40659835498): 4 ACL tests failed on the hosted runner; fixed in 69b2288, later runs (40668601313, 40673230961) are green |
| Release dry run: tag `v0.1.0-rc2` (commit 16e0ae3), `build-service` [40673230961](https://github.com/dfsolutions/bamburino/actions/runs/40673230961), `build-studio` [40673231043](https://github.com/dfsolutions/bamburino/actions/runs/40673231043), `release.yml` [40676412379](https://github.com/dfsolutions/bamburino/actions/runs/40676412379): [pre-release](https://github.com/dfsolutions/bamburino/releases/tag/v0.1.0-rc2) with service zip, Studio zip and both `.sha256` files; checksums and Studio zip root re-checked | verified | automated | 2026-10-28 |
| `release.yml` first attempt (run 40676323955) | failed | automated: its run-path check rejected tag-triggered runs; fixed in adadb2b, the second dispatch passed | 2026-10-28 |
| `build-studio` on tag `v0.1.0-rc1`, attempt 1 (run 40669441266) | failed | automated: the runner process tree and the build died at about 11:14 with no log message or event-log entry (cause not established; the PC was in use); the tag was deleted and replaced by rc2 | 2026-10-28 |
| `build-studio` on `main` at fafc0ed, attempt 1 (run 40659835599) | failed | automated: build killed mid-compilation (exit -1), no compile error; attempt 2 verified | 2026-10-28 |
| Live demo against Studio ([acceptance-mvp-service.md](acceptance-mvp-service.md)) | not run | none | - |
<!-- verification:end -->

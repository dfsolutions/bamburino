# Limitations

What Bamburino does not do, or does only within stated bounds. Status vocabulary used across the docs: `verified | failed | not run | manual`.

## Scope of the MVP

- **Windows x64 only.** One Bambu Studio version at a time: `v02.08.04.57`, with the Bamburino driver patch.
- **No printer actions.** No start of a print, no printer control, no sending to a printer. Not in this version, by design.
- **Bamburino does not start Studio.** Studio must be open (the Bamburino build, with Preferences > Local MCP server on). With Studio closed, only `bamburino_capabilities` and `catalog_search` work.
- **Not in the MVP:** support painting, lay-on-face, comparison of variants, filament presets, multi-plate workflows, import and export of files through a tool. The path-handling rules exist in the service, but no tool takes a path yet.

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

Per-function status is recorded in `docs/verification.md` (not published yet). Until a function appears there as `verified`, it is `not run`.

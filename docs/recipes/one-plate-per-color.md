# Recipe: one plate per colour

For an agent that sorts a project into plates by filament colour and hands the user the files. Tool details: [../tools.md](../tools.md).

## Decision rule

Objects that need two or more distinct filaments on one plate become one plate per filament, unless the user explicitly wants a multi-colour print. When in doubt, ask once before moving anything.

## Call order

1. `studio_get_state`: plates, objects, `revision`. Note which plates are locked.
2. `studio_list_filaments`: project slots (1-based) and, when present, the AMS trays. `ams: null` is normal; it comes with `ams_unavailable_reason`.
3. `studio_assign_filament` for each object, with the slot of the colour the user asked for. Pass the latest `revision` each time.
4. `studio_plate_create` once per colour, named by the colour (for example `Red`). Re-read `studio_get_state` for the new `revision`.
5. `studio_move_to_plate` for each object instance to its colour plate.
6. `studio_arrange` once per plate (required `plate_index`). Arrange is project-wide, so read `moved_between_plates`. It waits; a job still running at the timeout is polled with `studio_slice_status` using its `job_id`, and changes are refused as `BUSY` until it ends.
7. Plate by plate: `studio_slice` with that `plate_id`, poll `studio_slice_status` until `succeeded`, `studio_get_slice_summary`, then `studio_export_plate` to a `.gcode.3mf` path inside `allowed_dirs`. Slice and export one plate at a time: `slice_get_state` holds one result, so slicing every plate first loses all but the last.
8. Finally `studio_save_project` to a `.3mf` path inside `allowed_dirs`.

After every `studio_plate_delete`, re-read `studio_get_state`: the plate indices after the deleted one shift.

Paths: `bamburino_capabilities` lists `allowed_dirs`. Never overwrite without the user's yes (`overwrite: true`); on `FILE_EXISTS` pick another name or ask.

## What to tell the user, per plate

- the colour and the plate name;
- print time and grams from `studio_get_slice_summary`;
- the file path of the exported `.gcode.3mf` (and its `sha256` when it is not null);
- which AMS tray to load, when the colour is not loaded (from `studio_list_filaments`); never invent a tray.

Then the project file path from `studio_save_project`.

## Stop rules

- A requested colour that is in neither the project filaments nor the AMS: stop and ask the user. Never guess a colour or a slot.
- A locked plate (`PLATE_LOCKED`): say so and ask the user to unlock it in Studio. Do not work around it.
- `moved_between_plates` not empty after `studio_arrange`: tell the user which instances changed plate and re-check the layout (`studio_get_state`, `studio_list_models`) before slicing.
- `NOT_SLICED`: slice that plate (`studio_slice` with its `plate_id`, wait for `succeeded`) and export again. Do not retry the export blindly.
- `REVISION_CONFLICT`: the user changed the project; re-read and decide again.

## Hand-off text

Say that the files are opened with Bambu Connect or official Bambu Studio, and that printing is done there: the patched Studio used by Bamburino cannot print (its authorization control is not available). Bamburino never starts a print.

# Recipe: one plate per colour

For an agent that sorts a project into plates by filament colour and hands the user the files. Tool details: [../tools.md](../tools.md).

## Decision rule

Objects that need two or more distinct filaments on one plate become one plate per filament, unless the user explicitly wants a multi-colour print. When in doubt, ask once before moving anything.

## Call order

1. `studio_get_state`: plates, objects, `revision`. Note which plates are locked.
2. `studio_list_filaments`: project slots (1-based) and, when present, the AMS trays. `ams: null` is normal; it comes with `ams_unavailable_reason`.
3. `studio_assign_filament` for each object, with `filament_slot` (1-based) set to the slot of the colour the user asked for. Pass the latest `revision` each time. Assignment is per object, not per part: if an object has per-part or painted colours, see the stop rules.
4. Plate 0 is the first colour: `studio_plate_rename` plate 0 to that colour (for example `Red`). Then `studio_plate_create` once per further colour (N colours need N-1 new plates), named by the colour. Re-read `studio_get_state` for the new `revision` and the plate indices.
5. `studio_move_to_plate` for each object instance to its colour plate (instances of the first colour stay on plate 0). The driver centres a moved instance on the destination plate, so with one instance per plate nothing overlaps and no arrange is needed.
6. Only when a plate holds several instances: `studio_arrange`. Arrange is project-wide (`scope: "project"`), not per plate; `plate_index` only selects a plate (the selection persists and one undo snapshot is taken). It waits; a job still running at the timeout is polled with `studio_slice_status` using its `job_id`, and changes are refused as `BUSY` until it ends. Then check `moved_between_plates` in the result: if it is not empty, the arrange pulled instances off their colour plates. Redo those moves with `studio_move_to_plate`, position the instances on the plate with `studio_transform_model`, or ask the user to arrange that plate in Studio. Never loop arrange and moves.
7. Plate by plate: `studio_slice` with that `plate_id`, poll `studio_slice_status` until `succeeded`, `studio_get_slice_summary` (read colours from `filaments[].slot`, 1-based; `index` is 0-based), then `studio_export_plate` to a `.gcode.3mf` path inside `allowed_dirs`. Slice and export one plate at a time: `slice_get_state` holds one result, so slicing every plate first loses all but the last.
8. Finally `studio_save_project` to a `.3mf` path inside `allowed_dirs`.

After every `studio_plate_delete`, re-read `studio_get_state`: the plate indices after the deleted one shift.

Paths: `bamburino_capabilities` lists `allowed_dirs`. Never overwrite without the user's yes (`overwrite: true`); on `FILE_EXISTS` pick another name or ask.

## What to tell the user, per plate

- the colour and the plate name;
- print time and grams from `studio_get_slice_summary`;
- the file path of the exported `.gcode.3mf` (and its `sha256` when it is not null);
- which AMS tray to load, when the colour is not loaded (from `studio_list_filaments`); never invent a tray. When the colour is in more than one tray, name them all and let the user choose.

Then the project file path from `studio_save_project`.

## Stop rules

- A requested colour that is in neither the project filaments nor the AMS: stop and ask the user. Never guess a colour or a slot.
- A locked plate (`PLATE_LOCKED`): say so and ask the user to unlock it in Studio. Do not work around it.
- `moved_between_plates` not empty after `studio_arrange`: tell the user which instances changed plate, re-check the layout (`studio_get_state`, `studio_list_models`) and follow step 6. Do not slice before the layout is right.
- After `studio_get_slice_summary`, `filaments[]` must have exactly one row for a single-colour plate. Any other count: stop and tell the user the object has per-part or painted colours, which this recipe does not handle.
- `NOT_SLICED`: slice that plate (`studio_slice` with its `plate_id`, wait for `succeeded`) and export again. Do not retry the export blindly.
- `REVISION_CONFLICT`: the user changed the project; re-read and decide again.

## Hand-off text

Say that the files are opened with Bambu Connect or official Bambu Studio, and that printing is done there: the patched Studio used by Bamburino cannot print (its authorization control is not available). Bamburino never starts a print.

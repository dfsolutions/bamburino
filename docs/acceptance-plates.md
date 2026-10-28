# Plates, filaments and export acceptance

Outcome vocabulary: `verified | failed | not run | manual`. A row is `verified` only when the step was executed and its result recorded in the observed column. Every row below is `not run`. Rows P7 and P11 are `manual` checks: record what was seen.

| step | action | expected | observed | result | date |
|---|---|---|---|---|---|
| P1 | Open a project with two objects; load two AMS colours on the selected printer | Studio shows both objects and the AMS trays |  | not run | - |
| P2 | `studio_list_filaments` (record the raw `filament_colour` format and the AMS colour format seen) | Project slots 1-based with `color_hex`; `ams` lists the loaded trays | | not run | - |
| P3 | `studio_assign_filament` per object (record whether Studio's object list shows the new filament) | `filament_slot` set and read back; the object list shows the filament | | not run | - |
| P4 | `studio_plate_rename` plate 0 to the first colour, `studio_plate_create` for the second (named by colour), `studio_move_to_plate` of the second object | Two named plates, one object on each, each instance centred on its plate (driver centring; record the positions) | | not run | - |
| P5 | With two instances on one plate, `studio_arrange` (record whether it arranged all plates or one, and `moved_between_plates`); also record that with one instance per plate no arrange is needed | Objects arranged; the scope and any cross-plate move are recorded | | not run | - |
| P6 | Slice and `studio_export_plate` for plates A and B into Downloads | Two `.gcode.3mf` files with `bytes` and `sha256` | | not run | - |
| P7 | Open one exported file with Bambu Connect or official Bambu Studio (`manual`) | The file opens with the right colour | | not run | - |
| P8 | `studio_save_project` into Downloads | A `.3mf` project file with `bytes` and `sha256` | | not run | - |
| P9 | Repeat P8 and P6 with the same path and no `overwrite` | `FILE_EXISTS`, the file is unchanged | | not run | - |
| P10 | Service mode (LocalService): export to Downloads and read the result (LocalService versus Downloads) | The path is accepted and the file is read: `sha256` present, or `null` with `hash_unavailable_reason` (record which and the reason) | | not run | - |
| P11 | Ctrl+Z in Studio after the plate operations (`manual`, record what was observed) | Record whether plate create, move and delete are undone | | not run | - |

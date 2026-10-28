// SPDX-License-Identifier: AGPL-3.0-or-later
//! Mapping between [`DriverClient`](super::DriverClient) methods
//! and driver tools, plus the driver-side whitelists.
//!
//! | `DriverClient` method | Driver tool | Driver arguments |
//! |---|---|---|
//! | `app_state` | `app_get_state` | `{}` |
//! | `capabilities` | `app_get_capabilities` | `{}` |
//! | `plate_list` | `plate_list` | `{}` |
//! | `plate_select` | `plate_select` | `{plateIndex, expectedPlateRevision}`; returns the new plate state; mutation, takes an undo snapshot (earlier plate revisions go stale) |
//! | `model_list` | `model_list` | `{}` |
//! | `active_presets` | `preset_list` x3 | `{kind: "printer"\|"process"\|"filament"}` (selected preset: field `selected`; filaments: `slots`) |
//! | `writable_keys` | `settings_get` | `{}` -> `writableKeys` |
//! | `settings_effective` | `settings_effective` | `{keys?, plateIndex?+expectedPlateRevision?, objectId?}` |
//! | `settings_update` | `settings_update` | `{values}` |
//! | `settings_override` | `settings_override` | `{scope: "plate"\|"object", values, plateIndex+expectedPlateRevision \| objectId}`; returns new `plates.plateRevision` |
//! | `object_transform` | `object_transform` | `{objectId, instanceId (required), position, rotation, scale}`; returns the whole model state |
//! | `slice_start` | `slice_start` | `{plateIndex, expectedPlateRevision}`; selected plate only |
//! | `job_get` / `job_cancel` | `job_get` / `job_cancel` | `{jobId}` |
//! | `slice_state` | `slice_get_state` | `{}` (`busy`, `valid`, `sliceResultId`, `plateIndex`, `statistics.estimatedNormalTime` -> print time) |
//! | `preview_summary` | `preview_get_summary` | `{plateIndex, expectedPlateRevision, sliceResultId}`; returns move counts and warnings only. Print time comes from `slice_get_state.statistics.estimatedNormalTime` (dhms string, parsed); filament grams/meters/volume come from `export_gcode` plus the G-code header (`; filament: ids`, `; total filament weight [g] : ...`, etc.) |
//! | `export_gcode` | `export_gcode` | `{plateIndex, expectedPlateRevision, sliceResultId, path, overwrite: false}`; absolute `.gcode` path in an existing directory; the only `export_*` tool on the allowlist. Used by `studio_get_slice_summary` to read grams/meters/volume from the header of a temp file, which is deleted afterwards. `is_mutation() == true` is correct: the driver's read list (`McpTools.cpp:1482`) does not contain `export.gcode`, so the call passes the same BUSY gate as the other mutations (it writes a file, not editor state, and takes no undo snapshot) |
//! | `plate_create` | `plate_create` | `{}`; returns the plate state; mutation, takes an undo snapshot |
//! | `plate_rename` | `plate_rename` | `{plateIndex, expectedPlateRevision, name}`; returns the plate state; mutation, takes an undo snapshot |
//! | `plate_delete` | `plate_delete` | `{plateIndex, expectedPlateRevision, contents: "require_empty"\|"move"\|"delete", targetPlateIndex?}` (`targetPlateIndex` omitted when `None`); returns `{contents, movedInstanceCount, deletedObjectCount, destinationPlateIndex?, plates{...}, model{...}}`; mutation, takes an undo snapshot |
//! | `plate_move_instance` | `plate_move_instance` | `{objectId, instanceId, plateIndex, expectedPlateRevision}`; returns the plate state under `plates`; mutation, takes an undo snapshot |
//! | `filament_assign` | `filament_assign` | `{objectId, filamentIndex}` (0-based toward the driver, 1-based in Bamburino); returns `{target{objectId}, filamentIndex, preset}`; mutation, takes an undo snapshot |
//! | `arrange_start` | `arrange_start` | `{}`; returns a UI job (`error` may be a plain string); mutation, takes an undo snapshot |
//! | `device_list` | `device_list` | `{}` -> `{pluginAvailable, devices[{deviceId, name, selected, online, connected, ...}]}`; read-only, no `device_*` action tool is representable |
//! | `ams_inventory` | `ams_inventory` | `{deviceId}` -> units with `slots[{slotId, exists, filamentType, color, remainingPercent}]`; read; `remainingPercent` is kept raw (`-1` = unknown) |
//! | `project_save` | `project_save` | `{path, overwrite}`; absolute UTF-8 path, `overwrite` always sent (default `false`); mutation gate, no undo snapshot |
//! | `export_sliced_3mf` | `export_sliced_3mf` | `{plateIndex, expectedPlateRevision, sliceResultId, path, overwrite}`; exports only the selected plate; returns `{sliceResultId, plateIndex, path, bytes}`; mutation gate, no undo snapshot |
//!
//! Job result shape (`slice_start`, `job_get`, `job_cancel`, `export_gcode`):
//! `{jobId, plateIndex, status, error{code,message}, result{sliceResultId,reused}}`,
//! verified against `McpTools.cpp:653-663` (Task 6 review).
//!
//! The driver also accepts `scope: "volume"` / `volumeId` in `settings_effective`
//! and `settings_override`. This is out of the MVP: Bamburino exposes
//! project, plate and object scopes only.
//!
//! `model_import` takes no undo snapshot and is not part of the trait.

/// Keys accepted by the driver's `settings_update` (`writable_settings()`).
pub const WRITABLE_KEYS: [&str; 20] = [
    "layer_height",
    "initial_layer_print_height",
    "wall_loops",
    "top_shell_layers",
    "bottom_shell_layers",
    "sparse_infill_density",
    "sparse_infill_pattern",
    "enable_support",
    "support_type",
    "support_threshold_angle",
    "support_on_build_plate_only",
    "brim_type",
    "brim_width",
    "skirt_loops",
    "outer_wall_speed",
    "inner_wall_speed",
    "sparse_infill_speed",
    "travel_speed",
    "print_sequence",
    "seam_position",
];

/// Keys accepted by the driver's `settings_override` (`override_settings()`):
/// the writable keys without `print_sequence`.
pub const OVERRIDE_KEYS: [&str; 19] = [
    "layer_height",
    "initial_layer_print_height",
    "wall_loops",
    "top_shell_layers",
    "bottom_shell_layers",
    "sparse_infill_density",
    "sparse_infill_pattern",
    "enable_support",
    "support_type",
    "support_threshold_angle",
    "support_on_build_plate_only",
    "brim_type",
    "brim_width",
    "skirt_loops",
    "outer_wall_speed",
    "inner_wall_speed",
    "sparse_infill_speed",
    "travel_speed",
    "seam_position",
];

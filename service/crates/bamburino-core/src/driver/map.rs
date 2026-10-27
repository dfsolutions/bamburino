// SPDX-License-Identifier: AGPL-3.0-or-later
//! Provisional mapping between [`DriverClient`](super::DriverClient) methods
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
//! | `preview_summary` | `preview_get_summary` | `{plateIndex, expectedPlateRevision, sliceResultId}`; returns move counts and warnings only. Print time comes from `slice_get_state`; filament usage (grams/meters) and support volume: source TBD by audit (Task 18) |
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

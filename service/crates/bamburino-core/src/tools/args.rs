// SPDX-License-Identifier: AGPL-3.0-or-later
//! Argument types of the tools (also the MCP input schemas).

use schemars::JsonSchema;
use serde::Deserialize;

use crate::validate::Scope;

/// Default and maximum number of `catalog_search` hits.
pub const SEARCH_DEFAULT_LIMIT: u32 = 20;
pub const SEARCH_MAX_LIMIT: u32 = 50;

/// Which plate or object a scoped call refers to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetRef {
    /// Plate number as reported by `studio_get_state` (`plates[].plate_id`).
    pub plate_id: Option<u32>,
    /// Object id as reported by `studio_get_state` (`models[].object_id`).
    pub object_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetSettingsArgs {
    /// `project`, `plate` (needs `target.plate_id`) or `object` (needs `target.object_id`).
    pub scope: Scope,
    pub target: Option<TargetRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    /// Words of a setting key, label or description.
    pub query: String,
    /// Maximum number of hits (default 20, maximum 50).
    pub limit: Option<u32>,
}

/// One setting change of `studio_set_settings`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Change {
    /// Setting key, e.g. `sparse_infill_density` (see `catalog_search`).
    pub key: String,
    /// New value in natural form (number, string, bool or array).
    pub value: serde_json::Value,
    /// Defaults to `project`.
    pub scope: Option<Scope>,
    /// Needed for scope `plate` (`plate_id`) and `object` (`object_id`).
    pub target: Option<TargetRef>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetSettingsArgs {
    /// 1 to 100 changes, applied all-or-nothing as far as validation goes.
    pub changes: Vec<Change>,
    /// The `revision` from the last read; a mismatch returns 409 and writes nothing.
    pub expected_revision: u64,
    /// When true, validates and returns the diff without changing anything.
    pub dry_run: Option<bool>,
}

/// How `studio_transform_model` reads its values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransformMode {
    /// Rotation is composed with the current one, translation is added,
    /// scale is multiplied.
    #[default]
    Relative,
    /// The values replace the current ones.
    Absolute,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transform {
    /// Rotation about X, Y, Z in degrees.
    pub rotate_deg: Option<[f64; 3]>,
    /// Translation (relative) or position (absolute) in mm.
    pub translate_mm: Option<[f64; 3]>,
    /// Scale factors (relative: multiplied; absolute: replace). Must be > 0.
    pub scale: Option<[f64; 3]>,
    /// Defaults to `relative`.
    pub mode: Option<TransformMode>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransformArgs {
    /// Object id as reported by `studio_list_models`.
    pub object_id: String,
    /// Defaults to the first instance of the object.
    pub instance_id: Option<String>,
    /// At least one of `rotate_deg`, `translate_mm`, `scale`.
    pub transform: Transform,
    /// The `revision` from the last read; a mismatch returns 409 and writes nothing.
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SupportKind {
    Normal,
    Tree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SupportStyle {
    Auto,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SupportSettings {
    pub enabled: bool,
    /// `normal` or `tree`; with `style` it sets `support_type`.
    pub r#type: Option<SupportKind>,
    /// `auto` (default) or `manual`; only used together with `type`.
    pub style: Option<SupportStyle>,
    pub threshold_angle: Option<u32>,
    pub on_build_plate_only: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SupportTarget {
    /// `project`, `plate` (needs `plate_id`) or `object` (needs `object_id`).
    pub scope: Scope,
    pub plate_id: Option<u32>,
    pub object_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SupportsArgs {
    pub target: SupportTarget,
    pub settings: SupportSettings,
    /// The `revision` from the last read; a mismatch returns 409 and writes nothing.
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SliceArgs {
    /// Plate number as reported by `studio_get_state` (`plates[].plate_id`).
    /// Default: the selected plate. A different plate is selected first.
    pub plate_id: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobArgs {
    /// Job id returned by `studio_slice`.
    pub job_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryArgs {
    /// Plate to summarize. Default: the selected plate, which is the only one
    /// that can hold a current slice result.
    pub plate_id: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListFilamentsArgs {
    /// Printer whose AMS to read. Default: the printer selected in Studio.
    pub device_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssignFilamentArgs {
    /// Object to assign the filament to (`studio_list_objects`).
    pub object_id: String,
    /// Project filament slot, 1-based, as listed by `studio_list_filaments`.
    pub filament_slot: u32,
    /// Revision from `studio_get_state`; the call is refused when stale.
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlateCreateArgs {
    /// Name for the new plate. Default: the driver's "Plate N".
    pub name: Option<String>,
    /// Revision from `studio_get_state`; the call is refused when stale.
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlateRenameArgs {
    /// Plate to rename, 0-based (`studio_get_state` `plates[].plate_index`).
    pub plate_index: u32,
    /// New name; must not be empty.
    pub name: String,
    /// Revision from `studio_get_state`; the call is refused when stale.
    pub expected_revision: u64,
}

/// What `studio_plate_delete` does with the instances on the plate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlateDeleteContents {
    /// Move the instances to `target_plate_index` (must be empty and unlocked).
    Move,
    /// Delete the instances; objects left without instances go too.
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlateDeleteArgs {
    /// Plate to delete, 0-based. Later plates are renumbered.
    pub plate_index: u32,
    /// Required when the plate is not empty.
    pub contents: Option<PlateDeleteContents>,
    /// Destination plate for `contents: "move"`; refused with any other value.
    pub target_plate_index: Option<u32>,
    /// Revision from `studio_get_state`; the call is refused when stale.
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveToPlateArgs {
    /// Object the instance belongs to (`studio_list_models`).
    pub object_id: String,
    /// Instance to move. Optional only when the object has a single instance.
    pub instance_id: Option<String>,
    /// Destination plate, 0-based.
    pub plate_index: u32,
    /// Revision from `studio_get_state`; the call is refused when stale.
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArrangeArgs {
    /// Plate to select before arranging, 0-based. SIDE EFFECT: that plate
    /// stays selected afterwards (the result reports `previous_plate_index`).
    /// The driver arranges the whole project (`scope: "project"`) whichever
    /// plate is selected, so objects may move to other plates.
    pub plate_index: u32,
    /// Revision from `studio_get_state`; the call is refused when stale.
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveProjectArgs {
    /// Absolute path of the project file, ending `.3mf` (any case), inside one
    /// of the allowed directories (`allowed_dirs` in config.toml). The folder
    /// must exist; it is not created.
    pub path: String,
    /// Replace an existing file. Default false: an existing file is refused
    /// with FILE_EXISTS.
    pub overwrite: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportPlateArgs {
    /// Plate to export, 0-based. It must be the selected plate with a current,
    /// completed slice (`studio_slice` with `plate_id` = this index).
    pub plate_index: u32,
    /// Absolute path ending `.gcode.3mf` (any case), inside one of the allowed
    /// directories. The folder must exist; it is not created.
    pub path: String,
    /// Replace an existing file. Default false: an existing file is refused
    /// with FILE_EXISTS.
    pub overwrite: Option<bool>,
}

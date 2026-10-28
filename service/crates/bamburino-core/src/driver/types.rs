// SPDX-License-Identifier: AGPL-3.0-or-later
//! Plain data types exchanged with the driver.

/// Failure of a driver call.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DriverError {
    #[error("driver unreachable")]
    Unreachable,
    #[error("driver rejected the credentials")]
    Unauthorized,
    #[error("driver call timed out after {after_ms} ms")]
    Timeout { after_ms: u64 },
    #[error("driver busy: {message}")]
    Busy { message: String },
    #[error("stale reference: {message}")]
    StaleReference { message: String },
    #[error("invalid params: {message}")]
    InvalidParams { message: String },
    #[error("native error {code}: {message}")]
    Native { code: String, message: String },
    #[error("protocol error: {message}")]
    Protocol { message: String },
    #[error("tool not allowed: {tool}")]
    NotAllowed { tool: String },
}

/// Result of `app_get_capabilities`. Missing identity fields mean the build
/// is not a Bamburino build.
#[derive(Debug, Clone, PartialEq)]
pub struct DriverCapabilities {
    /// `appVersion`
    pub app_version: String,
    /// `buildSuffix`
    pub build_suffix: Option<String>,
    /// `patchId`
    pub patch_id: Option<String>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    pub session_id: String,
    pub app_version: String,
    pub project_name: String,
    pub project_path: String,
    pub dirty: bool,
    pub presets_dirty: bool,
    pub ui_job_running: bool,
    pub slicing_running: bool,
    pub modal_open: bool,
    pub object_count: u32,
    pub plate_count: u32,
    pub selected_plate_index: u32,
    pub undo_snapshot: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlateList {
    pub plates: Vec<Plate>,
    pub selected_index: u32,
    pub plate_revision: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plate {
    pub index: u32,
    pub name: String,
    pub locked: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelObject {
    pub id: String,
    pub index: u32,
    pub name: String,
    pub dimensions_mm: [f64; 3],
    pub instances: Vec<Instance>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub id: String,
    pub index: u32,
    pub position_mm: [f64; 3],
    pub rotation_deg: [f64; 3],
    pub scale: [f64; 3],
    pub plate_index: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivePresets {
    pub printer: Option<String>,
    pub process: Option<String>,
    pub filaments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsTarget {
    Project,
    Plate { index: u32, plate_revision: String },
    Object { object_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingSource {
    Project,
    Plate,
    Object,
    Volume,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveSetting {
    pub key: String,
    pub value: String,
    pub source: SettingSource,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbsoluteTransform {
    pub position_mm: [f64; 3],
    pub rotation_deg: [f64; 3],
    pub scale: [f64; 3],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
    /// The result was invalidated by a later edit.
    Stale,
    /// Any status string the driver reports that is not mapped above.
    /// (The driver never reports `queued`.)
    Unknown(String),
}

impl JobStatus {
    /// True for states a job never leaves.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Stale
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverJob {
    pub id: String,
    pub plate_index: u32,
    pub status: JobStatus,
    pub progress_percent: Option<u8>,
    pub message: Option<String>,
    pub error: Option<DriverJobError>,
    pub slice_result_id: Option<String>,
    /// `result.reused`: the driver returned a still-current earlier result.
    pub reused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverJobError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SliceState {
    pub running: bool,
    pub valid: bool,
    /// `statistics.estimatedNormalTime` of `slice_get_state`.
    pub estimated_time_s: Option<f64>,
    pub slice_result_id: Option<String>,
    pub plate_index: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SliceSummary {
    pub print_time_s: Option<f64>,
    pub filaments: Vec<FilamentUsage>,
    pub support_volume_mm3: Option<f64>,
    pub warnings: Vec<String>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FilamentUsage {
    pub index: u32,
    pub grams: Option<f64>,
    pub meters: Option<f64>,
}

/// What `plate_delete` does with the instances on the plate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlateContents {
    /// Refuse unless the plate is empty.
    RequireEmpty,
    /// Move the instances to another plate.
    Move,
    /// Delete the instances (objects left without instances go too).
    Delete,
}

impl PlateContents {
    /// The driver's wire string.
    pub fn wire(&self) -> &'static str {
        match self {
            Self::RequireEmpty => "require_empty",
            Self::Move => "move",
            Self::Delete => "delete",
        }
    }
}

/// Result of `plate_delete`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlateDeleted {
    pub plates: PlateList,
    pub moved_instances: u32,
    pub deleted_objects: u32,
    pub destination_plate_index: Option<u32>,
}

/// Result of `filament_assign`; `filament_index` is 0-based as the driver
/// reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilamentAssignment {
    pub object_id: String,
    pub filament_index: u32,
    pub preset: String,
}

/// One row of `device_list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSummary {
    pub device_id: String,
    pub name: String,
    pub selected: bool,
    pub online: bool,
    pub connected: bool,
}

/// One AMS slot. `remaining_percent` is raw (`-1` = unknown to the printer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmsTray {
    pub slot_id: String,
    pub exists: bool,
    pub filament_type: String,
    pub color: String,
    pub remaining_percent: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmsUnit {
    pub ams_id: String,
    pub trays: Vec<AmsTray>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmsInventory {
    pub units: Vec<AmsUnit>,
}

/// Result of `export_sliced_3mf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedPlate {
    pub slice_result_id: String,
    pub plate_index: u32,
    pub path: String,
    pub bytes: u64,
}

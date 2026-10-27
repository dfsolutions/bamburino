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
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    /// Any status string the driver reports that is not mapped above
    /// (e.g. `cancelling`, `stale`).
    Unknown(String),
}

impl JobStatus {
    /// True for states a job never leaves.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverJob {
    pub id: String,
    pub plate_index: u32,
    pub status: JobStatus,
    pub progress_percent: Option<u8>,
    pub message: Option<String>,
    pub error: Option<String>,
    pub slice_result_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceState {
    pub running: bool,
    pub valid: bool,
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

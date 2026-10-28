// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allowlist of driver tools the service may call.
//!
//! Physical-printer, calibration, account, history and preference tools of
//! the driver are deliberately not representable. The only exceptions are:
//!
//! - `project_save`: writes the project file the user asked to hand off;
//! - `export_gcode`: to a temp file, to read the filament usage out of its
//!   header;
//! - `export_sliced_3mf`: writes the sliced plate file the user asked for;
//! - `ams_inventory`: read-only, shows which filaments are loaded;
//! - `device_list`: read-only listing of the known printers (no `device_*`
//!   action tool is representable).
//!
//! This enum is the only way to build a `tools/call` request.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DriverTool {
    AppGetCapabilities,
    AppGetState,
    PlateList,
    PlateSelect,
    ModelList,
    PresetList,
    SettingsGet,
    SettingsEffective,
    SettingsUpdate,
    SettingsOverride,
    ObjectTransform,
    SliceGetState,
    SliceStart,
    JobGet,
    JobCancel,
    PreviewGetSummary,
    ExportGcode,
    PlateCreate,
    PlateDelete,
    PlateRename,
    PlateMoveInstance,
    ArrangeStart,
    FilamentAssign,
    AmsInventory,
    DeviceList,
    ProjectSave,
    ExportSliced3mf,
}

impl DriverTool {
    pub const ALL: [DriverTool; 27] = [
        Self::AppGetCapabilities,
        Self::AppGetState,
        Self::PlateList,
        Self::PlateSelect,
        Self::ModelList,
        Self::PresetList,
        Self::SettingsGet,
        Self::SettingsEffective,
        Self::SettingsUpdate,
        Self::SettingsOverride,
        Self::ObjectTransform,
        Self::SliceGetState,
        Self::SliceStart,
        Self::JobGet,
        Self::JobCancel,
        Self::PreviewGetSummary,
        Self::ExportGcode,
        Self::PlateCreate,
        Self::PlateDelete,
        Self::PlateRename,
        Self::PlateMoveInstance,
        Self::ArrangeStart,
        Self::FilamentAssign,
        Self::AmsInventory,
        Self::DeviceList,
        Self::ProjectSave,
        Self::ExportSliced3mf,
    ];

    /// Tool name as exposed by the driver (snake_case).
    pub fn name(&self) -> &'static str {
        match self {
            Self::AppGetCapabilities => "app_get_capabilities",
            Self::AppGetState => "app_get_state",
            Self::PlateList => "plate_list",
            Self::PlateSelect => "plate_select",
            Self::ModelList => "model_list",
            Self::PresetList => "preset_list",
            Self::SettingsGet => "settings_get",
            Self::SettingsEffective => "settings_effective",
            Self::SettingsUpdate => "settings_update",
            Self::SettingsOverride => "settings_override",
            Self::ObjectTransform => "object_transform",
            Self::SliceGetState => "slice_get_state",
            Self::SliceStart => "slice_start",
            Self::JobGet => "job_get",
            Self::JobCancel => "job_cancel",
            Self::PreviewGetSummary => "preview_get_summary",
            Self::ExportGcode => "export_gcode",
            Self::PlateCreate => "plate_create",
            Self::PlateDelete => "plate_delete",
            Self::PlateRename => "plate_rename",
            Self::PlateMoveInstance => "plate_move_instance",
            Self::ArrangeStart => "arrange_start",
            Self::FilamentAssign => "filament_assign",
            Self::AmsInventory => "ams_inventory",
            Self::DeviceList => "device_list",
            Self::ProjectSave => "project_save",
            Self::ExportSliced3mf => "export_sliced_3mf",
        }
    }

    /// True for tools the driver gates as mutations (refused while a modal
    /// dialog is open or a job is running). `ExportGcode` is one of them: it
    /// writes a file, not editor state, but it is not in the driver's read
    /// list (`McpTools.cpp:1482`), so it passes the same BUSY gate. `AmsInventory`
    /// and `DeviceList` are driver reads. `JobCancel` is a
    /// read in the driver (it bypasses that gate), so it is not listed.
    pub fn is_mutation(&self) -> bool {
        matches!(
            self,
            Self::PlateSelect
                | Self::SettingsUpdate
                | Self::SettingsOverride
                | Self::ObjectTransform
                | Self::SliceStart
                | Self::ExportGcode
                | Self::PlateCreate
                | Self::PlateDelete
                | Self::PlateRename
                | Self::PlateMoveInstance
                | Self::ArrangeStart
                | Self::FilamentAssign
                | Self::ProjectSave
                | Self::ExportSliced3mf
        )
    }
}

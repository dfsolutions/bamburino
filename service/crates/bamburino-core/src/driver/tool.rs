// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allowlist of driver tools the service may call.
//!
//! Physical-device, account, calibration, project, export, history and
//! preference tools of the driver are deliberately not representable: this
//! enum is the only way to build a `tools/call` request.

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
}

impl DriverTool {
    pub const ALL: [DriverTool; 16] = [
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
        }
    }

    /// True for tools that change editor state; the driver refuses these
    /// while a modal dialog is open or a job is running.
    pub fn is_mutation(&self) -> bool {
        matches!(
            self,
            Self::PlateSelect
                | Self::SettingsUpdate
                | Self::SettingsOverride
                | Self::ObjectTransform
                | Self::SliceStart
                | Self::JobCancel
        )
    }
}

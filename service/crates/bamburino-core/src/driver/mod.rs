// SPDX-License-Identifier: AGPL-3.0-or-later
//! Abstraction over the in-process MCP driver of the patched Bambu Studio.
//!
//! [`DriverClient`] is the only surface the service layers see; the HTTP
//! implementation and the in-memory `MockDriver` both implement it.

pub mod http;
pub mod map;
#[cfg(feature = "mock")]
pub mod mock;
pub mod tool;
pub mod types;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub use http::{HttpDriverClient, HttpDriverConfig};
pub use tool::DriverTool;
pub use types::*;

#[async_trait::async_trait]
pub trait DriverClient: Send + Sync {
    /// Whether a driver token is currently configured. Clients without a
    /// token concept (mocks) report `true`.
    fn token_configured(&self) -> bool {
        true
    }
    async fn app_state(&self) -> Result<AppState, DriverError>;
    async fn capabilities(&self) -> Result<DriverCapabilities, DriverError>;
    async fn plate_list(&self) -> Result<PlateList, DriverError>;
    /// Returns the plate state with the fresh revision issued after the
    /// selection snapshot.
    async fn plate_select(
        &self,
        plate_index: u32,
        plate_revision: &str,
    ) -> Result<PlateList, DriverError>;
    async fn model_list(&self) -> Result<Vec<ModelObject>, DriverError>;
    async fn active_presets(&self) -> Result<ActivePresets, DriverError>;
    async fn writable_keys(&self) -> Result<BTreeSet<String>, DriverError>;
    async fn settings_effective(
        &self,
        target: &SettingsTarget,
        keys: Option<&[String]>,
    ) -> Result<Vec<EffectiveSetting>, DriverError>;
    async fn settings_update(&self, values: &BTreeMap<String, String>) -> Result<(), DriverError>;
    /// Returns the plate state with the fresh `plates.plateRevision`.
    async fn settings_override(
        &self,
        target: &SettingsTarget,
        values: &BTreeMap<String, String>,
    ) -> Result<PlateList, DriverError>;
    async fn object_transform(
        &self,
        object_id: &str,
        instance_id: &str,
        t: &AbsoluteTransform,
    ) -> Result<ModelObject, DriverError>;
    async fn slice_start(
        &self,
        plate_index: u32,
        plate_revision: &str,
    ) -> Result<DriverJob, DriverError>;
    async fn job_get(&self, job_id: &str) -> Result<DriverJob, DriverError>;
    async fn job_cancel(&self, job_id: &str) -> Result<DriverJob, DriverError>;
    async fn slice_state(&self) -> Result<SliceState, DriverError>;
    async fn preview_summary(
        &self,
        plate_index: u32,
        plate_revision: &str,
        slice_result_id: &str,
    ) -> Result<SliceSummary, DriverError>;
    /// Exports the G-code of the current slice result of the selected plate
    /// to `path` (absolute, `.gcode`, existing directory, must not exist).
    /// Returns the (already terminal) export job. Takes no undo snapshot.
    async fn export_gcode(
        &self,
        plate_index: u32,
        plate_revision: &str,
        slice_result_id: &str,
        path: &Path,
    ) -> Result<DriverJob, DriverError>;
    /// Adds an empty plate. Takes an undo snapshot.
    async fn plate_create(&self) -> Result<PlateList, DriverError>;
    /// Renames a plate. Takes an undo snapshot.
    async fn plate_rename(
        &self,
        plate_index: u32,
        plate_revision: &str,
        name: &str,
    ) -> Result<PlateList, DriverError>;
    /// Deletes a plate, handling its instances per `contents`;
    /// `target_plate_index` is only sent with `Move`. Takes an undo snapshot.
    async fn plate_delete(
        &self,
        plate_index: u32,
        plate_revision: &str,
        contents: PlateContents,
        target_plate_index: Option<u32>,
    ) -> Result<PlateDeleted, DriverError>;
    /// Moves one instance to another plate. Takes an undo snapshot.
    async fn plate_move_instance(
        &self,
        object_id: &str,
        instance_id: &str,
        plate_index: u32,
        plate_revision: &str,
    ) -> Result<PlateList, DriverError>;
    /// Assigns an object to a filament slot (0-based `filament_index`).
    /// Takes an undo snapshot.
    async fn filament_assign(
        &self,
        object_id: &str,
        filament_index: u32,
    ) -> Result<FilamentAssignment, DriverError>;
    /// Starts the native arrange job (a UI job; poll it with `job_get`).
    /// Takes an undo snapshot.
    async fn arrange_start(&self) -> Result<DriverJob, DriverError>;
    /// Read-only listing of the printers Studio knows about.
    async fn device_list(&self) -> Result<Vec<DeviceSummary>, DriverError>;
    /// Read-only AMS inventory of one printer; `remaining_percent` is raw.
    async fn ams_inventory(&self, device_id: &str) -> Result<AmsInventory, DriverError>;
    /// Saves the project to `path` (absolute, UTF-8). `overwrite` must be
    /// `false` unless the caller explicitly asked. Takes no undo snapshot.
    async fn project_save(&self, path: &Path, overwrite: bool) -> Result<(), DriverError>;
    /// Exports the sliced 3MF of the selected plate only to `path`.
    /// Takes no undo snapshot.
    async fn export_sliced_3mf(
        &self,
        plate_index: u32,
        plate_revision: &str,
        slice_result_id: &str,
        path: &Path,
        overwrite: bool,
    ) -> Result<ExportedPlate, DriverError>;
}

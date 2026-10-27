// SPDX-License-Identifier: AGPL-3.0-or-later
//! Abstraction over the in-process MCP driver of the patched Bambu Studio.
//!
//! [`DriverClient`] is the only surface the service layers see; the HTTP
//! implementation and the in-memory `MockDriver` both implement it.

pub mod map;
#[cfg(feature = "mock")]
pub mod mock;
pub mod tool;
pub mod types;

use std::collections::{BTreeMap, BTreeSet};

pub use tool::DriverTool;
pub use types::*;

#[async_trait::async_trait]
pub trait DriverClient: Send + Sync {
    async fn app_state(&self) -> Result<AppState, DriverError>;
    async fn capabilities(&self) -> Result<DriverCapabilities, DriverError>;
    async fn plate_list(&self) -> Result<PlateList, DriverError>;
    async fn plate_select(&self, plate_index: u32, plate_revision: &str)
    -> Result<(), DriverError>;
    async fn model_list(&self) -> Result<Vec<ModelObject>, DriverError>;
    async fn active_presets(&self) -> Result<ActivePresets, DriverError>;
    async fn writable_keys(&self) -> Result<BTreeSet<String>, DriverError>;
    async fn settings_effective(
        &self,
        target: &SettingsTarget,
        keys: Option<&[String]>,
    ) -> Result<Vec<EffectiveSetting>, DriverError>;
    async fn settings_update(&self, values: &BTreeMap<String, String>) -> Result<(), DriverError>;
    async fn settings_override(
        &self,
        target: &SettingsTarget,
        values: &BTreeMap<String, String>,
    ) -> Result<(), DriverError>;
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
}

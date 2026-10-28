// SPDX-License-Identifier: AGPL-3.0-or-later
//! The MCP tools. This module holds the shared state; the read tools are in
//! `read`.

pub mod args;
mod mutate;
mod read;
mod slice;

use std::sync::Arc;
use std::time::Duration;

use crate::catalog::Catalog;
use crate::detect::{ProcessProbe, StatusProbe};
use crate::driver::{DriverClient, DriverError};
use crate::errors::{ToolError, ToolErrorCode};
use crate::revision::RevisionState;

pub use args::{
    Change, GetSettingsArgs, JobArgs, SearchArgs, SetSettingsArgs, SliceArgs, SummaryArgs,
    SupportKind, SupportSettings, SupportStyle, SupportTarget, SupportsArgs, TargetRef, Transform,
    TransformArgs, TransformMode,
};
pub use mutate::compose_rotation_deg;

#[derive(Debug, Clone)]
pub struct ToolConfig {
    /// Lets writes proceed on a Studio build that is not the pinned one.
    pub allow_version_mismatch: bool,
    /// Revision polling period.
    pub poll_every: Duration,
    /// Time without client requests after which polling stops.
    pub idle_after: Duration,
    /// Whether a driver token is configured (from the service configuration,
    /// Task 13); drives the `driver_token_missing` status.
    pub token_present: bool,
}

pub struct Tools {
    pub(crate) driver: Arc<dyn DriverClient>,
    pub(crate) catalog: &'static Catalog,
    pub(crate) probe: Arc<StatusProbe>,
    pub(crate) revision: Arc<RevisionState>,
    pub(crate) cfg: ToolConfig,
    /// Serializes mutations (revision check + write + refresh).
    pub(crate) write_lock: tokio::sync::Mutex<()>,
    /// Slice jobs started through this service: job id -> plate index.
    pub(crate) jobs: std::sync::Mutex<std::collections::HashMap<String, u32>>,
    /// Directory the driver writes temporary G-code exports to (see `slice`).
    pub(crate) export_dir: std::path::PathBuf,
}

impl Tools {
    /// The 12 tool names, in the order of the design (section 5).
    pub const NAMES: [&'static str; 12] = [
        "bamburino_capabilities",
        "studio_get_state",
        "studio_get_settings",
        "studio_set_settings",
        "studio_list_models",
        "studio_transform_model",
        "studio_set_supports",
        "studio_slice",
        "studio_slice_status",
        "studio_cancel_slice",
        "studio_get_slice_summary",
        "catalog_search",
    ];

    /// `export_dir`: where the driver may write temporary G-code exports. It
    /// must be writable by the interactive Bambu Studio user and readable and
    /// deletable by the service; production: `%ProgramData%\Bamburino\tmp`.
    pub fn new(
        driver: Arc<dyn DriverClient>,
        process: Arc<dyn ProcessProbe>,
        cfg: ToolConfig,
        export_dir: std::path::PathBuf,
    ) -> Self {
        // Task 13: `token_present` comes from the configuration (it was
        // hard-coded to `true` before the config existed).
        let probe = Arc::new(StatusProbe::new(driver.clone(), process, cfg.token_present));
        Self {
            driver,
            catalog: Catalog::embedded(),
            probe,
            revision: Arc::new(RevisionState::new()),
            cfg,
            write_lock: tokio::sync::Mutex::new(()),
            jobs: std::sync::Mutex::new(std::collections::HashMap::new()),
            export_dir,
        }
    }

    /// Revision state, so the server can start the poller on it.
    pub fn revision_state(&self) -> &Arc<RevisionState> {
        &self.revision
    }

    pub fn config(&self) -> &ToolConfig {
        &self.cfg
    }
}

/// Maps a driver failure that happened after Studio was found ready.
pub(crate) fn driver_error(e: DriverError) -> ToolError {
    match e {
        DriverError::Busy { message } => {
            tracing::warn!("driver busy");
            ToolError::new(ToolErrorCode::Busy, message)
        }
        DriverError::Unauthorized => ToolError::new(
            ToolErrorCode::DriverUnauthorized,
            "Run: bamburino.exe config set-driver-token",
        ),
        other @ DriverError::Timeout { after_ms } => {
            tracing::warn!(after_ms, "driver read timed out");
            ToolError::new(ToolErrorCode::DriverError, other.to_string())
        }
        other => ToolError::new(ToolErrorCode::DriverError, other.to_string()),
    }
}

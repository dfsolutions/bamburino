// SPDX-License-Identifier: AGPL-3.0-or-later
//! The MCP tools. This module holds the shared state; the read tools are in
//! `read`.

pub mod args;
mod filaments;
mod mutate;
mod plates;
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
    AssignFilamentArgs, Change, GetSettingsArgs, JobArgs, ListFilamentsArgs, MoveToPlateArgs,
    PlateCreateArgs, PlateDeleteArgs, PlateDeleteContents, PlateRenameArgs, SearchArgs,
    SetSettingsArgs, SliceArgs, SummaryArgs, SupportKind, SupportSettings, SupportStyle,
    SupportTarget, SupportsArgs, TargetRef, Transform, TransformArgs, TransformMode,
};
pub use filaments::{normalize_ams_colour, parse_project_colours};
pub use mutate::compose_rotation_deg;

#[derive(Debug, Clone)]
pub struct ToolConfig {
    /// Lets writes proceed on a Studio build that is not the pinned one.
    pub allow_version_mismatch: bool,
    /// Revision polling period.
    pub poll_every: Duration,
    /// Time without client requests after which polling stops.
    pub idle_after: Duration,
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
    /// Directory allowlist for the tools that will take paths (none does in
    /// the MVP); set once by the service at startup.
    path_guard: std::sync::OnceLock<crate::pathguard::PathGuard>,
    /// Poll period of the job-wait loops (plates, arrange, export).
    pub(crate) job_poll_every: Duration,
    /// Upper bound of a job-wait loop.
    pub(crate) job_timeout: Duration,
    /// Set by the first `start_poller` call.
    poller_started: std::sync::atomic::AtomicBool,
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
        // The driver client decides whether a token is configured
        // (`DriverClient::token_configured`, which follows `driver.token`
        // live), so the probe is not given a frozen flag.
        let probe = Arc::new(StatusProbe::new(driver.clone(), process, true));
        Self {
            driver,
            catalog: Catalog::embedded(),
            probe,
            revision: Arc::new(RevisionState::new()),
            cfg,
            write_lock: tokio::sync::Mutex::new(()),
            jobs: std::sync::Mutex::new(std::collections::HashMap::new()),
            export_dir,
            path_guard: std::sync::OnceLock::new(),
            job_poll_every: Duration::from_millis(500),
            job_timeout: Duration::from_secs(120),
            poller_started: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Overrides the job-wait settings (defaults 500 ms / 120 s); tests use
    /// short values.
    pub fn with_job_wait(mut self, poll_every: Duration, timeout: Duration) -> Self {
        self.job_poll_every = poll_every;
        self.job_timeout = timeout;
        self
    }

    /// Installs the path allowlist (first call wins).
    pub fn set_path_guard(&self, guard: crate::pathguard::PathGuard) {
        let _ = self.path_guard.set(guard);
    }

    /// The path allowlist, if the service installed one. Every tool that
    /// accepts a path must go through `PathGuard::check`.
    pub fn path_guard(&self) -> Option<&crate::pathguard::PathGuard> {
        self.path_guard.get()
    }

    /// Revision state, so the server can start the poller on it.
    pub fn revision_state(&self) -> &Arc<RevisionState> {
        &self.revision
    }

    /// Starts the revision poller on this tool set's driver; it polls only
    /// while clients are active. Stop it with `revision_state().stop_poller()`.
    /// Idempotent: only the first call starts a poller; later calls return
    /// `None`, so a second caller cannot leak one.
    pub fn start_poller(&self) -> Option<tokio::task::JoinHandle<()>> {
        if self
            .poller_started
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return None;
        }
        Some(self.revision.clone().spawn_poller(
            self.driver.clone(),
            self.cfg.poll_every,
            self.cfg.idle_after,
        ))
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
        // A modal dialog blocks the GUI thread the driver dispatches on: a
        // timed-out read is BUSY (design 9, step 10), not a driver fault.
        DriverError::Timeout { after_ms } => {
            tracing::warn!(after_ms, "driver read timed out");
            ToolError::new(
                ToolErrorCode::Busy,
                crate::detect::timeout_message(after_ms),
            )
            .with_details(serde_json::json!({ "reason": "driver_timeout" }))
        }
        other => ToolError::new(ToolErrorCode::DriverError, other.to_string()),
    }
}
